use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::base_system::context::Config;
use crate::download::downloader::{BookNameOption, ProgressSnapshot};

#[derive(Clone, Debug)]
pub(crate) struct ConfigView {
    pub(crate) old_cli: bool,
    pub(crate) use_official_api: bool,
    pub(crate) save_path: String,
    pub(crate) api_endpoints_len: usize,
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) bind_addrs: Arc<Vec<SocketAddr>>,
    pub(crate) config_view: Arc<ConfigView>,
    pub(crate) config: Arc<Mutex<Config>>, // allow runtime updates via Web UI
    pub(crate) library_root: Arc<PathBuf>,
    pub(crate) jobs: Arc<JobStore>,
    pub(crate) auth: Option<AuthState>,
}

#[derive(Clone)]
pub(crate) struct AuthState {
    pub(crate) password_sha256: [u8; 32],
    pub(crate) session_secret: [u8; 32],
}

const SESSION_TTL_SECS: u64 = 7 * 24 * 60 * 60;

impl AuthState {
    pub(crate) fn from_password(password: &str) -> Self {
        let mut h = Sha256::new();
        h.update(password.as_bytes());
        let out = h.finalize();
        let mut password_sha256 = [0u8; 32];
        password_sha256.copy_from_slice(&out);

        let nonce = Uuid::new_v4();
        let now = now_secs();
        let mut s = Sha256::new();
        s.update(password_sha256);
        s.update(now.to_le_bytes());
        s.update(nonce.as_bytes());
        let secret = s.finalize();
        let mut session_secret = [0u8; 32];
        session_secret.copy_from_slice(&secret);

        Self {
            password_sha256,
            session_secret,
        }
    }

    pub(crate) fn issue_session_token(&self) -> String {
        let exp = now_secs().saturating_add(SESSION_TTL_SECS);
        let nonce = Uuid::new_v4().simple().to_string();
        let payload = format!("{exp}.{nonce}");
        let sig = self.sign_payload(&payload);
        format!("{payload}.{sig}")
    }

    pub(crate) fn verify_session_token(&self, token: &str) -> bool {
        let mut parts = token.split('.');
        let Some(exp_raw) = parts.next() else {
            return false;
        };
        let Some(nonce_raw) = parts.next() else {
            return false;
        };
        let Some(sig_raw) = parts.next() else {
            return false;
        };
        if parts.next().is_some() {
            return false;
        }

        let Ok(exp) = exp_raw.parse::<u64>() else {
            return false;
        };
        if now_secs() > exp {
            return false;
        }

        let payload = format!("{exp_raw}.{nonce_raw}");
        let expected = self.sign_payload(&payload);
        constant_time_eq(sig_raw.as_bytes(), expected.as_bytes())
    }

    pub(crate) fn session_ttl_secs(&self) -> u64 {
        SESSION_TTL_SECS
    }

    fn sign_payload(&self, payload: &str) -> String {
        let mut h = Sha256::new();
        h.update(self.session_secret);
        h.update(payload.as_bytes());
        hex::encode(h.finalize())
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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
    pub(crate) book_name_options: Option<Vec<BookNameOption>>,
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
    book_name_sender: Option<std::sync::mpsc::Sender<Option<String>>>,
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
            book_name_options: None,
            created_ms: now,
            updated_ms: now,
        };

        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.insert(
            id,
            JobEntry {
                info,
                cancel: cancel.clone(),
                book_name_sender: None,
            },
        );

        JobHandle { id, cancel }
    }

    pub(crate) fn list(&self) -> Vec<JobInfo> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut v: Vec<JobInfo> = g.values().map(|e| e.info.clone()).collect();
        v.sort_by(|a, b| {
            b.updated_ms
                .cmp(&a.updated_ms)
                .then_with(|| b.id.cmp(&a.id))
        });
        v
    }

    pub(crate) fn set_running(&self, id: u64) {
        self.update(id, |j| {
            j.state = JobState::Running;
            j.message = None;
            j.book_name_options = None;
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
            j.book_name_options = None;
        });
    }

    pub(crate) fn set_failed(&self, id: u64, msg: String) {
        self.update(id, |j| {
            j.state = JobState::Failed;
            j.message = Some(msg);
            j.book_name_options = None;
        });
    }

    pub(crate) fn request_cancel(&self, id: u64) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = g.get_mut(&id) else {
            return false;
        };
        e.cancel.store(true, Ordering::Relaxed);
        e.info.state = JobState::Canceled;
        e.info.message = Some("cancel requested".to_string());
        if let Some(tx) = e.book_name_sender.take() {
            let _ = tx.send(None);
        }
        e.info.book_name_options = None;
        e.info.updated_ms = now_ms();
        true
    }

    pub(crate) fn request_cancel_and_remove(&self, id: u64) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut e) = g.remove(&id) else {
            return false;
        };
        e.cancel.store(true, Ordering::Relaxed);
        if let Some(tx) = e.book_name_sender.take() {
            let _ = tx.send(None);
        }
        true
    }

    pub(crate) fn remove(&self, id: u64) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut e) = g.remove(&id) else {
            return false;
        };
        if let Some(tx) = e.book_name_sender.take() {
            let _ = tx.send(None);
        }
        true
    }

    pub(crate) fn set_book_name_options(
        &self,
        id: u64,
        options: Vec<BookNameOption>,
        sender: std::sync::mpsc::Sender<Option<String>>,
    ) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = g.get_mut(&id) else {
            return;
        };
        e.info.book_name_options = Some(options);
        e.info.message = Some("等待选择书名".to_string());
        e.book_name_sender = Some(sender);
        e.info.updated_ms = now_ms();
    }

    pub(crate) fn submit_book_name_choice(&self, id: u64, choice: Option<String>) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = g.get_mut(&id) else {
            return false;
        };
        if let Some(tx) = e.book_name_sender.take() {
            let _ = tx.send(choice);
            e.info.book_name_options = None;
            e.info.message = None;
            e.info.updated_ms = now_ms();
            return true;
        }
        false
    }

    fn update<F: FnOnce(&mut JobInfo)>(&self, id: u64, f: F) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
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

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
