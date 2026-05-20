use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::ui::web::state::{AppState, LibraryScanRow, LibraryScanStore};

#[derive(Debug, Deserialize)]
pub(crate) struct LibraryQuery {
    pub(crate) path: Option<String>,
    /// 按文件名/文件夹名关键词模糊过滤（忽略大小写）
    pub(crate) name: Option<String>,
    /// 是否启动一次新扫描。默认 true；轮询/启动预缓存时会传 false。
    pub(crate) start: Option<bool>,
}

pub(crate) async fn api_library(
    State(state): State<AppState>,
    Query(q): Query<LibraryQuery>,
) -> Json<Value> {
    let base = state.library_root.clone();
    let rel = normalize_rel(q.path.unwrap_or_default());
    let should_start = q.start.unwrap_or(true) || state.library_scan.snapshot(&rel).is_none();

    if should_start {
        spawn_library_scan(
            base.as_ref().clone(),
            rel.clone(),
            state.library_scan.clone(),
        );
    }

    let snapshot = state
        .library_scan
        .snapshot(&rel)
        .unwrap_or_else(|| empty_snapshot(rel.clone()));
    let mut items = snapshot.items;

    if let Some(ref kw) = q.name {
        let kw_lower = kw.to_lowercase();
        items.retain(|i| i.name.to_lowercase().contains(&kw_lower));
    }

    Json(json!({
        "root": base.to_string_lossy(),
        "path": snapshot.path,
        "items": items,
        "running": snapshot.running,
        "scanned": snapshot.scanned,
        "error": snapshot.error,
        "started_ms": snapshot.started_ms,
        "updated_ms": snapshot.updated_ms,
    }))
}

pub(crate) fn spawn_library_scan(root: PathBuf, rel: String, store: Arc<LibraryScanStore>) {
    let rel = normalize_rel(rel);
    if !store.try_start(rel.clone()) {
        return;
    }

    thread::spawn(move || {
        if let Err(err) = scan_library_streaming(&root, &rel, &store) {
            store.finish_failed(&rel, err.to_string());
        }
    });
}

fn normalize_rel(rel: String) -> String {
    rel.replace('\\', "/").trim_matches('/').to_string()
}

fn empty_snapshot(path: String) -> crate::ui::web::state::LibraryScanInfo {
    crate::ui::web::state::LibraryScanInfo {
        path,
        running: false,
        scanned: 0,
        items: Vec::new(),
        error: None,
        started_ms: 0,
        updated_ms: 0,
    }
}

fn is_allowed_ext(ext: &str) -> bool {
    matches!(ext, "epub" | "txt" | "mp3" | "wav")
}

fn scan_library_streaming(root: &Path, rel: &str, store: &LibraryScanStore) -> std::io::Result<()> {
    let root_canon = std::fs::canonicalize(root)?;

    let target = if rel.trim().is_empty() {
        root_canon.clone()
    } else {
        let joined = root_canon.join(rel);
        let canon = std::fs::canonicalize(&joined)?;
        if !canon.starts_with(&root_canon) {
            store.finish(rel, Vec::new());
            return Ok(());
        }
        canon
    };

    let mut items = Vec::new();
    let mut scanned = 0usize;
    for entry in std::fs::read_dir(&target)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        let Some(item) = item_from_entry(&root_canon, &path, &meta) else {
            continue;
        };
        scanned += 1;
        store.push_item(rel, item.clone(), scanned);
        items.push(item);
    }

    store.finish(rel, items);
    Ok(())
}

fn item_from_entry(root: &Path, path: &Path, meta: &std::fs::Metadata) -> Option<LibraryScanRow> {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let rel_path = rel.to_string_lossy().replace('\\', "/");
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&rel_path)
        .to_string();
    let modified_ms = meta.modified().ok().and_then(system_time_ms);

    if meta.is_dir() {
        return Some(LibraryScanRow {
            kind: "dir".to_string(),
            name,
            rel_path,
            ext: String::new(),
            // 不再逐个目录统计子文件，避免书很多时根目录读取被每本书的子目录 IO 放大。
            size: 0,
            file_count: None,
            modified_ms,
        });
    }

    if !meta.is_file() {
        return None;
    }

    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !is_allowed_ext(&ext) {
        return None;
    }

    Some(LibraryScanRow {
        kind: "file".to_string(),
        name,
        rel_path,
        ext,
        size: meta.len(),
        file_count: None,
        modified_ms,
    })
}

fn system_time_ms(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}
