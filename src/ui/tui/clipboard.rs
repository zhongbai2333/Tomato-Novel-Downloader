//! 剪贴板工具（TUI 用）。
//!
//! - Desktop：通过 `clipboard-arboard` 使用 arboard。
//! - Android：优先使用 Termux `termux-clipboard-get`。

use anyhow::Result;

#[cfg(any(
    all(feature = "clipboard", target_os = "android"),
    all(
        feature = "clipboard",
        feature = "clipboard-arboard",
        not(target_os = "android")
    )
))]
use anyhow::Context;

#[cfg(all(feature = "clipboard", target_os = "android"))]
pub(super) fn get_text() -> Result<Option<String>> {
    use std::process::Command;

    let output = match Command::new("termux-clipboard-get").output() {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(e) => return Err(e).context("run termux-clipboard-get"),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("termux-clipboard-get failed: {}", stderr.trim());
    }

    let s = String::from_utf8_lossy(&output.stdout);
    let text = s.trim_end_matches(['\r', '\n']).to_string();
    if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}

#[cfg(all(
    feature = "clipboard",
    feature = "clipboard-arboard",
    not(target_os = "android")
))]
pub(super) fn get_text() -> Result<Option<String>> {
    let mut clip = arboard::Clipboard::new().context("init clipboard")?;
    let text = clip.get_text().context("get clipboard text")?;
    if text.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}

#[cfg(all(
    feature = "clipboard",
    not(target_os = "android"),
    not(feature = "clipboard-arboard")
))]
pub(super) fn get_text() -> Result<Option<String>> {
    // 构建启用了 `clipboard`，但没有可用后端（例如未启用 `clipboard-arboard`）。
    Ok(None)
}

#[cfg(not(feature = "clipboard"))]
pub(super) fn get_text() -> Result<Option<String>> {
    Ok(None)
}
