//! Thin wrapper around the Tao event loop proxy used to wake the main thread.
//!
//! This replaces the old window-ops focus helper with a local, crate-scoped
//! waker that simply posts a `UserEvent(())` when requested. The observer hook
//! is retained as a no-op for call-site compatibility.

use std::fmt;

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tao::event_loop::EventLoopProxy;

/// Errors that can occur while interacting with the main loop proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeError {
    /// No proxy has been registered yet.
    ProxyMissing,
    /// Posting the user event failed (usually because the loop is gone).
    SendFailed,
}

impl fmt::Display for WakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProxyMissing => write!(f, "main loop proxy not set"),
            Self::SendFailed => write!(f, "failed to post user event to main loop"),
        }
    }
}

impl std::error::Error for WakeError {}

static MAIN_PROXY: Lazy<Mutex<Option<EventLoopProxy<()>>>> = Lazy::new(|| Mutex::new(None));

/// Register the Tao main-thread proxy for later wake-ups.
pub fn set_main_proxy(proxy: EventLoopProxy<()>) {
    *MAIN_PROXY.lock() = Some(proxy);
}

/// Post a `UserEvent(())` to the main event loop, if available.
pub fn post_user_event() -> Result<(), WakeError> {
    let guard = MAIN_PROXY.lock();
    match &*guard {
        Some(p) => p.send_event(()).map_err(|_| WakeError::SendFailed),
        None => Err(WakeError::ProxyMissing),
    }
}

/// Placeholder for the old NSWorkspace observer install.
pub fn install_ns_workspace_observer() -> Result<(), WakeError> {
    Ok(())
}
