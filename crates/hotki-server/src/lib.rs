#![deny(clippy::disallowed_methods)]
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
//! World read path and forwarding
//! - The engine embeds and drives the `hotki-world` service; it is the sole
//!   source of truth for window/focus state on macOS.
//! - There is no engine‑side CG/AX fallback or separate focus watcher.
//! - The server forwards `WorldEvent`s to clients and exposes RPCs for world
//!   snapshots and status. Clients should use the snapshot on reconnect and
//!   then resume streaming.
//! - Permission state (AX/Input Monitoring/Screen Recording) is surfaced via
//!   `WorldStatus` and should be presented in the UI as actionable guidance.
//!
//! Errors and user guidance
//! - If world initialization encounters missing permissions or backpressure,
//!   the server emits notifications and status fields to guide the user. On
//!   macOS this commonly means granting Accessibility, Input Monitoring, and
//!   Screen Recording permissions in System Settings.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

use std::{env, path::PathBuf, process::id, sync::OnceLock};

mod client;
mod error;
mod ipc;
mod loop_wake;
mod process;
mod server;

mod util;

pub use client::Client;
pub use error::{Error, Result};
pub use ipc::{Connection, rpc::WorldSnapshotLite};
pub use server::Server;
pub mod smoketest_bridge;

/// Return the per-user runtime directory used for IPC socket files.
///
/// Preference order:
/// - `$XDG_RUNTIME_DIR/hotki`
/// - `~/Library/Caches/hotki/run` (macOS user cache)
fn socket_runtime_dir() -> PathBuf {
    if let Ok(xdg) = env::var("XDG_RUNTIME_DIR")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("hotki");
    }
    // Fallback: ~/Library/Caches/hotki/run
    let home = env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join("Library/Caches/hotki/run")
}

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
        socket_runtime_dir()
            .join(format!("hotki-server-{}-{}.sock", uid, pid))
            .to_string_lossy()
            .to_string()
    })
}

/// Compute the per‑process socket path for a specific PID (same scheme used by
/// `default_socket_path`). This avoids knowledge drift in external tools like
/// smoketests when connecting to a managed server.
pub fn socket_path_for_pid(pid: u32) -> String {
    let uid = unsafe { libc::getuid() };
    socket_runtime_dir()
        .join(format!("hotki-server-{}-{}.sock", uid, pid))
        .to_string_lossy()
        .to_string()
}
