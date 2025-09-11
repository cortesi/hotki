//! Hotki protocol types for client/server IPC and UI integration.
//!
//! This crate defines the serializable message types and supporting
//! structures that the backend server and the UI exchange.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

use serde::{Deserialize, Serialize};

/// Focused application context used by UI/HUD rendering.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct App {
    /// Application name (e.g., "Safari").
    pub app: String,
    /// Active window title for the focused app.
    pub title: String,
    /// Process identifier for the focused app.
    pub pid: i32,
}

/// Pointer into the loaded config's key hierarchy and UI overrides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    /// Indices into the parent `Keys.keys` vector for each descent step.
    path: Vec<u32>,

    /// True when showing the root HUD via a root view (no logical descent).
    #[serde(default)]
    pub viewing_root: bool,

    /// Optional override of the base theme name for this view.
    /// When `None`, uses the theme bundled in the loaded config.
    #[serde(default)]
    pub override_theme: Option<String>,

    /// When true, ignore user overlay and render the theme without user UI tweaks.
    #[serde(default)]
    pub user_ui_disabled: bool,

    /// Optional focused application context carried with the cursor for UI/HUD
    /// rendering. When absent, callers may fall back to empty strings.
    #[serde(default)]
    pub app: Option<App>,
}

impl Cursor {
    /// Construct a new Cursor from parts.
    pub fn new(path: Vec<u32>, viewing_root: bool) -> Self {
        Self {
            path,
            viewing_root,
            override_theme: None,
            user_ui_disabled: false,
            app: None,
        }
    }

    /// Logical depth equals the number of elements in the path (root = 0).
    pub fn depth(&self) -> usize {
        self.path.len()
    }

    /// Push an index step into the location path.
    pub fn push(&mut self, idx: u32) {
        self.path.push(idx);
    }

    /// Pop a step from the location path. Returns the popped index if any.
    pub fn pop(&mut self) -> Option<u32> {
        self.path.pop()
    }

    /// Clear the path, returning to root (does not change viewing_root flag).
    pub fn clear(&mut self) {
        self.path.clear();
    }

    /// Borrow the immutable path for inspection/logging.
    pub fn path(&self) -> &[u32] {
        &self.path
    }

    /// Attach an App context to this cursor and return it.
    pub fn with_app(mut self, app: App) -> Self {
        self.app = Some(app);
        self
    }

    /// Borrow the App context if present.
    pub fn app_ref(&self) -> Option<&App> {
        self.app.as_ref()
    }

    /// Set a theme override for this location. Use `None` to fall back to the
    /// theme loaded from disk.
    pub fn set_theme(&mut self, name: Option<&str>) {
        self.override_theme = name.map(|s| s.to_string());
    }

    /// Clear any theme override at this location (revert to loaded theme).
    pub fn clear_theme(&mut self) {
        self.override_theme = None;
    }

    /// Enable or disable user style overlays at this location.
    ///
    /// - `true` enables user-provided overlays
    /// - `false` disables them (rendering the base theme only)
    pub fn set_user_style_enabled(&mut self, enabled: bool) {
        self.user_ui_disabled = !enabled;
    }

    /// Returns `true` when user style overlays are enabled at this location.
    pub fn user_style_enabled(&self) -> bool {
        !self.user_ui_disabled
    }
}

/// Three-state toggle used for boolean-like actions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Toggle {
    /// Set the option to enabled/on.
    On,
    /// Set the option to disabled/off.
    Off,
    /// Flip the current option state.
    Toggle,
}

/// Lightweight window representation for world event streaming.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorldWindowLite {
    /// Application name.
    pub app: String,
    /// Window title.
    pub title: String,
    /// Process id.
    pub pid: i32,
    /// Core Graphics window id (`kCGWindowNumber`).
    pub id: u32,
    /// Z-order index (0 = frontmost) within the current snapshot.
    pub z: u32,
    /// True if focused according to AX-preferred rule.
    pub focused: bool,
    /// Display identifier with the greatest overlap, if known.
    pub display_id: Option<u32>,
}

/// Streamed world events from the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorldStreamMsg {
    /// A new window was observed.
    Added(WorldWindowLite),
    /// A window disappeared.
    Removed {
        /// Process id of the removed window.
        pid: i32,
        /// Core Graphics window id of the removed window.
        id: u32,
    },
    /// A window was updated; deltas are elided.
    Updated {
        /// Process id of the updated window.
        pid: i32,
        /// Core Graphics window id of the updated window.
        id: u32,
    },
    /// Focus changed to the provided context. `None` when no focused window.
    FocusChanged(Option<App>),
    /// Recommended resync when server dropped events due to backpressure.
    ResyncRecommended,
}

/// IPC-related helpers: channel aliases and message codec.
pub mod ipc {
    use super::MsgToUI;

    /// Tokio unbounded sender for UI messages.
    pub type UiTx = tokio::sync::mpsc::UnboundedSender<MsgToUI>;
    /// Tokio unbounded receiver for UI messages.
    pub type UiRx = tokio::sync::mpsc::UnboundedReceiver<MsgToUI>;

    /// Create a standard unbounded UI channel (sender, receiver).
    pub fn ui_channel() -> (UiTx, UiRx) {
        tokio::sync::mpsc::unbounded_channel::<MsgToUI>()
    }

    /// Codec for encoding/decoding UI messages used by the IPC layer.
    pub mod codec;

    /// Heartbeat tuning parameters shared by client and server.
    ///
    /// - `interval()` is how often the server emits a heartbeat.
    /// - `timeout()` is how long the client waits without receiving any
    ///   message (including heartbeat) before assuming the server is gone.
    pub mod heartbeat {
        use std::time::Duration;

        /// Default server→client heartbeat interval.
        pub const INTERVAL_MS: u64 = 500;
        /// Default client tolerance before declaring the server dead.
        pub const TIMEOUT_MS: u64 = 2_000;

        /// Convenience accessor for the interval as a `Duration`.
        pub fn interval() -> Duration {
            Duration::from_millis(INTERVAL_MS)
        }

        /// Convenience accessor for the timeout as a `Duration`.
        pub fn timeout() -> Duration {
            Duration::from_millis(TIMEOUT_MS)
        }
    }
}

/// Messages sent from the server to UI clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MsgToUI {
    /// Asynchronous event sent when a hotkey is triggered.
    HotkeyTriggered(String),

    /// HUD update containing the current cursor (with optional App context)
    HudUpdate {
        /// Cursor state describing the current key mode and overrides.
        cursor: Cursor,
    },

    /// Notification request for the UI
    Notify {
        /// Notification kind (controls styling/severity).
        kind: NotifyKind,
        /// Notification title text.
        title: String,
        /// Notification body text.
        text: String,
    },

    /// Request the UI to reload the configuration from disk
    ReloadConfig,

    /// Clear notifications request for the UI
    ClearNotifications,

    /// Control the details window visibility
    ShowDetails(Toggle),

    /// Switch to the next theme
    ThemeNext,

    /// Switch to the previous theme
    ThemePrev,

    /// Set a specific theme by name
    ThemeSet(String),

    /// Control user style configuration (HUD and notifications): on/off/toggle
    UserStyle(Toggle),

    /// Streaming log message from the server
    Log {
        /// Log level string (e.g., "info").
        level: String,
        /// Log target/module.
        target: String,
        /// Rendered log message fields.
        message: String,
    },

    /// Server→client heartbeat. The payload is a monotonic milliseconds
    /// tick value from the server for debugging; the client treats any
    /// received message as liveness.
    Heartbeat(u64),

    /// World service streaming event.
    World(WorldStreamMsg),
}

/// Notification kinds supported by the UI.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum NotifyKind {
    /// Informational notification.
    Info,
    /// Warning notification.
    Warn,
    /// Error notification.
    Error,
    /// Success/affirmation notification.
    Success,
}
