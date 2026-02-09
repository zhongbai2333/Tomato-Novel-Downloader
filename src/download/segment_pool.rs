//! 段评并发下载工作池。
//!
//! 负责在下载章节正文的同时，并行抓取段落评论（segment comments）并缓存到磁盘。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use crossbeam_channel as channel;

use super::progress::ProgressReporter;
use crate::base_system::context::Config;

// 共享类型与工具函数（与 book_parser 侧去重）
#[cfg(feature = "official-api")]
pub(crate) use crate::book_parser::segment_shared::{
    SegmentCommentsChapterCache, SegmentCommentsParaCache,
};
pub(crate) use crate::book_parser::segment_shared::{
    extract_item_version_map, extract_para_counts_from_stats, write_atomic,
};

#[cfg(feature = "official-api")]
use tomato_novel_official_api::{CommentDownloadOptions, DirectoryClient, ReviewClient};

#[cfg(feature = "official-api")]
use std::sync::OnceLock;

#[cfg(feature = "official-api")]
#[derive(Debug, Clone, Copy)]
enum SegmentEvent {
    Saved,
}

pub(crate) fn count_segment_comment_cache_files(seg_dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(seg_dir) else {
        return 0;
    };
    rd.filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("json"))
                .unwrap_or(false)
        })
        .count()
}

// ── 单章段评拉取 ──────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "official-api")]
fn fetch_segment_comments_for_chapter(
    client: &ReviewClient,
    _cfg: &Config,
    book_id: &str,
    chapter_id: &str,
    item_version: &str,
    top_n: usize,
    status_dir: Option<&Path>,
    cancel: Option<&Arc<AtomicBool>>,
) -> Option<SegmentCommentsChapterCache> {
    if cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
        return None;
    }
    let stats = client
        .fetch_comment_stats(chapter_id, item_version)
        .ok()??;
    let seg_counts = extract_para_counts_from_stats(&stats);

    let mut paras: std::collections::BTreeMap<String, SegmentCommentsParaCache> =
        std::collections::BTreeMap::new();
    let mut para_with_comments: Vec<i32> = Vec::new();
    for (k, v) in seg_counts {
        let Some(idx) = k.parse::<i32>().ok() else {
            continue;
        };
        let cnt = v.as_u64().unwrap_or(0);
        paras.insert(
            k.clone(),
            SegmentCommentsParaCache {
                count: cnt,
                detail: None,
            },
        );
        if cnt > 0 {
            para_with_comments.push(idx);
        }
    }

    if para_with_comments.is_empty() {
        return Some(SegmentCommentsChapterCache {
            chapter_id: chapter_id.to_string(),
            book_id: book_id.to_string(),
            item_version: item_version.to_string(),
            top_n,
            paras,
        });
    }

    // IMPORTANT: Do NOT spawn a per-paragraph thread pool here.
    // This function is called inside a chapter-level worker pool, and per-paragraph
    // parallelism (plus per-thread media pools) can explode into hundreds/thousands
    // of concurrent requests, easily triggering IP 风控.
    //
    // Keep it sequential and rely on the outer pool for parallelism.
    let _ = status_dir; // kept for API stability (media is handled by the ReviewClient options)
    for para_idx in &para_with_comments {
        if cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
            return None;
        }
        let fetched = client
            .fetch_para_comments(chapter_id, book_id, *para_idx, item_version, top_n, 2)
            .or_else(|_| {
                client.fetch_para_comments(chapter_id, book_id, *para_idx, item_version, top_n, 0)
            });
        if let Ok(Some(res)) = fetched
            && !res.response.reviews.is_empty()
            && let Some(entry) = paras.get_mut(&para_idx.to_string())
        {
            entry.detail = Some(res.response);
        } else {
            // Soft throttle on errors to reduce burst retries when upstream starts rate-limiting.
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    Some(SegmentCommentsChapterCache {
        chapter_id: chapter_id.to_string(),
        book_id: book_id.to_string(),
        item_version: item_version.to_string(),
        top_n,
        paras,
    })
}

// ── SegmentCommentPool（official-api 版本）──────────────────────

#[cfg(feature = "official-api")]
pub(crate) struct SegmentCommentPool {
    tx: Option<channel::Sender<String>>,
    rx_evt: channel::Receiver<SegmentEvent>,
    handles: Vec<std::thread::JoinHandle<()>>,
}

#[cfg(feature = "official-api")]
impl SegmentCommentPool {
    pub(crate) fn new(
        cfg: Config,
        book_id: String,
        status_dir: PathBuf,
        item_versions: HashMap<String, String>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Option<Self> {
        use super::progress::segment_enabled;

        if !segment_enabled(&cfg) {
            return None;
        }

        // Segment comments are very request-heavy (stats + many para requests + optional media).
        // Avoid nested/high fan-out concurrency that can easily trigger IP 风控.
        let workers = cfg.segment_comments_workers.clamp(1, 8);
        let (tx, rx) = channel::unbounded::<String>();
        let (tx_evt, rx_evt) = channel::unbounded::<SegmentEvent>();

        let item_versions = Arc::new(item_versions);
        let seg_dir = status_dir.join("segment_comments");
        let _ = std::fs::create_dir_all(&seg_dir);

        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let rx = rx.clone();
            let tx_evt = tx_evt.clone();
            let cfg = cfg.clone();
            let book_id = book_id.clone();
            let status_dir = status_dir.clone();
            let item_versions = item_versions.clone();
            let cancel = cancel.clone();

            handles.push(std::thread::spawn(move || {
                // Treat cfg.media_download_workers as a global budget and distribute it across
                // segment-comment workers to avoid multiplicative explosions.
                let media_workers = {
                    let total = cfg.media_download_workers.max(1);
                    let per = total.div_ceil(workers);
                    per.clamp(1, 8)
                };
                let review_options = CommentDownloadOptions {
                    enable_comments: true,
                    download_avatars: cfg.download_comment_avatars,
                    download_images: cfg.download_comment_images,
                    media_workers,
                    status_dir: Some(status_dir.clone()),
                    media_timeout_secs: 8,
                    media_retries: 2,
                };
                let client = match ReviewClient::new(review_options) {
                    Ok(c) => c,
                    Err(_) => return,
                };

                let seg_dir = status_dir.join("segment_comments");
                let _ = std::fs::create_dir_all(&seg_dir);

                // Fetch directory (item_version map) lazily if missing.
                let dir_cache: OnceLock<HashMap<String, String>> = OnceLock::new();

                loop {
                    if cancel
                        .as_ref()
                        .map(|c| c.load(Ordering::Relaxed))
                        .unwrap_or(false)
                    {
                        return;
                    }

                    let chapter_id = match rx.recv_timeout(Duration::from_millis(200)) {
                        Ok(id) => id,
                        Err(channel::RecvTimeoutError::Timeout) => continue,
                        Err(channel::RecvTimeoutError::Disconnected) => return,
                    };

                    let out_path = seg_dir.join(format!("{}.json", chapter_id));
                    if out_path.exists() {
                        let _ = tx_evt.send(SegmentEvent::Saved);
                        continue;
                    }

                    let item_version = item_versions
                        .get(&chapter_id)
                        .cloned()
                        .or_else(|| {
                            // Fall back to a one-time directory fetch if plan raw didn't include versions.
                            dir_cache
                                .get_or_init(|| {
                                    let mut map = HashMap::new();
                                    if let Ok(c) = DirectoryClient::new()
                                        && let Ok(dir) =
                                            c.fetch_directory_with_cover(&book_id, None, None)
                                    {
                                        map = extract_item_version_map(&dir.raw);
                                    }
                                    map
                                })
                                .get(&chapter_id)
                                .cloned()
                        })
                        .unwrap_or_else(|| "0".to_string());

                    let top_n = cfg.segment_comments_top_n.max(1);

                    let cache = fetch_segment_comments_for_chapter(
                        &client,
                        &cfg,
                        &book_id,
                        &chapter_id,
                        &item_version,
                        top_n,
                        Some(&status_dir),
                        cancel.as_ref(),
                    )
                    .unwrap_or_else(|| SegmentCommentsChapterCache {
                        chapter_id: chapter_id.clone(),
                        book_id: book_id.clone(),
                        item_version: item_version.clone(),
                        top_n,
                        paras: std::collections::BTreeMap::new(),
                    });

                    // Best-effort write; 仅在落盘成功后上报进度，避免"进度跑满但还在写"。
                    if let Ok(bytes) = serde_json::to_vec(&cache)
                        && write_atomic(&out_path, &bytes).is_ok()
                    {
                        let _ = tx_evt.send(SegmentEvent::Saved);
                    }
                }
            }));
        }

        Some(Self {
            tx: Some(tx),
            rx_evt,
            handles,
        })
    }

    pub(crate) fn submit(&self, chapter_id: &str) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(chapter_id.to_string());
        }
    }

    pub(crate) fn drain_progress(&self, progress: &mut ProgressReporter) {
        for evt in self.rx_evt.try_iter() {
            match evt {
                SegmentEvent::Saved => {
                    progress.inc_comment_fetch();
                    progress.inc_comment_saved();
                }
            }
        }
    }

    pub(crate) fn shutdown(&mut self, progress: &mut ProgressReporter) {
        self.tx.take();
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
        self.drain_progress(progress);
    }
}

// ── SegmentCommentPool（非 official-api 占位版本）──────────────────

#[cfg(not(feature = "official-api"))]
pub(crate) struct SegmentCommentPool;

#[cfg(not(feature = "official-api"))]
impl SegmentCommentPool {
    pub(crate) fn new(
        _cfg: Config,
        _book_id: String,
        _status_dir: PathBuf,
        _item_versions: HashMap<String, String>,
        _cancel: Option<Arc<AtomicBool>>,
    ) -> Option<Self> {
        None
    }

    pub(crate) fn submit(&self, _chapter_id: &str) {}

    pub(crate) fn drain_progress(&self, _progress: &mut ProgressReporter) {}

    pub(crate) fn shutdown(&mut self, _progress: &mut ProgressReporter) {}
}
