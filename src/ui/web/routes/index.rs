use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};

use crate::ui::web::templates;

pub(crate) async fn index() -> impl IntoResponse {
    let mut resp = Html(templates::INDEX_HTML).into_response();
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    resp.headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    resp.headers_mut()
        .insert(header::EXPIRES, HeaderValue::from_static("0"));
    resp
}

pub(crate) async fn asset_css() -> Response {
    let mut resp = Response::new(templates::APP_CSS.into());
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    resp.headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    resp.headers_mut()
        .insert(header::EXPIRES, HeaderValue::from_static("0"));
    resp
}

pub(crate) async fn asset_js() -> Response {
    let mut resp = Response::new(templates::APP_JS.into());
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/javascript; charset=utf-8"),
    );
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    resp.headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    resp.headers_mut()
        .insert(header::EXPIRES, HeaderValue::from_static("0"));
    resp
}

pub(crate) async fn asset_favicon_ico() -> Response {
    let mut resp = Response::new(templates::APP_FAVICON_ICO.into());
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("image/x-icon"),
    );
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    resp.headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    resp.headers_mut()
        .insert(header::EXPIRES, HeaderValue::from_static("0"));
    resp
}
