use anyhow::{Result, anyhow};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, CONNECTION, HeaderMap, HeaderValue, USER_AGENT};
use serde_json::Value;
use std::time::Duration;

const AID: &str = "1967";

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

fn derive_batch_full_base(endpoint: &str) -> String {
    let base = normalize_base(endpoint);

    // allow passing full urls too
    if base.contains("/reading/reader/batch_full") {
        return ensure_trailing_query_base(&base);
    }

    ensure_trailing_query_base(&format!("{}/reading/reader/batch_full/v", base))
}

fn join_params(params: &[(String, String)]) -> String {
    // IMPORTANT: Keep commas unescaped (item_ids is comma-separated).
    let mut out = String::new();
    for (i, (k, v)) in params.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        out.push_str(k);
        out.push('=');
        out.push_str(v);
    }
    out
}

/// 轻量第三方正文客户端：不依赖 Official-API。
///
/// 约定：第三方服务应返回可直接解析的 JSON（尽量与 Official-API 解密后的结构兼容），
/// 即 top-level 或 data 字段为 object，key 为 chapter_id/item_id，value 包含 content/title。
#[derive(Clone)]
pub(crate) struct ThirdPartyContentClient {
    client: Client,
    batch_full_base: String,
}

impl ThirdPartyContentClient {
    pub(crate) fn new(
        endpoint: &str,
        timeout_ms: Option<u64>,
        connect_timeout_ms: Option<u64>,
    ) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        headers.insert(CONNECTION, HeaderValue::from_static("keep-alive"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/plain, */*"),
        );
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120 Safari/537.36",
            ),
        );

        let mut builder = Client::builder().default_headers(headers);
        if let Some(ms) = timeout_ms {
            builder = builder.timeout(Duration::from_millis(ms.max(50)));
        }
        if let Some(ms) = connect_timeout_ms {
            builder = builder.connect_timeout(Duration::from_millis(ms.max(50)));
        }

        let client = builder.build()?;
        Ok(Self {
            client,
            batch_full_base: derive_batch_full_base(endpoint),
        })
    }

    pub(crate) fn get_contents_unthrottled(&self, item_ids: &str, epub: bool) -> Result<Value> {
        let item_ids = item_ids.trim();
        if item_ids.is_empty() {
            return Err(anyhow!("item_ids 不能为空"));
        }

        // Best-effort compatibility with Official API style params.
        // Many third-party services ignore extra params.
        let update_version_code = "0";
        let mut params: Vec<(String, String)> = vec![
            ("item_ids".to_string(), item_ids.to_string()),
            (
                "update_version_code".to_string(),
                update_version_code.to_string(),
            ),
            ("aid".to_string(), AID.to_string()),
            ("key_register_ts".to_string(), "0".to_string()),
            ("device_platform".to_string(), "android".to_string()),
            ("iid".to_string(), "0".to_string()),
        ];
        if epub {
            params.push(("version_code".to_string(), update_version_code.to_string()));
            params.push(("epub".to_string(), "1".to_string()));
        } else {
            params.push(("epub".to_string(), "0".to_string()));
        }

        let query = join_params(&params);
        let url = format!("{}{}", self.batch_full_base, query);

        let resp = self.client.get(&url).send()?;
        let resp = resp.error_for_status()?;
        let v: Value = resp.json()?;
        Ok(v)
    }
}
