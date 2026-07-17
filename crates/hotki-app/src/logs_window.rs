//! Dedicated diagnostic log window.

use egui::{CentralPanel, Context, Layout, RichText, ScrollArea, ViewportBuilder, ViewportCommand};
use eguidev::{DevMcp, DevUiExt, container};

use crate::{
    devtools,
    logs::{self, LogEntry, Side},
};

/// Stable viewport identity reused for every logs-window open.
fn logs_viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("hotki_logs")
}

/// Cached state and rendering for the diagnostic logs window.
pub struct LogsWindow {
    /// Whether the viewport should be visible.
    visible: bool,
    /// Whether the next frame should focus the viewport.
    want_focus: bool,
    /// Generation corresponding to `rows`.
    generation: u64,
    /// Cached log rows cloned only when the global buffer changes.
    rows: Vec<LogEntry>,
}

impl LogsWindow {
    /// Construct a hidden logs window with an invalid cache generation.
    pub fn new() -> Self {
        Self {
            visible: false,
            want_focus: false,
            generation: u64::MAX,
            rows: Vec::new(),
        }
    }

    /// Show and focus the stable logs viewport.
    pub fn show(&mut self) {
        self.visible = true;
        self.want_focus = true;
    }

    /// Hide the logs viewport.
    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Render the logs viewport when visible.
    pub fn render(&mut self, ctx: &Context, devmcp: &DevMcp) {
        if !self.visible {
            ctx.send_viewport_cmd_to(logs_viewport_id(), ViewportCommand::Visible(false));
            return;
        }
        if let Some(snapshot) = logs::snapshot_after(self.generation) {
            self.generation = snapshot.generation;
            self.rows = snapshot.entries;
        }

        let builder = ViewportBuilder::default()
            .with_title("Hotki Logs")
            .with_visible(true)
            .with_decorations(true)
            .with_resizable(true)
            .with_transparent(false)
            .with_has_shadow(true);
        ctx.show_viewport_immediate(logs_viewport_id(), builder, |vp_ui, _| {
            devtools::viewport_frame(devmcp, vp_ui, "logs", "logs.root", |vp_ui| {
                let window_ctx = vp_ui.ctx().clone();
                if window_ctx.input(|input| input.viewport().close_requested()) {
                    self.visible = false;
                    window_ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                    return;
                }
                if self.want_focus {
                    window_ctx.send_viewport_cmd_to(logs_viewport_id(), ViewportCommand::Focus);
                    self.want_focus = false;
                }
                CentralPanel::default().show(vp_ui, |ui| self.render_contents(ui));
            });
        });
    }

    /// Render the clear toolbar and monospaced log stream.
    fn render_contents(&mut self, ui: &mut egui::Ui) {
        container(ui, "logs.toolbar", |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.dev_button("logs.clear", "Clear").clicked() {
                        logs::clear();
                        self.generation = u64::MAX;
                        self.rows.clear();
                    }
                });
            });
        });
        ui.dev_separator("logs.separator");
        if self.rows.is_empty() {
            ui.dev_label("logs.empty", RichText::new("No logs yet.").weak());
            return;
        }
        ScrollArea::both()
            .id_salt("logs.scroll")
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                for (index, entry) in self.rows.iter().enumerate() {
                    let side = if matches!(entry.side, Side::Client) {
                        "client"
                    } else {
                        "server"
                    };
                    let text = format!(
                        "[{side:<6}] {:<5} {} - {}",
                        entry.level, entry.target, entry.message
                    );
                    container(ui, format!("logs.{index}.row"), |ui| {
                        ui.dev_label(
                            format!("logs.{index}.message"),
                            RichText::new(text).color(entry.color()),
                        );
                    });
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::{LogsWindow, logs_viewport_id};

    #[test]
    fn repeated_show_reuses_one_viewport() {
        let mut logs = LogsWindow::new();
        logs.show();
        logs.show();

        assert!(logs.visible);
        assert!(logs.want_focus);
        assert_eq!(
            logs_viewport_id(),
            egui::ViewportId::from_hash_of("hotki_logs")
        );
    }

    #[test]
    fn hide_keeps_cache_for_the_next_open() {
        let mut logs = LogsWindow::new();
        logs.generation = 42;
        logs.show();
        logs.hide();

        assert!(!logs.visible);
        assert_eq!(logs.generation, 42);
    }
}
