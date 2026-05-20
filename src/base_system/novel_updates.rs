//! 复用的“更新小说扫描”逻辑（供 TUI / Web / noui 共用）。

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone)]
pub struct NovelUpdateProgress {
    pub row: NovelUpdateRow,
    pub scanned: usize,
    pub total: usize,
}

#[derive(Debug, Clone)]
struct LocalBookStatus {
    book_id: String,
    book_name: String,
    folder: PathBuf,
    local_total: usize,
    local_failed: usize,
    is_ignored: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct UpdateCacheFile {
    #[serde(default)]
    entries: HashMap<String, CachedRemoteTotal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedRemoteTotal {
    remote_total: usize,
    checked_ms: u64,
}

const UPDATE_CACHE_FILE: &str = ".tnd_update_cache.json";
const UPDATE_CACHE_TTL_MS: u64 = 10 * 60 * 1000;
const UPDATE_SCAN_WORKERS: usize = 4;

/// 扫描保存目录下的书籍缓存文件夹（新版为 `<book_id>`，兼容旧版 `<book_id>_<book_name>`），并对比远端目录。
///
/// 备注："新章节" 以本地已知章节条目数（包含失败/空内容条目）为基准，避免把失败章误报成新章。
#[allow(dead_code)]
pub fn scan_novel_updates(save_dir: &Path) -> Result<NovelUpdateScanResult> {
    scan_novel_updates_with_progress(save_dir, |_| {})
}

/// 带进度回调的更新扫描。回调会在每本书拿到远端章节数后立即触发，适合 TUI/CLI 边扫边显示。
pub fn scan_novel_updates_with_progress<F>(
    save_dir: &Path,
    mut on_progress: F,
) -> Result<NovelUpdateScanResult>
where
    F: FnMut(NovelUpdateProgress),
{
    let local_books = collect_local_book_statuses(save_dir)?;
    if local_books.is_empty() {
        return Ok(NovelUpdateScanResult::default());
    }

    let total = local_books.len();
    let now = now_ms();
    let mut cache = load_update_cache(save_dir);
    let mut needs_refresh = Vec::new();
    let mut updates = Vec::new();
    let mut no_updates = Vec::new();
    let mut emitted = HashSet::new();
    let mut scanned = 0usize;

    let by_id: HashMap<String, LocalBookStatus> = local_books
        .iter()
        .cloned()
        .map(|book| (book.book_id.clone(), book))
        .collect();

    for book in &local_books {
        if let Some(cached) = cache.entries.get(&book.book_id) {
            let fresh = now.saturating_sub(cached.checked_ms) <= UPDATE_CACHE_TTL_MS;
            if fresh && cached.remote_total > 0 {
                record_update_row(
                    book,
                    cached.remote_total,
                    total,
                    &mut scanned,
                    &mut emitted,
                    &mut updates,
                    &mut no_updates,
                    &mut on_progress,
                );
                continue;
            }
        }
        needs_refresh.push(book.book_id.clone());
    }

    if !needs_refresh.is_empty() {
        let fetched = fetch_remote_totals_streaming(needs_refresh, |book_id, remote_total| {
            cache.entries.insert(
                book_id.clone(),
                CachedRemoteTotal {
                    remote_total,
                    checked_ms: now,
                },
            );

            if let Some(book) = by_id.get(&book_id) {
                record_update_row(
                    book,
                    remote_total,
                    total,
                    &mut scanned,
                    &mut emitted,
                    &mut updates,
                    &mut no_updates,
                    &mut on_progress,
                );
            }
        });

        // 如果本轮刷新失败但有旧缓存，先用旧缓存顶上，避免“无结果”导致 UI 看起来像书消失。
        for book in &local_books {
            if emitted.contains(&book.book_id) || fetched.contains_key(&book.book_id) {
                continue;
            }
            if let Some(cached) = cache.entries.get(&book.book_id)
                && cached.remote_total > 0
            {
                record_update_row(
                    book,
                    cached.remote_total,
                    total,
                    &mut scanned,
                    &mut emitted,
                    &mut updates,
                    &mut no_updates,
                    &mut on_progress,
                );
            }
        }

        save_update_cache(save_dir, &cache);
    }

    updates.sort_by(|a, b| b.new_count.cmp(&a.new_count));

    Ok(NovelUpdateScanResult {
        updates,
        no_updates,
    })
}

#[allow(clippy::too_many_arguments)]
fn record_update_row<F>(
    book: &LocalBookStatus,
    remote_total: usize,
    total: usize,
    scanned: &mut usize,
    emitted: &mut HashSet<String>,
    updates: &mut Vec<NovelUpdateRow>,
    no_updates: &mut Vec<NovelUpdateRow>,
    on_progress: &mut F,
) where
    F: FnMut(NovelUpdateProgress),
{
    if remote_total == 0 || !emitted.insert(book.book_id.clone()) {
        return;
    }

    let row = row_from_book(book, remote_total);
    *scanned += 1;
    on_progress(NovelUpdateProgress {
        row: row.clone(),
        scanned: *scanned,
        total,
    });

    if row.is_ignored || !row.has_update {
        no_updates.push(row);
    } else {
        updates.push(row);
    }
}

fn row_from_book(book: &LocalBookStatus, remote_total: usize) -> NovelUpdateRow {
    let new_count = remote_total.saturating_sub(book.local_total);
    let has_update = new_count > 0 || book.local_failed > 0;

    NovelUpdateRow {
        book_id: book.book_id.clone(),
        book_name: book.book_name.clone(),
        folder: book.folder.clone(),
        local_total: book.local_total,
        local_failed: book.local_failed,
        remote_total,
        new_count,
        has_update,
        is_ignored: book.is_ignored,
    }
}

fn collect_local_book_statuses(save_dir: &Path) -> Result<Vec<LocalBookStatus>> {
    if !save_dir.exists() {
        return Ok(Vec::new());
    }

    let dir_reader =
        fs::read_dir(save_dir).with_context(|| format!("read dir {}", save_dir.display()))?;

    let mut books = Vec::new();
    for entry in dir_reader.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let (book_id, legacy_name) = match parse_book_folder_name(name) {
            Some(v) => v,
            _ => continue,
        };

        // 只扫描真正有状态文件的目录；预览阶段仅有 cover.* 的缓存目录不能被误报为“已下载小说”。
        let Some(status_value) = read_status_json(&path, &book_id) else {
            continue;
        };
        let counts = counts_from_status(&status_value);
        let is_ignored = status_value
            .get("ignore_updates")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let book_name = status_value
            .get("book_name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .unwrap_or(legacy_name);
        let (local_total, _local_ok, local_failed) = counts.unwrap_or((0, 0, 0));

        books.push(LocalBookStatus {
            book_id,
            book_name,
            folder: path,
            local_total,
            local_failed,
            is_ignored,
        });
    }

    Ok(books)
}

fn load_update_cache(save_dir: &Path) -> UpdateCacheFile {
    let path = save_dir.join(UPDATE_CACHE_FILE);
    let Ok(raw) = fs::read_to_string(path) else {
        return UpdateCacheFile::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn save_update_cache(save_dir: &Path, cache: &UpdateCacheFile) {
    let path = save_dir.join(UPDATE_CACHE_FILE);
    let Ok(raw) = serde_json::to_string_pretty(cache) else {
        return;
    };
    let _ = fs::write(path, raw);
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn fetch_remote_totals_streaming<F>(
    book_ids: Vec<String>,
    mut on_result: F,
) -> HashMap<String, usize>
where
    F: FnMut(String, usize),
{
    if book_ids.is_empty() {
        return HashMap::new();
    }

    let workers = book_ids.len().clamp(1, UPDATE_SCAN_WORKERS);
    let queue = Arc::new(Mutex::new(VecDeque::from(book_ids)));
    let (tx, rx) = mpsc::channel();
    let mut handles = Vec::with_capacity(workers);

    for _ in 0..workers {
        let queue = Arc::clone(&queue);
        let tx = tx.clone();
        handles.push(thread::spawn(move || fetch_remote_totals_worker(queue, tx)));
    }
    drop(tx);

    let mut results = HashMap::new();
    for (book_id, remote_total) in rx {
        results.insert(book_id.clone(), remote_total);
        on_result(book_id, remote_total);
    }

    for handle in handles {
        let _ = handle.join();
    }

    results
}

#[cfg(feature = "official-api")]
fn fetch_remote_totals_worker(
    queue: Arc<Mutex<VecDeque<String>>>,
    tx: mpsc::Sender<(String, usize)>,
) {
    let Ok(client) = DirectoryClient::new() else {
        return;
    };
    while let Some(book_id) = queue.lock().ok().and_then(|mut q| q.pop_front()) {
        let total = client
            .fetch_directory(&book_id)
            .ok()
            .map(|d| d.chapters.len())
            .filter(|n| *n > 0);
        if let Some(total) = total {
            let _ = tx.send((book_id, total));
        }
    }
}

#[cfg(not(feature = "official-api"))]
fn fetch_remote_totals_worker(
    queue: Arc<Mutex<VecDeque<String>>>,
    tx: mpsc::Sender<(String, usize)>,
) {
    let Ok(client) = FanqieWebNetwork::new(FanqieWebConfig::default()) else {
        return;
    };
    while let Some(book_id) = queue.lock().ok().and_then(|mut q| q.pop_front()) {
        let total = client
            .fetch_chapter_list(&book_id)
            .map(|list| list.len())
            .filter(|n| *n > 0);
        if let Some(total) = total {
            let _ = tx.send((book_id, total));
        }
    }
}

fn parse_book_folder_name(name: &str) -> Option<(String, String)> {
    if name.chars().all(|c| c.is_ascii_digit()) {
        return Some((name.to_string(), name.to_string()));
    }

    let (id, title) = name.split_once('_')?;
    if id.chars().all(|c| c.is_ascii_digit()) {
        Some((id.to_string(), title.to_string()))
    } else {
        None
    }
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
#[allow(dead_code)]
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
