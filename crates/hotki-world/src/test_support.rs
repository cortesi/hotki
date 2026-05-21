//! Minimal test support utilities for `hotki-world` consumers.

use std::{future::Future, sync::OnceLock, time::Duration};

use parking_lot::Mutex;
use tokio::runtime::Builder;

static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Run an async test body on a dedicated multi-threaded Tokio runtime and shut it down promptly.
pub fn run_async_test<F>(fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    run_async_test_with_time(fut, false);
}

/// Run an async test body on a dedicated runtime with Tokio time paused.
pub fn run_async_test_paused<F>(fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    run_async_test_with_time(fut, true);
}

fn run_async_test_with_time<F>(fut: F, start_paused: bool)
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

    let mut builder = Builder::new_current_thread();
    builder.enable_all();
    builder.start_paused(start_paused);
    let guard = RuntimeGuard(Some(builder.build().expect("build test runtime")));

    if let Some(rt) = guard.0.as_ref() {
        rt.block_on(fut);
    }
    // RuntimeGuard drops here, enforcing an eager shutdown of the runtime.
}
