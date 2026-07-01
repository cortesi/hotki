//! Interactive selector popup rendering.

use egui::{
    CentralPanel, Color32, Context, Frame, Margin, Stroke, Vec2, ViewportBuilder, epaint::Shadow,
    style::ScrollStyle, text::LayoutJob, vec2,
};
use eguidev::{DevMcp, DevScrollAreaExt, DevUiExt, WidgetValue, container};
use hotki_protocol::{FontWeight, SelectorItemSnapshot, SelectorSnapshot, SelectorStyle};

use crate::{
    devtools,
    display::{DisplayMetrics, WindowGeometry},
    fonts,
    nswindow::{apply_transparent_rounded, frame_by_title, set_on_all_spaces},
    overlay::OverlayWindow,
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
    /// Shared overlay viewport state.
    viewport: OverlayWindow,
    /// Effective selector style (colors).
    style: SelectorStyle,
    /// Current selector snapshot, when visible.
    state: Option<SelectorSnapshot>,
    /// Whether NSWindow style has been applied for the current selector session.
    window_configured: bool,
}

/// Data needed to render one selector frame.
struct SelectorViewModel {
    /// Desired viewport geometry.
    geometry: WindowGeometry,
    /// Snapshot to paint.
    snapshot: SelectorSnapshot,
}

/// Values derived once per selector frame and shared by render helpers.
#[derive(Clone)]
struct SelectorRenderAssets {
    /// Effective selector style.
    style: SelectorStyle,
    /// Header font.
    title_font_id: egui::FontId,
    /// Query font.
    input_font_id: egui::FontId,
    /// Item label font.
    item_font_id: egui::FontId,
    /// Item sublabel font.
    sublabel_font_id: egui::FontId,
    /// Primary foreground color.
    fg: Color32,
    /// Dimmed foreground color.
    dim: Color32,
    /// Match highlight color.
    match_fg: Color32,
}

impl SelectorWindow {
    /// Create a new selector window instance.
    pub fn new(style: &SelectorStyle) -> Self {
        Self {
            viewport: OverlayWindow::new("hotki_selector"),
            style: style.clone(),
            state: None,
            window_configured: false,
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
        self.viewport.set_display_metrics(metrics);
    }

    /// Replace the selector snapshot state (shows the selector if needed).
    pub fn set_state(&mut self, snapshot: SelectorSnapshot) {
        if self.state.is_none() {
            self.viewport.reset_geometry();
            self.window_configured = false;
        }
        self.state = Some(snapshot);
    }

    /// Hide the selector window immediately.
    pub fn hide(&mut self, ctx: &Context) {
        self.state = None;
        self.viewport.reset_geometry();
        self.window_configured = false;
        self.viewport.hide(ctx);
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
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(snapshot.title.clone(), self.title_font_id(), Color32::WHITE)
                    .size()
                    .y
            })
        };

        let input_h = INPUT_HEIGHT;

        let items_h = ctx.fonts_mut(|f| {
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

    /// Build the data needed for one selector render pass.
    fn view_model(&self, ctx: &Context, snapshot: &SelectorSnapshot) -> SelectorViewModel {
        let size = self.desired_size(ctx, snapshot);
        SelectorViewModel {
            geometry: self
                .viewport
                .display()
                .active_bounds()
                .centered_geometry(size),
            snapshot: snapshot.clone(),
        }
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

    /// Render an optional selector item sublabel line.
    fn render_sublabel(
        ui: &mut egui::Ui,
        index: usize,
        sublabel: Option<&str>,
        assets: &SelectorRenderAssets,
    ) {
        let Some(sub) = sublabel else {
            return;
        };
        ui.add_space(2.0);
        ui.dev_label(
            format!("selector.item.{index}.sublabel"),
            egui::RichText::new(sub)
                .font(assets.sublabel_font_id.clone())
                .color(assets.dim),
        );
    }

    /// Render and update the selector viewport.
    pub fn render(&mut self, ctx: &Context, devmcp: &DevMcp) {
        let Some(snapshot) = self.state.as_ref() else {
            self.viewport.hide(ctx);
            return;
        };

        let model = self.view_model(ctx, snapshot);

        let mut builder = ViewportBuilder::default()
            .with_title("Hotki Selector")
            .with_decorations(false)
            .with_always_on_top()
            .with_transparent(true)
            .with_has_shadow(false)
            .with_visible(true)
            .with_inner_size(model.geometry.size);
        builder = self
            .viewport
            .sync_builder(ctx, builder, model.geometry.pos, model.geometry.size);

        let assets = self.render_assets();
        let snapshot = model.snapshot.clone();
        devtools::pump_viewport_input(devmcp, ctx, self.viewport.id());
        ctx.show_viewport_immediate(self.viewport.id(), builder, move |vp_ui, _| {
            devtools::viewport_frame(devmcp, vp_ui, |vp_ui| {
                let border = Self::rgba(assets.style.border, BG_ALPHA);
                let bg = Self::rgba(assets.style.bg, BG_ALPHA);
                let mut frame = Frame::new()
                    .fill(bg)
                    .stroke(Stroke::new(1.0_f32, border))
                    .corner_radius(egui::CornerRadius::same(SELECTOR_RADIUS as u8))
                    .inner_margin(Margin::same(SELECTOR_PADDING as i8));
                frame.shadow = Shadow {
                    offset: [0, 6],
                    blur: 24,
                    spread: 0,
                    color: Self::rgba(assets.style.shadow, 64),
                };

                CentralPanel::default().frame(frame).show(vp_ui, |ui| {
                    container(ui, "selector.panel", |ui| {
                        Self::render_panel(ui, &snapshot, &assets);
                    });
                });
            });
        });

        if !self.window_configured && frame_by_title("Hotki Selector").is_some() {
            if let Err(e) = apply_transparent_rounded("Hotki Selector", SELECTOR_RADIUS as f64) {
                tracing::error!("{}", e);
            }
            if let Err(e) = set_on_all_spaces("Hotki Selector") {
                tracing::error!("{}", e);
            }
            self.window_configured = true;
        }

        self.viewport
            .record_geometry(model.geometry.pos, model.geometry.size);
    }

    /// Build selector render values that are reused throughout one frame.
    fn render_assets(&self) -> SelectorRenderAssets {
        let fg = Color32::from_rgba_unmultiplied(230, 230, 230, 255);
        let dim = Color32::from_rgba_unmultiplied(160, 160, 160, 255);
        let match_fg = Color32::from_rgb(
            self.style.match_fg.0,
            self.style.match_fg.1,
            self.style.match_fg.2,
        );
        SelectorRenderAssets {
            style: self.style.clone(),
            title_font_id: self.title_font_id(),
            input_font_id: self.input_font_id(),
            item_font_id: self.item_font_id(),
            sublabel_font_id: self.sublabel_font_id(),
            fg,
            dim,
            match_fg,
        }
    }

    /// Render selector panel contents.
    fn render_panel(ui: &mut egui::Ui, snapshot: &SelectorSnapshot, assets: &SelectorRenderAssets) {
        ui.spacing_mut().item_spacing.y = 0.0;
        devtools::value_anchor(
            ui,
            "selector.placeholder",
            WidgetValue::Text(snapshot.placeholder.clone()),
        );
        devtools::value_anchor(
            ui,
            "selector.selected_index",
            WidgetValue::Int(snapshot.selected as i64),
        );
        Self::render_header(ui, snapshot, assets);
        Self::render_query(ui, snapshot, assets);
        ui.add_space(SECTION_GAP);
        Self::render_items(ui, snapshot, assets);
    }

    /// Render the optional selector title and match count.
    fn render_header(
        ui: &mut egui::Ui,
        snapshot: &SelectorSnapshot,
        assets: &SelectorRenderAssets,
    ) {
        if snapshot.title.trim().is_empty() {
            return;
        }
        let title_fmt = egui::TextFormat {
            color: assets.fg,
            font_id: assets.title_font_id.clone(),
            ..Default::default()
        };
        ui.horizontal(|ui| {
            let mut job = LayoutJob::default();
            job.append(snapshot.title.as_str(), 0.0, title_fmt);
            ui.dev_label("selector.title", job);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.dev_label(
                    "selector.result_count",
                    egui::RichText::new(format!("{}", snapshot.total_matches))
                        .size(SUBLABEL_FONT_SIZE)
                        .color(assets.dim),
                );
            });
        });
        ui.add_space(SECTION_GAP);
    }

    /// Render the selector query or placeholder field.
    fn render_query(ui: &mut egui::Ui, snapshot: &SelectorSnapshot, assets: &SelectorRenderAssets) {
        let input_bg = Self::rgba(assets.style.input_bg, BG_ALPHA);
        let input_frame = Frame::new()
            .fill(input_bg)
            .corner_radius(egui::CornerRadius::same(ROW_RADIUS as u8))
            .inner_margin(INPUT_MARGIN);
        input_frame.show(ui, |ui| {
            let margin = i16::from(INPUT_MARGIN.top) + i16::from(INPUT_MARGIN.bottom);
            let inner_h = (INPUT_HEIGHT - f32::from(margin)).max(0.0);
            let text = selector_query_text(snapshot, assets);
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), inner_h), egui::Sense::hover());
            let mut inner = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            inner.dev_label("selector.query", text);
        });
    }

    /// Render selector item rows or the empty state.
    fn render_items(ui: &mut egui::Ui, snapshot: &SelectorSnapshot, assets: &SelectorRenderAssets) {
        if snapshot.items.is_empty() {
            ui.dev_label(
                "selector.empty",
                egui::RichText::new("No results")
                    .size(ITEM_FONT_SIZE)
                    .color(assets.dim),
            );
            return;
        }
        ui.scope(|ui| {
            ui.style_mut().spacing.scroll = ScrollStyle::floating();
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .dev_show(ui, "selector.scroll", |ui| {
                    for (i, item) in snapshot.items.iter().enumerate() {
                        Self::render_item(ui, i, item, i == snapshot.selected, assets);
                        if i + 1 != snapshot.items.len() {
                            ui.add_space(ITEM_GAP);
                        }
                    }
                });
        });
    }

    /// Render a single selector item row.
    fn render_item(
        ui: &mut egui::Ui,
        index: usize,
        item: &SelectorItemSnapshot,
        selected: bool,
        assets: &SelectorRenderAssets,
    ) {
        container(ui, format!("selector.item.{index}"), |ui| {
            devtools::value_anchor(
                ui,
                format!("selector.item.{index}.selected"),
                WidgetValue::Bool(selected),
            );
            devtools::value_anchor(
                ui,
                format!("selector.item.{index}.match_indices"),
                WidgetValue::Text(match_indices_text(&item.label_match_indices)),
            );
            let item_bg = if selected {
                assets.style.item_selected_bg
            } else {
                assets.style.item_bg
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
                    assets.fg,
                    assets.match_fg,
                    assets.item_font_id.clone(),
                );
                ui.dev_label(format!("selector.item.{index}.label"), label_job);
                Self::render_sublabel(ui, index, item.sublabel.as_deref(), assets);
            });
        });
    }
}

/// Build the visible query text for the selector input row.
fn selector_query_text(
    snapshot: &SelectorSnapshot,
    assets: &SelectorRenderAssets,
) -> egui::RichText {
    if snapshot.query.is_empty() {
        egui::RichText::new(snapshot.placeholder.clone())
            .font(assets.input_font_id.clone())
            .color(assets.dim)
    } else {
        egui::RichText::new(format!("{}▏", snapshot.query))
            .font(assets.input_font_id.clone())
            .color(assets.fg)
    }
}

/// Render match codepoint indices as a stable comma-separated value.
fn match_indices_text(indices: &[u32]) -> String {
    indices
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use egui::{Color32, text::ByteIndex};

    use super::SelectorWindow;

    #[test]
    fn layout_label_with_matches_splits_highlighted_codepoints() {
        let job = SelectorWindow::layout_label_with_matches(
            "abcd",
            &[1, 2],
            Color32::WHITE,
            Color32::LIGHT_BLUE,
            egui::FontId::default(),
        );

        assert_eq!(job.text, "abcd");
        assert_eq!(job.sections.len(), 3);
        assert_eq!(job.sections[0].byte_range, ByteIndex(0)..ByteIndex(1));
        assert_eq!(job.sections[1].byte_range, ByteIndex(1)..ByteIndex(3));
        assert_eq!(job.sections[2].byte_range, ByteIndex(3)..ByteIndex(4));
        assert_eq!(job.sections[1].format.color, Color32::LIGHT_BLUE);
    }
}
