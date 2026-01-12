use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::ui::web::state::AppState;

#[derive(Debug, Clone, serde::Serialize)]
struct LibraryItem {
    kind: String,
    name: String,
    rel_path: String,
    ext: String,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_count: Option<u64>,
    modified_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LibraryQuery {
    pub(crate) path: Option<String>,
}

pub(crate) async fn api_library(
    State(state): State<AppState>,
    Query(q): Query<LibraryQuery>,
) -> Json<Value> {
    let base = state.library_root.clone();
    let base_for_task = base.clone();
    let rel = q.path.unwrap_or_default();
    let rel_for_task = rel.clone();
    let items = tokio::task::spawn_blocking(move || scan_library(&base_for_task, &rel_for_task))
        .await
        .unwrap_or_default();

    Json(json!({
        "root": base.to_string_lossy(),
        "path": rel,
        "items": items,
    }))
}

fn is_allowed_ext(ext: &str) -> bool {
    matches!(ext, "epub" | "txt" | "mp3" | "wav")
}

fn scan_library(root: &Path, rel: &str) -> Vec<LibraryItem> {
    let mut out = Vec::new();
    let Ok(root_canon) = std::fs::canonicalize(root) else {
        return out;
    };

    let target = if rel.trim().is_empty() {
        root_canon.clone()
    } else {
        let joined = root_canon.join(rel);
        let Ok(canon) = std::fs::canonicalize(&joined) else {
            return out;
        };
        if !canon.starts_with(&root_canon) {
            return out;
        }
        canon
    };

    let _ = list_level(&root_canon, &target, &mut out);
    out.sort_by(|a, b| {
        b.modified_ms
            .cmp(&a.modified_ms)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn list_level(root: &Path, dir: &Path, out: &mut Vec<LibraryItem>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        if meta.is_dir() {
            // 不把文件夹下内容全部递归平铺出来；改为“文件夹汇总”。
            if let Some(sum) = summarize_dir(&path) {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                let rel_path = rel.to_string_lossy().replace('\\', "/");
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&rel_path)
                    .to_string();

                out.push(LibraryItem {
                    kind: "dir".to_string(),
                    name,
                    rel_path,
                    ext: String::new(),
                    size: sum.total_size,
                    file_count: Some(sum.file_count),
                    modified_ms: sum.latest_modified_ms,
                });
            }
            continue;
        }

        if !meta.is_file() {
            continue;
        }

        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !is_allowed_ext(&ext) {
            continue;
        }

        let rel = path.strip_prefix(root).unwrap_or(&path);
        let rel_path = rel.to_string_lossy().replace('\\', "/");
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&rel_path)
            .to_string();

        let modified_ms = meta.modified().ok().and_then(system_time_ms);

        out.push(LibraryItem {
            kind: "file".to_string(),
            name,
            rel_path,
            ext,
            size: meta.len(),
            file_count: None,
            modified_ms,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct DirSummary {
    file_count: u64,
    total_size: u64,
    latest_modified_ms: Option<u64>,
}

fn summarize_dir(dir: &Path) -> Option<DirSummary> {
    let mut sum = DirSummary {
        file_count: 0,
        total_size: 0,
        latest_modified_ms: None,
    };
    let _ = walk_dir_summary(dir, &mut sum);
    if sum.file_count == 0 { None } else { Some(sum) }
}

fn walk_dir_summary(dir: &Path, sum: &mut DirSummary) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        if meta.is_dir() {
            let _ = walk_dir_summary(&path, sum);
            continue;
        }

        if !meta.is_file() {
            continue;
        }

        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !is_allowed_ext(&ext) {
            continue;
        }

        sum.file_count += 1;
        sum.total_size = sum.total_size.saturating_add(meta.len());

        if let Some(ms) = meta.modified().ok().and_then(system_time_ms) {
            sum.latest_modified_ms = match sum.latest_modified_ms {
                Some(prev) => Some(prev.max(ms)),
                None => Some(ms),
            };
        }
    }
    Ok(())
}

fn system_time_ms(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}
