use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::{Value, json};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::base_system::app_update;
use crate::base_system::self_update::SelfUpdateOutcome;
use crate::ui::web::state::{AppState, SelfUpdateState};

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) async fn api_app_update() -> Result<Json<Value>, StatusCode> {
    if cfg!(feature = "docker") {
        let current_tag = format!("v{VERSION}");
        return Ok(Json(json!({
            "current": VERSION,
            "current_tag": current_tag,
            "latest_tag": current_tag,
            "latest_name": "Docker build",
            "latest_body": "Docker 构建已禁用程序自更新，请通过重新拉取镜像进行升级。",
            "latest_url": Value::Null,
            "published_at": Value::Null,
            "has_update": false,
            "docker_build": true,
        })));
    }

    let latest = app_update::fetch_latest_release_async()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let current_tag = format!("v{VERSION}");
    let has_update = latest.tag_name != current_tag;

    // WebUI 的自动检查：如果版本号相同但检测到热更新（SHA 不同），则强制更新。
    // 为确保客户端能收到本次响应，先返回，再在后台线程触发检查/更新。
    if !has_update {
        thread::spawn(|| {
            thread::sleep(Duration::from_millis(600));
            let _ = crate::base_system::self_update::check_hotfix_and_apply(VERSION);
        });
    }

    Ok(Json(json!({
        "current": VERSION,
        "current_tag": current_tag,
        "latest_tag": latest.tag_name,
        "latest_name": latest.name,
        "latest_body": latest.body,
        "latest_url": latest.html_url,
        "published_at": latest.published_at,
        "has_update": has_update,
        "docker_build": false,
    })))
}

pub(crate) async fn api_self_update(
    State(state): State<AppState>,
) -> Result<Json<Value>, StatusCode> {
    if cfg!(feature = "docker") {
        return Err(StatusCode::BAD_REQUEST);
    }

    if !state.self_update.try_start() {
        let snap = state.self_update.snapshot();
        return Ok(Json(json!({
            "ok": true,
            "already_running": true,
            "status": snap,
        })));
    }

    let store = state.self_update.clone();
    let running = Arc::new(AtomicBool::new(true));
    let ticker_running = running.clone();
    let ticker_store = store.clone();

    thread::spawn(move || {
        while ticker_running.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(600));
            ticker_store.tick_running();
        }
    });

    thread::spawn(move || {
        store.set(SelfUpdateState::Running, "check", 8, "检查最新版本…");
        thread::sleep(Duration::from_millis(150));
        store.set(SelfUpdateState::Running, "download", 18, "开始下载更新包…");

        let result = crate::base_system::self_update::check_for_updates(VERSION, true);

        running.store(false, Ordering::Relaxed);
        match result {
            Ok(SelfUpdateOutcome::UpToDate) => {
                store.finish_done("done", "已是最新版本，无需更新");
            }
            Ok(SelfUpdateOutcome::Skipped) => {
                store.finish_done("skipped", "已跳过更新");
            }
            Ok(SelfUpdateOutcome::UpdateLaunched) => {
                store.finish_done("restart", "更新已完成，服务正在重启");
            }
            Err(e) => {
                store.finish_failed("failed", format!("自更新失败: {e}"));
            }
        }
    });

    Ok(Json(json!({
        "ok": true,
        "already_running": false,
        "message": "self update started"
    })))
}

pub(crate) async fn api_self_update_status(
    State(state): State<AppState>,
) -> Result<Json<Value>, StatusCode> {
    Ok(Json(json!(state.self_update.snapshot())))
}
