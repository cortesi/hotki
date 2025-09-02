use config::{Cursor, Toggle};
use serde::{Deserialize, Serialize};

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
}

/// Messages sent from the server to UI clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MsgToUI {
    /// Asynchronous event sent when a hotkey is triggered.
    HotkeyTriggered(String),

    /// HUD update containing the current cursor and focus snapshot
    HudUpdate { cursor: Cursor, focus: Focus },

    /// Notification request for the UI
    Notify {
        kind: NotifyKind,
        title: String,
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
        level: String,
        target: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Focus {
    pub app: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum NotifyKind {
    Info,
    Warn,
    Error,
    Success,
}
