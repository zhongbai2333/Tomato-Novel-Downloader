//! 冷却/退避重试策略。

use anyhow::{Result, anyhow};
use serde_json::Value;
use std::time::Duration;
use tomato_novel_official_api::FanqieClient;

pub fn fetch_with_cooldown_retry(
    client: &FanqieClient,
    ids: &str,
    epub_mode: bool,
) -> Result<Value> {
    let mut delay = Duration::from_millis(1100);
    for attempt in 0..6 {
        match client.get_contents(ids, epub_mode) {
            Ok(v) => return Ok(v),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("Cooldown") || msg.contains("CooldownNotReached") {
                    std::thread::sleep(delay);
                    delay = std::cmp::min(delay * 2, Duration::from_secs(8));
                    continue;
                }
                if attempt == 0
                    && (msg.contains("tomato_novel_network_core") || msg.contains("Library"))
                {
                    return Err(anyhow!(
                        "{}\n\n提示：请先构建 Tomato-Novel-Network-Core，并将动态库放到当前目录或设置 FANQIE_NETWORK_CORE_DLL 指向其绝对路径。",
                        msg
                    ));
                }
                return Err(anyhow!(msg));
            }
        }
    }
    Err(anyhow!("Cooldown exceeded retries"))
}
