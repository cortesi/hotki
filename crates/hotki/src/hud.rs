//! Heads-up display (HUD) rendering for key hints.
use egui::{CentralPanel, Color32, Context, Frame, Vec2, ViewportBuilder, vec2};
use eguidev::{DevMcp, DevUiExt, WidgetValue, container};
use hotki_protocol::{HudRow, HudRowStyle, HudStyle, Mode};
use mac_keycode::{Chord, Modifier};

use crate::{
    devtools,
    display::{DisplayMetrics, WindowGeometry},
    fonts,
    nswindow::{apply_transparent_rounded, frame_by_title, set_on_all_spaces},
    overlay::OverlayWindow,
};

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
    /// Full HUD configuration copied from the server-provided style.
    cfg: HudStyle,
    /// Shared overlay viewport state.
    viewport: OverlayWindow,
    /// Rows to display.
    rows: Vec<HudRow>,
    /// Breadcrumb titles for the current mode stack.
    breadcrumbs: Vec<String>,
    /// Whether NSWindow style has been applied for the current HUD session.
    window_configured: bool,
}

/// Data needed to render one HUD frame.
struct HudViewModel {
    /// Desired viewport geometry.
    geometry: WindowGeometry,
    /// Optional title for mini mode.
    mini_title: Option<String>,
    /// Measured content size for full HUD mode.
    content_size: Vec2,
}

impl Hud {
    /// Create a new HUD instance from configuration.
    pub fn new(cfg: &HudStyle) -> Self {
        Self {
            visible: false,
            cfg: cfg.clone(),
            viewport: OverlayWindow::new("hotki_hud"),
            rows: Vec::new(),
            breadcrumbs: Vec::new(),
            window_configured: false,
        }
    }

    /// Update the HUD style in-place and clear cached placement when it changes.
    pub fn set_style(&mut self, cfg: HudStyle) {
        if self.cfg != cfg {
            self.cfg = cfg;
            self.viewport.reset_geometry();
            self.window_configured = false;
        }
    }

    /// Deterministic sort order for modifier tokens.
    ///
    /// This matches the usual macOS visual ordering for key chords.
    fn modifier_order(m: &Modifier) -> usize {
        match m {
            Modifier::Command => 0,
            Modifier::Option => 1,
            Modifier::Control => 2,
            Modifier::Shift => 3,
            Modifier::Function => 4,
            Modifier::CapsLock => 5,
            Modifier::RightCommand => 6,
            Modifier::RightControl => 7,
            Modifier::RightOption => 8,
            Modifier::RightShift => 9,
        }
    }

    /// Convert a chord into ordered `(token, is_modifier)` pairs for display.
    fn tokens_for_chord(&self, chord: &Chord) -> Vec<(String, bool)> {
        let mut mods: Vec<Modifier> = chord.modifiers.iter().copied().collect();
        mods.sort_by_key(Self::modifier_order);
        let mut tokens = Vec::with_capacity(mods.len() + 1);
        for m in mods {
            tokens.push((m.to_spec(), true));
        }
        let key_is_mod = Modifier::try_from(chord.key).is_ok();
        tokens.push((chord.key.to_spec(), key_is_mod));
        tokens
    }

    /// Render rounded key token boxes for a chord, applying optional per-row style overrides.
    fn render_key_tokens(
        &self,
        ui: &mut egui::Ui,
        chord: &Chord,
        row_index: usize,
        row_style: Option<&HudRowStyle>,
    ) {
        let tokens = self.tokens_for_chord(chord);
        let rounding = egui::CornerRadius::same(self.cfg.key_radius as u8);
        let visuals = ui.visuals().clone();
        for (i, (tok, is_mod)) in tokens.iter().enumerate() {
            if i > 0 {
                ui.add_space(KEY_PLUS_GAP);
                let prev = ui.style().override_font_id.clone();
                ui.style_mut().override_font_id = Some(self.key_font_id());
                ui.label("+");
                ui.style_mut().override_font_id = prev;
                ui.add_space(KEY_PLUS_GAP);
            }
            let (fg, bg) = if *is_mod {
                row_style
                    .map(|s| (s.mod_fg, s.mod_bg))
                    .unwrap_or((self.cfg.mod_fg, self.cfg.mod_bg))
            } else {
                row_style
                    .map(|s| (s.key_fg, s.key_bg))
                    .unwrap_or((self.cfg.key_fg, self.cfg.key_bg))
            };
            let fill = Color32::from_rgb(bg.0, bg.1, bg.2);
            let stroke = visuals.widgets.inactive.bg_stroke;
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
                let prev = ui.style().override_font_id.clone();
                let fam = if *is_mod {
                    fonts::weight_family(self.cfg.mod_font_weight)
                } else {
                    fonts::weight_family(self.cfg.key_font_weight)
                };
                ui.style_mut().override_font_id =
                    Some(egui::FontId::new(self.cfg.key_font_size, fam));
                let style = ui.style_mut();
                style.visuals.override_text_color = Some(Color32::from_rgb(fg.0, fg.1, fg.2));
                ui.dev_label(format!("hud.row.{row_index}.token.{i}"), tok.as_str());
                ui.style_mut().override_font_id = prev;
            });
        }
    }

    /// Render all key rows for the HUD.
    fn render_full_hud_rows(&self, ui: &mut egui::Ui, hud_ctx: &egui::Context, avail: Vec2) {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = KEY_ROW_GAP;
            for (index, row) in self.rows.iter().enumerate() {
                container(ui, format!("hud.row.{index}"), |ui| {
                    self.render_key_row(ui, hud_ctx, avail, index, row);
                });
            }
        });
    }

    /// Render a single key row with tokens, description, and optional tag.
    fn render_key_row(
        &self,
        ui: &mut egui::Ui,
        hud_ctx: &egui::Context,
        avail: Vec2,
        row_index: usize,
        row: &HudRow,
    ) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            devtools::value_anchor(
                ui,
                format!("hud.row.{row_index}.chord"),
                WidgetValue::Text(row.chord.to_string()),
            );
            devtools::value_anchor(
                ui,
                format!("hud.row.{row_index}.is_mode"),
                WidgetValue::Bool(row.is_mode),
            );
            self.render_key_tokens(ui, &row.chord, row_index, row.style.as_ref());
            ui.add_space(KEY_DESC_GAP);
            ui.dev_label(format!("hud.row.{row_index}.desc"), &row.desc);
            if row.is_mode {
                let (token_boxes_w, _) = self.measure_token_boxes(hud_ctx, &row.chord);
                let desc_w = hud_ctx.fonts_mut(|f| {
                    f.layout_no_wrap(row.desc.clone(), self.title_font_id(), Color32::WHITE)
                        .size()
                        .x
                });
                let row_content_w = token_boxes_w + KEY_DESC_GAP + desc_w;
                let tag_w = hud_ctx.fonts_mut(|f| {
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
                let (tag_r, tag_g, tag_b) = row.style.map(|s| s.tag_fg).unwrap_or(self.cfg.tag_fg);
                ui.style_mut().visuals.override_text_color =
                    Some(Color32::from_rgb(tag_r, tag_g, tag_b));
                ui.dev_label(
                    format!("hud.row.{row_index}.tag"),
                    self.cfg.tag_submenu.as_str(),
                );
                ui.style_mut().override_font_id = prev_font;
                ui.style_mut().visuals.override_text_color = prev_color;
            }
        });
    }

    /// Update the current HUD state: rows, visibility, and breadcrumbs.
    pub fn set_state(&mut self, rows: Vec<HudRow>, visible: bool, breadcrumbs: Vec<String>) {
        self.rows = rows;
        self.breadcrumbs = breadcrumbs;
        if visible && !self.visible {
            self.viewport.reset_geometry();
            self.window_configured = false;
        } else if !visible {
            self.window_configured = false;
        }
        self.visible = visible;
    }

    /// Hide the HUD window immediately.
    pub fn hide(&mut self, ctx: &Context) {
        self.visible = false;
        self.rows.clear();
        self.breadcrumbs.clear();
        self.window_configured = false;
        self.viewport.hide(ctx);
    }

    /// Update display metrics used for positioning and clear cached placement when the
    /// active display changes.
    pub fn set_display_metrics(&mut self, metrics: DisplayMetrics) {
        self.viewport.set_display_metrics(metrics);
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

    /// Measure combined width and height of the rendered token boxes.
    fn measure_token_boxes(&self, ctx: &Context, chord: &Chord) -> (f32, f32) {
        let tokens = self.tokens_for_chord(chord);
        let key_font = self.key_font_id();
        let (tokens_text_w, token_text_h, plus_w) = ctx.fonts_mut(|f| {
            let plus_w = f
                .layout_no_wrap("+".to_owned(), key_font.clone(), Color32::WHITE)
                .size()
                .x;
            let mut w = 0.0f32;
            let mut h = 0.0f32;
            for (i, (tok, _)) in tokens.iter().enumerate() {
                let gal = f.layout_no_wrap(tok.clone(), key_font.clone(), Color32::WHITE);
                w += gal.size().x;
                h = h.max(gal.size().y);
                if i > 0 {
                    w += plus_w + 2.0 * KEY_PLUS_GAP;
                }
            }
            (w, h, plus_w)
        });
        let _ = plus_w; // computed inline, not cached
        let boxes_w = tokens_text_w + (tokens.len() as f32) * (2.0 * self.cfg.key_pad_x);
        let boxes_h = token_text_h + 2.0 * self.cfg.key_pad_y;
        (boxes_w, boxes_h)
    }

    /// Measure the HUD content area (excluding outer padding).
    fn measure_content_size(&self, ctx: &Context) -> Vec2 {
        let font_id_desc = self.title_font_id();
        let mut max_row_content_w: f32 = 0.0;
        let mut total_h: f32 = 0.0;
        let rows = self.rows.len();
        let mut any_tag = false;

        // Pre-measure tag text once
        let tag_col_w: f32 = ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                self.cfg.tag_submenu.clone(),
                self.tag_font_id(),
                Color32::WHITE,
            )
            .size()
            .x
        });
        for row in &self.rows {
            let (token_boxes_w, token_boxes_h) = self.measure_token_boxes(ctx, &row.chord);
            // Description width/height
            let (desc_w, desc_h) = ctx.fonts_mut(|f| {
                let g = f.layout_no_wrap(row.desc.clone(), font_id_desc.clone(), Color32::WHITE);
                (g.size().x, g.size().y)
            });
            if row.is_mode {
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
            // Compact size based only on the active breadcrumb title.
            if let Some(title) = self.breadcrumbs.last().filter(|s| !s.trim().is_empty()) {
                let (w, h) = ctx.fonts_mut(|f| {
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

    /// Build the data needed for one HUD render pass.
    fn view_model(&self, ctx: &Context) -> HudViewModel {
        let size = self.desired_size(ctx);
        let geometry = self.viewport.display().active_bounds().anchored_geometry(
            self.cfg.pos,
            size,
            12.0,
            self.cfg.offset,
        );
        let mini_title = matches!(self.cfg.mode, Mode::Mini)
            .then(|| {
                self.breadcrumbs
                    .last()
                    .filter(|s| !s.trim().is_empty())
                    .cloned()
            })
            .flatten();
        let content_size = if matches!(self.cfg.mode, Mode::Hud) {
            self.measure_content_size(ctx)
        } else {
            Vec2::ZERO
        };
        HudViewModel {
            geometry,
            mini_title,
            content_size,
        }
    }

    /// Render and update the HUD viewport.
    pub fn render(&mut self, ctx: &Context, devmcp: &DevMcp) {
        if !self.visible {
            self.viewport.hide(ctx);
            return;
        }

        let model = self.view_model(ctx);

        // Only update window position if it changed
        let mut builder = ViewportBuilder::default()
            .with_title("Hotki HUD")
            .with_decorations(false)
            .with_always_on_top()
            .with_transparent(true)
            .with_has_shadow(false)
            .with_visible(true)
            .with_inner_size(model.geometry.size)
            // Avoid specifying position here every frame; we set it on first create below.
            ;
        builder = self
            .viewport
            .sync_builder(ctx, builder, model.geometry.pos, model.geometry.size);

        ctx.show_viewport_immediate(self.viewport.id(), builder, |vp_ui, _| {
            devtools::viewport_frame(devmcp, vp_ui, |vp_ui| {
                let hud_ctx = vp_ui.ctx().clone();
                let mut frame =
                    Frame::default().corner_radius(egui::CornerRadius::same(self.cfg.radius as u8));
                let a = (self.cfg.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
                let (r, g, b) = self.cfg.bg;
                frame = frame.fill(Color32::from_rgba_unmultiplied(r, g, b, a));
                CentralPanel::default().frame(frame).show(vp_ui, |ui| {
                    container(ui, "hud.root", |ui| {
                        container(ui, "hud.panel", |ui| {
                            self.render_panel(ui, &hud_ctx, &model);
                        });
                    });
                });
            });
        });

        if !self.window_configured && frame_by_title("Hotki HUD").is_some() {
            if let Err(e) = apply_transparent_rounded("Hotki HUD", self.cfg.radius as f64) {
                tracing::error!("{}", e);
            }
            if let Err(e) = set_on_all_spaces("Hotki HUD") {
                tracing::error!("{}", e);
            }
            self.window_configured = true;
        }

        // Update last-known state after issuing commands
        self.viewport
            .record_geometry(model.geometry.pos, model.geometry.size);
    }

    /// Render either full or mini HUD content inside the HUD panel.
    fn render_panel(&self, ui: &mut egui::Ui, hud_ctx: &egui::Context, model: &HudViewModel) {
        self.render_metadata(ui, model);
        let style = ui.style_mut();
        style.override_font_id = Some(self.title_font_id());
        let (fr, fg, fb) = self.cfg.title_fg;
        style.visuals.override_text_color = Some(Color32::from_rgba_unmultiplied(fr, fg, fb, 255));

        if matches!(self.cfg.mode, Mode::Mini) {
            self.render_mini_hud(ui, hud_ctx, model.mini_title.as_deref());
        } else {
            self.render_full_hud(ui, hud_ctx, model);
        }
    }

    /// Record script-visible HUD state that is not otherwise represented by text widgets.
    fn render_metadata(&self, ui: &mut egui::Ui, model: &HudViewModel) {
        devtools::value_anchor(
            ui,
            "hud.mode",
            WidgetValue::Text(hud_mode_label(self.cfg.mode)),
        );
        devtools::value_anchor(ui, "hud.visible", WidgetValue::Bool(self.visible));
        devtools::value_anchor(
            ui,
            "hud.geometry.x",
            WidgetValue::Float(f64::from(model.geometry.pos.x)),
        );
        devtools::value_anchor(
            ui,
            "hud.geometry.y",
            WidgetValue::Float(f64::from(model.geometry.pos.y)),
        );
        devtools::value_anchor(
            ui,
            "hud.geometry.width",
            WidgetValue::Float(f64::from(model.geometry.size.x)),
        );
        devtools::value_anchor(
            ui,
            "hud.geometry.height",
            WidgetValue::Float(f64::from(model.geometry.size.y)),
        );
    }

    /// Render compact HUD mode.
    fn render_mini_hud(&self, ui: &mut egui::Ui, hud_ctx: &egui::Context, title: Option<&str>) {
        let Some(title) = title else {
            return;
        };
        let avail = ui.available_size();
        let text_h = hud_ctx.fonts_mut(|f| {
            f.layout_no_wrap(title.to_string(), self.title_font_id(), Color32::WHITE)
                .size()
                .y
        });
        let extra_y = (avail.y - (text_h + 2.0 * HUD_PADDING_Y)).max(0.0);
        let top_margin = HUD_PADDING_Y + extra_y / 2.0;
        ui.add_space(top_margin);
        ui.horizontal(|ui| {
            ui.add_space(HUD_PADDING_X);
            ui.dev_label("hud.mini.title", title);
        });
    }

    /// Render full HUD mode.
    fn render_full_hud(&self, ui: &mut egui::Ui, hud_ctx: &egui::Context, model: &HudViewModel) {
        let avail = ui.available_size();
        let extra_y = (avail.y - (model.content_size.y + 2.0 * HUD_PADDING_Y)).max(0.0);
        let top_margin = HUD_PADDING_Y + extra_y / 2.0;

        ui.add_space(top_margin);
        ui.horizontal(|ui| {
            ui.add_space(HUD_PADDING_X);
            self.render_full_hud_rows(ui, hud_ctx, avail);
        });
    }

    // (hide method is defined alongside state setters)
}

/// Stable script-visible label for the HUD mode.
fn hud_mode_label(mode: Mode) -> String {
    match mode {
        Mode::Hud => "hud",
        Mode::Hide => "hide",
        Mode::Mini => "mini",
    }
    .to_string()
}
