//! Internal hotkey server integration for Hotki.
//!
//!
//! This crate is internal-only and not intended for public use. It provides
//! a thin server/client layer around the core engine to manage global hotkeys
//! and communicate with the Hotki UI process.
//!
//! Important: Socket Path Is Per-Process
//! - Each UI instance owns its own server. The default socket path used by this
//!   crate is derived from the current UID and process ID, so it is unique to the
//!   running process. Downstream consumers should not assume a global singleton
//!   server; spawn/connect per UI instance or pass an explicit socket path if you
//!   want to coordinate between processes.
//!
//! Focus Watcher Contract
//! - Tao main thread: Creates the event loop and calls
//!   `mac_focus_watcher::set_main_proxy(...)` once so background code can post a
//!   user event to request NS observer installation.
//! - Service layer: When the engine first activates (e.g., on `set_mode`), the
//!   IPC service starts the focus watcher via `mac_focus_watcher::start_watcher(tx)`.
//!   This posts a Tao `UserEvent(())` and begins emitting `FocusEvent`s on `tx`.
//! - Main loop: Handles `Event::UserEvent(())` and calls
//!   `mac_focus_watcher::install_ns_workspace_observer()` on the main thread. This
//!   ties the NSWorkspace notifications into the same `FocusEvent` stream.
//!
//! Errors and User Guidance
//! - If the watcher fails to start, the server emits a UI notification with
//!   actionable guidance. On macOS this commonly means granting
//!   Accessibility and/or Input Monitoring permissions in System Settings.

use std::{process::id, sync::OnceLock};

mod client;
mod error;
mod ipc;
mod process;
mod server;

mod util;

pub use client::Client;
pub use error::{Error, Result};
pub use ipc::Connection;
pub use server::Server;

/// Get the default socket path for IPC communication used within this crate.
///
/// Note: This path is per-process. It includes the current UID and PID so that
/// each UI instance uses a dedicated MRPC server socket.
pub(crate) fn default_socket_path() -> &'static str {
    static SOCKET_PATH: OnceLock<String> = OnceLock::new();
    SOCKET_PATH.get_or_init(|| {
        let uid = unsafe { libc::getuid() };
        let pid = id();
        // Always use a unique socket path per process
        format!("/tmp/hotki-server-{}-{}.sock", uid, pid)
    })
}

/// Compute the per-process socket path for a specific PID (same scheme used by
/// `default_socket_path`). Exposed for internal tools (e.g., smoketests) to
/// avoid knowledge drift on the path convention.
pub fn socket_path_for_pid(pid: u32) -> String {
    let uid = unsafe { libc::getuid() };
    format!("/tmp/hotki-server-{}-{}.sock", uid, pid)
}
