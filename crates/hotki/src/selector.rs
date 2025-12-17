//! Interactive selector popup rendering.

use egui::{
    CentralPanel, Color32, Context, Frame, Margin, Pos2, Stroke, Vec2, ViewportBuilder,
    ViewportCommand, ViewportId, epaint::Shadow, pos2, text::LayoutJob, vec2,
};
use hotki_protocol::{FontWeight, SelectorSnapshot, SelectorStyle};

use crate::{
    display::DisplayMetrics,
    fonts,
    nswindow::{apply_transparent_rounded, set_on_all_spaces},
};

/// Fixed selector width in logical pixels.
const SELECTOR_WIDTH: f32 = 480.0;
/// Selector window corner radius in logical pixels.
const SELECTOR_RADIUS: f32 = 10.0;
/// Outer padding around selector content.
const SELECTOR_PADDING: f32 = 12.0;
/// Vertical spacing between selector sections.
const SECTION_GAP: f32 = 10.0;
/// Input field height (excluding outer padding).
const INPUT_HEIGHT: f32 = 34.0;
/// Corner radius for input and item rows.
const ROW_RADIUS: f32 = 6.0;
/// Vertical gap between items.
const ITEM_GAP: f32 = 6.0;
/// Item inner padding.
const ITEM_MARGIN: Margin = Margin {
    left: 10,
    right: 10,
    top: 8,
    bottom: 8,
};
/// Input field inner padding.
const INPUT_MARGIN: Margin = Margin {
    left: 10,
    right: 10,
    top: 8,
    bottom: 8,
};

/// Background alpha applied to all selector fills.
const BG_ALPHA: u8 = 240;

/// Font sizes used by the selector viewport.
const TITLE_FONT_SIZE: f32 = 16.0;
/// Font size used for the query input line.
const INPUT_FONT_SIZE: f32 = 15.0;
/// Font size used for selector item labels.
const ITEM_FONT_SIZE: f32 = 15.0;
/// Font size used for selector item sublabels.
const SUBLABEL_FONT_SIZE: f32 = 12.0;

/// Selector viewport state and rendering helpers.
pub struct SelectorWindow {
    /// Stable viewport identifier for the selector window.
    id: ViewportId,
    /// Effective selector style (colors).
    style: SelectorStyle,
    /// Current selector snapshot, when visible.
    state: Option<SelectorSnapshot>,
    /// Cached display metrics used for positioning.
    display: DisplayMetrics,
    /// Last computed position (used to keep the top edge stable).
    last_pos: Option<Pos2>,
    /// Last applied window size.
    last_size: Option<Vec2>,
}

impl SelectorWindow {
    /// Create a new selector window instance.
    pub fn new(style: &SelectorStyle) -> Self {
        Self {
            id: ViewportId::from_hash_of("hotki_selector"),
            style: style.clone(),
            state: None,
            display: DisplayMetrics::default(),
            last_pos: None,
            last_size: None,
        }
    }

    /// Update selector style in-place and clear cached placement when it changes.
    pub fn set_style(&mut self, style: SelectorStyle) {
        if self.style != style {
            self.style = style;
        }
    }

    /// Update display metrics used for positioning and clear cached placement when the
    /// active display changes.
    pub fn set_display_metrics(&mut self, metrics: DisplayMetrics) {
        let previous = self.display.active_frame();
        let next = metrics.active_frame();
        self.display = metrics;
        if previous != next {
            self.last_pos = None;
        }
    }

    /// Replace the selector snapshot state (shows the selector if needed).
    pub fn set_state(&mut self, snapshot: SelectorSnapshot) {
        if self.state.is_none() {
            self.last_pos = None;
        }
        self.state = Some(snapshot);
    }

    /// Hide the selector window immediately.
    pub fn hide(&mut self, ctx: &Context) {
        self.state = None;
        self.last_pos = None;
        self.last_size = None;
        ctx.send_viewport_cmd_to(self.id, ViewportCommand::Visible(false));
    }

    /// Title font for the selector header.
    fn title_font_id(&self) -> egui::FontId {
        egui::FontId::new(TITLE_FONT_SIZE, fonts::weight_family(FontWeight::Bold))
    }

    /// Font for the query input line.
    fn input_font_id(&self) -> egui::FontId {
        egui::FontId::new(INPUT_FONT_SIZE, fonts::weight_family(FontWeight::Regular))
    }

    /// Font for selector item labels.
    fn item_font_id(&self) -> egui::FontId {
        egui::FontId::new(ITEM_FONT_SIZE, fonts::weight_family(FontWeight::Regular))
    }

    /// Font for selector item sublabels.
    fn sublabel_font_id(&self) -> egui::FontId {
        egui::FontId::new(
            SUBLABEL_FONT_SIZE,
            fonts::weight_family(FontWeight::Regular),
        )
    }

    /// Compute an appropriate selector window size for the current snapshot.
    fn desired_size(&self, ctx: &Context, snapshot: &SelectorSnapshot) -> Vec2 {
        let content_w = SELECTOR_WIDTH.max(1.0);

        let title_h = if snapshot.title.trim().is_empty() {
            0.0
        } else {
            ctx.fonts(|f| {
                f.layout_no_wrap(snapshot.title.clone(), self.title_font_id(), Color32::WHITE)
                    .size()
                    .y
            })
        };

        let input_h = INPUT_HEIGHT;

        let items_h = ctx.fonts(|f| {
            snapshot
                .items
                .iter()
                .map(|it| {
                    let label_h = f
                        .layout_no_wrap(it.label.clone(), self.item_font_id(), Color32::WHITE)
                        .size()
                        .y;
                    let sub_h = it.sublabel.as_ref().map_or(0.0, |s| {
                        f.layout_no_wrap(s.clone(), self.sublabel_font_id(), Color32::WHITE)
                            .size()
                            .y
                    });
                    let gap = if it.sublabel.is_some() { 2.0 } else { 0.0 };
                    label_h + gap + sub_h + (ITEM_MARGIN.top + ITEM_MARGIN.bottom) as f32
                })
                .sum::<f32>()
        });

        let gaps = SECTION_GAP
            + if snapshot.title.trim().is_empty() {
                0.0
            } else {
                SECTION_GAP
            }
            + (snapshot.items.len().saturating_sub(1) as f32) * ITEM_GAP;

        let content_h = title_h + input_h + items_h + gaps;
        let total_h = (content_h + 2.0 * SELECTOR_PADDING).clamp(140.0, 520.0);

        vec2(content_w, total_h)
    }

    /// Center horizontally and keep the top edge stable while visible.
    fn desired_pos(&self, size: Vec2) -> Pos2 {
        let frame = self.display.active_frame();
        let x = frame.x + (frame.width - size.x) / 2.0;
        let y = self.last_pos.map_or_else(
            || self.display.active_frame_top_left_y() + (frame.height - size.y) / 2.0,
            |p| p.y,
        );
        pos2(x, y)
    }

    /// Convert an RGB tuple to a `Color32` with the supplied alpha.
    fn rgba((r, g, b): (u8, u8, u8), a: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(r, g, b, a)
    }

    /// Build a layout job that highlights matched codepoint indices.
    fn layout_label_with_matches(
        label: &str,
        match_indices: &[u32],
        fg: Color32,
        match_fg: Color32,
        font_id: egui::FontId,
    ) -> LayoutJob {
        let mut job = LayoutJob::default();
        if label.is_empty() {
            return job;
        }

        let normal_fmt = egui::TextFormat {
            color: fg,
            font_id: font_id.clone(),
            ..Default::default()
        };
        let match_fmt = egui::TextFormat {
            color: match_fg,
            font_id,
            ..Default::default()
        };

        let mut next_match = 0usize;
        let mut seg_start = 0usize;
        let mut seg_highlight = false;
        for (cp_index, (byte_idx, _ch)) in (0_u32..).zip(label.char_indices()) {
            let highlight = match_indices
                .get(next_match)
                .copied()
                .is_some_and(|m| m == cp_index);
            if highlight != seg_highlight {
                if seg_start < byte_idx {
                    let fmt = if seg_highlight {
                        match_fmt.clone()
                    } else {
                        normal_fmt.clone()
                    };
                    job.append(&label[seg_start..byte_idx], 0.0, fmt);
                }
                seg_start = byte_idx;
                seg_highlight = highlight;
            }
            if highlight {
                next_match += 1;
            }
        }

        if seg_start < label.len() {
            let fmt = if seg_highlight { match_fmt } else { normal_fmt };
            job.append(&label[seg_start..], 0.0, fmt);
        }

        job
    }

    /// Render and update the selector viewport.
    pub fn render(&mut self, ctx: &Context) {
        let Some(snapshot) = self.state.as_ref() else {
            ctx.send_viewport_cmd_to(self.id, ViewportCommand::Visible(false));
            return;
        };

        // Keep the selector window size stable while visible; resizing the viewport on every
        // keystroke can cause perceptible flicker.
        let size = self
            .last_size
            .unwrap_or_else(|| self.desired_size(ctx, snapshot));
        let pos = self.desired_pos(size);

        if self.last_pos != Some(pos) {
            ctx.send_viewport_cmd_to(self.id, ViewportCommand::OuterPosition(pos));
        }

        let mut builder = ViewportBuilder::default()
            .with_title("Hotki Selector")
            .with_decorations(false)
            .with_always_on_top()
            .with_transparent(true)
            .with_has_shadow(false)
            .with_visible(true)
            .with_inner_size(size);
        if self.last_pos.is_none() {
            builder = builder.with_position(pos);
        }
        if self.last_size != Some(size) {
            ctx.send_viewport_cmd_to(self.id, ViewportCommand::InnerSize(size));
        }

        let style = self.style.clone();
        let title_font_id = self.title_font_id();
        let input_font_id = self.input_font_id();
        let item_font_id = self.item_font_id();
        let sublabel_font_id = self.sublabel_font_id();
        let snapshot = snapshot.clone();
        ctx.show_viewport_immediate(self.id, builder, move |sel_ctx, _| {
            if let Err(e) = apply_transparent_rounded("Hotki Selector", SELECTOR_RADIUS as f64) {
                tracing::error!("{}", e);
            }
            if let Err(e) = set_on_all_spaces("Hotki Selector") {
                tracing::error!("{}", e);
            }

            let border = Self::rgba(style.border, BG_ALPHA);
            let bg = Self::rgba(style.bg, BG_ALPHA);
            let mut frame = Frame::new()
                .fill(bg)
                .stroke(Stroke::new(1.0, border))
                .corner_radius(egui::CornerRadius::same(SELECTOR_RADIUS as u8))
                .inner_margin(Margin::same(SELECTOR_PADDING as i8));
            frame.shadow = Shadow {
                offset: [0, 6],
                blur: 24,
                spread: 0,
                color: Self::rgba(style.shadow, 64),
            };

            CentralPanel::default().frame(frame).show(sel_ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;

                let fg = Color32::from_rgba_unmultiplied(230, 230, 230, 255);
                let dim = Color32::from_rgba_unmultiplied(160, 160, 160, 255);
                let match_fg =
                    Color32::from_rgb(style.match_fg.0, style.match_fg.1, style.match_fg.2);

                if !snapshot.title.trim().is_empty() {
                    let title_fmt = egui::TextFormat {
                        color: fg,
                        font_id: title_font_id.clone(),
                        ..Default::default()
                    };
                    ui.horizontal(|ui| {
                        let mut job = LayoutJob::default();
                        job.append(snapshot.title.as_str(), 0.0, title_fmt);
                        ui.label(job);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!("{}", snapshot.total_matches))
                                    .size(SUBLABEL_FONT_SIZE)
                                    .color(dim),
                            );
                        });
                    });
                    ui.add_space(SECTION_GAP);
                }

                let input_bg = Self::rgba(style.input_bg, BG_ALPHA);
                let input_frame = Frame::new()
                    .fill(input_bg)
                    .corner_radius(egui::CornerRadius::same(ROW_RADIUS as u8))
                    .inner_margin(INPUT_MARGIN);
                input_frame.show(ui, |ui| {
                    let inner_h = (INPUT_HEIGHT
                        - f32::from(i16::from(INPUT_MARGIN.top) + i16::from(INPUT_MARGIN.bottom)))
                    .max(0.0);
                    let text = if snapshot.query.is_empty() {
                        egui::RichText::new(snapshot.placeholder.clone())
                            .font(input_font_id.clone())
                            .color(dim)
                    } else {
                        egui::RichText::new(format!("{}▏", snapshot.query))
                            .font(input_font_id.clone())
                            .color(fg)
                    };
                    let (rect, _) = ui.allocate_exact_size(
                        vec2(ui.available_width(), inner_h),
                        egui::Sense::hover(),
                    );
                    let mut inner = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    inner.label(text);
                });

                ui.add_space(SECTION_GAP);

                if snapshot.items.is_empty() {
                    ui.label(
                        egui::RichText::new("No results")
                            .size(ITEM_FONT_SIZE)
                            .color(dim),
                    );
                    return;
                }

                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        for (i, item) in snapshot.items.iter().enumerate() {
                            let selected = i == snapshot.selected;
                            let item_bg = if selected {
                                style.item_selected_bg
                            } else {
                                style.item_bg
                            };
                            let fill = Self::rgba(item_bg, BG_ALPHA);
                            let row = Frame::new()
                                .fill(fill)
                                .corner_radius(egui::CornerRadius::same(ROW_RADIUS as u8))
                                .inner_margin(ITEM_MARGIN);
                            row.show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                let label_job = Self::layout_label_with_matches(
                                    item.label.as_str(),
                                    &item.label_match_indices,
                                    fg,
                                    match_fg,
                                    item_font_id.clone(),
                                );
                                ui.label(label_job);
                                if let Some(sub) = item.sublabel.as_ref() {
                                    ui.add_space(2.0);
                                    ui.label(
                                        egui::RichText::new(sub.clone())
                                            .font(sublabel_font_id.clone())
                                            .color(dim),
                                    );
                                }
                            });
                            if i + 1 != snapshot.items.len() {
                                ui.add_space(ITEM_GAP);
                            }
                        }
                    });
            });
        });

        self.last_pos = Some(pos);
        self.last_size = Some(size);
    }
}
