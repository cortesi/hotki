//! RPC driving helpers against the running server.

/// Blocking bridge client and reconnecting request loop.
mod client;
#[cfg(test)]
mod tests;
/// Shared bridge driver state, snapshots, and error types.
mod types;

pub use client::BridgeClient;
pub use hotki_server::smoketest_bridge::{BridgeEvent, ControlSocketScope};
pub use types::{BridgeEventRecord, BridgeHandshake, DriverError, DriverResult, HudSnapshot};
