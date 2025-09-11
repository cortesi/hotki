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
    if let Some(rt) = SHARED_RUNTIME.get() {
        return Ok(rt.clone());
    }
    let rt = Runtime::new()
        .map_err(|e| Error::InvalidState(format!("Failed to create tokio runtime: {}", e)))?;
    let arc = Arc::new(Mutex::new(rt));
    // If another thread set it first, return that instance to keep a single runtime.
    if SHARED_RUNTIME.set(arc.clone()).is_err() {
        Ok(SHARED_RUNTIME.get().unwrap().clone())
    } else {
        Ok(arc)
    }
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
