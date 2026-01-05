#![allow(dead_code)]

use reqwest::blocking::Client;
use reqwest::header::{
    ACCEPT, ACCEPT_ENCODING, CONNECTION, CONTENT_TYPE, HeaderMap, HeaderValue, REFERER, USER_AGENT,
};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, error, warn};

#[derive(Debug, Clone, Default)]
pub(crate) struct BookInfo {
    pub book_name: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub chapter_count: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct FanqieWebConfig {
    pub request_timeout: Duration,
    pub max_retries: usize,
    pub insecure_tls: bool,
    pub user_agent: String,
    pub cache_dir: PathBuf,
}

impl Default for FanqieWebConfig {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(15),
            max_retries: 3,
            insecure_tls: false,
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120 Safari/537.36".to_string(),
            cache_dir: std::env::temp_dir().join("tomato-novel-downloader").join("dir_cache"),
        }
    }
}

pub(crate) struct FanqieWebNetwork {
    client: Client,
    config: FanqieWebConfig,
    last_dir_fetch: Mutex<Instant>,
}

pub(crate) type BookInfoParts = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Vec<String>>,
    Option<usize>,
);

impl FanqieWebNetwork {
    pub(crate) fn new(config: FanqieWebConfig) -> anyhow::Result<Self> {
        let mut default_headers = HeaderMap::new();
        default_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        default_headers.insert(CONNECTION, HeaderValue::from_static("keep-alive"));

        let client = Client::builder()
            .default_headers(default_headers)
            .danger_accept_invalid_certs(config.insecure_tls)
            .timeout(config.request_timeout)
            .build()?;

        Ok(Self {
            client,
            config,
            last_dir_fetch: Mutex::new(Instant::now() - Duration::from_secs(60)),
        })
    }

    fn get_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            ),
        );
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&self.config.user_agent)
                .unwrap_or(HeaderValue::from_static("Mozilla/5.0")),
        );
        headers
    }

    fn get_json_headers(&self, book_id: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/plain, */*"),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&self.config.user_agent)
                .unwrap_or(HeaderValue::from_static("Mozilla/5.0")),
        );
        let referer = format!("https://fanqienovel.com/page/{book_id}");
        if let Ok(v) = HeaderValue::from_str(&referer) {
            headers.insert(REFERER, v);
        }
        headers
    }

    pub(crate) fn get_book_info(&self, book_id: &str) -> BookInfoParts {
        let book_info_url = format!("https://fanqienovel.com/page/{book_id}");

        // 发送请求
        match self
            .client
            .get(&book_info_url)
            .headers(self.get_headers())
            .send()
        {
            Ok(resp) => {
                if resp.status().as_u16() == 404 {
                    error!("小说ID {} 不存在！", book_id);
                    return (None, None, None, None, None);
                }
                let resp = match resp.error_for_status() {
                    Ok(r) => r,
                    Err(e) => {
                        error!("获取书籍信息失败: {}", e);
                        return (None, None, None, None, None);
                    }
                };

                match resp.text() {
                    Ok(text) => {
                        let info = ContentParser::parse_book_info(&text, book_id);
                        (
                            info.book_name,
                            info.author,
                            info.description,
                            info.tags,
                            info.chapter_count,
                        )
                    }
                    Err(e) => {
                        error!("获取书籍信息失败: {}", e);
                        (None, None, None, None, None)
                    }
                }
            }
            Err(e) => {
                error!("获取书籍信息失败: {}", e);
                (None, None, None, None, None)
            }
        }
    }

    /// 从 web API 获取章节列表（节流 + 403 预热 + 退避重试 + 本地缓存回退）。
    pub(crate) fn fetch_chapter_list(&self, book_id: &str) -> Option<Vec<Value>> {
        // 无效 book_id 直接返回 None，避免无意义请求
        if book_id.trim().is_empty() || !book_id.chars().all(|c| c.is_ascii_digit()) {
            warn!("fetch_chapter_list 跳过无效 book_id: '{}'", book_id);
            return None;
        }

        let api_url =
            format!("https://fanqienovel.com/api/reader/directory/detail?bookId={book_id}");

        // 节流：与上次请求间隔至少 0.8s，降低被限频概率
        self.throttle_directory(Duration::from_millis(800));

        let retries = self.config.max_retries.max(1);
        let mut backoff = 0.6f64;
        let mut last_error: Option<String> = None;

        for attempt in 1..=retries {
            debug!("开始获取章节列表，URL: {}", api_url);
            let headers = self.get_json_headers(book_id);

            if attempt == 1 {
                // 屏蔽 Cookie（如果未来启用 cookies feature，这里也不会泄露）
                let masked: Vec<(String, String)> = headers
                    .iter()
                    .map(|(k, v)| {
                        let key = k.as_str().to_string();
                        let val = if key.eq_ignore_ascii_case("cookie") {
                            "***".to_string()
                        } else {
                            v.to_str().unwrap_or("").to_string()
                        };
                        (key, val)
                    })
                    .collect();
                debug!("目录请求Header(精简): {:?}", masked);
            } else {
                debug!(
                    "重试第 {} 次获取目录（可能被限频/风控），URL: {}",
                    attempt, api_url
                );
            }

            let resp = self.client.get(&api_url).headers(headers).send();

            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    last_error = Some(e.to_string());
                    error!("获取章节列表失败: {}", e);
                    self.sleep_backoff(attempt, retries, &mut backoff, 0.3);
                    continue;
                }
            };

            debug!("章节列表响应状态: {}", resp.status().as_u16());

            // 显式处理 403：可能为风控或限频
            if resp.status().as_u16() == 403 {
                last_error = Some("403 Forbidden".to_string());

                // 首次遇到 403 时，尝试预热页面以获取必要 Cookie，再退避重试
                if attempt == 1 {
                    let warm_url = format!("https://fanqienovel.com/page/{book_id}");
                    match self
                        .client
                        .get(&warm_url)
                        .headers(self.get_headers())
                        .send()
                    {
                        Ok(_) => {
                            debug!("已尝试通过页面预热获取 Cookie，准备退避后重试目录 API");
                        }
                        Err(e) => {
                            debug!("页面预热失败: {}", e);
                        }
                    }
                }

                self.sleep_backoff(attempt, retries, &mut backoff, 0.4);
                continue;
            }

            let resp = match resp.error_for_status() {
                Ok(r) => r,
                Err(e) => {
                    last_error = Some(e.to_string());
                    error!("获取章节列表失败: {}", e);
                    self.sleep_backoff(attempt, retries, &mut backoff, 0.3);
                    continue;
                }
            };

            let data: Value = match resp.json() {
                Ok(v) => v,
                Err(e) => {
                    last_error = Some(e.to_string());
                    error!("获取章节列表失败: {}", e);
                    self.sleep_backoff(attempt, retries, &mut backoff, 0.3);
                    continue;
                }
            };

            // 成功则缓存原始 JSON，便于下次回退
            if let Err(e) = self.save_dir_cache(book_id, &data) {
                debug!("保存目录缓存失败(忽略): {}", e);
            }

            return Self::parse_chapter_data(&data);
        }

        debug!("重试仍失败：{:?}", last_error);

        // 重试仍失败：尝试使用本地缓存回退
        match self.load_dir_cache(book_id) {
            Ok(Some(cached)) => {
                debug!("使用本地缓存的章节目录回退: book_id={}", book_id);
                Self::parse_chapter_data(&cached)
            }
            _ => None,
        }
    }

    fn throttle_directory(&self, min_gap: Duration) {
        if let Ok(mut last) = self.last_dir_fetch.lock() {
            let elapsed = last.elapsed();
            if elapsed < min_gap {
                std::thread::sleep(min_gap - elapsed);
            }
            *last = Instant::now();
        }
    }

    fn sleep_backoff(&self, attempt: usize, retries: usize, backoff: &mut f64, jitter_max: f64) {
        if attempt >= retries {
            return;
        }
        let jitter = jitter_seconds(jitter_max);
        let sleep_s = (*backoff + jitter).min(3.0);
        std::thread::sleep(Duration::from_millis((sleep_s * 1000.0) as u64));
        *backoff = (*backoff * 2.0).min(3.0);
    }

    fn cache_path(&self, book_id: &str) -> PathBuf {
        self.config.cache_dir.join(format!("{book_id}.json"))
    }

    fn save_dir_cache(&self, book_id: &str, data: &Value) -> anyhow::Result<()> {
        let path = self.cache_path(book_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec(data)?;
        fs::write(path, bytes)?;
        Ok(())
    }

    fn load_dir_cache(&self, book_id: &str) -> anyhow::Result<Option<Value>> {
        let path = self.cache_path(book_id);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(path)?;
        let value: Value = serde_json::from_slice(&bytes)?;
        Ok(Some(value))
    }

    fn parse_chapter_data(data: &Value) -> Option<Vec<Value>> {
        // 兼容多种返回形态：尽量提取出“章节数组”。
        let root = data.get("data").unwrap_or(data);

        for key in [
            "chapterList",
            "chapter_list",
            "chapters",
            "item_list",
            "items",
            "list",
        ] {
            if let Some(arr) = root.get(key).and_then(Value::as_array) {
                return Some(arr.clone());
            }
        }

        // 有些接口会是 data.data.list
        if let Some(arr) = root
            .get("data")
            .and_then(|v| v.get("list"))
            .and_then(Value::as_array)
        {
            return Some(arr.clone());
        }

        None
    }
}

struct ContentParser;

impl ContentParser {
    /// 从 HTML 中尽量解析出书籍信息。
    ///
    /// 说明：番茄页面结构可能变化，这里采用“优先解析 __NEXT_DATA__ JSON，其次正则兜底”的策略。
    fn parse_book_info(html: &str, _book_id: &str) -> BookInfo {
        // 1) 优先解析 __NEXT_DATA__
        if let Some(json_text) = extract_next_data_json(html)
            && let Ok(value) = serde_json::from_str::<Value>(&json_text)
        {
            let book_name = find_string_by_key(&value, ["bookName", "book_name", "title", "name"]);
            let author = find_string_by_key(&value, ["author", "authorName", "author_name"]);
            let description =
                find_string_by_key(&value, ["abstract", "description", "intro", "introduce"]);
            let chapter_count = find_usize_by_key(&value, ["chapterCount", "chapter_count"]);
            let tags = find_string_array_by_key(&value, ["tags", "tagNames", "tag_names"]);

            if book_name.is_some()
                || author.is_some()
                || description.is_some()
                || chapter_count.is_some()
                || tags.is_some()
            {
                return BookInfo {
                    book_name,
                    author,
                    description,
                    tags,
                    chapter_count,
                };
            }
        }

        // 2) 正则兜底（在 HTML 内直接找 JSON 字段）
        let book_name = regex_json_string_field(html, "bookName")
            .or_else(|| regex_json_string_field(html, "book_name"));
        let author = regex_json_string_field(html, "author")
            .or_else(|| regex_json_string_field(html, "authorName"));
        let description = regex_json_string_field(html, "abstract")
            .or_else(|| regex_json_string_field(html, "description"));
        let chapter_count = regex_json_usize_field(html, "chapterCount")
            .or_else(|| regex_json_usize_field(html, "chapter_count"));

        BookInfo {
            book_name,
            author,
            description,
            tags: None,
            chapter_count,
        }
    }
}

fn extract_next_data_json(html: &str) -> Option<String> {
    // (?s) 让 . 匹配换行
    let re =
        regex::Regex::new(r#"(?s)<script[^>]*id=\"__NEXT_DATA__\"[^>]*>(.*?)</script>"#).ok()?;
    let caps = re.captures(html)?;
    let raw = caps.get(1)?.as_str();
    Some(raw.trim().to_string())
}

fn find_string_by_key<const N: usize>(value: &Value, keys: [&str; N]) -> Option<String> {
    for key in keys {
        if let Some(s) = find_first_string_for_key(value, key) {
            return Some(s);
        }
    }
    None
}

fn find_usize_by_key<const N: usize>(value: &Value, keys: [&str; N]) -> Option<usize> {
    for key in keys {
        if let Some(n) = find_first_usize_for_key(value, key) {
            return Some(n);
        }
    }
    None
}

fn find_string_array_by_key<const N: usize>(value: &Value, keys: [&str; N]) -> Option<Vec<String>> {
    for key in keys {
        if let Some(arr) = find_first_string_array_for_key(value, key) {
            return Some(arr);
        }
    }
    None
}

fn find_first_string_for_key(value: &Value, target: &str) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(v) = map.get(target)
                && let Some(s) = v.as_str()
            {
                return Some(s.to_string());
            }
            for v in map.values() {
                if let Some(found) = find_first_string_for_key(v, target) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => arr
            .iter()
            .find_map(|v| find_first_string_for_key(v, target)),
        _ => None,
    }
}

fn find_first_usize_for_key(value: &Value, target: &str) -> Option<usize> {
    match value {
        Value::Object(map) => {
            if let Some(v) = map.get(target) {
                if let Some(n) = v.as_u64() {
                    return Some(n as usize);
                }
                if let Some(s) = v.as_str()
                    && let Ok(n) = s.parse::<usize>()
                {
                    return Some(n);
                }
            }
            for v in map.values() {
                if let Some(found) = find_first_usize_for_key(v, target) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => arr.iter().find_map(|v| find_first_usize_for_key(v, target)),
        _ => None,
    }
}

fn find_first_string_array_for_key(value: &Value, target: &str) -> Option<Vec<String>> {
    match value {
        Value::Object(map) => {
            if let Some(v) = map.get(target)
                && let Some(arr) = v.as_array()
            {
                let out: Vec<String> = arr
                    .iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect();
                if !out.is_empty() {
                    return Some(out);
                }
            }
            for v in map.values() {
                if let Some(found) = find_first_string_array_for_key(v, target) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => arr
            .iter()
            .find_map(|v| find_first_string_array_for_key(v, target)),
        _ => None,
    }
}

fn regex_json_string_field(html: &str, field: &str) -> Option<String> {
    let pattern = format!(r#"\"{}\"\s*:\s*\"(.*?)\""#, regex::escape(field));
    let re = regex::Regex::new(&pattern).ok()?;
    let caps = re.captures(html)?;
    let raw = caps.get(1)?.as_str();

    // 尝试按 JSON 字符串规则反转义
    let quoted = format!("\"{}\"", raw);
    serde_json::from_str::<String>(&quoted)
        .ok()
        .or_else(|| Some(raw.to_string()))
}

fn regex_json_usize_field(html: &str, field: &str) -> Option<usize> {
    let pattern = format!(r#"\"{}\"\s*:\s*(\d+)"#, regex::escape(field));
    let re = regex::Regex::new(&pattern).ok()?;
    let caps = re.captures(html)?;
    caps.get(1)?.as_str().parse::<usize>().ok()
}

fn jitter_seconds(max: f64) -> f64 {
    if max <= 0.0 {
        return 0.0;
    }
    // 用时间戳制造一个轻量抖动（避免引入 rand 依赖）
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let bucket = (nanos % 10_000) as f64 / 10_000.0; // [0,1)
    bucket * max
}

fn _ensure_parent_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}
