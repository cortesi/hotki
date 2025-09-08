use egui::{
    CentralPanel, Color32, Context, RichText, ViewportBuilder, ViewportCommand, ViewportId, vec2,
};

use crate::runtime::ControlMsg;

pub struct PermissionsHelp {
    visible: bool,
    id: ViewportId,
    tx_ctrl: Option<tokio::sync::mpsc::UnboundedSender<ControlMsg>>,
}

impl PermissionsHelp {
    pub fn new() -> Self {
        Self {
            visible: false,
            id: ViewportId::from_hash_of("hotki_permissions"),
            tx_ctrl: None,
        }
    }

    pub fn show(&mut self) {
        self.visible = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn set_control_sender(&mut self, tx: tokio::sync::mpsc::UnboundedSender<ControlMsg>) {
        self.tx_ctrl = Some(tx);
    }

    pub fn render(&mut self, ctx: &Context) {
        if !self.visible {
            ctx.send_viewport_cmd_to(self.id, ViewportCommand::Visible(false));
            return;
        }

        let builder = ViewportBuilder::default()
            .with_title("Permissions Required")
            .with_visible(true)
            .with_decorations(true)
            .with_resizable(true)
            .with_transparent(false)
            .with_has_shadow(true)
            .with_inner_size(vec2(700.0, 520.0));

        ctx.show_viewport_immediate(self.id, builder, |wctx, _| {
            if wctx.input(|i| i.viewport().close_requested()) {
                self.visible = false;
                wctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                return;
            }

            let access_ok = ::permissions::accessibility_ok();
            let input_ok = ::permissions::input_monitoring_ok();

            let green = Color32::from_rgb(64, 201, 99);
            let red = Color32::from_rgb(220, 50, 47);
            // Nerd Font icons (PUA)
            let icon_ok = '\u{f05d}'; // circle-check
            let icon_bad = '\u{f52f}'; // not-okay indicator

            CentralPanel::default().show(wctx, |ui| {
                ui.heading(RichText::new("Hotki Needs Permissions").strong());
                ui.add_space(8.0);
                ui.label("Hotki requires Accessibility and Input Monitoring permissions to register hotkeys and synthesize keystrokes.");

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                // Accessibility section
                ui.horizontal(|ui| {
                    if access_ok {
                        ui.label(
                            RichText::new(icon_ok.to_string())
                                .size(26.0)
                                .color(green)
                                .strong(),
                        );
                        ui.add_space(6.0);
                        ui.label(RichText::new("Accessibility").color(green).strong());
                        ui.add_space(4.0);
                        ui.label(RichText::new("Enabled").color(green));
                    } else {
                        ui.label(
                            RichText::new(icon_bad.to_string())
                                .size(26.0)
                                .color(red)
                                .strong(),
                        );
                        ui.add_space(6.0);
                        ui.label(RichText::new("Accessibility").color(red).strong());
                        ui.add_space(4.0);
                        ui.label(RichText::new("Not enabled yet").color(red));
                    }
                });
                ui.add_space(4.0);
                ui.label("Grant permission in System Settings → Privacy & Security → Accessibility. If Hotki was updated or re-installed, remove the existing Hotki entry first, then add it again.");
                ui.add_space(6.0);
                if ui.button("Open Accessibility Settings").clicked()
                    && let Some(ref tx) = self.tx_ctrl
                {
                    let _ = tx.send(ControlMsg::OpenAccessibility);
                }
                ui.add_space(10.0);

                ui.separator();
                ui.add_space(8.0);
                // Input Monitoring section
                ui.horizontal(|ui| {
                    if input_ok {
                        ui.label(
                            RichText::new(icon_ok.to_string())
                                .size(26.0)
                                .color(green)
                                .strong(),
                        );
                        ui.add_space(6.0);
                        ui.label(RichText::new("Input Monitoring").color(green).strong());
                        ui.add_space(4.0);
                        ui.label(RichText::new("Enabled").color(green));
                    } else {
                        ui.label(
                            RichText::new(icon_bad.to_string())
                                .size(26.0)
                                .color(red)
                                .strong(),
                        );
                        ui.add_space(6.0);
                        ui.label(RichText::new("Input Monitoring").color(red).strong());
                        ui.add_space(4.0);
                        ui.label(RichText::new("Not enabled yet").color(red));
                    }
                });
                ui.add_space(4.0);

                ui.label("Grant permission in System Settings → Privacy & Security → Input Monitoring. If Hotki was updated or re-installed, remove the existing Hotki entry first, then add it again.");

                ui.add_space(6.0);
                if ui.button("Open Input Monitoring Settings").clicked()
                    && let Some(ref tx) = self.tx_ctrl
                {
                    let _ = tx.send(ControlMsg::OpenInputMonitoring);
                }

                ui.add_space(14.0);
                ui.separator();
                ui.add_space(8.0);
                ui.label(RichText::new("Tip").strong());
                ui.label("After changing permissions, restart Hotki if keys still don’t respond.");
            });
        });
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PermissionsStatus {
    pub accessibility_ok: bool,
    pub input_ok: bool,
}

pub(crate) fn check_permissions() -> PermissionsStatus {
    let st = ::permissions::check_permissions();
    PermissionsStatus {
        accessibility_ok: st.accessibility_ok,
        input_ok: st.input_ok,
    }
}
