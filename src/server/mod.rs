use std::{io, io::IsTerminal, net::SocketAddr};

use axum::{
    http::{header, HeaderValue},
    routing::get,
    Json, Router,
};
use tower_http::{
    cors::CorsLayer,
    set_header::SetResponseHeaderLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::card::{pcsc_ctx, read_nhi_cards, NHICardBasic};

pub async fn index_handler() -> Json<Vec<NHICardBasic>> {
    let pcsc_ctx = match pcsc_ctx() {
        Ok(pcsc_ctx) => pcsc_ctx,
        Err(err) => {
            tracing::warn!("找不到 PC/SC 服務，請確認讀卡機有連接上並安裝了正確的驅動程式：{err}");
            return Json(Vec::new());
        },
    };

    let result = read_nhi_cards(&pcsc_ctx).unwrap();

    Json(result)
}

pub async fn version_handler() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn create_app() -> Router {
    Router::new()
        .route("/", get(index_handler))
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
