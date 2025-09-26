//! Typed RPC shim for MRPC transport.
//!
//! Centralizes method names and parameter encoding/decoding to avoid
//! string drift between client and server. Keeps `mrpc::Value` as the
//! transport payload but exposes thin typed helpers for callers.

use mrpc::Value;
use serde::{Deserialize, Serialize};

/// RPC request methods supported by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyMethod {
    Shutdown,
    SetConfig,
    InjectKey,
    GetBindings,
    GetDepth,
    GetWorldStatus,
    GetServerStatus,
    GetWorldSnapshot,
}

impl HotkeyMethod {
    /// Stable string name for the method when talking to MRPC.
    pub fn as_str(&self) -> &'static str {
        match self {
            HotkeyMethod::Shutdown => "shutdown",
            HotkeyMethod::SetConfig => "set_config",
            HotkeyMethod::InjectKey => "inject_key",
            HotkeyMethod::GetBindings => "get_bindings",
            HotkeyMethod::GetDepth => "get_depth",
            HotkeyMethod::GetWorldStatus => "get_world_status",
            HotkeyMethod::GetServerStatus => "get_server_status",
            HotkeyMethod::GetWorldSnapshot => "get_world_snapshot",
        }
    }

    /// Parse a method name received over MRPC.
    pub fn try_from_str(s: &str) -> Option<Self> {
        match s {
            "shutdown" => Some(HotkeyMethod::Shutdown),
            "set_config" => Some(HotkeyMethod::SetConfig),
            "inject_key" => Some(HotkeyMethod::InjectKey),
            "get_bindings" => Some(HotkeyMethod::GetBindings),
            "get_depth" => Some(HotkeyMethod::GetDepth),
            "get_world_status" => Some(HotkeyMethod::GetWorldStatus),
            "get_server_status" => Some(HotkeyMethod::GetServerStatus),
            "get_world_snapshot" => Some(HotkeyMethod::GetWorldSnapshot),
            _ => None,
        }
    }
}

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
        (
            V::String("debounce_cache".into()),
            V::Integer((ws.debounce_cache as i64).into()),
        ),
        (
            V::String("debounce_pending".into()),
            V::Integer((ws.debounce_pending as i64).into()),
        ),
        (
            V::String("reconcile_seq".into()),
            V::Integer((ws.reconcile_seq as i64).into()),
        ),
        (
            V::String("suspects_pending".into()),
            V::Integer((ws.suspects_pending as i64).into()),
        ),
        (V::String("capabilities".into()), caps),
    ])
}

/// One-way server→client notification channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyNotification {
    Notify,
}

impl HotkeyNotification {
    pub fn as_str(&self) -> &'static str {
        match self {
            HotkeyNotification::Notify => "notify",
        }
    }
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

/// Lightweight server status snapshot surfaced for smoketest diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerStatusLite {
    /// Idle timeout configured on the server, in seconds.
    pub idle_timeout_secs: u64,
    /// True when the idle timer is currently armed.
    pub idle_timer_armed: bool,
    /// Optional wall-clock deadline in milliseconds since the Unix epoch.
    pub idle_deadline_ms: Option<u64>,
    /// Count of connected clients observed by the server.
    pub clients_connected: usize,
}

/// Encode a server status snapshot to msgpack binary `Value`.
pub fn enc_server_status(status: &ServerStatusLite) -> crate::Result<Value> {
    let bytes = rmp_serde::to_vec_named(status)?;
    Ok(Value::Binary(bytes))
}

/// Lightweight snapshot payload for `get_world_snapshot` method.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorldSnapshotLite {
    /// Current windows in z-order (0 = frontmost first).
    pub windows: Vec<hotki_protocol::WorldWindowLite>,
    /// Focused context, if any.
    pub focused: Option<hotki_protocol::App>,
}

/// Encode a world snapshot to msgpack binary `Value`.
pub fn enc_world_snapshot(snap: &WorldSnapshotLite) -> crate::Result<Value> {
    let bytes = rmp_serde::to_vec_named(snap)?;
    Ok(Value::Binary(bytes))
}

/// Inject key request: encoded as msgpack in a single Binary param.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InjectKeyReq {
    pub ident: String,
    pub kind: InjectKind,
    #[serde(default)]
    pub repeat: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InjectKind {
    Down,
    Up,
}

impl InjectKind {
    pub fn to_event_kind(&self) -> mac_hotkey::EventKind {
        match self {
            InjectKind::Down => mac_hotkey::EventKind::KeyDown,
            InjectKind::Up => mac_hotkey::EventKind::KeyUp,
        }
    }
}

/// Encode `inject_key` params as msgpack binary.
pub fn enc_inject_key(req: &InjectKeyReq) -> crate::Result<Value> {
    let bytes = rmp_serde::to_vec_named(req)?;
    Ok(Value::Binary(bytes))
}

// enc_inject_key_parts removed (unused)

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

// Error codes are defined in `crate::error::RpcErrorCode`.

#[cfg(test)]
mod tests {
    use super::*;

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
