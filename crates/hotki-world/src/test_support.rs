//! Test support utilities for `hotki-world` tests.
//! Public, lightweight helpers imported by the test suite.

use std::time::Duration;

use tokio::sync::broadcast;

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
