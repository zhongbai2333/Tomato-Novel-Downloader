use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use tomato_novel_official_api::SearchClient;

use crate::ui::web::state::AppState;

#[derive(Debug, Deserialize)]
pub(crate) struct SearchQuery {
    pub(crate) q: String,
}

pub(crate) async fn api_search(
    State(_state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Value>, StatusCode> {
    let keyword = q.q.trim().to_string();
    if keyword.is_empty() {
        return Ok(Json(json!({"items": []})));
    }

    let resp = tokio::task::spawn_blocking(move || {
        let client = SearchClient::new()?;
        client.search_books(&keyword)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::BAD_GATEWAY)?;

    let items: Vec<Value> = resp
        .books
        .into_iter()
        .map(|b| {
            json!({
                "book_id": b.book_id,
                "title": b.title,
                "author": b.author,
                "raw": b.raw,
            })
        })
        .collect();

    Ok(Json(json!({"items": items})))
}
