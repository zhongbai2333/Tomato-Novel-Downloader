//! 复用的“更新小说扫描”逻辑（供 TUI / Web / noui 共用）。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

#[cfg(feature = "official-api")]
use tomato_novel_official_api::DirectoryClient;

#[cfg(not(feature = "official-api"))]
use crate::network_parser::network::{FanqieWebConfig, FanqieWebNetwork};

#[derive(Debug, Clone)]
pub struct NovelUpdateRow {
    pub book_id: String,
    pub book_name: String,
    pub folder: PathBuf,
    pub local_total: usize,
    pub local_failed: usize,
    pub remote_total: usize,
    pub new_count: usize,
    pub has_update: bool,
    pub is_ignored: bool,
}

#[derive(Debug, Default, Clone)]
pub struct NovelUpdateScanResult {
    pub updates: Vec<NovelUpdateRow>,
    pub no_updates: Vec<NovelUpdateRow>,
}

/// 扫描保存目录下的书籍文件夹（形如 `<book_id>_<book_name>`），并对比远端目录。
///
/// 备注："新章节" 以本地已知章节条目数（包含失败/空内容条目）为基准，避免把失败章误报成新章。
pub fn scan_novel_updates(save_dir: &Path) -> Result<NovelUpdateScanResult> {
    if !save_dir.exists() {
        return Ok(NovelUpdateScanResult::default());
    }

    let dir_reader =
        fs::read_dir(save_dir).with_context(|| format!("read dir {}", save_dir.display()))?;

    #[cfg(feature = "official-api")]
    let client = DirectoryClient::new().context("init DirectoryClient")?;

    #[cfg(not(feature = "official-api"))]
    let client =
        FanqieWebNetwork::new(FanqieWebConfig::default()).context("init FanqieWebNetwork")?;

    let mut updates = Vec::new();
    let mut no_updates = Vec::new();

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
            Some((id, n)) if id.chars().all(|c| c.is_ascii_digit()) => {
                (id.to_string(), n.to_string())
            }
            _ => continue,
        };

        // 一次性读取 status.json，同时获取下载计数和忽略标志
        let (counts, is_ignored) = read_status_counts_and_ignore(&path, &book_id);
        let (local_total, _local_ok, local_failed) = counts.unwrap_or((0, 0, 0));

        #[cfg(feature = "official-api")]
        let remote_total = {
            let chapter_list = match client.fetch_directory(&book_id) {
                Ok(d) => d.chapters,
                Err(_) => Vec::new(),
            };
            if chapter_list.is_empty() {
                continue;
            }
            chapter_list.len()
        };

        #[cfg(not(feature = "official-api"))]
        let remote_total = {
            let chapter_list = match client.fetch_chapter_list(&book_id) {
                Some(list) => list,
                None => Vec::new(),
            };
            if chapter_list.is_empty() {
                continue;
            }
            chapter_list.len()
        };

        let new_count = remote_total.saturating_sub(local_total);
        let has_update = new_count > 0 || local_failed > 0;

        let row = NovelUpdateRow {
            book_id,
            book_name,
            folder: path,
            local_total,
            local_failed,
            remote_total,
            new_count,
            has_update,
            is_ignored,
        };

        // 被忽略的书籍始终归入"无更新"列表
        if is_ignored {
            no_updates.push(row);
        } else if has_update {
            updates.push(row);
        } else {
            no_updates.push(row);
        }
    }

    // Stable-ish order: most actionable first.
    updates.sort_by(|a, b| b.new_count.cmp(&a.new_count));

    Ok(NovelUpdateScanResult {
        updates,
        no_updates,
    })
}

/// 读取某本书本地状态文件中 "downloaded" 的统计信息：
/// - total: 条目数（包含失败/空内容的条目）
/// - ok: 成功下载的条目数（content/text 非空）
/// - failed: total - ok
pub fn read_downloaded_counts(folder: &Path, book_id: &str) -> Option<(usize, usize, usize)> {
    let value = read_status_json(folder, book_id)?;
    counts_from_status(&value)
}

/// 仅统计成功下载的章节数（content/text 非空）。
pub fn read_downloaded_ok_count(folder: &Path, book_id: &str) -> Option<usize> {
    let (_total, ok, _failed) = read_downloaded_counts(folder, book_id)?;
    Some(ok)
}

/// 读取书籍的ignore_updates标志
#[allow(dead_code)]
pub fn read_ignore_updates_flag(folder: &Path, book_id: &str) -> bool {
    read_status_json(folder, book_id)
        .and_then(|v| v.get("ignore_updates")?.as_bool())
        .unwrap_or(false)
}

/// 一次性读取下载计数和忽略标志，避免重复读取同一文件。
pub fn read_status_counts_and_ignore(
    folder: &Path,
    book_id: &str,
) -> (Option<(usize, usize, usize)>, bool) {
    match read_status_json(folder, book_id) {
        Some(value) => {
            let counts = counts_from_status(&value);
            let ignored = value
                .get("ignore_updates")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            (counts, ignored)
        }
        None => (None, false),
    }
}

/// 读取并解析 status.json（或旧格式 chapter_status_<id>.json）。
fn read_status_json(folder: &Path, book_id: &str) -> Option<Value> {
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
    serde_json::from_str(&data).ok()
}

/// 从已解析的 status JSON 中提取下载计数。
fn counts_from_status(value: &Value) -> Option<(usize, usize, usize)> {
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
