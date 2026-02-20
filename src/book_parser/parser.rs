//! 章节内容解析与文本处理。

use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::base_system::context::Config;

// 编译一次复用的正则表达式缓存
fn re_breaks() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?is)<br\s*/?>|</p\s*>|</div\s*>|</section\s*>|</h[1-6]\s*>").unwrap()
    })
}

fn re_open_p() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<p\b[^>]*>").unwrap())
}

fn re_para() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<p[^>]*>(.*?)</p>").unwrap())
}

fn re_strip_tags() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"<[^>]+>").unwrap())
}

fn re_strip_header() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"<header[^>]*>.*?</header>").unwrap())
}

fn re_strip_script() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"<script[^>]*>.*?</script>").unwrap())
}

fn re_strip_style() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"<style[^>]*>.*?</style>").unwrap())
}

fn re_strip_comments() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").unwrap())
}

fn re_extract_body() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)<body[^>]*>(.*?)</body>").unwrap())
}

fn re_br_normalize() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)<br\s*/?>").unwrap())
}

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
        let normalized = re_breaks().replace_all(raw, "\n");
        let normalized = re_open_p().replace_all(&normalized, "\n");
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
        let re_para = re_para();
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
        let s = re_strip_tags().replace_all(raw, "");
        s.replace("\r\n", "\n").replace('\r', "\n")
    }

    fn strip_header(raw: &str) -> String {
        // 移除 <header>...</header> 以及 <script>...</script>
        let tmp = re_strip_header().replace_all(raw, "");
        let tmp = re_strip_script().replace_all(&tmp, "");
        re_strip_style().replace_all(&tmp, "").to_string()
    }

    fn strip_comments(raw: &str) -> String {
        re_strip_comments().replace_all(raw, "").to_string()
    }

    fn extract_body(raw: &str) -> Option<String> {
        re_extract_body()
            .captures(raw)
            .and_then(|cap| cap.get(1))
            .map(|m| m.as_str().to_string())
    }

    fn sanitize_paragraph(inner: &str) -> String {
        // 保留换行，将 <br> 视为换行，去掉其他标签。
        let with_newlines = re_br_normalize().replace_all(inner, "\n");
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

        use std::sync::OnceLock;
        static RE_DECIMAL: OnceLock<Regex> = OnceLock::new();
        static RE_HEX: OnceLock<Regex> = OnceLock::new();

        let re_decimal = RE_DECIMAL.get_or_init(|| Regex::new(r"&#(\d+);").unwrap());
        let re_hex = RE_HEX.get_or_init(|| Regex::new(r"&#[xX]([0-9a-fA-F]+);").unwrap());

        let mut result = s.to_string();

        // Decode decimal numeric entities (&#NNN;)
        result = re_decimal
            .replace_all(&result, |caps: &regex::Captures| {
                if let Some(num_str) = caps.get(1)
                    && let Ok(code_point) = num_str.as_str().parse::<u32>()
                {
                    // Validate code point is in valid Unicode range (0 to 0x10FFFF)
                    if code_point <= 0x10FFFF
                        && let Some(ch) = char::from_u32(code_point)
                    {
                        return ch.to_string();
                    }
                }
                caps[0].to_string() // Return original if parsing fails
            })
            .to_string();

        // Decode hexadecimal numeric entities (&#xHH; or &#XHH;)
        result = re_hex
            .replace_all(&result, |caps: &regex::Captures| {
                if let Some(hex_str) = caps.get(1)
                    && let Ok(code_point) = u32::from_str_radix(hex_str.as_str(), 16)
                {
                    // Validate code point is in valid Unicode range (0 to 0x10FFFF)
                    if code_point <= 0x10FFFF
                        && let Some(ch) = char::from_u32(code_point)
                    {
                        return ch.to_string();
                    }
                }
                caps[0].to_string() // Return original if parsing fails
            })
            .to_string();

        // Then decode named entities
        result
            .replace("&nbsp;", " ")
            // Straight quotes and apostrophes
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            // Curly quotes (common in Chinese novels)
            .replace("&ldquo;", "\u{201C}")
            .replace("&rdquo;", "\u{201D}")
            .replace("&lsquo;", "\u{2018}")
            .replace("&rsquo;", "\u{2019}")
            .replace("&sbquo;", "\u{201A}")
            .replace("&bdquo;", "\u{201E}")
            // Dashes (common in Chinese novels)
            .replace("&ndash;", "\u{2013}")
            .replace("&mdash;", "\u{2014}")
            // Ellipsis
            .replace("&hellip;", "\u{2026}")
            // Other punctuation
            .replace("&bull;", "\u{2022}")
            .replace("&shy;", "\u{00AD}")
            // Angle brackets
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&lsaquo;", "\u{2039}")
            .replace("&rsaquo;", "\u{203A}")
            // Must be last to avoid double-decoding
            .replace("&amp;", "&")
    }
}
