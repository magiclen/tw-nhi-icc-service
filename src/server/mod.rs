use std::{
    future, io,
    io::IsTerminal,
    net::SocketAddr,
    sync::{
        Arc, LazyLock,
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
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::json;
use tokio::{
    signal,
    sync::watch,
    time::{self, Instant},
};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
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
/// 關閉服務時，等待 WebSocket 連線結束的最長時間。
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// WebSocket 重送讀卡狀態的最短間隔（秒）。
pub const MIN_WS_INTERVAL: u64 = 1;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub socket_addr:         SocketAddr,
    /// WebSocket 在讀卡狀態沒有變化時，重送目前狀態的預設間隔（秒）。
    pub default_ws_interval: u64,
    /// 允許存取此服務的網頁來源。空的代表允許所有來源。
    pub allowed_origins:     Vec<HeaderValue>,
}

#[derive(Debug, Clone)]
struct AppState {
    snapshot:            SnapshotReceiver,
    default_ws_interval: u64,
    allowed_origins:     Arc<[HeaderValue]>,
    shutdown:            watch::Receiver<bool>,
    ws_connections:      Arc<watch::Sender<usize>>,
}

impl AppState {
    /// 檢查 WebSocket 連線的來源。沒有設定白名單，或請求沒有帶 `Origin`（非瀏覽器客戶端）時一律允許。
    fn is_origin_allowed(&self, origin: Option<&HeaderValue>) -> bool {
        match origin {
            Some(origin) if !self.allowed_origins.is_empty() => {
                self.allowed_origins.contains(origin)
            },
            _ => true,
        }
    }
}

/// 計算目前的 WebSocket 連線數，讓關閉服務時可以等待連線送出 Close frame。
struct WSConnectionGuard(Arc<watch::Sender<usize>>);

impl WSConnectionGuard {
    #[inline]
    fn new(counter: &Arc<watch::Sender<usize>>) -> Self {
        counter.send_modify(|count| *count += 1);

        Self(counter.clone())
    }
}

impl Drop for WSConnectionGuard {
    #[inline]
    fn drop(&mut self) {
        self.0.send_modify(|count| *count -= 1);
    }
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
    headers: HeaderMap,
) -> Response {
    // CORS 管不到 WebSocket，所以要自己檢查來源
    if !state.is_origin_allowed(headers.get(header::ORIGIN)) {
        return StatusCode::FORBIDDEN.into_response();
    }

    let interval = interval.unwrap_or(state.default_ws_interval).max(MIN_WS_INTERVAL);

    ws.on_upgrade(move |socket| handle_socket(socket, state, interval))
}

async fn handle_socket(mut socket: WebSocket, state: AppState, interval: u64) {
    let _guard = WSConnectionGuard::new(&state.ws_connections);

    let id = WS_COUNTER.fetch_add(1, Ordering::Relaxed);

    tracing::info!(target: "websocket", id, "連線建立");

    let mut interval = Duration::from_secs(interval);

    let mut snapshot = state.snapshot.clone();
    let mut shutdown = state.shutdown.clone();

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
            _ = shutdown.changed() => {
                send_close(&mut socket, id, close_code::AWAY, "服務關閉").await;

                break;
            },
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
    let cors = if state.allowed_origins.is_empty() {
        CorsLayer::permissive()
    } else {
        CorsLayer::permissive()
            .allow_origin(AllowOrigin::list(state.allowed_origins.iter().cloned()))
    };

    Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .route("/version", get(version_handler))
        .layer(cors)
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

/// 等待 Ctrl+C 或作業系統要求結束程式的訊號。
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = signal::ctrl_c().await {
            tracing::warn!(?error, "cannot listen for Ctrl+C");

            future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            },
            Err(error) => {
                tracing::warn!(?error, "cannot listen for SIGTERM");

                future::pending::<()>().await;
            },
        }
    };

    #[cfg(windows)]
    let terminate = async {
        match signal::windows::ctrl_close() {
            Ok(mut signal) => {
                signal.recv().await;
            },
            Err(error) => {
                tracing::warn!(?error, "cannot listen for CTRL_CLOSE");

                future::pending::<()>().await;
            },
        }
    };

    #[cfg(not(any(unix, windows)))]
    let terminate = future::pending::<()>();

    tokio::select! {
        () = ctrl_c => (),
        () = terminate => (),
    }
}

#[inline]
pub async fn server_main(config: ServerConfig) -> anyhow::Result<()> {
    let ServerConfig {
        socket_addr,
        default_ws_interval,
        allowed_origins,
    } = config;

    let mut ansi_color = io::stdout().is_terminal();

    if ansi_color && enable_ansi_support::enable_ansi_support().is_err() {
        ansi_color = false;
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(ansi_color))
        .with(EnvFilter::builder().with_default_directive(Level::INFO.into()).from_env_lossy())
        .init();

    let (shutdown_sender, shutdown) = watch::channel(false);
    let ws_connections = Arc::new(watch::channel(0).0);

    let state = AppState {
        snapshot: spawn_card_monitor()?,
        default_ws_interval,
        allowed_origins: allowed_origins.into(),
        shutdown,
        ws_connections: ws_connections.clone(),
    };

    let app = create_app(state);

    let listener = tokio::net::TcpListener::bind(socket_addr).await?;
    tracing::info!("listening on http://{socket_addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;

            tracing::info!("shutting down");

            shutdown_sender.send_replace(true);
        })
        .await?;

    // 已升級成 WebSocket 的連線不在 axum 的等待範圍內，要另外等它們送出 Close frame
    let mut ws_connections = ws_connections.subscribe();

    if time::timeout(SHUTDOWN_TIMEOUT, ws_connections.wait_for(|count| *count == 0)).await.is_err()
    {
        tracing::warn!("some WebSocket connections did not close in time");
    }

    Ok(())
}
