//! Shared event, handshake, and error types for the server driver.

use std::collections::BTreeSet;

use hotki_protocol::{
    DisplaysSnapshot, MsgToUI,
    rpc::{RpcErrorCode, RpcFailure, ServerStatusLite},
};
use thiserror::Error;

/// Result alias for server driver operations.
pub type DriverResult<T> = Result<T, DriverError>;

/// Unique identifier assigned to locally observed server events.
pub type DriverEventId = u64;

/// Millisecond-precision wall-clock timestamp for event diagnostics.
pub type DriverTimestampMs = u64;

/// Raw server event record captured from the production `MsgToUI` stream.
#[derive(Debug, Clone)]
pub struct DriverEventRecord {
    /// Locally assigned event identifier.
    pub id: DriverEventId,
    /// Millisecond timestamp recorded when the driver observed the event.
    pub timestamp_ms: DriverTimestampMs,
    /// Protocol event payload emitted by the server.
    pub payload: MsgToUI,
}

/// Snapshot of the most recent HUD update observed on the server event stream.
#[derive(Debug, Clone)]
pub struct HudSnapshot {
    /// Identifier of the server event associated with this snapshot.
    pub event_id: DriverEventId,
    /// Millisecond timestamp when the snapshot was observed.
    pub received_ms: DriverTimestampMs,
    /// Fully rendered HUD state payload.
    pub hud: hotki_protocol::HudState,
    /// Display geometry snapshot carried with the HUD update.
    pub displays: DisplaysSnapshot,
    /// Canonicalized identifiers rendered by the HUD for readiness checks.
    pub idents: BTreeSet<String>,
}

/// Handshake payload captured from the server after connecting the driver.
#[derive(Debug, Clone)]
pub struct ServerHandshake {
    /// Server status reported by the production RPC API.
    pub status: ServerStatusLite,
}

/// Error variants surfaced by the smoketest server driver.
#[derive(Debug, Error)]
pub enum DriverError {
    /// Creating the local Tokio runtime failed.
    #[error("failed to create driver runtime: {message}")]
    Runtime {
        /// Human-readable runtime creation failure.
        message: String,
    },
    /// Connecting to the server socket failed.
    #[error("failed to connect to server socket '{socket_path}': {message}")]
    Connect {
        /// Socket path we attempted to reach.
        socket_path: String,
        /// Underlying connection error message.
        message: String,
    },
    /// A server command was attempted before initialization.
    #[error("server driver not initialized")]
    NotInitialized,
    /// Exhausted retries while waiting for the server to become ready.
    #[error(
        "timed out after {timeout_ms} ms initializing server driver at '{socket_path}': {last_error}"
    )]
    InitTimeout {
        /// Socket path we attempted to reach.
        socket_path: String,
        /// Timeout duration in milliseconds.
        timeout_ms: u64,
        /// Last observed error message.
        last_error: String,
    },
    /// The server accepted RPCs, but no asynchronous event arrived.
    #[error("timed out after {timeout_ms} ms waiting for server events at '{socket_path}'")]
    EventStreamTimeout {
        /// Socket path we connected to.
        socket_path: String,
        /// Timeout duration in milliseconds.
        timeout_ms: u64,
    },
    /// Server RPC or event-stream operation failed.
    #[error("server command failed: {message}")]
    ServerFailure {
        /// Human-readable error message from the server or MRPC client.
        message: String,
    },
    /// Server RPC returned a stable typed service error code.
    #[error("server command failed: {failure}")]
    ServerRpcFailure {
        /// Typed server-side RPC failure.
        failure: RpcFailure,
    },
    /// Waiting for a binding to appear timed out.
    #[error("timed out after {timeout_ms} ms waiting for binding '{ident}'")]
    BindingTimeout {
        /// Identifier we were waiting for.
        ident: String,
        /// Timeout duration in milliseconds.
        timeout_ms: u64,
    },
}

/// Validate invariants returned by the server before running tests.
pub(super) fn ensure_clean_handshake(handshake: &ServerHandshake) -> DriverResult<()> {
    if handshake.status.idle_timer_armed {
        return Err(DriverError::ServerFailure {
            message: format!(
                "server idle timer armed during handshake (deadline_ms={:?})",
                handshake.status.idle_deadline_ms
            ),
        });
    }
    if handshake.status.clients_connected < 2 {
        return Err(DriverError::ServerFailure {
            message: format!(
                "server reported {} connected client(s); expected UI plus smoketest driver",
                handshake.status.clients_connected
            ),
        });
    }
    let input = &handshake.status.input;
    if input.tap_mode != hotki_protocol::TapMode::InjectionOnly
        || input.secure_input != hotki_protocol::SecureInputState::Unknown
        || input.secure_input_owner.is_some()
        || input.blocked
    {
        return Err(DriverError::ServerFailure {
            message: format!(
                "smoketest server reported unexpected input health: mode={:?} secure_input={:?} owner={:?} blocked={}",
                input.tap_mode, input.secure_input, input.secure_input_owner, input.blocked
            ),
        });
    }
    Ok(())
}

/// Render a concise diagnostic string for initialization failures.
pub(super) fn describe_init_error(err: &DriverError) -> String {
    match err {
        DriverError::Connect { message, .. }
        | DriverError::Runtime { message }
        | DriverError::ServerFailure { message } => message.clone(),
        DriverError::ServerRpcFailure { failure } => failure.payload.message.clone(),
        DriverError::EventStreamTimeout { .. } => err.to_string(),
        other => other.to_string(),
    }
}

/// Normalize an identifier by parsing it as a chord when possible.
pub(super) fn canonicalize_ident(raw: &str) -> String {
    mac_keycode::Chord::parse(raw)
        .map(|chord| chord.to_string())
        .unwrap_or_else(|| raw.to_string())
}

/// Returns true when a driver error is the typed missing-binding RPC code.
pub(super) fn is_key_not_bound(err: &DriverError) -> bool {
    matches!(
        err,
        DriverError::ServerRpcFailure { failure }
            if failure.code == RpcErrorCode::KeyNotBound
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(clients_connected: usize, idle_timer_armed: bool) -> ServerStatusLite {
        ServerStatusLite {
            idle_timeout_secs: 5,
            idle_timer_armed,
            idle_deadline_ms: None,
            clients_connected,
            input: hotki_protocol::InputHealth::default(),
        }
    }

    #[test]
    fn handshake_requires_ui_and_driver_clients() {
        let handshake = ServerHandshake {
            status: status(1, false),
        };

        let err = ensure_clean_handshake(&handshake).unwrap_err();

        assert!(matches!(err, DriverError::ServerFailure { .. }));
    }

    #[test]
    fn handshake_rejects_armed_idle_timer() {
        let handshake = ServerHandshake {
            status: status(2, true),
        };

        let err = ensure_clean_handshake(&handshake).unwrap_err();

        assert!(matches!(err, DriverError::ServerFailure { .. }));
    }

    #[test]
    fn handshake_accepts_active_ui_and_driver() {
        let handshake = ServerHandshake {
            status: status(2, false),
        };

        ensure_clean_handshake(&handshake).unwrap();
    }

    #[test]
    fn missing_binding_detection_uses_rpc_code() {
        let err = DriverError::ServerRpcFailure {
            failure: RpcFailure::new(RpcErrorCode::KeyNotBound, "missing").with_ident("cmd+k"),
        };
        assert!(is_key_not_bound(&err));
        let DriverError::ServerRpcFailure { failure } = &err else {
            panic!("expected RPC failure");
        };
        assert_eq!(failure.payload.fields.ident.as_deref(), Some("cmd+k"));
        assert!(!is_key_not_bound(&DriverError::ServerFailure {
            message: "service error KeyNotBound".to_string(),
        }));
    }
}
