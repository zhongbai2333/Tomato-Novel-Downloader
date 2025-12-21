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
                .unwrap_or_else(|| cid.as_str());

            let processed = if cfg.novel_format.eq_ignore_ascii_case("txt") {
                Self::clean_plain(raw_content)
            } else {
                Self::clean_xhtml(raw_content, title)
            };

            out.insert(cid.clone(), (processed, title.to_string()));
        }

        out
    }

    /// 纯文本清洗：移除标签、统一换行并添加简单缩进。
    pub fn clean_plain(raw: &str) -> String {
        let without_tags = Self::strip_tags(raw);
        let mut lines = Vec::new();
        for line in without_tags.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            lines.push(format!("　　{}", trimmed));
        }
        if lines.is_empty() {
            without_tags.trim().to_string()
        } else {
            lines.join("\n")
        }
    }

    /// 简化的 XHTML 清洗：去掉 <header> 与脚本，保留主体文本。
    pub fn clean_xhtml(raw: &str, title: &str) -> String {
        let body = Self::strip_header(raw);
        if body.trim().is_empty() {
            return String::new();
        }
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>{}</title></head><body><h1>{}</h1><div>{}</div></body></html>",
            Self::escape_html(title),
            Self::escape_html(title),
            body
        )
    }

    /// 解析书籍信息（从 HTML 文本），回退实现：尝试抓取标题/作者/简介/标签/章节数。
    pub fn parse_book_info(
        html: &str,
        _book_id: &str,
    ) -> (String, String, String, Vec<String>, usize) {
        let title = Self::capture_text(html, r"<h1[^>]*>(.*?)</h1>")
            .unwrap_or_else(|| "未知书名".to_string());
        let author = Self::capture_text(html, r"author-name[^>]*>\s*<span[^>]*>(.*?)</span>")
            .unwrap_or_else(|| "未知作者".to_string());
        let description =
            Self::capture_text(html, r"page-abstract-content[^>]*>\s*<p[^>]*>(.*?)</p>")
                .unwrap_or_else(|| "无简介".to_string());

        let tag_re = Regex::new(r"info-label[^>]*>\s*([^<]+)\s*<").ok();
        let mut tags = Vec::new();
        if let Some(re) = tag_re {
            for cap in re.captures_iter(html) {
                if let Some(m) = cap.get(1) {
                    tags.push(m.as_str().trim().to_string());
                }
            }
        }

        let count = Self::capture_text(html, r"共\s*(\d+)\s*章")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);

        (title, author, description, tags, count)
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
        let tmp = re_header.replace_all(raw, "");
        re_script.replace_all(&tmp, "").to_string()
    }

    fn escape_html(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
    }

    fn capture_text(html: &str, pattern: &str) -> Option<String> {
        let re = Regex::new(pattern).ok()?;
        re.captures(html)
            .and_then(|cap| cap.get(1))
            .map(|m| Self::strip_tags(m.as_str()).trim().to_string())
            .filter(|s| !s.is_empty())
    }
}
