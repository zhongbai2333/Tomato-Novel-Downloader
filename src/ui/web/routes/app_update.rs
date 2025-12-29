use axum::Json;
use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::base_system::app_update;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) async fn api_app_update() -> Result<Json<Value>, StatusCode> {
    let latest = app_update::fetch_latest_release_async()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let current_tag = format!("v{VERSION}");
    let has_update = latest.tag_name != current_tag;

    Ok(Json(json!({
        "current": VERSION,
        "current_tag": current_tag,
        "latest_tag": latest.tag_name,
        "latest_name": latest.name,
        "latest_body": latest.body,
        "latest_url": latest.html_url,
        "published_at": latest.published_at,
        "has_update": has_update,
    })))
}
