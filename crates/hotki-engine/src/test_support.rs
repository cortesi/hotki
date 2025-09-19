//! Test support utilities for hotki-engine integration/unit tests.
//! These helpers are public to avoid dead_code warnings and are lightweight.
//! They are intended for use by the test suite only.

use std::{future::Future, time::Duration};

use hotki_protocol::MsgToUI;
use hotki_world::WorldView;
use tokio::time::{Instant, sleep};

/// Create a low-latency `hotki_world` configuration suitable for tests.
pub fn fast_world_cfg() -> hotki_world::WorldCfg {
    hotki_world::WorldCfg {
        poll_ms_min: 1,
        poll_ms_max: 10,
        ..hotki_world::WorldCfg::default()
    }
}

/// Run an asynchronous engine test body on a dedicated runtime with world overrides.
///
/// The helper disables the AX hint bridge (preventing long-lived threads from retaining the
/// world handle), enables accessibility/screen recording shims, and ensures the runtime shuts down
/// promptly once the test future completes.
pub fn run_engine_test<F>(fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    hotki_world::test_support::run_async_test(async move {
        let _guard = hotki_world::test_support::override_scope();
        hotki_world::test_api::set_ax_bridge_enabled(false);
        hotki_world::test_api::set_accessibility_ok(true);
        hotki_world::test_api::set_screen_recording_ok(true);
        hotki_world::test_api::set_displays(vec![(0, 0, 0, 1920, 1080)]);
        hotki_world::test_api::ensure_ax_pool_inited();
        hotki_world::test_api::ax_pool_reset_metrics_and_cache();
        fut.await;
    });
}

/// Await until the world snapshot satisfies `pred`, up to `timeout_ms`.
pub async fn wait_snapshot_until<F, W>(world: &W, timeout_ms: u64, mut pred: F) -> bool
where
    W: WorldView + ?Sized,
    F: FnMut(&[hotki_world::WorldWindow]) -> bool,
{
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let snapshot = world.snapshot().await;
        if pred(&snapshot) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(2)).await;
    }
}

/// Receive an error notification with a specific `title` within `timeout_ms`.
pub async fn recv_error_with_title(
    rx: &mut tokio::sync::mpsc::Receiver<MsgToUI>,
    title: &str,
    timeout_ms: u64,
) -> bool {
    let want = title.to_string();
    tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        while let Some(msg) = rx.recv().await {
            if let MsgToUI::Notify { kind, title, .. } = msg
                && matches!(kind, hotki_protocol::NotifyKind::Error)
                && title == want
            {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}

/// Receive UI messages until `pred` matches or `timeout_ms` elapses.
pub async fn recv_until<F>(
    rx: &mut tokio::sync::mpsc::Receiver<MsgToUI>,
    timeout_ms: u64,
    mut pred: F,
) -> bool
where
    F: FnMut(&MsgToUI) -> bool,
{
    tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        while let Some(msg) = rx.recv().await {
            if pred(&msg) {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}
