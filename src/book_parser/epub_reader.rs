//! EPUB → TXT 转换。
//!
//! 读取已有 EPUB（本程序生成的或第三方来源），按 OPF `spine` 顺序抽取章节 XHTML，
//! 复用 [`ContentParser`] 清洗为纯文本，并按 `finalize_txt` 的输出风格写出 txt。
//!
//! 只做本地文件转换，不涉及任何网络请求。

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use regex::Regex;
use tracing::{info, warn};

use super::parser::ContentParser;

/// 单本 EPUB 的转换结果。
#[derive(Debug, Clone)]
pub struct ConvertSummary {
    pub epub_path: PathBuf,
    pub txt_path: PathBuf,
    /// 写入 txt 的章节数
    pub chapter_count: usize,
    /// 读取失败/为空的章节数
    pub skipped_chapters: usize,
    /// 生成的 txt 字节数
    pub bytes: u64,
}

/// 批量转换结果（目录模式会尽力继续，不因单本失败而整体中断）。
#[derive(Debug, Default)]
pub struct BatchResult {
    pub converted: Vec<ConvertSummary>,
    pub failures: Vec<(PathBuf, String)>,
}

impl BatchResult {
    pub fn total_chapters(&self) -> usize {
        self.converted.iter().map(|c| c.chapter_count).sum()
    }
}

/// 解析后的 EPUB 内容。
#[derive(Debug, Clone, Default)]
pub struct EpubBook {
    pub title: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub identifier: Option<String>,
    pub chapters: Vec<EpubChapter>,
    /// 解析阶段被跳过的页面数（封面/空白/缺失文件）
    pub skipped_pages: usize,
}

#[derive(Debug, Clone, Default)]
pub struct EpubChapter {
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone)]
struct ManifestItem {
    id: String,
    href: String,
    media_type: String,
    properties: String,
}

// ── 正则缓存 ────────────────────────────────────────────────────────

fn re_attr_pairs() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r#"(?i)([A-Za-z_:][A-Za-z0-9_:.-]*)\s*=\s*("([^"]*)"|'([^']*)')"#).unwrap()
    })
}

fn re_rootfile() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<rootfile\b[^>]*>").unwrap())
}

fn re_manifest_item() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<item\b[^>]*>").unwrap())
}

fn re_itemref() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<itemref\b[^>]*>").unwrap())
}

fn re_spine() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<spine\b[^>]*>").unwrap())
}

fn re_dc() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<dc:([a-z]+)\b[^>]*>(.*?)</dc:[a-z]+>").unwrap())
}

fn re_ncx_text() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<text[^>]*>(.*?)</text>").unwrap())
}

fn re_ncx_navmap() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<navMap\b[^>]*>(.*?)</navMap>").unwrap())
}

fn re_ncx_content() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<content\b[^>]*>").unwrap())
}

fn re_nav_anchor() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<a\b[^>]*>(.*?)</a>").unwrap())
}

fn re_html_title() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap())
}

fn re_html_h1() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<h1[^>]*>(.*?)</h1>").unwrap())
}

fn re_strip_tags() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<[^>]+>").unwrap())
}

// ── 属性/路径工具 ───────────────────────────────────────────────────

fn parse_attrs(tag: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for caps in re_attr_pairs().captures_iter(tag) {
        let Some(name) = caps.get(1) else { continue };
        let value = caps
            .get(3)
            .or_else(|| caps.get(4))
            .map(|m| m.as_str())
            .unwrap_or("");
        map.insert(name.as_str().to_ascii_lowercase(), value.to_string());
    }
    map
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 将 `href` 相对 `base_dir` 规范化成 zip 内部路径（处理 `../`、`./`、`%20` 与锚点）。
fn normalize_zip_path(base_dir: &str, href: &str) -> String {
    let without_fragment = href.split(['#', '?']).next().unwrap_or(href);
    let decoded = percent_decode(without_fragment);
    let combined = if decoded.starts_with('/') {
        decoded.trim_start_matches('/').to_string()
    } else if base_dir.is_empty() {
        decoded
    } else {
        format!("{}/{}", base_dir.trim_end_matches('/'), decoded)
    };

    let mut parts: Vec<&str> = Vec::new();
    for seg in combined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(idx) => &path[..idx],
        None => "",
    }
}

fn strip_tags(raw: &str) -> String {
    re_strip_tags().replace_all(raw, "").into_owned()
}

fn collapse_ws(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn unescape_basic(raw: &str) -> String {
    let mut out = raw
        .replace("&nbsp;", " ")
        .replace("&ldquo;", "\u{201C}")
        .replace("&rdquo;", "\u{201D}")
        .replace("&lsquo;", "\u{2018}")
        .replace("&rsquo;", "\u{2019}")
        .replace("&mdash;", "\u{2014}")
        .replace("&ndash;", "\u{2013}")
        .replace("&hellip;", "\u{2026}")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    // `&amp;` 最后处理，避免二次解码
    out = out.replace("&amp;", "&");
    out
}

// ── zip 读取 ────────────────────────────────────────────────────────

type ZipArchive = zip::ZipArchive<File>;

fn build_name_index(archive: &mut ZipArchive) -> HashMap<String, String> {
    let mut index = HashMap::new();
    for i in 0..archive.len() {
        if let Ok(entry) = archive.by_index(i) {
            let name = entry.name().to_string();
            index.entry(name.to_ascii_lowercase()).or_insert(name);
        }
    }
    index
}

fn read_entry(archive: &mut ZipArchive, index: &HashMap<String, String>, name: &str) -> Option<String> {
    let actual = index.get(&name.to_ascii_lowercase())?;
    let mut entry = archive.by_name(actual).ok()?;
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf).ok()?;

    // 容忍 UTF-8 BOM
    let bytes = if buf.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &buf[3..]
    } else {
        buf.as_slice()
    };
    Some(String::from_utf8_lossy(bytes).into_owned())
}

fn find_first_opf(index: &HashMap<String, String>) -> Option<String> {
    let mut candidates: Vec<&String> = index
        .values()
        .filter(|name| name.to_ascii_lowercase().ends_with(".opf"))
        .collect();
    candidates.sort();
    candidates.first().map(|s| (*s).clone())
}

// ── OPF 解析 ────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct EpubMetadata {
    title: Option<String>,
    author: Option<String>,
    description: Option<String>,
    identifier: Option<String>,
}

fn parse_metadata(opf: &str) -> EpubMetadata {
    let mut meta = EpubMetadata::default();
    for caps in re_dc().captures_iter(opf) {
        let (Some(tag), Some(value)) = (caps.get(1), caps.get(2)) else {
            continue;
        };
        let value = value.as_str().trim();
        if value.is_empty() {
            continue;
        }
        match tag.as_str().to_ascii_lowercase().as_str() {
            "title" if meta.title.is_none() => {
                meta.title = Some(collapse_ws(&unescape_basic(&strip_tags(value))));
            }
            "creator" if meta.author.is_none() => {
                meta.author = Some(collapse_ws(&unescape_basic(&strip_tags(value))));
            }
            "description" if meta.description.is_none() => {
                let plain = unescape_basic(&strip_tags(value));
                let lines: Vec<&str> = plain
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .collect();
                meta.description = Some(lines.join("\n")).filter(|s| !s.is_empty());
            }
            "identifier" if meta.identifier.is_none() => {
                let id = collapse_ws(&unescape_basic(&strip_tags(value)));
                meta.identifier = Some(id).filter(|s| !s.is_empty());
            }
            _ => {}
        }
    }
    meta
}

fn parse_manifest(opf: &str, opf_dir: &str) -> Vec<ManifestItem> {
    let mut items = Vec::new();
    for tag in re_manifest_item().find_iter(opf) {
        let attrs = parse_attrs(tag.as_str());
        let Some(href) = attrs.get("href").filter(|s| !s.trim().is_empty()) else {
            continue;
        };
        items.push(ManifestItem {
            id: attrs.get("id").cloned().unwrap_or_default(),
            href: normalize_zip_path(opf_dir, href),
            media_type: attrs
                .get("media-type")
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default(),
            properties: attrs
                .get("properties")
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default(),
        });
    }
    items
}

fn parse_spine_ids(opf: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for tag in re_itemref().find_iter(opf) {
        let attrs = parse_attrs(tag.as_str());
        if attrs.get("linear").is_some_and(|v| v.eq_ignore_ascii_case("no")) {
            continue;
        }
        if let Some(idref) = attrs.get("idref").filter(|s| !s.trim().is_empty()) {
            ids.push(idref.clone());
        }
    }
    ids
}

/// 从 `<spine toc="ncx">` 里取出 NCX 的 manifest id。
fn spine_toc_id(opf: &str) -> Option<String> {
    re_spine()
        .find(opf)
        .and_then(|m| parse_attrs(m.as_str()).get("toc").cloned())
        .filter(|s| !s.trim().is_empty())
}

fn is_xhtml(item: &ManifestItem) -> bool {
    item.media_type.contains("xhtml")
        || item.media_type.contains("html")
        || item.href.to_ascii_lowercase().ends_with(".xhtml")
        || item.href.to_ascii_lowercase().ends_with(".html")
        || item.href.to_ascii_lowercase().ends_with(".htm")
}

// ── 目录（NCX / nav.xhtml）标题映射 ─────────────────────────────────

/// 解析 toc.ncx：按文档顺序把 `<text>` 与随后的 `<content src>` 配对。
///
/// 只取 `<navMap>` 内部，避免 `<docTitle>` / `<docAuthor>` 里的 text 打乱配对。
fn parse_ncx_titles(ncx: &str, ncx_dir: &str) -> HashMap<String, String> {
    let nav_map = re_ncx_navmap()
        .captures(ncx)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str())
        .unwrap_or(ncx);

    let texts: Vec<String> = re_ncx_text()
        .captures_iter(nav_map)
        .filter_map(|caps| {
            let raw = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let title = collapse_ws(&unescape_basic(&strip_tags(raw)));
            (!title.is_empty()).then_some(title)
        })
        .collect();

    let mut map = HashMap::new();
    let mut text_iter = texts.into_iter().peekable();
    for tag in re_ncx_content().find_iter(nav_map) {
        let attrs = parse_attrs(tag.as_str());
        let Some(src) = attrs.get("src") else {
            continue;
        };
        let href = normalize_zip_path(ncx_dir, src);
        if let Some(title) = text_iter.next() {
            map.entry(href).or_insert(title);
        }
    }
    map
}

/// 解析 EPUB3 nav 文档中的目录锚点。
fn parse_nav_titles(nav: &str, nav_dir: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for caps in re_nav_anchor().captures_iter(nav) {
        let Some(open_tag) = caps.get(0) else { continue };
        // 只取 <a ... href="..."> 的起始标签属性
        let start_tag_end = open_tag.as_str().find('>').unwrap_or(open_tag.as_str().len());
        let attrs = parse_attrs(&open_tag.as_str()[..start_tag_end]);
        let Some(href) = attrs.get("href").filter(|s| !s.trim().is_empty()) else {
            continue;
        };
        if href.contains("://") {
            continue;
        }
        let title = collapse_ws(&unescape_basic(&strip_tags(
            caps.get(1).map(|m| m.as_str()).unwrap_or(""),
        )));
        if title.is_empty() {
            continue;
        }
        map.entry(normalize_zip_path(nav_dir, href)).or_insert(title);
    }
    map
}

fn html_heading_title(html: &str) -> String {
    if let Some(caps) = re_html_title().captures(html) {
        let title = collapse_ws(&unescape_basic(&strip_tags(
            caps.get(1).map(|m| m.as_str()).unwrap_or(""),
        )));
        if !title.is_empty() {
            return title;
        }
    }
    if let Some(caps) = re_html_h1().captures(html) {
        let title = collapse_ws(&unescape_basic(&strip_tags(
            caps.get(1).map(|m| m.as_str()).unwrap_or(""),
        )));
        if !title.is_empty() {
            return title;
        }
    }
    String::new()
}

// ── 读取一本 EPUB ───────────────────────────────────────────────────

pub fn read_epub(path: &Path) -> Result<EpubBook> {
    let file =
        File::open(path).with_context(|| format!("打开 EPUB 失败: {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("解析 EPUB（zip）失败: {}", path.display()))?;
    let index = build_name_index(&mut archive);

    let opf_path = read_entry(&mut archive, &index, "META-INF/container.xml")
        .and_then(|xml| {
            re_rootfile()
                .find(&xml)
                .and_then(|m| parse_attrs(m.as_str()).get("full-path").cloned())
        })
        .filter(|p| !p.trim().is_empty())
        .or_else(|| find_first_opf(&index))
        .ok_or_else(|| anyhow!("EPUB 缺少 OPF（container.xml/rootfile）: {}", path.display()))?;

    let opf = read_entry(&mut archive, &index, &opf_path)
        .ok_or_else(|| anyhow!("无法读取 OPF: {opf_path}"))?;
    let opf_dir = parent_dir(&opf_path).to_string();

    let meta = parse_metadata(&opf);
    let manifest = parse_manifest(&opf, &opf_dir);
    let by_id: HashMap<&str, &ManifestItem> =
        manifest.iter().map(|item| (item.id.as_str(), item)).collect();

    // 正文顺序：spine → manifest
    let mut docs: Vec<String> = Vec::new();
    for idref in parse_spine_ids(&opf) {
        let Some(item) = by_id.get(idref.as_str()) else {
            continue;
        };
        if item.properties.contains("nav") {
            // EPUB3 目录页不算正文
            continue;
        }
        docs.push(item.href.clone());
    }
    if docs.is_empty() {
        warn!(
            target: "epub2txt",
            path = %path.display(),
            "EPUB spine 为空，回退为 manifest 中的全部 XHTML"
        );
        docs.extend(
            manifest
                .iter()
                .filter(|item| is_xhtml(item) && !item.properties.contains("nav"))
                .map(|item| item.href.clone()),
        );
    }

    // 标题映射：NCX 优先，其次 EPUB3 nav
    let mut title_map: HashMap<String, String> = HashMap::new();
    let ncx_item = spine_toc_id(&opf)
        .and_then(|id| by_id.get(id.as_str()).copied())
        .or_else(|| {
            manifest
                .iter()
                .find(|item| item.media_type.contains("dtbncx") || item.href.ends_with(".ncx"))
        });
    if let Some(item) = ncx_item
        && let Some(ncx) = read_entry(&mut archive, &index, &item.href)
    {
        title_map.extend(parse_ncx_titles(&ncx, parent_dir(&item.href)));
    }
    if let Some(item) = manifest.iter().find(|item| item.properties.contains("nav"))
        && let Some(nav) = read_entry(&mut archive, &index, &item.href)
    {
        for (k, v) in parse_nav_titles(&nav, parent_dir(&item.href)) {
            title_map.entry(k).or_insert(v);
        }
    }

    let mut chapters = Vec::new();
    let mut skipped = 0usize;
    for href in &docs {
        let raw = read_entry(&mut archive, &index, href);
        let mut title = title_map.get(href).cloned().unwrap_or_default();

        let text = match raw {
            Some(raw) => {
                if title.is_empty() {
                    title = html_heading_title(&raw);
                }
                // clean_xhtml 负责去掉 header/script/style 并保留段落，
                // clean_plain 再统一解码实体、加首行缩进并去掉重复标题行。
                let xhtml = ContentParser::clean_xhtml(&raw, &title);
                ContentParser::clean_plain(&xhtml, &title)
            }
            None => {
                skipped += 1;
                warn!(
                    target: "epub2txt",
                    href,
                    "章节文件缺失，写入占位"
                );
                String::new()
            }
        };

        if text.trim().is_empty() && title.trim().is_empty() {
            // 封面页/空白页：不进入 txt
            skipped += 1;
            continue;
        }

        let title = if title.trim().is_empty() {
            format!("章节 {}", chapters.len() + 1)
        } else {
            title
        };
        chapters.push(EpubChapter { title, text });
    }

    if chapters.is_empty() {
        return Err(anyhow!("EPUB 未解析到任何章节: {}", path.display()));
    }
    if skipped > 0 {
        info!(target: "epub2txt", path = %path.display(), skipped, "部分章节为空或缺失");
    }

    let title = meta
        .title
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.trim().to_string())
        });

    Ok(EpubBook {
        title,
        author: meta.author,
        description: meta.description,
        identifier: meta.identifier,
        chapters,
        skipped_pages: skipped,
    })
}

// ── 输出 ────────────────────────────────────────────────────────────

fn push_line(out: &mut String, text: &str) {
    out.push_str(text);
    out.push('\n');
}

fn render_txt(book: &EpubBook) -> String {
    let mut out = String::new();

    let title = book
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("未命名");
    push_line(&mut out, &format!("书名：{title}"));

    if let Some(author) = book.author.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        push_line(&mut out, &format!("作者：{author}"));
    }

    if let Some(id) = book
        .identifier
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if id.chars().all(|c| c.is_ascii_digit()) {
            push_line(&mut out, &format!("book_id={id}"));
        } else {
            push_line(&mut out, &format!("identifier={id}"));
        }
    }

    push_line(&mut out, "状态：未知");

    if let Some(desc) = book
        .description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        push_line(&mut out, "");
        push_line(&mut out, "简介：");
        push_line(&mut out, desc);
    }

    push_line(&mut out, "");
    push_line(&mut out, &"=".repeat(40));
    push_line(&mut out, "");

    for chapter in &book.chapters {
        let body = if chapter.text.trim().is_empty() {
            "（本章无正文内容）".to_string()
        } else {
            chapter.text.trim_end().to_string()
        };
        push_line(&mut out, &chapter.title);
        push_line(&mut out, "");
        push_line(&mut out, &body);
        push_line(&mut out, "");
        push_line(&mut out, "----------------------------------------");
        push_line(&mut out, "");
    }

    out
}

fn default_txt_path(epub_path: &Path) -> PathBuf {
    epub_path.with_extension("txt")
}

/// 转换单个 EPUB；`out_path` 为空时输出到源文件同目录同名 `.txt`。
pub fn convert_epub(epub_path: &Path, out_path: Option<&Path>) -> Result<ConvertSummary> {
    let book = read_epub(epub_path)?;
    let txt_path = out_path
        .map(PathBuf::from)
        .unwrap_or_else(|| default_txt_path(epub_path));

    if let Some(parent) = txt_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建输出目录失败: {}", parent.display()))?;
    }

    let content = render_txt(&book);
    fs::write(&txt_path, content.as_bytes())
        .with_context(|| format!("写入 TXT 失败: {}", txt_path.display()))?;

    let summary = ConvertSummary {
        epub_path: epub_path.to_path_buf(),
        txt_path,
        chapter_count: book.chapters.len(),
        skipped_chapters: book.skipped_pages,
        bytes: content.len() as u64,
    };

    info!(
        target: "epub2txt",
        epub = %summary.epub_path.display(),
        txt = %summary.txt_path.display(),
        chapters = summary.chapter_count,
        bytes = summary.bytes,
        "EPUB 转 TXT 完成"
    );
    Ok(summary)
}

fn is_epub_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
}

fn collect_epub_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = fs::read_dir(&current)
            .with_context(|| format!("读取目录失败: {}", current.display()))?;
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            // 不跟随符号链接，避免目录环
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                stack.push(path);
            } else if is_epub_path(&path) {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// 转换路径：可传单个 `.epub`，也可传目录（递归转换其中所有 epub）。
///
/// `out_path` 在单文件时表示目标 txt 路径；目录模式下必须是不存在的或已存在的目录。
pub fn convert_path(path: &Path, out_path: Option<&Path>) -> Result<BatchResult> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("路径不存在或不可访问: {}", path.display()))?;
    let mut result = BatchResult::default();

    if metadata.is_file() {
        if !is_epub_path(path) {
            return Err(anyhow!("不是 .epub 文件: {}", path.display()));
        }
        match convert_epub(path, out_path) {
            Ok(summary) => result.converted.push(summary),
            Err(err) => result
                .failures
                .push((path.to_path_buf(), format!("{err:#}"))),
        }
        return Ok(result);
    }

    if !metadata.is_dir() {
        return Err(anyhow!("不是文件也不是目录: {}", path.display()));
    }

    let epub_files = collect_epub_files(path)?;
    if epub_files.is_empty() {
        return Err(anyhow!(
            "目录下未找到任何 .epub 文件: {}",
            path.display()
        ));
    }

    let out_dir: Option<PathBuf> = match out_path {
        Some(target) => {
            if target.exists() && !target.is_dir() {
                return Err(anyhow!(
                    "批量转换时输出路径必须是目录: {}",
                    target.display()
                ));
            }
            Some(target.to_path_buf())
        }
        None => None,
    };

    for epub in epub_files {
        let target = match out_dir.as_ref() {
            Some(dir) => {
                let stem = epub
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("output");
                Some(dir.join(format!("{stem}.txt")))
            }
            None => None,
        };
        match convert_epub(&epub, target.as_deref()) {
            Ok(summary) => result.converted.push(summary),
            Err(err) => result
                .failures
                .push((epub.clone(), format!("{err:#}"))),
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{convert_epub, convert_path, read_epub};
    use crate::base_system::context::Config;
    use crate::book_parser::epub_generator::EpubGenerator;

    fn build_sample_epub(dir: &std::path::Path) -> std::path::PathBuf {
        let cfg = Config {
            save_path: dir.display().to_string(),
            ..Config::default()
        };
        let mut generator =
            EpubGenerator::new("7423591956359416856", "测试书名", "测试作者", "标签", "这是一段简介", &cfg)
                .unwrap();
        generator.add_chapter("第1章 开局", "<p>第一段正文</p><p>第二段正文</p>");
        generator.add_chapter("第2章 继续", "<p>第二章正文</p>");

        let path = dir.join("测试书名.epub");
        generator.generate(&path, &cfg).unwrap();
        path
    }

    #[test]
    fn read_epub_extracts_metadata_and_chapters_in_spine_order() {
        let temp = tempfile::tempdir().unwrap();
        let epub = build_sample_epub(temp.path());

        let book = read_epub(&epub).unwrap();
        assert_eq!(book.title.as_deref(), Some("测试书名"));
        assert_eq!(book.author.as_deref(), Some("测试作者"));
        assert_eq!(book.identifier.as_deref(), Some("7423591956359416856"));
        assert_eq!(book.chapters.len(), 2);
        assert!(book.chapters[0].title.contains("第1章"));
        assert!(book.chapters[0].text.contains("第一段正文"));
        assert!(book.chapters[1].text.contains("第二章正文"));
    }

    #[test]
    fn convert_epub_writes_txt_next_to_source() {
        let temp = tempfile::tempdir().unwrap();
        let epub = build_sample_epub(temp.path());

        let summary = convert_epub(&epub, None).unwrap();
        assert_eq!(summary.chapter_count, 2);
        assert_eq!(summary.txt_path, temp.path().join("测试书名.txt"));

        let content = std::fs::read_to_string(&summary.txt_path).unwrap();
        assert!(content.contains("书名：测试书名"));
        assert!(content.contains("作者：测试作者"));
        assert!(content.contains("book_id=7423591956359416856"));
        assert!(content.contains("第1章 开局"));
        assert!(content.contains("第一段正文"));
        assert!(content.contains("第2章 继续"));
    }

    #[test]
    fn convert_path_handles_directory_batch() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        std::fs::create_dir_all(src).unwrap();
        build_sample_epub(&src);

        let out = temp.path().join("out");
        std::fs::create_dir_all(&out).unwrap();

        let result = convert_path(&src, Some(out.as_path())).unwrap();
        assert_eq!(result.converted.len(), 1);
        assert!(result.failures.is_empty());
        assert!(out.join("测试书名.txt").exists());
    }
}
