use std::process::Command;

use egui::{
    CentralPanel, Color32, Context, RichText, ViewportBuilder, ViewportCommand, ViewportId, vec2,
};
use eguidev::{DevMcp, DevUiExt, WidgetValue, container};
use hotki_protocol::NotifyKind;
pub use permissions::PermissionsStatus;
use tokio::sync::mpsc::UnboundedSender;

use crate::{devtools, runtime::ControlMsg};

#[derive(Clone, Copy)]
/// Static content and action for one permission section.
struct PermissionSection<'a> {
    /// Stable eguidev id suffix.
    id: &'a str,
    /// Whether this permission is currently granted.
    enabled: bool,
    /// User-facing permission name.
    name: &'a str,
    /// Help text shown under the status row.
    help: &'a str,
    /// Function that opens the corresponding System Settings pane.
    open_settings: fn(),
    /// Notice text emitted after the opener is invoked.
    notice_text: &'a str,
    /// Whether the opener intent has been recorded in devtools fixture mode.
    open_intent: bool,
}

#[derive(Debug, Default, Clone, Copy)]
/// Recorded System Settings opener intents for deterministic devtools tests.
struct PermissionOpenIntents {
    /// Accessibility settings opener was invoked.
    accessibility: bool,
    /// Input Monitoring settings opener was invoked.
    input_monitoring: bool,
}

/// UI component that presents instructions to grant required macOS permissions.
/// This opens a separate viewport with current status and quick links.
pub struct PermissionsHelp {
    /// Whether the permissions help viewport is visible.
    visible: bool,
    /// Request focus the next time the viewport is shown.
    want_focus: bool,
    /// Stable viewport id for the help window.
    id: ViewportId,
    /// Control channel to the runtime for opening settings.
    tx_ctrl: Option<UnboundedSender<ControlMsg>>,
    /// Deterministic permission status used by devtools fixtures.
    status_override: Option<PermissionsStatus>,
    /// Recorded opener intents while `status_override` is active.
    open_intents: PermissionOpenIntents,
}

impl PermissionsHelp {
    /// Construct a new hidden permissions help component.
    pub fn new() -> Self {
        Self {
            visible: false,
            want_focus: false,
            id: ViewportId::from_hash_of("hotki_permissions"),
            tx_ctrl: None,
            status_override: None,
            open_intents: PermissionOpenIntents::default(),
        }
    }

    /// Show the permissions help window.
    pub fn show(&mut self) {
        self.visible = true;
        self.want_focus = true;
    }

    /// Hide the permissions help window.
    pub fn hide(&mut self) {
        self.visible = false;
        self.want_focus = false;
    }

    /// Set the runtime control sender used to trigger actions.
    pub fn set_control_sender(&mut self, tx: UnboundedSender<ControlMsg>) {
        self.tx_ctrl = Some(tx);
    }

    /// Override real macOS permission status for deterministic devtools fixtures.
    pub fn set_status_override(&mut self, status: Option<PermissionsStatus>) {
        self.status_override = status;
        self.open_intents = PermissionOpenIntents::default();
        self.show();
    }

    /// Render the permissions help viewport; manages visibility and UI content.
    pub fn render(&mut self, ctx: &Context, devmcp: &DevMcp) {
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

        devtools::pump_viewport_input(devmcp, ctx, self.id);
        ctx.show_viewport_immediate(self.id, builder, |vp_ui, _| {
            devtools::viewport_frame(devmcp, vp_ui, |vp_ui| {
                let wctx = vp_ui.ctx().clone();
                if wctx.input(|i| i.viewport().close_requested()) {
                    self.visible = false;
                    self.want_focus = false;
                    wctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    return;
                }
                if self.want_focus {
                    wctx.send_viewport_cmd_to(self.id, ViewportCommand::Focus);
                    self.want_focus = false;
                }

                let status = self.status();
                self.render_body(vp_ui, &status);
            });
        });
    }

    /// Current permission status, using a devtools fixture override when present.
    fn status(&self) -> PermissionsStatus {
        self.status_override.unwrap_or_else(check_permissions)
    }

    /// Render the permissions content inside the active viewport.
    fn render_body(&mut self, ui: &mut egui::Ui, status: &PermissionsStatus) {
        CentralPanel::default().show_inside(ui, |ui| {
            container(ui, "permissions.root", |ui| {
                container(ui, "permissions.panel", |ui| {
                    self.render_intro(ui);
                    self.render_section(
                        ui,
                        PermissionSection {
                            id: "accessibility",
                            enabled: status.accessibility_ok(),
                            name: "Accessibility",
                            help: "Grant permission in System Settings → Privacy & Security → Accessibility. If Hotki was updated or re-installed, remove the existing Hotki entry first, then add it again.",
                            open_settings: open_accessibility_settings,
                            notice_text: "Opening Accessibility settings...",
                            open_intent: self.open_intents.accessibility,
                        },
                    );
                    ui.add_space(10.0);
                    ui.dev_separator("permissions.separator.sections");
                    ui.add_space(8.0);
                    self.render_section(
                        ui,
                        PermissionSection {
                            id: "input_monitoring",
                            enabled: status.input_ok(),
                            name: "Input Monitoring",
                            help: "Grant permission in System Settings → Privacy & Security → Input Monitoring. If Hotki was updated or re-installed, remove the existing Hotki entry first, then add it again.",
                            open_settings: open_input_monitoring_settings,
                            notice_text: "Opening Input Monitoring settings...",
                            open_intent: self.open_intents.input_monitoring,
                        },
                    );
                    self.render_tip(ui);
                });
            });
        });
    }

    /// Render the permissions heading and shared explanation.
    fn render_intro(&self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("Hotki Needs Permissions").strong());
        ui.add_space(8.0);
        ui.dev_label(
            "permissions.description",
            "Hotki requires Accessibility and Input Monitoring permissions to register hotkeys and synthesize keystrokes.",
        );
        ui.add_space(12.0);
        ui.dev_separator("permissions.separator.intro");
        ui.add_space(8.0);
    }

    /// Render one permission status row, help text, and opener button.
    fn render_section(&mut self, ui: &mut egui::Ui, section: PermissionSection<'_>) {
        let (icon, color, status) = permission_status_parts(section.enabled);
        devtools::value_anchor(
            ui,
            format!("permissions.{}.granted", section.id),
            WidgetValue::Bool(section.enabled),
        );
        devtools::value_anchor(
            ui,
            format!("permissions.{}.open_intent", section.id),
            WidgetValue::Bool(section.open_intent),
        );
        ui.horizontal(|ui| {
            ui.dev_label(
                format!("permissions.{}.icon", section.id),
                RichText::new(icon.to_string())
                    .size(26.0)
                    .color(color)
                    .strong(),
            );
            ui.add_space(6.0);
            ui.dev_label(
                format!("permissions.{}.name", section.id),
                RichText::new(section.name).color(color).strong(),
            );
            ui.add_space(4.0);
            ui.dev_label(
                format!("permissions.{}.status", section.id),
                RichText::new(status).color(color),
            );
        });
        ui.add_space(4.0);
        ui.dev_label(format!("permissions.{}.help", section.id), section.help);
        ui.add_space(6.0);
        if ui
            .dev_button(
                format!("permissions.{}.open_settings", section.id),
                format!("Open {} Settings", section.name),
            )
            .clicked()
        {
            self.open_settings(section);
            self.send_settings_notice(section.name, section.notice_text);
        }
    }

    /// Open settings in production or record intent for deterministic fixtures.
    fn open_settings(&mut self, section: PermissionSection<'_>) {
        if self.status_override.is_some() {
            match section.id {
                "accessibility" => self.open_intents.accessibility = true,
                "input_monitoring" => self.open_intents.input_monitoring = true,
                _ => {}
            }
            return;
        }
        (section.open_settings)();
    }

    /// Send a notification after a System Settings opener has been invoked.
    fn send_settings_notice(&self, name: &str, notice_text: &str) {
        let Some(tx) = self.tx_ctrl.as_ref() else {
            return;
        };
        if tx
            .send(ControlMsg::Notice {
                kind: NotifyKind::Info,
                title: name.to_string(),
                text: notice_text.to_string(),
            })
            .is_err()
        {
            tracing::warn!("failed to send open-settings notice for {}", name);
        }
    }

    /// Render the restart tip footer.
    fn render_tip(&self, ui: &mut egui::Ui) {
        ui.add_space(14.0);
        ui.dev_separator("permissions.separator.tip");
        ui.add_space(8.0);
        ui.dev_label("permissions.tip.label", RichText::new("Tip").strong());
        ui.dev_label(
            "permissions.tip.text",
            "After changing permissions, restart Hotki if keys still don't respond.",
        );
    }
}

/// Presentation details for a permission status row.
fn permission_status_parts(enabled: bool) -> (char, Color32, &'static str) {
    if enabled {
        ('\u{f05d}', Color32::from_rgb(64, 201, 99), "Enabled")
    } else {
        (
            '\u{f52f}',
            Color32::from_rgb(220, 50, 47),
            "Not enabled yet",
        )
    }
}

/// Query the current process permissions.
pub fn check_permissions() -> PermissionsStatus {
    ::permissions::check_permissions()
}

/// Open macOS Accessibility settings in System Settings.
fn open_accessibility_settings() {
    if Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn()
        .is_err()
    {
        tracing::warn!("failed to open Accessibility settings");
    }
}

/// Open macOS Input Monitoring settings in System Settings.
fn open_input_monitoring_settings() {
    if Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
        .spawn()
        .is_err()
    {
        tracing::warn!("failed to open Input Monitoring settings");
    }
}
