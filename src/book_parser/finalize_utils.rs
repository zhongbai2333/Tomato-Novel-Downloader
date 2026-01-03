//! 导出收尾（finalize）与后处理。
//!
//! 包括写入最终文件、下载评论媒体、自动打开产物等“完成后”逻辑。

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crossbeam_channel as channel;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;

use serde_json::Value;
use std::time::Instant;

use tracing::{debug, error, info, warn};

use crossterm::event::EnableMouseCapture;
use crossterm::terminal::enable_raw_mode;
use image::GenericImageView;
use regex::Regex;
use sha1::{Digest, Sha1};

use super::audio_generator::generate_audiobook;
use super::book_manager::BookManager;
use super::epub_generator::EpubGenerator;
use crate::base_system::context::safe_fs_name;
use crate::book_parser::segment_utils;

#[cfg(feature = "official-api")]
use tomato_novel_official_api::{CommentDownloadOptions, DirectoryClient, ReviewClient};

#[cfg(feature = "official-api")]
#[derive(Debug, Clone, serde::Deserialize)]
struct SegmentCommentsParaCache {
    count: u64,
    #[serde(default)]
    detail: Option<tomato_novel_official_api::ReviewResponse>,
}

#[cfg(feature = "official-api")]
#[derive(Debug, Clone, serde::Deserialize)]
struct SegmentCommentsChapterCache {
    #[allow(dead_code)]
    chapter_id: String,
    #[allow(dead_code)]
    book_id: String,
    item_version: String,
    top_n: usize,
    #[serde(default)]
    paras: BTreeMap<String, SegmentCommentsParaCache>,
}

#[cfg(feature = "official-api")]
fn load_segment_comments_cache(
    manager: &BookManager,
    chapter_id: &str,
) -> Option<SegmentCommentsChapterCache> {
    let path = manager
        .book_folder()
        .join("segment_comments")
        .join(format!("{}.json", chapter_id));
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice::<SegmentCommentsChapterCache>(&bytes).ok()
}

fn decode_xhtml_attr_url(src: &str) -> std::borrow::Cow<'_, str> {
    if src.contains("&amp;") {
        return std::borrow::Cow::Owned(src.replace("&amp;", "&"));
    }
    std::borrow::Cow::Borrowed(src)
}

fn unescape_basic_entities(s: &str) -> std::borrow::Cow<'_, str> {
    if !(s.contains("&amp;")
        || s.contains("&lt;")
        || s.contains("&gt;")
        || s.contains("&quot;")
        || s.contains("&#39;")
        || s.contains("&#x27;")
        || s.contains("&nbsp;"))
    {
        return std::borrow::Cow::Borrowed(s);
    }

    std::borrow::Cow::Owned(
        s.replace("&nbsp;", " ")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&#x27;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

/// 生成最终输出；返回是否需要延迟清理缓存。
pub fn run_finalize(
    manager: &mut BookManager,
    chapters: &[Value],
    _result: i32,
    directory_raw: Option<&Value>,
    reporter: Option<&mut crate::download::downloader::ProgressReporter>,
    cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> bool {
    info!(target: "book_manager", "finalize start: chapters={}", chapters.len());

    let mut reporter = reporter;

    let fmt = manager.config.novel_format.to_lowercase();
    let output_path = match prepare_output_path(manager, &fmt) {
        Ok(p) => p,
        Err(e) => {
            error!(target: "book_manager", error = ?e, "prepare output path failed");
            return false;
        }
    };

    let result: anyhow::Result<()> = if fmt == "txt" {
        finalize_txt(manager, chapters, &output_path)
    } else {
        let reporter_ref = {
            #[allow(clippy::needless_option_as_deref)]
            reporter.as_deref_mut()
        };
        finalize_epub(manager, chapters, &output_path, directory_raw, reporter_ref)
    };

    if let Err(e) = result {
        error!(target: "book_manager", error = ?e, "finalize failed");
        return false;
    }

    info!(target: "book_manager", "written: {}", output_path.display());

    if manager.config.auto_open_downloaded_files {
        if let Err(e) = open_in_default_app(&output_path) {
            warn!(target: "book_manager", error = ?e, "auto open downloaded file failed");
        }

        // Best-effort: re-assert TUI terminal modes after spawning external opener.
        if reporter.as_ref().is_some_and(|r| r.has_ui_callback()) {
            let _ = enable_raw_mode();
            let mut out = std::io::stdout();
            let _ = crossterm::execute!(&mut out, EnableMouseCapture);
        }
    }

    let audiobook_bar = reporter.as_ref().and_then(|r| r.cli_save_bar());
    let quiet = reporter.as_ref().is_some_and(|r| r.has_ui_callback());
    let reporter_ref = {
        #[allow(clippy::needless_option_as_deref)]
        reporter.as_deref_mut()
    };
    if !generate_audiobook(
        manager,
        chapters,
        audiobook_bar.as_ref(),
        quiet,
        reporter_ref,
        cancel,
    ) {
        warn!(target: "book_manager", "audiobook generation failed");
    }

    true
}

fn open_in_default_app(path: &Path) -> std::io::Result<()> {
    if cfg!(target_os = "windows") {
        Command::new("explorer").arg(path).spawn()?;
        return Ok(());
    }
    if cfg!(target_os = "macos") {
        Command::new("open").arg(path).spawn()?;
        return Ok(());
    }
    Command::new("xdg-open").arg(path).spawn()?;
    Ok(())
}

fn prepare_output_path(manager: &BookManager, fmt: &str) -> std::io::Result<PathBuf> {
    let raw_name = if manager.book_name.is_empty() {
        "book"
    } else {
        manager.book_name.as_str()
    };
    let safe_book = safe_fs_name(raw_name, "_", 120);
    let dir = manager.default_save_dir();
    std::fs::create_dir_all(&dir)?;

    // bulk_files: TXT 每章一个文件，输出到“小说名”文件夹
    if fmt == "txt" && manager.config.bulk_files {
        return Ok(dir.join(safe_book));
    }

    let suffix = if fmt == "epub" { "epub" } else { "txt" };
    let output_path = dir.join(format!("{}.{}", safe_book, suffix));

    // 检查文件是否已存在且不允许覆盖
    if !manager.config.allow_overwrite_files && output_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("文件已存在且配置禁止覆盖: {}", output_path.display()),
        ));
    }

    Ok(output_path)
}

fn finalize_txt(manager: &BookManager, chapters: &[Value], path: &Path) -> anyhow::Result<()> {
    if manager.config.bulk_files {
        std::fs::create_dir_all(path)?;

        // 书籍信息（用于“TXT 下载模式最开始应该是基础信息”在散装模式下也成立）
        let mut meta = File::create(path.join("0000_书籍信息.txt"))?;
        writeln!(meta, "书名：{}", manager.book_name)?;
        if !manager.author.trim().is_empty() {
            writeln!(meta, "作者：{}", manager.author)?;
        }
        writeln!(meta, "book_id={}", manager.book_id)?;

        let status_text = match manager.finished {
            Some(true) => "完结",
            Some(false) => "连载",
            None => "未知",
        };
        writeln!(meta, "状态：{}", status_text)?;

        if let Some(score) = manager.score {
            writeln!(meta, "评分：{:.1}", score)?;
        }
        if let Some(word_count) = manager.word_count {
            writeln!(meta, "字数：{}", word_count)?;
        }
        if let Some(chapter_count) = manager.chapter_count {
            writeln!(meta, "章节：{}", chapter_count)?;
        }
        if let Some(category) = manager.category.as_deref()
            && !category.trim().is_empty()
        {
            writeln!(meta, "分类：{}", category.trim())?;
        }
        if !manager.tags.trim().is_empty() {
            writeln!(meta, "标签：{}", manager.tags)?;
        }
        if let Some(read_count_text) = manager.read_count_text.as_deref()
            && !read_count_text.trim().is_empty()
        {
            writeln!(meta, "在读：{}", read_count_text.trim())?;
        }

        if !manager.description.trim().is_empty() {
            writeln!(meta)?;
            writeln!(meta, "简介：")?;
            writeln!(meta, "{}", manager.description.trim())?;
        }

        // 章节拆分
        let width = chapters.len().to_string().len().max(4);
        for (idx, ch) in chapters.iter().enumerate() {
            let title = ch.get("title").and_then(|v| v.as_str()).unwrap_or("章节");
            let content = ch.get("content").and_then(|v| v.as_str()).unwrap_or("");

            let safe_title = safe_fs_name(title, "_", 120);
            let filename = format!(
                "{num:0width$}_{title}.txt",
                num = idx + 1,
                width = width,
                title = safe_title
            );
            let mut f = File::create(path.join(filename))?;
            writeln!(f, "{}", title)?;
            writeln!(f)?;
            // Do not `trim()` here: it will remove leading full-width indent (U+3000) from the first paragraph.
            writeln!(f, "{}", content.trim_end())?;
        }

        return Ok(());
    }

    let mut f = File::create(path)?;

    writeln!(f, "书名：{}", manager.book_name)?;
    if !manager.author.trim().is_empty() {
        writeln!(f, "作者：{}", manager.author)?;
    }
    writeln!(f, "book_id={}", manager.book_id)?;

    let status_text = match manager.finished {
        Some(true) => "完结",
        Some(false) => "连载",
        None => "未知",
    };
    writeln!(f, "状态：{}", status_text)?;

    if let Some(score) = manager.score {
        writeln!(f, "评分：{:.1}", score)?;
    }
    if let Some(word_count) = manager.word_count {
        writeln!(f, "字数：{}", word_count)?;
    }
    if let Some(chapter_count) = manager.chapter_count {
        writeln!(f, "章节：{}", chapter_count)?;
    }
    if let Some(category) = manager.category.as_deref()
        && !category.trim().is_empty()
    {
        writeln!(f, "分类：{}", category.trim())?;
    }
    if !manager.tags.trim().is_empty() {
        writeln!(f, "标签：{}", manager.tags)?;
    }
    if let Some(read_count_text) = manager.read_count_text.as_deref()
        && !read_count_text.trim().is_empty()
    {
        writeln!(f, "在读：{}", read_count_text.trim())?;
    }

    if !manager.description.trim().is_empty() {
        writeln!(f)?;
        writeln!(f, "简介：")?;
        writeln!(f, "{}", manager.description.trim())?;
    }

    writeln!(f)?;
    writeln!(f, "{}", "=".repeat(40))?;
    writeln!(f)?;

    for ch in chapters {
        let title = ch.get("title").and_then(|v| v.as_str()).unwrap_or("章节");
        let content = ch.get("content").and_then(|v| v.as_str()).unwrap_or("");
        writeln!(f, "{}\n", title)?;
        // Do not `trim()` here: it will remove leading full-width indent (U+3000) from the first paragraph.
        writeln!(f, "{}\n", content.trim_end())?;
        writeln!(f, "\n----------------------------------------\n")?;
    }
    Ok(())
}

fn extract_item_version_map(directory_raw: &Value) -> HashMap<String, String> {
    fn pick_string_or_number(v: Option<&Value>) -> Option<String> {
        match v {
            Some(Value::String(s)) => {
                let t = s.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            }
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        }
    }

    let mut out = HashMap::new();
    let candidates = [
        directory_raw.get("catalog_data"),
        directory_raw.get("item_data_list"),
        directory_raw.get("items"),
    ];

    for arr in candidates {
        let Some(arr) = arr.and_then(Value::as_array) else {
            continue;
        };

        for item in arr {
            let Some(obj) = item.as_object() else {
                continue;
            };
            let id = pick_string_or_number(
                obj.get("item_id")
                    .or_else(|| obj.get("catalog_id"))
                    .or_else(|| obj.get("id")),
            );
            let version = pick_string_or_number(
                obj.get("item_version")
                    .or_else(|| obj.get("version"))
                    .or_else(|| obj.get("item_version_code"))
                    .or_else(|| obj.get("item_version_str")),
            );
            let (Some(id), Some(version)) = (id, version) else {
                continue;
            };
            if !id.is_empty() && !version.is_empty() {
                out.insert(id, version);
            }
        }
    }

    out
}

fn extract_para_counts_from_stats(stats: &Value) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();

    fn pick_i64(v: Option<&Value>) -> Option<i64> {
        match v {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
            _ => None,
        }
    }

    fn push_from_array(out: &mut serde_json::Map<String, Value>, arr: &[Value]) {
        for item in arr {
            let Some(obj) = item.as_object() else {
                continue;
            };
            let idx = pick_i64(
                obj.get("para_index")
                    .or_else(|| obj.get("para_idx"))
                    .or_else(|| obj.get("index"))
                    .or_else(|| obj.get("para_id"))
                    .or_else(|| obj.get("paraId")),
            );
            let cnt = pick_i64(
                obj.get("count")
                    .or_else(|| obj.get("comment_count"))
                    .or_else(|| obj.get("commentCount"))
                    .or_else(|| obj.get("idea_count"))
                    .or_else(|| obj.get("total")),
            );

            let (Some(idx), Some(cnt)) = (idx, cnt) else {
                continue;
            };
            if idx < 0 || cnt <= 0 {
                continue;
            }

            out.insert(
                idx.to_string(),
                Value::Number(serde_json::Number::from(cnt as u64)),
            );
        }
    }

    // Python's comment stats shape (and observed app traffic) often returns an object map:
    // {
    //   "data": {
    //     "0": {"count": 3, ...},
    //     "1": {"count": 0, ...}
    //   },
    //   "extra": {...}
    // }
    // Or sometimes the stats itself is the map {"0": {...}, ...}.
    fn push_from_index_object_map(
        out: &mut serde_json::Map<String, Value>,
        obj: &serde_json::Map<String, Value>,
    ) {
        for (k, v) in obj {
            let Ok(idx) = k.parse::<i64>() else {
                continue;
            };
            if idx < 0 {
                continue;
            }

            let cnt = match v {
                Value::Object(m) => pick_i64(
                    m.get("count")
                        .or_else(|| m.get("comment_count"))
                        .or_else(|| m.get("commentCount"))
                        .or_else(|| m.get("idea_count"))
                        .or_else(|| m.get("total")),
                ),
                Value::Number(_) | Value::String(_) => pick_i64(Some(v)),
                _ => None,
            };

            let Some(cnt) = cnt else {
                continue;
            };
            if cnt <= 0 {
                continue;
            }

            out.insert(
                idx.to_string(),
                Value::Number(serde_json::Number::from(cnt as u64)),
            );
        }
    }

    // Try object-map forms early.
    if let Some(obj) = stats.as_object() {
        push_from_index_object_map(&mut out, obj);
        if !out.is_empty() {
            return out;
        }
    }
    if let Some(obj) = stats.get("data").and_then(Value::as_object) {
        push_from_index_object_map(&mut out, obj);
        if !out.is_empty() {
            return out;
        }
    }

    // Extra robust:
    // - Some variants return { paras: {"0": {count:..}, ... } }
    if let Some(paras) = stats.get("paras").and_then(|v| v.as_object()) {
        for (k, v) in paras {
            let cnt = pick_i64(v.get("count").or_else(|| v.get("comment_count")));
            if let (Ok(idx), Some(cnt)) = (k.parse::<i64>(), cnt)
                && idx >= 0
                && cnt > 0
            {
                out.insert(
                    idx.to_string(),
                    Value::Number(serde_json::Number::from(cnt as u64)),
                );
            }
        }
        if !out.is_empty() {
            return out;
        }
    }

    let candidates = [
        stats.get("data_list"),
        stats.get("list"),
        stats.get("idea_list"),
        stats.get("ideas"),
        stats.get("detail").and_then(|d| d.get("data_list")),
        stats.get("detail").and_then(|d| d.get("list")),
    ];

    for v in candidates {
        if let Some(arr) = v.and_then(Value::as_array) {
            push_from_array(&mut out, arr);
        }
    }

    // Fallback: sometimes stats is nested under {"data": ...}
    if out.is_empty()
        && let Some(inner) = stats.get("data")
    {
        if let Some(arr) = inner.get("data_list").and_then(Value::as_array) {
            push_from_array(&mut out, arr);
        }
        if let Some(arr) = inner.get("list").and_then(Value::as_array) {
            push_from_array(&mut out, arr);
        }
    }

    out
}

#[cfg(feature = "official-api")]
fn prefetch_comment_media(
    cfg: &crate::base_system::context::Config,
    per_para: &[(i32, tomato_novel_official_api::ReviewResponse)],
    images_dir: &Path,
) {
    if !(cfg.download_comment_images || cfg.download_comment_avatars) {
        return;
    }

    let mut urls: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for (_para_idx, resp) in per_para {
        for item in &resp.reviews {
            if cfg.download_comment_avatars
                && let Some(url) = item.user.avatar.as_deref()
            {
                let u = url.trim();
                if !u.is_empty() && seen.insert(u.to_string()) {
                    urls.push(u.to_string());
                }
            }
            if cfg.download_comment_images {
                for img in &item.images {
                    let u = img.url.trim();
                    if !u.is_empty() && seen.insert(u.to_string()) {
                        urls.push(u.to_string());
                    }
                }
            }
        }
    }

    if urls.is_empty() {
        return;
    }

    // Respect per-chapter cap (0 means no cap).
    if cfg.media_limit_per_chapter > 0 {
        urls.truncate(cfg.media_limit_per_chapter);
    }

    let workers = cfg.media_download_workers.clamp(1, 64);
    let worker_count = workers.min(urls.len().max(1));
    if worker_count <= 1 {
        for u in urls {
            let _ = ensure_cached_image(cfg, &u, images_dir);
        }
        return;
    }

    let (tx, rx) = channel::unbounded::<String>();
    for u in urls {
        let _ = tx.send(u);
    }
    drop(tx);

    let mut handles = Vec::with_capacity(worker_count);
    for _ in 0..worker_count {
        let rx = rx.clone();
        let images_dir = images_dir.to_path_buf();
        let cfg = cfg.clone();
        handles.push(std::thread::spawn(move || {
            for u in rx.iter() {
                let _ = ensure_cached_image(&cfg, &u, &images_dir);
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
}

#[cfg(feature = "official-api")]
#[allow(clippy::too_many_arguments)]
fn render_segment_comment_page(
    chapter_title: &str,
    chapter_file: &str,
    chapter_html: &str,
    per_para: &[(i32, tomato_novel_official_api::ReviewResponse)],
    cfg: &crate::base_system::context::Config,
    resources_added: &mut HashSet<String>,
    images_dir: &Path,
    epub: &mut EpubGenerator,
) -> anyhow::Result<String> {
    let mut avatar_used = 0usize;
    let mut image_used = 0usize;

    // Mirror Python `render_segment_comments_xhtml` output structure.
    let mut html = String::new();
    html.push_str(&format!("<h2>{} - 段评</h2>", escape_html(chapter_title)));

    for (para_idx, resp) in per_para {
        let idx_usize = (*para_idx).max(0) as usize;
        // Prefer API-provided para_content if present (like Python version); fallback to extract from chapter.
        let snippet = resp
            .meta
            .para_content
            .as_deref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| segment_utils::extract_para_snippet(chapter_html, idx_usize));
        let snippet = segment_utils::convert_bracket_emojis(&snippet);

        let disp_idx = idx_usize + 1;
        let cjk_idx = segment_utils::to_cjk_numeral(disp_idx as i32);
        let title_html = if !snippet.trim().is_empty() {
            format!(
                "<span class=\"para-title\"><span class=\"para-index\">{}、</span> <span class=\"para-src\">&quot;{}&quot;</span></span>",
                escape_html(&cjk_idx),
                escape_html(snippet.trim())
            )
        } else {
            format!(
                "<span class=\"para-title\">第 {} 段</span>",
                escape_html(&disp_idx.to_string())
            )
        };
        html.push_str(&format!(
            "<h3 id=\"para-{}\">{}</h3>",
            idx_usize, title_html
        ));
        html.push_str(&format!(
            "<div class=\"back-to-chapter\"><a href=\"{}#p-{}\">↩ 回到正文</a></div>",
            escape_html(chapter_file),
            idx_usize
        ));

        html.push_str("<ol>");
        for item in &resp.reviews {
            let user = item
                .user
                .name
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("匿名");
            let text = segment_utils::convert_bracket_emojis(&item.text);
            if text.trim().is_empty() {
                continue;
            }
            // Python: convert_bracket_emojis then html.escape.
            let text = escape_html(text.trim());

            let mut avatar_html = String::new();
            if cfg.download_comment_avatars
                && let Some(url) = item.user.avatar.as_deref()
            {
                if let Ok(Some((path, mime, ext))) = ensure_cached_image(cfg, url, images_dir) {
                    let hash = sha1_hex(url);
                    let resource_path = format!("images/{}{}", hash, ext);
                    if !resources_added.contains(&resource_path)
                        && let Ok(bytes) = fs::read(&path)
                        && epub.add_resource_bytes(&resource_path, bytes, mime).is_ok()
                    {
                        resources_added.insert(resource_path.clone());
                    }
                    if resources_added.contains(&resource_path) {
                        avatar_html = format!(
                            "<img class=\"avatar\" alt=\"\" src=\"{}\"/>",
                            escape_html(&resource_path)
                        );
                        avatar_used += 1;
                    } else {
                        debug!(target: "segment", url = %url, "avatar not added to epub resources (read/add_resource failed)");
                    }
                } else {
                    debug!(target: "segment", url = %url, "avatar ensure_cached_image failed/empty");
                }
            }

            let mut images_html = String::new();
            if cfg.download_comment_images {
                let mut imgs = Vec::new();
                for img in &item.images {
                    let url = img.url.trim();
                    if url.is_empty() {
                        continue;
                    }
                    if let Ok(Some((path, mime, ext))) = ensure_cached_image(cfg, url, images_dir) {
                        let hash = sha1_hex(url);
                        let resource_path = format!("images/{}{}", hash, ext);
                        if !resources_added.contains(&resource_path)
                            && let Ok(bytes) = fs::read(&path)
                            && epub.add_resource_bytes(&resource_path, bytes, mime).is_ok()
                        {
                            resources_added.insert(resource_path.clone());
                        }
                        if resources_added.contains(&resource_path) {
                            imgs.push(format!(
                                "<img alt=\"\" src=\"{}\"/>",
                                escape_html(&resource_path)
                            ));
                            image_used += 1;
                        } else {
                            debug!(target: "segment", url = %url, "comment image not added to epub resources (read/add_resource failed)");
                        }
                    } else {
                        debug!(target: "segment", url = %url, "comment image ensure_cached_image failed/empty");
                    }
                }
                if !imgs.is_empty() {
                    images_html = format!("<div class=\"seg-images\">{}</div>", imgs.join(""));
                }
            }

            // Python layout: <li><p>text</p><div class=seg-images>...</div><p><small class=seg-meta>...</small></p></li>
            let mut meta_line = String::new();
            meta_line.push_str("<small class=\"seg-meta\">");
            meta_line.push_str(&avatar_html);
            meta_line.push_str(&format!("作者：{}", escape_html(user)));

            if let Some(ts) = item.created_ts {
                // Same heuristic as Python: ms -> s if needed.
                let mut t = ts;
                if t > 1_000_000_000_000 {
                    t /= 1000;
                }
                if t > 0 {
                    // keep it simple (avoid extra deps): show epoch minutes.
                    meta_line.push_str(&format!(" | 时间：{}", escape_html(&t.to_string())));
                }
            }
            meta_line.push_str(&format!(" | 赞：{}", item.digg_count));
            meta_line.push_str("</small>");

            html.push_str("<li class=\"seg-item\">");
            html.push_str(&format!("<p>{}</p>", text));
            if !images_html.is_empty() {
                html.push_str(&images_html);
            }
            html.push_str(&format!("<p>{}</p>", meta_line));
            html.push_str("</li>");
        }
        html.push_str("</ol>");
    }

    let top_n_cfg = cfg.segment_comments_top_n.max(1);
    html.push_str(&format!(
        "<p><small>仅展示每段前 {} 条评论（若有），实际总数以接口为准。</small></p>",
        top_n_cfg
    ));

    info!(
        target: "segment",
        chapter = %chapter_title,
        para_groups = per_para.len(),
        avatar_used,
        image_used,
        download_comment_avatars = cfg.download_comment_avatars,
        download_comment_images = cfg.download_comment_images,
        "segment comment page rendered"
    );

    Ok(html)
}

fn finalize_epub(
    manager: &BookManager,
    chapters: &[Value],
    path: &Path,
    directory_raw: Option<&Value>,
    mut reporter: Option<&mut crate::download::downloader::ProgressReporter>,
) -> anyhow::Result<()> {
    let mut epub_gen = EpubGenerator::new(
        &manager.book_id,
        &manager.book_name,
        &manager.author,
        &manager.tags,
        &manager.description,
        &manager.config,
    )?;

    info!(
        target: "segment",
        enable_segment_comments = manager.config.enable_segment_comments,
        novel_format = %manager.config.novel_format,
        use_official_api = manager.config.use_official_api,
        top_n = manager.config.segment_comments_top_n,
        workers = manager.config.segment_comments_workers,
        download_comment_images = manager.config.download_comment_images,
        download_comment_avatars = manager.config.download_comment_avatars,
        directory_raw_present = directory_raw.is_some(),
        "segment comment pipeline start"
    );

    // 图片断点续传：先下载到该书临时目录 images/，生成时再从本地导入。
    let images_dir = manager.book_folder().join("images");
    fs::create_dir_all(&images_dir)?;

    // Cache: url -> (local_path, mime, ext) (avoid re-fetch across chapters)
    let mut image_cache: HashMap<String, (PathBuf, &'static str, &'static str)> = HashMap::new();
    // Track resources already added to epub (avoid duplicate add_resource)
    let mut resources_added: HashSet<String> = HashSet::new();

    // 简单的介绍页
    let intro_html = format!(
        "<p>书名：{}</p><p>作者：{}</p><p>标签：{}</p><p>简介：{}</p>",
        escape_html(&manager.book_name),
        escape_html(&manager.author),
        escape_html(&manager.tags),
        escape_html(&manager.description)
    );
    let _ = epub_gen.add_aux_page("简介", &intro_html, true);

    #[cfg(feature = "official-api")]
    let enable_segment_comments = manager.config.enable_segment_comments
        && manager.config.novel_format.eq_ignore_ascii_case("epub");
    #[cfg(not(feature = "official-api"))]
    let enable_segment_comments = false;

    #[cfg(feature = "official-api")]
    let mut item_versions = directory_raw
        .map(extract_item_version_map)
        .unwrap_or_default();

    #[cfg(not(feature = "official-api"))]
    let item_versions: HashMap<String, String> = HashMap::new();

    info!(
        target: "segment",
        item_versions = item_versions.len(),
        "item_version map prepared"
    );

    // If we don't have versions (common when directory came from a third-party mirror),
    // lazily fetch the official directory once to obtain item_version mapping.
    #[cfg(feature = "official-api")]
    let mut official_dir_fetched = false;

    #[cfg(feature = "official-api")]
    let review_options = CommentDownloadOptions {
        enable_comments: enable_segment_comments,
        download_avatars: false,
        download_images: false,
        media_workers: 1,
        status_dir: None,
        media_timeout_secs: 8,
        media_retries: 2,
    };

    #[cfg(feature = "official-api")]
    let review_client = if enable_segment_comments {
        match ReviewClient::new(review_options.clone()) {
            Ok(c) => {
                info!(target: "segment", "ReviewClient initialized");
                Some(c)
            }
            Err(e) => {
                warn!(target: "segment", error = %e.to_string(), "ReviewClient init failed; segment comments disabled for this run");
                None
            }
        }
    } else {
        None
    };

    // Pre-scan + build data for segment comments; comment pages must be appended at the end.
    // We will compute deterministic EPUB file names based on EpubGenerator's counter rules:
    // - first aux page (intro) is aux_00000.xhtml
    // - then chapters are chapter_00001.xhtml .. chapter_{N:05}.xhtml
    // - then segment comment pages will start from aux_{(1+N):05}.xhtml
    #[derive(Debug)]
    struct ChapterBuild {
        chapter_id: String,
        title: String,
        raw_xhtml: String,
        seg_counts: serde_json::Map<String, Value>,
        #[cfg(feature = "official-api")]
        per_para: Vec<(i32, tomato_novel_official_api::ReviewResponse)>,
        #[cfg(not(feature = "official-api"))]
        per_para: Vec<(i32, serde_json::Value)>,
    }

    let chapter_count = chapters.len();
    let base_comment_aux_index = 1 + chapter_count; // 1 intro page already consumed
    let mut builds: Vec<ChapterBuild> = Vec::with_capacity(chapter_count);

    for (ch_idx, ch) in chapters.iter().enumerate() {
        let chapter_id = ch.get("id").and_then(|v| v.as_str()).unwrap_or("0");
        let title = ch.get("title").and_then(|v| v.as_str()).unwrap_or("章节");
        let content_html = ch.get("content").and_then(|v| v.as_str()).unwrap_or("");

        info!(
            target: "segment",
            ch_idx,
            chapter_id = %chapter_id,
            title = %title,
            "segment comment scan chapter"
        );

        let rewritten = embed_inline_images_chapter_named(
            &mut epub_gen,
            &manager.config,
            chapter_id,
            content_html,
            &mut image_cache,
            &mut resources_added,
            &images_dir,
        )
        .unwrap_or_else(|_| content_html.to_string());

        // Default chapter output: keep original XHTML (to preserve paragraphs for segment indexing).
        let mut seg_counts = serde_json::Map::new();
        #[cfg(feature = "official-api")]
        let mut per_para: Vec<(i32, tomato_novel_official_api::ReviewResponse)> = Vec::new();
        #[cfg(not(feature = "official-api"))]
        let per_para: Vec<(i32, serde_json::Value)> = Vec::new();

        #[cfg(feature = "official-api")]
        if enable_segment_comments && let Some(client) = review_client.as_ref() {
            let mut did_network_fetch = false;

            // Prefer cache generated during download stage: status_folder/segment_comments/{chapter_id}.json
            if let Some(cache) = load_segment_comments_cache(manager, chapter_id) {
                debug!(
                    target: "segment",
                    chapter_id = %chapter_id,
                    cached_paras = cache.paras.len(),
                    item_version = %cache.item_version,
                    "segment cache loaded"
                );

                // Build seg_counts from cached counts (only keep positive counts, consistent with extract_para_counts_from_stats).
                for (k, v) in &cache.paras {
                    if v.count > 0 {
                        seg_counts
                            .insert(k.clone(), Value::Number(serde_json::Number::from(v.count)));
                    }
                }

                // Use cached detail where present.
                for (k, v) in &cache.paras {
                    let Ok(idx) = k.parse::<i32>() else {
                        continue;
                    };
                    if let Some(resp) = v.detail.as_ref()
                        && !resp.reviews.is_empty()
                    {
                        per_para.push((idx, resp.clone()));
                    }
                }
                per_para.sort_by_key(|(idx, _)| *idx);

                // Fill missing details (count>0 but missing/empty detail) online as fallback.
                let mut missing: Vec<i32> = cache
                    .paras
                    .iter()
                    .filter_map(|(k, v)| {
                        if v.count == 0 {
                            return None;
                        }
                        let Ok(idx) = k.parse::<i32>() else {
                            return None;
                        };
                        let has_detail = v
                            .detail
                            .as_ref()
                            .map(|d| !d.reviews.is_empty())
                            .unwrap_or(false);
                        if has_detail { None } else { Some(idx) }
                    })
                    .collect();
                missing.sort_unstable();

                if !missing.is_empty() {
                    did_network_fetch = true;
                    let item_version = cache.item_version.as_str();
                    let top_n = cache.top_n.max(1);
                    let workers = manager.config.segment_comments_workers.clamp(1, 64);
                    let worker_count = workers.min(missing.len().max(1));

                    info!(
                        target: "segment",
                        chapter_id = %chapter_id,
                        missing = missing.len(),
                        worker_count,
                        "segment cache incomplete; fetching missing paras"
                    );

                    if worker_count <= 1 {
                        for para_idx in &missing {
                            let fetched = client
                                .fetch_para_comments(
                                    chapter_id,
                                    &manager.book_id,
                                    *para_idx,
                                    item_version,
                                    top_n,
                                    2,
                                )
                                .or_else(|_| {
                                    client.fetch_para_comments(
                                        chapter_id,
                                        &manager.book_id,
                                        *para_idx,
                                        item_version,
                                        top_n,
                                        0,
                                    )
                                });
                            if let Ok(Some(res)) = fetched
                                && !res.response.reviews.is_empty()
                            {
                                per_para.push((*para_idx, res.response));
                            }
                        }
                        per_para.sort_by_key(|(idx, _)| *idx);
                    } else {
                        let (tx_jobs, rx_jobs) = channel::unbounded::<i32>();
                        let (tx_res, rx_res) = channel::unbounded::<(
                            i32,
                            Option<tomato_novel_official_api::ReviewResponse>,
                        )>();
                        for para_idx in &missing {
                            let _ = tx_jobs.send(*para_idx);
                        }
                        drop(tx_jobs);

                        let mut handles = Vec::with_capacity(worker_count);
                        for _ in 0..worker_count {
                            let rx = rx_jobs.clone();
                            let tx = tx_res.clone();
                            let chapter_id = chapter_id.to_string();
                            let book_id = manager.book_id.clone();
                            let item_version = item_version.to_string();
                            let options = review_options.clone();
                            handles.push(std::thread::spawn(move || {
                                let client = match ReviewClient::new(options) {
                                    Ok(c) => c,
                                    Err(_) => return,
                                };
                                for para_idx in rx.iter() {
                                    let fetched = client
                                        .fetch_para_comments(
                                            &chapter_id,
                                            &book_id,
                                            para_idx,
                                            &item_version,
                                            top_n,
                                            2,
                                        )
                                        .or_else(|_| {
                                            client.fetch_para_comments(
                                                &chapter_id,
                                                &book_id,
                                                para_idx,
                                                &item_version,
                                                top_n,
                                                0,
                                            )
                                        });
                                    if let Ok(Some(res)) = fetched {
                                        if !res.response.reviews.is_empty() {
                                            let _ = tx.send((para_idx, Some(res.response)));
                                        } else {
                                            let _ = tx.send((para_idx, None));
                                        }
                                    } else {
                                        let _ = tx.send((para_idx, None));
                                    }
                                }
                            }));
                        }
                        drop(tx_res);

                        let mut tmp: Vec<(i32, tomato_novel_official_api::ReviewResponse)> =
                            Vec::new();
                        for (para_idx, resp) in rx_res.iter() {
                            if let Some(resp) = resp {
                                tmp.push((para_idx, resp));
                            }
                        }
                        for h in handles {
                            let _ = h.join();
                        }
                        tmp.sort_by_key(|(idx, _)| *idx);

                        // Merge: prefer existing cached detail, then add fetched.
                        per_para.extend(tmp);
                        per_para.sort_by_key(|(idx, _)| *idx);
                        per_para.dedup_by_key(|(idx, _)| *idx);
                    }
                }
            } else {
                // No cache: use the old online logic.
                did_network_fetch = true;

                // Ensure we have item_version.
                if !item_versions.contains_key(chapter_id) && !official_dir_fetched {
                    info!(
                        target: "segment",
                        book_id = %manager.book_id,
                        "item_version missing; fetching official directory once"
                    );
                    if let Ok(c) = DirectoryClient::new() {
                        match c.fetch_directory(&manager.book_id) {
                            Ok(dir) => {
                                let before = item_versions.len();
                                item_versions.extend(extract_item_version_map(&dir.raw));
                                info!(
                                    target: "segment",
                                    before,
                                    after = item_versions.len(),
                                    chapters = dir.chapters.len(),
                                    "official directory fetched"
                                );
                            }
                            Err(e) => {
                                warn!(target: "segment", error = %e.to_string(), "official directory fetch failed");
                            }
                        }
                    } else {
                        warn!(target: "segment", "DirectoryClient init failed; cannot fetch official directory");
                    }
                    official_dir_fetched = true; // don't keep retrying
                }

                let item_version = item_versions
                    .get(chapter_id)
                    .map(|s| s.as_str())
                    .unwrap_or("0");

                debug!(
                    target: "segment",
                    chapter_id = %chapter_id,
                    item_version = %item_version,
                    has_version = item_versions.contains_key(chapter_id),
                    "using item_version"
                );

                let t_stats = Instant::now();
                match client.fetch_comment_stats(chapter_id, item_version) {
                    Ok(Some(stats)) => {
                        seg_counts = extract_para_counts_from_stats(&stats);
                        info!(
                            target: "segment",
                            chapter_id = %chapter_id,
                            ms = t_stats.elapsed().as_millis() as u64,
                            para_with_counts = seg_counts.len(),
                            keys = %format!("{}", stats.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(",")).unwrap_or_default()),
                            "comment stats fetched"
                        );
                    }
                    Ok(None) => {
                        warn!(
                            target: "segment",
                            chapter_id = %chapter_id,
                            ms = t_stats.elapsed().as_millis() as u64,
                            "comment stats empty (None)"
                        );
                    }
                    Err(e) => {
                        warn!(
                            target: "segment",
                            chapter_id = %chapter_id,
                            item_version = %item_version,
                            ms = t_stats.elapsed().as_millis() as u64,
                            error = %e.to_string(),
                            "comment stats fetch failed"
                        );
                    }
                }

                let mut para_with_comments: Vec<i32> = seg_counts
                    .iter()
                    .filter_map(|(k, v)| {
                        let cnt = v.as_u64().unwrap_or(0);
                        if cnt == 0 {
                            return None;
                        }
                        k.parse::<i32>().ok()
                    })
                    .collect();
                para_with_comments.sort_unstable();

                if !para_with_comments.is_empty() {
                    let sample = para_with_comments
                        .iter()
                        .take(6)
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    info!(
                        target: "segment",
                        chapter_id = %chapter_id,
                        paras = para_with_comments.len(),
                        sample = %sample,
                        "paras with comments"
                    );
                } else {
                    info!(
                        target: "segment",
                        chapter_id = %chapter_id,
                        "no paras with comments after parsing stats"
                    );
                }

                let top_n = manager.config.segment_comments_top_n.max(1);
                if !para_with_comments.is_empty() {
                    let workers = manager.config.segment_comments_workers.clamp(1, 64);
                    let worker_count = workers.min(para_with_comments.len().max(1));

                    if worker_count <= 1 {
                        for para_idx in &para_with_comments {
                            let t_para = Instant::now();
                            let fetched = client
                                .fetch_para_comments(
                                    chapter_id,
                                    &manager.book_id,
                                    *para_idx,
                                    item_version,
                                    top_n,
                                    2,
                                )
                                .or_else(|_| {
                                    client.fetch_para_comments(
                                        chapter_id,
                                        &manager.book_id,
                                        *para_idx,
                                        item_version,
                                        top_n,
                                        0,
                                    )
                                });

                            match fetched {
                                Ok(Some(res)) => {
                                    let reviews = res.response.reviews.len();
                                    debug!(
                                        target: "segment",
                                        chapter_id = %chapter_id,
                                        para_idx = *para_idx,
                                        ms = t_para.elapsed().as_millis() as u64,
                                        reviews,
                                        "para comments fetched"
                                    );
                                    if reviews > 0 {
                                        per_para.push((*para_idx, res.response));
                                    }
                                }
                                Ok(None) => {
                                    debug!(
                                        target: "segment",
                                        chapter_id = %chapter_id,
                                        para_idx = *para_idx,
                                        ms = t_para.elapsed().as_millis() as u64,
                                        "para comments empty (None)"
                                    );
                                }
                                Err(e) => {
                                    warn!(
                                        target: "segment",
                                        chapter_id = %chapter_id,
                                        para_idx = *para_idx,
                                        item_version = %item_version,
                                        ms = t_para.elapsed().as_millis() as u64,
                                        error = %e.to_string(),
                                        "para comments fetch failed"
                                    );
                                }
                            }
                        }
                    } else {
                        let (tx_jobs, rx_jobs) = channel::unbounded::<i32>();
                        let (tx_res, rx_res) = channel::unbounded::<(
                            i32,
                            Option<tomato_novel_official_api::ReviewResponse>,
                        )>();
                        for para_idx in &para_with_comments {
                            let _ = tx_jobs.send(*para_idx);
                        }
                        drop(tx_jobs);

                        let mut handles = Vec::with_capacity(worker_count);
                        for _ in 0..worker_count {
                            let rx = rx_jobs.clone();
                            let tx = tx_res.clone();
                            let chapter_id = chapter_id.to_string();
                            let book_id = manager.book_id.clone();
                            let item_version = item_version.to_string();
                            let options = review_options.clone();
                            handles.push(std::thread::spawn(move || {
                                let client = match ReviewClient::new(options) {
                                    Ok(c) => c,
                                    Err(_) => return,
                                };
                                for para_idx in rx.iter() {
                                    let fetched = client
                                        .fetch_para_comments(
                                            &chapter_id,
                                            &book_id,
                                            para_idx,
                                            &item_version,
                                            top_n,
                                            2,
                                        )
                                        .or_else(|_| {
                                            client.fetch_para_comments(
                                                &chapter_id,
                                                &book_id,
                                                para_idx,
                                                &item_version,
                                                top_n,
                                                0,
                                            )
                                        });
                                    if let Ok(Some(res)) = fetched {
                                        if !res.response.reviews.is_empty() {
                                            let _ = tx.send((para_idx, Some(res.response)));
                                        } else {
                                            let _ = tx.send((para_idx, None));
                                        }
                                    } else {
                                        let _ = tx.send((para_idx, None));
                                    }
                                }
                            }));
                        }
                        drop(tx_res);

                        let mut tmp: Vec<(i32, tomato_novel_official_api::ReviewResponse)> =
                            Vec::new();
                        for (para_idx, resp) in rx_res.iter() {
                            if let Some(resp) = resp {
                                tmp.push((para_idx, resp));
                            }
                        }
                        for h in handles {
                            let _ = h.join();
                        }
                        tmp.sort_by_key(|(idx, _)| *idx);
                        per_para = tmp;
                    }
                }
            }

            info!(
                target: "segment",
                chapter_id = %chapter_id,
                para_groups = per_para.len(),
                "segment comments collected for chapter"
            );

            // One chapter worth of segment-comment fetch finished only when we did network work.
            if did_network_fetch
                && let Some(r) = {
                    #[allow(clippy::needless_option_as_deref)]
                    reporter.as_deref_mut()
                }
            {
                r.inc_comment_fetch();
            }
        }

        builds.push(ChapterBuild {
            chapter_id: chapter_id.to_string(),
            title: title.to_string(),
            raw_xhtml: rewritten,
            seg_counts,
            per_para,
        });

        // (Optional) basic progress info via logs.
        let _ = ch_idx;
    }

    // First, add all正文 chapters in order.
    // Determine comment page filename per chapter that actually has comments.
    let mut comment_page_for_chapter: HashMap<String, String> = HashMap::new();
    let mut comment_pages: Vec<(String, String)> = Vec::new(); // (title, html)
    let mut comment_page_index = 0usize;

    for (idx, b) in builds.iter().enumerate() {
        // Determine chapter file name: intro consumes aux_00000, so chapter starts at 1.
        let chapter_file = format!("chapter_{:05}.xhtml", 1 + idx);

        #[cfg(feature = "official-api")]
        if !b.per_para.is_empty() {
            prefetch_comment_media(&manager.config, &b.per_para, &images_dir);

            let comment_file = format!(
                "aux_{:05}.xhtml",
                base_comment_aux_index + comment_page_index
            );
            comment_page_for_chapter.insert(b.chapter_id.clone(), comment_file.clone());

            let page_title = format!("{} - 段评", b.title);
            let page_html = render_segment_comment_page(
                &b.title,
                &chapter_file,
                &b.raw_xhtml,
                &b.per_para,
                &manager.config,
                &mut resources_added,
                &images_dir,
                &mut epub_gen,
            )?;
            comment_pages.push((page_title, page_html));
            comment_page_index += 1;

            if let Some(r) = reporter.as_deref_mut() {
                r.inc_comment_saved();
            }
        }
    }

    for b in &builds {
        let comment_file = comment_page_for_chapter
            .get(&b.chapter_id)
            .map(|s| s.as_str())
            .unwrap_or("");

        let chapter_out = if !comment_file.is_empty() {
            segment_utils::inject_segment_links(&b.raw_xhtml, comment_file, &b.seg_counts)
        } else {
            clean_epub_body(&b.raw_xhtml)
        };
        epub_gen.add_chapter(&b.title, &chapter_out);
    }

    // Finally, append all段评 pages at the end (in spine), to maximize reader compatibility.
    for (title, html) in comment_pages {
        let _ = epub_gen.add_aux_page(&title, &html, true);
    }

    epub_gen.generate(path, &manager.config)?;
    Ok(())
}

fn embed_inline_images_chapter_named(
    epub: &mut EpubGenerator,
    cfg: &crate::base_system::context::Config,
    _chapter_id: &str,
    html: &str,
    cache: &mut HashMap<String, (PathBuf, &'static str, &'static str)>,
    resources_added: &mut HashSet<String>,
    images_dir: &Path,
) -> anyhow::Result<String> {
    // Capture <img ... src="..." ...>. Keep it simple: API provides XHTML.
    let re_img = Regex::new(r#"(?is)<img[^>]*?\bsrc\s*=\s*['\"]([^'\"]+)['\"][^>]*>"#)?;

    // Per-chapter mapping: original src (raw/decoded) -> resource path
    let mut mapping: HashMap<String, String> = HashMap::new();
    for cap in re_img.captures_iter(html) {
        let src_raw = cap.get(1).map(|m| m.as_str()).unwrap_or("").trim();
        if src_raw.is_empty() {
            continue;
        }

        let decoded = decode_xhtml_attr_url(src_raw);
        let decoded = decoded.as_ref();
        if decoded.starts_with("images/") || decoded.starts_with("data:") {
            continue;
        }
        if !(decoded.starts_with("http://") || decoded.starts_with("https://")) {
            continue;
        }

        let normalized = if let Some((path, mime, ext)) = cache.get(decoded) {
            Some((path.clone(), *mime, *ext))
        } else {
            let fetched = ensure_cached_image(cfg, decoded, images_dir)?;
            if let Some((path, mime, ext)) = &fetched {
                cache.insert(decoded.to_string(), (path.clone(), *mime, *ext));
            }
            fetched
        };
        let Some((local_path, mime, ext)) = normalized else {
            continue;
        };

        // resource path uses URL hash to be stable across runs (resume-friendly)
        let hash = sha1_hex(decoded);
        let resource_path = format!("images/{}{}", hash, ext);

        if !resources_added.contains(&resource_path)
            && let Ok(bytes) = fs::read(&local_path)
            && epub.add_resource_bytes(&resource_path, bytes, mime).is_ok()
        {
            resources_added.insert(resource_path.clone());
        }

        if resources_added.contains(&resource_path) {
            mapping.insert(src_raw.to_string(), resource_path.clone());
            mapping.insert(decoded.to_string(), resource_path);
        }
    }

    let rewritten = re_img
        .replace_all(html, |caps: &regex::Captures| {
            let whole = caps.get(0).map(|m| m.as_str()).unwrap_or("");
            let src_raw = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let decoded = decode_xhtml_attr_url(src_raw);

            if let Some(path) = mapping
                .get(src_raw)
                .or_else(|| mapping.get(decoded.as_ref()))
            {
                whole.replacen(src_raw, path, 1)
            } else {
                whole.to_string()
            }
        })
        .to_string();
    Ok(rewritten)
}

fn sha1_hex(input: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

fn mime_from_ext(ext: &str) -> &'static str {
    match ext {
        ".jpg" | ".jpeg" => "image/jpeg",
        ".png" => "image/png",
        ".gif" => "image/gif",
        ".webp" => "image/webp",
        ".avif" => "image/avif",
        ".heic" | ".heif" => "image/heic",
        _ => "application/octet-stream",
    }
}

fn find_cached_image(
    images_dir: &Path,
    hash: &str,
) -> Option<(PathBuf, &'static str, &'static str)> {
    let exts = [
        ".jpeg", ".jpg", ".png", ".gif", ".webp", ".avif", ".heic", ".heif",
    ];
    for ext in exts {
        let p = images_dir.join(format!("{hash}{ext}"));
        if p.exists() {
            return Some((p, mime_from_ext(ext), ext));
        }
    }
    None
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!(
        "{}part",
        path.extension().and_then(|s| s.to_str()).unwrap_or("")
    ));
    fs::write(&tmp, bytes)?;
    // best-effort replace
    let _ = fs::remove_file(path);
    fs::rename(tmp, path)?;
    Ok(())
}

fn ensure_cached_image(
    cfg: &crate::base_system::context::Config,
    url: &str,
    images_dir: &Path,
) -> anyhow::Result<Option<(PathBuf, &'static str, &'static str)>> {
    if !cfg.blocked_media_domains.is_empty() {
        let lowered = url.to_ascii_lowercase();
        if cfg
            .blocked_media_domains
            .iter()
            .any(|d| !d.trim().is_empty() && lowered.contains(&d.to_ascii_lowercase()))
        {
            return Ok(None);
        }
    }

    let hash = sha1_hex(url);
    if let Some(hit) = find_cached_image(images_dir, &hash) {
        return Ok(Some(hit));
    }

    let fetched = fetch_and_normalize_image(cfg, url)?;
    let Some((bytes, mime, ext)) = fetched else {
        return Ok(None);
    };

    fs::create_dir_all(images_dir)?;
    let out_path = images_dir.join(format!("{hash}{ext}"));
    if !out_path.exists() {
        let _ = write_atomic(&out_path, &bytes);
    }
    Ok(Some((out_path, mime, ext)))
}

fn clean_epub_body(html: &str) -> String {
    let re_token =
        Regex::new(r"(?is)(<img\b[^>]*?>)|(<p\b[^>]*?>.*?</p>)|(<h[1-6]\b[^>]*?>.*?</h[1-6]>)")
            .unwrap();
    let re_src = Regex::new(r#"(?is)\bsrc\s*=\s*['\"]([^'\"]+)['\"]"#).unwrap();
    let re_img = Regex::new(r#"(?is)<img\b[^>]*?>"#).unwrap();
    let re_tags = Regex::new(r"(?is)<[^>]+>").unwrap();

    let mut out: Vec<String> = Vec::new();
    for cap in re_token.captures_iter(html) {
        if let Some(img_tag) = cap.get(1).map(|m| m.as_str()) {
            let src = re_src
                .captures(img_tag)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str())
                .unwrap_or("");
            if src.is_empty() {
                continue;
            }
            if src.starts_with("images/") {
                out.push(format!("<img alt=\"\" src=\"{}\"/>", escape_html(src)));
            }
            continue;
        }

        if let Some(p_tag) = cap.get(2).map(|m| m.as_str()) {
            // Keep picture captions (pictureDesc) as a dedicated line under image.
            let lower = p_tag.to_ascii_lowercase();
            if lower.contains("picturedesc") {
                let inner = re_tags.replace_all(p_tag, "");
                let inner = unescape_basic_entities(inner.as_ref());
                let text = inner.trim();
                if text.is_empty() {
                    continue;
                }
                let line = format!("﹝图﹞ {}", text);
                out.push(format!("<p class=\"img-desc\">{}</p>", escape_html(&line)));
                continue;
            }
            if lower.contains("<img") {
                // Some fanqie XHTML wraps images inside <p class="picture"> ... <img .../> ...</p>.
                // Extract those images and emit minimal <img> tags, preserving order.
                for img_tag in re_img.find_iter(p_tag).map(|m| m.as_str()) {
                    let src = re_src
                        .captures(img_tag)
                        .and_then(|c| c.get(1))
                        .map(|m| m.as_str())
                        .unwrap_or("");
                    if src.starts_with("images/") {
                        out.push(format!("<img alt=\"\" src=\"{}\"/>", escape_html(src)));
                    }
                }

                // If wrapper also contains pictureDesc text, keep it.
                if lower.contains("picturedesc") {
                    let inner = re_tags.replace_all(p_tag, "");
                    let inner = unescape_basic_entities(inner.as_ref());
                    let text = inner.trim();
                    if !text.is_empty() {
                        let line = format!("﹝图﹞ {}", text);
                        out.push(format!("<p class=\"img-desc\">{}</p>", escape_html(&line)));
                    }
                }
                continue;
            }
            let inner = re_tags.replace_all(p_tag, "");
            let inner = unescape_basic_entities(inner.as_ref());
            let text = inner.trim();
            if text.is_empty() {
                continue;
            }
            out.push(format!("<p>{}</p>", escape_html(text)));
            continue;
        }

        // Headings inside content: skip (EpubGenerator already injects a <h1>).
    }

    if out.is_empty() {
        let plain = re_tags.replace_all(html, "");
        let plain = unescape_basic_entities(plain.as_ref());
        for line in plain.lines() {
            let t = line.trim();
            if !t.is_empty() {
                out.push(format!("<p>{}</p>", escape_html(t)));
            }
        }
    }

    out.join("\n")
}

fn fetch_and_normalize_image(
    cfg: &crate::base_system::context::Config,
    url: &str,
) -> anyhow::Result<Option<(Vec<u8>, &'static str, &'static str)>> {
    if !cfg.blocked_media_domains.is_empty() {
        let lowered = url.to_ascii_lowercase();
        if cfg
            .blocked_media_domains
            .iter()
            .any(|d| !d.trim().is_empty() && lowered.contains(&d.to_ascii_lowercase()))
        {
            return Ok(None);
        }
    }

    let bytes = match crate::third_party::media_fetch::fetch_bytes(
        url,
        std::time::Duration::from_millis(10_000),
    ) {
        Some(b) => b,
        None => return Ok(None),
    };

    let (mime, ext) = sniff_mime_ext(&bytes);

    // 转码逻辑：
    // - force_convert_images_to_jpeg=true：无条件尽量转
    // - jpeg_retry_convert=true：当识别失败/或非 jpeg 时尝试转（可提升兼容性）
    let should_try_jpeg = cfg.force_convert_images_to_jpeg
        || cfg.jpeg_retry_convert && (mime == "application/octet-stream" || mime != "image/jpeg");
    if should_try_jpeg
        // HEIC/HEIF needs explicit opt-in for conversion attempt (image crate likely can't decode it).
        && (mime != "image/heic" || cfg.convert_heic_to_jpeg)
        && let Some(jpeg) = try_convert_to_jpeg(&bytes, cfg.jpeg_quality, cfg.media_max_dimension_px)
    {
        return Ok(Some((jpeg, "image/jpeg", ".jpeg")));
    }

    // If it's HEIC/HEIF and conversion failed, keep original only when configured.
    if mime == "image/heic" {
        if cfg.keep_heic_original {
            return Ok(Some((bytes, "image/heic", ext)));
        }
        return Ok(None);
    }

    if mime == "application/octet-stream" {
        return Ok(None);
    }

    Ok(Some((bytes, mime, ext)))
}

fn sniff_mime_ext(bytes: &[u8]) -> (&'static str, &'static str) {
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return ("image/jpeg", ".jpeg");
    }
    if bytes.len() >= 8
        && bytes[0] == 0x89
        && bytes[1] == 0x50
        && bytes[2] == 0x4E
        && bytes[3] == 0x47
        && bytes[4] == 0x0D
        && bytes[5] == 0x0A
        && bytes[6] == 0x1A
        && bytes[7] == 0x0A
    {
        return ("image/png", ".png");
    }
    if bytes.len() >= 6 && (&bytes[0..3] == b"GIF") {
        return ("image/gif", ".gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return ("image/webp", ".webp");
    }
    // HEIC/HEIF detection: ISO BMFF 'ftyp' brand.
    if bytes.len() >= 16 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if brand == b"heic"
            || brand == b"heif"
            || brand == b"heix"
            || brand == b"mif1"
            || brand == b"msf1"
        {
            return ("image/heic", ".heic");
        }
    }
    ("application/octet-stream", "")
}

fn try_convert_to_jpeg(bytes: &[u8], quality: u8, max_dim: u32) -> Option<Vec<u8>> {
    let mut img = image::load_from_memory(bytes).ok()?;

    if max_dim > 0 {
        let (w, h) = img.dimensions();
        let longest = w.max(h);
        if longest > max_dim {
            let scale = max_dim as f32 / longest as f32;
            let nw = ((w as f32) * scale).round().max(1.0) as u32;
            let nh = ((h as f32) * scale).round().max(1.0) as u32;
            img = img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3);
        }
    }

    let rgb = img.to_rgb8();
    let mut out = Vec::new();
    {
        let q = quality.clamp(1, 100);
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q);
        encoder
            .encode(
                &rgb,
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .ok()?;
    }
    Some(out)
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
