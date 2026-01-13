//! 段评/评论相关的解析与拼装工具。

use regex::Regex;
use std::sync::OnceLock;

// Compiled regexes for performance (compiled once, reused across calls)
fn class_attr_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    // Note: This pattern handles typical HTML class attributes but doesn't handle edge cases
    // like escaped quotes within the class value. In practice, Tomato Novel API HTML doesn't
    // use such complex patterns.
    REGEX.get_or_init(|| Regex::new(r#"(?i)\bclass\s*=\s*["']([^"']*)["']"#).unwrap())
}

// Regex to match both paragraphs and headings (for skipping headings like clean_epub_body does)
fn para_and_heading_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?is)(<p\b[^>]*>)(.*?)(</p>)|(<h[1-6]\b[^>]*?>.*?</h[1-6]>)").unwrap()
    })
}

fn id_attr_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?is)\bid\s*=").unwrap())
}

fn html_tags_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"<[^>]+>").unwrap())
}

fn bracket_emoji_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\[([\u4e00-\u9fa5]{1,4})\]").unwrap())
}

/// 将 [笑] 形式的简单表情替换为 emoji。
pub fn convert_bracket_emojis(text: &str) -> String {
    if !text.contains('[') {
        return text.to_string();
    }
    let map = emoji_map();
    bracket_emoji_regex()
        .replace_all(text, |caps: &regex::Captures| {
            let key = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            map.get(key).cloned().unwrap_or_else(|| caps[0].to_string())
        })
        .to_string()
}

pub fn to_cjk_numeral(n: i32) -> String {
    let digits = ["零", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    if n <= 0 {
        return n.to_string();
    }
    if n < 10 {
        return digits[n as usize].to_string();
    }
    if n == 10 {
        return "十".to_string();
    }
    if n < 20 {
        return format!("十{}", digits[(n - 10) as usize]);
    }
    if n < 100 {
        let shi = n / 10;
        let ge = n % 10;
        if ge == 0 {
            format!("{}十", digits[shi as usize])
        } else {
            format!("{}十{}", digits[shi as usize], digits[ge as usize])
        }
    } else {
        n.to_string()
    }
}

/// 检查段落是否应该在段评计数时跳过（如图片包装段落、卷标题等）。
fn should_skip_para_for_comments(open_tag: &str) -> bool {
    // List of class names that should be skipped when counting content paragraphs.
    // Uses linear search which is efficient for small lists (9 items).
    static SKIP_CLASSES: &[&str] = &[
        "picture",
        "volumetitle",
        "volume-title",
        "sectiontitle",
        "section-title",
        "catalogtitle",
        "catalog-title",
        "chaptertitle",
        "chapter-title",
    ];

    // Match class="..." or class='...' with exact class names or class lists
    if let Some(caps) = class_attr_regex().captures(open_tag)
        && let Some(class_list) = caps.get(1)
    {
        let classes = class_list.as_str();
        // Split by whitespace to get individual class names
        for class in classes.split_whitespace() {
            let lower = class.to_ascii_lowercase();
            if SKIP_CLASSES.contains(&lower.as_str()) {
                return true;
            }
        }
    }

    false
}

/// 提取指定段落的纯文本摘要（用于段评回链）。
pub fn extract_para_snippet(chapter_html: &str, target_idx: usize) -> String {
    let mut content_idx = 0;
    for cap in para_and_heading_regex().captures_iter(chapter_html) {
        // Skip headings (group 4) - they're not content paragraphs
        if cap.get(4).is_some() {
            continue;
        }

        let open_tag = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        // Skip non-content paragraphs to match API's paragraph counting
        if should_skip_para_for_comments(open_tag) {
            continue;
        }
        if content_idx == target_idx {
            let inner = cap.get(2).map(|m| m.as_str()).unwrap_or("");
            let inner_text = strip_tags(inner).trim().to_string();
            if inner_text.is_empty() {
                return String::new();
            }
            let cut = ["。", "！", "？", ".", "!", "?", "；", "…"]
                .iter()
                .filter_map(|sep| inner_text.find(sep).map(|p| p + sep.len()))
                .min()
                .unwrap_or_else(|| inner_text.len().min(20));
            return inner_text[..cut].trim().to_string();
        }
        content_idx += 1;
    }
    String::new()
}

pub fn inject_segment_links(
    content_html: &str,
    comments_file: &str,
    seg_counts: &serde_json::Map<String, serde_json::Value>,
) -> String {
    // Mirror Python logic in `segment_utils.py`:
    // - iterate <p> in-order with a monotonically increasing idx
    // - SKIP non-content paragraphs (picture wrappers, volume titles, etc.) to match API counting
    // - SKIP headings (EpubGenerator already injects <h1> for chapter title)
    // - if cnt>0 and <p> has no id=, add id="p-{idx}" while preserving other attrs
    // - append a badge link to the segment comment page

    let mut out = String::new();
    let mut last_end = 0usize;
    let mut content_idx = 0usize; // Index for content paragraphs only

    for m in para_and_heading_regex().find_iter(content_html) {
        out.push_str(&content_html[last_end..m.start()]);

        let caps = para_and_heading_regex().captures(m.as_str()).unwrap();

        // Check if this is a heading (group 4)
        if caps.get(4).is_some() {
            // Skip headings entirely (like clean_epub_body does) since wrap_chapter_html adds <h1>
            last_end = m.end();
            continue;
        }

        // This is a paragraph (groups 1-3)
        let mut open_tag = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
        let mut inner = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
        let close_tag = caps.get(3).map(|m| m.as_str()).unwrap_or("");

        // Skip non-content paragraphs to match API's paragraph counting
        if should_skip_para_for_comments(&open_tag) {
            out.push_str(&open_tag);
            out.push_str(&inner);
            out.push_str(close_tag);
            last_end = m.end();
            continue;
        }

        let cnt = seg_counts
            .get(&content_idx.to_string())
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        if cnt > 0 {
            if !id_attr_regex().is_match(&open_tag) && open_tag.ends_with('>') {
                open_tag.pop();
                open_tag.push_str(&format!(" id=\"p-{}\">", content_idx));
            }
            inner.push_str(&format!(
                " <a class=\"seg-count\" href=\"{}#para-{}\" title=\"查看本段评论\">({})</a>",
                html_escape_attr(comments_file),
                content_idx,
                cnt
            ));
        }

        out.push_str(&open_tag);
        out.push_str(&inner);
        out.push_str(close_tag);

        last_end = m.end();
        content_idx += 1; // Only increment for content paragraphs
    }

    out.push_str(&content_html[last_end..]);
    out
}

fn html_escape_attr(input: &str) -> String {
    // Sufficient for EPUB internal href attr.
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn strip_tags(raw: &str) -> String {
    html_tags_regex().replace_all(raw, "").to_string()
}

fn emoji_map() -> std::collections::HashMap<&'static str, String> {
    use std::collections::HashMap;
    let mut m = HashMap::new();
    m.insert("笑", "😄".to_string());
    m.insert("哭", "😭".to_string());
    m.insert("汗", "😅".to_string());
    m.insert("怒", "😡".to_string());
    m.insert("痛", "😣".to_string());
    m.insert("赞", "👍".to_string());
    m.insert("踩", "👎".to_string());
    m.insert("惊", "😲".to_string());
    m.insert("疑", "🤔".to_string());
    m.insert("色", "😍".to_string());
    m.insert("呆", "😐".to_string());
    m.insert("坏", "😈".to_string());
    m.insert("奸笑", "😏".to_string());
    m.insert("舔屏", "🤤".to_string());
    m.insert("委屈", "🥺".to_string());
    m.insert("飞吻", "😘".to_string());
    m.insert("酷", "😎".to_string());
    m.insert("送心", "💖".to_string());
    m.insert("我也强推", "💯".to_string());
    m.insert("惊呆", "😲".to_string());
    m.insert("偷笑", "🤭".to_string());
    m.insert("翻白眼", "🙄".to_string());
    m.insert("石化", "🗿".to_string());
    m
}
