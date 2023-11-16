use std::{
    io,
    io::IsTerminal,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Once,
    },
    time::{Duration, SystemTime},
};

use axum::{
    extract::{ws::Message, Query, State, WebSocketUpgrade},
    http::{header, HeaderValue},
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::{task, time};
use tower_http::{
    cors::CorsLayer,
    set_header::SetResponseHeaderLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::card::*;

static WS_COUNTER: AtomicU64 = AtomicU64::new(0);

static RETRY_COUNT: usize = 2;
static RETRY_INTERVAL: Duration = Duration::from_millis(1000);

static mut VERSION: String = String::new();

#[derive(Debug, Clone)]
pub struct AppState {
    pub default_card_fetch_interval: u64,
}

#[derive(Deserialize)]
struct WSQuery {
    interval: Option<u64>,
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(WSQuery {
        interval,
    }): Query<WSQuery>,
) -> impl IntoResponse {
    let card_fetch_interval =
        Arc::new(AtomicU64::new(interval.unwrap_or(state.default_card_fetch_interval)));

    ws.on_upgrade(|socket| async move {
        let id = WS_COUNTER.fetch_add(1, Ordering::Relaxed);

        tracing::info!(target: "websocket", id, "連線建立");

        let card_fetch_interval_a = card_fetch_interval.clone();

        let (mut sender, mut receiver) = socket.split();

        let t_m = task::spawn(async move {
            loop {
                let t = SystemTime::now();

                let json_string =
                    fetch_nhi_cards_json_string().await.unwrap_or_else(|_| String::from("[]"));

                if let Err(error) = sender.send(Message::Text(json_string.clone())).await {
                    let mut success = false;

                    for _ in 0..RETRY_COUNT {
                        time::sleep(RETRY_INTERVAL).await;

                        if sender.send(Message::Text(json_string.clone())).await.is_ok() {
                            success = true;

                            break;
                        }
                    }

                    if !success {
                        tracing::info!(target: "websocket", id, ?error);

                        let _ = sender.close().await;

                        break;
                    }
                }

                let d = t.elapsed().unwrap();

                let card_fetch_interval =
                    Duration::from_millis(card_fetch_interval.load(Ordering::Relaxed));

                if d < card_fetch_interval {
                    time::sleep(card_fetch_interval - d).await;
                }
            }
        });

        while let Some(message) = receiver.next().await {
            tracing::debug!(target: "websocket", id, ?message);

            match message {
                Ok(message) => match message {
                    Message::Close(reason) => {
                        if let Some(reason) = reason {
                            tracing::info!(target: "websocket", id, ?reason);
                        }

                        break;
                    },
                    Message::Text(s) => {
                        if s.eq_ignore_ascii_case("close") {
                            break;
                        } else if let Ok(millisecond) = s.parse::<u64>() {
                            card_fetch_interval_a.store(millisecond, Ordering::Relaxed);
                        }
                    },
                    _ => (),
                },
                Err(error) => {
                    tracing::info!(target: "websocket", id, ?error);

                    break;
                },
            }
        }

        t_m.abort();

        tracing::info!(target: "websocket", id, "連線結束");
    })
}

pub async fn index_handler() -> impl IntoResponse {
    let json_string = fetch_nhi_cards_json_string().await.unwrap_or_else(|_| String::from("[]"));

    ([(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))], json_string)
}

pub async fn version_handler() -> impl IntoResponse {
    static START: Once = Once::new();

    START.call_once(|| unsafe {
        VERSION = json!({
            "text": env!("CARGO_PKG_VERSION"),
            "major": env!("CARGO_PKG_VERSION_MAJOR").parse::<u32>().unwrap(),
            "minor": env!("CARGO_PKG_VERSION_MINOR").parse::<u32>().unwrap(),
            "patch": env!("CARGO_PKG_VERSION_PATCH").parse::<u32>().unwrap(),
            "pre": env!("CARGO_PKG_VERSION_PRE"),
        })
        .to_string();
    });

    ([(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))], unsafe {
        VERSION.as_str()
    })
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
pub async fn server_main(socket_addr: SocketAddr, state: AppState) -> anyhow::Result<()> {
    let mut ansi_color = io::stdout().is_terminal();

    if ansi_color && enable_ansi_support::enable_ansi_support().is_err() {
        ansi_color = false;
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(ansi_color))
        .with(EnvFilter::builder().with_default_directive(Level::INFO.into()).from_env_lossy())
        .init();

    let app = create_app(state);

    tracing::info!("listening on http://{socket_addr}");

    axum::Server::bind(&socket_addr).serve(app.into_make_service()).await?;

    Ok(())
}
