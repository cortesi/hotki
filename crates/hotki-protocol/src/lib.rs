use serde::{Deserialize, Serialize};

/// Focused application context used by UI/HUD rendering.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct App {
    pub app: String,
    pub title: String,
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
}

impl Cursor {
    /// Construct a new Cursor from parts.
    pub fn new(path: Vec<u32>, viewing_root: bool) -> Self {
        Self {
            path,
            viewing_root,
            override_theme: None,
            user_ui_disabled: false,
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
    On,
    Off,
    Toggle,
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
}

/// Messages sent from the server to UI clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MsgToUI {
    /// Asynchronous event sent when a hotkey is triggered.
    HotkeyTriggered(String),

    /// HUD update containing the current cursor and focus snapshot
    HudUpdate { cursor: Cursor, focus: App },

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


#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum NotifyKind {
    Info,
    Warn,
    Error,
    Success,
}
