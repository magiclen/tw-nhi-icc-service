mod docs;

use std::{
    future, io,
    io::IsTerminal,
    net::{IpAddr, SocketAddr},
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
        Query, Request, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, Utf8Bytes, WebSocket, close_code},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header, uri::Authority},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
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
use utoipa::{IntoParams, ToSchema};

use crate::card::*;

static WS_COUNTER: AtomicU64 = AtomicU64::new(0);

static VERSION: LazyLock<String> = LazyLock::new(|| {
    let version = Version {
        text:  env!("CARGO_PKG_VERSION"),
        major: env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap(),
        minor: env!("CARGO_PKG_VERSION_MINOR").parse().unwrap(),
        patch: env!("CARGO_PKG_VERSION_PATCH").parse().unwrap(),
        pre:   env!("CARGO_PKG_VERSION_PRE"),
    };

    serde_json::to_string(&version).unwrap()
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

/// 客戶端只會送出 `close` 或秒數，所以接收的 WebSocket 訊息不需要太大。
const MAX_WS_MESSAGE_SIZE: usize = 1024;

/// WebSocket 重送讀卡狀態的最短間隔（秒）。
pub const MIN_WS_INTERVAL: u64 = 1;
/// WebSocket 重送讀卡狀態的最長間隔（秒）。
pub const MAX_WS_INTERVAL: u64 = 86400;

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

/// 服務的版本。
#[derive(Serialize, ToSchema)]
struct Version {
    /// 完整的版本字串。
    #[schema(value_type = String, examples("0.3.2"))]
    text:  &'static str,
    #[schema(examples(0))]
    major: u32,
    #[schema(examples(3))]
    minor: u32,
    #[schema(examples(2))]
    patch: u32,
    /// 預發布版本的標籤，沒有時為空字串。
    #[schema(value_type = String, examples(""))]
    pre:   &'static str,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct WSQuery {
    /// 讀卡狀態沒有變化時，重送目前狀態的間隔（秒）。小於 1 時視為 1，大於 86400 時視為 86400；沒有指定時使用 `--default-ws-card-fetch-interval` 的值（預設為 3）。
    #[param(minimum = 1, maximum = 86400, example = 3)]
    interval: Option<u64>,
}

/// 將 WebSocket 重送讀卡狀態的間隔限制在合理的範圍內。
/// 太大的值會讓計算 heartbeat 時間時溢位而 panic。
#[inline]
fn ws_interval(seconds: u64) -> Duration {
    Duration::from_secs(seconds.clamp(MIN_WS_INTERVAL, MAX_WS_INTERVAL))
}

/// 檢查 `Host` 是否為 IP 或 `localhost`。沒有 `Host` 的請求（非瀏覽器客戶端）一律允許。
fn is_host_allowed(host: Option<&HeaderValue>) -> bool {
    let Some(host) = host else {
        return true;
    };

    let Ok(authority) = Authority::try_from(host.as_bytes()) else {
        return false;
    };

    let host = authority.host();

    host.eq_ignore_ascii_case("localhost")
        || host.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>().is_ok()
}

/// 防止 DNS rebinding。
/// 攻擊者的網域改為解析到本機後，瀏覽器會把請求當成同源而不送出 `Origin`，所以要另外限制 `Host`。
async fn check_host(request: Request, next: Next) -> Response {
    let host = request.headers().get(header::HOST);

    if !is_host_allowed(host) {
        tracing::warn!(?host, "host not allowed");

        return StatusCode::FORBIDDEN.into_response();
    }

    next.run(request).await
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

/// 以 WebSocket 接收讀卡狀態
///
/// 這個端點必須以 WebSocket 連線，無法用 Swagger UI 的 Try it out 測試。
///
/// 伺服器會以文字訊息送出 `Snapshot` 格式的 JSON（與 `GET /` 的回應相同），送出的時機如下：
///
/// - 連線建立後立即送出一次。
/// - 讀卡狀態改變（例如插拔卡片、接上或移除讀卡機）時立即送出。
/// - 超過 `interval` 秒都沒有送出任何訊息時，重送目前的狀態。客戶端可以據此判斷連線是否還活著，例如超過兩倍的 `interval` 都沒有收到訊息時就重新連線。
///
/// 客戶端可以送出以下文字訊息：
///
/// - 秒數（例如 `5`）：變更 `interval`，範圍與查詢中的 `interval` 相同。
/// - `close`：關閉連線，伺服器會回應代碼為 `1000` 的 Close frame。
///
/// 服務關閉時，伺服器會送出代碼為 `1001` 的 Close frame。伺服器每 25 秒會送出一次 Ping，超過 35 秒都沒有收到客戶端的任何 frame（包含 Pong）時，會在下一次送出 Ping 時中斷連線。
#[utoipa::path(
    get,
    path = "/ws",
    tag = "讀卡",
    params(WSQuery),
    responses(
        (status = 101, description = "切換為 WebSocket 協定"),
        (status = 400, description = "不是 WebSocket 升級請求，或 `interval` 不是正整數"),
        (status = 403, description = "請求的 `Origin` 不在 `--allow-origin` 白名單中"),
    ),
)]
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(WSQuery {
        interval,
    }): Query<WSQuery>,
    headers: HeaderMap,
) -> Response {
    let origin = headers.get(header::ORIGIN);

    // CORS 管不到 WebSocket，所以要自己檢查來源
    if !state.is_origin_allowed(origin) {
        tracing::warn!(target: "websocket", ?origin, "origin not allowed");

        return StatusCode::FORBIDDEN.into_response();
    }

    let interval = ws_interval(interval.unwrap_or(state.default_ws_interval));

    ws.max_frame_size(MAX_WS_MESSAGE_SIZE)
        .max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(move |socket| handle_socket(socket, state, interval))
}

async fn handle_socket(mut socket: WebSocket, state: AppState, mut interval: Duration) {
    let _guard = WSConnectionGuard::new(&state.ws_connections);

    let id = WS_COUNTER.fetch_add(1, Ordering::Relaxed);

    tracing::info!(target: "websocket", id, "連線建立");

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
                                    interval = ws_interval(seconds);
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

/// 取得所有讀卡機目前的狀態
///
/// 服務會在背景監控所有讀卡機，在插入卡片時讀取並快取，所以這個端點會立即回應。有些讀卡機的驅動程式會漏掉插拔卡事件，所以卡片插著時，服務每 3 秒會重新讀卡，確認卡片沒有被拔出或更換。
///
/// 一律回傳 `200`，服務本身的狀態請看 `status` 欄位。服務剛啟動、第一次掃描還沒完成時，最多會等待 5 秒；逾時的話，`status` 為 `pcsc_unavailable`，`error` 為 `Timeout`。
#[utoipa::path(
    get,
    path = "/",
    tag = "讀卡",
    responses(
        (status = 200, description = "所有讀卡機目前的狀態", body = SnapshotJSON, examples(
            ("nhi_card" = (summary = "一台讀卡機讀到健保卡，另一台沒有插卡", value = json!({
                "type": "snapshot",
                "status": "ok",
                "error": null,
                "readers": [
                    {
                        "name": "ACS ACR39U ICC Reader 00 00",
                        "state": "nhi_card",
                        "card": {
                            "card_no": "000012345678",
                            "full_name": "王小明",
                            "id_no": "A123456789",
                            "birth_date": "1990-01-01",
                            "birth_date_timestamp": 631123200000i64,
                            "sex": "M",
                            "issue_date": "2020-01-01",
                            "issue_date_timestamp": 1577808000000i64
                        },
                        "error": null
                    },
                    {
                        "name": "ACS ACR39U ICC Reader 01 00",
                        "state": "empty",
                        "card": null,
                        "error": null
                    }
                ]
            }))),
            ("no_readers" = (summary = "PC/SC 服務可以使用，但沒有接任何讀卡機", value = json!({
                "type": "snapshot",
                "status": "ok",
                "error": null,
                "readers": []
            }))),
            ("pcsc_unavailable" = (summary = "PC/SC 服務無法使用", value = json!({
                "type": "snapshot",
                "status": "pcsc_unavailable",
                "error": "NoService",
                "readers": []
            }))),
        )),
    ),
)]
async fn index_handler(State(state): State<AppState>) -> impl IntoResponse {
    let json_string = snapshot_json(&state).await;

    ([(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))], json_string)
}

/// 取得服務的版本
///
/// 可用來檢查服務是否正在執行。客戶端可以用 `major` 與 `minor` 判斷伺服器的 API 是否相容。
#[utoipa::path(
    get,
    path = "/version",
    tag = "服務",
    responses(
        (status = 200, description = "服務的版本", body = Version),
    ),
)]
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

    let mut router = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .route("/version", get(version_handler))
        .merge(docs::docs_router());

    // 沒有白名單時本來就允許所有來源，不需要防範 DNS rebinding
    if !state.allowed_origins.is_empty() {
        router = router.layer(middleware::from_fn(check_host));
    }

    router
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_allowed() {
        for host in ["127.0.0.1:12345", "127.0.0.1", "localhost:12345", "LocalHost", "[::1]:12345"]
        {
            assert!(is_host_allowed(Some(&HeaderValue::from_static(host))), "{host}");
        }

        assert!(is_host_allowed(None));

        for host in ["evil.example:12345", "evil.example", "localhost.evil.example:12345"] {
            assert!(!is_host_allowed(Some(&HeaderValue::from_static(host))), "{host}");
        }
    }
}
