//! Typed RPC shim for MRPC transport.
//!
//! Centralizes parameter encoding/decoding to avoid string drift between client
//! and server. Keeps `mrpc::Value` as the transport payload but exposes thin
//! typed helpers for callers.

pub use hotki_protocol::rpc::{
    HotkeyMethod, HotkeyNotification, InjectKeyReq, InjectKind, ServerStatusLite, WorldSnapshotLite,
};
use mrpc::Value;

/// Encode world status into an MRPC value for transport.
pub fn enc_world_status(ws: &hotki_world::WorldStatus) -> Value {
    use mrpc::Value as V;
    let focused = match ws.focused {
        Some(k) => V::Map(vec![
            (V::String("pid".into()), V::Integer((k.pid as i64).into())),
            (V::String("id".into()), V::Integer((k.id as i64).into())),
        ]),
        None => V::Nil,
    };
    let cap_to_i = |p: &hotki_world::PermissionState| match p {
        hotki_world::PermissionState::Granted => 1,
        hotki_world::PermissionState::Denied => 0,
        hotki_world::PermissionState::Unknown => -1,
    };
    let caps = V::Map(vec![
        (
            V::String("accessibility".into()),
            V::Integer(cap_to_i(&ws.capabilities.accessibility).into()),
        ),
        (
            V::String("screen_recording".into()),
            V::Integer(cap_to_i(&ws.capabilities.screen_recording).into()),
        ),
    ]);
    V::Map(vec![
        (
            V::String("windows_count".into()),
            V::Integer((ws.windows_count as i64).into()),
        ),
        (V::String("focused".into()), focused),
        (
            V::String("last_tick_ms".into()),
            V::Integer((ws.last_tick_ms as i64).into()),
        ),
        (
            V::String("current_poll_ms".into()),
            V::Integer((ws.current_poll_ms as i64).into()),
        ),
        (V::String("capabilities".into()), caps),
    ])
}

/// Encode `set_config` params.
pub fn enc_set_config(cfg: &config::Config) -> crate::Result<Value> {
    let bytes = rmp_serde::to_vec_named(cfg)?;
    Ok(Value::Binary(bytes))
}

/// Decode `set_config` params.
pub fn dec_set_config_param(v: &Value) -> Result<config::Config, mrpc::RpcError> {
    match v {
        Value::Binary(bytes) => rmp_serde::from_slice::<config::Config>(bytes).map_err(|e| {
            mrpc::RpcError::Service(mrpc::ServiceError {
                name: crate::error::RpcErrorCode::InvalidConfig.to_string(),
                value: Value::String(e.to_string().into()),
            })
        }),
        _ => Err(mrpc::RpcError::Service(mrpc::ServiceError {
            name: crate::error::RpcErrorCode::InvalidType.to_string(),
            value: Value::String("expected binary msgpack".into()),
        })),
    }
}

/// Encode a generic UI event for notifications to clients.
pub fn enc_event(event: &hotki_protocol::MsgToUI) -> crate::Result<Value> {
    hotki_protocol::ipc::codec::msg_to_value(event)
        .map_err(|e| crate::Error::Serialization(e.to_string()))
}

/// Decode a generic UI event from a notification param value.
pub fn dec_event(v: Value) -> Result<hotki_protocol::MsgToUI, crate::Error> {
    hotki_protocol::ipc::codec::value_to_msg(v)
        .map_err(|e| crate::Error::Serialization(e.to_string()))
}

/// Encode a server status snapshot to msgpack binary `Value`.
pub fn enc_server_status(status: &ServerStatusLite) -> crate::Result<Value> {
    let bytes = rmp_serde::to_vec_named(status)?;
    Ok(Value::Binary(bytes))
}

/// Encode a world snapshot to msgpack binary `Value`.
pub fn enc_world_snapshot(snap: &WorldSnapshotLite) -> crate::Result<Value> {
    let bytes = rmp_serde::to_vec_named(snap)?;
    Ok(Value::Binary(bytes))
}

/// Encode `inject_key` params as msgpack binary.
pub fn enc_inject_key(req: &InjectKeyReq) -> crate::Result<Value> {
    let bytes = rmp_serde::to_vec_named(req)?;
    Ok(Value::Binary(bytes))
}

/// Decode `inject_key` param from msgpack binary.
pub fn dec_inject_key_param(v: &Value) -> Result<InjectKeyReq, mrpc::RpcError> {
    match v {
        Value::Binary(bytes) => rmp_serde::from_slice::<InjectKeyReq>(bytes).map_err(|e| {
            mrpc::RpcError::Service(mrpc::ServiceError {
                name: crate::error::RpcErrorCode::InvalidConfig.to_string(),
                value: Value::String(e.to_string().into()),
            })
        }),
        _ => Err(mrpc::RpcError::Service(mrpc::ServiceError {
            name: crate::error::RpcErrorCode::InvalidType.to_string(),
            value: Value::String("expected binary msgpack".into()),
        })),
    }
}

/// Helper to convert protocol injection kind to internal event kind.
pub fn inject_kind_to_event(kind: InjectKind) -> mac_hotkey::EventKind {
    match kind {
        InjectKind::Down => mac_hotkey::EventKind::KeyDown,
        InjectKind::Up => mac_hotkey::EventKind::KeyUp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hotki_protocol::rpc::InjectKeyReq;

    #[test]
    fn notify_name_is_notify() {
        assert_eq!(HotkeyNotification::Notify.as_str(), "notify");
    }

    #[test]
    fn set_config_roundtrip() {
        let cfg = config::Config::default();
        let v = enc_set_config(&cfg).expect("encode");
        let dec = dec_set_config_param(&v).expect("decode");
        // Default roundtrip should preserve style key font size default, etc.
        assert_eq!(
            format!("{:?}", cfg.hud(&config::Cursor::default()).mode),
            format!("{:?}", dec.hud(&config::Cursor::default()).mode)
        );
    }

    #[test]
    fn set_config_invalid_type_error_code() {
        let err = dec_set_config_param(&Value::String("oops".into())).expect_err("should error");
        match err {
            mrpc::RpcError::Service(se) => {
                assert_eq!(se.name, crate::error::RpcErrorCode::InvalidType.to_string());
            }
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[test]
    fn set_config_invalid_binary_error_code() {
        let err = dec_set_config_param(&Value::Binary(vec![1, 2, 3])).expect_err("should error");
        match err {
            mrpc::RpcError::Service(se) => {
                assert_eq!(
                    se.name,
                    crate::error::RpcErrorCode::InvalidConfig.to_string()
                );
            }
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[test]
    fn inject_key_roundtrip() {
        let req = InjectKeyReq {
            ident: "shift+cmd+0".into(),
            kind: InjectKind::Down,
            repeat: false,
        };
        let v = enc_inject_key(&req).expect("encode");
        let dec = dec_inject_key_param(&v).expect("decode inject");
        assert_eq!(req, dec);
    }
}