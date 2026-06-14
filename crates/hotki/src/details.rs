use std::{fs, path::PathBuf};

use egui::{
    CentralPanel, Color32, Context, Layout, RichText, ScrollArea, ViewportBuilder, ViewportCommand,
    vec2,
};
use egui_extras::{Column, TableBuilder};
use eguidev::{
    DevMcp, DevUiExt, WidgetMeta, WidgetRole, WidgetValue, container, track_response_full,
};
use hotki_protocol::{NotifyKind, NotifyTheme};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    devtools,
    display::{DisplayMetrics, WindowGeometry},
    logs::{self, LogEntry, Side},
    notification::BacklogEntry,
    nswindow,
    overlay::OverlayWindow,
    runtime::ControlMsg,
};

/// Horizontal/vertical padding around details content.
const DETAILS_PAD: f32 = 12.0;
/// Table header height in logical pixels.
const HEADER_H: f32 = 22.0;
/// Minimum row height for notifications table.
const ROW_HEIGHT_MIN: f32 = 20.0;
/// Initial width for the Kind column.
const COL_KIND_INIT: f32 = 90.0;
/// Initial width for the Title column.
const COL_TITLE_INIT: f32 = 180.0;
/// Minimum width for the Kind column.
const COL_KIND_MIN: f32 = 70.0;
/// Minimum width for the Title column.
const COL_TITLE_MIN: f32 = 120.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Active tab within the Details window.
enum Tab {
    /// Notifications list view.
    Notifications,
    /// Configuration file view.
    Config,
    /// About screen.
    About,
    /// In-app log viewer.
    Logs,
}

/// Data needed to render the details Config tab.
struct ConfigTabModel<'a> {
    /// Display text for the active config path.
    path_text: String,
    /// Optional inline read error.
    error: Option<&'a str>,
    /// Current config contents.
    contents: &'a str,
}

/// State and rendering for the Details window.
pub struct Details {
    /// Whether the window is currently visible.
    visible: bool,
    /// Whether AppKit cursor rects are disabled for the window.
    cursor_rects_disabled: bool,
    /// Shared overlay viewport state.
    viewport: OverlayWindow,
    /// Request an initial default size on next show.
    want_initial_size: bool,
    /// Apply previously saved geometry once after opening.
    restore_pending: bool,
    /// Request focus on next frame.
    want_focus: bool,
    /// Last known geometry for this session.
    last_saved: Option<WindowGeometry>,
    /// Currently active tab.
    active_tab: Tab,
    /// Current notification theme for colors.
    theme: NotifyTheme,
    /// Optional path to the hotki config file.
    config_path: Option<PathBuf>,
    /// Current contents of the config file (for display).
    config_contents: String,
    /// Last config read error (if any) for inline display.
    config_error: Option<String>,
    /// Control channel to background runtime.
    tx_ctrl: Option<UnboundedSender<ControlMsg>>,
    /// Generation for the cached log rows.
    log_generation: u64,
    /// Cached log rows for the Logs tab.
    log_rows: Vec<LogEntry>,
}

impl Details {
    /// Construct a new Details window with the given theme.
    pub fn new(theme: NotifyTheme) -> Self {
        Self {
            visible: false,
            cursor_rects_disabled: false,
            viewport: OverlayWindow::new("hotki_details"),
            want_initial_size: false,
            restore_pending: false,
            want_focus: false,
            last_saved: None,
            active_tab: Tab::Notifications,
            theme,
            config_path: None,
            config_contents: String::new(),
            config_error: None,
            tx_ctrl: None,
            log_generation: u64::MAX,
            log_rows: Vec::new(),
        }
    }

    /// Make the window visible and request initial layout and focus.
    pub fn show(&mut self) {
        self.visible = true;
        self.viewport.reset_geometry();
        self.want_initial_size = true;
        self.restore_pending = true;
        self.want_focus = true;
    }

    /// Toggle visibility of the window.
    pub fn toggle(&mut self) {
        if self.visible {
            self.visible = false;
        } else {
            self.show();
        }
    }

    /// Hide the window.
    pub fn hide(&mut self) {
        self.visible = false;
        self.viewport.reset_geometry();
    }

    /// Update the active theme used for colors.
    pub fn update_theme(&mut self, theme: NotifyTheme) {
        self.theme = theme;
    }

    /// Update display metrics used to clamp and restore window geometry.
    pub fn set_display_metrics(&mut self, metrics: DisplayMetrics) {
        self.viewport.set_display_metrics(metrics);
    }

    /// Query the current Details window geometry converted to a top-left origin.
    fn current_geom_top_left(&self) -> Option<WindowGeometry> {
        // NSWindow uses bottom-left origin; convert to global top-left expected by winit.
        let (x_b, y_b, w, h) = nswindow::frame_by_title("Details")?;
        Some(WindowGeometry::from_bottom_left_frame(
            self.viewport.display().active_bounds(),
            x_b,
            y_b,
            w,
            h,
        ))
    }

    /// Clamp a window geometry to the current active screen's visible frame.
    fn clamp_to_active_frame(&self, geometry: WindowGeometry) -> WindowGeometry {
        self.viewport
            .display()
            .active_bounds()
            .clamp_geometry(geometry, vec2(100.0, 80.0))
    }

    /// Set the config file path shown in the Config tab and load.
    pub fn set_config_path(&mut self, path: Option<PathBuf>) {
        self.config_path = path;
        self.reload_config_contents();
    }

    /// Reload the config file contents into memory for display.
    pub fn reload_config_contents(&mut self) {
        // Load the config file contents if we have a path
        if let Some(ref p) = self.config_path {
            match fs::read_to_string(p) {
                Ok(s) => {
                    self.config_contents = s;
                    self.config_error = None;
                }
                Err(e) => {
                    // Clear stale contents and record error for inline display
                    self.config_contents.clear();
                    let msg = format!("Failed to read {}: {}", p.display(), e);
                    // Notify the user via notification center if available
                    if let Some(ref tx) = self.tx_ctrl
                        && let Err(e2) = tx.send(ControlMsg::Notice {
                            kind: NotifyKind::Error,
                            title: "Config".to_string(),
                            text: msg.clone(),
                        })
                    {
                        tracing::warn!("failed to enqueue notice: {}", e2);
                    }
                    self.config_error = Some(msg);
                    // Also log for diagnostics
                    tracing::warn!("{}", e);
                }
            }
        }
    }

    /// Set the control channel sender to communicate with the runtime.
    pub fn set_control_sender(&mut self, tx: UnboundedSender<ControlMsg>) {
        self.tx_ctrl = Some(tx);
    }

    /// Build the display model for the Config tab.
    fn config_tab_model(&self) -> ConfigTabModel<'_> {
        ConfigTabModel {
            path_text: self
                .config_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "No config file loaded".to_string()),
            error: self.config_error.as_deref(),
            contents: &self.config_contents,
        }
    }

    /// Render the window and its active tab.
    pub fn render(&mut self, ctx: &Context, backlog: &[BacklogEntry], devmcp: &DevMcp) {
        if !self.visible {
            self.ensure_cursor_rects_enabled();
            self.viewport.hide(ctx);
            return;
        }

        let mut builder = ViewportBuilder::default()
            .with_title("Details")
            .with_visible(true)
            .with_decorations(true)
            .with_resizable(true)
            .with_transparent(false)
            .with_has_shadow(true);
        if self.want_initial_size {
            builder = builder.with_inner_size(vec2(720.0, 420.0));
            self.want_initial_size = false;
        }

        devtools::pump_viewport_input(devmcp, ctx, self.viewport.id());
        ctx.show_viewport_immediate(self.viewport.id(), builder, |vp_ui, _| {
            devtools::viewport_frame(devmcp, vp_ui, |vp_ui| {
                let wctx = vp_ui.ctx().clone();
                if wctx.input(|i| i.viewport().close_requested()) {
                    // Close via decorations; stop rendering next frame
                    self.visible = false;
                    wctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    // Remember final geometry for this session only
                    if let Some(cur) = self.current_geom_top_left() {
                        self.last_saved = Some(cur);
                    }
                    return;
                }
                self.ensure_cursor_rects_disabled();
                // Apply saved geometry once after opening (clamped to active screen)
                if self.restore_pending {
                    if let Some(stored) = self.last_saved {
                        let clamped = self.clamp_to_active_frame(stored);
                        wctx.send_viewport_cmd_to(
                            self.viewport.id(),
                            ViewportCommand::InnerSize(clamped.size),
                        );
                        wctx.send_viewport_cmd_to(
                            self.viewport.id(),
                            ViewportCommand::OuterPosition(clamped.pos),
                        );
                    }
                    self.restore_pending = false;
                }
                // Focus the window when toggled on
                if self.want_focus {
                    wctx.send_viewport_cmd_to(self.viewport.id(), ViewportCommand::Focus);
                    self.want_focus = false;
                }
                CentralPanel::default().show_inside(vp_ui, |ui| {
                    container(ui, "details.root", |ui| {
                        self.render_contents(ui, backlog);
                    });
                });
                // Track geometry in-memory if it changed (no file persistence)
                if let Some(cur) = self.current_geom_top_left()
                    && self.last_saved.map(|g| g != cur).unwrap_or(true)
                {
                    self.last_saved = Some(cur);
                    self.viewport.record_geometry(cur.pos, cur.size);
                }
            });
        });
    }

    /// Render the Details window contents inside the already-created viewport.
    fn render_contents(&mut self, ui: &mut egui::Ui, backlog: &[BacklogEntry]) {
        ui.with_layout(Layout::top_down(egui::Align::Min), |ui| {
            ui.add_space(DETAILS_PAD);
            self.render_tab_bar(ui);
            devtools::value_anchor(
                ui,
                "details.active_tab",
                WidgetValue::Text(self.active_tab.label().to_string()),
            );
            ui.dev_separator("details.separator.tabs");
            ui.add_space(DETAILS_PAD);
            self.render_active_tab(ui, backlog);
            ui.add_space(DETAILS_PAD);
        });
    }

    /// Render the Details tab selector row.
    fn render_tab_bar(&mut self, ui: &mut egui::Ui) {
        container(ui, "details.tabs", |ui| {
            ui.horizontal(|ui| {
                ui.dev_selectable_value(
                    "details.tab.notifications",
                    &mut self.active_tab,
                    Tab::Notifications,
                    "Notifications",
                );
                ui.dev_selectable_value(
                    "details.tab.config",
                    &mut self.active_tab,
                    Tab::Config,
                    "Config",
                );
                ui.dev_selectable_value(
                    "details.tab.logs",
                    &mut self.active_tab,
                    Tab::Logs,
                    "Logs",
                );
                ui.dev_selectable_value(
                    "details.tab.about",
                    &mut self.active_tab,
                    Tab::About,
                    "About",
                );
            });
        });
    }

    /// Render the currently selected Details tab.
    fn render_active_tab(&mut self, ui: &mut egui::Ui, backlog: &[BacklogEntry]) {
        match self.active_tab {
            Tab::Notifications => self.render_notifications(ui, backlog),
            Tab::Config => self.render_config_tab_with_reload(ui),
            Tab::About => Self::render_about_tab(ui),
            Tab::Logs => self.render_logs(ui),
        }
    }

    /// Render the Config tab and apply reload side effects.
    fn render_config_tab_with_reload(&mut self, ui: &mut egui::Ui) {
        let reload_clicked = Self::render_config_tab(ui, &self.config_tab_model());
        if reload_clicked {
            self.send_reload();
            self.reload_config_contents();
        }
    }

    /// Render the About tab.
    fn render_about_tab(ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.dev_label(
                "details.about.name",
                RichText::new("Hotki")
                    .size(32.0)
                    .color(Color32::from_rgb(255, 255, 255))
                    .strong(),
            );
            ui.add_space(15.0);
            ui.dev_label(
                "details.about.description",
                "A powerful hotkey management system for macOS",
            );
            ui.add_space(30.0);
            ui.dev_separator("details.about.separator");
            ui.add_space(30.0);
            ui.dev_label(
                "details.about.version",
                RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                    .color(Color32::from_rgb(200, 200, 200)),
            );
            ui.add_space(30.0);
            ui.dev_hyperlink_to(
                "details.about.repository",
                RichText::new("github.com/cortesi/hotki").color(Color32::from_rgb(100, 149, 237)),
                "https://github.com/cortesi/hotki",
            );
            ui.add_space(40.0);
        });
    }

    /// Disable AppKit cursor rects once the Details window exists.
    ///
    /// This is intentionally tied to window visibility rather than per-hover
    /// cursor state. The flicker was caused by AppKit's cursor-rect updates
    /// racing egui's cursor assignments in interactive regions. By disabling
    /// cursor rects for the entire window while it is visible, we prevent
    /// AppKit from reasserting the default arrow during display-cycle updates.
    fn ensure_cursor_rects_disabled(&mut self) {
        if !self.cursor_rects_disabled
            && let Ok(true) = nswindow::disable_cursor_rects("Details")
        {
            self.cursor_rects_disabled = true;
        }
    }

    /// Re-enable AppKit cursor rects when the Details window is hidden.
    ///
    /// This restores normal AppKit behavior outside the Details window's
    /// lifecycle to avoid surprising cursor behavior elsewhere.
    fn ensure_cursor_rects_enabled(&mut self) {
        if self.cursor_rects_disabled && nswindow::enable_cursor_rects("Details").is_ok() {
            self.cursor_rects_disabled = false;
        }
    }
    /// Send a reload signal to the runtime, if connected.
    fn send_reload(&self) {
        if let Some(ref tx) = self.tx_ctrl
            && let Err(e) = tx.send(ControlMsg::Reload)
        {
            tracing::warn!("failed to send reload: {}", e);
        }
    }

    /// Render the notifications table.
    fn render_notifications(&self, ui: &mut egui::Ui, backlog: &[BacklogEntry]) {
        if backlog.is_empty() {
            ui.dev_label(
                "details.notifications.empty",
                RichText::new("No notifications yet").weak(),
            );
            return;
        }

        ui.dev_label(
            "details.notification.count",
            format!("{} notifications", backlog.len()),
        );
        TableBuilder::new(ui)
            .column(
                Column::initial(COL_KIND_INIT)
                    .at_least(COL_KIND_MIN)
                    .resizable(true),
            )
            .column(
                Column::initial(COL_TITLE_INIT)
                    .at_least(COL_TITLE_MIN)
                    .resizable(true),
            )
            .column(Column::remainder())
            .header(HEADER_H, |mut header| {
                header.col(|ui| {
                    ui.label(RichText::new("Kind").strong());
                });
                header.col(|ui| {
                    ui.label(RichText::new("Title").strong());
                });
                header.col(|ui| {
                    ui.label(RichText::new("Text").strong());
                });
            })
            .body(|mut body| {
                for (index, ent) in backlog.iter().enumerate() {
                    let fg = kind_color(&self.theme, ent.kind);
                    body.row(ROW_HEIGHT_MIN, |mut row| {
                        row.col(|ui| {
                            ui.dev_label(
                                format!("details.notification.{index}.kind"),
                                RichText::new(kind_label(ent.kind)).color(fg),
                            );
                        });
                        row.col(|ui| {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                            ui.dev_label(
                                format!("details.notification.{index}.title"),
                                RichText::new(&ent.title).color(fg),
                            );
                        });
                        row.col(|ui| {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                            ui.dev_label(
                                format!("details.notification.{index}.text"),
                                RichText::new(&ent.text).color(fg),
                            );
                        });
                        let response = row.response();
                        track_response_full(
                            format!("details.notification.{index}.row"),
                            &response,
                            WidgetMeta {
                                role: WidgetRole::Unknown,
                                label: Some(format!(
                                    "{} notification: {}",
                                    kind_label(ent.kind),
                                    ent.title
                                )),
                                visible: true,
                                ..Default::default()
                            },
                        );
                    });
                }
            });
    }

    /// Render the Config tab and return whether reload was requested.
    fn render_config_tab(ui: &mut egui::Ui, model: &ConfigTabModel<'_>) -> bool {
        let mut reload_clicked = false;
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.dev_label(
                    "details.config.path.label",
                    RichText::new("Config File:").strong(),
                );
                ui.dev_label("details.config.path", &model.path_text);
            });

            ui.add_space(10.0);

            if let Some(err) = model.error {
                ui.dev_label(
                    "details.config.error",
                    RichText::new(err).color(Color32::from_rgb(220, 50, 47)),
                );
                ui.add_space(10.0);
            }

            reload_clicked = ui
                .dev_button("details.config.reload", "Reload Config")
                .clicked();

            ui.add_space(10.0);
            ui.dev_separator("details.config.separator");
            ui.add_space(10.0);

            ui.dev_label(
                "details.config.contents.label",
                RichText::new("Contents:").strong(),
            );
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                ui.dev_label("details.config.contents", model.contents);
            });
        });
        reload_clicked
    }

    /// Render the in-app logs viewer.
    fn render_logs(&mut self, ui: &mut egui::Ui) {
        if let Some(snapshot) = logs::snapshot_after(self.log_generation) {
            self.log_generation = snapshot.generation;
            self.log_rows = snapshot.entries;
        }

        ui.vertical(|ui| {
            if ui.dev_button("details.logs.clear", "Clear").clicked() {
                logs::clear();
                self.log_generation = u64::MAX;
                self.log_rows.clear();
            }
            ui.dev_label(
                "details.logs.count",
                format!("{} log rows", self.log_rows.len()),
            );
            ui.add_space(8.0);
            ui.dev_separator("details.logs.separator");
            ui.add_space(8.0);
            ScrollArea::vertical()
                .auto_shrink(false)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                    for (index, ent) in self.log_rows.iter().enumerate() {
                        let prefix = if matches!(ent.side, Side::Client) {
                            "client"
                        } else {
                            "server"
                        };
                        let text = format!(
                            "[{:<6}] {:<5} {:<} - {}",
                            prefix, ent.level, ent.target, ent.message
                        );
                        container(ui, format!("details.log.{index}.row"), |ui| {
                            ui.dev_label(
                                format!("details.log.{index}.message"),
                                RichText::new(text).color(ent.color()),
                            );
                        });
                    }
                });
        });
    }
}

impl Tab {
    /// Stable script-visible label for this tab.
    fn label(self) -> &'static str {
        match self {
            Self::Notifications => "notifications",
            Self::Config => "config",
            Self::About => "about",
            Self::Logs => "logs",
        }
    }
}

/// Foreground color for a notification kind using the active theme.
fn kind_color(theme: &NotifyTheme, kind: NotifyKind) -> Color32 {
    let (r, g, b) = theme.style_for(kind).title_fg;
    Color32::from_rgb(r, g, b)
}

/// Human-readable label for a notification kind.
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

    use super::Details;
    use crate::display::{DisplayMetrics, WindowGeometry};

    #[test]
    fn clamp_to_active_frame_uses_shared_top_left_geometry() {
        let mut details = Details::new(NotifyTheme::default());
        details.set_display_metrics(DisplayMetrics::from_snapshot(&DisplaysSnapshot {
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

        let clamped =
            details.clamp_to_active_frame(WindowGeometry::new(pos2(0.0, 0.0), vec2(900.0, 20.0)));

        assert_eq!(clamped.pos, pos2(50.0, 500.0));
        assert_eq!(clamped.size, vec2(500.0, 80.0));
    }

    #[test]
    fn config_tab_model_names_missing_path_without_copying_contents() {
        let mut details = Details::new(NotifyTheme::default());
        details.config_contents = "hotki.root(function() end)".to_string();

        let model = details.config_tab_model();

        assert_eq!(model.path_text, "No config file loaded");
        assert_eq!(model.contents, "hotki.root(function() end)");
        assert!(model.error.is_none());
    }
}
