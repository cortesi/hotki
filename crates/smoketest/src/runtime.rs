//! Shared async runtime management for tests.

use std::sync::{Arc, Mutex, OnceLock};
use tokio::runtime::Runtime;

use crate::error::{Error, Result};

/// Global shared runtime instance
static SHARED_RUNTIME: OnceLock<Arc<Mutex<Runtime>>> = OnceLock::new();

/// Get or create the shared runtime.
///
/// This ensures we use a single runtime instance across all tests,
/// avoiding the overhead of creating multiple runtimes.
pub fn shared_runtime() -> Result<Arc<Mutex<Runtime>>> {
    Ok(SHARED_RUNTIME
        .get_or_init(|| {
            let rt = Runtime::new().expect("Failed to create tokio runtime");
            Arc::new(Mutex::new(rt))
        })
        .clone())
}

/// Execute an async function on the shared runtime.
///
/// This is a convenience function that gets the shared runtime
/// and blocks on the provided future.
pub fn block_on<F, T>(fut: F) -> Result<T>
where
    F: std::future::Future<Output = T>,
{
    let rt = shared_runtime()?;
    let runtime = rt
        .lock()
        .map_err(|_| Error::InvalidState("Runtime lock poisoned".into()))?;
    Ok(runtime.block_on(fut))
}
