use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use serde_json::{Map, Value};
use tracing::{debug, error, info};

use crate::base_system::context::{Config, safe_fs_name};
use crate::book_parser::book_manager::BookManager;
use crate::book_parser::finalize_utils;
use crate::book_parser::parser::ContentParser;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tomato_novel_official_api::{
    ChapterRef, DirectoryClient, DirectoryMeta, FanqieClient, SearchClient,
};

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
    pub raw: Value,
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

    fn inc_comment_fetch(&mut self) {
        self.snapshot.comment_fetch += 1;
        self.emit();
    }

    fn inc_comment_saved(&mut self) {
        self.snapshot.comment_saved += 1;
        self.emit();
    }
}

pub struct ChapterDownloader {
    book_id: String,
    client: FanqieClient,
    config: Config,
}

impl ChapterDownloader {
    pub fn new(book_id: &str, config: Config, client: FanqieClient) -> Self {
        Self {
            book_id: book_id.to_string(),
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

/// 下载整本书（用于 UI 调用，默认下载全部章节）。
pub fn download_book(config: &Config, book_id: &str) -> Result<()> {
    let plan = prepare_download_plan(config, book_id, BookMeta::default())?;
    download_with_plan(config, plan, None, None, None)
}

/// 预先拉取目录与元数据，便于 UI 展示预览/范围选择。
pub fn prepare_download_plan(
    config: &Config,
    book_id: &str,
    meta_hint: BookMeta,
) -> Result<DownloadPlan> {
    info!(target: "download", book_id, "准备下载计划");
    let directory = DirectoryClient::new().context("init DirectoryClient")?;
    let api_url = config.api_endpoints.first().map(|s| s.as_str());

    // 首次获取目录和元数据。
    let mut dir = directory
        .fetch_directory_with_cover(book_id, api_url, None)
        .with_context(|| format!("fetch directory for book_id={book_id}"))?;

    // 如果需要封面，按实际书名构建目标路径后重新获取并下载封面。
    if dir.meta.cover_url.is_some() {
        if let Some(cover_dir) = cover_dir_for(config, book_id, dir.meta.book_name.as_deref()) {
            if let Ok(with_cover) =
                directory.fetch_directory_with_cover(book_id, api_url, Some(&cover_dir))
            {
                dir = with_cover;
            }
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
        raw: dir.raw,
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
    manager.end = meta.finished.unwrap_or(false);

    let resume_book_id = manager.book_id.clone();
    let resume_book_name = manager.book_name.clone();
    let resumed = manager.load_existing_status(&resume_book_id, &resume_book_name);
    if resumed {
        info!("检测到已存在的下载状态，尝试断点续传");
    }

    let pending: Vec<ChapterRef> = chosen_chapters
        .iter()
        .cloned()
        .filter(|ch| match manager.downloaded.get(&ch.id) {
            Some((_, Some(_))) => false,
            _ => true,
        })
        .collect();

    let total = chosen_chapters.len();
    let group_total = (pending.len() + 24) / 25;
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

    if pending.is_empty() {
        info!("已全部下载，跳过下载阶段");
        reporter.snapshot.group_done = reporter.snapshot.group_total;
        reporter.snapshot.saved_chapters = total;
        if reporter.snapshot.comment_total > 0 {
            reporter.snapshot.comment_fetch = reporter.snapshot.comment_total;
            reporter.snapshot.comment_saved = reporter.snapshot.comment_total;
        }
        reporter.emit();
    } else {
        debug!(target: "download", pending = pending.len(), total = chosen_chapters.len(), "待下载章节统计");
        let client = FanqieClient::new().context("init FanqieClient")?;
        let downloader = ChapterDownloader::new(&plan.book_id, config.clone(), client);
        let book_name = manager.book_name.clone();
        let result = downloader.download_book(
            &mut manager,
            &book_name,
            &pending,
            &mut reporter,
            cancel_flag.as_ref(),
        )?;
        info!(
            "下载结束: 成功 {} 章，失败 {} 章，跳过 {} 章",
            result.success,
            result.failed,
            chosen_chapters.len() as u32 - pending.len() as u32
        );
    }

    debug!(target: "download", "保存下载状态");
    manager.save_download_status();

    // 将下载内容组装为章节列表，用于生成最终输出文件
    let mut chapter_values = Vec::with_capacity(manager.downloaded.len());
    for ch in &chosen_chapters {
        if let Some((title, Some(content))) = manager.downloaded.get(&ch.id) {
            let mut obj = Map::new();
            obj.insert("id".to_string(), Value::String(ch.id.clone()));
            obj.insert("title".to_string(), Value::String(title.clone()));
            obj.insert("content".to_string(), Value::String(content.clone()));
            chapter_values.push(Value::Object(obj));
        }
    }

    let result_code = 0; // 当前未区分失败章节输出，生成阶段以成功内容为准
    let cleanup_deferred = finalize_utils::run_finalize(&mut manager, &chapter_values, result_code);
    manager.save_download_status();
    if cleanup_deferred {
        finalize_utils::perform_deferred_cleanup(&mut manager);
    }

    if manager.end && chapter_values.len() == chosen_chapters.len() {
        if let Err(e) = manager.delete_status_folder() {
            error!(target: "book_manager", error = ?e, "删除状态目录失败");
        }
    }

    Ok(())
}

fn fetch_with_cooldown_retry(client: &FanqieClient, ids: &str, epub_mode: bool) -> Result<Value> {
    let mut delay = Duration::from_millis(1100);
    for attempt in 0..6 {
        match client.get_contents(ids, epub_mode) {
            Ok(v) => return Ok(v),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("Cooldown") || msg.contains("CooldownNotReached") {
                    std::thread::sleep(delay);
                    delay = std::cmp::min(delay * 2, Duration::from_secs(8));
                    continue;
                }
                if attempt == 0 {
                    if msg.contains("tomato_novel_network_core") || msg.contains("Library") {
                        return Err(anyhow!(
                            "{}\n\n提示：请先构建 Tomato-Novel-Network-Core，并将动态库放到当前目录或设置 FANQIE_NETWORK_CORE_DLL 指向其绝对路径。",
                            msg
                        ));
                    }
                }
                return Err(anyhow!(msg));
            }
        }
    }
    Err(anyhow!("Cooldown exceeded retries"))
}

fn extract_book_metadata(raw: &Value) -> BookMeta {
    let mut name = None;
    let mut author = None;
    let mut description = None;
    let mut tags: Vec<String> = Vec::new();
    let mut cover = None;
    let mut detail_cover = None;
    let mut finished = None;
    let mut chapter_count = None;
    let mut word_count = None;
    let mut score = None;
    let mut read_count = None;
    let mut read_count_text = None;
    let mut book_short_name = None;
    let mut original_book_name = None;
    let mut first_chapter_title = None;
    let mut last_chapter_title = None;
    let mut category = None;
    let mut cover_primary_color = None;

    let sources: Vec<&serde_json::Map<String, Value>> = raw
        .as_object()
        .into_iter()
        .flat_map(|top| {
            let mut list = vec![top];
            if let Some(info) = top.get("book_info").and_then(|v| v.as_object()) {
                list.push(info);
            }
            if let Some(info) = top.get("bookInfo").and_then(|v| v.as_object()) {
                list.push(info);
            }
            list
        })
        .collect();

    for map in &sources {
        if name.is_none() {
            name = pick_string(
                map,
                &[
                    "book_name",
                    "bookTitle",
                    "title",
                    "name",
                    "book_title",
                    "bookName",
                ],
            );
        }
        if author.is_none() {
            author = pick_string(
                map,
                &[
                    "author",
                    "author_name",
                    "authorNickname",
                    "author_nickname",
                    "author_info",
                    "creator",
                ],
            );
        }
        if description.is_none() {
            description = pick_string(
                map,
                &[
                    "description",
                    "desc",
                    "abstract",
                    "intro",
                    "summary",
                    "book_abstract",
                    "recommendation_reason",
                ],
            );
        }

        if tags.is_empty() {
            tags = pick_tags(map);
        }

        if cover.is_none() {
            cover = pick_cover(map);
        }

        if detail_cover.is_none() {
            detail_cover = pick_detail_cover(map);
        }

        if finished.is_none() {
            finished = pick_finished(map);
        }

        if chapter_count.is_none() {
            chapter_count = pick_chapter_count(map);
        }

        if word_count.is_none() {
            word_count = pick_word_count(map);
        }

        if score.is_none() {
            score = pick_score(map);
        }

        if read_count.is_none() {
            read_count = pick_read_count(map);
        }

        if read_count_text.is_none() {
            read_count_text = pick_read_count_text(map);
        }

        if book_short_name.is_none() {
            book_short_name = pick_book_short_name(map);
        }

        if original_book_name.is_none() {
            original_book_name = pick_original_book_name(map);
        }

        if first_chapter_title.is_none() {
            first_chapter_title = pick_first_chapter_title(map);
        }

        if last_chapter_title.is_none() {
            last_chapter_title = pick_last_chapter_title(map);
        }

        if category.is_none() {
            category = pick_category(map);
        }

        if cover_primary_color.is_none() {
            cover_primary_color = pick_cover_primary_color(map);
        }
    }

    BookMeta {
        book_name: name,
        author,
        description,
        tags,
        cover_url: cover,
        detail_cover_url: detail_cover,
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
    }
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
    let maps = collect_maps(&book.raw);

    let description = maps.iter().find_map(|m| {
        pick_string(
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
        .find_map(|m| Some(pick_tags(m)))
        .unwrap_or_default();
    let cover_url = maps.iter().find_map(|m| pick_cover(m));
    let detail_cover_url = maps.iter().find_map(|m| pick_detail_cover(m));
    let finished = maps.iter().find_map(|m| pick_finished(m));
    let chapter_count = maps.iter().find_map(|m| pick_chapter_count(m));
    let word_count = maps.iter().find_map(|m| pick_word_count(m));
    let score = maps.iter().find_map(|m| pick_score(m));
    let read_count = maps.iter().find_map(|m| pick_read_count(m));
    let read_count_text = maps.iter().find_map(|m| pick_read_count_text(m));
    let book_short_name = maps.iter().find_map(|m| pick_book_short_name(m));
    let original_book_name = maps.iter().find_map(|m| pick_original_book_name(m));
    let first_chapter_title = maps.iter().find_map(|m| pick_first_chapter_title(m));
    let last_chapter_title = maps.iter().find_map(|m| pick_last_chapter_title(m));
    let category = maps.iter().find_map(|m| pick_category(m));
    let cover_primary_color = maps.iter().find_map(|m| pick_cover_primary_color(m));

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

fn cover_dir_for(
    config: &Config,
    book_id: &str,
    book_name: Option<&str>,
) -> Option<std::path::PathBuf> {
    let save_dir = config.default_save_dir();
    let safe_book_id = safe_fs_name(book_id, "_", 120);
    let safe_book_name = safe_fs_name(book_name.unwrap_or(book_id), "_", 120);
    Some(save_dir.join(format!("{}_{}", safe_book_id, safe_book_name)))
}

fn collect_maps<'a>(raw: &'a Value) -> Vec<&'a serde_json::Map<String, Value>> {
    let mut maps = Vec::new();
    if let Some(map) = raw.as_object() {
        maps.push(map);
        if let Some(info) = map.get("book_info").and_then(|v| v.as_object()) {
            maps.push(info);
        }
        if let Some(info) = map.get("bookInfo").and_then(|v| v.as_object()) {
            maps.push(info);
        }
        if let Some(info) = map.get("book_data").and_then(|v| v.as_object()) {
            maps.push(info);
        }
        if let Some(info) = map.get("data").and_then(|v| v.as_object()) {
            maps.push(info);
        }
        if let Some(info) = map.get("meta").and_then(|v| v.as_object()) {
            maps.push(info);
        }
    }
    maps
}

fn pick_string(map: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(val) = map.get(*key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            } else if let Some(n) = val.as_i64() {
                return Some(n.to_string());
            }
        }
    }
    None
}

fn pick_tags(map: &serde_json::Map<String, Value>) -> Vec<String> {
    let candidates = [
        "tags",
        "book_tags",
        "tag",
        "category",
        "categories",
        "classify_tags",
    ];
    for key in candidates {
        if let Some(val) = map.get(key) {
            let out = tags_from_value(val);
            if !out.is_empty() {
                return out;
            }
        }
    }
    Vec::new()
}

fn pick_cover(map: &serde_json::Map<String, Value>) -> Option<String> {
    let candidates = [
        "cover",
        "cover_url",
        "pic_url",
        "thumb_url",
        "thumb",
        "coverUrl",
        "picUrl",
        "book_cover",
    ];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(url) = val.as_str() {
                let trimmed = url.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn pick_finished(map: &serde_json::Map<String, Value>) -> Option<bool> {
    let candidates = [
        "is_finish",
        "is_finished",
        "finish_status",
        "finishstate",
        "finish_state",
        "is_end",
        "isEnd",
        "finish",
        "finished",
        "book_status",
        "status",
    ];

    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(b) = val.as_bool() {
                return Some(b);
            }
            if let Some(n) = val.as_i64() {
                if n == 1 || n == 2 {
                    return Some(true);
                } else if n == 0 {
                    return Some(false);
                }
            }
            if let Some(s) = val.as_str() {
                if let Ok(n) = s.parse::<i64>() {
                    if n == 1 || n == 2 {
                        return Some(true);
                    } else if n == 0 {
                        return Some(false);
                    }
                }
                let lower = s.to_ascii_lowercase();
                if ["true", "yes", "finished", "end", "completed", "serial_end"]
                    .contains(&lower.as_str())
                {
                    return Some(true);
                }
                if [
                    "false",
                    "no",
                    "ongoing",
                    "serialize",
                    "serializing",
                    "serial",
                ]
                .contains(&lower.as_str())
                {
                    return Some(false);
                }
            }
        }
    }
    None
}

fn pick_chapter_count(map: &serde_json::Map<String, Value>) -> Option<usize> {
    let candidates = [
        "item_cnt",
        "book_item_cnt",
        "chapter_num",
        "chapter_count",
        "chapter_total_cnt",
        "serial_count",
        "content_chapter_number",
        "content_count",
        "total_chapter_count",
    ];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(n) = val.as_u64() {
                return Some(n as usize);
            }
            if let Some(s) = val.as_str() {
                if let Ok(n) = s.parse::<usize>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

fn pick_word_count(map: &serde_json::Map<String, Value>) -> Option<usize> {
    let candidates = ["word_number", "word_count", "word_cnt", "words"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(n) = val.as_u64() {
                return Some(n as usize);
            }
            if let Some(s) = val.as_str() {
                if let Ok(n) = s.parse::<usize>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

fn pick_score(map: &serde_json::Map<String, Value>) -> Option<f32> {
    let candidates = ["score", "book_score", "rating"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(n) = val.as_f64() {
                return Some(n as f32);
            }
            if let Some(s) = val.as_str() {
                if let Ok(n) = s.parse::<f32>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

fn pick_read_count(map: &serde_json::Map<String, Value>) -> Option<String> {
    let candidates = ["read_count", "read_count_all", "readcnt", "pv"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            } else if let Some(n) = val.as_u64() {
                return Some(n.to_string());
            }
        }
    }
    None
}

fn pick_read_count_text(map: &serde_json::Map<String, Value>) -> Option<String> {
    let candidates = ["read_cnt_text", "read_count_text"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn pick_book_short_name(map: &serde_json::Map<String, Value>) -> Option<String> {
    let candidates = ["book_short_name", "short_name", "short_title"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn pick_original_book_name(map: &serde_json::Map<String, Value>) -> Option<String> {
    let candidates = ["original_book_name", "origin_title", "original_title"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn pick_first_chapter_title(map: &serde_json::Map<String, Value>) -> Option<String> {
    let candidates = [
        "first_chapter_title",
        "first_catalog_title",
        "firstItemTitle",
    ];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn pick_last_chapter_title(map: &serde_json::Map<String, Value>) -> Option<String> {
    let candidates = [
        "last_chapter_title",
        "latest_catalog_title",
        "lastItemTitle",
    ];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn pick_category(map: &serde_json::Map<String, Value>) -> Option<String> {
    let candidates = ["category", "category_name", "book_category", "classify"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn pick_cover_primary_color(map: &serde_json::Map<String, Value>) -> Option<String> {
    let candidates = ["cover_primary_color", "primary_color", "cover_color"];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn pick_detail_cover(map: &serde_json::Map<String, Value>) -> Option<String> {
    let candidates = [
        "detail_page_thumb_url",
        "detail_thumb",
        "detail_cover",
        "detail_cover_url",
    ];
    for key in candidates {
        if let Some(val) = map.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn tags_from_value(value: &Value) -> Vec<String> {
    match value {
        Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect(),
        Value::String(s) => s
            .split(|c| c == '|' || c == ',' || c == ' ')
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string())
            .collect(),
        _ => Vec::new(),
    }
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
