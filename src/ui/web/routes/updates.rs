use std::fs;
use std::path::Path;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use tomato_novel_official_api::DirectoryClient;

use crate::ui::web::state::AppState;

pub(crate) async fn api_updates(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let cfg = state.config.lock().unwrap().clone();
    let save_dir = cfg.default_save_dir();

    let result = tokio::task::spawn_blocking(move || scan_updates(&save_dir))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match result {
        Ok(payload) => Ok(Json(payload)),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

fn scan_updates(save_dir: &Path) -> Result<Value> {
    let mut updates: Vec<Value> = Vec::new();
    let mut no_updates: Vec<Value> = Vec::new();

    if !save_dir.exists() {
        return Ok(json!({
            "save_dir": save_dir.display().to_string(),
            "updates": [],
            "no_updates": [],
        }));
    }

    let dir_reader = fs::read_dir(save_dir).with_context(|| format!("read dir {}", save_dir.display()))?;
    let client = DirectoryClient::new().context("init DirectoryClient")?;

    for entry in dir_reader.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let (book_id, book_name) = match name.split_once('_') {
            Some((id, n)) if id.chars().all(|c| c.is_ascii_digit()) => (id.to_string(), n.to_string()),
            _ => continue,
        };

        let (local_total, _local_ok, local_failed) =
            read_downloaded_counts(&path, &book_id).unwrap_or((0, 0, 0));

        let chapter_list = match client.fetch_directory(&book_id) {
            Ok(d) => d.chapters,
            Err(_) => Vec::new(),
        };
        if chapter_list.is_empty() {
            continue;
        }
        let remote_total = chapter_list.len();

        // "新章节" 基于本地已知章节条目数量（包含失败/空内容条目），避免把失败章误报成新章。
        let new_count = remote_total.saturating_sub(local_total);
        let has_update = new_count > 0 || local_failed > 0;

        let row = json!({
            "book_id": book_id,
            "book_name": book_name,
            "folder": path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string(),
            "local_total": local_total,
            "local_failed": local_failed,
            "remote_total": remote_total,
            "new_count": new_count,
            "has_update": has_update,
        });

        if has_update {
            updates.push(row);
        } else {
            no_updates.push(row);
        }
    }

    // Stable-ish order: most actionable first.
    updates.sort_by(|a, b| {
        let an = a.get("new_count").and_then(|v| v.as_u64()).unwrap_or(0);
        let bn = b.get("new_count").and_then(|v| v.as_u64()).unwrap_or(0);
        bn.cmp(&an)
    });

    Ok(json!({
        "save_dir": save_dir.display().to_string(),
        "updates": updates,
        "no_updates": no_updates,
    }))
}

fn read_downloaded_counts(folder: &Path, book_id: &str) -> Option<(usize, usize, usize)> {
    let status_new = folder.join("status.json");
    let status_old = folder.join(format!("chapter_status_{}.json", book_id));
    let path = if status_new.exists() {
        status_new
    } else if status_old.exists() {
        status_old
    } else {
        return None;
    };

    let data = fs::read_to_string(&path).ok()?;
    let value: Value = serde_json::from_str(&data).ok()?;
    let downloaded = value.get("downloaded")?.as_object()?;

    let total = downloaded.len();
    let mut ok = 0usize;
    for (_cid, pair) in downloaded {
        match pair {
            Value::Array(arr) => {
                if arr.get(1).and_then(|v| v.as_str()).is_some() {
                    ok += 1;
                }
            }
            Value::Object(obj) => {
                if obj
                    .get("content")
                    .or_else(|| obj.get("text"))
                    .and_then(|v| v.as_str())
                    .is_some()
                {
                    ok += 1;
                }
            }
            _ => {}
        }
    }
    let failed = total.saturating_sub(ok);
    Some((total, ok, failed))
}
