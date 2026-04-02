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
use serde_json::{Map, Value, json};
use tracing::{debug, error, info};

use crate::base_system::book_paths;
use crate::base_system::context::Config;
use crate::base_system::cooldown_retry::fetch_with_cooldown_retry;
use crate::base_system::download_history::{DownloadHistoryRecord, append_download_history};
use crate::book_parser::book_manager::BookManager;
use crate::book_parser::finalize_utils;
use crate::book_parser::parser::ContentParser;

use super::progress::{make_reporter, segment_enabled};
use super::segment_pool::{
    SegmentCommentPool, count_segment_comment_cache_files, extract_item_version_map,
};
use super::third_party::{fetch_group_third_party, validate_endpoints};

#[cfg(feature = "official-api")]
use tomato_novel_official_api::{ContentFetchReport, FanqieClient};

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
    book_id: String,
    client: FanqieClient,
    config: Config,
}

#[cfg(feature = "official-api")]
#[derive(Debug, Clone)]
struct DeferredChapter {
    chapter: ChapterRef,
    reason: String,
}

#[cfg(feature = "official-api")]
impl DeferredChapter {
    fn new(chapter: ChapterRef, reason: impl Into<String>) -> Self {
        Self {
            chapter,
            reason: reason.into(),
        }
    }
}

#[cfg(feature = "official-api")]
#[derive(Debug)]
struct GroupFetchOutcome {
    group: Vec<ChapterRef>,
    value: Value,
    deferred: Vec<DeferredChapter>,
}

#[cfg(feature = "official-api")]
impl ChapterDownloader {
    pub fn new(book_id: &str, config: Config, client: FanqieClient) -> Self {
        Self {
            book_id: book_id.to_string(),
            client,
            config,
        }
    }

    /// 下载一批章节，使用官方批量接口，每批动态分组 15~25 章。
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

        let groups = build_dynamic_chapter_groups(chapters);
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
        let mut deferred_retry: Vec<DeferredChapter> = Vec::new();
        let epub_mode = self.config.novel_format == "epub";

        if worker_count <= 1 {
            for (group_idx, group) in groups.iter().enumerate() {
                if cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
                    info!(target: "download", "收到停止信号，结束任务");
                    return Err(anyhow!("用户停止下载"));
                }

                let outcome = match fetch_group_best_effort(
                    &self.client,
                    group,
                    epub_mode,
                    Some(&self.book_id),
                ) {
                    Ok(v) => v,
                    Err(err) => {
                        let reason = err.to_string();
                        info!(
                            target: "download",
                            reason = %reason,
                            count = group.len(),
                            "首轮批量拉取失败，整组章节加入延后重试队列"
                        );
                        GroupFetchOutcome {
                            group: group.to_vec(),
                            value: json!({"code": 0, "data": {}}),
                            deferred: group
                                .iter()
                                .cloned()
                                .map(|ch| DeferredChapter::new(ch, reason.clone()))
                                .collect(),
                        }
                    }
                };

                let parsed = ContentParser::extract_api_content(&outcome.value, &self.config);
                for ch in &outcome.group {
                    if let Some(deferred) = outcome
                        .deferred
                        .iter()
                        .find(|item| item.chapter.id == ch.id)
                    {
                        deferred_retry.push(deferred.clone());
                        continue;
                    }

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
                        _ => {
                            deferred_retry
                                .push(DeferredChapter::new(ch.clone(), "章节内容缺失或为空"));
                        }
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
            let (tx_res, rx_res) = channel::unbounded::<Result<GroupFetchOutcome>>();

            for group in groups.iter() {
                let _ = tx_jobs.send(group.to_vec());
            }
            drop(tx_jobs);

            for _ in 0..worker_count {
                let rx = rx_jobs.clone();
                let tx = tx_res.clone();
                let cfg = self.config.clone();
                let cancel = cancel.cloned();
                let book_id_clone = self.book_id.clone();
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
                        let epub_mode = cfg.novel_format == "epub";
                        let value = fetch_group_best_effort(
                            &client,
                            &group,
                            epub_mode,
                            Some(&book_id_clone),
                        )
                        .or_else(|err| {
                            let reason = err.to_string();
                            Ok(GroupFetchOutcome {
                                group: group.clone(),
                                value: json!({"code": 0, "data": {}}),
                                deferred: group
                                    .into_iter()
                                    .map(|ch| DeferredChapter::new(ch, reason.clone()))
                                    .collect(),
                            })
                        });

                        let _ = tx.send(value);
                    }
                });
            }
            drop(tx_res);

            let mut done_groups: u64 = 0;
            for res in rx_res.iter() {
                if cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
                    return Err(anyhow!("用户停止下载"));
                }

                let outcome = res?;

                let parsed = ContentParser::extract_api_content(&outcome.value, &self.config);
                for ch in &outcome.group {
                    if let Some(deferred) = outcome
                        .deferred
                        .iter()
                        .find(|item| item.chapter.id == ch.id)
                    {
                        deferred_retry.push(deferred.clone());
                        continue;
                    }

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
                            progress.inc_saved();
                            saved_in_job += 1;
                        }
                        _ => {
                            deferred_retry
                                .push(DeferredChapter::new(ch.clone(), "章节内容缺失或为空"));
                        }
                    }
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

        if !deferred_retry.is_empty() {
            info!(
                target: "download",
                count = deferred_retry.len(),
                "首轮下载完成，统一刷新 IID 后重试失败章节"
            );

            if let Err(e) = self.client.force_refresh_session() {
                error!(target: "download", error = %e, "统一重试前刷新 IID 失败，将继续使用当前会话重试");
            }

            for deferred in deferred_retry {
                if cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
                    return Err(anyhow!("用户停止下载"));
                }

                let chapter = deferred.chapter.clone();
                let outcome = match fetch_group_best_effort(
                    &self.client,
                    std::slice::from_ref(&chapter),
                    epub_mode,
                    Some(&self.book_id),
                ) {
                    Ok(v) => v,
                    Err(err) => GroupFetchOutcome {
                        group: vec![chapter.clone()],
                        value: json!({"code": 0, "data": {}}),
                        deferred: vec![DeferredChapter::new(chapter.clone(), err.to_string())],
                    },
                };

                let parsed = ContentParser::extract_api_content(&outcome.value, &self.config);
                if let Some((content, title)) = parsed.get(&chapter.id)
                    && !content.is_empty()
                    && outcome.deferred.is_empty()
                {
                    let cleaned = if self.config.novel_format.eq_ignore_ascii_case("epub") {
                        extract_body_fragment(content)
                    } else {
                        content.clone()
                    };
                    manager.save_chapter(&chapter.id, title, &cleaned);
                    manager.append_downloaded_chapter(&chapter.id, title, &cleaned);
                    result.success += 1;
                    if let Some(pool) = seg_pool.as_mut() {
                        pool.submit(&chapter.id);
                    }
                } else {
                    let final_reason = outcome
                        .deferred
                        .first()
                        .map(|item| item.reason.as_str())
                        .unwrap_or(deferred.reason.as_str());
                    log_failed_chapter(&chapter, final_reason);
                    manager.save_error_chapter(&chapter.id, &chapter.title);
                    result.failed += 1;
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

            manager.save_download_status();
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
        let result = match download_chapters_into_manager(
            config,
            &plan.book_id,
            &book_name,
            &mut manager,
            &chosen_chapters,
            &pending,
            Some(&plan._raw),
            &mut reporter,
            cancel_flag.as_ref(),
        ) {
            Ok(v) => v,
            Err(e) => {
                let success = count_success_for_chosen(&manager, &chosen_chapters);
                let failed = chosen_chapters.len().saturating_sub(success);
                append_download_history(&DownloadHistoryRecord::new(
                    manager.book_id.clone(),
                    manager.book_name.clone(),
                    manager.author.clone(),
                    chosen_chapters.len(),
                    success,
                    failed,
                    "failed".to_string(),
                ));
                return Err(e);
            }
        };

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

    let finalize_result = finalize_from_manager(
        &mut manager,
        &chosen_chapters,
        Some(&plan._raw),
        Some(&mut reporter),
        cancel_flag.as_ref(),
        &mut book_name_asker,
    );

    let success = count_success_for_chosen(&manager, &chosen_chapters);
    let failed = chosen_chapters.len().saturating_sub(success);
    let status = if finalize_result.is_ok() && failed == 0 {
        "success"
    } else {
        "failed"
    };
    append_download_history(&DownloadHistoryRecord::new(
        manager.book_id.clone(),
        manager.book_name.clone(),
        manager.author.clone(),
        chosen_chapters.len(),
        success,
        failed,
        status.to_string(),
    ));

    finalize_result
}

// ── Manager 初始化与辅助 ──────────────────────────────────────

fn rename_old_folder_if_needed(config: &Config, book_id: &str, new_book_name: &str) -> Result<()> {
    let new_folder = book_paths::book_folder_path(config, book_id, Some(new_book_name));

    if new_folder.exists() {
        return Ok(());
    }

    let Some(old_folder) = config.find_existing_status_folder_by_book_id(book_id, None)? else {
        return Ok(());
    };

    if old_folder != new_folder {
        let old_name = old_folder
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|name| name.split_once('_').map(|(_, title)| title.to_string()))
            .unwrap_or_default();

        info!(
            target: "download",
            old = %old_folder.display(),
            new = %new_folder.display(),
            "检测到书名变更，按 BookID 命中旧目录并重命名"
        );

        if let Err(e) = std::fs::rename(&old_folder, &new_folder) {
            debug!(
                target: "download",
                error = ?e,
                "重命名文件夹失败，将继续复用旧目录"
            );
        } else {
            rename_cover_files_if_needed(&new_folder, &old_name, new_book_name);
            return Ok(());
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

    if let Err(e) = rename_old_folder_if_needed(config, &plan.book_id, &book_name) {
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

fn count_success_for_chosen(manager: &BookManager, chapters: &[ChapterRef]) -> usize {
    chapters
        .iter()
        .filter(|ch| matches!(manager.downloaded.get(&ch.id), Some((_, Some(_)))))
        .count()
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

    for group in build_dynamic_chapter_groups(pending_chapters) {
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
                    log_failed_chapter(ch, "章节内容缺失或为空");
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
        match manager.downloaded.get(&ch.id) {
            Some((title, Some(content))) => {
                let mut obj = Map::new();
                obj.insert("id".to_string(), Value::String(ch.id.clone()));
                obj.insert("title".to_string(), Value::String(title.clone()));
                obj.insert("content".to_string(), Value::String(content.clone()));
                chapter_values.push(Value::Object(obj));
            }
            Some((title, None)) => {
                let mut obj = Map::new();
                obj.insert("id".to_string(), Value::String(ch.id.clone()));
                obj.insert("title".to_string(), Value::String(title.clone()));
                obj.insert(
                    "content".to_string(),
                    Value::String("[本章下载失败]".to_string()),
                );
                chapter_values.push(Value::Object(obj));
            }
            None => {
                let mut obj = Map::new();
                obj.insert("id".to_string(), Value::String(ch.id.clone()));
                obj.insert("title".to_string(), Value::String(ch.title.clone()));
                obj.insert(
                    "content".to_string(),
                    Value::String("[本章下载失败]".to_string()),
                );
                chapter_values.push(Value::Object(obj));
            }
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

    let all_success = count_success_for_chosen(manager, chosen) == chosen.len();
    if finalize_ok
        && manager.config.auto_clear_dump
        && finished
        && full_book_range
        && all_success
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

fn log_failed_chapter(chapter: &ChapterRef, reason: &str) {
    error!(
        target: "download",
        chapter_id = %chapter.id,
        chapter_title = %chapter.title,
        reason,
        "章节下载失败：{} ({})",
        chapter.title,
        chapter.id
    );
}

pub(crate) const MIN_DYNAMIC_GROUP_SIZE: usize = 15;
pub(crate) const MAX_DYNAMIC_GROUP_SIZE: usize = 25;

pub(crate) fn build_dynamic_chapter_groups(chapters: &[ChapterRef]) -> Vec<&[ChapterRef]> {
    if chapters.is_empty() {
        return Vec::new();
    }

    let len = chapters.len();
    if len <= MAX_DYNAMIC_GROUP_SIZE {
        return vec![chapters];
    }

    let min_groups = len.div_ceil(MAX_DYNAMIC_GROUP_SIZE);
    let max_groups = len / MIN_DYNAMIC_GROUP_SIZE;

    let group_count = if min_groups <= max_groups {
        max_groups
    } else {
        min_groups
    }
    .max(1);

    let base_size = len / group_count;
    let remainder = len % group_count;

    let mut groups = Vec::with_capacity(group_count);
    let mut start = 0;
    for idx in 0..group_count {
        let extra = usize::from(idx < remainder);
        let size = base_size + extra;
        let end = start + size;
        groups.push(&chapters[start..end]);
        start = end;
    }

    groups
}

pub(crate) fn dynamic_group_count(total: usize) -> usize {
    build_dynamic_chapter_groups(&vec![
        ChapterRef {
            id: String::new(),
            title: String::new(),
        };
        total
    ])
    .len()
}

#[cfg(feature = "official-api")]
fn fetch_group_best_effort(
    client: &FanqieClient,
    group: &[ChapterRef],
    epub_mode: bool,
    book_id: Option<&str>,
) -> Result<GroupFetchOutcome> {
    let ids = group
        .iter()
        .map(|c| c.id.as_str())
        .collect::<Vec<_>>()
        .join(",");

    let report = fetch_best_effort_with_cooldown_retry(client, &ids, epub_mode, book_id)?;

    if should_escalate_full_group_retry(group.len(), &report) {
        let reason = report
            .error
            .as_deref()
            .unwrap_or("整组章节均缺失，疑似 IID/会话异常");
        info!(
            target: "download",
            total = group.len(),
            reason,
            "检测到整组章节全部失败，立即切回整组换 IID 重试策略"
        );

        let value = fetch_with_cooldown_retry(client, &ids, epub_mode, book_id)?;
        return Ok(GroupFetchOutcome {
            group: group.to_vec(),
            value,
            deferred: Vec::new(),
        });
    }

    let deferred = map_report_to_deferred(group, &report);
    if let Some(reason) = report.error.as_deref()
        && !deferred.is_empty()
    {
        info!(
            target: "download",
            deferred = deferred.len(),
            total = group.len(),
            reason,
            "首轮下载发现缺失章节，加入延后重试队列"
        );
    }

    Ok(GroupFetchOutcome {
        group: group.to_vec(),
        value: report.value,
        deferred,
    })
}

#[cfg(feature = "official-api")]
fn fetch_best_effort_with_cooldown_retry(
    client: &FanqieClient,
    ids: &str,
    epub_mode: bool,
    book_id: Option<&str>,
) -> Result<ContentFetchReport> {
    let mut delay = std::time::Duration::from_millis(1100);
    for attempt in 0..6 {
        match client.get_contents_best_effort(ids, epub_mode, book_id) {
            Ok(v) => return Ok(v),
            Err(err) => {
                let msg = err.to_string();
                if msg.contains("Cooldown") || msg.contains("CooldownNotReached") {
                    std::thread::sleep(delay);
                    delay = std::cmp::min(delay * 2, std::time::Duration::from_secs(8));
                    continue;
                }
                if attempt == 0
                    && (msg.contains("tomato_novel_network_core") || msg.contains("Library"))
                {
                    return Err(anyhow!(
                        "{}\n\n提示：请先构建 Tomato-Novel-Network-Core，并将动态库放到当前目录或设置 FANQIE_NETWORK_CORE_DLL 指向其绝对路径。",
                        msg
                    ));
                }
                return Err(anyhow!(msg));
            }
        }
    }

    Err(anyhow!("Cooldown exceeded retries"))
}

#[cfg(feature = "official-api")]
fn map_report_to_deferred(
    group: &[ChapterRef],
    report: &ContentFetchReport,
) -> Vec<DeferredChapter> {
    let reason = report.error.as_deref().unwrap_or("章节内容缺失或为空");

    group
        .iter()
        .filter(|ch| report.missing_ids.iter().any(|id| id == &ch.id))
        .cloned()
        .map(|ch| DeferredChapter::new(ch, reason))
        .collect()
}

#[cfg(feature = "official-api")]
fn should_escalate_full_group_retry(group_len: usize, report: &ContentFetchReport) -> bool {
    group_len > 1 && report.missing_ids.len() == group_len
}

#[allow(dead_code)]
fn merge_content_values(values: Vec<Value>) -> Value {
    let mut merged = json!({
        "code": 0,
        "data": {}
    });

    for value in values {
        let Some(data_map) = value.get("data").and_then(|v| v.as_object()) else {
            continue;
        };

        if let Some(merged_map) = merged.get_mut("data").and_then(|v| v.as_object_mut()) {
            for (cid, info) in data_map {
                merged_map.insert(cid.clone(), info.clone());
            }
        }
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_content_values_combines_data_entries() {
        let merged = merge_content_values(vec![
            json!({
                "code": 0,
                "data": {
                    "1": { "content": "A", "title": "甲" }
                }
            }),
            json!({
                "code": 0,
                "data": {
                    "2": { "content": "B", "title": "乙" }
                }
            }),
        ]);

        let data = merged.get("data").and_then(|v| v.as_object()).unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(
            data.get("1")
                .and_then(|v| v.get("content"))
                .and_then(|v| v.as_str()),
            Some("A")
        );
        assert_eq!(
            data.get("2")
                .and_then(|v| v.get("content"))
                .and_then(|v| v.as_str()),
            Some("B")
        );
    }

    #[test]
    fn merge_content_values_ignores_entries_without_data_map() {
        let merged = merge_content_values(vec![
            json!({"code": 0, "data": {"1": {"content": "A"}}}),
            json!({"code": 0, "message": "bad"}),
        ]);

        let data = merged.get("data").and_then(|v| v.as_object()).unwrap();
        assert_eq!(data.len(), 1);
        assert!(data.contains_key("1"));
    }

    #[test]
    fn format_failed_log_uses_title_and_id() {
        let ch = ChapterRef {
            id: "123".to_string(),
            title: "测试章节".to_string(),
        };

        let msg = format!("章节下载失败：{} ({})", ch.title, ch.id);
        assert_eq!(msg, "章节下载失败：测试章节 (123)");
    }

    #[test]
    fn map_report_to_deferred_marks_only_missing_ids() {
        let group = vec![
            ChapterRef {
                id: "1".to_string(),
                title: "甲".to_string(),
            },
            ChapterRef {
                id: "2".to_string(),
                title: "乙".to_string(),
            },
        ];
        let report = ContentFetchReport {
            value: json!({"data": {"1": {"content": "ok"}}}),
            missing_ids: vec!["2".to_string()],
            error: Some("缺 1 章".to_string()),
        };

        let deferred = map_report_to_deferred(&group, &report);
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred[0].chapter.id, "2");
        assert_eq!(deferred[0].reason, "缺 1 章");
    }

    #[test]
    fn should_escalate_full_group_retry_only_for_complete_group_failure() {
        let all_missing = ContentFetchReport {
            value: json!({"data": {}}),
            missing_ids: vec!["1".to_string(), "2".to_string()],
            error: Some("register_key empty".to_string()),
        };
        let partial_missing = ContentFetchReport {
            value: json!({"data": {"1": {"content": "ok"}}}),
            missing_ids: vec!["2".to_string()],
            error: Some("缺 1 章".to_string()),
        };

        assert!(should_escalate_full_group_retry(2, &all_missing));
        assert!(!should_escalate_full_group_retry(2, &partial_missing));
        assert!(!should_escalate_full_group_retry(1, &all_missing));
    }

    #[test]
    fn build_dynamic_chapter_groups_keeps_group_size_within_range() {
        let chapters: Vec<ChapterRef> = (1..=80)
            .map(|i| ChapterRef {
                id: i.to_string(),
                title: format!("第{i}章"),
            })
            .collect();

        let groups = build_dynamic_chapter_groups(&chapters);
        assert_eq!(
            groups.iter().map(|g| g.len()).sum::<usize>(),
            chapters.len()
        );
        assert!(groups.iter().all(|g| (15..=25).contains(&g.len())));
        assert_eq!(groups.len(), 5);
    }

    #[test]
    fn build_dynamic_chapter_groups_allows_small_tail_as_single_group() {
        let chapters: Vec<ChapterRef> = (1..=14)
            .map(|i| ChapterRef {
                id: i.to_string(),
                title: format!("第{i}章"),
            })
            .collect();

        let groups = build_dynamic_chapter_groups(&chapters);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 14);
    }

    #[test]
    fn dynamic_group_count_matches_balanced_distribution() {
        assert_eq!(dynamic_group_count(0), 0);
        assert_eq!(dynamic_group_count(14), 1);
        assert_eq!(dynamic_group_count(25), 1);
        assert_eq!(dynamic_group_count(26), 2);
        assert_eq!(dynamic_group_count(30), 2);
        assert_eq!(dynamic_group_count(50), 3);
        assert_eq!(dynamic_group_count(80), 5);
    }

    #[test]
    fn rename_old_folder_by_book_id_even_if_api_book_name_changed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = crate::base_system::context::Config::default();
        config.save_path = temp_dir.path().display().to_string();

        let old_folder = temp_dir.path().join("123_旧书名");
        std::fs::create_dir_all(&old_folder).unwrap();
        std::fs::write(old_folder.join("status.json"), "{}\n").unwrap();

        rename_old_folder_if_needed(&config, "123", "新书名").unwrap();

        let new_folder = temp_dir.path().join("123_新书名");
        assert!(new_folder.exists());
        assert!(!old_folder.exists());
    }
}
