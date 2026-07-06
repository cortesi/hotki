//! Transient in-app notifications with stacking, animation, and theming.
use std::time::{Duration, Instant};

use egui::{Color32, Context, Frame, Pos2, Vec2, ViewportBuilder, pos2, text::LayoutJob};
use eguidev::{
    DevMcp, DevUiExt, WidgetMeta, WidgetRole, WidgetValue, container, track_response_full,
};
use hotki_protocol::{FontWeight, NotifyConfig, NotifyKind, NotifyPos, NotifyTheme};

use crate::{
    devtools,
    display::{DisplayBounds, DisplayMetrics, WindowGeometry},
    fonts, nswindow,
    overlay::{OverlayMetrics, OverlayWindow},
};

/// Duration for easing-based adjustment movements (seconds).
pub const ADJUST_MOVE_SECS: f32 = 0.25;
/// Screen edge margin for notification stacks.
const NOTIFICATION_MARGIN: f32 = 12.0;
/// Vertical gap between stacked notifications.
const NOTIFICATION_GAP: f32 = 8.0;
/// Inner padding used by notification cards.
const NOTIFICATION_PAD: f32 = 12.0;
/// Minimum supported notification timeout.
const NOTIFICATION_TIMEOUT_MIN_SECS: f32 = 0.1;
/// Maximum supported notification timeout.
const NOTIFICATION_TIMEOUT_MAX_SECS: f32 = 3600.0;

/// Body text width inside a notification card after horizontal padding.
fn notification_body_wrap_width(width: f32) -> f32 {
    (width - 2.0 * NOTIFICATION_PAD).max(1.0)
}

/// Build the body text layout used by both measurement and rendering.
fn notification_body_layout_job(
    text: &str,
    font_size: f32,
    font_weight: FontWeight,
    color: Color32,
    wrap_width: f32,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.wrap.break_anywhere = true;
    job.append(
        text,
        0.0,
        egui::TextFormat {
            color,
            font_id: egui::FontId::new(font_size, fonts::weight_family(font_weight)),
            ..Default::default()
        },
    );
    job
}

#[derive(Debug, Clone)]
/// A single notification retained in the backlog list.
pub struct BacklogEntry {
    /// Notification kind.
    pub kind: NotifyKind,
    /// Notification title.
    pub title: String,
    /// Notification body text.
    pub text: String,
}

/// Root-viewport stack alias for one live notification.
#[derive(Clone)]
pub struct NotificationStackAlias {
    /// Stack index, newest first.
    pub(crate) index: usize,
    /// Stable live notification id.
    pub(crate) live_id: String,
    /// Stable kind label.
    pub(crate) kind: &'static str,
    /// Notification title.
    pub(crate) title: String,
}

/// Runtime state for an on-screen notification viewport.
struct NotificationItem {
    /// Shared overlay viewport state for this notification window.
    viewport: OverlayWindow,
    /// Stable eguidev id prefix for this notification.
    dev_id: String,
    /// Title text.
    title: String,
    /// Body text.
    text: String,
    /// Kind/level (affects style and color).
    kind: NotifyKind,
    /// Creation time used for expiry.
    created: Instant,
    /// Remaining time-to-live.
    timeout: Duration,
    /// Computed each frame from stack order.
    target_pos: Pos2,
    /// Current animated position.
    current_pos: Pos2,
    /// Animation state for position transitions.
    anim_start_pos: Pos2,
    /// Animation start timestamp.
    anim_start_time: Instant,
    /// If true, snap to target (used for newly inserted items so only existing ones animate).
    snap_to_target: bool,
    /// Cached window size used to build the viewport.
    size: Vec2,
    /// Whether NSWindow style has been applied for this notification viewport.
    window_configured: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
/// Placement decision for one notification card.
struct NotificationPlacement {
    /// Top-left geometry for the notification viewport.
    geometry: WindowGeometry,
    /// Next bottom-left cursor position for lower cards in the stack.
    next_cursor_bottom: f32,
}

#[derive(Clone, Copy)]
/// Presentation details for a notification title row.
struct NotificationTitleStyle<'a> {
    /// Optional leading icon.
    icon: Option<&'a str>,
    /// Title font size.
    title_size: f32,
    /// Title font weight.
    title_weight: FontWeight,
    /// Title foreground color.
    title_fg: Color32,
}

/// Manages transient in-app notifications and their windows.
pub struct NotificationCenter {
    /// Ephemeral, on-screen notification windows.
    items: Vec<NotificationItem>,
    /// Backlog of all notifications (most-recent first).
    backlog: Vec<BacklogEntry>,
    /// Maximum backlog size.
    max_items: usize,
    /// Notification card width in logical pixels.
    width: f32,
    /// Screen side to anchor notifications (left or right).
    side: NotifyPos,
    /// Opacity of notification background [0..1].
    opacity: f32,
    /// Default on-screen lifetime for notifications.
    timeout: Duration,
    /// Monotonic counter for generating unique viewport ids.
    counter: u64,
    /// Per-kind concrete styling (fg, bg, weight) from config.
    theme: NotifyTheme,
    /// Window corner radius for notifications.
    radius: f32,
    /// Display metrics used for anchoring windows.
    metrics: OverlayMetrics,
}

impl NotificationCenter {
    /// Initialize a new notification center with defaults from `cfg`.
    pub fn new(cfg: &NotifyConfig) -> Self {
        Self {
            items: Vec::new(),
            backlog: Vec::new(),
            max_items: cfg.buffer,
            width: cfg.width,
            side: cfg.pos,
            opacity: cfg.opacity,
            timeout: notification_timeout(cfg.timeout),
            counter: 0,
            theme: cfg.theme.clone(),
            radius: cfg.radius,
            metrics: OverlayMetrics::default(),
        }
    }

    /// Generate the next unique viewport id for a notification.
    fn next_notification_identity(&mut self) -> (OverlayWindow, String) {
        self.counter += 1;
        let id = self.counter;
        (
            OverlayWindow::new(format!("hotki_notify_{id}")),
            format!("notification.live.{id}"),
        )
    }

    /// Update display metrics used for anchoring notifications.
    pub fn set_display_metrics(&mut self, metrics: DisplayMetrics) {
        if self.metrics.set_display_metrics(metrics) {
            for item in &mut self.items {
                item.viewport.reset_geometry();
                item.snap_to_target = true;
            }
        }
    }

    /// Render the title row with optional icon.
    fn render_title_row(
        ui: &mut egui::Ui,
        nctx: &Context,
        id_base: &str,
        title: &str,
        style: NotificationTitleStyle<'_>,
    ) {
        let title_fmt = egui::TextFormat {
            color: style.title_fg,
            font_id: egui::FontId::new(style.title_size, fonts::weight_family(style.title_weight)),
            ..Default::default()
        };
        if let Some(ic) = style.icon
            && !ic.is_empty()
        {
            let icon_text = egui::RichText::new(ic).font(egui::FontId::new(
                style.title_size * 2.0,
                egui::FontFamily::Proportional,
            ));
            ui.dev_label(format!("{id_base}.icon"), icon_text.color(style.title_fg));
            let (icon_h, title_h) = nctx.fonts_mut(|f| {
                let ih = f
                    .layout_no_wrap(
                        ic.to_string(),
                        egui::FontId::new(style.title_size * 2.0, egui::FontFamily::Proportional),
                        style.title_fg,
                    )
                    .size()
                    .y;
                let th = f
                    .layout_no_wrap(
                        title.to_string(),
                        egui::FontId::new(
                            style.title_size,
                            fonts::weight_family(style.title_weight),
                        ),
                        style.title_fg,
                    )
                    .size()
                    .y;
                (ih, th)
            });
            let vpad = ((icon_h - title_h) / 2.0).max(0.0);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                if vpad > 0.0 {
                    ui.add_space(vpad);
                }
                let mut title_job = LayoutJob::default();
                title_job.append(title, 0.0, title_fmt);
                ui.dev_label(format!("{id_base}.title"), title_job);
            });
            return;
        }
        let mut title_job = LayoutJob::default();
        title_job.append(title, 0.0, title_fmt);
        ui.dev_label(format!("{id_base}.title"), title_job);
    }

    /// Queue a new notification to be displayed.
    pub fn push(&mut self, kind: NotifyKind, title: String, text: String) {
        let created = Instant::now();
        // Record in backlog first
        self.backlog.insert(
            0,
            BacklogEntry {
                kind,
                title: title.clone(),
                text: text.clone(),
            },
        );
        // Trim backlog to configured size
        if self.backlog.len() > self.max_items {
            self.backlog.truncate(self.max_items);
        }

        let (viewport, dev_id) = self.next_notification_identity();
        let item = NotificationItem {
            viewport,
            dev_id,
            title,
            text,
            kind,
            created,
            timeout: self.timeout,
            target_pos: pos2(0.0, 0.0),
            current_pos: pos2(0.0, 0.0),
            anim_start_pos: pos2(0.0, 0.0),
            anim_start_time: Instant::now(),
            snap_to_target: true,
            size: Vec2::ZERO,
            window_configured: false,
        };
        self.items.insert(0, item);
    }

    /// Trim live notifications to the configured maximum, hiding removed windows when possible.
    fn trim_live_items(&mut self, ctx: Option<&Context>) {
        while self.items.len() > self.max_items {
            let Some(mut item) = self.items.pop() else {
                break;
            };
            if let Some(ctx) = ctx {
                item.viewport.hide(ctx);
            }
        }
    }

    /// Compute a notification card position from measured size and stack cursor.
    fn placement_for(
        bounds: DisplayBounds,
        side: NotifyPos,
        width: f32,
        height: f32,
        y_cursor: f32,
    ) -> NotificationPlacement {
        let pos_bottom = y_cursor - height;
        let geometry = WindowGeometry::new(
            pos2(
                bounds.notification_x(side, width, NOTIFICATION_MARGIN),
                bounds.to_top_left_y(pos_bottom, height),
            ),
            Vec2::new(width, height),
        );
        NotificationPlacement {
            geometry,
            next_cursor_bottom: pos_bottom - NOTIFICATION_GAP,
        }
    }

    /// Compute layout positions for notification windows and update animations.
    fn layout(&mut self, ctx: &Context) {
        let bounds = self.metrics.display().active_bounds();
        let frame = bounds.frame();
        let mut y_cursor = frame.y + frame.height - NOTIFICATION_MARGIN;
        let width = self.width.max(1.0);
        let body_wrap_width = notification_body_wrap_width(width);

        // Measure each notification to compute height using the same fonts and paddings as render
        for item in &mut self.items {
            let style = self.theme.style_for(item.kind);
            let title_font = egui::FontId::new(
                style.title_font_size,
                fonts::weight_family(style.title_font_weight),
            );
            let text_gal = ctx.fonts_mut(|f| {
                f.layout_job(notification_body_layout_job(
                    &item.text,
                    style.body_font_size,
                    style.body_font_weight,
                    Color32::WHITE,
                    body_wrap_width,
                ))
            });
            let title_gal = ctx.fonts_mut(|f| {
                f.layout_no_wrap(item.title.clone(), title_font.clone(), Color32::WHITE)
            });
            // Account for icon height (rendered at 2x title size) when computing title line height
            let icon_h = if let Some(ic) = &style.icon {
                if !ic.is_empty() {
                    ctx.fonts_mut(|f| {
                        f.layout_no_wrap(
                            ic.clone(),
                            // Use proportional family to allow fallback for symbol glyphs
                            egui::FontId::new(
                                style.title_font_size * 2.0,
                                egui::FontFamily::Proportional,
                            ),
                            Color32::WHITE,
                        )
                        .size()
                        .y
                    })
                } else {
                    0.0
                }
            } else {
                0.0
            };
            // Vertical spacing between title and body is 6.0 in render
            let content_h = title_gal.size().y.max(icon_h) + 6.0 + text_gal.size().y;
            // Guard for negative/degenerate heights and ensure a minimal positive size.
            let total_h = (content_h + 2.0 * NOTIFICATION_PAD).max(1.0);
            let placement = Self::placement_for(bounds, self.side, width, total_h, y_cursor);
            let new_target = placement.geometry.pos;
            let old_target = item.target_pos;
            item.target_pos = new_target;
            item.size = placement.geometry.size;
            y_cursor = placement.next_cursor_bottom;

            // Decide whether to animate or snap to target
            if item.snap_to_target {
                item.current_pos = item.target_pos;
                item.anim_start_pos = item.target_pos;
                item.anim_start_time = Instant::now();
                item.snap_to_target = false;
            } else if (old_target.x - new_target.x).abs() > f32::EPSILON
                || (old_target.y - new_target.y).abs() > f32::EPSILON
            {
                // Start new animation from current position
                item.anim_start_pos = item.current_pos;
                item.anim_start_time = Instant::now();
            }
        }
    }

    /// Render notification windows and advance animations.
    pub fn render(&mut self, ctx: &Context, devmcp: &DevMcp) -> bool {
        // Remove expired
        let now = Instant::now();
        self.items
            .retain(|it| now.duration_since(it.created) < it.timeout);
        self.trim_live_items(Some(ctx));
        // Compute positions and sizes
        self.layout(ctx);
        let mut any_animating = false;

        let bounds = self.metrics.display().active_bounds();

        // Update animation and draw. Items that would fall below the bottom of the active
        // screen are not rendered, but remain in the backlog and ephemeral list until
        // they time out naturally.
        for it in &mut self.items {
            // Progress for easing towards target
            let t = (now
                .saturating_duration_since(it.anim_start_time)
                .as_secs_f32()
                / ADJUST_MOVE_SECS)
                .clamp(0.0, 1.0);
            // Ease-out cubic
            let ease = 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t);
            let nx = it.anim_start_pos.x + (it.target_pos.x - it.anim_start_pos.x) * ease;
            let ny = it.anim_start_pos.y + (it.target_pos.y - it.anim_start_pos.y) * ease;
            it.current_pos = pos2(nx, ny);
            if t < 1.0 {
                any_animating = true;
            }

            // Skip rendering windows that would be completely off-screen below the bottom.
            if !bounds.is_visible_vertically(WindowGeometry::new(it.target_pos, it.size)) {
                it.viewport.hide(ctx);
                continue;
            }

            let builder = ViewportBuilder::default()
                .with_title("Hotki Notification")
                .with_decorations(false)
                .with_always_on_top()
                .with_transparent(true)
                .with_has_shadow(false)
                .with_visible(true)
                .with_inner_size(it.size);
            let builder = it
                .viewport
                .sync_builder(ctx, builder, it.current_pos, it.size);

            ctx.show_viewport_immediate(it.viewport.id(), builder, |vp_ui, _| {
                devtools::viewport_frame(
                    devmcp,
                    vp_ui,
                    it.dev_id.clone(),
                    it.dev_id.clone(),
                    |vp_ui| {
                        let nctx = vp_ui.ctx().clone();
                        let style = self.theme.style_for(it.kind);
                        let bg = Color32::from_rgb(style.bg.0, style.bg.1, style.bg.2);
                        let title_fg =
                            Color32::from_rgb(style.title_fg.0, style.title_fg.1, style.title_fg.2);
                        let body_fg =
                            Color32::from_rgb(style.body_fg.0, style.body_fg.1, style.body_fg.2);
                        let a = (self.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
                        let frame = Frame::new()
                            .fill(Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), a))
                            .corner_radius(egui::CornerRadius::same(self.radius as u8))
                            .inner_margin(egui::Margin {
                                left: 12,
                                right: 12,
                                top: 12,
                                bottom: 12,
                            });
                        egui::CentralPanel::default()
                            .frame(frame)
                            .show(vp_ui, |ui| {
                                render_notification_metadata(ui, it, self.side, bounds);
                                ui.spacing_mut().item_spacing = egui::vec2(0.0, 6.0);
                                ui.horizontal(|ui| {
                                    Self::render_title_row(
                                        ui,
                                        &nctx,
                                        &it.dev_id,
                                        &it.title,
                                        NotificationTitleStyle {
                                            icon: style.icon.as_deref(),
                                            title_size: style.title_font_size,
                                            title_weight: style.title_font_weight,
                                            title_fg,
                                        },
                                    );
                                });
                                let body_wrap_width = notification_body_wrap_width(it.size.x);
                                let text_job = notification_body_layout_job(
                                    &it.text,
                                    style.body_font_size,
                                    style.body_font_weight,
                                    body_fg,
                                    body_wrap_width,
                                );
                                let body_response = ui
                                    .vertical(|ui| {
                                        ui.set_width(body_wrap_width);
                                        ui.add(egui::Label::new(text_job).wrap())
                                    })
                                    .inner;
                                track_response_full(
                                    format!("{}.body", it.dev_id),
                                    &body_response,
                                    WidgetMeta {
                                        role: WidgetRole::Label,
                                        label: Some(it.text.clone()),
                                        value: Some(WidgetValue::Text(it.text.clone())),
                                        visible: ui.is_visible()
                                            && ui.is_rect_visible(body_response.rect),
                                        ..Default::default()
                                    },
                                );
                            });
                    },
                );
            });
            if !it.window_configured && nswindow::frame_by_title("Hotki Notification").is_some() {
                if let Err(e) =
                    nswindow::apply_transparent_rounded("Hotki Notification", self.radius as f64)
                {
                    tracing::error!("{}", e);
                }
                it.window_configured = true;
            }
            it.viewport.record_geometry(it.current_pos, it.size);
        }

        if any_animating {
            ctx.request_repaint();
        }
        any_animating
    }

    /// Update sizing/placement/opacity config without clearing existing notifications.
    /// Trims the stack if the new buffer is smaller than the current number of items.
    pub fn reconfigure(&mut self, cfg: &NotifyConfig) {
        self.max_items = cfg.buffer;
        self.width = cfg.width;
        self.side = cfg.pos;
        self.opacity = cfg.opacity;
        self.timeout = notification_timeout(cfg.timeout);
        self.theme = cfg.theme.clone();
        if self.radius != cfg.radius {
            self.radius = cfg.radius;
            for item in &mut self.items {
                item.window_configured = false;
            }
        }
        // Trim backlog to the new buffer size if necessary
        if self.backlog.len() > self.max_items {
            self.backlog.truncate(self.max_items);
        }
    }

    /// Clear all current notifications immediately and hide their windows.
    pub fn clear_all(&mut self, ctx: &Context, devmcp: &DevMcp) {
        for it in &mut self.items {
            it.viewport.hide(ctx);
            eguidev::clear_viewport(devmcp, it.viewport.id());
        }
        self.items.clear();
        self.backlog.clear();
    }
    /// Access the current backlog entries (newest first).
    pub fn backlog(&self) -> &[BacklogEntry] {
        &self.backlog
    }

    /// Return root-viewport stack aliases for current live notifications.
    pub(crate) fn stack_aliases(&self) -> Vec<NotificationStackAlias> {
        self.items
            .iter()
            .enumerate()
            .map(|(index, item)| NotificationStackAlias {
                index,
                live_id: item.dev_id.clone(),
                kind: notify_kind_label(item.kind),
                title: item.title.clone(),
            })
            .collect()
    }
}

/// Record metadata for the visible notification stack.
pub fn render_stack_metadata(ui: &mut egui::Ui, stack: &[NotificationStackAlias]) {
    container(ui, "notification.stack", |ui| {
        devtools::value_anchor(
            ui,
            "notification.stack.count",
            WidgetValue::Float(stack.len() as f64),
        );
        for item in stack {
            render_stack_item_metadata(ui, item);
        }
    });
}

/// Record stack-order aliases for the current live notification.
fn render_stack_item_metadata(ui: &mut egui::Ui, item: &NotificationStackAlias) {
    let id = format!("notification.stack.item.{}", item.index);
    devtools::value_anchor(
        ui,
        format!("{id}.live_id"),
        WidgetValue::Text(item.live_id.clone()),
    );
    devtools::value_anchor(
        ui,
        format!("{id}.kind"),
        WidgetValue::Text(item.kind.to_string()),
    );
    devtools::value_anchor(
        ui,
        format!("{id}.title"),
        WidgetValue::Text(item.title.clone()),
    );
}

/// Record script-visible metadata for one live notification viewport.
fn render_notification_metadata(
    ui: &mut egui::Ui,
    item: &NotificationItem,
    side: NotifyPos,
    bounds: DisplayBounds,
) {
    let id = &item.dev_id;
    let expected_x = bounds.notification_x(side, item.size.x, NOTIFICATION_MARGIN);
    devtools::value_anchor(
        ui,
        format!("{id}.kind"),
        WidgetValue::Text(notify_kind_label(item.kind).to_string()),
    );
    devtools::value_anchor(
        ui,
        format!("{id}.side"),
        WidgetValue::Text(notification_side_label(side).to_string()),
    );
    devtools::value_anchor(
        ui,
        format!("{id}.timeout_secs"),
        WidgetValue::Float(item.timeout.as_secs_f64()),
    );
    devtools::value_anchor(
        ui,
        format!("{id}.current_x"),
        WidgetValue::Float(f64::from(item.current_pos.x)),
    );
    devtools::value_anchor(
        ui,
        format!("{id}.current_y"),
        WidgetValue::Float(f64::from(item.current_pos.y)),
    );
    devtools::value_anchor(
        ui,
        format!("{id}.target_x"),
        WidgetValue::Float(f64::from(item.target_pos.x)),
    );
    devtools::value_anchor(
        ui,
        format!("{id}.expected_x"),
        WidgetValue::Float(f64::from(expected_x)),
    );
    devtools::value_anchor(
        ui,
        format!("{id}.target_y"),
        WidgetValue::Float(f64::from(item.target_pos.y)),
    );
    devtools::value_anchor(
        ui,
        format!("{id}.width"),
        WidgetValue::Float(f64::from(item.size.x)),
    );
    devtools::value_anchor(
        ui,
        format!("{id}.body_wrap_width"),
        WidgetValue::Float(f64::from(notification_body_wrap_width(item.size.x))),
    );
    devtools::value_anchor(
        ui,
        format!("{id}.height"),
        WidgetValue::Float(f64::from(item.size.y)),
    );
}

/// Stable script-visible label for notification kind.
fn notify_kind_label(kind: NotifyKind) -> &'static str {
    match kind {
        NotifyKind::Info => "info",
        NotifyKind::Success => "success",
        NotifyKind::Warn => "warn",
        NotifyKind::Error => "error",
        NotifyKind::Ignore => "ignore",
    }
}

/// Stable script-visible label for notification stack side.
fn notification_side_label(side: NotifyPos) -> &'static str {
    match side {
        NotifyPos::Left => "left",
        NotifyPos::Right => "right",
    }
}

/// Convert a user timeout into a safe notification lifetime.
fn notification_timeout(timeout: f32) -> Duration {
    let seconds = if timeout.is_finite() {
        timeout.clamp(NOTIFICATION_TIMEOUT_MIN_SECS, NOTIFICATION_TIMEOUT_MAX_SECS)
    } else {
        NOTIFICATION_TIMEOUT_MIN_SECS
    };
    Duration::from_secs_f32(seconds)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use egui::{pos2, vec2};
    use hotki_protocol::{DisplayFrame, DisplaysSnapshot, NotifyPos};

    use super::NotificationCenter;
    use crate::display::{DisplayBounds, DisplayMetrics};

    fn bounds() -> DisplayBounds {
        DisplayMetrics::from_snapshot(&DisplaysSnapshot {
            global_top: 900.0,
            active: Some(DisplayFrame {
                id: 1,
                x: 0.0,
                y: 0.0,
                width: 400.0,
                height: 300.0,
            }),
            displays: Vec::new(),
        })
        .active_bounds()
    }

    #[test]
    fn placement_for_stacks_from_top_right_in_top_left_coordinates() {
        let first = NotificationCenter::placement_for(
            bounds(),
            NotifyPos::Right,
            120.0,
            40.0,
            300.0 - 12.0,
        );

        assert_eq!(first.geometry.pos, pos2(268.0, 612.0));
        assert_eq!(first.geometry.size, vec2(120.0, 40.0));
        assert_eq!(first.next_cursor_bottom, 240.0);
    }

    #[test]
    fn placement_for_stacks_from_top_left_in_top_left_coordinates() {
        let first =
            NotificationCenter::placement_for(bounds(), NotifyPos::Left, 120.0, 40.0, 300.0 - 12.0);

        assert_eq!(first.geometry.pos, pos2(12.0, 612.0));
        assert_eq!(first.geometry.size, vec2(120.0, 40.0));
        assert_eq!(first.next_cursor_bottom, 240.0);
    }

    #[test]
    fn placement_for_collapses_oversized_width_to_left_margin() {
        let placed = NotificationCenter::placement_for(
            bounds(),
            NotifyPos::Right,
            800.0,
            40.0,
            300.0 - 12.0,
        );

        assert_eq!(placed.geometry.pos.x, 12.0);
    }

    #[test]
    fn timeout_conversion_handles_non_finite_values() {
        assert_eq!(
            super::notification_timeout(f32::INFINITY),
            Duration::from_secs_f32(super::NOTIFICATION_TIMEOUT_MIN_SECS)
        );
    }

    #[test]
    fn body_layout_wraps_long_tokens_inside_card_width() {
        let wrap_width = super::notification_body_wrap_width(64.0);
        let job = super::notification_body_layout_job(
            "averylongunbrokennotificationmessage",
            12.0,
            hotki_protocol::FontWeight::Regular,
            egui::Color32::WHITE,
            wrap_width,
        );

        assert_eq!(wrap_width, 40.0);
        assert_eq!(job.wrap.max_width, 40.0);
        assert!(job.wrap.break_anywhere);
    }

    #[test]
    fn trim_live_items_keeps_newest_notifications() {
        let mut center = NotificationCenter::new(&hotki_protocol::NotifyConfig {
            buffer: 2,
            ..hotki_protocol::NotifyConfig::default()
        });
        center.push(hotki_protocol::NotifyKind::Info, "one".into(), "1".into());
        center.push(hotki_protocol::NotifyKind::Info, "two".into(), "2".into());
        center.push(hotki_protocol::NotifyKind::Info, "three".into(), "3".into());

        center.trim_live_items(None);

        assert_eq!(center.items.len(), 2);
        assert_eq!(center.items[0].title, "three");
        assert_eq!(center.items[1].title, "two");
    }
}
