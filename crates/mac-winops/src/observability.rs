use std::sync::atomic::{AtomicUsize, Ordering};

use once_cell::sync::Lazy;
use tracing::info;

static FOCUSED_FALLBACKS: Lazy<AtomicUsize> = Lazy::new(|| AtomicUsize::new(0));

pub(crate) fn record_focused_fallback(op: &'static str, pid: i32) {
    let count = FOCUSED_FALLBACKS.fetch_add(1, Ordering::SeqCst) + 1;
    info!(
        target = "mac_winops::fallback",
        operation = op,
        pid,
        count,
        "Focused placement fallback invoked without explicit world window id"
    );
}

pub fn focused_fallback_count() -> usize {
    FOCUSED_FALLBACKS.load(Ordering::SeqCst)
}

pub fn reset_focused_fallback_count() {
    FOCUSED_FALLBACKS.store(0, Ordering::SeqCst);
}
