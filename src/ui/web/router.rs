use axum::Router;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};

use sha2::{Digest, Sha256};
use tracing::info;

use super::routes;
use super::state::AppState;

pub(crate) fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/", get(routes::index::index))
        .route("/assets/app.css", get(routes::index::asset_css))
        .route("/assets/app.js", get(routes::index::asset_js))
        .route("/assets/favicon.ico", get(routes::index::asset_favicon_ico))
        .route("/api/login", post(routes::auth::api_login))
        .route("/api/status", get(routes::status::api_status))
        .route("/api/app_update", get(routes::app_update::api_app_update))
        .route(
            "/api/self_update",
            post(routes::app_update::api_self_update),
        )
        .route(
            "/api/config",
            get(routes::auth::get_config).post(routes::auth::set_config),
        )
        .route(
            "/api/config/raw",
            get(routes::auth::get_config_raw).post(routes::auth::set_config_raw),
        )
        .route(
            "/api/config/full",
            get(routes::auth::get_config_full).post(routes::auth::set_config_full),
        )
        .route("/api/library", get(routes::library::api_library))
        .route("/download/*path", get(routes::download::download_file))
        .route("/download-zip/*path", get(routes::download::download_zip))
        .route("/api/search", get(routes::search::api_search))
        .route("/api/preview/:book_id", get(routes::preview::api_preview))
        .route(
            "/api/jobs",
            get(routes::jobs::list_jobs).post(routes::jobs::create_job),
        )
        .route("/api/jobs/:id", delete(routes::jobs::delete_job))
        .route("/api/jobs/:id/cancel", post(routes::jobs::cancel_job))
        .route(
            "/api/jobs/:id/book_name",
            post(routes::jobs::submit_book_name_choice),
        )
        .route("/api/updates", get(routes::updates::api_updates));

    protected
        .layer(from_fn_with_state(state.clone(), auth_and_log_mw))
        .with_state(state)
}

async fn auth_and_log_mw(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    let method = req.method().to_string();
    let ip = req
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|c| c.0)
        .map(|a| a.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // If lock mode enabled, require password for any non-asset route,
    // except the login endpoint and landing page.
    if let Some(auth) = &state.auth {
        let allow = path == "/" || path.starts_with("/assets/") || path == "/api/login";

        if !allow {
            let provided_header = req
                .headers()
                .get("x-tomato-password")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");

            let session_cookie = req
                .headers()
                .get(axum::http::header::COOKIE)
                .and_then(|v| v.to_str().ok())
                .and_then(|raw| {
                    cookie_value(raw, "tomato_session").or_else(|| cookie_value(raw, "auth_token"))
                });

            let mut authorized = session_cookie
                .map(|token| auth.verify_session_token(token))
                .unwrap_or(false);

            // 向后兼容：允许请求头密码（便于脚本/旧客户端）。
            if !authorized && !provided_header.is_empty() {
                let mut h = Sha256::new();
                h.update(provided_header.as_bytes());
                let out = h.finalize();
                authorized = out.as_slice() == auth.password_sha256;
            }

            if !authorized {
                info!(target: "web_access", ip = %ip, method = %method, path = %path, status = 401, "unauthorized");
                return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
            }
        }
    }

    let resp = next.run(req).await;
    info!(target: "web_access", ip = %ip, method = %method, path = %path, status = %resp.status().as_u16(), "ok");
    resp
}

fn cookie_value<'a>(raw_cookie: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    raw_cookie
        .split(';')
        .map(|p| p.trim())
        .find_map(|p| p.strip_prefix(&prefix))
}
