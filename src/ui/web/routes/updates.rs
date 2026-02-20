use std::path::Path;

use crate::base_system::novel_updates;
use anyhow::Result;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::ui::web::state::AppState;

pub(crate) async fn api_updates(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let cfg = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let save_dir = cfg.default_save_dir();

    let result = tokio::task::spawn_blocking(move || scan_updates(&save_dir, &cfg))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match result {
        Ok(payload) => Ok(Json(payload)),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

fn scan_updates(save_dir: &Path, _config: &crate::base_system::context::Config) -> Result<Value> {
    let scan = novel_updates::scan_novel_updates(save_dir)?;

    let to_row = |it: novel_updates::NovelUpdateRow| {
        json!({
            "book_id": it.book_id,
            "book_name": it.book_name,
            "folder": it.folder.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string(),
            "local_total": it.local_total,
            "local_failed": it.local_failed,
            "remote_total": it.remote_total,
            "new_count": it.new_count,
            "has_update": it.has_update,
            "is_ignored": it.is_ignored,
        })
    };

    Ok(json!({
        "save_dir": save_dir.display().to_string(),
        "updates": scan.updates.into_iter().map(to_row).collect::<Vec<_>>(),
        "no_updates": scan.no_updates.into_iter().map(to_row).collect::<Vec<_>>(),
    }))
}
