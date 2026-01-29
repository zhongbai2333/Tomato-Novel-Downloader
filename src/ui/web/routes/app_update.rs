use axum::Json;
use axum::http::StatusCode;
use serde_json::{Value, json};

use std::thread;
use std::time::Duration;

use crate::base_system::app_update;

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

pub(crate) async fn api_self_update() -> Result<Json<Value>, StatusCode> {
    if cfg!(feature = "docker") {
        return Err(StatusCode::BAD_REQUEST);
    }

    // 先返回响应，再启动自更新；否则进程 exit 可能导致客户端收不到响应。
    thread::spawn(|| {
        thread::sleep(Duration::from_millis(600));
        let _ = crate::base_system::self_update::check_for_updates(VERSION, true);
    });

    Ok(Json(json!({
        "ok": true,
        "message": "self update scheduled"
    })))
}
