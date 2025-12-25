use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;

use serde_json::Value;
use tracing::{error, info};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STD;
use image::GenericImageView;
use regex::Regex;
use sha1::{Digest, Sha1};

use super::book_manager::BookManager;
use super::epub_generator::EpubGenerator;
use crate::base_system::context::safe_fs_name;
use tomato_novel_official_api::call_operation;

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
pub fn run_finalize(manager: &mut BookManager, chapters: &[Value], _result: i32) -> bool {
    info!(target: "book_manager", "finalize start: chapters={}", chapters.len());

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
        finalize_epub(manager, chapters, &output_path)
    };

    if let Err(e) = result {
        error!(target: "book_manager", error = ?e, "finalize failed");
        return false;
    }

    info!(target: "book_manager", "written: {}", output_path.display());
    manager.config.auto_clear_dump
}

/// 执行延迟清理（当前直接调用）。
pub fn perform_deferred_cleanup(manager: &mut BookManager) {
    if manager.config.auto_clear_dump {
        if let Err(e) = manager.delete_status_folder() {
            error!(target: "book_manager", error = ?e, "deferred cleanup failed");
        }
        return;
    }

    if let Err(e) = manager.cleanup_status_folder() {
        error!(target: "book_manager", error = ?e, "deferred cleanup failed");
    }
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
    Ok(dir.join(format!("{}.{}", safe_book, suffix)))
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

fn finalize_epub(manager: &BookManager, chapters: &[Value], path: &Path) -> anyhow::Result<()> {
    let mut epub_gen = EpubGenerator::new(&manager.book_id, &manager.book_name, &manager.config)?;

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
    epub_gen.add_aux_page("简介", &intro_html, true);

    for ch in chapters {
        let chapter_id = ch.get("id").and_then(|v| v.as_str()).unwrap_or("0");
        let title = ch.get("title").and_then(|v| v.as_str()).unwrap_or("章节");
        let content_html = ch.get("content").and_then(|v| v.as_str()).unwrap_or("");

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

        let cleaned = clean_epub_body(&rewritten);
        epub_gen.add_chapter(title, &cleaned);
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

        if !resources_added.contains(&resource_path) {
            if let Ok(bytes) = fs::read(&local_path) {
                if epub.add_resource_bytes(&resource_path, bytes, mime).is_ok() {
                    resources_added.insert(resource_path.clone());
                }
            }
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
        _ => "application/octet-stream",
    }
}

fn find_cached_image(
    images_dir: &Path,
    hash: &str,
) -> Option<(PathBuf, &'static str, &'static str)> {
    let exts = [".jpeg", ".jpg", ".png", ".gif", ".webp", ".avif"];
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

    let payload = serde_json::json!({
        "url": url,
        "timeout_ms": 10000u64,
    });
    let value = match call_operation("media_fetch", &payload) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };

    let b64 = value
        .get("body_b64")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("data").and_then(|v| v.as_str()));
    let Some(b64) = b64 else {
        return Ok(None);
    };
    let bytes = match BASE64_STD.decode(b64) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };

    let (mime, ext) = sniff_mime_ext(&bytes);

    // 转码逻辑：
    // - force_convert_images_to_jpeg=true：无条件尽量转
    // - jpeg_retry_convert=true：当识别失败/或非 jpeg 时尝试转（可提升兼容性）
    let should_try_jpeg = cfg.force_convert_images_to_jpeg
        || cfg.jpeg_retry_convert && (mime == "application/octet-stream" || mime != "image/jpeg");
    if should_try_jpeg {
        if let Some(jpeg) =
            try_convert_to_jpeg(&bytes, cfg.jpeg_quality, cfg.media_max_dimension_px)
        {
            return Ok(Some((jpeg, "image/jpeg", ".jpeg")));
        }
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
