//! 程序自更新（检查 GitHub Release 并替换当前可执行文件）。
//!
//! 这是对历史 Python `update.py` 的 Rust 侧移植：
//! - 通过 GitHub Releases API 获取最新版本
//! - 选择匹配当前平台/架构的资产
//! - 可选使用 `https://gh-proxy.org/` 前缀加速（可通过 `TND_DISABLE_ACCEL=1` 禁用）
//! - 下载后按需校验 SHA256（若 Release 资产提供 digest）
//! - Windows 使用临时 .bat 进行替换并重启；Unix 直接替换并重启

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, USER_AGENT};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tracing::{info, warn};

const OWNER: &str = "zhongbai2333";
const REPO: &str = "Tomato-Novel-Downloader";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfUpdateOutcome {
    UpToDate,
    Skipped,
    UpdateLaunched,
}

#[derive(Debug, Deserialize)]
struct ReleaseInfo {
    name: Option<String>,
    tag_name: Option<String>,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    size: Option<u64>,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Debug, Clone)]
struct MatchedReleaseAsset {
    release_name: String,
    tag_name: String,
    download_url: String,
    size: u64,
    sha256: Option<String>,
}

pub fn check_for_updates(current_version: &str, auto_yes: bool) -> Result<SelfUpdateOutcome> {
    info!(target: "self_update", "正在检查程序更新…");

    let current_tag = format!("v{current_version}");
    let matched = get_latest_release_asset()?;
    let is_new_version = matched.tag_name != current_tag;
    if is_new_version {
        info!(
            target: "self_update",
            latest = %matched.tag_name,
            current = %current_tag,
            "检测到新版本"
        );

        if !auto_yes {
            let mut input = String::new();
            print!("是否下载并升级到最新版？[Y/n]: ");
            std::io::stdout().flush().ok();
            std::io::stdin().read_line(&mut input).ok();
            let ans = input.trim().to_ascii_lowercase();
            if !(ans.is_empty() || ans == "y" || ans == "yes") {
                warn!(target: "self_update", "用户取消升级");
                return Ok(SelfUpdateOutcome::Skipped);
            }
        }

        start_update(&matched)?;
        return Ok(SelfUpdateOutcome::UpdateLaunched);
    }

    info!(target: "self_update", "本地版本与最新相同，检查热补丁…");

    if let Some(expected) = matched.sha256.as_deref() {
        let self_hash = compute_file_sha256(&current_executable_path()?)?;
        if !eq_hash(&self_hash, expected) {
            info!(target: "self_update", "检测到热补丁（SHA256 不同），开始更新…");
            start_update(&matched)?;
            return Ok(SelfUpdateOutcome::UpdateLaunched);
        }
    }

    Ok(SelfUpdateOutcome::UpToDate)
}

fn eq_hash(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn github_latest_release_url() -> String {
    format!("https://api.github.com/repos/{OWNER}/{REPO}/releases/latest")
}

fn build_http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("init http client")
}

fn fetch_latest_release(client: &Client) -> Result<ReleaseInfo> {
    let url = github_latest_release_url();
    let resp = client
        .get(url)
        .header(ACCEPT, "application/vnd.github+json")
        .header(USER_AGENT, "Tomato-Novel-Downloader/1.0")
        .send()
        .context("request latest release")?
        .error_for_status()
        .context("latest release status")?;

    resp.json::<ReleaseInfo>()
        .context("parse latest release json")
}

fn detect_platform_keyword() -> String {
    // 对齐 Python 版本：
    // - Linux:  Linux_amd64 / Linux_arm64
    // - Windows: Win64
    // - macOS:  macOS_amd64 / macOS_arm64
    let system = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let arch_key = match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };

    match system {
        "linux" => format!("Linux_{arch_key}"),
        "windows" => "Win64".to_string(),
        "macos" => format!("macOS_{arch_key}"),
        other => other.to_string(),
    }
}

fn get_latest_release_asset() -> Result<MatchedReleaseAsset> {
    let client = build_http_client()?;
    let latest = fetch_latest_release(&client)?;

    let platform_key = detect_platform_keyword();
    let release_name = latest.name.unwrap_or_else(|| "".to_string());
    let tag_name = latest.tag_name.unwrap_or_else(|| "".to_string());

    if tag_name.is_empty() {
        return Err(anyhow!("latest release missing tag_name"));
    }

    for asset in latest.assets {
        if asset.name.contains(&platform_key) {
            let original_url = asset.browser_download_url;
            let accel_disabled = std::env::var("TND_DISABLE_ACCEL").ok().as_deref() == Some("1");
            let download_url = if accel_disabled {
                original_url.clone()
            } else {
                get_accelerated_url(&original_url)
            };

            let sha256 = asset
                .digest
                .as_deref()
                .and_then(|d| d.split(':').next_back())
                .map(|s| s.trim().to_string())
                .filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()));

            return Ok(MatchedReleaseAsset {
                release_name,
                tag_name,
                download_url,
                size: asset.size.unwrap_or(0),
                sha256,
            });
        }
    }

    Err(anyhow!(
        "no matching release asset for platform_key={platform_key}"
    ))
}

fn get_accelerated_url(original_url: &str) -> String {
    // 直接使用 gh-proxy 前缀加速：
    // https://gh-proxy.org/<Github Download URL>
    // 例如：
    // https://gh-proxy.org/https://github.com/<owner>/<repo>/releases/download/<tag>/<asset>
    let url = format!("https://gh-proxy.org/{original_url}");
    info!(target: "self_update", "使用 gh-proxy 加速下载地址: {url}");
    url
}

fn start_update(matched: &MatchedReleaseAsset) -> Result<()> {
    info!(
        target: "self_update",
        name = %matched.release_name,
        tag = %matched.tag_name,
        "开始下载最新版本"
    );

    let tmp_dir = TempDir::new().context("create temp dir")?;
    let tmp_file = download_and_verify(tmp_dir.path(), matched)?;

    info!(target: "self_update", "下载完成，开始应用更新…");

    if cfg!(windows) {
        windows_apply_and_restart(&tmp_file)?;
        // windows 通过 bat 异步完成替换并拉起新进程
        std::process::exit(0);
    }

    let new_exe = unix_apply(&tmp_file)?;
    info!(target: "self_update", "更新完成，正在重启程序…");

    let mut cmd = Command::new(&new_exe);
    cmd.args(std::env::args_os().skip(1));
    cmd.env("PYINSTALLER_RESET_ENVIRONMENT", "1");
    cmd.spawn().context("spawn new executable")?;
    std::process::exit(0);
}

fn download_and_verify(tmp_dir: &Path, matched: &MatchedReleaseAsset) -> Result<PathBuf> {
    let client = build_http_client()?;
    let url = &matched.download_url;

    let resp = client
        .get(url)
        .header(USER_AGENT, "Mozilla/5.0 (X11; Linux x86_64) Updater/1.0")
        .timeout(Duration::from_secs(60))
        .send()
        .with_context(|| format!("download asset: {url}"))?
        .error_for_status()
        .context("download status")?;

    let total = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .or(Some(matched.size))
        .unwrap_or(0);

    let fname = Path::new(url)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("update.bin");

    let out_path = tmp_dir.join(fname);

    let pb = if total > 0 {
        let pb = ProgressBar::new(total);
        pb.set_style(
            ProgressStyle::with_template(
                "{msg} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
            )
            .unwrap()
            .progress_chars("##-"),
        );
        pb.set_message(format!("Downloading {fname}"));
        Some(pb)
    } else {
        None
    };

    let mut hasher = Sha256::new();
    let mut file = fs::File::create(&out_path).context("create temp file")?;
    let mut reader = resp;
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf).context("read download stream")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("write temp file")?;
        hasher.update(&buf[..n]);
        if let Some(pb) = pb.as_ref() {
            pb.inc(n as u64);
        }
    }
    if let Some(pb) = pb {
        pb.finish_and_clear();
    }

    let actual = hex::encode(hasher.finalize());
    if let Some(expected) = matched.sha256.as_deref()
        && !eq_hash(&actual, expected)
    {
        let _ = fs::remove_file(&out_path);
        return Err(anyhow!(
            "SHA256 校验失败：下载文件 {} 的哈希 {} 与期望 {} 不符",
            out_path.display(),
            actual,
            expected
        ));
    }

    Ok(out_path)
}

fn current_executable_path() -> Result<PathBuf> {
    std::env::current_exe().context("current_exe")
}

fn compute_file_sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).context("read file")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn unix_apply(tmp_file: &Path) -> Result<PathBuf> {
    let local_exe = current_executable_path()?;
    let parent = local_exe
        .parent()
        .ok_or_else(|| anyhow!("cannot determine executable directory"))?;

    let file_name = tmp_file
        .file_name()
        .ok_or_else(|| anyhow!("temp file missing name"))?;
    let new_exe = parent.join(file_name);

    if local_exe.exists() {
        let _ = fs::remove_file(&local_exe);
    }

    fs::rename(tmp_file, &new_exe).context("move new executable")?;

    // chmod 755 best-effort
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(&new_exe)?.permissions();
        perm.set_mode(0o755);
        let _ = fs::set_permissions(&new_exe, perm);
    }

    Ok(new_exe)
}

fn windows_apply_and_restart(tmp_file: &Path) -> Result<()> {
    let local_exe = current_executable_path()?;
    let parent = local_exe
        .parent()
        .ok_or_else(|| anyhow!("cannot determine executable directory"))?;

    let file_name = tmp_file
        .file_name()
        .ok_or_else(|| anyhow!("temp file missing name"))?;
    let new_exe = parent.join(file_name);

    // move to <name>.new next to executable
    let new_name = new_exe
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("invalid new exe name"))?;
    let staged = parent.join(format!("{new_name}.new"));
    let _ = fs::remove_file(&staged);
    fs::rename(tmp_file, &staged).context("stage new executable")?;

    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let arg_line = args
        .drain(..)
        .map(|a| {
            let s = a.to_string_lossy();
            // very small quoting for cmd.exe
            if s.contains(' ') || s.contains('"') {
                OsString::from(format!("\"{}\"", s.replace('"', "\\\"")))
            } else {
                OsString::from(s.as_ref())
            }
        })
        .collect::<Vec<_>>();

    let exe_name = local_exe
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("invalid exe name"))?;

    // bat: wait -> delete old -> rename staged -> start -> delete self
    let mut lines = Vec::new();
    lines.push("@echo off".to_string());
    lines.push("echo Waiting...".to_string());
    lines.push("timeout /t 3 /nobreak".to_string());
    lines.push("".to_string());
    lines.push(format!("cd /d \"{}\"", parent.display()));
    lines.push("".to_string());
    lines.push(format!(
        "if exist \"{}\" (del /F /Q \"{}\")",
        exe_name, exe_name
    ));
    lines.push(format!(
        "if exist \"{new_name}.new\" (ren \"{new_name}.new\" \"{new_name}\")"
    ));
    lines.push("".to_string());
    lines.push("set PYINSTALLER_RESET_ENVIRONMENT=1".to_string());

    let mut start_line = format!("start \"\" \"{}\"", new_name);
    for a in arg_line {
        start_line.push(' ');
        start_line.push_str(&a.to_string_lossy());
    }
    lines.push(start_line);
    lines.push("".to_string());
    lines.push("del \"%~f0\"".to_string());

    let bat_content = lines.join("\r\n");
    let bat_path = std::env::temp_dir().join("tnd_update_script.bat");
    fs::write(&bat_path, bat_content).context("write update bat")?;

    Command::new(&bat_path)
        .spawn()
        .context("spawn update bat")?;

    info!(target: "self_update", "请稍等，更新完成后将自动重启程序。");
    Ok(())
}
