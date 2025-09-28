//! Display geometry helpers shared by the UI layer.

use hotki_protocol::{DisplayRect, DisplaysSnapshot};

/// Default fallback display frame when the world has not reported displays yet.
const DEFAULT_FRAME: DisplayFrame = DisplayFrame {
    id: 0,
    x: 0.0,
    y: 0.0,
    width: 1440.0,
    height: 900.0,
};

/// Rectangular bounds for a display in bottom-left origin coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DisplayFrame {
    /// CoreGraphics display identifier (`CGDirectDisplayID`).
    pub id: u32,
    /// Horizontal origin in pixels (bottom-left origin).
    pub x: f32,
    /// Vertical origin in pixels (bottom-left origin).
    pub y: f32,
    /// Display width in pixels.
    pub width: f32,
    /// Display height in pixels.
    pub height: f32,
}

impl DisplayFrame {
    /// Top edge (`y + height`) expressed in bottom-left coordinates.
    #[must_use]
    pub fn top(&self) -> f32 {
        self.y + self.height
    }
}

impl From<&DisplayRect> for DisplayFrame {
    fn from(rect: &DisplayRect) -> Self {
        Self {
            id: rect.id,
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }
}

/// Snapshot of display geometry used for HUD, notification, and details placement.
#[derive(Clone, Debug, Default)]
pub struct DisplayMetrics {
    /// Active display selected for placement, if any.
    active: Option<DisplayFrame>,
    /// All visible display frames.
    frames: Vec<DisplayFrame>,
    /// Maximum top Y across all displays.
    global_top: f32,
}

impl DisplayMetrics {
    /// Construct metrics from a serialized snapshot.
    #[must_use]
    pub fn from_snapshot(snapshot: &DisplaysSnapshot) -> Self {
        let frames: Vec<DisplayFrame> = snapshot.displays.iter().map(DisplayFrame::from).collect();
        let active = snapshot.active.as_ref().map(DisplayFrame::from);
        let global_top = snapshot.global_top;
        Self {
            active,
            frames,
            global_top,
        }
    }

    /// Active display frame, falling back to `DEFAULT_FRAME` when unknown.
    #[must_use]
    pub fn active_frame(&self) -> DisplayFrame {
        self.active
            .or_else(|| self.frames.first().copied())
            .unwrap_or(DEFAULT_FRAME)
    }

    /// Maximum top Y coordinate for converting to top-left origin.
    #[must_use]
    pub fn global_top(&self) -> f32 {
        if self.global_top.is_finite() && self.global_top > 0.0 {
            self.global_top
        } else {
            self.active_frame().top()
        }
    }
}
