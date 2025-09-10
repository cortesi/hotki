//! Internal hotkey server integration for Hotki.
//!
//! This crate provides a thin server/client layer around the core engine to
//! manage global hotkeys and communicate with the Hotki UI process.
//!
//! Public API (internal stability)
//! - `Server`: runs the Tao event loop and hosts the MRPC IPC server.
//! - `Client`: connects to a server; can auto‑spawn a managed server.
//! - `Connection`: typed RPCs and a stream of UI events (`MsgToUI`).
//! - `socket_path_for_pid(pid)`: derives the per‑process socket path.
//!
//! Connection lifecycle and conventions
//! - Per‑process socket path: The default socket path is derived from the
//!   current UID and process ID. Each UI instance owns its own server; do not
//!   assume a global singleton. To coordinate between processes, pass an
//!   explicit socket path.
//! - Auto‑spawn: `Client` can launch the current binary in `--server` mode and
//!   propagate `RUST_LOG`. The parent UI PID is exported via `HOTKI_PARENT_PID`
//!   so the backend exits immediately if the UI process terminates.
//! - Idle shutdown: After the last client disconnects, the server starts an
//!   idle timer (configurable; defaults to a few seconds) and exits when it
//!   fires. A new client connection cancels the timer.
//! - Event stream: Upon connection the server forwards log messages and UI
//!   events (`MsgToUI`) to all clients. A lightweight heartbeat is sent at a
//!   fixed interval to signal liveness.
//!
//! Focus watcher contract
//! - Tao main thread: Creates the event loop and installs a proxy so background
//!   code can post a user event to request NS observer installation.
//! - Engine‑owned watcher: The engine owns and coalesces focus updates and
//!   emits snapshots; the main loop performs the NS observer installation on
//!   demand in response to a user event.
//!
//! Errors and user guidance
//! - If the watcher fails to start, the server emits a UI notification with
//!   actionable guidance. On macOS this commonly means granting Accessibility
//!   and/or Input Monitoring permissions in System Settings.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

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

/// Compute the per‑process socket path for a specific PID (same scheme used by
/// `default_socket_path`). This avoids knowledge drift in external tools like
/// smoketests when connecting to a managed server.
pub fn socket_path_for_pid(pid: u32) -> String {
    let uid = unsafe { libc::getuid() };
    format!("/tmp/hotki-server-{}-{}.sock", uid, pid)
}
