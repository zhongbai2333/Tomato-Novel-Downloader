use std::sync::atomic::{AtomicBool, Ordering};

static PREWARMING: AtomicBool = AtomicBool::new(false);

pub fn mark_prewarm_start() {
    PREWARMING.store(true, Ordering::SeqCst);
}

pub fn mark_prewarm_done() {
    PREWARMING.store(false, Ordering::SeqCst);
}

pub fn is_prewarm_in_progress() -> bool {
    PREWARMING.load(Ordering::SeqCst)
}
