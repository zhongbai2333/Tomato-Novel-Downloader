//! WebUI 本地 FIFO 下载队列（默认关闭，通过 `TOMATO_WEB_DOWNLOAD_QUEUE=1` 开启）。
//!
//! 架构说明：
//! - 真正的任务队列是一个有界 `Mutex<VecDeque<QueueTask>>`（容量 = 等待任务上限），
//!   容量检查与入队在同一个锁内完成，保证并发下不会超限。
//! - `std::sync::mpsc::sync_channel(1)` 作为有界唤醒信号通道：入队后发送一个
//!   `()` 信号唤醒唯一的 Worker；信号已满时直接忽略（说明 Worker 即将被唤醒，
//!   它会一次性取空任务队列）。
//! - 全局只有一个 Worker 线程（服务启动时创建），任何时刻最多执行一本小说，
//!   严格按照 FIFO 顺序执行，不存在“每个任务一个线程”的路径。
//! - Worker 不持有唤醒信号发送端：服务关闭时队列被 drop，发送端随之断开，
//!   Worker 的 `recv()` 返回错误后自然退出，不会泄漏线程或资源。
//! - 取消等待中的任务：直接从 `VecDeque` 中移除该任务，不会在通道中留下
//!   占位的僵尸消息。
//! - 单个任务 panic / 失败不会终止 Worker：使用 `catch_unwind` 隔离，
//!   任务标记为 Failed 后自动继续下一项。
//! - 任务执行函数通过 [`TaskExecutor`] 抽象注入，便于测试时使用 Mock，
//!   自动化测试不会请求番茄小说上游接口。

use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tracing::{error, info, warn};

use crate::base_system::context::Config;
use crate::ui::web::state::{JobHandle, JobStore};

/// 队列开关环境变量。
pub(crate) const ENV_QUEUE_ENABLED: &str = "TOMATO_WEB_DOWNLOAD_QUEUE";
/// 任务间隔环境变量。
pub(crate) const ENV_QUEUE_DELAY_SECS: &str = "TOMATO_WEB_QUEUE_DELAY_SECS";
/// 默认最大等待任务数（正在执行的任务不计入等待容量）。
pub(crate) const DEFAULT_QUEUE_CAPACITY: usize = 20;
/// 任务间隔默认值（秒）。
pub(crate) const DEFAULT_DELAY_SECS: u64 = 2;
/// 任务间隔允许的最大值（秒）。
pub(crate) const MAX_DELAY_SECS: u64 = 60;

/// 解析队列开关：`1` / `true`（忽略大小写）视为开启，其余（含未设置）关闭。
pub(crate) fn queue_enabled_from_env(raw: Option<String>) -> bool {
    matches!(
        raw.as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("1") | Some("true")
    )
}

/// 解析任务间隔（秒），范围 0..=60，非法输入回落到默认值。
pub(crate) fn delay_secs_from_env(raw: Option<String>) -> u64 {
    match raw.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => s
            .parse::<u64>()
            .map(|v| v.min(MAX_DELAY_SECS))
            .unwrap_or(DEFAULT_DELAY_SECS),
        _ => DEFAULT_DELAY_SECS,
    }
}

/// 一条等待执行的下载任务（提交时的配置快照）。
#[derive(Debug)]
pub(crate) struct QueueTask {
    pub(crate) handle: JobHandle,
    pub(crate) book_id: String,
    pub(crate) config: Config,
    pub(crate) range_start: Option<usize>,
    pub(crate) range_end: Option<usize>,
}

/// 任务执行器抽象：生产环境调用真实下载流程，测试环境使用 Mock。
pub(crate) trait TaskExecutor: Send + Sync {
    /// 执行单个任务。实现方负责状态回写（Running / Done / Failed / Canceled）。
    fn execute(&self, jobs: &Arc<JobStore>, task: &QueueTask);
}

/// 生产环境执行器：调用 `routes::jobs::execute_download_job` 中的真实下载流程。
#[derive(Debug, Default)]
pub(crate) struct RealTaskExecutor;

impl TaskExecutor for RealTaskExecutor {
    fn execute(&self, jobs: &Arc<JobStore>, task: &QueueTask) {
        crate::ui::web::routes::jobs::execute_download_job(
            jobs,
            &task.handle,
            &task.book_id,
            &task.config,
            task.range_start,
            task.range_end,
        );
    }
}

type ExecuteFn = dyn Fn(&Arc<JobStore>, &QueueTask) + Send + Sync;
pub(crate) type SharedExecutor = Arc<ExecuteFn>;

#[derive(Debug)]
struct QueueInner {
    tasks: VecDeque<QueueTask>,
    capacity: usize,
}

/// Worker 与队列共享的核心状态（不包含唤醒信号发送端）。
struct QueueCore {
    jobs: Arc<JobStore>,
    inner: Mutex<QueueInner>,
    executor: Arc<ExecuteFn>,
    delay: Duration,
}

/// WebUI 本地 FIFO 下载队列。
pub(crate) struct DownloadQueue {
    core: Arc<QueueCore>,
    /// 有界唤醒信号通道发送端（容量 1，见模块注释）。
    wake_tx: std::sync::mpsc::SyncSender<()>,
    /// 唤醒信号接收端，仅在启动 Worker 时取出一次。
    wake_rx: Mutex<Option<Receiver<()>>>,
    enabled: bool,
}

impl DownloadQueue {
    /// 创建队列。`enabled = false` 时不启动 Worker，保持旧行为（单任务限制 + 429）。
    pub(crate) fn new(
        jobs: Arc<JobStore>,
        enabled: bool,
        capacity: usize,
        delay_secs: u64,
        executor: Arc<ExecuteFn>,
    ) -> Self {
        let (wake_tx, wake_rx) = sync_channel::<()>(1);
        let delay_secs = delay_secs.min(MAX_DELAY_SECS);
        let capacity = capacity.max(1);
        Self {
            core: Arc::new(QueueCore {
                jobs,
                inner: Mutex::new(QueueInner {
                    tasks: VecDeque::new(),
                    capacity,
                }),
                executor,
                delay: Duration::from_secs(delay_secs),
            }),
            wake_tx,
            wake_rx: Mutex::new(Some(wake_rx)),
            enabled,
        }
    }

    /// 创建生产环境队列（读取环境变量；开启时由 `spawn_worker` 启动唯一 Worker）。
    pub(crate) fn from_env(jobs: Arc<JobStore>) -> Self {
        let enabled = queue_enabled_from_env(std::env::var(ENV_QUEUE_ENABLED).ok());
        let delay_secs = delay_secs_from_env(std::env::var(ENV_QUEUE_DELAY_SECS).ok());
        let queue = Self::new(
            jobs,
            enabled,
            DEFAULT_QUEUE_CAPACITY,
            delay_secs,
            Arc::new(real_executor),
        );
        if enabled {
            info!(
                target: "web",
                capacity = queue.capacity(),
                delay_secs = queue.delay_secs(),
                "download queue enabled (single worker, FIFO)"
            );
        }
        queue
    }

    /// 启动唯一的 Worker 线程（仅在队列启用时有效）。
    /// 队列被 drop（服务关闭）后发送端断开，Worker 自动退出。
    pub(crate) fn spawn_worker(self: &Arc<Self>) {
        if !self.enabled {
            return;
        }
        let wake_rx = self
            .wake_rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let Some(wake_rx) = wake_rx else {
            warn!(target: "web", "download queue worker already spawned");
            return;
        };
        let core = self.core.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("web-download-queue-worker".to_string())
            .spawn(move || worker_loop(core, wake_rx))
        {
            error!(target: "web", error = %e, "failed to spawn download queue worker");
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    /// 等待队列容量上限（仅统计等待任务，不含正在执行的任务）。
    pub(crate) fn capacity(&self) -> usize {
        self.core
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .capacity
    }

    pub(crate) fn delay_secs(&self) -> u64 {
        self.core.delay.as_secs()
    }

    /// 当前等待中的任务数量。
    pub(crate) fn waiting(&self) -> usize {
        self.core
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tasks
            .len()
    }

    /// 当前等待任务的 FIFO 顺序（job id 列表），用于排队位置展示。
    pub(crate) fn waiting_order(&self) -> Vec<u64> {
        let g = self.core.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.tasks.iter().map(|t| t.handle.id).collect()
    }

    /// 尝试入队。返回 `false` 表示队列已满（任务被丢弃，
    /// 调用方需用 `task.handle.id` 清理对应的 JobStore 条目）。
    /// 容量检查与入队在同一个锁内完成，保证并发下不超限。
    pub(crate) fn push(&self, task: QueueTask) -> bool {
        let should_wake = {
            let mut g = self.core.inner.lock().unwrap_or_else(|e| e.into_inner());
            if g.tasks.len() >= g.capacity {
                return false;
            }
            g.tasks.push_back(task);
            true
        };
        if should_wake {
            // 唤醒信号：容量 1，若已满说明 Worker 马上会醒来取空队列，忽略即可。
            match self.wake_tx.try_send(()) {
                Ok(()) => {}
                Err(TrySendError::Full(())) => {}
                Err(TrySendError::Disconnected(())) => {
                    // Worker 已退出（服务正在关闭），任务不会再被执行。
                    warn!(target: "web", "queue worker channel closed; task will not run");
                }
            }
        }
        true
    }

    /// 取消一个仍在等待中的任务：从队列移除并返回 true。
    /// 若任务不在等待队列（已开始执行或不存在），返回 false。
    pub(crate) fn cancel_waiting(&self, id: u64) -> bool {
        let mut g = self.core.inner.lock().unwrap_or_else(|e| e.into_inner());
        let before = g.tasks.len();
        g.tasks.retain(|t| t.handle.id != id);
        g.tasks.len() != before
    }
}

impl Drop for DownloadQueue {
    fn drop(&mut self) {
        // 服务关闭：清空残留任务；发送端随字段 drop 断开，Worker 自然退出。
        if let Ok(mut g) = self.core.inner.lock() {
            g.tasks.clear();
        }
    }
}

/// 生产环境执行函数（包一层以满足 Arc<ExecuteFn> 的类型要求）。
fn real_executor(jobs: &Arc<JobStore>, task: &QueueTask) {
    RealTaskExecutor.execute(jobs, task);
}

/// 唯一的 Worker 主循环：等待唤醒 → FIFO 取任务 → 原子地 Queued→Running → 执行。
fn worker_loop(core: Arc<QueueCore>, wake_rx: Receiver<()>) {
    info!(target: "web", "download queue worker started");
    loop {
        // 等待唤醒信号；发送端全部关闭（服务退出）时结束 Worker。
        if wake_rx.recv().is_err() {
            info!(target: "web", "download queue worker stopped");
            return;
        }

        // 一次性取空当前积压的任务，逐个顺序执行。
        while let Some(task) = pop_task(&core) {
            run_task(&core, task);

            // 任务间隔：降低连续任务之间的请求突发。不持有任何锁。
            if core.delay.as_secs() > 0 {
                std::thread::sleep(core.delay);
            }
        }
    }
}

fn pop_task(core: &QueueCore) -> Option<QueueTask> {
    let mut g = core.inner.lock().unwrap_or_else(|e| e.into_inner());
    g.tasks.pop_front()
}

/// 执行单个任务（含 panic 隔离与状态回写）。
fn run_task(core: &QueueCore, task: QueueTask) {
    let QueueTask {
        handle, book_id, ..
    } = &task;
    let jobs = core.jobs.clone();

    // 取消/删除竞态防护：仅允许 Queued → Running 的原子转换。
    // 已取消或已删除的任务在这里被跳过，不会重新变成 Running。
    if !jobs.try_mark_running(handle.id) {
        info!(target: "web", job_id = handle.id, book_id = %book_id, "skip queued task (canceled or removed)");
        return;
    }

    let executor = core.executor.clone();
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| executor(&jobs, &task)));

    if let Err(payload) = result {
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };
        error!(target: "web", job_id = handle.id, book_id = %book_id, "download task panicked: {msg}");
        if handle.cancel.load(Ordering::Relaxed) {
            let _ = jobs.request_cancel(handle.id);
        } else {
            jobs.set_failed(handle.id, format!("download task panicked: {msg}"));
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
