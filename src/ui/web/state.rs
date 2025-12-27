use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::base_system::context::Config;
use crate::download::downloader::ProgressSnapshot;

#[derive(Clone, Debug)]
pub(crate) struct ConfigView {
    pub(crate) old_cli: bool,
    pub(crate) use_official_api: bool,
    pub(crate) save_path: String,
    pub(crate) api_endpoints_len: usize,
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) bind: SocketAddr,
    pub(crate) config_view: Arc<ConfigView>,
    pub(crate) config: Arc<Mutex<Config>>, // allow runtime updates via Web UI
    pub(crate) library_root: Arc<PathBuf>,
    pub(crate) jobs: Arc<JobStore>,
    pub(crate) auth: Option<AuthState>,
}

#[derive(Clone)]
pub(crate) struct AuthState {
    pub(crate) password_sha256: [u8; 32],
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct JobInfo {
    pub(crate) id: u64,
    pub(crate) book_id: String,
    pub(crate) title: Option<String>,
    pub(crate) author: Option<String>,
    pub(crate) state: JobState,
    pub(crate) progress: Option<ProgressSnapshot>,
    pub(crate) message: Option<String>,
    pub(crate) created_ms: u64,
    pub(crate) updated_ms: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct JobHandle {
    pub(crate) id: u64,
    pub(crate) cancel: Arc<AtomicBool>,
}

#[derive(Debug)]
struct JobEntry {
    info: JobInfo,
    cancel: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
pub(crate) struct JobStore {
    next_id: AtomicU64,
    inner: Mutex<HashMap<u64, JobEntry>>,
}

impl JobStore {
    pub(crate) fn create(&self, book_id: String) -> JobHandle {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let now = now_ms();
        let cancel = Arc::new(AtomicBool::new(false));

        let info = JobInfo {
            id,
            book_id,
            title: None,
            author: None,
            state: JobState::Queued,
            progress: None,
            message: None,
            created_ms: now,
            updated_ms: now,
        };

        let mut g = self.inner.lock().unwrap();
        g.insert(
            id,
            JobEntry {
                info,
                cancel: cancel.clone(),
            },
        );

        JobHandle { id, cancel }
    }

    pub(crate) fn list(&self) -> Vec<JobInfo> {
        let g = self.inner.lock().unwrap();
        let mut v: Vec<JobInfo> = g.values().map(|e| e.info.clone()).collect();
        v.sort_by(|a, b| {
            b.updated_ms
                .cmp(&a.updated_ms)
                .then_with(|| b.id.cmp(&a.id))
        });
        v
    }

    #[allow(dead_code)]
    pub(crate) fn get_handle(&self, id: u64) -> Option<JobHandle> {
        let g = self.inner.lock().unwrap();
        g.get(&id).map(|e| JobHandle {
            id,
            cancel: e.cancel.clone(),
        })
    }

    pub(crate) fn set_running(&self, id: u64) {
        self.update(id, |j| {
            j.state = JobState::Running;
            j.message = None;
        });
    }

    pub(crate) fn set_meta(&self, id: u64, title: Option<String>, author: Option<String>) {
        self.update(id, |j| {
            j.title = title;
            j.author = author;
        });
    }

    pub(crate) fn set_progress(&self, id: u64, snap: ProgressSnapshot) {
        self.update(id, |j| {
            j.progress = Some(snap);
        });
    }

    pub(crate) fn set_done(&self, id: u64) {
        self.update(id, |j| {
            j.state = JobState::Done;
            j.message = None;
        });
    }

    pub(crate) fn set_failed(&self, id: u64, msg: String) {
        self.update(id, |j| {
            j.state = JobState::Failed;
            j.message = Some(msg);
        });
    }

    pub(crate) fn request_cancel(&self, id: u64) -> bool {
        let mut g = self.inner.lock().unwrap();
        let Some(e) = g.get_mut(&id) else {
            return false;
        };
        e.cancel.store(true, Ordering::Relaxed);
        e.info.state = JobState::Canceled;
        e.info.message = Some("cancel requested".to_string());
        e.info.updated_ms = now_ms();
        true
    }

    fn update<F: FnOnce(&mut JobInfo)>(&self, id: u64, f: F) {
        let mut g = self.inner.lock().unwrap();
        let Some(e) = g.get_mut(&id) else {
            return;
        };
        f(&mut e.info);
        e.info.updated_ms = now_ms();
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
