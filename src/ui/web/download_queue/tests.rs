//! DownloadQueue 单元测试。
//!
//! 全部使用 Mock 执行器，不会请求番茄小说上游接口。

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::*;
use crate::ui::web::state::JobState;

/// 测试用执行记录器：记录执行顺序、并发数、执行线程。
#[derive(Default)]
struct Recorder {
    /// 已开始执行的任务 book_id 顺序。
    order: Mutex<Vec<String>>,
    /// 当前并发执行数。
    active: AtomicUsize,
    /// 观测到的最大并发数。
    max_active: AtomicUsize,
    /// 执行任务的线程 id（应只有一个）。
    threads: Mutex<Vec<thread::ThreadId>>,
    /// 每个任务的模拟耗时（毫秒，可在运行中调整）。
    sleep_ms: AtomicU64,
    /// 这些 book_id 的任务执行时标记为 Failed（模拟下载失败）。
    fail_ids: Vec<String>,
    /// 这些 book_id 的任务执行时直接 panic（模拟程序异常）。
    panic_ids: Vec<String>,
}

impl Recorder {
    fn record(&self, jobs: &Arc<JobStore>, task: &QueueTask) {
        let entered = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(entered, Ordering::SeqCst);
        self.threads.lock().unwrap().push(thread::current().id());
        self.order.lock().unwrap().push(task.book_id.clone());

        let ms = self.sleep_ms.load(Ordering::SeqCst);
        if ms > 0 {
            thread::sleep(Duration::from_millis(ms));
        }

        if self.panic_ids.contains(&task.book_id) {
            self.active.fetch_sub(1, Ordering::SeqCst);
            panic!("mock panic for {}", task.book_id);
        }
        if self.fail_ids.contains(&task.book_id) {
            jobs.set_failed(task.handle.id, format!("mock failure: {}", task.book_id));
        } else {
            jobs.set_done(task.handle.id);
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 轮询等待条件成立，超时返回 false。
fn wait_for(cond: impl Fn() -> bool, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    cond()
}

/// 构造一条队列任务（需先在 JobStore 中创建对应任务）。
fn make_task(jobs: &Arc<JobStore>, book_id: &str) -> QueueTask {
    let handle = jobs.create(book_id.to_string());
    QueueTask {
        handle,
        book_id: book_id.to_string(),
        config: crate::base_system::context::Config::default(),
        range_start: None,
        range_end: None,
    }
}

/// 启动一个带 Mock 执行器的队列并返回（已启动 Worker）。
fn queue_with(recorder: Arc<Recorder>, capacity: usize, delay_secs: u64) -> Arc<DownloadQueue> {
    let jobs = Arc::new(JobStore::default());
    let rec = recorder.clone();
    let executor: Arc<ExecuteFn> =
        Arc::new(move |jobs: &Arc<JobStore>, task: &QueueTask| rec.record(jobs, task));
    let queue = Arc::new(DownloadQueue::new(
        jobs, true, capacity, delay_secs, executor,
    ));
    queue.spawn_worker();
    queue
}

#[test]
fn env_flags_are_parsed() {
    assert!(!queue_enabled_from_env(None));
    assert!(!queue_enabled_from_env(Some("0".into())));
    assert!(!queue_enabled_from_env(Some("off".into())));
    assert!(!queue_enabled_from_env(Some("".into())));
    assert!(queue_enabled_from_env(Some("1".into())));
    assert!(queue_enabled_from_env(Some("true".into())));
    assert!(queue_enabled_from_env(Some("TRUE".into())));

    assert_eq!(delay_secs_from_env(None), DEFAULT_DELAY_SECS);
    assert_eq!(delay_secs_from_env(Some("5".into())), 5);
    assert_eq!(delay_secs_from_env(Some("0".into())), 0);
    assert_eq!(delay_secs_from_env(Some("999".into())), MAX_DELAY_SECS);
    assert_eq!(delay_secs_from_env(Some("abc".into())), DEFAULT_DELAY_SECS);
}

#[test]
fn tasks_execute_in_strict_fifo_order_with_single_worker() {
    let recorder = Arc::new(Recorder {
        sleep_ms: AtomicU64::new(5),
        ..Default::default()
    });
    let queue = queue_with(recorder.clone(), 10, 0);

    let jobs = queue.core.jobs.clone();
    for id in ["a", "b", "c", "d", "e"] {
        let task = make_task(&jobs, id);
        assert!(queue.push(task), "push should succeed");
    }

    assert!(wait_for(
        || recorder.order.lock().unwrap().len() >= 5,
        Duration::from_secs(10)
    ));

    // 严格按照 FIFO 顺序执行。
    assert_eq!(
        *recorder.order.lock().unwrap(),
        vec!["a", "b", "c", "d", "e"]
    );
    // 同时 Running 的任务数量始终不超过 1。
    assert_eq!(recorder.max_active.load(Ordering::SeqCst), 1);
    // 单个 Worker：只有一个线程执行过任务。
    let threads = recorder.threads.lock().unwrap();
    assert_eq!(threads.len(), 5);
    assert!(threads.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn queue_rejects_new_tasks_when_full_and_allows_after_cancel() {
    // 关闭队列（不启动 Worker），任务会停留在队列中，便于确定性测试容量。
    let jobs = Arc::new(JobStore::default());
    let executor: Arc<ExecuteFn> = Arc::new(|_jobs, _task| {});
    let queue = Arc::new(DownloadQueue::new(jobs.clone(), false, 2, 0, executor));

    let t1 = make_task(&jobs, "1");
    let t2 = make_task(&jobs, "2");
    assert!(queue.push(t1));
    assert!(queue.push(t2));
    assert_eq!(queue.waiting(), 2);

    // 队列满：拒绝（任务被丢弃，调用方负责清理 JobStore 条目）。
    let t3 = make_task(&jobs, "3");
    let t3_id = t3.handle.id;
    assert!(!queue.push(t3));
    jobs.remove(t3_id);
    assert_eq!(queue.waiting(), 2);
    assert_eq!(queue.capacity(), 2);

    // 取消（移除）一个等待任务后可以继续添加新任务。
    assert!(queue.cancel_waiting(2));
    assert_eq!(queue.waiting(), 1);
    let t4 = make_task(&jobs, "4");
    assert!(queue.push(t4));
    assert_eq!(queue.waiting(), 2);
    // FIFO 顺序保持：剩余为 1 → 4。
    assert_eq!(queue.waiting_order(), vec![1, 4]);
}

#[test]
fn canceled_waiting_task_is_skipped_and_others_still_run() {
    let recorder = Arc::new(Recorder::default());
    let queue = queue_with(recorder.clone(), 10, 0);
    let jobs = queue.core.jobs.clone();

    // 1) 先提交一个较慢的任务 a，确保 Worker 忙于 a 时再去操作 b。
    recorder.sleep_ms.store(500, Ordering::SeqCst);
    let task_a = make_task(&jobs, "a");
    assert!(queue.push(task_a), "push a should succeed");
    assert!(wait_for(
        || !recorder.order.lock().unwrap().is_empty(),
        Duration::from_secs(10)
    ));

    // 2) Worker 正在执行 a 时提交 b、c，并在 Worker 取出 b 之前取消它。
    let task_b = make_task(&jobs, "b");
    let b_id = task_b.handle.id;
    assert!(queue.push(task_b), "push b should succeed");
    let task_c = make_task(&jobs, "c");
    assert!(queue.push(task_c), "push c should succeed");
    assert!(queue.cancel_waiting(b_id));
    assert!(jobs.request_cancel_active(b_id));

    // 3) 恢复正常耗时，等待后续任务完成。
    recorder.sleep_ms.store(0, Ordering::SeqCst);
    assert!(wait_for(
        || recorder.order.lock().unwrap().len() >= 2,
        Duration::from_secs(10)
    ));

    // b 被跳过，不影响 a、c。
    assert_eq!(*recorder.order.lock().unwrap(), vec!["a", "c"]);
    // b 始终未被执行（order 中不包含 b）。
    assert!(!recorder.order.lock().unwrap().contains(&"b".to_string()));

    let list = jobs.list();
    let state_of = |id: u64| list.iter().find(|j| j.id == id).map(|j| j.state);
    assert!(matches!(state_of(b_id), Some(JobState::Canceled)));
    assert_eq!(recorder.max_active.load(Ordering::SeqCst), 1);
}

#[test]
fn failed_task_does_not_stop_the_queue() {
    let recorder = Arc::new(Recorder {
        fail_ids: vec!["bad".to_string()],
        ..Default::default()
    });
    let queue = queue_with(recorder.clone(), 10, 0);
    let jobs = queue.core.jobs.clone();

    let mut bad_id = 0;
    for id in ["bad", "next"] {
        let task = make_task(&jobs, id);
        if id == "bad" {
            bad_id = task.handle.id;
        }
        assert!(queue.push(task), "push should succeed");
    }

    assert!(wait_for(
        || recorder.order.lock().unwrap().len() >= 2,
        Duration::from_secs(10)
    ));

    assert_eq!(*recorder.order.lock().unwrap(), vec!["bad", "next"]);
    let list = jobs.list();
    let bad = list
        .iter()
        .find(|j| j.id == bad_id)
        .expect("bad job exists");
    assert!(matches!(bad.state, JobState::Failed));
    assert!(
        bad.message
            .as_deref()
            .unwrap_or_default()
            .contains("mock failure")
    );
}

#[test]
fn panicking_task_is_isolated_and_queue_continues() {
    let recorder = Arc::new(Recorder {
        panic_ids: vec!["boom".to_string()],
        ..Default::default()
    });
    let queue = queue_with(recorder.clone(), 10, 0);
    let jobs = queue.core.jobs.clone();

    let mut boom_id = 0;
    for id in ["boom", "after"] {
        let task = make_task(&jobs, id);
        if id == "boom" {
            boom_id = task.handle.id;
        }
        assert!(queue.push(task), "push should succeed");
    }

    assert!(wait_for(
        || recorder.order.lock().unwrap().len() >= 2,
        Duration::from_secs(10)
    ));

    // panic 的任务被隔离标记为 Failed，后续任务仍能启动。
    assert_eq!(*recorder.order.lock().unwrap(), vec!["boom", "after"]);
    let list = jobs.list();
    let boom = list
        .iter()
        .find(|j| j.id == boom_id)
        .expect("boom job exists");
    assert!(matches!(boom.state, JobState::Failed));
    assert!(
        boom.message
            .as_deref()
            .unwrap_or_default()
            .contains("panicked")
    );
}

#[test]
fn job_deleted_before_worker_claim_is_skipped() {
    let recorder = Arc::new(Recorder::default());
    let queue = queue_with(recorder.clone(), 10, 0);
    let jobs = queue.core.jobs.clone();

    // 不启动 Worker 的场景无法保证时序，这里直接验证 try_mark_running 的防线：
    // 删除任务后 Worker 的原子转换必然失败。
    let task = make_task(&jobs, "ghost");
    assert!(jobs.remove(task.handle.id));
    assert!(!jobs.try_mark_running(task.handle.id));
    assert!(recorder.order.lock().unwrap().is_empty());
}
