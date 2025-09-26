//! Test bridge protocol used by the smoketest harness to proxy RPCs through the UI.
use hotki_protocol::{App, Cursor, NotifyKind};
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
    /// Block until the world reconcile sequence reaches at least `target`.
    WaitForWorldSeq {
        /// Minimum reconcile sequence expected from the world service.
        target: u64,
        /// Maximum time to wait before returning an error (milliseconds).
        #[serde(default = "default_wait_world_seq_timeout_ms")]
        timeout_ms: u64,
    },
    /// Request a graceful backend shutdown.
    Shutdown,
}

/// Default timeout (in milliseconds) applied when waiting for the world
/// reconcile sequence to advance.
pub const fn default_wait_world_seq_timeout_ms() -> u64 {
    5_000
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Success reporting the reconcile sequence reached by the world service.
    WorldSeq {
        /// Reconcile sequence observed when the wait completed.
        reached: u64,
    },
    /// Success containing a world snapshot.
    WorldSnapshot {
        /// Serialized world snapshot payload.
        snapshot: WorldSnapshotLite,
    },
    /// Asynchronous event emitted by the UI runtime.
    Event {
        /// Event payload describing the observed state change.
        event: BridgeEvent,
    },
    /// Initial handshake response with server/runtime state.
    Handshake {
        /// Current server idle timer snapshot.
        idle_timer: BridgeIdleTimerState,
        /// Pending notifications queued on the UI side.
        notifications: Vec<BridgeNotification>,
    },
    /// Error with a message for diagnostics.
    Err {
        /// Human-readable error message.
        message: String,
    },
}

/// Event payload streamed from the UI runtime to the smoketest harness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BridgeEvent {
    /// HUD state changed after evaluating a cursor update.
    Hud {
        /// Cursor context describing the HUD state.
        cursor: Cursor,
        /// Logical depth associated with the cursor.
        depth: usize,
        /// Optional parent title when the HUD is nested under another item.
        parent_title: Option<String>,
        /// Keys currently visible in the HUD.
        keys: Vec<BridgeHudKey>,
    },
    /// World service reported a focus change.
    WorldFocus {
        /// Latest focused application context, if any.
        app: Option<App>,
        /// Reconcile sequence observed when the focus change occurred.
        reconcile_seq: u64,
    },
}

/// HUD key metadata forwarded to the smoketest harness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeHudKey {
    /// Key chord string as rendered by the HUD.
    pub ident: String,
    /// Human-readable description provided by the config.
    pub description: String,
    /// True when the key represents a mode binding.
    pub is_mode: bool,
}

/// Snapshot of the server idle timer state returned during handshake.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeIdleTimerState {
    /// Idle timeout configuration in seconds.
    pub timeout_secs: u64,
    /// True when the timer is currently armed on the server.
    pub armed: bool,
    /// Optional wall-clock deadline for the idle timer in milliseconds since epoch.
    pub deadline_ms: Option<u64>,
    /// Number of clients currently connected to the server.
    pub clients_connected: usize,
}

/// Pending notification payload returned during handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeNotification {
    /// Notification severity kind.
    pub kind: NotifyKind,
    /// Notification title text.
    pub title: String,
    /// Notification body text.
    pub text: String,
}

/// Response envelope emitted by the UI runtime back to the smoketest harness.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Extract the reconcile sequence value from the response.
    pub fn into_world_seq(self) -> Result<u64, String> {
        match self {
            BridgeResponse::WorldSeq { reached } => Ok(reached),
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
