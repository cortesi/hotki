//! mac-focus-watcher: observe focused app and window title changes on macOS.
//!
//! This crate provides a focused watcher for foreground application and window
//! title changes by combining three sources:
//! - CoreGraphics `CGWindowList` polling for bootstrap and fallback.
//! - Accessibility (AX) observer attached to the active app for real-time
//!   focused window/title updates.
//! - NSWorkspace activation notifications on the main thread.
//!
//! Integration overview (no code):
//! - Call `set_main_proxy` exactly once on the Tao main thread after creating
//!   the event loop; this allows cross-thread requests to install the
//!   NSWorkspace observer.
//! - When your app is ready to start focus tracking (e.g., after handshake),
//!   call `start_watcher(tx)` from any thread. This will:
//!   - Register `tx` as the sink for [`FocusEvent`]s emitted by both the
//!     NSWorkspace callback and the background CG/AX watcher thread.
//!   - Post a request to the Tao event loop to install the NSWorkspace observer
//!     on the main thread.
//!   - Spawn a background thread that polls CGWindowList and attaches an AX
//!     observer for real-time title updates (if Accessibility permission is
//!     granted).
//! - In the Tao event loop, handle the posted user event by calling
//!   `install_ns_workspace_observer()` on the main thread. The user event is a
//!   Tao `Event::UserEvent(())` posted exactly once by `start_watcher` (via
//!   the proxy set with [`set_main_proxy`]). In your `match` arm for
//!   `Event::UserEvent(())`, call `install_ns_workspace_observer()`. This function is
//!   idempotent and safe to call multiple times; only the first call performs
//!   installation.
//!
//! All operations are macOS-only and may require Accessibility permission.

mod ax;
mod cg;
mod event;
mod ns;
mod watcher;

// Ensure Accessibility symbols (kAX* constants, AX* functions) link correctly
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {}

pub use event::FocusEvent;
pub use ns::{install_ns_workspace_observer, set_main_proxy, wake_main_loop};

use thiserror::Error;
use tokio::sync::mpsc::UnboundedSender;

/// Errors that can occur when interacting with mac-focus-watcher public APIs.
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
/// When to call:
/// - Invoke once your application is ready to receive focus updates (for
///   instance, after completing any IPC or UI handshake).
/// - May be called from any thread; it spawns a background thread for CG/AX.
///
/// Preconditions:
/// - The Tao main thread must have provided its `EventLoopProxy<()>` via
///   [`set_main_proxy`] so that the NSWorkspace observer can be installed on
///   the main thread.
///
/// Effects:
/// - Registers `tx` as the sink for [`FocusEvent`]s (both NS and CG/AX).
/// - Posts exactly one Tao user event, `Event::UserEvent(())`, via the
///   `EventLoopProxy(())` set by [`set_main_proxy`], requesting installation of
///   the NSWorkspace observer on the main thread. Handle that specific
///   `UserEvent(())` in your Tao event loop by calling
///   [`install_ns_workspace_observer`].
/// - Spawns the background watcher thread which emits [`FocusEvent`]s to `tx`.
pub fn start_watcher(tx: UnboundedSender<FocusEvent>) -> Result<(), Error> {
    ns::set_ns_sink(tx.clone());
    ns::request_ns_observer_install()?;
    watcher::start_watcher(tx);
    Ok(())
}
