//! Compact main window with runtime notice, recent activity, and commands.

use egui::{
    CentralPanel, Color32, Context, Label, Layout, Panel, Pos2, RichText, ScrollArea, Sense, Vec2,
    ViewportBuilder, ViewportCommand, vec2,
};
use eguidev::{
    DevMcp, DevUiExt, WidgetMeta, WidgetRole, WidgetValue, container, track_response_full,
};
use hotki_protocol::{NotifyKind, NotifyTheme};

use crate::{
    devtools,
    display::{DisplayBounds, DisplayMetrics, WindowGeometry},
    health::{NoticeTone, PrimaryAction, RuntimeHealth, RuntimeNotice, RuntimePresentation},
    notification::BacklogEntry,
    nswindow,
    overlay::OverlayWindow,
};

/// Native title and AppKit lookup identity for the main window.
const MAIN_WINDOW_TITLE: &str = "Hotki";
/// Default inner size for a new session.
const DEFAULT_SIZE: Vec2 = Vec2::new(560.0, 360.0);
/// Minimum supported inner size.
const MIN_SIZE: Vec2 = Vec2::new(440.0, 280.0);
/// Content margin for the notice, activity list, and footer.
const CONTENT_PAD: f32 = 14.0;
/// Compact vertical gap between related content.
const CONTENT_GAP: f32 = 8.0;
/// Width reserved for a fixed-format `HH:MM` activity timestamp.
const TIMESTAMP_WIDTH: f32 = 34.0;

/// One command emitted by the main-window renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainWindowCommand {
    /// Execute the state-derived primary action.
    Primary(PrimaryAction),
    /// Open or raise the dedicated logs window.
    ShowLogs,
}

/// Session geometry with global outer position and egui inner size kept separate.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MainWindowGeometry {
    /// Decorated top-left position in global top-left coordinates.
    pos: Pos2,
    /// Egui viewport inner size.
    inner_size: Vec2,
}

impl MainWindowGeometry {
    /// Convert an AppKit outer frame and egui inner size into restore geometry.
    fn from_appkit_outer_frame(
        bounds: DisplayBounds,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        inner_size: Vec2,
    ) -> Self {
        let outer = WindowGeometry::from_bottom_left_frame(bounds, x, y, width, height);
        Self {
            pos: outer.pos,
            inner_size,
        }
    }

    /// Return the shape accepted by shared display clamping.
    fn window_geometry(self) -> WindowGeometry {
        WindowGeometry::new(self.pos, self.inner_size)
    }
}

/// State and rendering for Hotki's main overview window.
pub struct MainWindow {
    /// Whether the viewport should be visible.
    visible: bool,
    /// Shared viewport state and display metrics.
    viewport: OverlayWindow,
    /// Request default size on the next first-session open.
    want_initial_size: bool,
    /// Apply saved geometry once after opening.
    restore_pending: bool,
    /// Focus the viewport on its next frame.
    want_focus: bool,
    /// Last session-local position and inner size.
    last_saved: Option<MainWindowGeometry>,
    /// Current notification theme used only for severity marks.
    theme: NotifyTheme,
    /// Complete presentation derived from the latest runtime snapshot.
    presentation: RuntimePresentation,
}

impl MainWindow {
    /// Construct a hidden main window using the current notification theme.
    pub fn new(theme: NotifyTheme) -> Self {
        Self {
            visible: false,
            viewport: OverlayWindow::new("hotki_main"),
            want_initial_size: false,
            restore_pending: false,
            want_focus: false,
            last_saved: None,
            theme,
            presentation: RuntimeHealth::default().presentation(),
        }
    }

    /// Show and focus the main window.
    pub fn show(&mut self) {
        self.visible = true;
        self.viewport.reset_geometry();
        self.want_initial_size = true;
        self.restore_pending = true;
        self.want_focus = true;
    }

    /// Toggle main-window visibility.
    pub fn toggle(&mut self) {
        if self.visible {
            self.hide();
        } else {
            self.show();
        }
    }

    /// Hide the main window without discarding session geometry.
    pub fn hide(&mut self) {
        self.visible = false;
        self.viewport.reset_geometry();
    }

    /// Update severity-mark colors after a runtime style change.
    pub fn update_theme(&mut self, theme: NotifyTheme) {
        self.theme = theme;
    }

    /// Update display metrics used for session geometry restore.
    pub fn set_display_metrics(&mut self, metrics: DisplayMetrics) {
        self.viewport.set_display_metrics(metrics);
    }

    /// Replace the presentation model from one complete runtime snapshot.
    pub(crate) fn set_runtime_health(&mut self, health: &RuntimeHealth) {
        self.presentation = health.presentation();
        if health.is_shutting_down() {
            self.hide();
        }
    }

    /// Render the main viewport and return at most one clicked command.
    pub fn render(
        &mut self,
        ctx: &Context,
        backlog: &[BacklogEntry],
        devmcp: &DevMcp,
    ) -> Option<MainWindowCommand> {
        if !self.visible {
            self.viewport.hide(ctx);
            return None;
        }

        let mut builder = ViewportBuilder::default()
            .with_title(MAIN_WINDOW_TITLE)
            .with_visible(true)
            .with_decorations(true)
            .with_resizable(true)
            .with_min_inner_size(MIN_SIZE)
            .with_transparent(false)
            .with_has_shadow(true);
        if self.want_initial_size && self.last_saved.is_none() {
            builder = builder.with_inner_size(DEFAULT_SIZE);
        }
        self.want_initial_size = false;

        let mut command = None;
        ctx.show_viewport_immediate(self.viewport.id(), builder, |vp_ui, _| {
            devtools::viewport_frame(devmcp, vp_ui, "main", "main.root", |vp_ui| {
                let window_ctx = vp_ui.ctx().clone();
                if window_ctx.input(|input| input.viewport().close_requested()) {
                    self.visible = false;
                    window_ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                    self.save_geometry(&window_ctx);
                    return;
                }
                self.restore_geometry(&window_ctx);
                if self.want_focus {
                    window_ctx.send_viewport_cmd_to(self.viewport.id(), ViewportCommand::Focus);
                    self.want_focus = false;
                }
                CentralPanel::default().show(vp_ui, |ui| {
                    command = self.render_contents(ui, backlog);
                });
                self.save_geometry(&window_ctx);
            });
        });
        command
    }

    /// Restore saved geometry once, clamped to the active display.
    fn restore_geometry(&mut self, ctx: &Context) {
        if !self.restore_pending {
            return;
        }
        if let Some(stored) = self.last_saved {
            let clamped = self
                .viewport
                .display()
                .active_bounds()
                .clamp_geometry(stored.window_geometry(), vec2(100.0, 80.0));
            ctx.send_viewport_cmd_to(self.viewport.id(), ViewportCommand::InnerSize(clamped.size));
            ctx.send_viewport_cmd_to(
                self.viewport.id(),
                ViewportCommand::OuterPosition(clamped.pos),
            );
        }
        self.restore_pending = false;
    }

    /// Capture current AppKit position and egui inner size for this session.
    fn save_geometry(&mut self, ctx: &Context) {
        let viewport = ctx.input(|input| input.viewport().clone());
        let Some(inner_size) = viewport.inner_rect.map(|rect| rect.size()) else {
            return;
        };
        let Some((x, y, width, height)) = nswindow::frame_by_title(MAIN_WINDOW_TITLE) else {
            return;
        };
        let current = MainWindowGeometry::from_appkit_outer_frame(
            self.viewport.display().active_bounds(),
            x,
            y,
            width,
            height,
            inner_size,
        );
        if self.last_saved != Some(current) {
            self.last_saved = Some(current);
            self.viewport
                .record_geometry(current.pos, current.inner_size);
        }
    }

    /// Render the fixed footer and notice around the scrolling activity list.
    fn render_contents(
        &self,
        ui: &mut egui::Ui,
        backlog: &[BacklogEntry],
    ) -> Option<MainWindowCommand> {
        let mut command = None;
        Panel::bottom("main.footer.panel")
            .resizable(false)
            .show(ui, |ui| {
                ui.add_space(CONTENT_GAP);
                command = self.render_footer(ui);
                ui.add_space(CONTENT_GAP);
            });
        if let Some(notice) = self.presentation.notice.as_ref() {
            Panel::top("main.notice.panel")
                .resizable(false)
                .show(ui, |ui| self.render_notice(ui, notice));
        }
        CentralPanel::default().show(ui, |ui| self.render_activity(ui, backlog));
        command
    }

    /// Render the optional state notice with a semantic mark.
    fn render_notice(&self, ui: &mut egui::Ui, notice: &RuntimeNotice) {
        ui.add_space(CONTENT_PAD);
        container(ui, "main.notice", |ui| {
            devtools::value_anchor(
                ui,
                "main.notice.tone",
                WidgetValue::Text(notice.tone.label().to_string()),
            );
            devtools::value_anchor(
                ui,
                "main.notice.progress",
                WidgetValue::Bool(notice.progress),
            );
            ui.horizontal(|ui| {
                self.render_notice_mark(ui, notice);
                ui.add_space(CONTENT_GAP);
                ui.vertical(|ui| {
                    ui.dev_label("main.notice.title", RichText::new(notice.title).strong());
                    if let Some(detail) = notice.detail.as_deref() {
                        ui.dev_label("main.notice.detail", detail);
                    }
                });
            });
        });
        ui.add_space(CONTENT_PAD);
        ui.dev_separator("main.notice.separator");
    }

    /// Render either an animated spinner or a static painted notice circle.
    fn render_notice_mark(&self, ui: &mut egui::Ui, notice: &RuntimeNotice) {
        if notice.progress {
            ui.dev_spinner("main.notice.mark");
            return;
        }
        let color = match notice.tone {
            NoticeTone::Progress => ui.visuals().hyperlink_color,
            NoticeTone::Attention => ui.visuals().warn_fg_color,
            NoticeTone::Error => ui.visuals().error_fg_color,
        };
        let (rect, response) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 5.0, color);
        track_response_full(
            "main.notice.mark",
            &response,
            WidgetMeta {
                role: WidgetRole::Unknown,
                label: Some(notice.tone.label().to_string()),
                value: Some(WidgetValue::Text(notice.tone.label().to_string())),
                visible: true,
                ..Default::default()
            },
        );
    }

    /// Render recent activity newest first inside the only scrolling region.
    fn render_activity(&self, ui: &mut egui::Ui, backlog: &[BacklogEntry]) {
        ui.add_space(CONTENT_PAD);
        container(ui, "main.activity", |ui| {
            if backlog.is_empty() {
                ui.dev_label(
                    "main.activity.empty",
                    RichText::new("No recent activity.").weak(),
                );
                return;
            }
            ScrollArea::vertical()
                .id_salt("main.activity.scroll")
                .auto_shrink(false)
                .show(ui, |ui| {
                    for (index, entry) in backlog.iter().enumerate() {
                        if index > 0 {
                            ui.dev_separator(format!("main.activity.{index}.separator"));
                        }
                        self.render_activity_row(ui, index, entry);
                    }
                });
        });
        ui.add_space(CONTENT_PAD);
    }

    /// Render one timestamped activity row.
    fn render_activity_row(&self, ui: &mut egui::Ui, index: usize, entry: &BacklogEntry) {
        container(ui, format!("main.activity.{index}.row"), |ui| {
            ui.add_space(CONTENT_GAP);
            ui.horizontal_top(|ui| {
                let color = severity_color(&self.theme, entry.kind);
                let (rect, response) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, color);
                track_response_full(
                    format!("main.activity.{index}.mark"),
                    &response,
                    WidgetMeta {
                        role: WidgetRole::Unknown,
                        label: Some(kind_label(entry.kind).to_string()),
                        value: Some(WidgetValue::Text(kind_label(entry.kind).to_string())),
                        visible: true,
                        ..Default::default()
                    },
                );
                ui.add_space(CONTENT_GAP);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.dev_label(
                            format!("main.activity.{index}.title"),
                            RichText::new(&entry.title).strong(),
                        );
                        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                            let text = entry.received_at.format("%H:%M").to_string();
                            let response = ui.add_sized(
                                [TIMESTAMP_WIDTH, ui.spacing().interact_size.y],
                                Label::new(RichText::new(&text).weak()),
                            );
                            track_response_full(
                                format!("main.activity.{index}.time"),
                                &response,
                                WidgetMeta {
                                    role: WidgetRole::Label,
                                    label: Some(text.clone()),
                                    value: Some(WidgetValue::Text(text)),
                                    visible: true,
                                    ..Default::default()
                                },
                            );
                        });
                    });
                    let response = ui.add(Label::new(&entry.text).wrap().selectable(true));
                    track_response_full(
                        format!("main.activity.{index}.text"),
                        &response,
                        WidgetMeta {
                            role: WidgetRole::Selectable,
                            label: Some(entry.text.clone()),
                            value: Some(WidgetValue::Text(entry.text.clone())),
                            visible: true,
                            ..Default::default()
                        },
                    );
                });
            });
            ui.add_space(CONTENT_GAP);
        });
    }

    /// Render the state-derived leading command and stable trailing logs command.
    fn render_footer(&self, ui: &mut egui::Ui) -> Option<MainWindowCommand> {
        let mut command = None;
        container(ui, "main.footer", |ui| {
            ui.horizontal(|ui| {
                if let Some(action) = self.presentation.primary_action
                    && ui
                        .dev_button("main.footer.primary", action.label())
                        .clicked()
                {
                    command = Some(MainWindowCommand::Primary(action));
                }
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.dev_button("main.footer.logs", "Show Logs").clicked() {
                        command = Some(MainWindowCommand::ShowLogs);
                    }
                });
            });
        });
        command
    }
}

/// Severity-mark color derived from the configured notification theme.
fn severity_color(theme: &NotifyTheme, kind: NotifyKind) -> Color32 {
    let (red, green, blue) = theme.style_for(kind).title_fg;
    Color32::from_rgb(red, green, blue)
}

/// Stable human-readable severity name exposed to UI automation.
fn kind_label(kind: NotifyKind) -> &'static str {
    match kind {
        NotifyKind::Info | NotifyKind::Ignore => "info",
        NotifyKind::Warn => "warn",
        NotifyKind::Error => "error",
        NotifyKind::Success => "success",
    }
}

#[cfg(test)]
mod tests {
    use egui::{pos2, vec2};
    use hotki_protocol::{DisplayFrame, DisplaysSnapshot, NotifyTheme};

    use super::{DEFAULT_SIZE, MIN_SIZE, MainWindow, MainWindowGeometry};
    use crate::display::{DisplayMetrics, WindowGeometry};

    #[test]
    fn target_geometry_is_compact_and_resizable() {
        assert_eq!(DEFAULT_SIZE, vec2(560.0, 360.0));
        assert_eq!(MIN_SIZE, vec2(440.0, 280.0));
    }

    #[test]
    fn saved_geometry_keeps_inner_size_separate_from_decorated_frame() {
        let mut window = MainWindow::new(NotifyTheme::default());
        window.set_display_metrics(DisplayMetrics::from_snapshot(&DisplaysSnapshot {
            global_top: 900.0,
            active: Some(DisplayFrame {
                id: 1,
                x: 0.0,
                y: 0.0,
                width: 1000.0,
                height: 800.0,
            }),
            displays: Vec::new(),
        }));
        let saved = MainWindowGeometry::from_appkit_outer_frame(
            window.viewport.display().active_bounds(),
            80.0,
            120.0,
            560.0,
            388.0,
            DEFAULT_SIZE,
        );
        let restored = window
            .viewport
            .display()
            .active_bounds()
            .clamp_geometry(saved.window_geometry(), vec2(100.0, 80.0));

        assert_eq!(saved.pos, pos2(80.0, 392.0));
        assert_eq!(saved.inner_size, DEFAULT_SIZE);
        assert_eq!(restored.size, DEFAULT_SIZE);
    }

    #[test]
    fn clamp_keeps_minimum_visible_on_the_active_display() {
        let mut window = MainWindow::new(NotifyTheme::default());
        window.set_display_metrics(DisplayMetrics::from_snapshot(&DisplaysSnapshot {
            global_top: 900.0,
            active: Some(DisplayFrame {
                id: 1,
                x: 50.0,
                y: 100.0,
                width: 500.0,
                height: 300.0,
            }),
            displays: Vec::new(),
        }));
        let clamped = window.viewport.display().active_bounds().clamp_geometry(
            WindowGeometry::new(pos2(0.0, 0.0), vec2(900.0, 20.0)),
            vec2(100.0, 80.0),
        );

        assert_eq!(clamped.pos, pos2(50.0, 500.0));
        assert_eq!(clamped.size, vec2(500.0, 80.0));
    }
}
