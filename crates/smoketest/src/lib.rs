#![warn(missing_docs)]
//! Shared ownership harness for launched Hotki app sessions.

/// Deadline and pacing configuration shared by harness consumers.
pub mod config;
/// Harness error types.
pub mod error;
/// Owned child-process utilities.
pub mod process;
/// Synchronous RPC and event driver for a session-owned server.
pub mod server_drive;
/// App process, RPC, and graceful-shutdown ownership.
pub mod session;
/// PID-scoped native window discovery and capture.
#[path = "cases/ui/window_inspection.rs"]
pub mod windows;
