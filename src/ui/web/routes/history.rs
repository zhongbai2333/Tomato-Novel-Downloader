use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::base_system::download_history::read_download_history;
use crate::ui::web::state::AppState;

#[derive(Debug, Deserialize)]
pub(crate) struct HistoryQuery {
    pub(crate) limit: Option<usize>,
    pub(crate) q: Option<String>,
}

pub(crate) async fn api_history(
    State(_state): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Value>, StatusCode> {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let keyword = q.q.as_deref();
    let items = read_download_history(limit, keyword);

    Ok(Json(json!({
        "items": items,
        "limit": limit,
        "keyword": q.q,
    })))
}
