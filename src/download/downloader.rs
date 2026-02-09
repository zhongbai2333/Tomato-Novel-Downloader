//! 下载主流程编排。
//!
//! 负责章节批量下载、保存与断点续传、finalize 等核心编排链路。
//! 具体子模块职责参见 `mod.rs`。

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use crossbeam_channel as channel;
use serde_json::{Map, Value};
use tracing::{debug, error, info};

use crate::base_system::book_paths;
use crate::base_system::context::Config;
use crate::base_system::cooldown_retry::fetch_with_cooldown_retry;
use crate::book_parser::book_manager::BookManager;
use crate::book_parser::finalize_utils;
use crate::book_parser::parser::ContentParser;

use super::progress::{make_reporter, segment_enabled};
use super::segment_pool::{
    SegmentCommentPool, count_segment_comment_cache_files, extract_item_version_map,
};
use super::third_party::{fetch_group_third_party, validate_endpoints};

#[cfg(feature = "official-api")]
use tomato_novel_official_api::FanqieClient;

use std::sync::atomic::AtomicBool;

// ── 向后兼容重导出（外部代码通过 download::downloader::Xxx 引用）──
pub use super::models::{
    BookMeta, BookNameAsker, BookNameOption, ChapterRange, ChapterRef, DownloadFlowOptions,
    DownloadMode, DownloadPlan, DownloadResult, ProgressSnapshot, RetryFailed, SavePhase,
};
pub(crate) use super::plan::apply_range;
pub use super::plan::prepare_download_plan;
pub(crate) use super::progress::ProgressReporter;

// ── ChapterDownloader（官方 API 批量下载）──────────────────────

#[cfg(feature = "official-api")]
pub struct ChapterDownloader {
    _book_id: String,
    client: FanqieClient,
    config: Config,
}

#[cfg(feature = "official-api")]
impl ChapterDownloader {
    pub fn new(book_id: &str, config: Config, client: FanqieClient) -> Self {
        Self {
            _book_id: book_id.to_string(),
            client,
            config,
        }
    }

    /// 下载一批章节，使用官方批量接口，每批最多 25 章。
    pub fn download_book(
        &self,
        manager: &mut BookManager,
        book_name: &str,
        chapters: &[ChapterRef],
        progress: &mut ProgressReporter,
        cancel: Option<&Arc<AtomicBool>>,
        mut seg_pool: Option<&mut SegmentCommentPool>,
    ) -> Result<DownloadResult> {
        if chapters.is_empty() {
            return Ok(DownloadResult::default());
        }

        let start = Instant::now();
        info!("开始下载：{} ({} 章)", book_name, chapters.len());

        let groups: Vec<&[ChapterRef]> = chapters.chunks(25).collect();
        let total_groups = groups.len() as u64;
        let total_chapters = chapters.len() as u64;
        let mut saved_in_job: u64 = 0;

        let worker_count = self.config.max_workers.max(1);

        let use_bars =
            progress.cb.is_none() && worker_count <= 1 && progress.cli_download_bar().is_some();
        let mut download_bar = if use_bars {
            progress.cli_download_bar()
        } else {
            None
        };
        let mut save_bar = if use_bars {
            progress.cli_save_bar()
        } else {
            None
        };

        let mut result = DownloadResult::default();

        if worker_count <= 1 {
            'group_loop: for (group_idx, group) in groups.iter().enumerate() {
                if cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
                    info!(target: "download", "收到停止信号，结束任务");
                    return Err(anyhow!("用户停止下载"));
                }

                let ids = group
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>()
                    .join(",");

                let epub_mode = self.config.novel_format == "epub";
                let mut decrypt_failures = 0usize;
                let value = loop {
                    match fetch_with_cooldown_retry(&self.client, &ids, epub_mode) {
                        Ok(v) => break v,
                        Err(err) => {
                            let msg = err.to_string();
                            if msg.contains("Decryption failed") {
                                decrypt_failures += 1;
                                error!(
                                    target: "download",
                                    attempt = decrypt_failures,
                                    "批量获取章节解密失败，将强制刷新 IID/密钥并重试"
                                );
                                if let Err(e) = self.client.force_refresh_session() {
                                    error!(target: "download", error = %e, "强制刷新会话失败");
                                }
                                if decrypt_failures >= 3 {
                                    return Err(anyhow!(
                                        "内容解密失败连续 {} 次，停止下载",
                                        decrypt_failures
                                    ));
                                }
                                continue;
                            }

                            error!("批量获取章节失败: {}", msg);
                            for ch in *group {
                                manager.save_error_chapter(&ch.id, &ch.title);
                                result.failed += 1;
                                if let Some(bar) = save_bar.as_ref() {
                                    bar.inc(1);
                                }
                                progress.inc_saved();
                            }
                            if let Some(bar) = download_bar.as_ref() {
                                bar.inc(1);
                            }
                            progress.inc_group();
                            continue 'group_loop;
                        }
                    }
                };

                let parsed = ContentParser::extract_api_content(&value, &self.config);
                for ch in *group {
                    match parsed.get(&ch.id) {
                        Some((content, title)) if !content.is_empty() => {
                            let cleaned = if self.config.novel_format.eq_ignore_ascii_case("epub") {
                                extract_body_fragment(content)
                            } else {
                                content.clone()
                            };
                            manager.save_chapter(&ch.id, title, &cleaned);
                            manager.append_downloaded_chapter(&ch.id, title, &cleaned);
                            result.success += 1;
                            if let Some(pool) = seg_pool.as_mut() {
                                pool.submit(&ch.id);
                            }
                        }
                        _ => {
                            manager.save_error_chapter(&ch.id, &ch.title);
                            result.failed += 1;
                        }
                    }
                    if let Some(bar) = save_bar.as_ref() {
                        bar.inc(1);
                    }
                    progress.inc_saved();
                    saved_in_job += 1;
                    let remaining = total_chapters.saturating_sub(saved_in_job);

                    if saved_in_job.is_multiple_of(10) || remaining == 0 {
                        info!(
                            target: "download",
                            done = saved_in_job,
                            remaining,
                            "保存完成 {} 章 剩 {} 章",
                            saved_in_job,
                            remaining
                        );
                    } else {
                        debug!(
                            target: "download",
                            done = saved_in_job,
                            remaining,
                            "保存完成 {} 章 剩 {} 章",
                            saved_in_job,
                            remaining
                        );
                    }
                }

                if let Some(pool) = seg_pool.as_ref() {
                    pool.drain_progress(progress);
                }

                if let Some(bar) = download_bar.as_ref() {
                    bar.inc(1);
                }
                progress.inc_group();

                manager.save_download_status();
                let done_groups = (group_idx + 1) as u64;
                let remaining_groups = total_groups.saturating_sub(done_groups);
                info!(target: "download", done = done_groups, remaining = remaining_groups, "下载完成 {} 组 剩 {} 组", done_groups, remaining_groups);
            }
        } else {
            // 多线程模式
            let (tx_jobs, rx_jobs) = channel::unbounded::<Vec<ChapterRef>>();
            let (tx_res, rx_res) = channel::unbounded::<Result<(Vec<ChapterRef>, Value)>>();

            for group in groups.iter() {
                let _ = tx_jobs.send(group.to_vec());
            }
            drop(tx_jobs);

            for _ in 0..worker_count {
                let rx = rx_jobs.clone();
                let tx = tx_res.clone();
                let cfg = self.config.clone();
                let cancel = cancel.cloned();
                std::thread::spawn(move || {
                    let client = match FanqieClient::new() {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx.send(Err(anyhow!(e.to_string())));
                            return;
                        }
                    };
                    for group in rx.iter() {
                        if cancel
                            .as_ref()
                            .map(|c| c.load(Ordering::Relaxed))
                            .unwrap_or(false)
                        {
                            let _ = tx.send(Err(anyhow!("用户停止下载")));
                            return;
                        }
                        let ids = group
                            .iter()
                            .map(|c| c.id.as_str())
                            .collect::<Vec<_>>()
                            .join(",");

                        let epub_mode = cfg.novel_format == "epub";
                        let mut decrypt_failures = 0usize;
                        let value = loop {
                            match fetch_with_cooldown_retry(&client, &ids, epub_mode) {
                                Ok(v) => break Ok(v),
                                Err(err) => {
                                    let msg = err.to_string();
                                    if msg.contains("Decryption failed") {
                                        decrypt_failures += 1;
                                        let _ = client.force_refresh_session();
                                        if decrypt_failures >= 3 {
                                            break Err(anyhow!(
                                                "内容解密失败连续 {} 次，停止下载",
                                                decrypt_failures
                                            ));
                                        }
                                        continue;
                                    }
                                    break Err(anyhow!(msg));
                                }
                            }
                        };

                        let _ = tx.send(value.map(|v| (group, v)));
                    }
                });
            }
            drop(tx_res);

            let mut done_groups: u64 = 0;
            for res in rx_res.iter() {
                if cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
                    return Err(anyhow!("用户停止下载"));
                }

                let (group, value) = res?;

                let parsed = ContentParser::extract_api_content(&value, &self.config);
                for ch in &group {
                    match parsed.get(&ch.id) {
                        Some((content, title)) if !content.is_empty() => {
                            let cleaned = if self.config.novel_format.eq_ignore_ascii_case("epub") {
                                extract_body_fragment(content)
                            } else {
                                content.clone()
                            };
                            manager.save_chapter(&ch.id, title, &cleaned);
                            manager.append_downloaded_chapter(&ch.id, title, &cleaned);
                            result.success += 1;
                            if let Some(pool) = seg_pool.as_mut() {
                                pool.submit(&ch.id);
                            }
                        }
                        _ => {
                            manager.save_error_chapter(&ch.id, &ch.title);
                            result.failed += 1;
                        }
                    }
                    progress.inc_saved();
                    saved_in_job += 1;
                }

                if let Some(pool) = seg_pool.as_ref() {
                    pool.drain_progress(progress);
                }

                progress.inc_group();
                done_groups += 1;
                let remaining_groups = total_groups.saturating_sub(done_groups);
                let remaining_chapters = total_chapters.saturating_sub(saved_in_job);
                info!(
                    target: "download",
                    done = done_groups,
                    remaining = remaining_groups,
                    chapters_remaining = remaining_chapters,
                    "下载完成 {} 组 剩 {} 组（剩余章节约 {}）",
                    done_groups,
                    remaining_groups,
                    remaining_chapters
                );

                manager.save_download_status();
            }
        }

        let _ = download_bar.take();
        let _ = save_bar.take();

        let elapsed = start.elapsed().as_secs_f32();
        info!(
            "下载完成：{} 成功 {} 章，失败 {} 章，用时 {:.1}s",
            book_name, result.success, result.failed, elapsed
        );

        Ok(result)
    }
}

// ── 使用已准备好的计划执行下载 ──────────────────────────────────

/// 使用已准备好的计划执行下载，并支持区间选择。
#[allow(dead_code)]
pub fn download_with_plan(
    config: &Config,
    plan: DownloadPlan,
    range: Option<ChapterRange>,
    progress: Option<Box<dyn FnMut(ProgressSnapshot) + Send>>,
    cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<()> {
    download_with_plan_flow(
        config,
        plan,
        None,
        DownloadFlowOptions {
            mode: DownloadMode::Resume,
            range,
            retry_failed: RetryFailed::Never,
            stage_callback: None,
            book_name_asker: None,
        },
        progress,
        cancel_flag,
    )
}

pub fn download_with_plan_flow(
    config: &Config,
    plan: DownloadPlan,
    manager: Option<BookManager>,
    options: DownloadFlowOptions,
    progress: Option<Box<dyn FnMut(ProgressSnapshot) + Send>>,
    cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<()> {
    info!(target: "download", book_id = %plan.book_id, "启动下载");

    let DownloadFlowOptions {
        mode,
        range,
        mut retry_failed,
        mut stage_callback,
        mut book_name_asker,
    } = options;

    let chosen_chapters = apply_range(&plan.chapters, range);
    if chosen_chapters.is_empty() {
        return Err(anyhow!("范围无效或章节为空"));
    }

    let mut manager = if let Some(manager) = manager {
        manager
    } else {
        let mut manager = init_manager_from_plan(config, &plan)?;
        let _ = manager.load_existing_status(&manager.book_id.clone(), &manager.book_name.clone());
        manager
    };

    if matches!(mode, DownloadMode::Full | DownloadMode::RangeIgnoreHistory) {
        manager.downloaded.clear();
    }

    let mut pending = match mode {
        DownloadMode::FailedOnly => pending_failed(&manager, &chosen_chapters),
        _ => pending_resume(&manager, &chosen_chapters),
    };

    let mut reporter = make_reporter(config, &chosen_chapters, &pending, progress);

    loop {
        let book_name = manager.book_name.clone();
        let result = download_chapters_into_manager(
            config,
            &plan.book_id,
            &book_name,
            &mut manager,
            &chosen_chapters,
            &pending,
            Some(&plan._raw),
            &mut reporter,
            cancel_flag.as_ref(),
        )?;

        if let Some(cb) = stage_callback.as_mut() {
            cb(result);
        }

        pending = pending_failed(&manager, &chosen_chapters);
        if pending.is_empty() {
            break;
        }

        let should_retry = match retry_failed {
            RetryFailed::Never => false,
            RetryFailed::Decide(ref mut f) => f(pending.len()),
        };

        if !should_retry {
            break;
        }

        reporter.reset_for_retry(chosen_chapters.len(), pending.len());
    }

    finalize_from_manager(
        &mut manager,
        &chosen_chapters,
        Some(&plan._raw),
        Some(&mut reporter),
        cancel_flag.as_ref(),
        &mut book_name_asker,
    )
}

// ── Manager 初始化与辅助 ──────────────────────────────────────

fn rename_old_folder_if_needed(
    config: &Config,
    book_id: &str,
    new_book_name: &str,
    meta: &BookMeta,
) -> Result<()> {
    let possible_old_names = vec![
        meta.book_name.as_deref(),
        meta.original_book_name.as_deref(),
        meta.book_short_name.as_deref(),
    ];

    let new_folder = book_paths::book_folder_path(config, book_id, Some(new_book_name));

    if new_folder.exists() {
        return Ok(());
    }

    for old_name_str in possible_old_names.into_iter().flatten() {
        if old_name_str == new_book_name {
            continue;
        }

        let old_folder = book_paths::book_folder_path(config, book_id, Some(old_name_str));

        if old_folder.exists() && old_folder != new_folder {
            info!(
                target: "download",
                old = %old_folder.display(),
                new = %new_folder.display(),
                "检测到书名偏好变更，重命名文件夹"
            );

            if let Err(e) = std::fs::rename(&old_folder, &new_folder) {
                debug!(
                    target: "download",
                    error = ?e,
                    "重命名文件夹失败，将使用新文件夹"
                );
            } else {
                rename_cover_files_if_needed(&new_folder, old_name_str, new_book_name);
                return Ok(());
            }
        }
    }

    Ok(())
}

fn rename_cover_files_if_needed(folder: &Path, old_book_name: &str, new_book_name: &str) {
    use crate::base_system::context::safe_fs_name;

    let old_safe_name = safe_fs_name(old_book_name, "_", 120);
    let new_safe_name = safe_fs_name(new_book_name, "_", 120);

    if old_safe_name == new_safe_name {
        return;
    }

    let extensions = ["jpg", "jpeg", "png", "webp"];

    for ext in &extensions {
        let old_cover = folder.join(format!("{}.{}", old_safe_name, ext));
        if old_cover.exists() {
            let new_cover = folder.join(format!("{}.{}", new_safe_name, ext));
            if let Err(e) = std::fs::rename(&old_cover, &new_cover) {
                debug!(
                    target: "download",
                    error = ?e,
                    old = %old_cover.display(),
                    new = %new_cover.display(),
                    "重命名封面文件失败"
                );
            } else {
                info!(
                    target: "download",
                    old = %old_cover.display(),
                    new = %new_cover.display(),
                    "重命名封面文件"
                );
            }
        }
    }
}

pub(crate) fn init_manager_from_plan(config: &Config, plan: &DownloadPlan) -> Result<BookManager> {
    let meta = &plan.meta;
    let book_name = meta
        .book_name
        .clone()
        .unwrap_or_else(|| plan.book_id.clone());

    if let Err(e) = rename_old_folder_if_needed(config, &plan.book_id, &book_name, meta) {
        debug!(
            target: "download",
            error = ?e,
            "重命名旧文件夹失败，将继续使用新文件夹"
        );
    }

    let mut manager = BookManager::new(config.clone(), &plan.book_id, &book_name)?;
    manager.book_id = plan.book_id.clone();
    manager.book_name = book_name;
    manager.author = meta.author.clone().unwrap_or_default();
    manager.description = meta.description.clone().unwrap_or_default();
    manager.tags = meta.tags.join("|");
    manager.finished = meta.finished;
    manager.end = meta.finished.unwrap_or(false);
    manager.chapter_count = meta.chapter_count;
    manager.word_count = meta.word_count;
    manager.score = meta.score;
    manager.read_count_text = meta.read_count_text.clone();
    manager.category = meta.category.clone();
    manager.original_book_name = meta.original_book_name.clone();
    manager.book_short_name = meta.book_short_name.clone();
    Ok(manager)
}

pub(crate) fn pending_resume(manager: &BookManager, chapters: &[ChapterRef]) -> Vec<ChapterRef> {
    chapters
        .iter()
        .filter(|ch| !matches!(manager.downloaded.get(&ch.id), Some((_, Some(_)))))
        .cloned()
        .collect()
}

pub(crate) fn pending_failed(manager: &BookManager, chapters: &[ChapterRef]) -> Vec<ChapterRef> {
    chapters
        .iter()
        .filter(|ch| matches!(manager.downloaded.get(&ch.id), Some((_, None))))
        .cloned()
        .collect()
}

// ── 核心下载编排 ──────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(crate) fn download_chapters_into_manager(
    config: &Config,
    book_id: &str,
    book_name: &str,
    manager: &mut BookManager,
    chosen_chapters: &[ChapterRef],
    pending_chapters: &[ChapterRef],
    directory_raw: Option<&Value>,
    reporter: &mut ProgressReporter,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<DownloadResult> {
    // 初始化段评进度：以磁盘缓存为准，避免断点续传时"假满"。
    if segment_enabled(config) && reporter.snapshot.comment_total > 0 {
        let seg_dir = manager.book_folder().join("segment_comments");
        let _ = std::fs::create_dir_all(&seg_dir);
        let cached = count_segment_comment_cache_files(&seg_dir);
        reporter.snapshot.comment_fetch = cached.min(reporter.snapshot.comment_total);
        reporter.snapshot.comment_saved = reporter.snapshot.comment_fetch;
        reporter.emit();
    }

    if pending_chapters.is_empty() {
        info!("没有需要下载的章节，跳过下载阶段（断点续传：仅补段评缓存）");
    }

    debug!(target: "download", pending = pending_chapters.len(), total = reporter.snapshot.chapter_total, "待下载章节统计");

    let item_versions = directory_raw
        .map(extract_item_version_map)
        .unwrap_or_default();
    let status_dir = manager.book_folder().to_path_buf();
    let mut seg_pool = SegmentCommentPool::new(
        config.clone(),
        book_id.to_string(),
        status_dir,
        item_versions,
        cancel.cloned(),
    );

    // 段评与正文同时开始：先为缺失缓存的章节提交段评抓取任务。
    if let Some(pool) = seg_pool.as_ref() {
        let seg_dir = manager.book_folder().join("segment_comments");
        for ch in chosen_chapters {
            let out_path = seg_dir.join(format!("{}.json", ch.id));
            if !out_path.exists() {
                pool.submit(&ch.id);
            }
        }
    }

    if pending_chapters.is_empty() {
        if let Some(pool) = seg_pool.as_mut() {
            pool.shutdown(reporter);
        }
        reporter.snapshot.group_done = reporter.snapshot.group_total;
        reporter.snapshot.saved_chapters = reporter.snapshot.chapter_total;
        reporter.emit();
        return Ok(DownloadResult::default());
    }

    #[cfg(feature = "official-api")]
    let result = if config.use_official_api {
        let client = FanqieClient::new().context("init FanqieClient")?;
        let downloader = ChapterDownloader::new(book_id, config.clone(), client);
        downloader.download_book(
            manager,
            book_name,
            pending_chapters,
            reporter,
            cancel,
            seg_pool.as_mut(),
        )
    } else {
        download_third_party_flow(
            config,
            book_name,
            manager,
            pending_chapters,
            reporter,
            cancel,
            seg_pool.as_ref(),
        )
    };

    #[cfg(not(feature = "official-api"))]
    let result = download_third_party_flow(
        config,
        book_name,
        manager,
        pending_chapters,
        reporter,
        cancel,
        seg_pool.as_ref(),
    );

    if let Some(pool) = seg_pool.as_mut() {
        pool.shutdown(reporter);
    }

    result
}

/// 第三方 API 模式下载流程（提取以避免 `#[cfg]` 块之间代码重复）。
fn download_third_party_flow(
    config: &Config,
    book_name: &str,
    manager: &mut BookManager,
    pending_chapters: &[ChapterRef],
    reporter: &mut ProgressReporter,
    cancel: Option<&Arc<AtomicBool>>,
    seg_pool: Option<&SegmentCommentPool>,
) -> Result<DownloadResult> {
    if config.api_endpoints.is_empty() {
        return Err(anyhow!("use_official_api=false 时，api_endpoints 不能为空"));
    }

    let probe_chapter_id = pending_chapters
        .first()
        .map(|c| c.id.as_str())
        .unwrap_or("");
    if probe_chapter_id.is_empty() {
        return Err(anyhow!("章节列表为空，无法预热第三方 API"));
    }

    let mut valid = validate_endpoints(config, probe_chapter_id);
    if valid.is_empty() {
        valid = config
            .api_endpoints
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    if valid.is_empty() {
        return Err(anyhow!("第三方 API 地址池为空"));
    }

    info!(target: "download", endpoints = valid.len(), "第三方 API 地址池预热完成");

    let endpoints = Arc::new(std::sync::Mutex::new(valid));
    let picker = Arc::new(AtomicUsize::new(0));
    let worker_count = config.max_workers.max(1);
    let epub_mode = config.novel_format.eq_ignore_ascii_case("epub");

    let (tx_jobs, rx_jobs) = channel::unbounded::<Vec<ChapterRef>>();
    let (tx_res, rx_res) = channel::unbounded::<Result<(Vec<ChapterRef>, Value)>>();

    for group in pending_chapters.chunks(25) {
        tx_jobs.send(group.to_vec()).ok();
    }
    drop(tx_jobs);

    for _ in 0..worker_count {
        let rx = rx_jobs.clone();
        let tx = tx_res.clone();
        let cfg = config.clone();
        let endpoints = endpoints.clone();
        let picker = picker.clone();
        let cancel = cancel.cloned();
        std::thread::spawn(move || {
            for group in rx.iter() {
                if cancel
                    .as_ref()
                    .map(|c| c.load(Ordering::Relaxed))
                    .unwrap_or(false)
                {
                    let _ = tx.send(Err(anyhow!("用户停止下载")));
                    return;
                }
                let value = fetch_group_third_party(&cfg, &endpoints, &picker, &group, epub_mode);
                let _ = tx.send(value.map(|v| (group, v)));
            }
        });
    }
    drop(tx_res);

    let mut result = DownloadResult::default();
    for res in rx_res.iter() {
        if cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
            return Err(anyhow!("用户停止下载"));
        }

        let (group, value) = res?;

        let parsed = ContentParser::extract_api_content(&value, config);
        for ch in &group {
            match parsed.get(&ch.id) {
                Some((content, title)) if !content.is_empty() => {
                    let cleaned = if epub_mode {
                        extract_body_fragment(content)
                    } else {
                        content.clone()
                    };
                    manager.save_chapter(&ch.id, title, &cleaned);
                    manager.append_downloaded_chapter(&ch.id, title, &cleaned);
                    result.success += 1;
                    if let Some(pool) = seg_pool {
                        pool.submit(&ch.id);
                    }
                }
                _ => {
                    manager.save_error_chapter(&ch.id, &ch.title);
                    result.failed += 1;
                }
            }
            reporter.inc_saved();
        }
        reporter.inc_group();
        if let Some(pool) = seg_pool {
            pool.drain_progress(reporter);
        }

        manager.save_download_status();
    }

    info!(
        target: "download",
        "第三方下载完成：{} ({} 章)",
        book_name,
        pending_chapters.len()
    );
    Ok(result)
}

// ── Finalize ──────────────────────────────────────────────────

pub(crate) fn finalize_from_manager(
    manager: &mut BookManager,
    chosen: &[ChapterRef],
    directory_raw: Option<&Value>,
    mut reporter: Option<&mut ProgressReporter>,
    cancel: Option<&Arc<AtomicBool>>,
    book_name_asker: &mut Option<BookNameAsker>,
) -> Result<()> {
    if manager.config.is_ask_after_download()
        && !manager.book_name_selected_after_download
        && let Some(asker) = book_name_asker.as_mut()
        && let Some(chosen_name) = asker(manager)
    {
        let old_name = manager.book_name.clone();
        manager.book_name = chosen_name.clone();
        manager.book_name_selected_after_download = true;
        if old_name != chosen_name {
            rename_cover_files_if_needed(manager.book_folder(), &old_name, &chosen_name);
        }
    }

    debug!(target: "download", "保存下载状态");
    manager.save_download_status();

    let mut chapter_values = Vec::with_capacity(manager.downloaded.len());
    for ch in chosen {
        if let Some((title, Some(content))) = manager.downloaded.get(&ch.id) {
            let mut obj = Map::new();
            obj.insert("id".to_string(), Value::String(ch.id.clone()));
            obj.insert("title".to_string(), Value::String(title.clone()));
            obj.insert("content".to_string(), Value::String(content.clone()));
            chapter_values.push(Value::Object(obj));
        }
    }

    let result_code = 0;
    let reporter_ref = reporter.as_deref_mut();
    let finalize_ok = finalize_utils::run_finalize(
        manager,
        &chapter_values,
        result_code,
        directory_raw,
        reporter_ref,
        cancel,
    );
    manager.save_download_status();

    let finished = manager.finished.unwrap_or(manager.end);
    let full_book_range = manager
        .chapter_count
        .map(|n| n == chosen.len())
        .unwrap_or(false);

    if finalize_ok
        && manager.config.auto_clear_dump
        && finished
        && full_book_range
        && chapter_values.len() == chosen.len()
        && let Err(e) = manager.delete_status_folder()
    {
        error!(target: "book_manager", error = ?e, "删除状态目录失败");
    }

    if let Some(r) = reporter {
        r.finish_cli_bars();
    }

    Ok(())
}

pub(crate) fn collect_book_name_options(manager: &BookManager) -> Vec<BookNameOption> {
    let mut options: Vec<BookNameOption> = Vec::new();

    let default_name = manager.book_name.clone();
    if !default_name.is_empty() {
        options.push(BookNameOption {
            label: "默认书名".to_string(),
            value: default_name.clone(),
        });
    }

    if let Some(orig) = &manager.original_book_name
        && !orig.is_empty()
        && orig != &default_name
    {
        options.push(BookNameOption {
            label: "原始书名".to_string(),
            value: orig.clone(),
        });
    }

    if let Some(short) = &manager.book_short_name
        && !short.is_empty()
        && short != &default_name
    {
        let dup = manager
            .original_book_name
            .as_ref()
            .is_some_and(|o| o == short);
        if !dup {
            options.push(BookNameOption {
                label: "短书名".to_string(),
                value: short.clone(),
            });
        }
    }

    options
}

// ── 工具函数 ──────────────────────────────────────────────────

fn extract_body_fragment(input: &str) -> String {
    let lower = input.to_lowercase();
    if let Some(body_idx) = lower.find("<body")
        && let Some(open_end) = lower[body_idx..].find('>')
    {
        let start = body_idx + open_end + 1;
        if let Some(close_idx) = lower[start..].find("</body>") {
            return input[start..start + close_idx].to_string();
        }
    }
    input.to_string()
}
