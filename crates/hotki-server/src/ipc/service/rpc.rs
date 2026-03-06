use hotki_protocol::{
    FocusSnapshot, MsgToUI,
    rpc::{InjectKeyReq, InjectKind, WorldSnapshotLite},
};
use mrpc::{RpcError, ServiceError, Value};

/// Construct a typed `RpcError::Service` with a stable name and structured fields.
pub(super) fn typed_err(code: crate::error::RpcErrorCode, fields: &[(&str, Value)]) -> RpcError {
    let map = fields
        .iter()
        .map(|(key, value)| (Value::String((*key).into()), value.clone()))
        .collect();
    RpcError::Service(ServiceError {
        name: code.to_string(),
        value: Value::Map(map),
    })
}

/// Extract a required UTF-8 string parameter from an MRPC request.
pub(super) fn string_param(
    params: &[Value],
    method: &str,
    expected: &str,
    missing_code: crate::error::RpcErrorCode,
) -> Result<String, RpcError> {
    let Some(value) = params.first() else {
        return Err(typed_err(
            missing_code,
            &[
                ("method", Value::String(method.into())),
                ("expected", Value::String(expected.into())),
            ],
        ));
    };

    match value {
        Value::String(raw) => raw.as_str().map(|value| value.to_string()).ok_or_else(|| {
            typed_err(
                crate::error::RpcErrorCode::InvalidType,
                &[(
                    "expected",
                    Value::String(format!("utf8 string {}", expected).into()),
                )],
            )
        }),
        _ => Err(typed_err(
            crate::error::RpcErrorCode::InvalidType,
            &[(
                "expected",
                Value::String(format!("string {}", expected).into()),
            )],
        )),
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
    match rmp_serde::to_vec_named(status) {
        Ok(bytes) => Value::Binary(bytes),
        Err(_) => Value::Nil,
    }
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
    let bytes = rmp_serde::to_vec_named(status)?;
    Ok(Value::Binary(bytes))
}

/// Encode a world snapshot to a msgpack binary value.
pub(super) fn enc_world_snapshot(snapshot: &WorldSnapshotLite) -> crate::Result<Value> {
    let bytes = rmp_serde::to_vec_named(snapshot)?;
    Ok(Value::Binary(bytes))
}

/// Decode an `inject_key` parameter from msgpack binary.
pub(crate) fn dec_inject_key_param(value: &Value) -> Result<InjectKeyReq, RpcError> {
    match value {
        Value::Binary(bytes) => rmp_serde::from_slice::<InjectKeyReq>(bytes).map_err(|err| {
            typed_err(
                crate::error::RpcErrorCode::InvalidConfig,
                &[("message", Value::String(err.to_string().into()))],
            )
        }),
        _ => Err(typed_err(
            crate::error::RpcErrorCode::InvalidType,
            &[("expected", Value::String("binary msgpack".into()))],
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
