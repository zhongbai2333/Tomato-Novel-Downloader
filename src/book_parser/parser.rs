//! 章节内容解析与文本处理。

use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;

use crate::base_system::context::Config;

pub struct ContentParser;

impl ContentParser {
    /// 解析 API 返回的章节内容映射: chapter_id -> (内容, 标题)
    pub fn extract_api_content(value: &Value, cfg: &Config) -> HashMap<String, (String, String)> {
        let mut out = HashMap::new();
        let data = value
            .get("data")
            .and_then(|v| v.as_object())
            .or_else(|| value.as_object());

        let Some(map) = data else {
            return out;
        };

        for (cid, info) in map {
            let obj = info.as_object();
            let raw_content = obj
                .and_then(|o| o.get("content"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let title = obj
                .and_then(|o| o.get("title"))
                .and_then(Value::as_str)
                .or_else(|| {
                    obj.and_then(|o| o.get("origin_chapter_title"))
                        .and_then(Value::as_str)
                })
                .unwrap_or(cid.as_str());

            let processed = if cfg.novel_format.eq_ignore_ascii_case("txt") {
                Self::clean_plain(raw_content)
            } else if cfg.novel_format.eq_ignore_ascii_case("epub") {
                // EPUB 模式：尽量保留原始 XHTML（包括 <img> 等标签），后续在 finalize 阶段
                // 负责下载并替换图片资源。
                Self::prepare_epub_xhtml(raw_content)
            } else {
                Self::clean_xhtml(raw_content, title)
            };

            out.insert(cid.clone(), (processed, title.to_string()));
        }

        out
    }

    /// EPUB 专用：保留正文 XHTML，移除 header/script/style 并抽取 body 内容。
    fn prepare_epub_xhtml(raw: &str) -> String {
        let stripped = Self::strip_header(raw);
        let body = Self::extract_body(&stripped).unwrap_or(stripped);
        Self::strip_comments(&body)
    }

    /// 纯文本清洗：移除标签、统一换行并添加简单缩进。
    pub fn clean_plain(raw: &str) -> String {
        // Many chapters come as XHTML fragments (<p>, <br>, etc.).
        // If we strip tags directly, paragraphs collapse into a single line.
        let re_breaks =
            Regex::new(r"(?is)<br\s*/?>|</p\s*>|</div\s*>|</section\s*>|</h[1-6]\s*>").unwrap();
        let re_open_p = Regex::new(r"(?is)<p\b[^>]*>").unwrap();

        let normalized = re_breaks.replace_all(raw, "\n");
        let normalized = re_open_p.replace_all(&normalized, "\n");
        let normalized = normalized.replace("\r\n", "\n").replace('\r', "\n");

        let without_tags = Self::strip_tags(&normalized);
        let without_tags = Self::unescape_html_entities(&without_tags);
        let without_tags = without_tags.replace("\r\n", "\n").replace('\r', "\n");

        // Keep paragraph breaks: output blank lines between paragraphs.
        let mut out = Vec::new();
        let mut last_blank = true;
        for line in without_tags.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                if !last_blank {
                    out.push(String::new());
                    last_blank = true;
                }
                continue;
            }
            last_blank = false;
            out.push(format!("　　{}", trimmed));
        }

        while out.last().is_some_and(|l| l.trim().is_empty()) {
            out.pop();
        }

        if out.is_empty() {
            without_tags.trim().to_string()
        } else {
            out.join("\n")
        }
    }

    /// 简化的 XHTML 清洗：去掉 <header> 与脚本，保留主体文本。
    pub fn clean_xhtml(raw: &str, _title: &str) -> String {
        let stripped = Self::strip_header(raw);
        let body = Self::extract_body(&stripped).unwrap_or(stripped);
        let body = Self::strip_comments(&body);
        let mut paragraphs = Vec::new();

        // 尝试提取已有段落，清理标签，保留基本文本。
        let re_para = Regex::new(r"(?is)<p[^>]*>(.*?)</p>").unwrap();
        for cap in re_para.captures_iter(&body) {
            let inner = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let cleaned = Self::sanitize_paragraph(inner);
            if !cleaned.is_empty() {
                paragraphs.push(format!("<p>{}</p>", cleaned));
            }
        }

        if paragraphs.is_empty() {
            let plain = Self::strip_tags(&body);
            for line in plain.split('\n') {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    paragraphs.push(format!("<p>{}</p>", Self::escape_html(trimmed)));
                }
            }
        }

        paragraphs.join("\n")
    }

    fn strip_tags(raw: &str) -> String {
        // 粗暴去标签，避免引入额外 HTML 解析库
        let re = Regex::new(r"<[^>]+>").unwrap();
        let s = re.replace_all(raw, "");
        s.replace("\r\n", "\n").replace('\r', "\n")
    }

    fn strip_header(raw: &str) -> String {
        // 移除 <header>...</header> 以及 <script>...</script>
        let re_header = Regex::new(r"<header[^>]*>.*?</header>").unwrap();
        let re_script = Regex::new(r"<script[^>]*>.*?</script>").unwrap();
        let re_style = Regex::new(r"<style[^>]*>.*?</style>").unwrap();
        let tmp = re_header.replace_all(raw, "");
        let tmp = re_script.replace_all(&tmp, "");
        re_style.replace_all(&tmp, "").to_string()
    }

    fn strip_comments(raw: &str) -> String {
        let re = Regex::new(r"(?s)<!--.*?-->").unwrap();
        re.replace_all(raw, "").to_string()
    }

    fn extract_body(raw: &str) -> Option<String> {
        let re = Regex::new(r"(?is)<body[^>]*>(.*?)</body>").ok()?;
        re.captures(raw)
            .and_then(|cap| cap.get(1))
            .map(|m| m.as_str().to_string())
    }

    fn sanitize_paragraph(inner: &str) -> String {
        // 保留换行，将 <br> 视为换行，去掉其他标签。
        let br_normalized = Regex::new(r"(?i)<br\s*/?>").unwrap();
        let with_newlines = br_normalized.replace_all(inner, "\n");
        let text = Self::strip_tags(&with_newlines);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        Self::escape_html(trimmed)
    }

    fn escape_html(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
    }

    fn unescape_html_entities(s: &str) -> String {
        // Decode common HTML entities that may appear in the API response
        // Note: &amp; must be replaced last to avoid double-decoding issues
        if !(s.contains('&')) {
            return s.to_string();
        }

        s.replace("&nbsp;", " ")
            // Straight quotes and apostrophes
            .replace("&quot;", "\"")
            .replace("&#34;", "\"")
            .replace("&#x22;", "\"")
            .replace("&apos;", "'")
            .replace("&#39;", "'")
            .replace("&#x27;", "'")
            // Curly quotes (common in Chinese novels)
            .replace("&ldquo;", "\u{201C}")
            .replace("&#8220;", "\u{201C}")
            .replace("&#x201C;", "\u{201C}")
            .replace("&rdquo;", "\u{201D}")
            .replace("&#8221;", "\u{201D}")
            .replace("&#x201D;", "\u{201D}")
            .replace("&lsquo;", "\u{2018}")
            .replace("&#8216;", "\u{2018}")
            .replace("&#x2018;", "\u{2018}")
            .replace("&rsquo;", "\u{2019}")
            .replace("&#8217;", "\u{2019}")
            .replace("&#x2019;", "\u{2019}")
            .replace("&sbquo;", "\u{201A}")
            .replace("&#8218;", "\u{201A}")
            .replace("&bdquo;", "\u{201E}")
            .replace("&#8222;", "\u{201E}")
            // Dashes (common in Chinese novels)
            .replace("&ndash;", "\u{2013}")
            .replace("&#8211;", "\u{2013}")
            .replace("&#x2013;", "\u{2013}")
            .replace("&mdash;", "\u{2014}")
            .replace("&#8212;", "\u{2014}")
            .replace("&#x2014;", "\u{2014}")
            // Ellipsis
            .replace("&hellip;", "\u{2026}")
            .replace("&#8230;", "\u{2026}")
            .replace("&#x2026;", "\u{2026}")
            // Other punctuation
            .replace("&bull;", "\u{2022}")
            .replace("&#8226;", "\u{2022}")
            .replace("&shy;", "\u{00AD}")
            // Angle brackets
            .replace("&lt;", "<")
            .replace("&#60;", "<")
            .replace("&#x3C;", "<")
            .replace("&#x3c;", "<")
            .replace("&gt;", ">")
            .replace("&#62;", ">")
            .replace("&#x3E;", ">")
            .replace("&#x3e;", ">")
            .replace("&lsaquo;", "\u{2039}")
            .replace("&#8249;", "\u{2039}")
            .replace("&rsaquo;", "\u{203A}")
            .replace("&#8250;", "\u{203A}")
            // Must be last to avoid double-decoding
            .replace("&amp;", "&")
    }
}
