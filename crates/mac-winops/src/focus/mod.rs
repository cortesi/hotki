//! Focus watcher: observe focused app and window title changes on macOS.
//!
//! - Combines CGWindowList polling, Accessibility (AX) observer, and
//!   NSWorkspace notifications.
//! - Exposed from `mac-winops` to avoid a separate crate.

mod ax;
mod cg;
mod event;
mod ns;
mod watcher;

pub use event::FocusEvent;
pub use ns::{install_ns_workspace_observer, post_user_event, set_main_proxy};

use thiserror::Error;
use tokio::sync::mpsc::UnboundedSender;

/// Errors that can occur when interacting with focus watcher public APIs.
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

/// Starts the focus watcher end-to-end.
///
/// - Registers the sink for [`FocusEvent`]s (both NS and CG/AX).
/// - Posts a Tao `UserEvent(())` via the proxy set by [`set_main_proxy`],
///   requesting installation of the NSWorkspace observer on the main thread.
/// - Spawns the background watcher thread which emits [`FocusEvent`]s to `tx`.
pub fn start_watcher(tx: UnboundedSender<FocusEvent>) -> Result<(), Error> {
    ns::set_ns_sink(tx.clone());
    ns::request_ns_observer_install()?;
    watcher::start_watcher(tx);
    Ok(())
}
