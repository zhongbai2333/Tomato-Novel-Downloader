//! 程序版本更新提示（从 GitHub Releases API 拉取最新版本与更新日志）。
//!
//! 该模块用于：
//! - Web UI：提供 /api/app_update
//! - TUI / noui：启动时检查更新 + 手动“检查更新”入口
//! - 支持“不再提醒”某个 release tag（本地持久化）

use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::header::{ACCEPT, USER_AGENT};
use serde::{Deserialize, Serialize};
use tracing::warn;

const OWNER: &str = "zhongbai2333";
const REPO: &str = "Tomato-Novel-Downloader";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LatestRelease {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub html_url: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UpdateCheckReport {
    pub current_tag: String,
    pub latest: LatestRelease,
    pub is_new_version: bool,
    pub is_dismissed: bool,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
struct LocalUpdateState {
    dismissed_release_tag: Option<String>,
}

fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "unknown panic payload".to_string()
}

fn catch_update_panic<F>(op_name: &str, f: F) -> Result<UpdateCheckReport>
where
    F: FnOnce() -> Result<UpdateCheckReport>,
{
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            let detail = panic_payload_to_string(payload);
            warn!(
                target: "app_update",
                "捕获到上游 panic（{op_name}）：{detail}；已阻止进程崩溃"
            );
            Err(anyhow!(
                "app-update panic in {op_name}: {detail}（已拦截，程序继续运行）"
            ))
        }
    }
}

fn state_file_path() -> PathBuf {
    // 与 config.yml 同目录（默认工作目录）即可。
    PathBuf::from(".tnd_state.json")
}

fn load_state() -> LocalUpdateState {
    let path = state_file_path();
    let Ok(raw) = fs::read_to_string(&path) else {
        return LocalUpdateState::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn save_state(state: &LocalUpdateState) -> Result<()> {
    let path = state_file_path();
    let raw = serde_json::to_string_pretty(state).context("serialize update state")?;
    fs::write(&path, raw).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn dismiss_release_tag(tag: &str) -> Result<()> {
    let mut state = load_state();
    state.dismissed_release_tag = Some(tag.to_string());
    save_state(&state)
}

pub fn dismissed_release_tag() -> Option<String> {
    load_state().dismissed_release_tag
}

fn github_latest_release_url() -> String {
    format!("https://api.github.com/repos/{OWNER}/{REPO}/releases/latest")
}

#[derive(Debug, Deserialize)]
struct ReleaseInfo {
    name: Option<String>,
    tag_name: Option<String>,
    body: Option<String>,
    html_url: Option<String>,
    published_at: Option<String>,
}

fn normalize_tag(tag: &str) -> String {
    // 统一成 vX.Y.Z 形式，便于比较/展示。
    let t = tag.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.starts_with('v') || t.starts_with('V') {
        format!("v{}", &t[1..])
    } else {
        format!("v{t}")
    }
}

fn build_blocking_client(timeout: Duration) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .context("init http client")
}

fn fetch_latest_release_blocking(client: &reqwest::blocking::Client) -> Result<ReleaseInfo> {
    let resp = client
        .get(github_latest_release_url())
        .header(ACCEPT, "application/vnd.github+json")
        .header(USER_AGENT, "Tomato-Novel-Downloader/1.0")
        .send()
        .context("request latest release")?
        .error_for_status()
        .context("latest release status")?;

    resp.json::<ReleaseInfo>()
        .context("parse latest release json")
}

pub fn check_update_report_blocking(current_version: &str) -> Result<UpdateCheckReport> {
    catch_update_panic("check_update_report_blocking", || {
        check_update_report_blocking_with_timeout_impl(current_version, Duration::from_secs(12))
    })
}

pub fn check_update_report_blocking_with_timeout(
    current_version: &str,
    timeout: Duration,
) -> Result<UpdateCheckReport> {
    catch_update_panic("check_update_report_blocking_with_timeout", || {
        check_update_report_blocking_with_timeout_impl(current_version, timeout)
    })
}

fn check_update_report_blocking_with_timeout_impl(
    current_version: &str,
    timeout: Duration,
) -> Result<UpdateCheckReport> {
    if cfg!(feature = "docker") {
        return Ok(docker_update_report(current_version));
    }

    let current_tag = normalize_tag(current_version);

    let client = build_blocking_client(timeout)?;
    let info = fetch_latest_release_blocking(&client)?;
    let tag_name = info.tag_name.unwrap_or_default();
    if tag_name.trim().is_empty() {
        return Err(anyhow!("latest release missing tag_name"));
    }
    let latest_tag = normalize_tag(&tag_name);

    let state = load_state();
    let is_dismissed = state
        .dismissed_release_tag
        .as_deref()
        .is_some_and(|t| normalize_tag(t) == latest_tag);

    let latest = LatestRelease {
        tag_name: latest_tag.clone(),
        name: info.name,
        body: info.body,
        html_url: info.html_url,
        published_at: info.published_at,
    };

    Ok(UpdateCheckReport {
        current_tag: current_tag.clone(),
        is_new_version: latest_tag != current_tag,
        is_dismissed,
        latest,
    })
}

fn docker_update_report(current_version: &str) -> UpdateCheckReport {
    let current_tag = normalize_tag(current_version);
    UpdateCheckReport {
        current_tag: current_tag.clone(),
        latest: LatestRelease {
            tag_name: current_tag.clone(),
            name: Some("Docker build".to_string()),
            body: Some("Docker 构建已禁用程序自更新，请通过重新拉取镜像进行升级。".to_string()),
            html_url: None,
            published_at: None,
        },
        is_new_version: false,
        is_dismissed: false,
    }
}

pub async fn fetch_latest_release_async() -> Result<LatestRelease> {
    if cfg!(feature = "docker") {
        return Ok(LatestRelease {
            tag_name: normalize_tag(env!("CARGO_PKG_VERSION")),
            name: Some("Docker build".to_string()),
            body: Some("Docker 构建已禁用程序自更新，请通过重新拉取镜像进行升级。".to_string()),
            html_url: None,
            published_at: None,
        });
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .context("init http client")?;

    let resp = client
        .get(github_latest_release_url())
        .header(ACCEPT, "application/vnd.github+json")
        .header(USER_AGENT, "Tomato-Novel-Downloader/1.0")
        .send()
        .await
        .context("request latest release")?
        .error_for_status()
        .context("latest release status")?;

    let info = resp
        .json::<ReleaseInfo>()
        .await
        .context("parse latest release json")?;
    let tag_name = info.tag_name.unwrap_or_default();
    if tag_name.trim().is_empty() {
        return Err(anyhow!("latest release missing tag_name"));
    }

    Ok(LatestRelease {
        tag_name: normalize_tag(&tag_name),
        name: info.name,
        body: info.body,
        html_url: info.html_url,
        published_at: info.published_at,
    })
}

pub fn should_notify_startup(report: &UpdateCheckReport) -> bool {
    report.is_new_version && !report.is_dismissed
}
