//! Display geometry helpers shared by the UI layer.

use egui::{Pos2, Vec2, pos2, vec2};
use hotki_protocol::{DisplayFrame, DisplaysSnapshot, NotifyPos, Offset, Pos};

/// Default fallback display frame when the world has not reported displays yet.
const DEFAULT_FRAME: DisplayFrame = DisplayFrame {
    id: 0,
    x: 0.0,
    y: 0.0,
    width: 1440.0,
    height: 900.0,
};

/// Snapshot of display geometry used for HUD, notification, and main-window placement.
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
        self.active_frame_or_default()
    }

    /// Active display bounds with the global coordinate conversion anchor.
    #[must_use]
    pub fn active_bounds(&self) -> DisplayBounds {
        DisplayBounds {
            frame: self.active_frame_or_default(),
            global_top: self.global_top(),
        }
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

    /// Active display frame without recomputing bounds.
    fn active_frame_or_default(&self) -> DisplayFrame {
        self.active
            .or_else(|| self.frames.first().copied())
            .unwrap_or(DEFAULT_FRAME)
    }
}

/// Active display geometry plus the global top edge used for coordinate conversion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayBounds {
    /// Active display frame in bottom-left origin coordinates.
    frame: DisplayFrame,
    /// Maximum top edge across displays.
    global_top: f32,
}

impl DisplayBounds {
    /// Active display frame in bottom-left origin coordinates.
    #[must_use]
    pub fn frame(self) -> DisplayFrame {
        self.frame
    }

    /// Convert a bottom-left origin `y` plus `height` into a top-left origin `y`.
    #[must_use]
    pub fn to_top_left_y(self, y_bottom: f32, height: f32) -> f32 {
        self.global_top - (y_bottom + height)
    }

    /// Convert a top-left origin `y` plus `height` into a bottom-left origin `y`.
    #[must_use]
    pub fn to_bottom_left_y(self, y_top: f32, height: f32) -> f32 {
        self.global_top - y_top - height
    }

    /// Top-left origin `y` coordinate of the active display's top edge.
    #[must_use]
    pub fn top_left_y(self) -> f32 {
        self.to_top_left_y(self.frame.y, self.frame.height)
    }

    /// Active display bounds as a top-left-origin rectangle.
    #[must_use]
    pub fn top_left_rect(self) -> WindowGeometry {
        WindowGeometry::new(
            pos2(self.frame.x, self.top_left_y()),
            vec2(self.frame.width, self.frame.height),
        )
    }

    /// Clamp a top-left-origin geometry to the active display.
    #[must_use]
    pub fn clamp_geometry(self, geometry: WindowGeometry, min_size: Vec2) -> WindowGeometry {
        let screen = self.top_left_rect();
        let width = geometry.size.x.max(min_size.x).min(screen.size.x);
        let height = geometry.size.y.max(min_size.y).min(screen.size.y);
        let max_x = (screen.pos.x + screen.size.x - width).max(screen.pos.x);
        let max_y = (screen.pos.y + screen.size.y - height).max(screen.pos.y);

        WindowGeometry::new(
            pos2(
                geometry.pos.x.clamp(screen.pos.x, max_x),
                geometry.pos.y.clamp(screen.pos.y, max_y),
            ),
            vec2(width, height),
        )
    }

    /// Center a window on the active display in top-left-origin coordinates.
    #[must_use]
    pub fn centered_geometry(self, size: Vec2) -> WindowGeometry {
        let screen = self.top_left_rect();
        WindowGeometry::new(
            pos2(
                screen.pos.x + (screen.size.x - size.x) / 2.0,
                screen.pos.y + (screen.size.y - size.y) / 2.0,
            ),
            size,
        )
    }

    /// Position a window at a named display anchor and clamp it to the display.
    #[must_use]
    pub fn anchored_geometry(
        self,
        anchor: Pos,
        size: Vec2,
        margin: f32,
        offset: Offset,
    ) -> WindowGeometry {
        let size = vec2(size.x.max(1.0), size.y.max(1.0));
        let frame = self.frame;
        let (x_bottom, y_bottom) = match anchor {
            Pos::N => (
                frame.x + (frame.width - size.x) / 2.0,
                frame.y + frame.height - size.y - margin,
            ),
            Pos::NE => (
                frame.x + frame.width - size.x - margin,
                frame.y + frame.height - size.y - margin,
            ),
            Pos::E => (
                frame.x + frame.width - size.x - margin,
                frame.y + (frame.height - size.y) / 2.0,
            ),
            Pos::SE => (frame.x + frame.width - size.x - margin, frame.y + margin),
            Pos::S => (frame.x + (frame.width - size.x) / 2.0, frame.y + margin),
            Pos::SW => (frame.x + margin, frame.y + margin),
            Pos::W => (frame.x + margin, frame.y + (frame.height - size.y) / 2.0),
            Pos::NW => (frame.x + margin, frame.y + frame.height - size.y - margin),
            Pos::Center => (
                frame.x + (frame.width - size.x) / 2.0,
                frame.y + (frame.height - size.y) / 2.0,
            ),
        };
        let geometry = WindowGeometry::new(
            pos2(
                x_bottom + offset.x,
                self.to_top_left_y(y_bottom, size.y) + offset.y,
            ),
            size,
        );
        self.clamp_geometry(geometry, vec2(1.0, 1.0))
    }

    /// Compute the left edge for a notification stack on the active display.
    #[must_use]
    pub fn notification_x(self, side: NotifyPos, width: f32, margin: f32) -> f32 {
        let min_x = self.frame.x + margin;
        let max_x = (self.frame.x + self.frame.width - width - margin).max(min_x);
        match side {
            NotifyPos::Left => min_x,
            NotifyPos::Right => max_x,
        }
    }

    /// Return true if a top-left geometry is at least partly above the active display bottom.
    #[must_use]
    pub fn is_visible_vertically(self, geometry: WindowGeometry) -> bool {
        self.to_bottom_left_y(geometry.pos.y, geometry.size.y) >= self.frame.y
    }
}

/// Window geometry in top-left-origin coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WindowGeometry {
    /// Top-left window position.
    pub pos: Pos2,
    /// Inner window size.
    pub size: Vec2,
}

impl WindowGeometry {
    /// Construct a top-left-origin geometry.
    #[must_use]
    pub fn new(pos: Pos2, size: Vec2) -> Self {
        Self { pos, size }
    }

    /// Construct geometry from an AppKit bottom-left-origin frame.
    #[must_use]
    pub fn from_bottom_left_frame(bounds: DisplayBounds, x: f32, y: f32, w: f32, h: f32) -> Self {
        Self::new(pos2(x, bounds.to_top_left_y(y, h)), vec2(w, h))
    }
}

#[cfg(test)]
mod tests {
    use egui::{pos2, vec2};
    use hotki_protocol::{DisplayFrame, DisplaysSnapshot, NotifyPos, Offset, Pos};

    use super::{DisplayMetrics, WindowGeometry};

    fn metrics() -> DisplayMetrics {
        DisplayMetrics::from_snapshot(&DisplaysSnapshot {
            global_top: 1000.0,
            active: Some(DisplayFrame {
                id: 7,
                x: 100.0,
                y: 100.0,
                width: 800.0,
                height: 600.0,
            }),
            displays: Vec::new(),
        })
    }

    #[test]
    fn anchored_geometry_uses_top_left_coordinates_and_clamps_to_display() {
        let bounds = metrics().active_bounds();

        let placed = bounds.anchored_geometry(
            Pos::SE,
            vec2(120.0, 80.0),
            12.0,
            Offset { x: 20.0, y: 30.0 },
        );

        assert_eq!(placed.pos, pos2(780.0, 820.0));
        assert_eq!(placed.size, vec2(120.0, 80.0));
    }

    #[test]
    fn clamp_geometry_limits_size_and_position_to_active_display() {
        let bounds = metrics().active_bounds();
        let geometry = WindowGeometry::new(pos2(-50.0, 50.0), vec2(2000.0, 10.0));

        let clamped = bounds.clamp_geometry(geometry, vec2(100.0, 80.0));

        assert_eq!(clamped.pos, pos2(100.0, 300.0));
        assert_eq!(clamped.size, vec2(800.0, 80.0));
    }

    #[test]
    fn notification_x_collapses_when_width_exceeds_display() {
        let bounds = metrics().active_bounds();

        assert_eq!(bounds.notification_x(NotifyPos::Left, 1000.0, 12.0), 112.0);
        assert_eq!(bounds.notification_x(NotifyPos::Right, 1000.0, 12.0), 112.0);
    }

    #[test]
    fn notification_x_honors_requested_side() {
        let bounds = metrics().active_bounds();

        assert_eq!(bounds.notification_x(NotifyPos::Left, 320.0, 12.0), 112.0);
        assert_eq!(bounds.notification_x(NotifyPos::Right, 320.0, 12.0), 568.0);
    }
}
