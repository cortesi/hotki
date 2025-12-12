//! Display geometry helpers shared by the UI layer.

use hotki_protocol::{DisplayFrame, DisplaysSnapshot};

/// Default fallback display frame when the world has not reported displays yet.
const DEFAULT_FRAME: DisplayFrame = DisplayFrame {
    id: 0,
    x: 0.0,
    y: 0.0,
    width: 1440.0,
    height: 900.0,
};

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
        Self {
            active: snapshot.active,
            frames: snapshot.displays.clone(),
            global_top: snapshot.global_top,
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

    /// Convert a bottom-left origin `y` plus `height` into a top-left origin `y`.
    ///
    /// AppKit/CoreGraphics use bottom-left origins; egui/winit expect top-left.
    #[must_use]
    pub fn to_top_left_y(&self, y_bottom: f32, height: f32) -> f32 {
        self.global_top() - (y_bottom + height)
    }

    /// Top-left origin `y` coordinate of the active display's top edge.
    #[must_use]
    pub fn active_frame_top_left_y(&self) -> f32 {
        let frame = self.active_frame();
        self.to_top_left_y(frame.y, frame.height)
    }

    /// Convert a top-left origin `y` plus `height` into a bottom-left origin `y`.
    #[must_use]
    pub fn to_bottom_left_y(&self, y_top: f32, height: f32) -> f32 {
        self.global_top() - y_top - height
    }

    /// Active screen frame as `(x, y, width, height, global_top)`.
    ///
    /// Coordinates follow AppKit semantics:
    /// - `(x, y, width, height)` are in bottom-left origin space for the active screen.
    /// - `global_top` is the maximum top Y across all screens, used to convert to
    ///   top-left coordinates expected by winit/egui.
    #[must_use]
    pub fn active_screen_frame(&self) -> (f32, f32, f32, f32, f32) {
        let frame = self.active_frame();
        (
            frame.x,
            frame.y,
            frame.width,
            frame.height,
            self.global_top(),
        )
    }
}
