//! HTML/XHTML 文本处理工具。
//!
//! 转义、清理 EPUB 正文、描述渲染等纯文本操作。

use regex::Regex;
use std::sync::OnceLock;

// 编译一次复用的正则缓存
fn re_epub_token() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?is)(<img\b[^>]*?>)|(<p\b[^>]*?>.*?</p>)|(<h[1-6]\b[^>]*?>.*?</h[1-6]>)")
            .unwrap()
    })
}

fn re_src_attr() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?is)\bsrc\s*=\s*['"]([^'"]+)['"]"#).unwrap())
}

fn re_img_tag() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?is)<img\b[^>]*?>"#).unwrap())
}

fn re_all_tags() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<[^>]+>").unwrap())
}

// ── 实体解码 ────────────────────────────────────────────────────

pub(crate) fn decode_xhtml_attr_url(src: &str) -> std::borrow::Cow<'_, str> {
    if src.contains("&amp;") {
        return std::borrow::Cow::Owned(src.replace("&amp;", "&"));
    }
    std::borrow::Cow::Borrowed(src)
}

pub(crate) fn unescape_basic_entities(s: &str) -> std::borrow::Cow<'_, str> {
    if !(s.contains("&amp;")
        || s.contains("&lt;")
        || s.contains("&gt;")
        || s.contains("&quot;")
        || s.contains("&#34;")
        || s.contains("&#39;")
        || s.contains("&#x27;")
        || s.contains("&#x22;")
        || s.contains("&nbsp;"))
    {
        return std::borrow::Cow::Borrowed(s);
    }

    std::borrow::Cow::Owned(
        s.replace("&nbsp;", " ")
            .replace("&quot;", "\"")
            .replace("&#34;", "\"")
            .replace("&#x22;", "\"")
            .replace("&#39;", "'")
            .replace("&#x27;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

// ── HTML 转义 ───────────────────────────────────────────────────

pub(crate) fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// ── HTML 检测 ───────────────────────────────────────────────────

fn looks_like_html(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }

    // Fast-path: most real HTML descriptions include <p> or <br>.
    let lower = t.to_ascii_lowercase();
    lower.contains("<p")
        || lower.contains("</p")
        || lower.contains("<br")
        || lower.contains("<div")
        || lower.contains("<span")
        || lower.contains("<a ")
        || lower.contains("<img")
}

// ── script/style 移除 ──────────────────────────────────────────

fn strip_script_and_style_blocks(html: &str) -> String {
    fn remove_tag_block(input: &str, tag: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let lower = input.to_ascii_lowercase();
        let open_pat = format!("<{}", tag);
        let close_pat = format!("</{}>", tag);

        let mut i = 0;
        while i < input.len() {
            if lower[i..].starts_with(&open_pat) {
                if let Some(close_pos) = lower[i..].find(&close_pat) {
                    i += close_pos + close_pat.len();
                    continue;
                } else {
                    break;
                }
            }

            let ch = input[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }

        out
    }

    let without_script = remove_tag_block(html, "script");
    remove_tag_block(&without_script, "style")
}

// ── HTML → XHTML 片段归一化 ────────────────────────────────────

fn normalize_html_to_xhtml_fragment(html: &str) -> String {
    let mut s = strip_script_and_style_blocks(html);

    // Normalize line endings.
    s = s.replace("\r\n", "\n").replace('\r', "\n");

    // <br> must be self-closed.
    s = s
        .replace("<br>", "<br/>")
        .replace("<br />", "<br/>")
        .replace("<BR>", "<br/>")
        .replace("<BR />", "<br/>");

    // Some sources wrap content with <article>...</article><footer>...</footer>.
    let lower = s.to_ascii_lowercase();
    if lower.contains("<article")
        && let (Some(a_start), Some(a_end)) = (lower.find("<article"), lower.rfind("</article>"))
        && let Some(gt) = lower[a_start..].find('>')
    {
        let body_start = a_start + gt + 1;
        let body_end = a_end;
        if body_start <= body_end && body_end <= s.len() {
            s = s[body_start..body_end].to_string();
        }
    }

    s.trim().to_string()
}

// ── 描述渲染 ────────────────────────────────────────────────────

pub(crate) fn render_description_xhtml_fragment(description: &str) -> String {
    let raw = description.trim();
    if raw.is_empty() {
        return "<p></p>".to_string();
    }

    if looks_like_html(raw) {
        let normalized = normalize_html_to_xhtml_fragment(raw);
        if normalized.is_empty() {
            return "<p></p>".to_string();
        }
        return normalized;
    }

    // Plain-text: preserve line breaks as empty <p></p> and normal paragraphs.
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::new();
    for line in normalized.split('\n') {
        let trimmed_end = line.trim_end();
        if trimmed_end.trim().is_empty() {
            out.push_str("<p></p>");
        } else {
            out.push_str("<p>");
            out.push_str(&escape_html(trimmed_end));
            out.push_str("</p>");
        }
    }
    out
}

pub(crate) fn description_to_plain_text(description: &str) -> String {
    let raw = description.trim();
    if raw.is_empty() {
        return String::new();
    }
    if !looks_like_html(raw) {
        return raw.split_whitespace().collect::<Vec<_>>().join(" ");
    }

    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ => {
                if !in_tag {
                    out.push(ch);
                }
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ── EPUB 正文清理 ──────────────────────────────────────────────

pub(crate) fn clean_epub_body(html: &str) -> String {
    let re_token = re_epub_token();
    let re_src = re_src_attr();
    let re_img = re_img_tag();
    let re_tags = re_all_tags();

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
