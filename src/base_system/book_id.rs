//! 书籍 ID/链接解析与规范化。

use regex::Regex;
use std::sync::OnceLock;

static RE_URL: OnceLock<Regex> = OnceLock::new();
static RE_QS: OnceLock<Regex> = OnceLock::new();
static RE_PAGE: OnceLock<Regex> = OnceLock::new();

fn re_url() -> &'static Regex {
    RE_URL.get_or_init(|| Regex::new(r"https?://\S+").expect("compile RE_URL"))
}

fn re_qs() -> &'static Regex {
    RE_QS.get_or_init(|| Regex::new(r"(?i)(book_id|bookId)=([0-9]+)").expect("compile RE_QS"))
}

fn re_page() -> &'static Regex {
    RE_PAGE.get_or_init(|| Regex::new(r"/page/(\d+)").expect("compile RE_PAGE"))
}

pub fn parse_book_id(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Some(trimmed.to_string());
    }

    // If user pasted extra text around the URL, try to extract URL first.
    let target = re_url()
        .find(trimmed)
        .map(|m| m.as_str())
        .unwrap_or(trimmed);

    if let Some(caps) = re_qs().captures(target) {
        return caps.get(2).map(|m| m.as_str().to_string());
    }

    if let Some(caps) = re_page().captures(target) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }

    None
}
