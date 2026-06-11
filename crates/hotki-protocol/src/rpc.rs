//! Typed RPC definitions for the Hotki protocol.
//!
//! This module defines the method names, request/response structures, and
//! notification types used by the Hotki server and client.

use serde::{Deserialize, Serialize};

use crate::{DisplaysSnapshot, FocusSnapshot};

/// RPC request methods supported by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyMethod {
    /// Request a server shutdown.
    Shutdown,
    /// Set the configuration path (server loads config from disk).
    SetConfigPath,
    /// Set the active theme by name.
    SetTheme,
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
            Self::SetTheme => "set_theme",
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
            "set_theme" => Some(Self::SetTheme),
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
