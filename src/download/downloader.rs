use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use serde_json::{Map, Value};
use tracing::{debug, error, info};

use crate::base_system::book_paths;
use crate::base_system::context::Config;
use crate::base_system::cooldown_retry::fetch_with_cooldown_retry;
use crate::base_system::json_extract;
use crate::book_parser::book_manager::BookManager;
use crate::book_parser::finalize_utils;
use crate::book_parser::parser::ContentParser;
use crossbeam_channel as channel;
use std::sync::atomic::AtomicUsize;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tomato_novel_official_api::{
    ChapterRef, DirectoryClient, DirectoryMeta, FanqieClient, SearchClient,
};

fn normalize_base(base: &str) -> String {
    base.trim().trim_end_matches('/').to_string()
}

fn ensure_trailing_query_base(url: &str) -> String {
    let u = url.trim();
    if u.ends_with('?') || u.ends_with('&') {
        return u.to_string();
    }
    if u.contains('?') {
        return format!("{}&", u);
    }
    format!("{}?", u)
}

fn resolve_api_urls(
    cfg: &Config,
) -> Result<(Option<String>, Option<(String, String)>), anyhow::Error> {
    if cfg.use_official_api {
        return Ok((None, None));
    }

    let base = cfg
        .api_endpoints
        .first()
        .map(|s| s.as_str())
        .map(normalize_base)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("use_official_api=false 时，api_endpoints 不能为空"))?;

    // 目录接口（网页端）
    let directory_url = if base.contains("/api/") && base.contains("directory") {
        base.clone()
    } else {
        format!("{}/api/reader/directory/detail", base)
    };

    // 正文 batch_full + registerkey（reading 域名反代）
    let register_key_url = if base.contains("registerkey") {
        base.clone()
    } else {
        format!("{}/reading/crypt/registerkey", base)
    };
    let batch_full_url = if base.contains("batch_full") {
        ensure_trailing_query_base(&base)
    } else {
        ensure_trailing_query_base(&format!("{}/reading/reader/batch_full/v", base))
    };

    Ok((
        Some(directory_url),
        Some((register_key_url, batch_full_url)),
    ))
}

fn ms_from_connect_timeout_secs(v: f64) -> Option<u64> {
    if v <= 0.0 {
        return None;
    }
    let ms = (v * 1000.0).round() as i64;
    if ms <= 0 { None } else { Some(ms as u64) }
}

fn derive_registerkey_and_batchfull(endpoint: &str) -> (String, String) {
    let base = normalize_base(endpoint);

    // allow passing full urls too
    if base.contains("/reading/crypt/registerkey") {
        let host = base
            .split("/reading/crypt/registerkey")
            .next()
            .unwrap_or(&base);
        let rk = format!("{}/reading/crypt/registerkey", host);
        let bf = ensure_trailing_query_base(&format!("{}/reading/reader/batch_full/v", host));
        return (rk, bf);
    }

    if base.contains("/reading/reader/batch_full") {
        let host = base.split("/reading/reader/").next().unwrap_or(&base);
        let rk = format!("{}/reading/crypt/registerkey", host);
        let bf = ensure_trailing_query_base(&base);
        return (rk, bf);
    }

    let rk = format!("{}/reading/crypt/registerkey", base);
    let bf = ensure_trailing_query_base(&format!("{}/reading/reader/batch_full/v", base));
    (rk, bf)
}

fn third_party_client_for_endpoint(cfg: &Config, endpoint: &str) -> Result<FanqieClient> {
    let (rk, bf) = derive_registerkey_and_batchfull(endpoint);
    let timeout_ms = Some(cfg.request_timeout.saturating_mul(1000).max(100));
    let connect_timeout_ms = ms_from_connect_timeout_secs(cfg.min_connect_timeout);
    FanqieClient::new_with_base_urls_and_timeouts(rk, bf, timeout_ms, connect_timeout_ms)
        .map_err(|e| anyhow!(e.to_string()))
}

fn has_any_content_for_group(value: &Value, group: &[ChapterRef], cfg: &Config) -> bool {
    let parsed = ContentParser::extract_api_content(value, cfg);
    group.iter().any(|ch| {
        parsed
            .get(&ch.id)
            .map(|(content, _)| !content.trim().is_empty())
            .unwrap_or(false)
    })
}

fn validate_endpoints(cfg: &Config, probe_chapter_id: &str) -> Vec<String> {
    let mut ok = Vec::new();
    for ep in &cfg.api_endpoints {
        let ep = ep.trim();
        if ep.is_empty() {
            continue;
        }
        let client = match third_party_client_for_endpoint(cfg, ep) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let value = match client.get_contents_unthrottled(probe_chapter_id, false) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // probe 请求只含 1 个 chapter_id，用 group 校验最简单
        let probe_group = [ChapterRef {
            id: probe_chapter_id.to_string(),
            title: String::new(),
        }];
        if has_any_content_for_group(&value, &probe_group, cfg) {
            ok.push(ep.to_string());
        }
    }
    ok
}

fn sleep_backoff(cfg: &Config, attempt: u32) {
    let min_ms = cfg.min_wait_time.max(1);
    let max_ms = cfg.max_wait_time.max(min_ms);
    let shift = attempt.min(10);
    let factor = 1u64.checked_shl(shift).unwrap_or(u64::MAX);
    let mut wait = min_ms.saturating_mul(factor);
    if wait > max_ms {
        wait = max_ms;
    }
    std::thread::sleep(Duration::from_millis(wait));
}

fn fetch_group_third_party(
    cfg: &Config,
    endpoints: &Arc<std::sync::Mutex<Vec<String>>>,
    pick: &Arc<AtomicUsize>,
    group: &[ChapterRef],
    epub_mode: bool,
) -> Result<Value> {
    let tries = cfg.max_retries.max(1);
    let ids = group
        .iter()
        .map(|c| c.id.as_str())
        .collect::<Vec<_>>()
        .join(",");

    for attempt in 0..tries {
        let ep = {
            let guard = endpoints.lock().unwrap();
            if guard.is_empty() {
                return Err(anyhow!("第三方 API 地址池已为空（全部判定无效）"));
            }
            let idx = pick.fetch_add(1, Ordering::Relaxed) % guard.len();
            guard[idx].clone()
        };

        let client = third_party_client_for_endpoint(cfg, &ep)?;
        match client.get_contents_unthrottled(&ids, epub_mode) {
            Ok(v) => {
                if !has_any_content_for_group(&v, group, cfg) {
                    let mut guard = endpoints.lock().unwrap();
                    guard.retain(|x| x != &ep);
                    drop(guard);
                    sleep_backoff(cfg, attempt);
                    continue;
                }
                return Ok(v);
            }
            Err(_) => {
                sleep_backoff(cfg, attempt);
                continue;
            }
        }
    }

    Err(anyhow!("第三方 API 请求重试耗尽"))
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DownloadResult {
    pub success: u32,
    pub failed: u32,
    pub canceled: u32,
}

#[derive(Debug, Clone, Default)]
pub struct BookMeta {
    pub book_name: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub cover_url: Option<String>,
    pub detail_cover_url: Option<String>,
    pub finished: Option<bool>,
    pub chapter_count: Option<usize>,
    pub word_count: Option<usize>,
    pub score: Option<f32>,
    pub read_count: Option<String>,
    pub read_count_text: Option<String>,
    pub book_short_name: Option<String>,
    pub original_book_name: Option<String>,
    pub first_chapter_title: Option<String>,
    pub last_chapter_title: Option<String>,
    pub category: Option<String>,
    pub cover_primary_color: Option<String>,
}

impl From<DirectoryMeta> for BookMeta {
    fn from(value: DirectoryMeta) -> Self {
        Self {
            book_name: value.book_name,
            author: value.author,
            description: value.description,
            tags: value.tags,
            cover_url: value
                .cover_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .or(value.cover_url),
            detail_cover_url: value.detail_cover_url,
            finished: value.finished,
            chapter_count: value.chapter_count,
            word_count: value.word_count,
            score: value.score,
            read_count: value.read_count,
            read_count_text: value.read_count_text,
            book_short_name: value.book_short_name,
            original_book_name: value.original_book_name,
            first_chapter_title: value.first_chapter_title,
            last_chapter_title: value.last_chapter_title,
            category: value.category,
            cover_primary_color: value.cover_primary_color,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DownloadPlan {
    pub book_id: String,
    pub meta: BookMeta,
    pub chapters: Vec<ChapterRef>,
    pub _raw: Value,
}

#[derive(Debug, Clone, Copy)]
pub struct ChapterRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProgressSnapshot {
    pub group_done: usize,
    pub group_total: usize,
    pub saved_chapters: usize,
    pub chapter_total: usize,
    pub comment_fetch: usize,
    pub comment_total: usize,
    pub comment_saved: usize,
}

pub(crate) struct ProgressReporter {
    snapshot: ProgressSnapshot,
    cb: Option<Box<dyn FnMut(ProgressSnapshot) + Send>>, // optional UI callback
}

impl ProgressReporter {
    fn emit(&mut self) {
        if let Some(cb) = self.cb.as_mut() {
            cb(self.snapshot);
        }
    }

    fn inc_group(&mut self) {
        self.snapshot.group_done += 1;
        self.emit();
    }

    fn inc_saved(&mut self) {
        self.snapshot.saved_chapters += 1;
        self.emit();
    }
}

pub struct ChapterDownloader {
    _book_id: String,
    client: FanqieClient,
    config: Config,
}

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

        let use_bars = progress.cb.is_none();
        let (mut _mp, mut download_bar, mut save_bar) = if use_bars {
            let mp = MultiProgress::with_draw_target(ProgressDrawTarget::stderr());
            let style = ProgressStyle::with_template(
                "{prefix} [{elapsed_precise}] {wide_bar} {pos}/{len} ({eta})",
            )?
            .progress_chars("##-");

            let download_bar = mp.add(ProgressBar::new(total_groups));
            download_bar.set_style(style.clone());
            download_bar.set_prefix("章节下载");

            let save_bar = mp.add(ProgressBar::new(total_chapters));
            save_bar.set_style(style);
            save_bar.set_prefix("正文保存");

            (Some(mp), Some(download_bar), Some(save_bar))
        } else {
            (None, None, None)
        };

        let mut result = DownloadResult::default();

        for (group_idx, group) in groups.iter().enumerate() {
            if cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
                info!(target: "download", "收到停止信号，结束任务");
                let mut result = result;
                result.canceled = 1;
                return Err(anyhow!("用户停止下载"));
            }

            if self.config.graceful_exit {
                // 可选的优雅退出开关：预留外部信号处理
            }

            let ids = group
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>()
                .join(",");

            let value = match fetch_with_cooldown_retry(
                &self.client,
                &ids,
                self.config.novel_format == "epub",
            ) {
                Ok(v) => v,
                Err(err) => {
                    error!("批量获取章节失败: {}", err);
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
                    continue;
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
                info!(target: "download", done = saved_in_job, remaining, "保存完成 {} 章 剩 {} 章", saved_in_job, remaining);
            }

            if let Some(bar) = download_bar.as_ref() {
                bar.inc(1);
            }
            progress.inc_group();

            // 每组完成后立即落盘一次状态，保证断点续传。
            manager.save_download_status();
            let done_groups = (group_idx + 1) as u64;
            let remaining_groups = total_groups.saturating_sub(done_groups);
            info!(target: "download", done = done_groups, remaining = remaining_groups, "下载完成 {} 组 剩 {} 组", done_groups, remaining_groups);
        }

        if let Some(bar) = download_bar.take() {
            bar.finish_and_clear();
        }
        if let Some(bar) = save_bar.take() {
            bar.finish_and_clear();
        }

        let elapsed = start.elapsed().as_secs_f32();
        info!(
            "下载完成：{} 成功 {} 章，失败 {} 章，用时 {:.1}s",
            book_name, result.success, result.failed, elapsed
        );

        Ok(result)
    }
}

/// 预先拉取目录与元数据，便于 UI 展示预览/范围选择。
pub fn prepare_download_plan(
    config: &Config,
    book_id: &str,
    meta_hint: BookMeta,
) -> Result<DownloadPlan> {
    info!(target: "download", book_id, "准备下载计划");
    let directory = DirectoryClient::new().context("init DirectoryClient")?;
    let (dir_url, _content_urls) = resolve_api_urls(config)?;
    let api_url = dir_url.as_deref();

    // 首次获取目录和元数据。
    let mut dir = directory
        .fetch_directory_with_cover(book_id, api_url, None)
        .with_context(|| format!("fetch directory for book_id={book_id}"))?;

    // 如果需要封面，按实际书名构建目标路径后重新获取并下载封面。
    if dir.meta.cover_url.is_some() {
        let cover_dir =
            book_paths::book_folder_path(config, book_id, dir.meta.book_name.as_deref());
        if let Ok(with_cover) =
            directory.fetch_directory_with_cover(book_id, api_url, Some(&cover_dir))
        {
            dir = with_cover;
        }
    }

    if dir.chapters.is_empty() {
        return Err(anyhow!("目录为空"));
    }

    let meta_from_dir: BookMeta = dir.meta.into();
    // Prefer authoritative directory metadata, but keep any hints we already have
    // (e.g., search title/author) as fallback.
    let merged = merge_meta(meta_from_dir, meta_hint);
    let completed_meta =
        if merged.book_name.is_some() && merged.author.is_some() && merged.description.is_some() {
            merged
        } else {
            merge_meta(merged, search_metadata(book_id).unwrap_or_default())
        };

    Ok(DownloadPlan {
        book_id: dir.book_id.clone(),
        meta: completed_meta,
        chapters: dir.chapters,
        _raw: dir.raw,
    })
}

/// 使用已准备好的计划执行下载，并支持区间选择。
pub fn download_with_plan(
    config: &Config,
    plan: DownloadPlan,
    range: Option<ChapterRange>,
    progress: Option<Box<dyn FnMut(ProgressSnapshot) + Send>>,
    cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<()> {
    info!(target: "download", book_id = %plan.book_id, "启动下载");

    let chosen_chapters = apply_range(&plan.chapters, range);
    if chosen_chapters.is_empty() {
        return Err(anyhow!("范围无效或章节为空"));
    }

    let mut manager = init_manager_from_plan(config, &plan)?;
    let _ = manager.load_existing_status(&manager.book_id.clone(), &manager.book_name.clone());

    let pending = pending_resume(&manager, &chosen_chapters);
    let mut reporter = make_reporter(config, &chosen_chapters, &pending, progress);

    let book_name = manager.book_name.clone();
    download_chapters_into_manager(
        config,
        &plan.book_id,
        &book_name,
        &mut manager,
        &pending,
        &mut reporter,
        cancel_flag.as_ref(),
    )?;

    finalize_from_manager(&mut manager, &chosen_chapters)
}

pub(crate) fn init_manager_from_plan(config: &Config, plan: &DownloadPlan) -> Result<BookManager> {
    let meta = &plan.meta;
    let book_name = meta
        .book_name
        .clone()
        .unwrap_or_else(|| plan.book_id.clone());
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

pub(crate) fn make_reporter(
    config: &Config,
    chosen: &[ChapterRef],
    pending: &[ChapterRef],
    progress: Option<Box<dyn FnMut(ProgressSnapshot) + Send>>,
) -> ProgressReporter {
    let total = chosen.len();
    let group_total = pending.len().div_ceil(25);
    let mut reporter = ProgressReporter {
        snapshot: ProgressSnapshot {
            group_done: 0,
            group_total,
            saved_chapters: total.saturating_sub(pending.len()),
            chapter_total: total,
            comment_fetch: 0,
            comment_total: if config.enable_segment_comments {
                total
            } else {
                0
            },
            comment_saved: 0,
        },
        cb: progress,
    };
    reporter.emit();
    reporter
}

pub(crate) fn download_chapters_into_manager(
    config: &Config,
    book_id: &str,
    book_name: &str,
    manager: &mut BookManager,
    chapters: &[ChapterRef],
    reporter: &mut ProgressReporter,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<DownloadResult> {
    if chapters.is_empty() {
        info!("没有需要下载的章节，跳过下载阶段");
        reporter.snapshot.group_done = reporter.snapshot.group_total;
        reporter.snapshot.saved_chapters = reporter.snapshot.chapter_total;
        if reporter.snapshot.comment_total > 0 {
            reporter.snapshot.comment_fetch = reporter.snapshot.comment_total;
            reporter.snapshot.comment_saved = reporter.snapshot.comment_total;
        }
        reporter.emit();
        return Ok(DownloadResult::default());
    }

    debug!(target: "download", pending = chapters.len(), total = reporter.snapshot.chapter_total, "待下载章节统计");

    // 官方 API 模式：速度/冷却基本固定，网络参数意义不大，保留原逻辑。
    if config.use_official_api {
        let client = FanqieClient::new().context("init FanqieClient")?;
        let downloader = ChapterDownloader::new(book_id, config.clone(), client);
        return downloader.download_book(manager, book_name, chapters, reporter, cancel);
    }

    // 第三方 API 模式：地址池 + 预热剔除 + 并发抓取
    if config.api_endpoints.is_empty() {
        return Err(anyhow!("use_official_api=false 时，api_endpoints 不能为空"));
    }

    let probe_chapter_id = chapters.first().map(|c| c.id.as_str()).unwrap_or("");
    if probe_chapter_id.is_empty() {
        return Err(anyhow!("章节列表为空，无法预热第三方 API"));
    }

    let mut valid = validate_endpoints(config, probe_chapter_id);
    if valid.is_empty() {
        // 防御：避免探测逻辑误伤导致无法启动（后续请求阶段仍会自动剔除无效 endpoint）
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

    for group in chapters.chunks(25) {
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

        let (group, value) = match res {
            Ok(v) => v,
            Err(e) => return Err(e),
        };

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
                }
                _ => {
                    manager.save_error_chapter(&ch.id, &ch.title);
                    result.failed += 1;
                }
            }
            reporter.inc_saved();
        }
        reporter.inc_group();

        // 每组完成后立即落盘一次状态，保证断点续传。
        manager.save_download_status();
    }

    info!(target: "download", "第三方下载完成：{} ({} 章)", book_name, chapters.len());
    Ok(result)
}

pub(crate) fn finalize_from_manager(
    manager: &mut BookManager,
    chosen: &[ChapterRef],
) -> Result<()> {
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
    let cleanup_deferred = finalize_utils::run_finalize(manager, &chapter_values, result_code);
    manager.save_download_status();
    if cleanup_deferred {
        finalize_utils::perform_deferred_cleanup(manager);
    }

    if manager.end && chapter_values.len() == chosen.len() {
        if let Err(e) = manager.delete_status_folder() {
            error!(target: "book_manager", error = ?e, "删除状态目录失败");
        }
    }

    Ok(())
}

fn merge_meta(primary: BookMeta, fallback: BookMeta) -> BookMeta {
    BookMeta {
        book_name: primary.book_name.or(fallback.book_name),
        author: primary.author.or(fallback.author),
        description: primary.description.or(fallback.description),
        tags: if primary.tags.is_empty() {
            fallback.tags
        } else {
            primary.tags
        },
        cover_url: primary.cover_url.or(fallback.cover_url),
        detail_cover_url: primary.detail_cover_url.or(fallback.detail_cover_url),
        finished: primary.finished.or(fallback.finished),
        chapter_count: primary.chapter_count.or(fallback.chapter_count),
        word_count: primary.word_count.or(fallback.word_count),
        score: primary.score.or(fallback.score),
        read_count: primary.read_count.or(fallback.read_count),
        read_count_text: primary.read_count_text.or(fallback.read_count_text),
        book_short_name: primary.book_short_name.or(fallback.book_short_name),
        original_book_name: primary.original_book_name.or(fallback.original_book_name),
        first_chapter_title: primary.first_chapter_title.or(fallback.first_chapter_title),
        last_chapter_title: primary.last_chapter_title.or(fallback.last_chapter_title),
        category: primary.category.or(fallback.category),
        cover_primary_color: primary.cover_primary_color.or(fallback.cover_primary_color),
    }
}

fn search_metadata(book_id: &str) -> Option<BookMeta> {
    let client = SearchClient::new().ok()?;
    let resp = client.search_books(book_id).ok()?;
    let book = resp.books.into_iter().find(|b| b.book_id == book_id)?;
    let maps = json_extract::collect_maps(&book.raw);

    let description = maps.iter().find_map(|m| {
        json_extract::pick_string(
            m,
            &[
                "description",
                "desc",
                "abstract",
                "intro",
                "summary",
                "book_abstract",
                "recommendation_reason",
            ],
        )
    });
    let tags = maps
        .iter()
        .find_map(|m| json_extract::pick_tags_opt(m))
        .unwrap_or_default();
    let cover_url = maps.iter().find_map(|m| json_extract::pick_cover(m));
    let detail_cover_url = maps.iter().find_map(|m| json_extract::pick_detail_cover(m));
    let finished = maps.iter().find_map(|m| json_extract::pick_finished(m));
    let chapter_count = maps
        .iter()
        .find_map(|m| json_extract::pick_chapter_count(m));
    let word_count = maps.iter().find_map(|m| json_extract::pick_word_count(m));
    let score = maps.iter().find_map(|m| json_extract::pick_score(m));
    let read_count = maps.iter().find_map(|m| json_extract::pick_read_count(m));
    let read_count_text = maps
        .iter()
        .find_map(|m| json_extract::pick_read_count_text(m));
    let book_short_name = maps
        .iter()
        .find_map(|m| json_extract::pick_book_short_name(m));
    let original_book_name = maps
        .iter()
        .find_map(|m| json_extract::pick_original_book_name(m));
    let first_chapter_title = maps
        .iter()
        .find_map(|m| json_extract::pick_first_chapter_title(m));
    let last_chapter_title = maps
        .iter()
        .find_map(|m| json_extract::pick_last_chapter_title(m));
    let category = maps.iter().find_map(|m| json_extract::pick_category(m));
    let cover_primary_color = maps
        .iter()
        .find_map(|m| json_extract::pick_cover_primary_color(m));

    Some(BookMeta {
        book_name: book.title,
        author: book.author,
        description,
        tags,
        cover_url,
        detail_cover_url,
        finished,
        chapter_count,
        word_count,
        score,
        read_count,
        read_count_text,
        book_short_name,
        original_book_name,
        first_chapter_title,
        last_chapter_title,
        category,
        cover_primary_color,
    })
}

fn apply_range(chapters: &[ChapterRef], range: Option<ChapterRange>) -> Vec<ChapterRef> {
    let total = chapters.len();
    match range {
        None => chapters.to_vec(),
        Some(r) => {
            if r.start == 0 || r.start > r.end {
                return Vec::new();
            }
            let start_idx = r.start.saturating_sub(1);
            let end_idx = r.end.min(total).saturating_sub(1);
            if start_idx >= chapters.len() {
                return Vec::new();
            }
            chapters
                .iter()
                .skip(start_idx)
                .take(end_idx.saturating_sub(start_idx) + 1)
                .cloned()
                .collect()
        }
    }
}

fn extract_body_fragment(input: &str) -> String {
    let lower = input.to_lowercase();
    if let Some(body_idx) = lower.find("<body") {
        if let Some(open_end) = lower[body_idx..].find('>') {
            let start = body_idx + open_end + 1;
            if let Some(close_idx) = lower[start..].find("</body>") {
                return input[start..start + close_idx].to_string();
            }
        }
    }
    input.to_string()
}
