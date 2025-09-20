//! Test support utilities for `hotki-world` tests.
//! Public, lightweight helpers imported by the test suite.

use std::{collections::HashMap, future::Future, sync::OnceLock, time::Duration};

use parking_lot::Mutex;
use tokio::{runtime::Builder, sync::broadcast, time::Instant};

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

static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Run an async test body on a dedicated multi-threaded Tokio runtime and shut it down promptly.
pub fn run_async_test<F>(fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    let _guard = TEST_LOCK.get_or_init(|| Mutex::new(())).lock();
    struct RuntimeGuard(Option<tokio::runtime::Runtime>);

    impl Drop for RuntimeGuard {
        fn drop(&mut self) {
            if let Some(rt) = self.0.take() {
                rt.shutdown_timeout(Duration::from_millis(50));
            }
        }
    }

    let guard = RuntimeGuard(Some(
        Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build test runtime"),
    ));

    if let Some(rt) = guard.0.as_ref() {
        rt.block_on(fut);
    }
    // RuntimeGuard drops here, enforcing an eager shutdown of the runtime.
}

/// Synchronous harness for world tests that need deterministic runtime control.
pub struct TestHarness {
    _guard: parking_lot::MutexGuard<'static, ()>,
    runtime: Option<tokio::runtime::Runtime>,
}

impl Default for TestHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl TestHarness {
    /// Create a new harness with a dedicated multi-threaded runtime.
    pub fn new() -> Self {
        let guard = TEST_LOCK.get_or_init(|| Mutex::new(())).lock();
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build test runtime");
        Self {
            _guard: guard,
            runtime: Some(runtime),
        }
    }

    /// Run an async future to completion on the harness runtime.
    pub fn block_on<F, T>(&self, fut: F) -> T
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        self.runtime
            .as_ref()
            .expect("harness runtime available")
            .block_on(fut)
    }

    /// Pump main-thread operations until either the queue drains or the timeout expires.
    pub fn pump_main_until(&self, world: &crate::WorldHandle, timeout_ms: u64) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        world.pump_main_until(deadline)
    }
}

impl Drop for TestHarness {
    fn drop(&mut self) {
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_timeout(Duration::from_millis(50));
        }
    }
}

/// Re-export the canonical world snapshot wait helper.
pub use crate::test_api::wait_snapshot_until;

/// Await until the frames snapshot satisfies `pred`, up to `timeout_ms` milliseconds.
/// Returns `true` if the predicate matched before timing out.
pub async fn wait_frames_until<F>(world: &crate::WorldHandle, timeout_ms: u64, mut pred: F) -> bool
where
    F: FnMut(&HashMap<crate::WindowKey, crate::Frames>) -> bool,
{
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let frames = world.frames_snapshot().await;
        if pred(&frames) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

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
        let metrics = world.metrics_snapshot();
        if metrics.debounce_pending == expected {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Poll world metrics until `pred` evaluates to true or `timeout_ms` elapses.
/// Returns the matching metrics snapshot when successful.
pub async fn wait_metrics_until<F>(
    world: &crate::WorldHandle,
    timeout_ms: u64,
    mut pred: F,
) -> Option<crate::WorldMetricsSnapshot>
where
    F: FnMut(&crate::WorldMetricsSnapshot) -> bool,
{
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let snapshot = world.metrics_snapshot();
        if pred(&snapshot) {
            return Some(snapshot);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
