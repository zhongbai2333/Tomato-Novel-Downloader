use std::fs;
use std::net::SocketAddr;
use std::path::Path;

use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::http::header::SET_COOKIE;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use serde_yaml;
use sha2::{Digest, Sha256};
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

    let cfg = state.config.lock().unwrap().clone();
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

    let old_cfg = state.config.lock().unwrap().clone();
    let path = Path::new(<Config as ConfigSpec>::FILE_NAME);
    if let Err(e) = write_with_comments(&cfg, path) {
        let mut g = state.config.lock().unwrap();
        *g = old_cfg;
        tracing::error!(target: "web_config", err = %e, "failed to persist config.yml");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let mut g = state.config.lock().unwrap();
    *g = cfg;

    Ok(Json(json!({"ok": true})))
}

pub(crate) async fn get_config_full(State(state): State<AppState>) -> Json<Config> {
    let cfg = state.config.lock().unwrap().clone();
    Json(cfg)
}

pub(crate) async fn set_config_full(
    State(state): State<AppState>,
    Json(mut cfg): Json<Config>,
) -> Result<Json<Value>, StatusCode> {
    normalize_config(&mut cfg);
    validate_config(&cfg).map_err(|_| StatusCode::BAD_REQUEST)?;

    let old_cfg = state.config.lock().unwrap().clone();
    let path = Path::new(<Config as ConfigSpec>::FILE_NAME);
    if let Err(e) = write_with_comments(&cfg, path) {
        let mut g = state.config.lock().unwrap();
        *g = old_cfg;
        tracing::error!(target: "web_config", err = %e, "failed to persist config.yml");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let mut g = state.config.lock().unwrap();
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
