use axum::Router;
use utoipa::OpenApi;
use utoipa_swagger_ui::{Config, SwaggerUi};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "TW NHI IC Card Service",
        description = "透過 HTTP API 讀取中華民國健保卡。\n\n健保卡資料屬於個人資料。若網頁系統的網域是固定的，建議以 `--allow-origin` 啟動服務，限制可以存取此服務的網頁來源。",
    ),
    paths(super::index_handler, super::ws_handler, super::version_handler),
    tags(
        (name = "讀卡", description = "取得讀卡機與健保卡的狀態"),
        (name = "服務", description = "服務本身的資訊"),
    ),
)]
struct ApiDoc;

/// `GET /docs` 為 Swagger UI，`GET /docs/json` 為 OpenAPI 文件。
pub(super) fn docs_router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    let mut openapi = ApiDoc::openapi();

    // utoipa 會自動從 Cargo.toml 帶入聯絡人與授權，這裡不需要
    openapi.info.contact = None;
    openapi.info.license = None;

    // 使用 BaseLayout 來拿掉上方輸入文件網址的列
    let config =
        Config::default().use_base_layout().try_it_out_enabled(true).display_request_duration(true);

    SwaggerUi::new("/docs").url("/docs/json", openapi).config(config).into()
}
