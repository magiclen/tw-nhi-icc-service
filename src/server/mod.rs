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
use tokio::{sync::mpsc, task, time};
use tower_http::{
    cors::CorsLayer,
    set_header::SetResponseHeaderLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::card::*;

static WS_COUNTER: AtomicU64 = AtomicU64::new(0);

static mut VERSION: String = String::new();

static PING_INTERVAL: Duration = Duration::from_secs(20);

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

        let (sender_ctrl, mut receiver_ctrl) = mpsc::channel::<()>(1);

        let t_sender = task::spawn(async move {
            'outer: loop {
                let t = SystemTime::now();

                let json_string =
                    fetch_nhi_cards_json_string().await.unwrap_or_else(|_| String::from("[]"));

                tracing::debug!(target: "websocket", id, "send {json_string:?}");

                if let Err(error) = sender.send(Message::Text(json_string)).await {
                    tracing::info!(target: "websocket", id, ?error);

                    sender_ctrl.send(()).await.unwrap();

                    break;
                }

                // wait and ping
                loop {
                    let d = t.elapsed().unwrap();

                    let card_fetch_interval =
                        Duration::from_secs(card_fetch_interval.load(Ordering::Relaxed));

                    if d >= card_fetch_interval {
                        break;
                    }

                    let sleep_interval = card_fetch_interval - d;

                    if sleep_interval <= PING_INTERVAL {
                        time::sleep(sleep_interval).await;

                        break;
                    } else {
                        time::sleep(PING_INTERVAL).await;

                        tracing::debug!(target: "websocket", id, "send ping");

                        if let Err(error) = sender.send(Message::Ping(vec![1, 2, 3])).await {
                            tracing::info!(target: "websocket", id, ?error);

                            sender_ctrl.send(()).await.unwrap();

                            break 'outer;
                        }
                    }
                }
            }
        });

        loop {
            tokio::select! {
                _ = receiver_ctrl.recv() => break,
                message = receiver.next() => {
                    if let Some(message) = message {
                        tracing::debug!(target: "websocket", id, ?message, "receive");

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
                                    } else if let Ok(seconds) = s.parse::<u64>() {
                                        card_fetch_interval_a.store(seconds, Ordering::Relaxed);
                                    }
                                },
                                _ => (),
                            },
                            Err(error) => {
                                tracing::info!(target: "websocket", id, ?error);

                                break;
                            },
                        }
                    } else {
                        break;
                    }
                }
            }
        }

        t_sender.abort();

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
