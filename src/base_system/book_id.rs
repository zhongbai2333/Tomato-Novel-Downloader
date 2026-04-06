//! 书籍 ID/链接解析与规范化。

use regex::Regex;
use std::sync::OnceLock;

static RE_URL: OnceLock<Regex> = OnceLock::new();
static RE_QS: OnceLock<Regex> = OnceLock::new();
static RE_PAGE: OnceLock<Regex> = OnceLock::new();
static RE_SHORT_LINK: OnceLock<Regex> = OnceLock::new();
static HTTP_CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();

/// Known domains that issue short-link share URLs of the form `/t/<token>`.
/// Only these hosts are followed during redirect resolution to prevent SSRF.
const ALLOWED_SHORT_LINK_HOSTS: &[&str] = &[
    "changdunovel.com",
    "www.changdunovel.com",
    "fanqienovel.com",
    "www.fanqienovel.com",
    "fqnovel.com",
    "www.fqnovel.com",
];

fn re_url() -> &'static Regex {
    RE_URL.get_or_init(|| Regex::new(r"https?://\S+").expect("compile RE_URL"))
}

fn re_qs() -> &'static Regex {
    RE_QS.get_or_init(|| Regex::new(r"(?i)(book_id|bookId)=([0-9]+)").expect("compile RE_QS"))
}

fn re_page() -> &'static Regex {
    RE_PAGE.get_or_init(|| Regex::new(r"/page/(\d+)").expect("compile RE_PAGE"))
}

fn re_short_link() -> &'static Regex {
    RE_SHORT_LINK.get_or_init(|| {
        Regex::new(r"(?i)https?://[^/\s]+/t/[A-Za-z0-9]+/?").expect("compile RE_SHORT_LINK")
    })
}

fn http_client() -> &'static reqwest::blocking::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("build HTTP client for short-link resolution")
    })
}

/// Extracts the host (without port) from a URL string, lowercased.
fn url_host(url: &str) -> Option<String> {
    let after_scheme = url
        .trim()
        .strip_prefix("https://")
        .or_else(|| url.trim().strip_prefix("http://"))?;
    let host_and_rest = after_scheme.split('/').next()?;
    // Strip port if present
    let host = host_and_rest.split(':').next()?;
    Some(host.to_lowercase())
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

/// Returns `true` if `input` contains a short-redirect share link from a
/// known allowed domain (e.g. `https://changdunovel.com/t/550lVQoKokk/`).
pub fn is_short_link(input: &str) -> bool {
    let trimmed = input.trim();
    let target = re_url()
        .find(trimmed)
        .map(|m| m.as_str())
        .unwrap_or(trimmed);
    if !re_short_link().is_match(target) {
        return false;
    }
    url_host(target)
        .map(|h| ALLOWED_SHORT_LINK_HOSTS.contains(&h.as_str()))
        .unwrap_or(false)
}

/// Like [`parse_book_id`], but also handles short-redirect share links by
/// following the HTTP redirect and parsing the resolved URL.
///
/// Only short links from [`ALLOWED_SHORT_LINK_HOSTS`] are followed to
/// prevent SSRF.  This function performs a blocking network request when
/// `input` is a short link.  Call it from a blocking context (e.g. inside
/// `tokio::task::spawn_blocking`) when used from async code.
pub fn resolve_book_id(input: &str) -> Option<String> {
    if let Some(id) = parse_book_id(input) {
        return Some(id);
    }

    let trimmed = input.trim();
    let url = re_url().find(trimmed).map(|m| m.as_str())?;

    if !is_short_link(url) {
        return None;
    }

    let response = match http_client().get(url).send() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(url = %url, error = %e, "短链接跳转失败");
            return None;
        }
    };
    let final_url = response.url().to_string();

    let book_id = parse_book_id(&final_url);
    if book_id.is_none() {
        tracing::warn!(url = %url, final_url = %final_url, "短链接跳转后仍无法解析 book_id");
    }
    book_id
}
