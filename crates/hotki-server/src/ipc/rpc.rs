//! Typed RPC shim for MRPC transport.
//!
//! Centralizes method names and parameter encoding/decoding to avoid
//! string drift between client and server. Keeps `mrpc::Value` as the
//! transport payload but exposes thin typed helpers for callers.

use mrpc::Value;

/// RPC request methods supported by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyMethod {
    Shutdown,
    SetConfig,
}

impl HotkeyMethod {
    /// Stable string name for the method when talking to MRPC.
    pub fn as_str(&self) -> &'static str {
        match self {
            HotkeyMethod::Shutdown => "shutdown",
            HotkeyMethod::SetConfig => "set_config",
        }
    }

    /// Parse a method name received over MRPC.
    pub fn try_from_str(s: &str) -> Option<Self> {
        match s {
            "shutdown" => Some(HotkeyMethod::Shutdown),
            "set_config" => Some(HotkeyMethod::SetConfig),
            _ => None,
        }
    }
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
pub fn enc_set_config(cfg: &config::Config) -> Value {
    let bytes = rmp_serde::to_vec_named(cfg).expect("config::Config to msgpack");
    Value::Binary(bytes)
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
pub fn enc_event(event: &hotki_protocol::MsgToUI) -> Value {
    hotki_protocol::ipc::codec::msg_to_value(event)
}

/// Decode a generic UI event from a notification param value.
pub fn dec_event(v: Value) -> Result<hotki_protocol::MsgToUI, crate::Error> {
    hotki_protocol::ipc::codec::value_to_msg(v)
        .map_err(|e| crate::Error::Serialization(e.to_string()))
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
        let v = enc_set_config(&cfg);
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
}
