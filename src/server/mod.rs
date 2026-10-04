use std::{
    io,
    io::IsTerminal,
    net::SocketAddr,
    sync::{
        LazyLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::Bytes,
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, Utf8Bytes, WebSocket, close_code},
    },
    http::{HeaderValue, header},
    response::IntoResponse,
    routing::get,
};
use serde::Deserialize;
use serde_json::json;
use tokio::time::{self, Instant};
use tower_http::{
    cors::CorsLayer,
    set_header::SetResponseHeaderLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::card::*;

static WS_COUNTER: AtomicU64 = AtomicU64::new(0);

static VERSION: LazyLock<String> = LazyLock::new(|| {
    json!({
        "text": env!("CARGO_PKG_VERSION"),
        "major": env!("CARGO_PKG_VERSION_MAJOR").parse::<u32>().unwrap(),
        "minor": env!("CARGO_PKG_VERSION_MINOR").parse::<u32>().unwrap(),
        "patch": env!("CARGO_PKG_VERSION_PATCH").parse::<u32>().unwrap(),
        "pre": env!("CARGO_PKG_VERSION_PRE"),
    })
    .to_string()
});

/// WebSocket 送出 ping 的間隔。
const PING_INTERVAL: Duration = Duration::from_secs(25);
/// 超過這段時間沒有收到客戶端的任何 frame 就斷線。
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(35);
/// 送出一則 WebSocket 訊息的最長時間。
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// 等待第一次讀卡掃描完成的最長時間。
const FIRST_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);

/// WebSocket 重送讀卡狀態的最短間隔（秒）。
pub const MIN_WS_INTERVAL: u64 = 1;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub socket_addr:         SocketAddr,
    /// WebSocket 在讀卡狀態沒有變化時，重送目前狀態的預設間隔（秒）。
    pub default_ws_interval: u64,
}

#[derive(Debug, Clone)]
struct AppState {
    snapshot:            SnapshotReceiver,
    default_ws_interval: u64,
}

#[derive(Deserialize)]
struct WSQuery {
    interval: Option<u64>,
}

/// 取得最新讀卡狀態的 JSON。第一次掃描還沒完成時會先等待，逾時則視為 PC/SC 服務無法使用。
async fn snapshot_json(state: &AppState) -> String {
    let mut receiver = state.snapshot.clone();

    if let Ok(Ok(snapshot)) =
        time::timeout(FIRST_SNAPSHOT_TIMEOUT, receiver.wait_for(Option::is_some)).await
        && let Some(snapshot) = snapshot.as_deref()
    {
        return snapshot.json().to_owned();
    }

    Snapshot::new(SnapshotStatus::PcscUnavailable(pcsc::Error::Timeout)).json().to_owned()
}

/// 送出 WebSocket 訊息。失敗或逾時回傳 `false`。
async fn send(socket: &mut WebSocket, id: u64, message: Message) -> bool {
    match time::timeout(SEND_TIMEOUT, socket.send(message)).await {
        Ok(Ok(())) => true,
        Ok(Err(error)) => {
            tracing::info!(target: "websocket", id, ?error);

            false
        },
        Err(_) => {
            tracing::info!(target: "websocket", id, "送出訊息逾時");

            false
        },
    }
}

/// 送出 Close frame。
async fn send_close(socket: &mut WebSocket, id: u64, code: u16, reason: &'static str) {
    let frame = CloseFrame {
        code,
        reason: Utf8Bytes::from_static(reason),
    };

    send(socket, id, Message::Close(Some(frame))).await;
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(WSQuery {
        interval,
    }): Query<WSQuery>,
) -> impl IntoResponse {
    let interval = interval.unwrap_or(state.default_ws_interval).max(MIN_WS_INTERVAL);

    ws.on_upgrade(move |socket| handle_socket(socket, state, interval))
}

async fn handle_socket(mut socket: WebSocket, state: AppState, interval: u64) {
    let id = WS_COUNTER.fetch_add(1, Ordering::Relaxed);

    tracing::info!(target: "websocket", id, "連線建立");

    let mut interval = Duration::from_secs(interval);

    let mut snapshot = state.snapshot.clone();

    // 讓迴圈一開始就送出目前的讀卡狀態
    snapshot.mark_changed();

    let mut last_sent = Instant::now();
    let mut last_received = Instant::now();

    // 讀卡狀態沒有變化時，每隔 `interval` 重送一次，讓瀏覽器端可以判斷連線是否還活著
    let heartbeat = time::sleep(interval);
    tokio::pin!(heartbeat);

    let mut ping = time::interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);

    loop {
        tokio::select! {
            result = snapshot.changed() => {
                if result.is_err() {
                    break;
                }

                let message = snapshot.borrow_and_update().as_deref().map(|snapshot| Message::Text(snapshot.json().into()));

                if let Some(message) = message {
                    tracing::debug!(target: "websocket", id, "send snapshot");

                    if !send(&mut socket, id, message).await {
                        break;
                    }

                    last_sent = Instant::now();
                    heartbeat.as_mut().reset(last_sent + interval);
                }
            },
            () = &mut heartbeat => {
                let message = snapshot.borrow().as_deref().map(|snapshot| Message::Text(snapshot.json().into()));

                if let Some(message) = message {
                    tracing::debug!(target: "websocket", id, "send snapshot (heartbeat)");

                    if !send(&mut socket, id, message).await {
                        break;
                    }

                    last_sent = Instant::now();
                }

                heartbeat.as_mut().reset(Instant::now() + interval);
            },
            _ = ping.tick() => {
                if last_received.elapsed() > RECEIVE_TIMEOUT {
                    tracing::info!(target: "websocket", id, "客戶端沒有回應");

                    break;
                }

                tracing::debug!(target: "websocket", id, "send ping");

                if !send(&mut socket, id, Message::Ping(Bytes::new())).await {
                    break;
                }
            },
            message = socket.recv() => {
                let Some(message) = message else {
                    break;
                };

                tracing::debug!(target: "websocket", id, ?message, "receive");

                match message {
                    Ok(message) => {
                        last_received = Instant::now();

                        match message {
                            Message::Close(frame) => {
                                if let Some(frame) = frame {
                                    tracing::info!(target: "websocket", id, ?frame);
                                }

                                break;
                            },
                            Message::Text(s) => {
                                if s.eq_ignore_ascii_case("close") {
                                    send_close(&mut socket, id, close_code::NORMAL, "").await;

                                    break;
                                } else if let Ok(seconds) = s.parse::<u64>() {
                                    interval = Duration::from_secs(seconds.max(MIN_WS_INTERVAL));
                                    heartbeat.as_mut().reset(last_sent + interval);
                                }
                            },
                            _ => (),
                        }
                    },
                    Err(error) => {
                        tracing::info!(target: "websocket", id, ?error);

                        break;
                    },
                }
            },
        }
    }

    tracing::info!(target: "websocket", id, "連線結束");
}

async fn index_handler(State(state): State<AppState>) -> impl IntoResponse {
    let json_string = snapshot_json(&state).await;

    ([(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))], json_string)
}

async fn version_handler() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))], VERSION.as_str())
}

fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .route("/version", get(version_handler))
        .layer(CorsLayer::permissive())
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(state)
}

#[inline]
pub async fn server_main(config: ServerConfig) -> anyhow::Result<()> {
    let ServerConfig {
        socket_addr,
        default_ws_interval,
    } = config;

    let mut ansi_color = io::stdout().is_terminal();

    if ansi_color && enable_ansi_support::enable_ansi_support().is_err() {
        ansi_color = false;
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(ansi_color))
        .with(EnvFilter::builder().with_default_directive(Level::INFO.into()).from_env_lossy())
        .init();

    let state = AppState {
        snapshot: spawn_card_monitor()?,
        default_ws_interval,
    };

    let app = create_app(state);

    let listener = tokio::net::TcpListener::bind(socket_addr).await?;
    tracing::info!("listening on http://{socket_addr}");
    axum::serve(listener, app).await?;

    Ok(())
}
