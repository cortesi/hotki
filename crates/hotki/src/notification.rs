use std::time::{Duration, Instant};

use crate::fonts;
use egui::{
    Color32, Context, Frame, Pos2, Vec2, ViewportBuilder, ViewportCommand, ViewportId, pos2,
};

use config::{Notify, NotifyPos, NotifyTheme};
use hotki_protocol::NotifyKind;

// Duration for easing-based adjustment movements (seconds)
pub const ADJUST_MOVE_SECS: f32 = 0.25;

#[derive(Debug, Clone)]
pub struct BacklogEntry {
    pub kind: NotifyKind,
    pub title: String,
    pub text: String,
}

struct NotificationItem {
    id: ViewportId,
    title: String,
    text: String,
    kind: NotifyKind,
    created: Instant,
    timeout: Duration,
    // Computed each frame from stack order
    target_pos: Pos2,
    // Current animated position
    current_pos: Pos2,
    // Animation state for position transitions
    anim_start_pos: Pos2,
    anim_start_time: Instant,
    // If true, snap to target (used for newly inserted items so only existing ones animate)
    snap_to_target: bool,
    size: Vec2,
}

pub struct NotificationCenter {
    // Ephemeral, on-screen notification windows
    items: Vec<NotificationItem>,
    // Backlog of all notifications (most-recent first)
    backlog: Vec<BacklogEntry>,
    // Maximum backlog size
    max_items: usize,
    width: f32,
    side: NotifyPos,
    opacity: f32,
    timeout: Duration,
    counter: u64,
    // Per-kind concrete styling (fg, bg, weight) from config
    theme: NotifyTheme,
    radius: f32,
}

impl NotificationCenter {
    pub fn new(cfg: &Notify) -> Self {
        Self {
            items: Vec::new(),
            backlog: Vec::new(),
            max_items: cfg.buffer,
            width: cfg.width,
            side: cfg.pos,
            opacity: cfg.opacity,
            timeout: Duration::from_secs_f32(cfg.timeout.max(0.1)),
            counter: 0,
            theme: cfg.theme(),
            radius: cfg.radius,
        }
    }

    fn style_for(kind: NotifyKind, theme: &NotifyTheme) -> &config::NotifyWindowStyle {
        match kind {
            NotifyKind::Info => &theme.info,
            NotifyKind::Warn => &theme.warn,
            NotifyKind::Error => &theme.error,
            NotifyKind::Success => &theme.success,
        }
    }

    fn next_id(&mut self) -> ViewportId {
        self.counter += 1;
        ViewportId::from_hash_of(format!("hotki_notify_{}", self.counter))
    }

    fn active_screen_frame() -> (f32, f32, f32, f32, f32) {
        mac_winops::screen::active_frame()
    }

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

        let id = self.next_id();
        let item = NotificationItem {
            id,
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
        };
        self.items.insert(0, item);
    }

    fn layout(&mut self, ctx: &Context) {
        let m = 12.0; // screen margin
        let gap = 8.0; // vertical gap between notifications
        let (sx, sy, sw, sh, global_top) = Self::active_screen_frame();
        let mut y_cursor = sy + sh - m; // start at top (bottom-left coordinates)
        let x_left = sx + m;
        let x_right = sx + sw - self.width - m;

        // Measure each notification to compute height using the same fonts and paddings as render
        for item in &mut self.items {
            let style = Self::style_for(item.kind, &self.theme);
            let title_font = egui::FontId::new(
                style.title_font_size,
                fonts::weight_family(style.title_font_weight),
            );
            let body_font = egui::FontId::new(
                style.body_font_size,
                fonts::weight_family(style.body_font_weight),
            );
            let text_gal = ctx.fonts(|f| {
                f.layout(
                    item.text.clone(),
                    body_font.clone(),
                    Color32::WHITE,
                    self.width - 24.0, // left+right inner margin
                )
            });
            let title_gal = ctx.fonts(|f| {
                f.layout_no_wrap(item.title.clone(), title_font.clone(), Color32::WHITE)
            });
            // Account for icon height (rendered at 2x title size) when computing title line height
            let icon_h = if let Some(ic) = &style.icon {
                if !ic.is_empty() {
                    ctx.fonts(|f| {
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
            let total_h = content_h + 2.0 * 12.0; // padding
            let pos_b = y_cursor - total_h; // bottom-left y for this window
            let x_b = match self.side {
                NotifyPos::Left => x_left,
                NotifyPos::Right => x_right,
            };
            // Convert to top-left coordinates for egui
            let x_top = x_b;
            let y_top = global_top - (pos_b + total_h);
            let new_target = pos2(x_top, y_top);
            let old_target = item.target_pos;
            item.target_pos = new_target;
            item.size = Vec2::new(self.width, total_h);
            y_cursor = pos_b - gap; // move down for next

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

    pub fn render(&mut self, ctx: &Context) {
        // Remove expired
        let now = Instant::now();
        self.items
            .retain(|it| now.duration_since(it.created) < it.timeout);
        // Compute positions and sizes
        self.layout(ctx);
        let mut any_animating = false;

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
            let (_sx, sy, _sw, _sh, global_top) = Self::active_screen_frame();
            let pos_b = global_top - it.target_pos.y - it.size.y;
            if pos_b < sy {
                // Hide viewport if it was shown previously
                ctx.send_viewport_cmd_to(it.id, ViewportCommand::Visible(false));
                continue;
            }

            let builder = ViewportBuilder::default()
                .with_title("Hotki Notification")
                .with_decorations(false)
                .with_always_on_top()
                .with_transparent(true)
                .with_has_shadow(false)
                .with_visible(true)
                .with_inner_size(it.size)
                .with_position(it.current_pos);

            // Update size/pos in case of changes
            ctx.send_viewport_cmd_to(it.id, ViewportCommand::InnerSize(it.size));
            ctx.send_viewport_cmd_to(it.id, ViewportCommand::OuterPosition(it.current_pos));

            ctx.show_viewport_immediate(it.id, builder, |nctx, _| {
                if let Err(e) = mac_winops::nswindow::apply_transparent_rounded(
                    "Hotki Notification",
                    self.radius as f64,
                ) {
                    tracing::error!("{}", e);
                }

                let (bg, title_fg, body_fg, title_size, title_weight, body_size, body_weight, icon) =
                    match it.kind {
                        NotifyKind::Info => {
                            let s = &self.theme.info;
                            (
                                Color32::from_rgb(s.bg.0, s.bg.1, s.bg.2),
                                Color32::from_rgb(s.title_fg.0, s.title_fg.1, s.title_fg.2),
                                Color32::from_rgb(s.body_fg.0, s.body_fg.1, s.body_fg.2),
                                s.title_font_size,
                                s.title_font_weight,
                                s.body_font_size,
                                s.body_font_weight,
                                s.icon.clone(),
                            )
                        }
                        NotifyKind::Warn => {
                            let s = &self.theme.warn;
                            (
                                Color32::from_rgb(s.bg.0, s.bg.1, s.bg.2),
                                Color32::from_rgb(s.title_fg.0, s.title_fg.1, s.title_fg.2),
                                Color32::from_rgb(s.body_fg.0, s.body_fg.1, s.body_fg.2),
                                s.title_font_size,
                                s.title_font_weight,
                                s.body_font_size,
                                s.body_font_weight,
                                s.icon.clone(),
                            )
                        }
                        NotifyKind::Error => {
                            let s = &self.theme.error;
                            (
                                Color32::from_rgb(s.bg.0, s.bg.1, s.bg.2),
                                Color32::from_rgb(s.title_fg.0, s.title_fg.1, s.title_fg.2),
                                Color32::from_rgb(s.body_fg.0, s.body_fg.1, s.body_fg.2),
                                s.title_font_size,
                                s.title_font_weight,
                                s.body_font_size,
                                s.body_font_weight,
                                s.icon.clone(),
                            )
                        }
                        NotifyKind::Success => {
                            let s = &self.theme.success;
                            (
                                Color32::from_rgb(s.bg.0, s.bg.1, s.bg.2),
                                Color32::from_rgb(s.title_fg.0, s.title_fg.1, s.title_fg.2),
                                Color32::from_rgb(s.body_fg.0, s.body_fg.1, s.body_fg.2),
                                s.title_font_size,
                                s.title_font_weight,
                                s.body_font_size,
                                s.body_font_weight,
                                s.icon.clone(),
                            )
                        }
                    };
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
                egui::CentralPanel::default().frame(frame).show(nctx, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 6.0);
                    ui.horizontal(|ui| {
                        let title_fmt = egui::TextFormat {
                            color: title_fg,
                            font_id: egui::FontId::new(title_size, fonts::weight_family(title_weight)),
                            ..Default::default()
                        };
                        if let Some(ic) = icon.as_ref() {
                            if !ic.is_empty() {
                                // Draw icon first at 2x size
                                // Render the icon using the proportional family to keep emoji/symbol fallback.
                                let icon_text = egui::RichText::new(ic.clone())
                                    .font(egui::FontId::new(title_size * 2.0, egui::FontFamily::Proportional));
                                ui.label(icon_text.color(title_fg));

                                // Vertically center the title relative to the taller icon
                                let (icon_h, title_h) = nctx.fonts(|f| {
                                    let ih = f
                                        .layout_no_wrap(
                                            ic.clone(),
                                            egui::FontId::new(title_size * 2.0, egui::FontFamily::Proportional),
                                            title_fg,
                                        )
                                        .size()
                                        .y;
                                    let th = f
                                        .layout_no_wrap(
                                            it.title.clone(),
                                            egui::FontId::new(
                                                title_size,
                                                fonts::weight_family(title_weight),
                                            ),
                                            title_fg,
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
                                    let mut title_job = egui::text::LayoutJob::default();
                                    title_job.append(&it.title, 0.0, title_fmt);
                                    ui.label(title_job);
                                });
                            } else {
                                // No icon content, just title
                                let mut title_job = egui::text::LayoutJob::default();
                                title_job.append(&it.title, 0.0, title_fmt);
                                ui.label(title_job);
                            }
                        } else {
                            // No icon configured
                            let mut title_job = egui::text::LayoutJob::default();
                            title_job.append(&it.title, 0.0, title_fmt);
                            ui.label(title_job);
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        let mut text_job = egui::text::LayoutJob::default();
                        text_job.append(
                            &it.text,
                            0.0,
                            egui::TextFormat {
                                color: body_fg,
                                font_id: egui::FontId::new(
                                    body_size,
                                    fonts::weight_family(body_weight),
                                ),
                                ..Default::default()
                            },
                        );
                        ui.label(text_job);
                    });
                });
            });
        }

        if any_animating {
            ctx.request_repaint();
        }
    }

    /// Update sizing/placement/opacity config without clearing existing notifications.
    /// Trims the stack if the new buffer is smaller than the current number of items.
    pub fn reconfigure(&mut self, cfg: &config::Notify) {
        self.max_items = cfg.buffer;
        self.width = cfg.width;
        self.side = cfg.pos;
        self.opacity = cfg.opacity;
        self.timeout = Duration::from_secs_f32(cfg.timeout.max(0.1));
        self.theme = cfg.theme();
        self.radius = cfg.radius;
        // Trim backlog to the new buffer size if necessary
        if self.backlog.len() > self.max_items {
            self.backlog.truncate(self.max_items);
        }
    }

    /// Clear all current notifications immediately and hide their windows.
    pub fn clear_all(&mut self, ctx: &Context) {
        for it in &self.items {
            ctx.send_viewport_cmd_to(it.id, ViewportCommand::Visible(false));
        }
        self.items.clear();
        self.backlog.clear();
    }
    /// Access the current backlog entries (newest first).
    pub fn backlog(&self) -> &[BacklogEntry] {
        &self.backlog
    }
}
