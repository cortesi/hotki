use std::{fs, path::PathBuf};

use egui::{
    CentralPanel, Color32, Context, Layout, RichText, ScrollArea, ViewportBuilder, ViewportCommand,
    ViewportId, vec2,
};
use egui_extras::{Column, TableBuilder};

use config::NotifyTheme;
use hotki_protocol::NotifyKind;

use crate::{macos_window, notification::BacklogEntry, runtime::ControlMsg};

const DETAILS_PAD: f32 = 12.0;
const HEADER_H: f32 = 22.0;
const ROW_HEIGHT_MIN: f32 = 20.0;
const COL_KIND_INIT: f32 = 90.0;
const COL_TITLE_INIT: f32 = 180.0;
const COL_KIND_MIN: f32 = 70.0;
const COL_TITLE_MIN: f32 = 120.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Notifications,
    Config,
    About,
    Logs,
}

pub struct Details {
    visible: bool,
    id: ViewportId,
    want_initial_size: bool,
    restore_pending: bool,
    want_focus: bool,
    last_saved: Option<WindowGeom>,
    active_tab: Tab,
    theme: NotifyTheme,
    config_path: Option<PathBuf>,
    config_contents: String,
    config_error: Option<String>,
    tx_ctrl: Option<tokio::sync::mpsc::UnboundedSender<ControlMsg>>,
}

impl Details {
    pub fn new(theme: NotifyTheme) -> Self {
        Self {
            visible: false,
            id: ViewportId::from_hash_of("hotki_details"),
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
        }
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.want_initial_size = true;
        self.restore_pending = true;
        self.want_focus = true;
    }

    pub fn toggle(&mut self) {
        if self.visible {
            self.visible = false;
        } else {
            self.show();
        }
    }

    pub fn update_theme(&mut self, theme: NotifyTheme) {
        self.theme = theme;
    }

    pub fn set_config_path(&mut self, path: Option<PathBuf>) {
        self.config_path = path;
        self.reload_config_contents();
    }

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
                    if let Some(ref tx) = self.tx_ctrl {
                        let _ = tx.send(ControlMsg::Notice {
                            kind: NotifyKind::Error,
                            title: "Config".to_string(),
                            text: msg.clone(),
                        });
                    }
                    self.config_error = Some(msg);
                    // Also log for diagnostics
                    tracing::warn!("{}", e);
                }
            }
        }
    }

    pub fn set_control_sender(&mut self, tx: tokio::sync::mpsc::UnboundedSender<ControlMsg>) {
        self.tx_ctrl = Some(tx);
    }

    pub fn render(&mut self, ctx: &Context, backlog: &[BacklogEntry]) {
        if !self.visible {
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
                if let Some(cur) = current_geom_top_left() {
                    self.last_saved = Some(cur);
                }
                return;
            }
            // Apply saved geometry once after opening
            if self.restore_pending {
                if let Some(stored) = self.last_saved {
                    wctx.send_viewport_cmd_to(
                        self.id,
                        ViewportCommand::InnerSize(vec2(stored.size.0, stored.size.1)),
                    );
                    wctx.send_viewport_cmd_to(
                        self.id,
                        ViewportCommand::OuterPosition(egui::pos2(stored.pos.0, stored.pos.1)),
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
                            // Unified table with resizable columns
                            if backlog.is_empty() {
                                ui.weak("No notifications yet");
                            } else {
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
                                            let fg = match ent.kind {
                                                NotifyKind::Info => {
                                                    let s = &self.theme.info;
                                                    Color32::from_rgb(
                                                        s.title_fg.0,
                                                        s.title_fg.1,
                                                        s.title_fg.2,
                                                    )
                                                }
                                                NotifyKind::Warn => {
                                                    let s = &self.theme.warn;
                                                    Color32::from_rgb(
                                                        s.title_fg.0,
                                                        s.title_fg.1,
                                                        s.title_fg.2,
                                                    )
                                                }
                                                NotifyKind::Error => {
                                                    let s = &self.theme.error;
                                                    Color32::from_rgb(
                                                        s.title_fg.0,
                                                        s.title_fg.1,
                                                        s.title_fg.2,
                                                    )
                                                }
                                                NotifyKind::Success => {
                                                    let s = &self.theme.success;
                                                    Color32::from_rgb(
                                                        s.title_fg.0,
                                                        s.title_fg.1,
                                                        s.title_fg.2,
                                                    )
                                                }
                                            };

                                            body.row(ROW_HEIGHT_MIN, |mut row| {
                                                row.col(|ui| {
                                                    let label = match ent.kind {
                                                        NotifyKind::Info => "info",
                                                        NotifyKind::Warn => "warn",
                                                        NotifyKind::Error => "error",
                                                        NotifyKind::Success => "success",
                                                    };
                                                    ui.colored_label(fg, label);
                                                });
                                                row.col(|ui| {
                                                    ui.style_mut().wrap_mode =
                                                        Some(egui::TextWrapMode::Wrap);
                                                    ui.colored_label(fg, &ent.title);
                                                });
                                                row.col(|ui| {
                                                    ui.style_mut().wrap_mode =
                                                        Some(egui::TextWrapMode::Wrap);
                                                    ui.colored_label(fg, &ent.text);
                                                });
                                            });
                                        }
                                    });
                            }
                        }
                        Tab::Config => {
                            ui.vertical(|ui| {
                                // Show config file path
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("Config File:").strong());
                                    if let Some(ref path) = self.config_path {
                                        ui.label(path.display().to_string());
                                    } else {
                                        ui.label("No config file loaded");
                                    }
                                });

                                ui.add_space(10.0);

                                if let Some(ref err) = self.config_error {
                                    ui.colored_label(Color32::from_rgb(220, 50, 47), err);
                                    ui.add_space(10.0);
                                }

                                // Reload button
                                if ui.button("Reload Config").clicked() {
                                    if let Some(ref tx) = self.tx_ctrl {
                                        let _ = tx.send(ControlMsg::Reload);
                                    }
                                    // Reload the file contents too
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
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    if ui.button("Clear").clicked() {
                                        crate::logs::clear();
                                    }
                                });
                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(8.0);
                                ScrollArea::vertical()
                                    .auto_shrink([false; 2])
                                    .stick_to_bottom(true)
                                    .show(ui, |ui| {
                                        ui.style_mut().override_font_id =
                                            Some(egui::FontId::monospace(12.0));
                                        for ent in crate::logs::snapshot() {
                                            let prefix = match ent.side {
                                                crate::logs::Side::Client => "client",
                                                crate::logs::Side::Server => "server",
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

                    ui.add_space(DETAILS_PAD);
                });
            });
            // Track geometry in-memory if it changed (no file persistence)
            if let Some(cur) = current_geom_top_left()
                && self
                    .last_saved
                    .map(|g| g.pos != cur.pos || g.size != cur.size)
                    .unwrap_or(true)
            {
                self.last_saved = Some(cur);
            }
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WindowGeom {
    pub pos: (f32, f32),
    pub size: (f32, f32),
}

fn current_geom_top_left() -> Option<WindowGeom> {
    // NSWindow uses bottom-left origin; convert to global top-left expected by winit.
    let (_sx, _sy, _sw, _sh, global_top) = macos_window::active_screen_frame();
    let (x_b, y_b, w, h) = match macos_window::get_window_frame("Details") {
        Ok(Some(frame)) => frame,
        Ok(None) => return None,
        Err(e) => {
            tracing::error!("{}", e);
            return None;
        }
    };
    let x_t = x_b; // x is the same
    let y_t = global_top - (y_b + h);
    Some(WindowGeom {
        pos: (x_t, y_t),
        size: (w, h),
    })
}
