//! Typed RPC definitions for the Hotki protocol.
//!
//! This module defines the method names, request/response structures, and
//! notification types used by the Hotki server and client.

use std::{fmt, str::FromStr};

use mrpc::{RpcError, ServiceError, Value};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{DisplaysSnapshot, FocusSnapshot};

/// Stable error codes returned by Hotki RPC methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcErrorCode {
    /// Server is shutting down and cannot accept the request.
    ShuttingDown,
    /// Required request parameters were missing.
    MissingParams,
    /// Request parameter or payload type was invalid.
    InvalidType,
    /// Requested config update was invalid.
    InvalidConfig,
    /// Requested RPC method is unknown.
    MethodNotFound,
    /// Engine rejected a config update.
    EngineSetConfig,
    /// Requested key identifier is not currently bound.
    KeyNotBound,
    /// Engine dispatch failed while handling a key injection.
    EngineDispatch,
}

impl RpcErrorCode {
    /// Stable service-error name used on the MRPC wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ShuttingDown => "ShuttingDown",
            Self::MissingParams => "MissingParams",
            Self::InvalidType => "InvalidType",
            Self::InvalidConfig => "InvalidConfig",
            Self::MethodNotFound => "MethodNotFound",
            Self::EngineSetConfig => "EngineSetConfig",
            Self::KeyNotBound => "KeyNotBound",
            Self::EngineDispatch => "EngineDispatch",
        }
    }

    /// Parse a stable MRPC service-error name.
    pub fn from_service_name(name: &str) -> Option<Self> {
        Some(match name {
            "ShuttingDown" => Self::ShuttingDown,
            "MissingParams" => Self::MissingParams,
            "InvalidType" => Self::InvalidType,
            "InvalidConfig" => Self::InvalidConfig,
            "MethodNotFound" => Self::MethodNotFound,
            "EngineSetConfig" => Self::EngineSetConfig,
            "KeyNotBound" => Self::KeyNotBound,
            "EngineDispatch" => Self::EngineDispatch,
            _ => return None,
        })
    }
}

impl fmt::Display for RpcErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for RpcErrorCode {
    type Err = RpcErrorDecodeError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::from_service_name(name)
            .ok_or_else(|| RpcErrorDecodeError::UnknownCode(name.to_string()))
    }
}

/// Machine-readable fields attached to an RPC failure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RpcErrorFields {
    /// RPC method involved in the failure.
    pub method: Option<String>,
    /// Human-readable description of the expected input.
    pub expected: Option<String>,
    /// Key identifier involved in the failure.
    pub ident: Option<String>,
}

/// Readable and structured payload attached to an RPC failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcErrorPayload {
    /// Human-readable failure message.
    pub message: String,
    /// Machine-readable fields for callers that need structured recovery.
    pub fields: RpcErrorFields,
}

/// Complete typed RPC failure reconstructed from an MRPC service error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcFailure {
    /// Stable failure code.
    pub code: RpcErrorCode,
    /// Readable and structured failure payload.
    pub payload: RpcErrorPayload,
}

impl fmt::Display for RpcFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "service error {}: {}",
            self.code, self.payload.message
        )
    }
}

impl RpcFailure {
    /// Construct a message-only typed failure.
    pub fn new(code: RpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            payload: RpcErrorPayload {
                message: message.into(),
                fields: RpcErrorFields::default(),
            },
        }
    }

    /// Attach the RPC method involved in this failure.
    pub fn with_method(mut self, method: impl Into<String>) -> Self {
        self.payload.fields.method = Some(method.into());
        self
    }

    /// Attach the expected request input.
    pub fn with_expected(mut self, expected: impl Into<String>) -> Self {
        self.payload.fields.expected = Some(expected.into());
        self
    }

    /// Attach the key identifier involved in this failure.
    pub fn with_ident(mut self, ident: impl Into<String>) -> Self {
        self.payload.fields.ident = Some(ident.into());
        self
    }
}

/// Error returned while decoding a Hotki service error.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RpcErrorDecodeError {
    /// Service name is not a recognized Hotki RPC error code.
    #[error("unknown RPC error code '{0}'")]
    UnknownCode(String),
    /// Service payload is not a map.
    #[error("RPC error payload must be a map")]
    PayloadType,
    /// Required message field is absent.
    #[error("RPC error payload is missing message")]
    MissingMessage,
    /// A known payload field has the wrong type.
    #[error("RPC error payload field '{0}' must be a UTF-8 string")]
    FieldType(&'static str),
    /// Structured fields are not encoded as a map.
    #[error("RPC error payload fields must be a map")]
    FieldsType,
}

/// Encode a protocol-owned failure into the MRPC service-error envelope.
pub fn encode_rpc_failure(failure: RpcFailure) -> RpcError {
    let mut fields = Vec::new();
    push_string_field(&mut fields, "method", failure.payload.fields.method);
    push_string_field(&mut fields, "expected", failure.payload.fields.expected);
    push_string_field(&mut fields, "ident", failure.payload.fields.ident);
    RpcError::Service(ServiceError {
        name: failure.code.as_str().to_string(),
        value: Value::Map(vec![
            (
                Value::String("message".into()),
                Value::String(failure.payload.message.into()),
            ),
            (Value::String("fields".into()), Value::Map(fields)),
        ]),
    })
}

/// Decode a Hotki failure from an MRPC service-error envelope.
pub fn decode_rpc_failure(service: &ServiceError) -> Result<RpcFailure, RpcErrorDecodeError> {
    let code = RpcErrorCode::from_str(&service.name)?;
    let Value::Map(payload) = &service.value else {
        return Err(RpcErrorDecodeError::PayloadType);
    };
    let message = required_string(payload, "message")?;
    let fields = match map_value(payload, "fields") {
        None => RpcErrorFields::default(),
        Some(Value::Map(fields)) => RpcErrorFields {
            method: optional_string(fields, "method")?,
            expected: optional_string(fields, "expected")?,
            ident: optional_string(fields, "ident")?,
        },
        Some(_) => return Err(RpcErrorDecodeError::FieldsType),
    };
    Ok(RpcFailure {
        code,
        payload: RpcErrorPayload { message, fields },
    })
}

/// Append one optional string field to an MRPC map.
fn push_string_field(fields: &mut Vec<(Value, Value)>, name: &str, value: Option<String>) {
    if let Some(value) = value {
        fields.push((Value::String(name.into()), Value::String(value.into())));
    }
}

/// Find a named value in an MRPC map.
fn map_value<'a>(map: &'a [(Value, Value)], name: &str) -> Option<&'a Value> {
    map.iter()
        .find(|(key, _)| key.as_str() == Some(name))
        .map(|(_, value)| value)
}

/// Decode one required string field from an MRPC map.
fn required_string(
    map: &[(Value, Value)],
    name: &'static str,
) -> Result<String, RpcErrorDecodeError> {
    match map_value(map, name) {
        Some(value) => value
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or(RpcErrorDecodeError::FieldType(name)),
        None if name == "message" => Err(RpcErrorDecodeError::MissingMessage),
        None => Err(RpcErrorDecodeError::FieldType(name)),
    }
}

/// Decode one optional string field from an MRPC map.
fn optional_string(
    map: &[(Value, Value)],
    name: &'static str,
) -> Result<Option<String>, RpcErrorDecodeError> {
    map_value(map, name)
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or(RpcErrorDecodeError::FieldType(name))
        })
        .transpose()
}

/// RPC request methods supported by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyMethod {
    /// Request a server shutdown.
    Shutdown,
    /// Set the configuration path (server loads config from disk).
    SetConfigPath,
    /// Inject a synthetic key event.
    InjectKey,
    /// Get the current key bindings.
    GetBindings,
    /// Get the current stack depth.
    GetDepth,
    /// Get the current world status.
    GetWorldStatus,
    /// Get the server status.
    GetServerStatus,
    /// Get the world snapshot (focus + displays).
    GetWorldSnapshot,
}

impl HotkeyMethod {
    /// Stable string name for the method when talking to MRPC.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Shutdown => "shutdown",
            Self::SetConfigPath => "set_config_path",
            Self::InjectKey => "inject_key",
            Self::GetBindings => "get_bindings",
            Self::GetDepth => "get_depth",
            Self::GetWorldStatus => "get_world_status",
            Self::GetServerStatus => "get_server_status",
            Self::GetWorldSnapshot => "get_world_snapshot",
        }
    }

    /// Parse a method name received over MRPC.
    pub fn try_from_str(s: &str) -> Option<Self> {
        match s {
            "shutdown" => Some(Self::Shutdown),
            "set_config_path" => Some(Self::SetConfigPath),
            "inject_key" => Some(Self::InjectKey),
            "get_bindings" => Some(Self::GetBindings),
            "get_depth" => Some(Self::GetDepth),
            "get_world_status" => Some(Self::GetWorldStatus),
            "get_server_status" => Some(Self::GetServerStatus),
            "get_world_snapshot" => Some(Self::GetWorldSnapshot),
            _ => None,
        }
    }
}

/// One-way server→client notification channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyNotification {
    /// Generic notification channel.
    Notify,
}

impl HotkeyNotification {
    /// Stable string name for the notification channel.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Notify => "notify",
        }
    }
}

/// Lightweight server status snapshot surfaced for smoketest diagnostics.
///
/// Field names use `#[serde(rename)]` to emit compact diagnostics while keeping
/// descriptive Rust identifiers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerStatusLite {
    /// Idle timeout configured on the server, in seconds.
    #[serde(rename = "timeout_secs")]
    pub idle_timeout_secs: u64,
    /// True when the idle timer is currently armed.
    #[serde(rename = "armed")]
    pub idle_timer_armed: bool,
    /// Optional wall-clock deadline in milliseconds since the Unix epoch.
    #[serde(rename = "deadline_ms")]
    pub idle_deadline_ms: Option<u64>,
    /// Count of connected clients observed by the server.
    pub clients_connected: usize,
}

/// Lightweight snapshot payload for `get_world_snapshot` method (focus + displays only).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct WorldSnapshotLite {
    /// Focused context, if any.
    pub focused: Option<FocusSnapshot>,
    /// Display snapshot for placement decisions.
    pub displays: DisplaysSnapshot,
}

/// Inject key request: encoded as msgpack in a single Binary param.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InjectKeyReq {
    /// The key chord identifier (e.g., "cmd+c").
    pub ident: String,
    /// The action to perform (up/down).
    pub kind: InjectKind,
    /// Whether to simulate a key repeat.
    #[serde(default)]
    pub repeat: bool,
}

/// The kind of key injection to perform.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InjectKind {
    /// Key down event.
    Down,
    /// Key up event.
    Up,
}

#[cfg(test)]
mod tests {
    use mrpc::{ServiceError, Value};

    use super::{
        RpcErrorCode, RpcErrorDecodeError, RpcFailure, decode_rpc_failure, encode_rpc_failure,
    };

    #[test]
    fn rpc_error_codes_round_trip_stable_names() {
        for code in [
            RpcErrorCode::ShuttingDown,
            RpcErrorCode::MissingParams,
            RpcErrorCode::InvalidType,
            RpcErrorCode::InvalidConfig,
            RpcErrorCode::MethodNotFound,
            RpcErrorCode::EngineSetConfig,
            RpcErrorCode::KeyNotBound,
            RpcErrorCode::EngineDispatch,
        ] {
            assert_eq!(RpcErrorCode::from_service_name(code.as_str()), Some(code));
        }
        assert_eq!(RpcErrorCode::from_service_name("Other"), None);
    }

    #[test]
    fn rpc_failures_round_trip_message_and_fields() {
        let failure = RpcFailure::new(RpcErrorCode::KeyNotBound, "key is not bound: cmd+k")
            .with_method("inject_key")
            .with_expected("bound key")
            .with_ident("cmd+k");
        let mrpc::RpcError::Service(encoded) = encode_rpc_failure(failure.clone()) else {
            panic!("expected service error");
        };

        assert_eq!(decode_rpc_failure(&encoded), Ok(failure));
    }

    #[test]
    fn rpc_failures_round_trip_message_only() {
        let failure = RpcFailure::new(RpcErrorCode::ShuttingDown, "server is shutting down");
        let mrpc::RpcError::Service(encoded) = encode_rpc_failure(failure.clone()) else {
            panic!("expected service error");
        };

        assert_eq!(decode_rpc_failure(&encoded), Ok(failure));
    }

    #[test]
    fn rpc_failure_decode_rejects_unknown_and_malformed_payloads() {
        let unknown = ServiceError {
            name: "Other".to_string(),
            value: Value::Map(Vec::new()),
        };
        assert_eq!(
            decode_rpc_failure(&unknown),
            Err(RpcErrorDecodeError::UnknownCode("Other".to_string()))
        );

        let missing_message = ServiceError {
            name: RpcErrorCode::InvalidType.as_str().to_string(),
            value: Value::Map(Vec::new()),
        };
        assert_eq!(
            decode_rpc_failure(&missing_message),
            Err(RpcErrorDecodeError::MissingMessage)
        );

        let malformed_fields = ServiceError {
            name: RpcErrorCode::InvalidType.as_str().to_string(),
            value: Value::Map(vec![
                (
                    Value::String("message".into()),
                    Value::String("bad input".into()),
                ),
                (Value::String("fields".into()), Value::Boolean(false)),
            ]),
        };
        assert_eq!(
            decode_rpc_failure(&malformed_fields),
            Err(RpcErrorDecodeError::FieldsType)
        );
    }
}
