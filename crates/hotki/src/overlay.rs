use std::hash::Hash;

use egui::{Context, Pos2, Vec2, ViewportBuilder, ViewportCommand, ViewportId};

use crate::display::DisplayMetrics;

/// Shared display metrics cache used by overlay windows and window collections.
#[derive(Debug, Clone, Default)]
pub struct OverlayMetrics {
    /// Cached display snapshot used for geometry decisions.
    display: DisplayMetrics,
}

impl OverlayMetrics {
    /// Access the cached display metrics.
    pub fn display(&self) -> &DisplayMetrics {
        &self.display
    }

    /// Replace the cached display metrics and return true when the active frame changed.
    pub fn set_display_metrics(&mut self, metrics: DisplayMetrics) -> bool {
        let changed = self.display.active_frame() != metrics.active_frame();
        self.display = metrics;
        changed
    }
}

/// Shared viewport state for a single overlay window.
#[derive(Debug, Clone)]
pub struct OverlayWindow {
    /// Stable viewport identifier for this overlay window.
    id: ViewportId,
    /// Cached display snapshot shared with the owner window/widget.
    metrics: OverlayMetrics,
    /// Last known outer position applied to the viewport.
    last_pos: Option<Pos2>,
    /// Last known inner size applied to the viewport.
    last_size: Option<Vec2>,
}

impl OverlayWindow {
    /// Create a new overlay window with a stable viewport id.
    pub fn new(id: impl Hash) -> Self {
        Self {
            id: ViewportId::from_hash_of(id),
            metrics: OverlayMetrics::default(),
            last_pos: None,
            last_size: None,
        }
    }

    /// Return the viewport identifier.
    pub fn id(&self) -> ViewportId {
        self.id
    }

    /// Access the cached display metrics.
    pub fn display(&self) -> &DisplayMetrics {
        self.metrics.display()
    }

    /// Reset cached geometry so the next render recomputes placement.
    pub fn reset_geometry(&mut self) {
        self.last_pos = None;
        self.last_size = None;
    }

    /// Hide the viewport immediately.
    pub fn hide(&mut self, ctx: &Context) {
        self.reset_geometry();
        ctx.send_viewport_cmd_to(self.id, ViewportCommand::Visible(false));
    }

    /// Update cached display metrics and return true when the active frame changed.
    pub fn set_display_metrics(&mut self, metrics: DisplayMetrics) -> bool {
        let changed = self.metrics.set_display_metrics(metrics);
        if changed {
            self.last_pos = None;
        }
        changed
    }

    /// Keep the cached viewport geometry in sync with the desired geometry.
    pub fn sync_builder(
        &self,
        ctx: &Context,
        mut builder: ViewportBuilder,
        pos: Pos2,
        size: Vec2,
    ) -> ViewportBuilder {
        if self.last_pos != Some(pos) {
            ctx.send_viewport_cmd_to(self.id, ViewportCommand::OuterPosition(pos));
        }
        if self.last_pos.is_none() {
            builder = builder.with_position(pos);
        }
        if self.last_size != Some(size) {
            ctx.send_viewport_cmd_to(self.id, ViewportCommand::InnerSize(size));
        }
        builder
    }

    /// Record the most recently applied geometry.
    pub fn record_geometry(&mut self, pos: Pos2, size: Vec2) {
        self.last_pos = Some(pos);
        self.last_size = Some(size);
    }
}
