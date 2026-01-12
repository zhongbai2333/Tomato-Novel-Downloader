use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};

use crate::ui::web::templates;

pub(crate) async fn index() -> impl IntoResponse {
    Html(templates::INDEX_HTML)
}

pub(crate) async fn asset_css() -> Response {
    let mut resp = Response::new(templates::APP_CSS.into());
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
    resp
}

pub(crate) async fn asset_js() -> Response {
    let mut resp = Response::new(templates::APP_JS.into());
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/javascript; charset=utf-8"),
    );
    resp
}
