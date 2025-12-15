//! Hotki protocol types for client/server IPC and UI integration.
//!
//! This crate defines the serializable message types and supporting
//! structures that the backend server and the UI exchange.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

use mac_keycode::Chord;
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

/// Display mode selection for the HUD.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Full HUD is visible.
    Hud,
    /// HUD is hidden.
    Hide,
    /// Minimal HUD variant.
    Mini,
}

/// Font weight used throughout UI elements.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FontWeight {
    /// Thin weight.
    Thin,
    /// Extra-light weight.
    ExtraLight,
    /// Light weight.
    Light,
    /// Regular weight.
    Regular,
    /// Medium weight.
    Medium,
    /// Semi-bold weight.
    SemiBold,
    /// Bold weight.
    Bold,
    /// Extra-bold weight.
    ExtraBold,
    /// Black weight.
    Black,
}

/// Screen anchor position for HUD placement.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Pos {
    /// Center of the active display.
    Center,
    /// North (top center).
    N,
    /// Northeast (top right).
    NE,
    /// East (right center).
    E,
    /// Southeast (bottom right).
    SE,
    /// South (bottom center).
    S,
    /// Southwest (bottom left).
    SW,
    /// West (left center).
    W,
    /// Northwest (top left).
    NW,
}

/// Pixel offset relative to an anchor position (x moves right, y moves up).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Offset {
    /// Horizontal offset in pixels.
    pub x: f32,
    /// Vertical offset in pixels.
    pub y: f32,
}

/// Side of the screen used to stack notifications.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NotifyPos {
    /// Left side of the active display.
    #[serde(alias = "l")]
    Left,
    /// Right side of the active display.
    #[serde(alias = "r")]
    Right,
}

/// Concrete per-window styling with fully parsed colors and sizes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NotifyWindowStyle {
    /// Background fill color.
    pub bg: (u8, u8, u8),
    /// Foreground color for the notification title text.
    pub title_fg: (u8, u8, u8),
    /// Foreground color for the notification body text.
    pub body_fg: (u8, u8, u8),
    /// Title font size.
    pub title_font_size: f32,
    /// Title font weight.
    pub title_font_weight: FontWeight,
    /// Body font size.
    pub body_font_size: f32,
    /// Body font weight.
    pub body_font_weight: FontWeight,
    /// Optional icon/glyph to show next to the title.
    pub icon: Option<String>,
}

/// Fully resolved notification theme for all kinds (info/warn/error/success).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NotifyTheme {
    /// Styling for Info notifications.
    pub info: NotifyWindowStyle,
    /// Styling for Warn notifications.
    pub warn: NotifyWindowStyle,
    /// Styling for Error notifications.
    pub error: NotifyWindowStyle,
    /// Styling for Success notifications.
    pub success: NotifyWindowStyle,
}

impl NotifyTheme {
    /// Pick the appropriate window style for a given notification kind.
    pub fn style_for(&self, kind: NotifyKind) -> &NotifyWindowStyle {
        match kind {
            NotifyKind::Info | NotifyKind::Ignore => &self.info,
            NotifyKind::Warn => &self.warn,
            NotifyKind::Error => &self.error,
            NotifyKind::Success => &self.success,
        }
    }
}

/// Fully resolved notification configuration (layout + per-kind styling).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NotifyConfig {
    /// Fixed width in pixels for each notification window.
    pub width: f32,
    /// Screen side where the notification stack is anchored (left or right).
    pub pos: NotifyPos,
    /// Overall window opacity in the range [0.0, 1.0].
    pub opacity: f32,
    /// Auto-dismiss timeout for a notification, in seconds.
    pub timeout: f32,
    /// Maximum number of notifications kept in the on-screen stack.
    pub buffer: usize,
    /// Corner radius for notification windows.
    pub radius: f32,
    /// Resolved per-kind styling.
    pub theme: NotifyTheme,
}

/// HUD style configuration with parsed colors and typography settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HudStyle {
    /// Display mode selection for the HUD.
    pub mode: Mode,
    /// Screen anchor position for the HUD window.
    pub pos: Pos,
    /// Pixel offset added to the anchored position.
    pub offset: Offset,
    /// Base font size for descriptions and general HUD text.
    pub font_size: f32,
    /// Font weight for title/description text.
    pub title_font_weight: FontWeight,
    /// Font size for key tokens inside their rounded boxes.
    pub key_font_size: f32,
    /// Font weight for non-modifier key tokens.
    pub key_font_weight: FontWeight,
    /// Font size for the tag indicator shown for sub-modes.
    pub tag_font_size: f32,
    /// Font weight for the sub-mode tag indicator.
    pub tag_font_weight: FontWeight,
    /// Foreground color for title/description text.
    pub title_fg: (u8, u8, u8),
    /// HUD background fill color.
    pub bg: (u8, u8, u8),
    /// Foreground color for non-modifier key tokens.
    pub key_fg: (u8, u8, u8),
    /// Background color for non-modifier key tokens.
    pub key_bg: (u8, u8, u8),
    /// Foreground color for modifier key tokens.
    pub mod_fg: (u8, u8, u8),
    /// Font weight for modifier key tokens.
    pub mod_font_weight: FontWeight,
    /// Background color for modifier key tokens.
    pub mod_bg: (u8, u8, u8),
    /// Foreground color for the sub-mode tag indicator.
    pub tag_fg: (u8, u8, u8),
    /// Window opacity in the range [0.0, 1.0].
    pub opacity: f32,
    /// Corner radius for key boxes.
    pub key_radius: f32,
    /// Horizontal padding inside key boxes.
    pub key_pad_x: f32,
    /// Vertical padding inside key boxes.
    pub key_pad_y: f32,
    /// Corner radius for the HUD window itself.
    pub radius: f32,
    /// Text tag shown for sub-modes at the end of rows.
    pub tag_submenu: String,
}

/// Effective UI style state computed on the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Style {
    /// HUD style settings.
    pub hud: HudStyle,
    /// Notification style settings.
    pub notify: NotifyConfig,
}

/// Optional per-binding HUD style overrides after resolution.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HudRowStyle {
    /// Foreground color for non-modifier key tokens.
    pub key_fg: (u8, u8, u8),
    /// Background color for non-modifier key tokens.
    pub key_bg: (u8, u8, u8),
    /// Foreground color for modifier key tokens.
    pub mod_fg: (u8, u8, u8),
    /// Background color for modifier key tokens.
    pub mod_bg: (u8, u8, u8),
    /// Foreground color for the mode tag token.
    pub tag_fg: (u8, u8, u8),
}

/// One HUD row entry produced by server-side rendering.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HudRow {
    /// Key chord that triggers the binding.
    pub chord: Chord,
    /// Human-readable description.
    pub desc: String,
    /// True when the binding enters a child mode.
    pub is_mode: bool,
    /// Optional per-row style overrides.
    pub style: Option<HudRowStyle>,
}

/// HUD snapshot pushed from the server to the UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HudState {
    /// Whether the HUD should be visible.
    pub visible: bool,
    /// Rows to display.
    pub rows: Vec<HudRow>,
    /// Current stack depth (root = 0).
    pub depth: usize,
    /// Mode titles from root→current (excluding the synthetic root frame).
    pub breadcrumbs: Vec<String>,
    /// Effective computed style (HUD + notifications).
    pub style: Style,
    /// True when capture-all mode is active.
    pub capture: bool,
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

/// Rectangular bounds for a display in bottom-left origin coordinates.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DisplayFrame {
    /// CoreGraphics display identifier (`CGDirectDisplayID`).
    pub id: u32,
    /// Horizontal origin in pixels.
    pub x: f32,
    /// Vertical origin in pixels.
    pub y: f32,
    /// Width in pixels.
    pub width: f32,
    /// Height in pixels.
    pub height: f32,
}

impl DisplayFrame {
    /// Upper edge (`y + height`) in bottom-left origin coordinates.
    #[must_use]
    pub fn top(&self) -> f32 {
        self.y + self.height
    }
}

/// Snapshot describing active/visible displays for HUD/layout decisions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct DisplaysSnapshot {
    /// Maximum top Y across all displays (used for top-left conversions).
    pub global_top: f32,
    /// Active display chosen for anchoring, if known.
    pub active: Option<DisplayFrame>,
    /// All displays currently tracked.
    pub displays: Vec<DisplayFrame>,
}

/// Streamed world events from the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorldStreamMsg {
    /// Focus changed to the provided context. `None` when no focused window.
    FocusChanged(Option<App>),
}

/// IPC-related helpers: channel aliases and message codec.
pub mod ipc {
    use super::MsgToUI;

    /// Default capacity for the bounded UI event pipeline.
    /// Large enough to absorb short spikes without unbounded growth.
    pub const DEFAULT_UI_CHANNEL_CAPACITY: usize = 10_000;

    /// Tokio bounded sender for UI messages.
    pub type UiTx = tokio::sync::mpsc::Sender<MsgToUI>;
    /// Tokio bounded receiver for UI messages.
    pub type UiRx = tokio::sync::mpsc::Receiver<MsgToUI>;

    /// Create the standard bounded UI channel (sender, receiver).
    pub fn ui_channel() -> (UiTx, UiRx) {
        tokio::sync::mpsc::channel::<MsgToUI>(DEFAULT_UI_CHANNEL_CAPACITY)
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

/// Typed RPC definitions.
pub mod rpc;

/// Messages sent from the server to UI clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MsgToUI {
    /// Asynchronous event sent when a hotkey is triggered.
    HotkeyTriggered(String),

    /// HUD update containing the fully rendered state.
    HudUpdate {
        /// HUD state snapshot.
        hud: Box<HudState>,
        /// Display geometry snapshot for UI placement.
        displays: DisplaysSnapshot,
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

    /// Server loaded a new config from disk and is notifying clients.
    ///
    /// The config payload is msgpack-encoded `config::Config` bytes.
    ConfigLoaded {
        /// The config path the server loaded.
        path: String,
        /// The msgpack-encoded config payload.
        config: Vec<u8>,
    },

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyKind {
    /// Informational notification.
    Info,
    /// Warning notification.
    Warn,
    /// Error notification.
    Error,
    /// Success/affirmation notification.
    Success,
    /// Ignore output; produce no notification.
    Ignore,
}
