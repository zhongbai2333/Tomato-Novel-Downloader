use axum::Router;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Request, StatusCode, Uri};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};

use sha2::{Digest, Sha256};
use tracing::{info, warn};

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
            get(routes::app_update::api_self_update_status)
                .post(routes::app_update::api_self_update),
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
            "/api/preview/:book_id/cleanup",
            post(routes::preview::api_preview_cleanup),
        )
        .route(
            "/api/preview-cover/:key",
            get(routes::preview::api_preview_cover),
        )
        .route(
            "/api/preview-cover-by-book/:book_id",
            get(routes::preview::api_preview_cover_by_book),
        )
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
        .route(
            "/api/jobs/:id/format",
            post(routes::jobs::submit_format_choice),
        )
        .route("/api/updates", get(routes::updates::api_updates))
        .route("/api/history", get(routes::history::api_history));

    let base_path = state.base_path.clone();
    let app = protected
        .layer(from_fn_with_state(state.clone(), auth_and_log_mw))
        .with_state(state);

    if base_path.as_str().is_empty() {
        app
    } else {
        let root = base_path.url("/");
        let redirect_root = root.clone();
        Router::new()
            .route(
                base_path.as_str(),
                get(move |uri: Uri| {
                    let mut destination = redirect_root.clone();
                    if let Some(query) = uri.query() {
                        destination.push('?');
                        destination.push_str(query);
                    }
                    async move { Redirect::permanent(&destination) }
                }),
            )
            .nest(&root, app)
    }
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
                warn!(
                    target: "web_security",
                    ip = %ip,
                    method = %method,
                    path_len = path.len(),
                    status = 401,
                    "unauthorized"
                );
                return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
            }
        }
    }

    let resp = next.run(req).await;
    info!(
        target: "web_access",
        ip = %ip,
        method = %method,
        path_len = path.len(),
        status = %resp.status().as_u16(),
        "ok"
    );
    resp
}

fn cookie_value<'a>(raw_cookie: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    raw_cookie
        .split(';')
        .map(|p| p.trim())
        .find_map(|p| p.strip_prefix(&prefix))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use reqwest::Client;
    use serde_json::{Value, json};

    use super::*;
    use crate::base_system::context::Config;
    use crate::ui::web::base_path::WebBasePath;
    use crate::ui::web::state::{
        AuthState, ConfigView, JobStore, LibraryScanStore, SelfUpdateStore, UpdateScanStore,
    };

    struct TestServer {
        origin: String,
        client: Client,
        task: tokio::task::JoinHandle<()>,
        _files: tempfile::TempDir,
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn serve(base_path: &str, locked: bool) -> TestServer {
        let files = tempfile::tempdir().unwrap();
        std::fs::create_dir(files.path().join("books")).unwrap();
        std::fs::write(files.path().join("books/sample.txt"), "sample book").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let config = Config::default();
        let state = AppState {
            bind_addrs: Arc::new(vec![addr]),
            base_path: WebBasePath::parse(base_path).unwrap(),
            config_view: Arc::new(ConfigView {
                old_cli: config.old_cli,
                use_official_api: config.use_official_api,
                save_path: config.save_path.clone(),
                api_endpoints_len: config.api_endpoints.len(),
            }),
            config: Arc::new(Mutex::new(config)),
            config_path: Arc::new(files.path().join("config.yml")),
            library_root: Arc::new(files.path().to_path_buf()),
            jobs: Arc::new(JobStore::default()),
            self_update: Arc::new(SelfUpdateStore::default()),
            library_scan: Arc::new(LibraryScanStore::default()),
            update_scan: Arc::new(UpdateScanStore::default()),
            auth: locked.then(|| AuthState::from_password("test-password", false)),
            #[cfg(feature = "official-api")]
            api_semaphore: Arc::new(tokio::sync::Semaphore::new(2)),
        };
        let app = build_router(state);
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        TestServer {
            origin: format!("http://{addr}"),
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            task,
            _files: files,
        }
    }

    #[tokio::test]
    async fn root_and_subpath_serve_assets_apis_and_downloads() {
        for prefix in ["", "/tomato", "/apps/tomato"] {
            let server = serve(prefix, false).await;
            let root = format!("{}{prefix}", server.origin);
            let response = server.client.get(format!("{root}/")).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let html = response.text().await.unwrap();
            assert!(html.contains(&format!("data-base-path=\"{prefix}\"")));
            for asset in ["app.js", "app.css", "favicon.ico"] {
                let path = format!("{prefix}/assets/{asset}");
                assert!(html.contains(&format!("\"{path}\"")));
                let response = server
                    .client
                    .get(format!("{}{path}", server.origin))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK, "{path}");
            }
            let response = server
                .client
                .get(format!("{root}/api/status"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.json::<Value>().await.unwrap()["locked"], false);

            let response = server
                .client
                .get(format!("{root}/download/books/sample.txt"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.text().await.unwrap(), "sample book");
            let response = server
                .client
                .get(format!("{root}/download-zip/books"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let zip = response.bytes().await.unwrap();
            let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
            assert_eq!(archive.by_name("sample.txt").unwrap().size(), 11);

            if !prefix.is_empty() {
                let response = server
                    .client
                    .get(format!("{root}?view=library"))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
                assert_eq!(
                    response.headers()["location"],
                    format!("{prefix}/?view=library")
                );
                for outside in [
                    "/",
                    "/api/status",
                    "/assets/app.js",
                    "/tomato-other/api/status",
                ] {
                    let response = server
                        .client
                        .get(format!("{}{outside}", server.origin))
                        .send()
                        .await
                        .unwrap();
                    assert_eq!(response.status(), StatusCode::NOT_FOUND, "{outside}");
                }
            }
        }
    }

    #[tokio::test]
    async fn subpath_login_cookies_authorize_only_protected_routes() {
        for prefix in ["", "/apps/tomato"] {
            let server = serve(prefix, true).await;
            let root = format!("{}{prefix}", server.origin);
            for path in [
                "/api/status",
                "/api/config",
                "/api/preview-cover/missing",
                "/download/books/sample.txt",
                "/download-zip/books",
            ] {
                let response = server
                    .client
                    .get(format!("{root}{path}"))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
            }
            // Assets and the login endpoint remain reachable through the nested auth middleware.
            let response = server
                .client
                .get(format!("{root}/assets/app.js"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let response = server
                .client
                .post(format!("{root}/api/login"))
                .json(&json!({"password": "wrong"}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            let response = server
                .client
                .post(format!("{root}/api/login"))
                .json(&json!({"password": "test-password"}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let cookies: Vec<_> = response
                .headers()
                .get_all("set-cookie")
                .iter()
                .map(|v| v.to_str().unwrap())
                .collect();
            assert_eq!(cookies.len(), 3);
            for cookie in &cookies {
                assert!(cookie.contains(&format!("; Path={prefix}/; HttpOnly; SameSite=Lax")));
            }
            let session = cookies[0].split(';').next().unwrap();
            for path in [
                "/api/status",
                "/download/books/sample.txt",
                "/download-zip/books",
            ] {
                let response = server
                    .client
                    .get(format!("{root}{path}"))
                    .header("cookie", session)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK, "{path}");
            }
        }
    }
}
