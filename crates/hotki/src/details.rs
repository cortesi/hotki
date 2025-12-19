use std::{fs, path::PathBuf};

use egui::{
    CentralPanel, Color32, Context, Layout, RichText, ScrollArea, ViewportBuilder, ViewportCommand,
    ViewportId, vec2,
};
use egui_extras::{Column, TableBuilder};
use hotki_protocol::{NotifyKind, NotifyTheme};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    display::DisplayMetrics,
    logs::{self, Side},
    notification::BacklogEntry,
    nswindow,
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

/// State and rendering for the Details window.
pub struct Details {
    /// Whether the window is currently visible.
    visible: bool,
    /// Whether AppKit cursor rects are disabled for the window.
    cursor_rects_disabled: bool,
    /// Stable viewport identifier for the window.
    id: ViewportId,
    /// Request an initial default size on next show.
    want_initial_size: bool,
    /// Apply previously saved geometry once after opening.
    restore_pending: bool,
    /// Request focus on next frame.
    want_focus: bool,
    /// Last known geometry for this session.
    last_saved: Option<WindowGeom>,
    /// Display geometry used for placement/clamping.
    display: DisplayMetrics,
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
}

impl Details {
    /// Construct a new Details window with the given theme.
    pub fn new(theme: NotifyTheme) -> Self {
        Self {
            visible: false,
            cursor_rects_disabled: false,
            id: ViewportId::from_hash_of("hotki_details"),
            want_initial_size: false,
            restore_pending: false,
            want_focus: false,
            last_saved: None,
            display: DisplayMetrics::default(),
            active_tab: Tab::Notifications,
            theme,
            config_path: None,
            config_contents: String::new(),
            config_error: None,
            tx_ctrl: None,
        }
    }

    /// Make the window visible and request initial layout and focus.
    pub fn show(&mut self) {
        self.visible = true;
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
    }

    /// Update the active theme used for colors.
    pub fn update_theme(&mut self, theme: NotifyTheme) {
        self.theme = theme;
    }

    /// Update display metrics used to clamp and restore window geometry.
    pub fn set_display_metrics(&mut self, metrics: DisplayMetrics) {
        self.display = metrics;
    }

    /// Query the current Details window geometry converted to a top-left origin.
    fn current_geom_top_left(&self) -> Option<WindowGeom> {
        // NSWindow uses bottom-left origin; convert to global top-left expected by winit.
        let (x_b, y_b, w, h) = nswindow::frame_by_title("Details")?;
        let x_t = x_b;
        let y_t = self.display.to_top_left_y(y_b, h);
        Some(WindowGeom {
            pos: (x_t, y_t),
            size: (w, h),
        })
    }

    /// Clamp a window geometry to the current active screen's visible frame.
    fn clamp_to_active_frame(&self, g: WindowGeom) -> WindowGeom {
        let frame = self.display.active_frame();
        let sx_b = frame.x;
        let _sy_b = frame.y;
        let sw = frame.width;
        let sh = frame.height;

        // Convert active screen rect to top-left origin
        let screen_left = sx_b;
        let screen_right = sx_b + sw;
        let screen_top_tl = self.display.active_frame_top_left_y();
        let screen_bottom_tl = screen_top_tl + sh;

        // Ensure minimally positive size and at most screen size
        let min_w = 100.0_f32;
        let min_h = 80.0_f32;
        let clamped_w = g.size.0.max(min_w).min(sw);
        let clamped_h = g.size.1.max(min_h).min(sh);

        // Compute clamped position ranges; collapse if window is larger than screen
        let x_min = screen_left;
        let x_max = (screen_right - clamped_w).max(x_min);
        let y_min = screen_top_tl;
        let y_max = (screen_bottom_tl - clamped_h).max(y_min);

        let x = g.pos.0.clamp(x_min, x_max);
        let y = g.pos.1.clamp(y_min, y_max);

        WindowGeom {
            pos: (x, y),
            size: (clamped_w, clamped_h),
        }
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

    /// Render the window and its active tab.
    pub fn render(&mut self, ctx: &Context, backlog: &[BacklogEntry]) {
        if !self.visible {
            self.ensure_cursor_rects_enabled();
            // Ensure window is hidden if not visible
            ctx.send_viewport_cmd_to(self.id, ViewportCommand::Visible(false));
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

        ctx.show_viewport_immediate(self.id, builder, |wctx, _| {
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
                        self.id,
                        ViewportCommand::InnerSize(vec2(clamped.size.0, clamped.size.1)),
                    );
                    wctx.send_viewport_cmd_to(
                        self.id,
                        ViewportCommand::OuterPosition(egui::pos2(clamped.pos.0, clamped.pos.1)),
                    );
                }
                self.restore_pending = false;
            }
            // Focus the window when toggled on
            if self.want_focus {
                wctx.send_viewport_cmd_to(self.id, ViewportCommand::Focus);
                self.want_focus = false;
            }
            CentralPanel::default().show(wctx, |ui| {
                ui.with_layout(Layout::top_down(egui::Align::Min), |ui| {
                    ui.add_space(DETAILS_PAD);

                    // Tab bar
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut self.active_tab,
                            Tab::Notifications,
                            "Notifications",
                        );
                        ui.selectable_value(&mut self.active_tab, Tab::Config, "Config");
                        ui.selectable_value(&mut self.active_tab, Tab::Logs, "Logs");
                        ui.selectable_value(&mut self.active_tab, Tab::About, "About");
                    });

                    ui.separator();
                    ui.add_space(DETAILS_PAD);

                    // Tab content
                    match self.active_tab {
                        Tab::Notifications => {
                            self.render_notifications(ui, backlog);
                        }
                        Tab::Config => {
                            ui.vertical(|ui| {
                                // Show config file path
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("Config File:").strong());
                                    let path_text = self
                                        .config_path
                                        .as_ref()
                                        .map(|p| p.display().to_string())
                                        .unwrap_or_else(|| "No config file loaded".to_string());
                                    ui.label(path_text);
                                });

                                ui.add_space(10.0);

                                if let Some(ref err) = self.config_error {
                                    ui.colored_label(Color32::from_rgb(220, 50, 47), err);
                                    ui.add_space(10.0);
                                }

                                // Reload button
                                if ui.button("Reload Config").clicked() {
                                    self.send_reload();
                                    self.reload_config_contents();
                                }

                                ui.add_space(10.0);
                                ui.separator();
                                ui.add_space(10.0);

                                // Show config contents in a scrollable area
                                ui.label(RichText::new("Contents:").strong());
                                ScrollArea::vertical()
                                    .auto_shrink([false; 2])
                                    .show(ui, |ui| {
                                        // Use monospace font for config display
                                        ui.style_mut().override_font_id =
                                            Some(egui::FontId::monospace(12.0));
                                        ui.label(&self.config_contents);
                                    });
                            });
                        }
                        Tab::About => {
                            ui.vertical_centered(|ui| {
                                ui.add_space(40.0);

                                // Large, bright title
                                ui.label(
                                    RichText::new("Hotki")
                                        .size(32.0)
                                        .color(Color32::from_rgb(255, 255, 255))
                                        .strong(),
                                );

                                ui.add_space(15.0);
                                ui.label("A powerful hotkey management system for macOS");

                                ui.add_space(30.0);
                                ui.separator();
                                ui.add_space(30.0);

                                ui.label(
                                    RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                                        .color(Color32::from_rgb(200, 200, 200)),
                                );

                                ui.add_space(30.0);

                                ui.hyperlink_to(
                                    RichText::new("github.com/cortesi/hotki")
                                        .color(Color32::from_rgb(100, 149, 237)),
                                    "https://github.com/cortesi/hotki",
                                );

                                ui.add_space(40.0);
                            });
                        }
                        Tab::Logs => {
                            self.render_logs(ui);
                        }
                    }

                    ui.add_space(DETAILS_PAD);
                });
            });
            // Track geometry in-memory if it changed (no file persistence)
            if let Some(cur) = self.current_geom_top_left()
                && self
                    .last_saved
                    .map(|g| g.pos != cur.pos || g.size != cur.size)
                    .unwrap_or(true)
            {
                self.last_saved = Some(cur);
            }
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
            ui.weak("No notifications yet");
            return;
        }

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
                for ent in backlog {
                    let fg = kind_color(&self.theme, ent.kind);
                    body.row(ROW_HEIGHT_MIN, |mut row| {
                        row.col(|ui| {
                            ui.colored_label(fg, kind_label(ent.kind));
                        });
                        row.col(|ui| {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                            ui.colored_label(fg, &ent.title);
                        });
                        row.col(|ui| {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                            ui.colored_label(fg, &ent.text);
                        });
                    });
                }
            });
    }

    /// Render the in-app logs viewer.
    fn render_logs(&self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            if ui.button("Clear").clicked() {
                logs::clear();
            }
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);
            ScrollArea::vertical()
                .auto_shrink([false; 2])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                    for ent in logs::snapshot() {
                        let prefix = if matches!(ent.side, Side::Client) {
                            "client"
                        } else {
                            "server"
                        };
                        let text = format!(
                            "[{:<6}] {:<5} {:<} - {}",
                            prefix, ent.level, ent.target, ent.message
                        );
                        ui.colored_label(ent.color(), text);
                    }
                });
        });
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

#[derive(Debug, Clone, Copy, PartialEq, Default)]
/// Simple window geometry snapshot (top-left origin).
pub struct WindowGeom {
    /// Top-left window position `(x, y)` in screen coordinates.
    pub pos: (f32, f32),
    /// Inner size `(w, h)` in logical pixels.
    pub size: (f32, f32),
}
