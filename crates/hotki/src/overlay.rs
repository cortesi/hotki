use egui::{Context, Pos2, Vec2, ViewportBuilder, ViewportCommand, ViewportId};

use crate::display::DisplayMetrics;

/// Hide a viewport immediately.
pub fn hide_viewport(ctx: &Context, id: ViewportId) {
    ctx.send_viewport_cmd_to(id, ViewportCommand::Visible(false));
}

/// Update display metrics and return true when the active frame changed.
pub fn update_display_metrics(display: &mut DisplayMetrics, metrics: DisplayMetrics) -> bool {
    let changed = display.active_frame() != metrics.active_frame();
    *display = metrics;
    changed
}

/// Keep a viewport's cached position and size in sync with the desired geometry.
pub fn sync_viewport_geometry(
    ctx: &Context,
    id: ViewportId,
    last_pos: &Option<Pos2>,
    last_size: &Option<Vec2>,
    mut builder: ViewportBuilder,
    pos: Pos2,
    size: Vec2,
) -> ViewportBuilder {
    if *last_pos != Some(pos) {
        ctx.send_viewport_cmd_to(id, ViewportCommand::OuterPosition(pos));
    }
    if last_pos.is_none() {
        builder = builder.with_position(pos);
    }
    if *last_size != Some(size) {
        ctx.send_viewport_cmd_to(id, ViewportCommand::InnerSize(size));
    }
    builder
}
