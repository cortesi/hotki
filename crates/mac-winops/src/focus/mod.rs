//! Focus watcher: observe foreground application and window title changes.
//!
//! Combines:
//! - CoreGraphics CGWindowList polling for bootstrap/fallback
//! - Accessibility (AX) observer attached to the active app for real-time title changes
//! - NSWorkspace activation notifications on the main thread

mod ax;
mod event;
mod ns;
mod watcher;

pub use event::FocusEvent;
pub use ns::{install_ns_workspace_observer, set_main_proxy, wake_main_loop};

use thiserror::Error;
use tokio::sync::mpsc::UnboundedSender;

/// Errors from focus watcher public APIs.
#[derive(Debug, Error)]
pub enum Error {
    #[error("NS main proxy not set; call set_main_proxy() on the main thread first")]
    MainProxyNotSet,
    #[error("NS main proxy mutex poisoned")]
    MainProxyPoisoned,
    #[error("Failed to post install request to main thread")]
    PostEventFailed,
    #[error("NS observer state mutex poisoned")]
    NsObserverPoisoned,
}

/// Start the focus watcher: register sink, request NS install, spawn CG/AX thread.
pub fn start_watcher(tx: UnboundedSender<FocusEvent>) -> Result<(), Error> {
    ns::set_ns_sink(tx.clone());
    ns::request_ns_observer_install()?;
    watcher::start_watcher(tx);
    Ok(())
}
