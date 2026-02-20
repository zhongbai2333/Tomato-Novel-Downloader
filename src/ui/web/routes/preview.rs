use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::base_system::book_id::parse_book_id;
use crate::download::downloader as dl;
use crate::ui::web::state::AppState;

pub(crate) async fn api_preview(
    State(state): State<AppState>,
    Path(book_id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let book_id = match parse_book_id(&book_id) {
        Some(id) => id,
        None => {
            return Err(StatusCode::BAD_REQUEST);
        }
    };
    if book_id.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let cfg = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let book_id_for_plan = book_id.clone();

    let plan = tokio::task::spawn_blocking(move || {
        dl::prepare_download_plan(&cfg, &book_id_for_plan, dl::BookMeta::default())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::BAD_GATEWAY)?;

    let meta = &plan.meta;
    let chapter_count = plan.chapters.len();

    Ok(Json(json!({
        "book_id": book_id,
        "book_name": meta.book_name,
        "original_book_name": meta.original_book_name,
        "author": meta.author,
        "description": meta.description,
        "tags": meta.tags,
        "cover_url": meta.cover_url,
        "detail_cover_url": meta.detail_cover_url,
        "finished": meta.finished,
        "chapter_count": chapter_count,
        "word_count": meta.word_count,
        "score": meta.score,
        "read_count": meta.read_count,
        "read_count_text": meta.read_count_text,
        "category": meta.category,
        "first_chapter_title": meta.first_chapter_title,
        "last_chapter_title": meta.last_chapter_title,
    })))
}
