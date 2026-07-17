//! Serializable physical-input health shared by server and UI.

use serde::{Deserialize, Serialize};

/// Whether the server observes the physical keyboard.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TapMode {
    /// A production CoreGraphics event tap is installed.
    Physical,
    /// Events can only arrive through the explicit injection API.
    #[default]
    InjectionOnly,
}

/// Current lifecycle of the server's physical event tap.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TapLifecycle {
    /// The event-tap thread is starting.
    Starting,
    /// The event tap and run loop are active.
    Running,
    /// No physical event tap is running.
    #[default]
    Stopped,
}

/// Last sampled state of macOS Secure Event Input.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SecureInputState {
    /// No production sample has been taken yet.
    #[default]
    Unknown,
    /// Secure Event Input was inactive at the last observation.
    Inactive,
    /// Secure Event Input was active at the last observation.
    Active,
}

/// Best-effort identity of the application owning Secure Event Input.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecureInputOwner {
    /// Process identifier reported by the current macOS session.
    pub pid: u32,
    /// AppKit localized application name resolved at observation time.
    pub app_name: String,
}

/// Complete server-owned physical-input health snapshot.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InputHealth {
    /// Whether this server has a physical event tap.
    pub tap_mode: TapMode,
    /// Current event-tap lifecycle.
    pub tap_lifecycle: TapLifecycle,
    /// Last sampled Secure Event Input state.
    pub secure_input: SecureInputState,
    /// Best-effort active owner identity.
    pub secure_input_owner: Option<SecureInputOwner>,
    /// Whether Secure Event Input currently blocks registered physical hotkeys.
    pub blocked: bool,
    /// Number of currently registered hotkeys.
    pub registered_hotkeys: usize,
    /// Number of physical key events observed by the tap.
    pub physical_event_count: u64,
    /// Age of the latest physical event at observation time.
    pub physical_event_age_ms: Option<u64>,
    /// Number of callbacks reporting that macOS disabled the tap.
    pub os_disable_count: u64,
    /// Number of successful tap re-enable checks.
    pub os_reenable_count: u64,
    /// Wall-clock time of the Secure Input observation.
    pub observed_at_ms: Option<u64>,
    /// PID of the server process that owns the event tap.
    pub server_pid: u32,
}

/// Structured server heartbeat carrying input health.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Heartbeat {
    /// Wall-clock send time in milliseconds since the Unix epoch.
    pub sent_at_ms: u64,
    /// Latest server-owned input-health snapshot.
    pub input: InputHealth,
}

impl Heartbeat {
    /// Construct a heartbeat from its send time and input snapshot.
    pub fn new(sent_at_ms: u64, input: InputHealth) -> Self {
        Self { sent_at_ms, input }
    }
}
