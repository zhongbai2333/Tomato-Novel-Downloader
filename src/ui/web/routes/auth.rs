use std::net::SocketAddr;
use std::path::Path;

use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::http::header::SET_COOKIE;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tracing::info;

use crate::base_system::config::{ConfigSpec, write_with_comments};
use crate::base_system::context::Config;
use crate::ui::web::state::AppState;

#[derive(Debug, Deserialize)]
pub(crate) struct LoginReq {
    pub(crate) password: String,
}

pub(crate) async fn api_login(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<LoginReq>,
) -> Result<axum::response::Response, StatusCode> {
    // If lock mode not enabled, treat as always OK.
    let Some(auth) = &state.auth else {
        info!(target: "web_auth", ip = %addr, ok = true, "login (unlocked)");
        return Ok(Json(json!({"ok": true, "locked": false})).into_response());
    };

    let provided = req.password.trim();
    if provided.is_empty() {
        info!(target: "web_auth", ip = %addr, ok = false, "login failed (empty)");
        return Err(StatusCode::UNAUTHORIZED);
    }

    let mut h = Sha256::new();
    h.update(provided.as_bytes());
    let out = h.finalize();

    if out.as_slice() != auth.password_sha256 {
        info!(target: "web_auth", ip = %addr, ok = false, "login failed");
        return Err(StatusCode::UNAUTHORIZED);
    }

    info!(target: "web_auth", ip = %addr, ok = true, "login ok");

    // Store password in an HttpOnly cookie so normal link downloads work.
    // Note: this is intended for LAN/self-host usage.
    let cookie = format!(
        "tomato_pw={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800",
        req.password
    );

    Ok((
        [(SET_COOKIE, cookie)],
        Json(json!({"ok": true, "locked": true})),
    )
        .into_response())
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WebConfigView {
    pub(crate) novel_format: String,
    pub(crate) bulk_files: bool,
    pub(crate) enable_audiobook: bool,
    pub(crate) audiobook_format: String,
}

pub(crate) async fn get_config(State(state): State<AppState>) -> Json<WebConfigView> {
    let cfg = state.config.lock().unwrap().clone();
    Json(WebConfigView {
        novel_format: cfg.novel_format,
        bulk_files: cfg.bulk_files,
        enable_audiobook: cfg.enable_audiobook,
        audiobook_format: cfg.audiobook_format,
    })
}

#[derive(Debug, Deserialize)]
pub(crate) struct WebConfigPatch {
    pub(crate) novel_format: Option<String>,
    pub(crate) bulk_files: Option<bool>,
    pub(crate) enable_audiobook: Option<bool>,
    pub(crate) audiobook_format: Option<String>,
}

pub(crate) async fn set_config(
    State(state): State<AppState>,
    Json(patch): Json<WebConfigPatch>,
) -> Result<Json<Value>, StatusCode> {
    let (old_cfg, new_cfg) = {
        let mut g = state.config.lock().unwrap();
        let old = g.clone();

        if let Some(v) = patch.novel_format {
            let v = v.trim().to_lowercase();
            if v != "txt" && v != "epub" {
                return Err(StatusCode::BAD_REQUEST);
            }
            g.novel_format = v;
        }

        if let Some(v) = patch.bulk_files {
            g.bulk_files = v;
        }

        if let Some(v) = patch.enable_audiobook {
            g.enable_audiobook = v;
        }

        if let Some(v) = patch.audiobook_format {
            let v = v.trim().to_lowercase();
            if v != "mp3" && v != "wav" {
                return Err(StatusCode::BAD_REQUEST);
            }
            g.audiobook_format = v;
        }

        (old, g.clone())
    };

    let path = Path::new(<Config as ConfigSpec>::FILE_NAME);
    if let Err(e) = write_with_comments(&new_cfg, path) {
        // revert memory changes if persistence fails
        let mut g = state.config.lock().unwrap();
        *g = old_cfg;
        tracing::error!(target: "web_config", err = %e, "failed to persist config.yml");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    Ok(Json(json!({"ok": true})))
}
