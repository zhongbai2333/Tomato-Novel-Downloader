//! Web Server UI（纯 HTML 前端）。

mod router;
mod routes;
mod state;
mod templates;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tracing::{info, warn};

use crate::base_system::context::Config;
use state::{AppState, AuthState, ConfigView, JobStore};

pub fn run(config: &mut Config, password: Option<String>) -> Result<()> {
    let bind_raw = std::env::var("TOMATO_WEB_ADDR").unwrap_or_else(|_| DEFAULT_BIND.to_string());
    let bind_addrs: Vec<SocketAddr> = parse_bind_addrs(&bind_raw)?;

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
                Some(AuthState::from_password(&p))
            }
        });

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(run_async(
        bind_addrs,
        view,
        config.clone(),
        library_root,
        auth,
    ))
}

const DEFAULT_BIND: &str = "127.0.0.1:18423";

fn parse_bind_addr(raw: &str) -> Result<SocketAddr> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(anyhow!("empty bind addr"));
    }

    // Standard formats:
    // - IPv4: 127.0.0.1:18423
    // - IPv6: [::1]:18423
    if let Ok(a) = s.parse::<SocketAddr>() {
        return Ok(a);
    }

    // Tolerate missing brackets for IPv6, e.g. "::1:18423".
    // We interpret the last ':' segment as port if it's all digits.
    if !s.starts_with('[')
        && s.contains(':')
        && let Some((host, port)) = s.rsplit_once(':')
        && !host.is_empty()
        && port.chars().all(|c| c.is_ascii_digit())
        && host.contains(':')
    {
        let wrapped = format!("[{host}]:{port}");
        if let Ok(a) = wrapped.parse::<SocketAddr>() {
            return Ok(a);
        }
    }

    Err(anyhow!(
        "invalid TOMATO_WEB_ADDR: '{s}'. Use '127.0.0.1:18423' or '[::1]:18423' (IPv6 needs brackets). For multiple binds, separate by comma: '0.0.0.0:18423,[::]:18423'."
    ))
}

fn parse_bind_addrs(raw: &str) -> Result<Vec<SocketAddr>> {
    let parts: Vec<&str> = raw
        .split([',', ';'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() {
        return Err(anyhow!("empty TOMATO_WEB_ADDR"));
    }

    if parts.len() == 1 {
        return Ok(vec![parse_bind_addr(parts[0])?]);
    }

    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        let a = parse_bind_addr(p)?;
        if !out.contains(&a) {
            out.push(a);
        }
    }

    if out.is_empty() {
        return Err(anyhow!("no valid bind addresses"));
    }

    Ok(out)
}

async fn run_async(
    bind_addrs: Vec<SocketAddr>,
    view: ConfigView,
    config: Config,
    library_root: PathBuf,
    auth: Option<AuthState>,
) -> Result<()> {
    let state = AppState {
        bind_addrs: Arc::new(bind_addrs.clone()),
        config_view: Arc::new(view),
        config: Arc::new(std::sync::Mutex::new(config)),
        library_root: Arc::new(library_root),
        jobs: Arc::new(JobStore::default()),
        auth,
    };

    let locked = state.auth.is_some();

    // Shared shutdown trigger for all listeners.
    let notify = Arc::new(tokio::sync::Notify::new());
    {
        let notify = notify.clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            notify.notify_waiters();
        });
    }

    let mut servers = Vec::new();
    for bind in bind_addrs {
        let listener = match tokio::net::TcpListener::bind(bind).await {
            Ok(l) => l,
            Err(e) => {
                // On some platforms, binding both [::]:PORT and 0.0.0.0:PORT can fail with
                // AddrInUse because IPv6 listener may already accept IPv4 (dual-stack).
                // If at least one listener has started, treat AddrInUse as non-fatal.
                if !servers.is_empty() && e.kind() == std::io::ErrorKind::AddrInUse {
                    warn!(target: "web", bind = %bind, error = %e, "bind failed (AddrInUse), likely already covered by another listener; skipping");
                    continue;
                }
                return Err(anyhow!(e).context(format!("bind failed: {bind}")));
            }
        };

        info!(target: "web", "Web UI listening on http://{bind}/ (set TOMATO_WEB_ADDR to override)");
        if locked {
            info!(target: "web", "Web UI lock mode enabled (password required)");
            println!("Web UI listening on http://{bind}/ (LOCKED)");
        } else {
            println!("Web UI listening on http://{bind}/");
        }

        let app = router::build_router(state.clone());
        let notify = notify.clone();
        servers.push(tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                notify.notified().await;
            })
            .await
        }));
    }

    if servers.is_empty() {
        return Err(anyhow!("no listeners started (check TOMATO_WEB_ADDR)"));
    }

    println!("Press Ctrl+C to stop.");

    // Wait for all servers to exit.
    for h in servers {
        h.await
            .map_err(|e| anyhow!("server task join failed: {e}"))?
            .map_err(|e| anyhow!(e))?;
    }

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    println!("Stoping Server...");
}
