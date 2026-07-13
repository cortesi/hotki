//! Transient in-app notifications with stacking, animation, and theming.
use std::time::{Duration, Instant};

use egui::{Color32, Context, Frame, Pos2, Vec2, ViewportBuilder, pos2, text::LayoutJob, vec2};
use eguidev::{DevMcp, WidgetMeta, WidgetRole, WidgetValue, container, track_response_full};
use hotki_protocol::{
    FontWeight, NotifyConfig, NotifyKind, NotifyPos, NotifyTheme, NotifyWindowStyle,
};

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
/// Horizontal gap between a notification icon and title text.
const NOTIFICATION_ICON_TITLE_GAP: f32 = 8.0;
/// Vertical gap between a notification title row and body.
const NOTIFICATION_TITLE_BODY_GAP: f32 = 6.0;
/// Allowed layout delta for runtime compactness assertions.
const LAYOUT_SLOP_PX: f32 = 2.0;
/// Minimum supported notification timeout.
const NOTIFICATION_TIMEOUT_MIN_SECS: f32 = 0.1;
/// Maximum supported notification timeout.
const NOTIFICATION_TIMEOUT_MAX_SECS: f32 = 3600.0;

/// Body text width inside a notification card after horizontal padding.
fn notification_body_wrap_width(width: f32) -> f32 {
    (width - 2.0 * NOTIFICATION_PAD).max(1.0)
}

/// Build wrapped notification text used by both measurement and rendering.
fn notification_layout_job(
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

/// Clamp configured notification width to fit inside the active display margin.
fn notification_card_width(configured_width: f32, display_width: f32) -> f32 {
    let available_width = (display_width - 2.0 * NOTIFICATION_MARGIN).max(1.0);
    configured_width.max(1.0).min(available_width)
}

/// Maximum notification card height for a display.
fn notification_max_card_height(display_height: f32) -> f32 {
    (display_height - 2.0 * NOTIFICATION_MARGIN).max(1.0)
}

/// Return the text rows whose glyphs are visible within a clipped height.
fn notification_visible_text(galley: &egui::Galley, visible_height: f32) -> String {
    galley
        .rows
        .iter()
        .take_while(|row| row.rect().min.y < visible_height)
        .flat_map(|row| row.glyphs.iter().map(|glyph| glyph.chr))
        .collect()
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
    /// Last measured card layout.
    layout: NotificationCardMeasure,
    /// Whether NSWindow style has been applied for this notification viewport.
    window_configured: bool,
}

impl NotificationItem {
    /// Advance the easing animation and return whether another repaint is needed.
    fn advance_animation(&mut self, now: Instant) -> bool {
        let progress = (now
            .saturating_duration_since(self.anim_start_time)
            .as_secs_f32()
            / ADJUST_MOVE_SECS)
            .clamp(0.0, 1.0);
        let ease = 1.0 - (1.0 - progress) * (1.0 - progress) * (1.0 - progress);
        let nx = self.anim_start_pos.x + (self.target_pos.x - self.anim_start_pos.x) * ease;
        let ny = self.anim_start_pos.y + (self.target_pos.y - self.anim_start_pos.y) * ease;
        self.current_pos = pos2(nx, ny);
        progress < 1.0
    }
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

#[derive(Clone, Copy, Debug)]
/// Shared measurement for one notification card.
struct NotificationCardMeasure {
    /// Body text width after horizontal card padding.
    body_wrap_width: f32,
    /// Title text width after accounting for the optional icon.
    title_wrap_width: f32,
    /// Optional icon width.
    icon_width: f32,
    /// Optional icon height.
    icon_height: f32,
    /// Wrapped title text height.
    title_height: f32,
    /// Full title row height, including the optional icon.
    title_row_height: f32,
    /// Full body text height before vertical truncation.
    body_height: f32,
    /// Content height before frame padding.
    content_height: f32,
    /// Card height before applying display maximum height.
    unclamped_height: f32,
    /// Maximum card height for the active display.
    max_height: f32,
    /// Final card height after applying maximum height.
    height: f32,
    /// Body height that can be painted inside the final card.
    body_visible_height: f32,
    /// Whether body content must be vertically clipped.
    truncated: bool,
}

impl NotificationCardMeasure {
    /// Minimal placeholder used before a card has been measured against a display.
    fn empty(width: f32) -> Self {
        let body_wrap_width = notification_body_wrap_width(width);
        Self {
            body_wrap_width,
            title_wrap_width: body_wrap_width,
            icon_width: 0.0,
            icon_height: 0.0,
            title_height: 0.0,
            title_row_height: 0.0,
            body_height: 0.0,
            content_height: 0.0,
            unclamped_height: 1.0,
            max_height: 1.0,
            height: 1.0,
            body_visible_height: 0.0,
            truncated: false,
        }
    }
}

/// Measure one card using the same text wrapping, icon sizing, and padding as render.
fn measure_notification_card(
    ctx: &Context,
    width: f32,
    display_height: f32,
    title: &str,
    body: &str,
    style: &NotifyWindowStyle,
) -> NotificationCardMeasure {
    let width = width.max(1.0);
    let body_wrap_width = notification_body_wrap_width(width);
    let title_fg = Color32::from_rgb(style.title_fg.0, style.title_fg.1, style.title_fg.2);
    let body_fg = Color32::from_rgb(style.body_fg.0, style.body_fg.1, style.body_fg.2);
    let (icon_width, icon_height) = measure_notification_icon(ctx, style, title_fg);
    let title_wrap_width = if icon_width > 0.0 {
        (body_wrap_width - icon_width - NOTIFICATION_ICON_TITLE_GAP).max(1.0)
    } else {
        body_wrap_width
    };

    let title_galley = ctx.fonts_mut(|fonts| {
        fonts.layout_job(notification_layout_job(
            title,
            style.title_font_size,
            style.title_font_weight,
            title_fg,
            title_wrap_width,
        ))
    });
    let body_galley = ctx.fonts_mut(|fonts| {
        fonts.layout_job(notification_layout_job(
            body,
            style.body_font_size,
            style.body_font_weight,
            body_fg,
            body_wrap_width,
        ))
    });
    let title_height = title_galley.size().y;
    let title_row_height = title_height.max(icon_height);
    let body_height = body_galley.size().y;
    let content_height = title_row_height + NOTIFICATION_TITLE_BODY_GAP + body_height;
    let unclamped_height = (content_height + 2.0 * NOTIFICATION_PAD).max(1.0);
    let max_height = notification_max_card_height(display_height);
    let height = unclamped_height.min(max_height).max(1.0);
    let available_body_height =
        height - 2.0 * NOTIFICATION_PAD - title_row_height - NOTIFICATION_TITLE_BODY_GAP;
    let body_visible_height = available_body_height.clamp(0.0, body_height);

    NotificationCardMeasure {
        body_wrap_width,
        title_wrap_width,
        icon_width,
        icon_height,
        title_height,
        title_row_height,
        body_height,
        content_height,
        unclamped_height,
        max_height,
        height,
        body_visible_height,
        truncated: body_visible_height + LAYOUT_SLOP_PX < body_height,
    }
}

/// Measure the optional notification icon.
fn measure_notification_icon(
    ctx: &Context,
    style: &NotifyWindowStyle,
    title_fg: Color32,
) -> (f32, f32) {
    let Some(icon) = style.icon.as_deref().filter(|icon| !icon.is_empty()) else {
        return (0.0, 0.0);
    };
    ctx.fonts_mut(|fonts| {
        let galley = fonts.layout_no_wrap(
            icon.to_string(),
            egui::FontId::new(style.title_font_size * 2.0, egui::FontFamily::Proportional),
            title_fg,
        );
        (galley.size().x, galley.size().y)
    })
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
    /// Whether a live notification of this kind is scheduled for presentation.
    pub(crate) fn contains_kind(&self, kind: NotifyKind) -> bool {
        self.items.iter().any(|item| item.kind == kind)
    }

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
        id_base: &str,
        title: &str,
        style: NotificationTitleStyle<'_>,
        measure: NotificationCardMeasure,
    ) {
        let (row_rect, _) = ui.allocate_exact_size(
            vec2(measure.body_wrap_width, measure.title_row_height),
            egui::Sense::hover(),
        );
        let mut title_x = row_rect.min.x;
        if let Some(ic) = style.icon
            && !ic.is_empty()
        {
            let icon_rect = egui::Rect::from_min_size(
                pos2(
                    row_rect.min.x,
                    row_rect.min.y + (measure.title_row_height - measure.icon_height) * 0.5,
                ),
                vec2(measure.icon_width, measure.icon_height),
            );
            let icon_job = notification_layout_job(
                ic,
                style.title_size * 2.0,
                FontWeight::Regular,
                style.title_fg,
                measure.icon_width,
            );
            let mut icon_job = icon_job;
            if let Some(section) = icon_job.sections.first_mut() {
                section.format.font_id =
                    egui::FontId::new(style.title_size * 2.0, egui::FontFamily::Proportional);
            }
            Self::paint_measured_label(
                ui,
                format!("{id_base}.icon"),
                ic,
                icon_job,
                icon_rect,
                style.title_fg,
            );
            title_x = icon_rect.max.x + NOTIFICATION_ICON_TITLE_GAP;
        }
        let title_rect = egui::Rect::from_min_size(
            pos2(
                title_x,
                row_rect.min.y + (measure.title_row_height - measure.title_height) * 0.5,
            ),
            vec2(measure.title_wrap_width, measure.title_height),
        );
        let title_job = notification_layout_job(
            title,
            style.title_size,
            style.title_weight,
            style.title_fg,
            measure.title_wrap_width,
        );
        Self::paint_measured_label(
            ui,
            format!("{id_base}.title"),
            title,
            title_job,
            title_rect,
            style.title_fg,
        );
    }

    /// Paint a measured label inside an explicit rect and record eguidev metadata.
    fn paint_measured_label(
        ui: &mut egui::Ui,
        id: impl Into<String>,
        text: &str,
        job: LayoutJob,
        rect: egui::Rect,
        color: Color32,
    ) -> egui::Response {
        let id = id.into();
        let response = ui.allocate_rect(rect, egui::Sense::hover());
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        ui.painter()
            .with_clip_rect(rect.intersect(ui.clip_rect()))
            .galley(rect.min, galley, color);
        track_response_full(
            id,
            &response,
            WidgetMeta {
                role: WidgetRole::Label,
                label: Some(text.to_string()),
                value: Some(WidgetValue::Text(text.to_string())),
                visible: ui.is_visible() && ui.is_rect_visible(rect),
                ..Default::default()
            },
        );
        response
    }

    /// Paint the measured body region, clipping vertically when the card is too short.
    fn render_body(
        ui: &mut egui::Ui,
        id_base: &str,
        text: &str,
        style: &NotifyWindowStyle,
        body_fg: Color32,
        measure: NotificationCardMeasure,
    ) {
        let body_size = vec2(measure.body_wrap_width, measure.body_visible_height);
        let (rect, response) = ui.allocate_exact_size(body_size, egui::Sense::hover());
        let text_job = notification_layout_job(
            text,
            style.body_font_size,
            style.body_font_weight,
            body_fg,
            measure.body_wrap_width,
        );
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(text_job));
        let visible_text = if measure.truncated {
            notification_visible_text(&galley, measure.body_visible_height)
        } else {
            text.to_string()
        };
        ui.painter()
            .with_clip_rect(rect.intersect(ui.clip_rect()))
            .galley(rect.min, galley, body_fg);
        track_response_full(
            format!("{id_base}.body"),
            &response,
            WidgetMeta {
                role: WidgetRole::Label,
                label: Some(text.to_string()),
                value: Some(WidgetValue::Text(visible_text)),
                visible: ui.is_visible() && ui.is_rect_visible(rect),
                ..Default::default()
            },
        );
    }

    /// Queue a new notification to be displayed.
    pub fn push(&mut self, kind: NotifyKind, title: String, text: String) {
        let created = Instant::now();
        self.backlog.insert(
            0,
            BacklogEntry {
                kind,
                title: title.clone(),
                text: text.clone(),
            },
        );
        if self.backlog.len() > self.max_items {
            self.backlog.truncate(self.max_items);
        }

        let (viewport, dev_id) = self.next_notification_identity();
        let width = self.width.max(1.0);
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
            anim_start_time: created,
            snap_to_target: true,
            size: Vec2::ZERO,
            layout: NotificationCardMeasure::empty(width),
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
    fn layout(&mut self, ctx: &Context, now: Instant) {
        let bounds = self.metrics.display().active_bounds();
        let frame = bounds.frame();
        let mut y_cursor = frame.y + frame.height - NOTIFICATION_MARGIN;
        let width = notification_card_width(self.width, frame.width);

        for item in &mut self.items {
            let style = self.theme.style_for(item.kind);
            let measure =
                measure_notification_card(ctx, width, frame.height, &item.title, &item.text, style);
            let placement = Self::placement_for(bounds, self.side, width, measure.height, y_cursor);
            let new_target = placement.geometry.pos;
            let old_target = item.target_pos;
            item.target_pos = new_target;
            item.size = placement.geometry.size;
            item.layout = measure;
            y_cursor = placement.next_cursor_bottom;

            if item.snap_to_target {
                item.current_pos = item.target_pos;
                item.anim_start_pos = item.target_pos;
                item.anim_start_time = now;
                item.snap_to_target = false;
            } else if (old_target.x - new_target.x).abs() > f32::EPSILON
                || (old_target.y - new_target.y).abs() > f32::EPSILON
            {
                item.anim_start_pos = item.current_pos;
                item.anim_start_time = now;
            }
        }
    }

    /// Render notification windows and advance animations.
    pub fn render(&mut self, ctx: &Context, devmcp: &DevMcp) -> bool {
        let now = Instant::now();
        self.items
            .retain(|it| now.duration_since(it.created) < it.timeout);
        self.trim_live_items(Some(ctx));
        self.layout(ctx, now);
        let mut any_animating = false;

        let bounds = self.metrics.display().active_bounds();

        // Update animation and draw. Items that would fall below the bottom of the active
        // screen are not rendered, but remain in the backlog and ephemeral list until
        // they time out naturally.
        for it in &mut self.items {
            if it.advance_animation(now) {
                any_animating = true;
            }

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
                        let style = self.theme.style_for(it.kind);
                        render_notification_metadata(vp_ui, it, self.side, bounds);
                        Self::render_card(vp_ui, it, style, self.opacity, self.radius);
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

    /// Render the card contents for one visible notification viewport.
    fn render_card(
        ui: &mut egui::Ui,
        item: &NotificationItem,
        style: &NotifyWindowStyle,
        opacity: f32,
        radius: f32,
    ) {
        let bg = Color32::from_rgb(style.bg.0, style.bg.1, style.bg.2);
        let title_fg = Color32::from_rgb(style.title_fg.0, style.title_fg.1, style.title_fg.2);
        let body_fg = Color32::from_rgb(style.body_fg.0, style.body_fg.1, style.body_fg.2);
        let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        let frame = Frame::new()
            .fill(Color32::from_rgba_unmultiplied(
                bg.r(),
                bg.g(),
                bg.b(),
                alpha,
            ))
            .corner_radius(egui::CornerRadius::same(radius as u8))
            .inner_margin(egui::Margin::same(NOTIFICATION_PAD as i8));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            Self::render_title_row(
                ui,
                &item.dev_id,
                &item.title,
                NotificationTitleStyle {
                    icon: style.icon.as_deref(),
                    title_size: style.title_font_size,
                    title_weight: style.title_font_weight,
                    title_fg,
                },
                item.layout,
            );
            ui.add_space(NOTIFICATION_TITLE_BODY_GAP);
            Self::render_body(ui, &item.dev_id, &item.text, style, body_fg, item.layout);
        });
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
    ui: &egui::Ui,
    item: &NotificationItem,
    side: NotifyPos,
    bounds: DisplayBounds,
) {
    let id = &item.dev_id;
    let area_id = egui::Id::new(format!("{id}.metadata"));
    egui::Area::new(area_id)
        .fixed_pos(Pos2::ZERO)
        .interactable(false)
        .show(ui.ctx(), |ui| {
            let frame = bounds.frame();
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
                WidgetValue::Float(f64::from(item.layout.body_wrap_width)),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.height"),
                WidgetValue::Float(f64::from(item.size.y)),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.content_height"),
                WidgetValue::Float(f64::from(item.layout.content_height)),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.unclamped_height"),
                WidgetValue::Float(f64::from(item.layout.unclamped_height)),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.max_height"),
                WidgetValue::Float(f64::from(item.layout.max_height)),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.body_visible_height"),
                WidgetValue::Float(f64::from(item.layout.body_visible_height)),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.body_height"),
                WidgetValue::Float(f64::from(item.layout.body_height)),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.body_text"),
                WidgetValue::Text(item.text.clone()),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.truncated"),
                WidgetValue::Bool(item.layout.truncated),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.padding"),
                WidgetValue::Float(f64::from(NOTIFICATION_PAD)),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.title_body_gap"),
                WidgetValue::Float(f64::from(NOTIFICATION_TITLE_BODY_GAP)),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.layout_slop"),
                WidgetValue::Float(f64::from(LAYOUT_SLOP_PX)),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.display_x"),
                WidgetValue::Float(f64::from(frame.x)),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.display_y"),
                WidgetValue::Float(f64::from(frame.y)),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.display_width"),
                WidgetValue::Float(f64::from(frame.width)),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.display_height"),
                WidgetValue::Float(f64::from(frame.height)),
            );
            devtools::value_anchor(
                ui,
                format!("{id}.margin"),
                WidgetValue::Float(f64::from(NOTIFICATION_MARGIN)),
            );
        });
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
    use hotki_protocol::{
        DisplayFrame, DisplaysSnapshot, NotifyKind, NotifyPos, NotifyTheme, NotifyWindowStyle,
    };

    use super::{NotificationCenter, measure_notification_card};
    use crate::{
        display::{DisplayBounds, DisplayMetrics},
        fonts,
    };

    fn test_context() -> egui::Context {
        let ctx = egui::Context::default();
        fonts::install_fonts(&ctx);
        ctx.begin_pass(egui::RawInput::default());
        ctx
    }

    fn style(kind: NotifyKind) -> NotifyWindowStyle {
        NotifyTheme::default().style_for(kind).clone()
    }

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
        let job = super::notification_layout_job(
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
    fn notification_card_width_clamps_to_display_margin() {
        assert_eq!(super::notification_card_width(420.0, 900.0), 420.0);
        assert_eq!(
            super::notification_card_width(420.0, 100.0),
            100.0 - 2.0 * super::NOTIFICATION_MARGIN
        );
    }

    #[test]
    fn measure_notification_card_keeps_representative_short_card_compact() {
        let ctx = test_context();
        let style = style(NotifyKind::Success);

        let measure = measure_notification_card(&ctx, 420.0, 900.0, "Done", "Short body", &style);

        assert!(!measure.truncated);
        assert_eq!(measure.height, measure.unclamped_height);
        assert!(measure.height <= measure.content_height + 2.0 * super::NOTIFICATION_PAD);
        assert_eq!(measure.body_visible_height, measure.body_height);
        assert_eq!(measure.max_height, 900.0 - 2.0 * super::NOTIFICATION_MARGIN);
    }

    #[test]
    fn measure_notification_card_wraps_long_body_title_and_icon_variants() {
        let ctx = test_context();
        let icon_style = style(NotifyKind::Error);
        let mut no_icon_style = icon_style.clone();
        no_icon_style.icon = None;
        let long_body = "/Users/example/hotki/long-unbroken-path-that-must-wrap-inside-the-card-\
            without-being-clipped-or-forcing-horizontal-growth";
        let wrapped_body = "This notification body has enough ordinary prose to wrap across \
            multiple lines while staying within the configured card width.";
        let long_title = "A very long notification title that should wrap inside the card";

        let long_body_measure =
            measure_notification_card(&ctx, 420.0, 900.0, "Error", long_body, &icon_style);
        let wrapped_body_measure =
            measure_notification_card(&ctx, 420.0, 900.0, "Info", wrapped_body, &icon_style);
        let long_title_measure =
            measure_notification_card(&ctx, 220.0, 900.0, long_title, "body", &icon_style);
        let no_icon_measure =
            measure_notification_card(&ctx, 420.0, 900.0, "Plain", "body", &no_icon_style);

        assert!(long_body_measure.body_height > no_icon_measure.body_height);
        assert!(wrapped_body_measure.body_height > no_icon_measure.body_height);
        assert!(long_title_measure.title_height > no_icon_measure.title_height);
        assert!(long_title_measure.title_wrap_width < long_title_measure.body_wrap_width);
        assert!(icon_style.icon.is_some());
        assert_eq!(no_icon_measure.icon_width, 0.0);
        assert_eq!(
            no_icon_measure.title_wrap_width,
            no_icon_measure.body_wrap_width
        );
    }

    #[test]
    fn measure_notification_card_reports_vertical_truncation_when_display_is_short() {
        let ctx = test_context();
        let style = style(NotifyKind::Warn);
        let body = "line ".repeat(200);
        let display_height = 90.0;

        let measure =
            measure_notification_card(&ctx, 420.0, display_height, "Warning", &body, &style);

        assert_eq!(
            measure.max_height,
            display_height - 2.0 * super::NOTIFICATION_MARGIN
        );
        assert!(measure.height <= measure.max_height);
        assert!(measure.unclamped_height > measure.max_height);
        assert!(measure.truncated);
        assert!(measure.body_visible_height > 0.0);
        assert!(measure.body_visible_height < measure.body_height);
        assert!(
            super::NOTIFICATION_PAD
                + measure.title_row_height
                + super::NOTIFICATION_TITLE_BODY_GAP
                + measure.body_visible_height
                + super::NOTIFICATION_PAD
                <= measure.height + super::LAYOUT_SLOP_PX
        );
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
