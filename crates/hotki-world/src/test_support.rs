//! Minimal test support utilities for `hotki-world` consumers.

use std::{future::Future, sync::OnceLock, time::Duration};

use parking_lot::Mutex;
use tokio::runtime::Builder;

/// Drop guard that clears test overrides on scope exit.
pub struct TestOverridesGuard;

impl Drop for TestOverridesGuard {
    fn drop(&mut self) {
        // No global overrides in the simplified world; placeholder for API
        // stability.
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
