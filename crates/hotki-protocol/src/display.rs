use serde::{Deserialize, Serialize};

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
