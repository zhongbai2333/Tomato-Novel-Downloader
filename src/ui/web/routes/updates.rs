use std::path::Path;
use std::thread;

use crate::base_system::novel_updates;
use anyhow::Result;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::ui::web::state::{AppState, UpdateScanRow, UpdateScanStore};

#[derive(Debug, Deserialize)]
pub(crate) struct UpdatesQuery {
    /// 是否启动一次新扫描。默认 true；前端轮询进度时会传 false，避免扫描结束后立刻重开。
    pub(crate) start: Option<bool>,
}

pub(crate) async fn api_updates(
    State(state): State<AppState>,
    Query(q): Query<UpdatesQuery>,
) -> Result<Json<Value>, StatusCode> {
    let cfg = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let save_dir = cfg.default_save_dir();
    let save_dir_display = save_dir.display().to_string();

    if q.start.unwrap_or(true) && state.update_scan.try_start(save_dir_display.clone()) {
        let store = state.update_scan.clone();
        thread::spawn(move || {
            if let Err(err) = scan_updates(&save_dir, store.clone()) {
                store.finish_failed(err.to_string());
            }
        });
    }

    let snapshot = state.update_scan.snapshot();
    Ok(Json(json!({
        "running": snapshot.running,
        "scanned": snapshot.scanned,
        "total": snapshot.total,
        "save_dir": if snapshot.save_dir.is_empty() { save_dir_display } else { snapshot.save_dir },
        "updates": snapshot.updates,
        "no_updates": snapshot.no_updates,
        "error": snapshot.error,
        "started_ms": snapshot.started_ms,
        "updated_ms": snapshot.updated_ms,
    })))
}

fn scan_updates(save_dir: &Path, store: std::sync::Arc<UpdateScanStore>) -> Result<()> {
    let scan = novel_updates::scan_novel_updates_with_progress(save_dir, |progress| {
        store.push_progress(
            row_from_update(progress.row),
            progress.scanned,
            progress.total,
        );
    })?;

    store.finish(
        save_dir.display().to_string(),
        scan.updates.into_iter().map(row_from_update).collect(),
        scan.no_updates.into_iter().map(row_from_update).collect(),
    );
    Ok(())
}

fn row_from_update(it: novel_updates::NovelUpdateRow) -> UpdateScanRow {
    UpdateScanRow {
        book_id: it.book_id,
        book_name: it.book_name,
        folder: it
            .folder
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string(),
        local_total: it.local_total,
        local_failed: it.local_failed,
        remote_total: it.remote_total,
        new_count: it.new_count,
        has_update: it.has_update,
        is_ignored: it.is_ignored,
    }
}
