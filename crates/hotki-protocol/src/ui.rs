use mac_keycode::Chord;
use serde::{Deserialize, Serialize};

use crate::{display::DisplaysSnapshot, focus::FocusSnapshot, style::Style};

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
    /// Effective computed style (HUD + notifications + selector).
    pub style: Style,
    /// True when capture-all mode is active.
    pub capture: bool,
}

/// One selector item entry produced by server-side fuzzy matching.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SelectorItemSnapshot {
    /// Primary text displayed in the list.
    pub label: String,
    /// Optional secondary text displayed alongside the label.
    pub sublabel: Option<String>,
    /// Codepoint indices in `label` to highlight.
    pub label_match_indices: Vec<u32>,
}

/// Selector snapshot pushed from the server to the UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SelectorSnapshot {
    /// Title shown in the selector header.
    pub title: String,
    /// Placeholder shown when the query is empty.
    pub placeholder: String,
    /// Current query text.
    pub query: String,
    /// Visible items for the selector.
    pub items: Vec<SelectorItemSnapshot>,
    /// Index of the selected item within `items`.
    pub selected: usize,
    /// Total matched item count.
    pub total_matches: usize,
}

/// Three-state toggle used for boolean-like actions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Toggle {
    /// Set the option to enabled/on.
    On,
    /// Set the option to disabled/off.
    Off,
    /// Flip the current option state.
    Toggle,
}

/// Notification kinds supported by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// Streamed world events from the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorldStreamMsg {
    /// Focus changed to the provided context. `None` when no focused window.
    FocusChanged(Option<FocusSnapshot>),
}

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
    /// Show/update selector popup.
    SelectorUpdate(SelectorSnapshot),
    /// Hide selector popup.
    SelectorHide,
    /// Notification request for the UI.
    Notify {
        /// Notification kind (controls styling/severity).
        kind: NotifyKind,
        /// Notification title text.
        title: String,
        /// Notification body text.
        text: String,
    },
    /// Clear notifications request for the UI.
    ClearNotifications,
    /// Control the details window visibility.
    ShowDetails(Toggle),
    /// Streaming log message from the server.
    Log {
        /// Log level string (e.g., "info").
        level: String,
        /// Log target/module.
        target: String,
        /// Rendered log message fields.
        message: String,
    },
    /// Server→client heartbeat. The payload is a monotonic milliseconds tick.
    Heartbeat(u64),
    /// World service streaming event.
    World(WorldStreamMsg),
}
