//! Test bridge protocol used by the smoketest harness to proxy RPCs through the UI.
use serde::{Deserialize, Serialize};

use crate::ipc::rpc::WorldSnapshotLite;

/// Unique identifier assigned to each bridge command.
pub type BridgeCommandId = u64;

/// Millisecond-precision wall-clock timestamp carried by bridge envelopes.
pub type BridgeTimestampMs = u64;

/// Request envelope transmitted from the smoketest harness to the UI runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeCommand {
    /// Monotonic command identifier allocated by the harness.
    pub command_id: BridgeCommandId,
    /// Millisecond timestamp recorded when the harness issued the command.
    pub issued_at_ms: BridgeTimestampMs,
    /// Bridge request payload.
    pub request: BridgeRequest,
}

/// Request type for the smoketest bridge between the smoketest harness and the UI runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BridgeRequest {
    /// Handshake/ping used to verify the bridge is ready.
    Ping,
    /// Apply a configuration file located at `path`.
    SetConfig {
        /// Filesystem path to the configuration file to load.
        path: String,
    },
    /// Inject a synthetic key event.
    InjectKey {
        /// Identifier to inject (e.g., chord string).
        ident: String,
        /// Key action to perform.
        kind: BridgeKeyKind,
        #[serde(default)]
        /// When true, treat the event as a repeat key down.
        repeat: bool,
    },
    /// Fetch the current bindings snapshot.
    GetBindings,
    /// Fetch the current depth for liveness checks.
    GetDepth,
    /// Fetch a lightweight world snapshot.
    GetWorldSnapshot,
    /// Request a graceful backend shutdown.
    Shutdown,
}

/// Key event kind forwarded through the bridge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BridgeKeyKind {
    /// Simulate a key-down event.
    Down,
    /// Simulate a key-up event.
    Up,
}

/// Response type for the smoketest bridge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BridgeResponse {
    /// Acknowledge receipt of a command while it waits in the UI queue.
    Ack {
        /// Number of commands currently queued (including the acknowledged one).
        queued: usize,
    },
    /// Success without additional payload.
    Ok,
    /// Success containing a list of bindings.
    Bindings {
        /// Current bindings snapshot.
        bindings: Vec<String>,
    },
    /// Success containing the current depth.
    Depth {
        /// Current depth value.
        depth: usize,
    },
    /// Success containing a world snapshot.
    WorldSnapshot {
        /// Serialized world snapshot payload.
        snapshot: WorldSnapshotLite,
    },
    /// Error with a message for diagnostics.
    Err {
        /// Human-readable error message.
        message: String,
    },
}

/// Response envelope emitted by the UI runtime back to the smoketest harness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BridgeReply {
    /// Identifier of the command that produced this response.
    pub command_id: BridgeCommandId,
    /// Millisecond timestamp recorded when the runtime flushed the response.
    pub timestamp_ms: BridgeTimestampMs,
    /// Response payload.
    pub response: BridgeResponse,
}

impl BridgeResponse {
    /// Map the response into a `Result`, discarding the payload.
    pub fn into_result(self) -> Result<(), String> {
        match self {
            BridgeResponse::Ok => Ok(()),
            BridgeResponse::Err { message } => Err(message),
            other => Err(format!("unexpected bridge response: {:?}", other)),
        }
    }

    /// Extract a payload of bindings from the response.
    pub fn into_bindings(self) -> Result<Vec<String>, String> {
        match self {
            BridgeResponse::Bindings { bindings } => Ok(bindings),
            BridgeResponse::Err { message } => Err(message),
            other => Err(format!("unexpected bridge response: {:?}", other)),
        }
    }

    /// Extract a depth value from the response.
    pub fn into_depth(self) -> Result<usize, String> {
        match self {
            BridgeResponse::Depth { depth } => Ok(depth),
            BridgeResponse::Err { message } => Err(message),
            other => Err(format!("unexpected bridge response: {:?}", other)),
        }
    }

    /// Extract a world snapshot from the response.
    pub fn into_snapshot(self) -> Result<WorldSnapshotLite, String> {
        match self {
            BridgeResponse::WorldSnapshot { snapshot } => Ok(snapshot),
            BridgeResponse::Err { message } => Err(message),
            other => Err(format!("unexpected bridge response: {:?}", other)),
        }
    }
}
