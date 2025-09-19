//! Test support utilities for `hotki-world` tests.
//! Public, lightweight helpers imported by the test suite.

use std::time::Duration;

use tokio::{sync::broadcast, time::Instant};

/// Drop guard that clears any test overrides on scope exit.
pub struct TestOverridesGuard;

impl Drop for TestOverridesGuard {
    fn drop(&mut self) {
        crate::test_api::clear();
    }
}

/// Create a guard that resets world overrides when dropped.
#[must_use]
pub fn override_scope() -> TestOverridesGuard {
    TestOverridesGuard
}

/// Re-export the canonical world snapshot wait helper.
pub use crate::test_api::wait_snapshot_until;

/// Drain any immediately available world events from a broadcast receiver.
pub fn drain_events(rx: &mut broadcast::Receiver<crate::WorldEvent>) {
    while rx.try_recv().is_ok() {}
}

/// Receive events until `pred` matches or `timeout_ms` elapses.
pub async fn recv_event_until<F>(
    rx: &mut broadcast::Receiver<crate::WorldEvent>,
    timeout_ms: u64,
    mut pred: F,
) -> Option<crate::WorldEvent>
where
    F: FnMut(&crate::WorldEvent) -> bool,
{
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Ok(ev)) => {
                if pred(&ev) {
                    return Some(ev);
                }
            }
            Ok(Err(_)) => return None,
            Err(_) => return None,
        }
    }
}

/// Wait until the world's debounce queue matches `expected` pending entries.
/// Returns false if the condition was not met before `timeout_ms` elapsed.
pub async fn wait_debounce_pending(
    world: &crate::WorldHandle,
    expected: usize,
    timeout_ms: u64,
) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let status = world.status().await;
        if status.debounce_pending == expected {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
