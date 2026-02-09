//! 第三方 API 地址解析、请求、重试逻辑。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow};

use super::models::ChapterRef;
use crate::base_system::context::Config;
use crate::book_parser::parser::ContentParser;
use crate::third_party::content_client::ThirdPartyContentClient;

fn normalize_base(base: &str) -> String {
    base.trim().trim_end_matches('/').to_string()
}

fn ensure_trailing_query_base(url: &str) -> String {
    let u = url.trim();
    if u.ends_with('?') || u.ends_with('&') {
        return u.to_string();
    }
    if u.contains('?') {
        return format!("{}&", u);
    }
    format!("{}?", u)
}

#[allow(clippy::type_complexity)]
pub(crate) fn resolve_api_urls(
    cfg: &Config,
) -> Result<(Option<String>, Option<(String, String)>), anyhow::Error> {
    if cfg.use_official_api {
        return Ok((None, None));
    }

    let base = cfg
        .api_endpoints
        .first()
        .map(|s| s.as_str())
        .map(normalize_base)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("use_official_api=false 时，api_endpoints 不能为空"))?;

    // 目录接口（网页端）
    let directory_url = if base.contains("/api/") && base.contains("directory") {
        base.clone()
    } else {
        format!("{}/api/reader/directory/detail", base)
    };

    // 正文 batch_full + registerkey（reading 域名反代）
    let register_key_url = if base.contains("registerkey") {
        base.clone()
    } else {
        format!("{}/reading/crypt/registerkey", base)
    };
    let batch_full_url = if base.contains("batch_full") {
        ensure_trailing_query_base(&base)
    } else {
        ensure_trailing_query_base(&format!("{}/reading/reader/batch_full/v", base))
    };

    Ok((
        Some(directory_url),
        Some((register_key_url, batch_full_url)),
    ))
}

pub(crate) fn ms_from_connect_timeout_secs(v: f64) -> Option<u64> {
    if v <= 0.0 {
        return None;
    }
    let ms = (v * 1000.0).round() as i64;
    if ms <= 0 { None } else { Some(ms as u64) }
}

pub(crate) fn third_party_client_for_endpoint(
    cfg: &Config,
    endpoint: &str,
) -> Result<ThirdPartyContentClient> {
    let timeout_ms = Some(cfg.request_timeout.saturating_mul(1000).max(100));
    let connect_timeout_ms = ms_from_connect_timeout_secs(cfg.min_connect_timeout);
    ThirdPartyContentClient::new(endpoint, timeout_ms, connect_timeout_ms)
}

pub(crate) fn has_any_content_for_group(
    value: &serde_json::Value,
    group: &[ChapterRef],
    cfg: &Config,
) -> bool {
    let parsed = ContentParser::extract_api_content(value, cfg);
    group.iter().any(|ch| {
        parsed
            .get(&ch.id)
            .map(|(content, _)| !content.trim().is_empty())
            .unwrap_or(false)
    })
}

pub(crate) fn validate_endpoints(cfg: &Config, probe_chapter_id: &str) -> Vec<String> {
    let mut ok = Vec::new();
    for ep in &cfg.api_endpoints {
        let ep = ep.trim();
        if ep.is_empty() {
            continue;
        }
        let client = match third_party_client_for_endpoint(cfg, ep) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let value = match client.get_contents_unthrottled(probe_chapter_id, false) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // probe 请求只含 1 个 chapter_id，用 group 校验最简单
        let probe_group = [ChapterRef {
            id: probe_chapter_id.to_string(),
            title: String::new(),
        }];
        if has_any_content_for_group(&value, &probe_group, cfg) {
            ok.push(ep.to_string());
        }
    }
    ok
}

pub(crate) fn sleep_backoff(cfg: &Config, attempt: u32) {
    let min_ms = cfg.min_wait_time.max(1);
    let max_ms = cfg.max_wait_time.max(min_ms);
    let shift = attempt.min(10);
    let factor = 1u64.checked_shl(shift).unwrap_or(u64::MAX);
    let mut wait = min_ms.saturating_mul(factor);
    if wait > max_ms {
        wait = max_ms;
    }
    std::thread::sleep(Duration::from_millis(wait));
}

pub(crate) fn fetch_group_third_party(
    cfg: &Config,
    endpoints: &Arc<std::sync::Mutex<Vec<String>>>,
    pick: &Arc<AtomicUsize>,
    group: &[ChapterRef],
    epub_mode: bool,
) -> Result<serde_json::Value> {
    let tries = cfg.max_retries.max(1);
    let ids = group
        .iter()
        .map(|c| c.id.as_str())
        .collect::<Vec<_>>()
        .join(",");

    for attempt in 0..tries {
        let ep = {
            let guard = endpoints.lock().unwrap();
            if guard.is_empty() {
                return Err(anyhow!("第三方 API 地址池已为空（全部判定无效）"));
            }
            let idx = pick.fetch_add(1, Ordering::Relaxed) % guard.len();
            guard[idx].clone()
        };

        let client = third_party_client_for_endpoint(cfg, &ep)?;
        match client.get_contents_unthrottled(&ids, epub_mode) {
            Ok(v) => {
                if !has_any_content_for_group(&v, group, cfg) {
                    let mut guard = endpoints.lock().unwrap();
                    guard.retain(|x| x != &ep);
                    drop(guard);
                    sleep_backoff(cfg, attempt);
                    continue;
                }
                return Ok(v);
            }
            Err(_) => {
                sleep_backoff(cfg, attempt);
                continue;
            }
        }
    }

    Err(anyhow!("第三方 API 请求重试耗尽"))
}
