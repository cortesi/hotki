//! Heads-up display (HUD) rendering for key hints.
use config::{FontWeight, Mode, Pos};
use egui::{
    CentralPanel, Color32, Context, Frame, Pos2, Vec2, ViewportBuilder, ViewportCommand,
    ViewportId, pos2, vec2,
};
use mac_winops::{
    nswindow::{apply_transparent_rounded, set_on_all_spaces},
    screen,
};

use crate::fonts;

/// Minimum HUD width in logical pixels.
const HUD_MIN_WIDTH: f32 = 240.0;
/// Minimum HUD height in logical pixels.
const HUD_MIN_HEIGHT: f32 = 80.0;
/// Horizontal HUD padding from edges.
const HUD_PADDING_X: f32 = 12.0;
/// Vertical HUD padding from edges.
const HUD_PADDING_Y: f32 = 12.0;

/// Vertical gap between key rows.
const KEY_ROW_GAP: f32 = 10.0;
/// Gap between the last key box row and the description text.
const KEY_DESC_GAP: f32 = 16.0;
/// Horizontal gap (each side) around the plus separator between key boxes.
const KEY_PLUS_GAP: f32 = 6.0;
/// Gap between tag items.
const HUD_TAG_GAP: f32 = 8.0;

/// HUD state and rendering helpers.
pub struct Hud {
    /// Whether the HUD is currently shown.
    visible: bool,
    /// Full HUD configuration copied from config.
    cfg: config::Hud,
    /// Stable viewport identifier for the HUD window.
    id: ViewportId,
    /// Keys to display as `(token, description, is_modifier)` triplets.
    keys: Vec<(String, String, bool)>,
    /// Optional parent window title used for placement.
    parent_title: Option<String>,
    /// Last computed position (for smooth movement).
    last_pos: Option<Pos2>,
    /// Last applied opacity.
    last_opacity: Option<f32>,
    /// Last applied size for the HUD.
    last_size: Option<Vec2>,
    /// Cached width of the '+' glyph for current key font (size, weight -> width).
    plus_w_cache: Option<(f32, FontWeight, f32)>,
}

impl Hud {
    /// Create a new HUD instance from configuration.
    pub fn new(cfg: &config::Hud) -> Self {
        Self {
            visible: false,
            cfg: cfg.clone(),
            id: ViewportId::from_hash_of("hotki_hud"),
            keys: Vec::new(),
            parent_title: None,
            last_pos: None,
            last_opacity: None,
            last_size: None,
            plus_w_cache: None,
        }
    }

    /// Create a shallow clone for measurement helpers without changing state.
    fn clone_for_measure(&self) -> Self {
        Self {
            visible: self.visible,
            cfg: self.cfg.clone(),
            id: self.id,
            keys: self.keys.clone(),
            parent_title: self.parent_title.clone(),
            last_pos: self.last_pos,
            last_opacity: self.last_opacity,
            last_size: self.last_size,
            plus_w_cache: self.plus_w_cache,
        }
    }

    /// Return true if a token represents a modifier key.
    fn is_modifier(tok: &str) -> bool {
        matches!(
            tok.to_ascii_lowercase().as_str(),
            "ctrl"
                | "control"
                | "cmd"
                | "command"
                | "super"
                | "win"
                | "windows"
                | "meta"
                | "alt"
                | "option"
                | "opt"
                | "shift"
        )
    }

    /// Render rounded key token boxes for a key sequence.
    fn render_key_tokens(&self, ui: &mut egui::Ui, key: &str) {
        let tokens: Vec<&str> = self.parse_tokens(key);
        let rounding = egui::CornerRadius::same(self.cfg.key_radius as u8);
        let visuals = ui.visuals().clone();
        for (i, tok) in tokens.iter().enumerate() {
            if i > 0 {
                ui.add_space(KEY_PLUS_GAP);
                let prev = ui.style().override_font_id.clone();
                ui.style_mut().override_font_id = Some(self.key_font_id());
                ui.label("+");
                ui.style_mut().override_font_id = prev;
                ui.add_space(KEY_PLUS_GAP);
            }
            let is_mod = Self::is_modifier(tok);
            let (fg, bg) = if is_mod {
                (self.cfg.mod_fg, self.cfg.mod_bg)
            } else {
                (self.cfg.key_fg, self.cfg.key_bg)
            };
            let fill = Color32::from_rgb(bg.0, bg.1, bg.2);
            let stroke = visuals.widgets.inactive.bg_stroke; // keep default stroke
            let frame = Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(rounding)
                .inner_margin(egui::Margin {
                    left: self.cfg.key_pad_x as i8,
                    right: self.cfg.key_pad_x as i8,
                    top: self.cfg.key_pad_y as i8,
                    bottom: self.cfg.key_pad_y as i8,
                });
            frame.show(ui, |ui| {
                // Use key-specific font size inside key boxes
                let prev = ui.style().override_font_id.clone();
                let fam = if is_mod {
                    fonts::weight_family(self.cfg.mod_font_weight)
                } else {
                    fonts::weight_family(self.cfg.key_font_weight)
                };
                ui.style_mut().override_font_id =
                    Some(egui::FontId::new(self.cfg.key_font_size, fam));
                let style = ui.style_mut();
                style.visuals.override_text_color = Some(Color32::from_rgb(fg.0, fg.1, fg.2));
                ui.label(*tok);
                // restore previous font override
                ui.style_mut().override_font_id = prev;
            });
        }
    }

    /// Render all key rows for the HUD.
    fn render_full_hud_rows(&self, ui: &mut egui::Ui, hud_ctx: &egui::Context, avail: Vec2) {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = KEY_ROW_GAP;
            for (k, d, is_mode) in &self.keys {
                self.render_key_row(ui, hud_ctx, avail, k, d, *is_mode);
            }
        });
    }

    /// Render a single key row with tokens, description, and optional tag.
    fn render_key_row(
        &self,
        ui: &mut egui::Ui,
        hud_ctx: &egui::Context,
        avail: Vec2,
        key: &str,
        desc: &str,
        is_mode: bool,
    ) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.spacing_mut().item_spacing.x = 0.0;
            self.render_key_tokens(ui, key);
            ui.add_space(KEY_DESC_GAP);
            ui.label(desc);
            if is_mode {
                let (token_boxes_w, _) = {
                    let mut tmp = Self {
                        plus_w_cache: None,
                        ..self.clone_for_measure()
                    };
                    tmp.measure_token_boxes(hud_ctx, key)
                };
                let desc_w = hud_ctx.fonts(|f| {
                    f.layout_no_wrap(desc.to_string(), self.title_font_id(), Color32::WHITE)
                        .size()
                        .x
                });
                let row_content_w = token_boxes_w + KEY_DESC_GAP + desc_w;
                let tag_w = hud_ctx.fonts(|f| {
                    f.layout_no_wrap(
                        self.cfg.tag_submenu.clone(),
                        self.tag_font_id(),
                        Color32::WHITE,
                    )
                    .size()
                    .x
                });

                let available_content_width = avail.x - 2.0 * HUD_PADDING_X;
                let spacer =
                    (available_content_width - row_content_w - HUD_TAG_GAP - tag_w).max(0.0);
                ui.add_space(spacer);
                ui.add_space(HUD_TAG_GAP);

                let prev_font = ui.style().override_font_id.clone();
                ui.style_mut().override_font_id = Some(self.tag_font_id());
                let prev_color = ui.style().visuals.override_text_color;
                let (tag_r, tag_g, tag_b) = self.cfg.tag_fg;
                ui.style_mut().visuals.override_text_color =
                    Some(Color32::from_rgb(tag_r, tag_g, tag_b));
                ui.label(self.cfg.tag_submenu.as_str());
                ui.style_mut().override_font_id = prev_font;
                ui.style_mut().visuals.override_text_color = prev_color;
            }
        });
    }

    /// Update the displayed keys, externally-computed visibility, and parent title.
    pub fn set_keys(
        &mut self,
        keys: Vec<(String, String, bool)>,
        visible: bool,
        parent_title: Option<String>,
    ) {
        self.keys = keys;
        self.parent_title = parent_title.filter(|s| !s.trim().is_empty());
        if visible && !self.visible {
            // Force a position recompute and apply on next show
            self.last_pos = None;
        }
        self.visible = visible;
    }

    /// Get the current keys and visibility state (returns keys and visible flag)
    pub fn get_state(&self) -> (Vec<(String, String, bool)>, bool, Option<String>) {
        (self.keys.clone(), self.visible, self.parent_title.clone())
    }

    /// Get the active screen frame as `(x, y, w, h, global_top)`.
    fn active_screen_frame() -> (f32, f32, f32, f32, f32) {
        screen::active_frame()
    }

    /// FontId for key tokens inside key boxes.
    fn key_font_id(&self) -> egui::FontId {
        egui::FontId::new(
            self.cfg.key_font_size,
            fonts::weight_family(self.cfg.key_font_weight),
        )
    }

    /// FontId for title/description text.
    fn title_font_id(&self) -> egui::FontId {
        egui::FontId::new(
            self.cfg.font_size,
            fonts::weight_family(self.cfg.title_font_weight),
        )
    }

    /// FontId for the sub-mode tag indicator.
    fn tag_font_id(&self) -> egui::FontId {
        egui::FontId::new(
            self.cfg.tag_font_size,
            fonts::weight_family(self.cfg.tag_font_weight),
        )
    }

    /// Cached width of the plus separator for the current key font.
    fn plus_width(&mut self, ctx: &Context) -> f32 {
        if let Some((sz, wt, w)) = self.plus_w_cache
            && (sz - self.cfg.key_font_size).abs() < f32::EPSILON
            && wt == self.cfg.key_font_weight
        {
            return w;
        }
        let w = ctx.fonts(|f| {
            f.layout_no_wrap("+".to_owned(), self.key_font_id(), Color32::WHITE)
                .size()
                .x
        });
        self.plus_w_cache = Some((self.cfg.key_font_size, self.cfg.key_font_weight, w));
        w
    }

    /// Split a key sequence like "Ctrl+C" into tokens.
    fn parse_tokens<'a>(&self, key: &'a str) -> Vec<&'a str> {
        key.split('+')
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .collect()
    }

    /// Measure combined width and height of the rendered token boxes.
    fn measure_token_boxes(&mut self, ctx: &Context, key: &str) -> (f32, f32) {
        let tokens = self.parse_tokens(key);
        let mut tokens_text_w = 0.0f32;
        let mut token_text_h: f32 = 0.0;
        let plus_w = self.plus_width(ctx);
        ctx.fonts(|f| {
            for (i, tok) in tokens.iter().enumerate() {
                let gal = f.layout_no_wrap((*tok).to_owned(), self.key_font_id(), Color32::WHITE);
                tokens_text_w += gal.size().x;
                token_text_h = token_text_h.max(gal.size().y);
                if i > 0 {
                    tokens_text_w += plus_w + 2.0 * KEY_PLUS_GAP;
                }
            }
        });
        let boxes_w = tokens_text_w + (tokens.len() as f32) * (2.0 * self.cfg.key_pad_x);
        let boxes_h = token_text_h + 2.0 * self.cfg.key_pad_y;
        (boxes_w, boxes_h)
    }

    /// Measure the HUD content area (excluding outer padding).
    fn measure_content_size(&self, ctx: &Context) -> Vec2 {
        let font_id_desc = self.title_font_id();
        let mut max_row_content_w: f32 = 0.0;
        let mut total_h: f32 = 0.0;
        let rows = self.keys.len();
        let mut any_tag = false;

        // Pre-measure tag text once
        let tag_col_w: f32 = ctx.fonts(|f| {
            f.layout_no_wrap(
                self.cfg.tag_submenu.clone(),
                self.tag_font_id(),
                Color32::WHITE,
            )
            .size()
            .x
        });
        for (k, d, is_mode) in &self.keys {
            let (token_boxes_w, token_boxes_h) = {
                // self is not mutable here, but plus width cache is an optimization.
                // Use a temporary mutable borrow of a clone of self to reuse the helper.
                let mut tmp = Self {
                    plus_w_cache: None,
                    ..self.clone_for_measure()
                };
                tmp.measure_token_boxes(ctx, k)
            };
            // Description width/height
            let (desc_w, desc_h) = ctx.fonts(|f| {
                let g = f.layout_no_wrap(d.clone(), font_id_desc.clone(), Color32::WHITE);
                (g.size().x, g.size().y)
            });
            if *is_mode {
                any_tag = true;
            }
            // Row content width (without the right-aligned tag column)
            let row_content_w = token_boxes_w + KEY_DESC_GAP + desc_w;
            let row_h = token_boxes_h.max(desc_h);

            max_row_content_w = max_row_content_w.max(row_content_w);
            total_h += row_h;
        }

        // Add inter-row spacing using our constant
        if rows.saturating_sub(1) > 0 {
            total_h += KEY_ROW_GAP * (rows.saturating_sub(1) as f32);
        }

        // If any row has a tag, reserve a right-aligned column for it
        let total_w = if any_tag {
            max_row_content_w + HUD_TAG_GAP + tag_col_w
        } else {
            max_row_content_w
        };

        vec2(total_w, total_h)
    }

    /// Desired HUD window size including padding and minimums.
    fn desired_size(&self, ctx: &Context) -> Vec2 {
        if matches!(self.cfg.mode, Mode::Mini) {
            // Compact size based only on parent_title
            if let Some(title) = &self.parent_title {
                let (w, h) = ctx.fonts(|f| {
                    let g = f.layout_no_wrap(title.clone(), self.title_font_id(), Color32::WHITE);
                    (g.size().x, g.size().y)
                });
                return vec2(w + 2.0 * HUD_PADDING_X, h + 2.0 * HUD_PADDING_Y);
            }
        }
        let content = self.measure_content_size(ctx);
        let mut w = content.x + 2.0 * HUD_PADDING_X;
        let mut h = content.y + 2.0 * HUD_PADDING_Y;

        // Clamp to minimums only in full HUD mode
        if matches!(self.cfg.mode, Mode::Hud) {
            if w < HUD_MIN_WIDTH {
                w = HUD_MIN_WIDTH;
            }
            if h < HUD_MIN_HEIGHT {
                h = HUD_MIN_HEIGHT;
            }
        }
        vec2(w, h)
    }

    /// Compute the anchored top-left position for the HUD window.
    fn anchor_pos(&self, _ctx: &Context, size: Vec2) -> Pos2 {
        let (sx, sy, sw, sh, global_top) = Self::active_screen_frame();
        let m = 12.0;
        // Compute bottom-left origin x_b,y_b for the window's bottom-left
        let (x_b, y_b) = match self.cfg.pos {
            Pos::N => (sx + (sw - size.x) / 2.0, sy + sh - size.y - m),
            Pos::NE => (sx + sw - size.x - m, sy + sh - size.y - m),
            Pos::E => (sx + sw - size.x - m, sy + (sh - size.y) / 2.0),
            Pos::SE => (sx + sw - size.x - m, sy + m),
            Pos::S => (sx + (sw - size.x) / 2.0, sy + m),
            Pos::SW => (sx + m, sy + m),
            Pos::W => (sx + m, sy + (sh - size.y) / 2.0),
            Pos::NW => (sx + m, sy + sh - size.y - m),
            Pos::Center => (sx + (sw - size.x) / 2.0, sy + (sh - size.y) / 2.0),
        };
        // Convert to top-left global coordinates expected by winit/egui OuterPosition
        let mut x_top = x_b + self.cfg.offset.x;
        let mut y_top = global_top - (y_b + size.y) + self.cfg.offset.y;
        // Clamp within the chosen screen bounds in top-left coordinates
        let screen_top_y = global_top - (sy + sh);
        let min_x = sx;
        let max_x = sx + sw - size.x;
        let min_y = screen_top_y;
        let max_y = screen_top_y + (sh - size.y);
        if x_top < min_x {
            x_top = min_x;
        }
        if x_top > max_x {
            x_top = max_x;
        }
        if y_top < min_y {
            y_top = min_y;
        }
        if y_top > max_y {
            y_top = max_y;
        }
        pos2(x_top, y_top)
    }

    /// Render and update the HUD viewport.
    pub fn render(&mut self, ctx: &Context) {
        if !self.visible {
            // Ensure the viewport is hidden if we were previously visible
            ctx.send_viewport_cmd_to(self.id, ViewportCommand::Visible(false));
            return;
        }

        let size = self.desired_size(ctx);
        let pos = self.anchor_pos(ctx, size);

        // Only update window position if it changed
        if self.last_pos != Some(pos) {
            ctx.send_viewport_cmd_to(self.id, ViewportCommand::OuterPosition(pos));
            // Record the last position we commanded
            // (we'll update after building the viewport to avoid races)
        }

        let mut builder = ViewportBuilder::default()
            .with_title("Hotki HUD")
            .with_decorations(false)
            .with_always_on_top()
            .with_transparent(true)
            .with_has_shadow(false)
            .with_visible(true)
            .with_inner_size(size)
            // Avoid specifying position here every frame; we set it on first create below.
            ;
        // Ensure correct initial placement when the viewport is first created
        if self.last_pos.is_none() {
            builder = builder.with_position(pos);
        }

        // If size changed after previous frame, request a resize
        if self.last_size != Some(size) {
            ctx.send_viewport_cmd_to(self.id, ViewportCommand::InnerSize(size));
        }

        ctx.show_viewport_immediate(self.id, builder, |hud_ctx, _| {
            // Ensure the NSWindow is transparent and uses full alpha for perfect edge blending.
            if let Err(e) = apply_transparent_rounded("Hotki HUD", self.cfg.radius as f64) {
                tracing::error!("{}", e);
            }
            // Make HUD appear on all desktops/spaces
            if let Err(e) = set_on_all_spaces("Hotki HUD") {
                tracing::error!("{}", e);
            }

            let mut frame =
                Frame::default().corner_radius(egui::CornerRadius::same(self.cfg.radius as u8));
            let a = (self.cfg.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
            let (r, g, b) = self.cfg.bg;
            frame = frame.fill(Color32::from_rgba_unmultiplied(r, g, b, a));
            CentralPanel::default().frame(frame).show(hud_ctx, |ui| {
                let style = ui.style_mut();
                style.override_font_id = Some(self.title_font_id());
                let (fr, fg, fb) = self.cfg.title_fg;
                style.visuals.override_text_color =
                    Some(Color32::from_rgba_unmultiplied(fr, fg, fb, 255));

                if matches!(self.cfg.mode, Mode::Mini) {
                    // Compact: center vertically; left padding; single title line
                    if let Some(title) = &self.parent_title {
                        let avail = ui.available_size();
                        // compute text height to center vertically
                        let text_h = hud_ctx.fonts(|f| {
                            f.layout_no_wrap(title.clone(), self.title_font_id(), Color32::WHITE)
                                .size()
                                .y
                        });
                        let extra_y = (avail.y - (text_h + 2.0 * HUD_PADDING_Y)).max(0.0);
                        let left_margin = HUD_PADDING_X;
                        let top_margin = HUD_PADDING_Y + extra_y / 2.0;
                        ui.add_space(top_margin);
                        ui.horizontal(|ui| {
                            ui.add_space(left_margin);
                            ui.label(title);
                        });
                    }
                } else {
                    // Full HUD
                    // Left-align content with padding, center vertically if needed
                    let content = self.measure_content_size(hud_ctx);
                    let avail = ui.available_size();
                    let extra_y = (avail.y - (content.y + 2.0 * HUD_PADDING_Y)).max(0.0);
                    let left_margin = HUD_PADDING_X; // Always align to left with standard padding
                    let top_margin = HUD_PADDING_Y + extra_y / 2.0;

                    ui.add_space(top_margin);
                    ui.horizontal(|ui| {
                        ui.add_space(left_margin);
                        self.render_full_hud_rows(ui, hud_ctx, avail);
                    });
                }
            });
        });

        // Update last-known state after issuing commands
        self.last_pos = Some(pos);
        self.last_opacity = Some(self.cfg.opacity.clamp(0.0, 1.0));
        self.last_size = Some(size);
    }

    /// Hide the HUD viewport and stop rendering until made visible again.
    pub fn hide(&mut self, ctx: &Context) {
        self.visible = false;
        ctx.send_viewport_cmd_to(self.id, ViewportCommand::Visible(false));
    }
}
