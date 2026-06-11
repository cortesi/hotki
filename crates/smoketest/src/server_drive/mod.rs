//! RPC driving helpers against the running server.

/// Synchronous server driver over the production MRPC client.
mod client;
/// Shared driver state, snapshots, and error types.
mod types;

pub use client::ServerDriver;
pub use types::{DriverError, DriverEventRecord, DriverResult, HudSnapshot, ServerHandshake};
