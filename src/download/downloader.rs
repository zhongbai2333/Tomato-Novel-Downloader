use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use serde_json::{Map, Value};
use tracing::{error, info};

use crate::base_system::context::Config;
use crate::book_parser::book_manager::BookManager;
use crate::book_parser::finalize_utils;
use crate::book_parser::parser::ContentParser;
use tomato_novel_official_api::{ChapterRef, DirectoryClient, FanqieClient};

#[derive(Debug, Default, Clone, Copy)]
pub struct DownloadResult {
    pub success: u32,
    pub failed: u32,
    pub canceled: u32,
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
    ) -> Result<DownloadResult> {
        if chapters.is_empty() {
            return Ok(DownloadResult::default());
        }

        let start = Instant::now();
        info!("开始下载：{} ({} 章)", book_name, chapters.len());

        let groups: Vec<&[ChapterRef]> = chapters.chunks(25).collect();
        let total_groups = groups.len() as u64;
        let total_chapters = chapters.len() as u64;

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

        let mut result = DownloadResult::default();

        for group in groups {
            if self.config.graceful_exit {
                // 可选的优雅退出开关：这里仅预留，未来可接收外部信号。
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
                    for ch in group {
                        manager.save_error_chapter(&ch.id, &ch.title);
                        result.failed += 1;
                        save_bar.inc(1);
                    }
                    download_bar.inc(1);
                    continue;
                }
            };

            let parsed = ContentParser::extract_api_content(&value, &self.config);
            for ch in group {
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
                save_bar.inc(1);
            }

            download_bar.inc(1);
        }

        download_bar.finish_and_clear();
        save_bar.finish_and_clear();

        let elapsed = start.elapsed().as_secs_f32();
        info!(
            "下载完成：{} 成功 {} 章，失败 {} 章，用时 {:.1}s",
            book_name, result.success, result.failed, elapsed
        );

        Ok(result)
    }
}

/// 下载整本书（用于 UI 调用）。
pub fn download_book(config: &Config, book_id: &str) -> Result<()> {
    let directory = DirectoryClient::new().context("init DirectoryClient")?;
    let dir = directory
        .fetch_directory(book_id)
        .with_context(|| format!("fetch directory for book_id={book_id}"))?;

    if dir.chapters.is_empty() {
        return Err(anyhow!("目录为空"));
    }

    let mut manager = BookManager::new(config.clone());
    manager.book_id = dir.book_id.clone();
    let (book_name, author, description, tags) = extract_book_metadata(&dir.raw);
    manager.book_name = book_name.unwrap_or_else(|| book_id.to_string());
    manager.author = author.unwrap_or_default();
    manager.description = description.unwrap_or_default();
    manager.tags = tags;

    let existing_book_name = manager.book_name.clone();
    let resumed = manager.load_existing_status(book_id, &existing_book_name);
    if resumed {
        info!("检测到已存在的下载状态，尝试断点续传");
    }

    let pending: Vec<ChapterRef> = dir
        .chapters
        .iter()
        .cloned()
        .filter(|ch| match manager.downloaded.get(&ch.id) {
            Some((_, Some(_))) => false,
            _ => true,
        })
        .collect();

    if pending.is_empty() {
        info!("已全部下载，跳过下载阶段");
    } else {
        let client = FanqieClient::new().context("init FanqieClient")?;
        let downloader = ChapterDownloader::new(book_id, config.clone(), client);
        let book_name = manager.book_name.clone();
        let result = downloader.download_book(&mut manager, &book_name, &pending)?;
        info!(
            "下载结束: 成功 {} 章，失败 {} 章，跳过 {} 章",
            result.success,
            result.failed,
            dir.chapters.len() as u32 - pending.len() as u32
        );
    }

    manager.save_download_status();

    // 将下载内容组装为章节列表，用于生成最终输出文件
    let mut chapter_values = Vec::with_capacity(manager.downloaded.len());
    for ch in &dir.chapters {
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

fn extract_book_metadata(raw: &Value) -> (Option<String>, Option<String>, Option<String>, String) {
    let mut name = None;
    let mut author = None;
    let mut description = None;
    let mut tags: Vec<String> = Vec::new();

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
    }

    (name, author, description, tags.join("|"))
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
            let mut out = tags_from_value(val);
            if !out.is_empty() {
                return out;
            }
        }
    }
    Vec::new()
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
