use serde::{Deserialize, Serialize};

/// Focused application context used by UI/HUD rendering and world snapshots.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FocusSnapshot {
    /// Application name (e.g., "Safari").
    pub app: String,
    /// Active window title for the focused app.
    pub title: String,
    /// Process identifier for the focused app.
    pub pid: i32,
    /// Identifier of the display containing the focused window, if known.
    #[serde(default)]
    pub display_id: Option<u32>,
}
