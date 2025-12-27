//! Web Server UI（纯 HTML 前端）。

mod router;
mod routes;
mod state;
mod templates;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tracing::info;

use crate::base_system::context::Config;
use sha2::{Digest, Sha256};
use state::{AppState, AuthState, ConfigView, JobStore};

pub fn run(config: &mut Config, password: Option<String>) -> Result<()> {
    let bind: SocketAddr = std::env::var("TOMATO_WEB_ADDR")
        .unwrap_or_else(|_| DEFAULT_BIND.to_string())
        .parse()?;

    let view = ConfigView {
        old_cli: config.old_cli,
        use_official_api: config.use_official_api,
        save_path: config.save_path.clone(),
        api_endpoints_len: config.api_endpoints.len(),
    };

    let library_root = config.default_save_dir();

    let auth = password
        .or_else(|| std::env::var("TOMATO_WEB_PASSWORD").ok())
        .and_then(|p| {
            let p = p.trim().to_string();
            if p.is_empty() {
                None
            } else {
                let mut h = Sha256::new();
                h.update(p.as_bytes());
                let out = h.finalize();
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&out);
                Some(AuthState {
                    password_sha256: arr,
                })
            }
        });

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(run_async(bind, view, config.clone(), library_root, auth))
}

const DEFAULT_BIND: &str = "127.0.0.1:18423";

async fn run_async(
    bind: SocketAddr,
    view: ConfigView,
    config: Config,
    library_root: PathBuf,
    auth: Option<AuthState>,
) -> Result<()> {
    let state = AppState {
        bind,
        config_view: Arc::new(view),
        config: Arc::new(std::sync::Mutex::new(config)),
        library_root: Arc::new(library_root),
        jobs: Arc::new(JobStore::default()),
        auth,
    };

    let locked = state.auth.is_some();
    let app = router::build_router(state.clone());

    let listener = tokio::net::TcpListener::bind(bind).await?;
    info!(target: "web", "Web UI listening on http://{bind}/ (set TOMATO_WEB_ADDR to override)");

    if locked {
        info!(target: "web", "Web UI lock mode enabled (password required)");
        println!("Web UI listening on http://{bind}/ (LOCKED)");
    } else {
        println!("Web UI listening on http://{bind}/");
    }
    println!("Press Ctrl+C to stop.");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
