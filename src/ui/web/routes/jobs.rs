use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::base_system::book_id::resolve_book_id;
use crate::base_system::context::Config;
use crate::download::downloader as dl;
use crate::ui::web::download_queue::QueueTask;
use crate::ui::web::state::{
    AppState, JobHandle, JobState, JobStore, RECENT_DONE_JOB_RETENTION_MS,
};

#[derive(Debug, Deserialize)]
pub(crate) struct ListJobsQuery {
    /// 按 job id 精确过滤
    pub(crate) id: Option<u64>,
    /// 按书名/书ID关键词模糊过滤（忽略大小写）
    pub(crate) name: Option<String>,
    /// 返回全部任务；默认会自动隐藏/清理 2 小时前已完成的任务。
    pub(crate) all: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateJobReq {
    pub(crate) book_id: String,
    pub(crate) range_start: Option<usize>,
    pub(crate) range_end: Option<usize>,
}

pub(crate) async fn list_jobs(
    State(state): State<AppState>,
    Query(q): Query<ListJobsQuery>,
) -> Json<Value> {
    if q.id.is_none() && !q.all.unwrap_or(false) {
        state
            .jobs
            .prune_done_older_than(RECENT_DONE_JOB_RETENTION_MS);
    }
    let mut items = state.jobs.list();

    // 排队位置仅在队列启用时展示。
    if state.download_queue.enabled() {
        let order = state.download_queue.waiting_order();
        state.jobs.apply_queue_positions(&mut items, &order);
    }

    // 队列概览（基于全量任务计算，不受过滤条件影响）。
    let queue_value = queue_summary(&state);

    if let Some(id) = q.id {
        items.retain(|j| j.id == id);
    }
    if let Some(ref kw) = q.name {
        let kw_lower = kw.to_lowercase();
        items.retain(|j| {
            j.title
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .contains(&kw_lower)
                || j.book_id.to_lowercase().contains(&kw_lower)
        });
    }
    Json(json!({
        "items": items,
        "done_retention_ms": RECENT_DONE_JOB_RETENTION_MS,
        "queue": queue_value,
    }))
}

/// 构建队列概览信息（当前执行 / 等待数量 / 下一本等），供 WebUI 展示。
fn queue_summary(state: &AppState) -> Value {
    let queue = &state.download_queue;
    let all = state.jobs.list();

    let running = all
        .iter()
        .find(|j| matches!(j.state, JobState::Running))
        .map(|j| {
            json!({
                "id": j.id,
                "book_id": j.book_id,
                "title": j.title,
            })
        });

    let next = queue
        .waiting_order()
        .first()
        .and_then(|id| all.iter().find(|j| j.id == *id))
        .map(|j| {
            json!({
                "id": j.id,
                "book_id": j.book_id,
                "title": j.title,
            })
        });

    json!({
        "enabled": queue.enabled(),
        "capacity": queue.capacity(),
        "delay_secs": queue.delay_secs(),
        "queued_count": queue.waiting(),
        "running_count": state.jobs.count_running(),
        "current": running,
        "next": next,
    })
}

pub(crate) async fn create_job(
    State(state): State<AppState>,
    Json(req): Json<CreateJobReq>,
) -> Result<Json<Value>, Response> {
    let book_id_raw = req.book_id.clone();
    let book_id = tokio::task::spawn_blocking(move || resolve_book_id(&book_id_raw))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?
        .ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?;
    if book_id.is_empty() {
        return Err(StatusCode::BAD_REQUEST.into_response());
    }

    let queue_enabled = state.download_queue.enabled();

    // 并发限制（队列关闭时保持旧行为）：同时只允许一个活跃任务，
    // 防止 API 被滥用为多用户服务。队列开启时改由 FIFO 队列调度，
    // 不再因为已有一本小说正在下载就直接返回 429。
    if !queue_enabled && state.jobs.count_active() >= 1 {
        return Err(StatusCode::TOO_MANY_REQUESTS.into_response());
    }

    // Validate range parameters if provided
    if let (Some(start), Some(end)) = (req.range_start, req.range_end) {
        if start < 1 || end < 1 || start > end {
            return Err(StatusCode::BAD_REQUEST.into_response());
        }
    } else if req.range_start.is_some() || req.range_end.is_some() {
        // Both range_start and range_end must be provided together
        return Err(StatusCode::BAD_REQUEST.into_response());
    }

    let handle = state.jobs.create(book_id.clone());
    let book_id_for_resp = book_id.clone();

    let jobs = state.jobs.clone();
    let cfg = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let range_start = req.range_start;
    let range_end = req.range_end;

    if queue_enabled {
        // 保存当前下载配置快照并加入 FIFO 队列；由唯一 Worker 顺序执行。
        let task = QueueTask {
            handle: handle.clone(),
            book_id: book_id.clone(),
            config: cfg,
            range_start,
            range_end,
        };
        if !state.download_queue.push(task) {
            // 队列满：清理刚创建的任务，避免留下无法执行的幽灵任务。
            state.jobs.remove(handle.id);
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": "queue_full",
                    "message": format!(
                        "下载队列已满（最多等待 {} 个任务），请等待已有任务完成后再试",
                        state.download_queue.capacity()
                    ),
                })),
            )
                .into_response());
        }
        return Ok(Json(
            json!({ "id": handle.id, "book_id": book_id_for_resp, "state": JobState::Queued }),
        ));
    }

    // 队列关闭：保持原有行为（每个请求一个下载线程）。
    let handle_for_thread = handle.clone();
    thread::spawn(move || {
        execute_download_job(
            &jobs,
            &handle_for_thread,
            &book_id,
            &cfg,
            range_start,
            range_end,
        );
    });

    Ok(Json(
        json!({ "id": handle.id, "book_id": book_id_for_resp, "state": JobState::Queued }),
    ))
}

/// 执行单个下载任务（队列关闭时的线程入口，也是队列 Worker 的执行体）。
///
/// 负责完整的状态回写：Running → Done / Failed / Canceled。
/// 该函数被设计为可独立调用，便于测试与队列复用。
pub(crate) fn execute_download_job(
    jobs: &Arc<JobStore>,
    handle: &JobHandle,
    book_id: &str,
    config: &Config,
    range_start: Option<usize>,
    range_end: Option<usize>,
) {
    jobs.set_running(handle.id);

    let plan = match dl::prepare_download_plan(config, book_id, dl::BookMeta::default()) {
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

    let jobs_ask = jobs.clone();
    let book_name_asker = move |manager: &crate::book_parser::book_manager::BookManager| {
        let options = dl::collect_book_name_options(manager);
        if options.len() <= 1 {
            return None;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        jobs_ask.set_book_name_options(id, options, tx);
        rx.recv().ok().flatten()
    };

    let jobs_fmt = jobs.clone();
    let format_asker = move |_manager: &crate::book_parser::book_manager::BookManager| {
        let options = dl::collect_output_format_options();
        let (tx, rx) = std::sync::mpsc::channel();
        jobs_fmt.set_format_options(id, options, tx);
        rx.recv().ok().flatten()
    };

    let result = dl::download_with_plan_flow(
        config,
        plan,
        None,
        dl::DownloadFlowOptions {
            mode: dl::DownloadMode::Resume,
            range,
            retry_failed: {
                let mut retried = false;
                dl::RetryFailed::Decide(Box::new(move |_pending_len| {
                    if retried {
                        return false;
                    }
                    retried = true;
                    true
                }))
            },
            stage_callback: None,
            book_name_asker: Some(Box::new(book_name_asker)),
            format_asker: Some(Box::new(format_asker)),
        },
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
}

#[derive(Debug, Deserialize)]
pub(crate) struct BookNameChoiceReq {
    pub(crate) value: Option<String>,
}

pub(crate) async fn submit_book_name_choice(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(req): Json<BookNameChoiceReq>,
) -> Result<Json<Value>, StatusCode> {
    if state.jobs.submit_book_name_choice(id, req.value) {
        Ok(Json(json!({"ok": true})))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub(crate) async fn submit_format_choice(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(req): Json<BookNameChoiceReq>,
) -> Result<Json<Value>, StatusCode> {
    if state.jobs.submit_format_choice(id, req.value) {
        Ok(Json(json!({"ok": true})))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub(crate) async fn cancel_job(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Json<Value>, StatusCode> {
    if !state.download_queue.enabled() {
        // 队列关闭：保持原有行为（取消并立即从列表移除）。
        if state.jobs.request_cancel_and_remove(id) {
            return Ok(Json(json!({"ok": true})));
        }
        return Err(StatusCode::NOT_FOUND);
    }

    // 队列开启：
    // 1) 先尝试按“等待中的任务”取消：从 FIFO 队列移除并标记 Canceled；
    // 2) 若任务不在等待队列（可能已开始执行），按运行中任务取消：
    //    设置 AtomicBool 取消标志并标记 Canceled（条目保留用于展示与重试）。
    let was_waiting = state.download_queue.cancel_waiting(id);
    if state.jobs.request_cancel_active(id) || was_waiting {
        return Ok(Json(json!({"ok": true})));
    }
    // 任务不存在或已结束。
    if state.jobs.contains(id) {
        // 已结束（Done/Failed/Canceled）的任务无需取消，视为成功以保持幂等。
        return Ok(Json(json!({"ok": true})));
    }
    Err(StatusCode::NOT_FOUND)
}

pub(crate) async fn delete_job(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Json<Value>, StatusCode> {
    // 若任务仍在等待队列中，先从队列移除，避免占用等待容量。
    state.download_queue.cancel_waiting(id);
    if state.jobs.remove(id) {
        Ok(Json(json!({"ok": true})))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

#[cfg(test)]
mod api_tests {
    //! API 层测试：使用 Mock 执行器，不会请求番茄小说上游接口。

    use super::*;
    use crate::ui::web::download_queue::SharedExecutor;
    use crate::ui::web::state::{ConfigView, JobState};
    use axum::body::to_bytes;

    /// 构造测试用 AppState（队列开启，容量 capacity，Mock 执行器）。
    fn test_state(capacity: usize, enabled: bool) -> AppState {
        let jobs = Arc::new(JobStore::default());
        let executor: SharedExecutor = {
            Arc::new(move |jobs: &Arc<JobStore>, task: &QueueTask| {
                jobs.set_done(task.handle.id);
            })
        };
        make_state(jobs, capacity, enabled, executor, true)
    }

    /// 用自定义执行器构造 AppState；`spawn` 控制是否启动 Worker。
    fn make_state(
        jobs: Arc<JobStore>,
        capacity: usize,
        enabled: bool,
        executor: SharedExecutor,
        spawn: bool,
    ) -> AppState {
        let queue = Arc::new(crate::ui::web::download_queue::DownloadQueue::new(
            jobs.clone(),
            enabled,
            capacity,
            0,
            executor,
        ));
        if enabled && spawn {
            queue.spawn_worker();
        }
        let config = crate::base_system::context::Config::default();
        AppState {
            bind_addrs: Arc::new(Vec::new()),
            config_view: Arc::new(ConfigView {
                old_cli: false,
                use_official_api: false,
                save_path: String::new(),
                api_endpoints_len: 0,
            }),
            config: Arc::new(std::sync::Mutex::new(config)),
            config_path: Arc::new(std::path::PathBuf::from("config.yml")),
            library_root: Arc::new(std::path::PathBuf::from(".")),
            jobs,
            download_queue: queue,
            self_update: Arc::new(crate::ui::web::state::SelfUpdateStore::default()),
            library_scan: Arc::new(crate::ui::web::state::LibraryScanStore::default()),
            update_scan: Arc::new(crate::ui::web::state::UpdateScanStore::default()),
            auth: None,
        }
    }

    async fn create(state: &AppState, book_id: &str) -> axum::response::Response {
        match create_job(
            State(state.clone()),
            Json(CreateJobReq {
                book_id: book_id.to_string(),
                range_start: None,
                range_end: None,
            }),
        )
        .await
        {
            Ok(Json(v)) => Json(v).into_response(),
            Err(resp) => resp,
        }
    }

    #[allow(dead_code)]
    async fn create_with_range(
        state: &AppState,
        book_id: &str,
        range_start: Option<usize>,
        range_end: Option<usize>,
    ) -> axum::response::Response {
        match create_job(
            State(state.clone()),
            Json(CreateJobReq {
                book_id: book_id.to_string(),
                range_start,
                range_end,
            }),
        )
        .await
        {
            Ok(Json(v)) => Json(v).into_response(),
            Err(resp) => resp,
        }
    }

    async fn body_json(resp: Response) -> Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn queue_disabled_returns_429_when_a_job_is_active() {
        let state = test_state(20, false);
        // 直接在 store 中预置一个活跃任务（避免触发真实下载线程）。
        state.jobs.create("111".to_string());
        assert_eq!(state.jobs.count_active(), 1);

        let resp = create(&state, "714303869").await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn queue_enabled_allows_multiple_sequential_creates() {
        let state = test_state(20, true);

        for book in ["714303869", "7423591956359416856", "123"] {
            let resp = create(&state, book).await;
            assert_eq!(resp.status(), StatusCode::OK);
            let body = body_json(resp).await;
            assert_eq!(body["book_id"], book);
            assert_eq!(body["state"], "queued");
        }

        // 等待 Mock 执行器完成全部任务。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline && state.jobs.count_active() > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let list = state.jobs.list();
        assert!(
            list.iter()
                .all(|j| matches!(j.state, JobState::Done | JobState::Running))
        );
    }

    #[tokio::test]
    async fn queue_full_returns_clear_error() {
        // 不启动 Worker：任务全部滞留在队列中，容量确定性满。
        let jobs = Arc::new(JobStore::default());
        let executor: SharedExecutor = Arc::new(|_jobs, _task| {});
        let state = make_state(jobs, 1, true, executor, false);

        let resp1 = create(&state, "111").await;
        assert_eq!(resp1.status(), StatusCode::OK);

        let resp2 = create(&state, "222").await;
        assert_eq!(resp2.status(), StatusCode::TOO_MANY_REQUESTS);
        let body = body_json(resp2).await;
        assert_eq!(body["error"], "queue_full");
        assert!(body["message"].as_str().unwrap_or("").contains("队列已满"));

        // 被拒绝的任务已被清理，不会留下幽灵任务。
        assert_eq!(state.jobs.list().len(), 1);
    }

    #[tokio::test]
    async fn invalid_range_returns_400() {
        let state = test_state(20, true);

        // 只提供 range_start（必须成对提供）。
        let resp = create_with_range(&state, "714303869", Some(1), None).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // start > end。
        let resp = create_with_range(&state, "714303869", Some(5), Some(2)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    async fn cancel(state: &AppState, id: u64) -> axum::response::Response {
        match cancel_job(State(state.clone()), Path(id)).await {
            Ok(Json(v)) => Json(v).into_response(),
            Err(e) => e.into_response(),
        }
    }

    #[tokio::test]
    async fn cancel_waiting_job_marks_canceled_and_keeps_others() {
        // 慢执行器：保证取消发生时任务 2 仍在等待队列中。
        let jobs = Arc::new(JobStore::default());
        let executor: SharedExecutor = Arc::new(|jobs: &Arc<JobStore>, task: &QueueTask| {
            std::thread::sleep(std::time::Duration::from_millis(300));
            jobs.set_done(task.handle.id);
        });
        let state = make_state(jobs, 20, true, executor, true);

        let r1 = body_json(create(&state, "111").await).await;
        let r2 = body_json(create(&state, "222").await).await;
        let r3 = body_json(create(&state, "333").await).await;
        let id2 = r2["id"].as_u64().unwrap();

        // 第一个任务正在执行（约 300ms），此时取消等待中的 job2。
        let resp = cancel(&state, id2).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let jobs = state.jobs.list();
        let job2 = jobs.iter().find(|j| j.id == id2).expect("job2 exists");
        assert!(matches!(job2.state, JobState::Canceled));
        // 其他任务仍然存在。
        assert!(jobs.iter().any(|j| j.id == r1["id"].as_u64().unwrap()));
        assert!(jobs.iter().any(|j| j.id == r3["id"].as_u64().unwrap()));
    }

    #[tokio::test]
    async fn list_jobs_reports_queue_info_and_positions() {
        let jobs = Arc::new(JobStore::default());
        let executor: SharedExecutor = Arc::new(|_jobs, _task| {});
        // 不启动 Worker：任务全部滞留，排队位置可确定性断言。
        let state = make_state(jobs, 20, true, executor, false);

        let _ = body_json(create(&state, "111").await).await;
        let _ = body_json(create(&state, "222").await).await;
        let _ = body_json(create(&state, "333").await).await;

        let query = ListJobsQuery {
            id: None,
            name: None,
            all: None,
        };
        let resp = list_jobs(State(state.clone()), Query(query)).await;
        let body = body_json(resp.into_response()).await;

        let queue_info = &body["queue"];
        assert_eq!(queue_info["enabled"], true);
        assert_eq!(queue_info["capacity"], 20);
        assert_eq!(queue_info["queued_count"], 3);
        assert_eq!(queue_info["running_count"], 0);
        assert_eq!(queue_info["next"]["book_id"], "111");

        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        let pos: Vec<Option<u64>> = items
            .iter()
            .filter(|it| it["state"] == "queued")
            .map(|it| it["queue_position"].as_u64())
            .collect();
        // FIFO 位置：三个等待任务分别处于第 1、2、3 位。
        let mut sorted = pos.clone();
        sorted.sort();
        assert_eq!(sorted, vec![Some(1), Some(2), Some(3)]);
    }

    #[tokio::test]
    async fn list_jobs_without_queue_has_no_position_field() {
        let state = test_state(20, false);
        state.jobs.create("111".to_string());

        let query = ListJobsQuery {
            id: None,
            name: None,
            all: None,
        };
        let resp = list_jobs(State(state), Query(query)).await;
        let body = body_json(resp.into_response()).await;
        assert_eq!(body["queue"]["enabled"], false);
        let items = body["items"].as_array().unwrap();
        assert!(items.iter().all(|it| it.get("queue_position").is_none()));
    }

    #[tokio::test]
    async fn cancel_then_delete_flow_works_with_queue_enabled() {
        let state = test_state(20, false);
        let handle = state.jobs.create("111".to_string());

        // 队列关闭时取消仍保持旧行为：取消并立即从列表移除。
        let resp = cancel(&state, handle.id).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(state.jobs.list().is_empty());

        // 再次取消返回 404（任务已不存在）。
        let resp = cancel(&state, handle.id).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
