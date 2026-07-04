//! Thin wrapper around the Tao event loop proxy used to wake the main thread.
//!
//! Posts typed user events to the main event loop when requested.

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tao::event_loop::EventLoopProxy;

/// Main-loop wake reason sent through Tao `UserEvent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeEvent {
    /// Shutdown state changed and the loop should re-check it.
    Shutdown,
    /// A client connected and any idle timer should be canceled.
    ClientConnected,
    /// The last observed client disconnected and idle shutdown can be armed.
    ClientDisconnected,
}

/// Errors that can occur while interacting with the main loop proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WakeError {
    /// No proxy has been registered yet.
    #[error("main loop proxy not set")]
    ProxyMissing,
    /// Posting the user event failed (usually because the loop is gone).
    #[error("failed to post user event to main loop")]
    SendFailed,
}

static MAIN_PROXY: Lazy<Mutex<Option<EventLoopProxy<WakeEvent>>>> = Lazy::new(|| Mutex::new(None));

/// Register the Tao main-thread proxy for later wake-ups.
pub fn set_main_proxy(proxy: EventLoopProxy<WakeEvent>) {
    *MAIN_PROXY.lock() = Some(proxy);
}

/// Post a typed wake event to the main event loop, if available.
pub fn post_user_event(event: WakeEvent) -> Result<(), WakeError> {
    let guard = MAIN_PROXY.lock();
    match &*guard {
        Some(p) => p.send_event(event).map_err(|_| WakeError::SendFailed),
        None => Err(WakeError::ProxyMissing),
    }
}
