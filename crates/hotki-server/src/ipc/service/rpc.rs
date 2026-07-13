use hotki_protocol::{
    FocusSnapshot, MsgToUI,
    rpc::{
        InjectKeyReq, InjectKind, RpcErrorCode, RpcFailure, WorldSnapshotLite, encode_rpc_failure,
    },
};
use mrpc::{RpcError, Value};

use crate::ipc::value;

/// Encode a protocol-owned typed failure for MRPC.
pub(super) fn typed_err(failure: RpcFailure) -> RpcError {
    encode_rpc_failure(failure)
}

/// Extract a required UTF-8 string parameter from an MRPC request.
pub(super) fn string_param(
    params: &[Value],
    method: &str,
    expected: &str,
    missing_code: RpcErrorCode,
) -> Result<String, RpcError> {
    let Some(value) = params.first() else {
        return Err(typed_err(
            RpcFailure::new(missing_code, format!("{method} requires {expected}"))
                .with_method(method)
                .with_expected(expected),
        ));
    };

    match value {
        Value::String(raw) => raw.as_str().map(|value| value.to_string()).ok_or_else(|| {
            let expected = format!("UTF-8 string {expected}");
            typed_err(
                RpcFailure::new(RpcErrorCode::InvalidType, format!("expected {expected}"))
                    .with_method(method)
                    .with_expected(expected),
            )
        }),
        _ => {
            let expected = format!("string {expected}");
            Err(typed_err(
                RpcFailure::new(RpcErrorCode::InvalidType, format!("expected {expected}"))
                    .with_method(method)
                    .with_expected(expected),
            ))
        }
    }
}

/// Build the lightweight world snapshot payload returned over MRPC.
pub(super) fn build_snapshot_payload(
    displays: hotki_world::DisplaysSnapshot,
    focused: Option<FocusSnapshot>,
) -> WorldSnapshotLite {
    WorldSnapshotLite { focused, displays }
}

/// Encode world status into an MRPC value for transport.
pub(super) fn enc_world_status(status: &hotki_world::WorldStatus) -> Value {
    value::binary_param(status).unwrap_or(Value::Nil)
}

/// Encode a generic UI event for notifications to clients.
pub(super) fn enc_event(event: &MsgToUI) -> crate::Result<Value> {
    hotki_protocol::ipc::codec::msg_to_value(event)
        .map_err(|err| crate::Error::Serialization(err.to_string()))
}

/// Encode a server status snapshot to a msgpack binary value.
pub(super) fn enc_server_status(
    status: &hotki_protocol::rpc::ServerStatusLite,
) -> crate::Result<Value> {
    value::binary_param(status)
}

/// Encode a world snapshot to a msgpack binary value.
pub(super) fn enc_world_snapshot(snapshot: &WorldSnapshotLite) -> crate::Result<Value> {
    value::binary_param(snapshot)
}

/// Decode an `inject_key` parameter from msgpack binary.
pub(crate) fn dec_inject_key_param(value: &Value) -> Result<InjectKeyReq, RpcError> {
    match value {
        Value::Binary(bytes) => rmp_serde::from_slice::<InjectKeyReq>(bytes).map_err(|err| {
            typed_err(RpcFailure::new(
                RpcErrorCode::InvalidConfig,
                format!("invalid inject request: {err}"),
            ))
        }),
        _ => Err(typed_err(
            RpcFailure::new(RpcErrorCode::InvalidType, "expected binary MessagePack")
                .with_expected("binary MessagePack"),
        )),
    }
}

/// Convert protocol injection kinds to internal hotkey event kinds.
pub(super) fn inject_kind_to_event(kind: InjectKind) -> mac_hotkey::EventKind {
    match kind {
        InjectKind::Down => mac_hotkey::EventKind::KeyDown,
        InjectKind::Up => mac_hotkey::EventKind::KeyUp,
    }
}
