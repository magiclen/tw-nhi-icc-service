use std::{
    io,
    io::IsTerminal,
    net::SocketAddr,
    sync::atomic::{AtomicU64, Ordering},
};

use axum::{
    extract::{ws::Message, WebSocketUpgrade},
    http::{header, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use tower_http::{
    cors::CorsLayer,
    set_header::SetResponseHeaderLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::card::*;

static WS_COUNTER: AtomicU64 = AtomicU64::new(0);

async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(|mut socket| async move {
        let id = WS_COUNTER.fetch_add(1, Ordering::Relaxed);

        tracing::info!(target: "websocket", id, "連線建立");

        if socket.send(Message::Ping(vec![1, 2, 3])).await.is_err() {
            return;
        }

        tracing::info!(target: "websocket", id, "連線結束");
    })
}

pub async fn index_handler() -> impl IntoResponse {
    let json_string = fetch_nhi_cards_json_string().await.unwrap_or_else(|_| String::from("[]"));

    (StatusCode::OK, [(header::CONTENT_TYPE, "application/json; charset=utf-8")], json_string)
}

pub async fn version_handler() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn create_app() -> Router {
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
}

#[inline]
pub async fn server_main(socket_addr: SocketAddr) -> anyhow::Result<()> {
    let mut ansi_color = io::stdout().is_terminal();

    if ansi_color && enable_ansi_support::enable_ansi_support().is_err() {
        ansi_color = false;
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(ansi_color))
        .with(EnvFilter::builder().with_default_directive(Level::INFO.into()).from_env_lossy())
        .init();

    let app = create_app();

    tracing::info!("listening on http://{socket_addr}");

    axum::Server::bind(&socket_addr).serve(app.into_make_service()).await?;

    Ok(())
}
