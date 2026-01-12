use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, CONNECTION, HeaderMap, HeaderValue};
use std::time::Duration;

pub(crate) fn fetch_bytes(url: &str, timeout: Duration) -> Option<Vec<u8>> {
    if url.trim().is_empty() {
        return None;
    }

    // reqwest in this project is built without default features (no gzip decoder).
    // Request identity encoding so the returned bytes are directly usable.
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    headers.insert(CONNECTION, HeaderValue::from_static("keep-alive"));

    let client = Client::builder()
        .default_headers(headers)
        .timeout(timeout)
        .build()
        .ok()?;

    let resp = client.get(url).send().ok()?;
    let resp = resp.error_for_status().ok()?;
    let bytes = resp.bytes().ok()?;
    Some(bytes.to_vec())
}
