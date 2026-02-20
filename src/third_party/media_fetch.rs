use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, CONNECTION, HeaderMap, HeaderValue};
use std::sync::OnceLock;
use std::time::Duration;

/// 复用 HTTP Client，避免每次调用都重建导致连接池和 TLS 握手浪费。
fn shared_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        headers.insert(CONNECTION, HeaderValue::from_static("keep-alive"));
        Client::builder()
            .default_headers(headers)
            .build()
            .expect("failed to build shared HTTP client")
    })
}

pub(crate) fn fetch_bytes(url: &str, timeout: Duration) -> Option<Vec<u8>> {
    if url.trim().is_empty() {
        return None;
    }

    let client = shared_client();
    let resp = client.get(url).timeout(timeout).send().ok()?;
    let resp = resp.error_for_status().ok()?;
    let bytes = resp.bytes().ok()?;
    Some(bytes.to_vec())
}
