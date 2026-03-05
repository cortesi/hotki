use std::{collections::BTreeSet, io};

use hotki_protocol::{DisplaysSnapshot, rpc::ServerStatusLite};
use hotki_server::smoketest_bridge::{
    BridgeCommandId, BridgeEvent, BridgeNotification, BridgeTimestampMs,
};
use thiserror::Error;

/// Result alias for bridge driver operations.
pub type DriverResult<T> = Result<T, DriverError>;

/// Raw bridge event record captured from the UI runtime stream.
#[derive(Debug, Clone)]
pub struct BridgeEventRecord {
    /// Command identifier assigned to the streamed event.
    pub id: BridgeCommandId,
    /// Millisecond timestamp recorded when the UI flushed the event.
    pub timestamp_ms: BridgeTimestampMs,
    /// Event payload describing the state change.
    pub payload: BridgeEvent,
}

/// Snapshot of the most recent HUD update observed on the bridge stream.
#[derive(Debug, Clone)]
pub struct HudSnapshot {
    /// Identifier of the bridge event associated with this snapshot.
    pub event_id: BridgeCommandId,
    /// Millisecond timestamp when the snapshot was observed.
    pub received_ms: BridgeTimestampMs,
    /// Fully rendered HUD state payload.
    pub hud: hotki_protocol::HudState,
    /// Display geometry snapshot carried with the HUD update.
    pub displays: DisplaysSnapshot,
    /// Canonicalized identifiers rendered by the HUD for readiness checks.
    pub idents: BTreeSet<String>,
}

/// Handshake payload returned when the smoketest bridge establishes a session.
#[derive(Debug, Clone)]
pub struct BridgeHandshake {
    /// Idle timer snapshot reported by the UI runtime.
    pub idle_timer: ServerStatusLite,
    /// Pending notifications surfaced by the UI.
    pub notifications: Vec<BridgeNotification>,
}

/// Error variants surfaced by the smoketest bridge driver.
#[derive(Debug, Error)]
pub enum DriverError {
    /// Connecting to the bridge socket failed.
    #[error("failed to connect to bridge socket '{socket_path}': {source}")]
    Connect {
        /// Socket path we attempted to reach.
        socket_path: String,
        /// Underlying IO error.
        #[source]
        source: io::Error,
    },
    /// A bridge command was attempted before initialization.
    #[error("bridge connection not initialized")]
    NotInitialized,
    /// Exhausted retries while waiting for the bridge to become ready.
    #[error("timed out after {timeout_ms} ms initializing bridge at '{socket_path}': {last_error}")]
    InitTimeout {
        /// Socket path we attempted to reach.
        socket_path: String,
        /// Timeout duration in milliseconds.
        timeout_ms: u64,
        /// Last observed error message.
        last_error: String,
    },
    /// Bridge reported a failure while handling a command.
    #[error("bridge command failed: {message}")]
    BridgeFailure {
        /// Human-readable error message from the bridge.
        message: String,
    },
    /// The bridge did not acknowledge a command fast enough.
    #[error("bridge acknowledgement for command {command_id} timed out after {timeout_ms} ms")]
    AckTimeout {
        /// Command identifier we waited on.
        command_id: BridgeCommandId,
        /// Timeout budget that was exceeded in milliseconds.
        timeout_ms: u64,
    },
    /// Bridge responses arrived out of sequence.
    #[error("bridge sequence mismatch: expected command {expected}, got {got}")]
    SequenceMismatch {
        /// Command identifier we expected.
        expected: BridgeCommandId,
        /// Command identifier we observed.
        got: BridgeCommandId,
    },
    /// Bridge failed to emit an acknowledgement before responding.
    #[error("bridge missing ACK for command {command_id}")]
    AckMissing {
        /// Command identifier lacking an acknowledgement.
        command_id: BridgeCommandId,
    },
    /// Waiting for a binding to appear timed out.
    #[error("timed out after {timeout_ms} ms waiting for binding '{ident}'")]
    BindingTimeout {
        /// Identifier we were waiting for.
        ident: String,
        /// Timeout duration in milliseconds.
        timeout_ms: u64,
    },
    /// Bridge IO error while sending/receiving commands.
    #[error("bridge I/O error: {source}")]
    Io {
        /// Underlying IO error.
        #[source]
        source: io::Error,
    },
    /// Bridge produced additional messages after shutdown was acknowledged.
    #[error("unexpected bridge message after shutdown: {message}")]
    PostShutdownMessage {
        /// Raw message payload observed.
        message: String,
    },
}

/// Validate invariants returned by the bridge handshake before running tests.
pub(super) fn ensure_clean_handshake(handshake: &BridgeHandshake) -> DriverResult<()> {
    if handshake.idle_timer.idle_timer_armed {
        return Err(DriverError::BridgeFailure {
            message: format!(
                "server idle timer armed during handshake (deadline_ms={:?})",
                handshake.idle_timer.idle_deadline_ms
            ),
        });
    }
    if handshake.idle_timer.clients_connected == 0 {
        return Err(DriverError::BridgeFailure {
            message: "server reported zero connected clients during handshake".to_string(),
        });
    }
    if let Some(sample) = handshake.notifications.first() {
        return Err(DriverError::BridgeFailure {
            message: format!(
                "bridge reported {} pending notifications, starting with '{}': {}",
                handshake.notifications.len(),
                sample.title,
                sample.text
            ),
        });
    }
    Ok(())
}

/// Render a concise diagnostic string for initialization failures.
pub(super) fn describe_init_error(err: &DriverError) -> String {
    match err {
        DriverError::Connect { source, .. } => source.to_string(),
        DriverError::BridgeFailure { message } => message.clone(),
        DriverError::Io { source } => source.to_string(),
        other => other.to_string(),
    }
}

/// Normalize an identifier by parsing it as a chord when possible.
pub(super) fn canonicalize_ident(raw: &str) -> String {
    mac_keycode::Chord::parse(raw)
        .map(|chord| chord.to_string())
        .unwrap_or_else(|| raw.to_string())
}

/// Returns true when the bridge error message indicates a missing key binding.
pub(super) fn message_contains_key_not_bound(msg: &str) -> bool {
    msg.contains("KeyNotBound")
}
