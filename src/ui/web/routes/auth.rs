use std::fs;
use std::net::SocketAddr;
use std::path::Path;

use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::http::header::SET_COOKIE;
use axum::response::{AppendHeaders, IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use serde_yaml;
use tracing::info;

use crate::base_system::config::{ConfigSpec, generate_yaml_with_comments, write_with_comments};
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

    if !verify_password(auth, provided) {
        info!(target: "web_auth", ip = %addr, ok = false, "login failed");
        return Err(StatusCode::UNAUTHORIZED);
    }

    info!(target: "web_auth", ip = %addr, ok = true, "login ok");

    // 使用服务端签名的会话 token，避免在 Cookie 中存储明文密码。
    let token = auth.issue_session_token();
    let cookie = format!(
        "tomato_session={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        token,
        auth.session_ttl_secs()
    );
    // 兼容旧前端/旧代理配置：同值下发 auth_token，避免出现“Cookie 已下发但后端不识别”。
    let compat_cookie = format!(
        "auth_token={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        token,
        auth.session_ttl_secs()
    );
    let clear_legacy_cookie = "tomato_pw=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0";

    Ok((
        AppendHeaders([
            (SET_COOKIE, cookie),
            (SET_COOKIE, compat_cookie),
            (SET_COOKIE, clear_legacy_cookie.to_string()),
        ]),
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
    let cfg = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
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
        let mut g = state.config.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut g = state.config.lock().unwrap_or_else(|e| e.into_inner());
        *g = old_cfg;
        tracing::error!(target: "web_config", err = %e, "failed to persist config.yml");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    Ok(Json(json!({"ok": true})))
}

#[derive(Debug, Serialize)]
pub(crate) struct WebConfigRawView {
    pub(crate) yaml: String,
    pub(crate) generated: bool,
}

pub(crate) async fn get_config_raw(
    State(state): State<AppState>,
) -> Result<Json<WebConfigRawView>, StatusCode> {
    let path = Path::new(<Config as ConfigSpec>::FILE_NAME);
    if let Ok(raw) = fs::read_to_string(path) {
        return Ok(Json(WebConfigRawView {
            yaml: raw,
            generated: false,
        }));
    }

    let cfg = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let yaml = generate_yaml_with_comments(&cfg).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(WebConfigRawView {
        yaml,
        generated: true,
    }))
}

#[derive(Debug, Deserialize)]
pub(crate) struct WebConfigRawPatch {
    pub(crate) yaml: String,
}

pub(crate) async fn set_config_raw(
    State(state): State<AppState>,
    Json(patch): Json<WebConfigRawPatch>,
) -> Result<Json<Value>, StatusCode> {
    if patch.yaml.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut cfg: Config = serde_yaml::from_str(&patch.yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
    normalize_config(&mut cfg);
    validate_config(&cfg).map_err(|_| StatusCode::BAD_REQUEST)?;

    let old_cfg = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let path = Path::new(<Config as ConfigSpec>::FILE_NAME);
    if let Err(e) = write_with_comments(&cfg, path) {
        let mut g = state.config.lock().unwrap_or_else(|e| e.into_inner());
        *g = old_cfg;
        tracing::error!(target: "web_config", err = %e, "failed to persist config.yml");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let mut g = state.config.lock().unwrap_or_else(|e| e.into_inner());
    *g = cfg;

    Ok(Json(json!({"ok": true})))
}

pub(crate) async fn get_config_full(State(state): State<AppState>) -> Json<Config> {
    let cfg = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    Json(cfg)
}

pub(crate) async fn set_config_full(
    State(state): State<AppState>,
    Json(mut cfg): Json<Config>,
) -> Result<Json<Value>, StatusCode> {
    normalize_config(&mut cfg);
    validate_config(&cfg).map_err(|_| StatusCode::BAD_REQUEST)?;

    let old_cfg = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let path = Path::new(<Config as ConfigSpec>::FILE_NAME);
    if let Err(e) = write_with_comments(&cfg, path) {
        let mut g = state.config.lock().unwrap_or_else(|e| e.into_inner());
        *g = old_cfg;
        tracing::error!(target: "web_config", err = %e, "failed to persist config.yml");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let mut g = state.config.lock().unwrap_or_else(|e| e.into_inner());
    *g = cfg;

    Ok(Json(json!({"ok": true})))
}

fn normalize_config(cfg: &mut Config) {
    cfg.novel_format = cfg.novel_format.trim().to_ascii_lowercase();
    cfg.audiobook_format = cfg.audiobook_format.trim().to_ascii_lowercase();
    cfg.audiobook_tts_provider = cfg.audiobook_tts_provider.trim().to_ascii_lowercase();
    cfg.preferred_book_name_field = cfg.preferred_book_name_field.trim().to_ascii_lowercase();
}

fn validate_config(cfg: &Config) -> Result<(), String> {
    if cfg.novel_format != "txt" && cfg.novel_format != "epub" {
        return Err("novel_format must be txt or epub".to_string());
    }
    if cfg.enable_segment_comments && cfg.novel_format != "epub" {
        return Err("segment comments require epub".to_string());
    }
    if cfg.audiobook_format != "mp3" && cfg.audiobook_format != "wav" {
        return Err("audiobook_format must be mp3 or wav".to_string());
    }
    if cfg.max_workers == 0 {
        return Err("max_workers must be > 0".to_string());
    }
    if cfg.request_timeout == 0 {
        return Err("request_timeout must be > 0".to_string());
    }
    if cfg.min_connect_timeout <= 0.0 {
        return Err("min_connect_timeout must be > 0".to_string());
    }
    if cfg.min_wait_time > cfg.max_wait_time {
        return Err("min_wait_time cannot exceed max_wait_time".to_string());
    }
    if cfg.audiobook_concurrency == 0 {
        return Err("audiobook_concurrency must be > 0".to_string());
    }
    if cfg.segment_comments_top_n == 0 {
        return Err("segment_comments_top_n must be > 0".to_string());
    }
    if cfg.segment_comments_workers == 0 {
        return Err("segment_comments_workers must be > 0".to_string());
    }
    if cfg.media_download_workers == 0 {
        return Err("media_download_workers must be > 0".to_string());
    }
    if cfg.jpeg_quality > 100 {
        return Err("jpeg_quality must be 0-100".to_string());
    }
    if cfg.first_line_indent_em < 0.0 {
        return Err("first_line_indent_em must be >= 0".to_string());
    }
    match cfg.preferred_book_name_field.as_str() {
        "" | "book_name" | "original_book_name" | "book_short_name" | "ask_after_download" => {}
        _ => {
            return Err(
                "preferred_book_name_field must be book_name/original_book_name/book_short_name/ask_after_download"
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn verify_password(auth: &crate::ui::web::state::AuthState, provided: &str) -> bool {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(provided.as_bytes());
    let out = h.finalize();
    out.as_slice() == auth.password_sha256
}
