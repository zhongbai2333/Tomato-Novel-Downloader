//!
//! Edge TTS（Read Aloud）客户端实现。
//!
//! 目标：在 musl / Android 等环境也可用（避免 native-tls / curl / OpenSSL 等原生依赖）。

use std::net::TcpStream;

use anyhow::{Context, Result, anyhow};
use sha2::{Digest, Sha256};
use tungstenite::Message;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::header;
use tungstenite::stream::MaybeTlsStream;

// 关键常量来自公开的 Edge ReadAloud WebSocket 接口。
// 这些值与 msedge-tts 的实现保持一致，以提升兼容性。
//
// 注：近期微软侧策略变更可能对 UA/Origin 更敏感；这里使用较新的 EdgA UA。
const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 10; HD1913) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.7499.193 Mobile Safari/537.36 EdgA/143.0.3650.125";
const ORIGIN: &str = "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold";
const TRUSTED_CLIENT_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
const WSS_URL_PREFIX: &str = "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1?TrustedClientToken=6A5AA1D4EAFF4E9FB37E23D68491D6F4&ConnectionId=";

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct SpeechConfig {
    pub voice_name: String,
    pub audio_format: String,
    pub pitch: i32,
    pub rate: i32,
    pub volume: i32,
}

#[derive(Debug, Clone)]
pub struct SynthesizedAudio {
    #[allow(dead_code)]
    pub audio_format: String,
    pub audio_bytes: Vec<u8>,
}

pub struct EdgeTtsClient {
    websocket: tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
}

impl EdgeTtsClient {
    pub fn connect() -> Result<Self> {
        let request = build_websocket_request()?;
        let (websocket, _) = tungstenite::connect(request).context("connect edge tts websocket")?;
        Ok(Self { websocket })
    }

    pub fn synthesize(&mut self, text: &str, config: &SpeechConfig) -> Result<SynthesizedAudio> {
        let config_message = build_config_message(config);
        let ssml_message = build_ssml_message(text, config);

        self.websocket
            .send(config_message)
            .context("send speech.config")?;
        self.websocket.send(ssml_message).context("send ssml")?;

        let mut audio_bytes = Vec::new();
        let mut turn_start = false;
        let mut response = false;
        let mut turn_end = false;

        loop {
            if turn_end {
                break;
            }

            let message = self.websocket.read().context("read websocket")?;
            if let Some(payload) =
                process_message(message, &mut turn_start, &mut response, &mut turn_end)?
            {
                audio_bytes.extend_from_slice(&payload);
            }
        }

        Ok(SynthesizedAudio {
            audio_format: config.audio_format.clone(),
            audio_bytes,
        })
    }
}

// try to fix china mainland 403 forbidden issue
// solution from:
// https://github.com/rany2/edge-tts/issues/290#issuecomment-2464956570
fn gen_sec_ms_gec() -> String {
    // UTC time from 1601-01-01
    // reference: msedge-tts implementation
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        + std::time::Duration::from_secs(11644473600);

    // 100ns ticks
    let ticks = duration.as_nanos() / 100;
    // align
    let ticks = ticks - ticks % 3_000_000_000;

    let mut hasher = Sha256::new();
    hasher.update(format!("{ticks}{TRUSTED_CLIENT_TOKEN}"));
    let hash_code = hasher.finalize();

    let mut hex_str = String::with_capacity(hash_code.len() * 2);
    for &byte in hash_code.iter() {
        hex_str.push_str(&format!("{:02X}", byte));
    }
    hex_str
}

fn build_websocket_request() -> Result<tungstenite::handshake::client::Request> {
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    let sec_ms_gec = gen_sec_ms_gec();
    let sec_ms_gec_version = "1-130.0.2849.68";

    let mut request = format!(
        "{}{}&Sec-MS-GEC={}&Sec-MS-GEC-Version={}",
        WSS_URL_PREFIX, uuid, sec_ms_gec, sec_ms_gec_version
    )
    .into_client_request()
    .map_err(|e| anyhow!("build websocket request: {e}"))?;

    let headers = request.headers_mut();
    headers.insert(header::PRAGMA, "no-cache".parse().unwrap());
    headers.insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
    headers.insert(header::USER_AGENT, USER_AGENT.parse().unwrap());
    headers.insert(header::ORIGIN, ORIGIN.parse().unwrap());

    Ok(request)
}

fn build_config_message(config: &SpeechConfig) -> Message {
    static SPEECH_CONFIG_HEAD: &str = r#"{"context":{"synthesis":{"audio":{"metadataoptions":{"sentenceBoundaryEnabled":"false","wordBoundaryEnabled":"true"},"outputFormat":""#;
    static SPEECH_CONFIG_TAIL: &str = r#""}}}}"#;

    let ts = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc2822)
        .unwrap_or_else(|_| "".to_string());

    let msg = format!(
        "X-Timestamp:{}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n{}{}{}",
        ts, SPEECH_CONFIG_HEAD, config.audio_format, SPEECH_CONFIG_TAIL
    );

    Message::Text(msg)
}

fn build_ssml_message(text: &str, config: &SpeechConfig) -> Message {
    let ssml = format!(
        "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'><voice name='{}'><prosody pitch='{:+}Hz' rate='{:+}%' volume='{:+}%'>{}</prosody></voice></speak>",
        config.voice_name,
        config.pitch,
        config.rate,
        config.volume,
        xml_escape(text),
    );

    let request_id = uuid::Uuid::new_v4().simple().to_string();
    let ts = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc2822)
        .unwrap_or_else(|_| "".to_string());

    let msg = format!(
        "X-RequestId:{}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{}\r\nPath:ssml\r\n\r\n{}",
        request_id, ts, ssml
    );

    Message::Text(msg)
}

fn process_message(
    message: Message,
    turn_start: &mut bool,
    response: &mut bool,
    turn_end: &mut bool,
) -> Result<Option<Vec<u8>>> {
    match message {
        Message::Text(text) => {
            if text.contains("audio.metadata") {
                Ok(None)
            } else if text.contains("turn.start") {
                *turn_start = true;
                Ok(None)
            } else if text.contains("response") {
                *response = true;
                Ok(None)
            } else if text.contains("turn.end") {
                *turn_end = true;
                Ok(None)
            } else {
                Err(anyhow!("unexpected text message: {text}"))
            }
        }
        Message::Binary(bytes) => {
            if *turn_start || *response {
                if bytes.len() < 2 {
                    return Ok(None);
                }
                let header_len = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
                let start = header_len + 2;
                if start > bytes.len() {
                    return Ok(None);
                }
                Ok(Some(bytes[start..].to_vec()))
            } else {
                Ok(None)
            }
        }
        Message::Close(_) => {
            *turn_end = true;
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}
