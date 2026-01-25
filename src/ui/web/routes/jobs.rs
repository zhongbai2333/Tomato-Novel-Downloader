use std::sync::atomic::Ordering;
use std::thread;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::base_system::book_id::parse_book_id;
use crate::download::downloader as dl;
use crate::ui::web::state::{AppState, JobState};

#[derive(Debug, Deserialize)]
pub(crate) struct CreateJobReq {
    pub(crate) book_id: String,
    pub(crate) range_start: Option<usize>,
    pub(crate) range_end: Option<usize>,
}

pub(crate) async fn list_jobs(State(state): State<AppState>) -> Json<Value> {
    let items = state.jobs.list();
    Json(json!({ "items": items }))
}

pub(crate) async fn create_job(
    State(state): State<AppState>,
    Json(req): Json<CreateJobReq>,
) -> Result<Json<Value>, StatusCode> {
    let book_id = match parse_book_id(&req.book_id) {
        Some(id) => id,
        None => {
            return Err(StatusCode::BAD_REQUEST);
        }
    };
    if book_id.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Validate range parameters if provided
    if let (Some(start), Some(end)) = (req.range_start, req.range_end) {
        if start < 1 || end < 1 || start > end {
            return Err(StatusCode::BAD_REQUEST);
        }
    } else if req.range_start.is_some() || req.range_end.is_some() {
        // Both range_start and range_end must be provided together
        return Err(StatusCode::BAD_REQUEST);
    }

    let handle = state.jobs.create(book_id.clone());
    let book_id_for_resp = book_id.clone();

    let jobs = state.jobs.clone();
    let cfg = state.config.lock().unwrap().clone();
    let range_start = req.range_start;
    let range_end = req.range_end;

    thread::spawn(move || {
        jobs.set_running(handle.id);

        let plan = match dl::prepare_download_plan(&cfg, &book_id, dl::BookMeta::default()) {
            Ok(p) => p,
            Err(e) => {
                jobs.set_failed(handle.id, format!("prepare plan failed: {e}"));
                return;
            }
        };

        jobs.set_meta(
            handle.id,
            plan.meta.book_name.clone(),
            plan.meta.author.clone(),
        );

        let id = handle.id;
        let jobs_cb = jobs.clone();

        // Build chapter range if specified
        let range = if let (Some(start), Some(end)) = (range_start, range_end) {
            let total = plan.chapters.len();
            if start >= 1 && end >= 1 && start <= end && end <= total {
                Some(dl::ChapterRange { start, end })
            } else {
                None
            }
        } else {
            None
        };

        let result = dl::download_with_plan(
            &cfg,
            plan,
            range,
            Some(Box::new(move |snap| jobs_cb.set_progress(id, snap))),
            Some(handle.cancel.clone()),
        );

        match result {
            Ok(_) => jobs.set_done(handle.id),
            Err(e) => {
                if handle.cancel.load(Ordering::Relaxed) {
                    // ensure state is canceled
                    let _ = jobs.request_cancel(handle.id);
                } else {
                    jobs.set_failed(handle.id, format!("download failed: {e}"));
                }
            }
        }
    });

    Ok(Json(
        json!({ "id": handle.id, "book_id": book_id_for_resp, "state": JobState::Queued }),
    ))
}

pub(crate) async fn cancel_job(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Json<Value>, StatusCode> {
    if state.jobs.request_cancel(id) {
        Ok(Json(json!({"ok": true})))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}
