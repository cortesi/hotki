//! RPC driving helpers against the running server.

mod client;
mod deadline;
mod event_cache;
mod rpc;
mod types;

pub use client::ServerDriver;
pub use types::{DriverError, DriverEventRecord, DriverResult, HudSnapshot, ServerHandshake};
