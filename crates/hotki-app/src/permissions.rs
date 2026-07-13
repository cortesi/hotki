use std::process::Command;

use egui::{
    CentralPanel, Color32, Context, Frame, RichText, ScrollArea, ViewportBuilder, ViewportCommand,
    ViewportId, vec2,
};
use eguidev::{DevMcp, DevUiExt, WidgetValue, container};
use hotki_protocol::NotifyKind;
use objc2_app_kit::NSApplication;
use objc2_foundation::MainThreadMarker;
pub use permissions::PermissionsStatus;
use tokio::sync::mpsc::UnboundedSender;

use crate::{devtools, runtime::ControlMsg};

/// One permission snapshot together with the authority that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionObservation {
    /// Observed permission values.
    status: PermissionsStatus,
    /// Whether deterministic devtools state replaced the macOS observation.
    overridden: bool,
}

impl PermissionObservation {
    /// Build a permission observation read from macOS.
    pub(crate) fn system(status: PermissionsStatus) -> Self {
        Self {
            status,
            overridden: false,
        }
    }

    /// Build a permission observation supplied by deterministic devtools state.
    pub(crate) fn devtools(status: PermissionsStatus) -> Self {
        Self {
            status,
            overridden: true,
        }
    }

    /// Return the observed permission values.
    pub(crate) fn status(self) -> PermissionsStatus {
        self.status
    }

    /// Whether this observation came from a deterministic devtools override.
    pub(crate) fn is_overridden(self) -> bool {
        self.overridden
    }
}

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
    /// Latest permission status observed from macOS or a devtools override.
    current_status: PermissionsStatus,
    /// Whether the observer must refresh on the next app frame.
    observation_pending: bool,
    /// Whether AppKit reported Hotki active on the previous frame.
    app_was_active: bool,
    /// Recorded opener intents while `status_override` is active.
    open_intents: PermissionOpenIntents,
    /// Last permission observation reported to the runtime control loop.
    last_reported_status: Option<PermissionObservation>,
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
            current_status: PermissionsStatus::default(),
            observation_pending: true,
            app_was_active: false,
            open_intents: PermissionOpenIntents::default(),
            last_reported_status: None,
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
        self.last_reported_status = None;
        self.observation_pending = true;
    }

    /// Observe permission changes and render the help viewport when visible.
    pub fn render(&mut self, ctx: &Context, devmcp: &DevMcp) {
        self.observe_status(app_is_active(), self.visible);

        if !self.visible {
            ctx.send_viewport_cmd_to(self.id, ViewportCommand::Visible(false));
            return;
        }

        let builder = ViewportBuilder::default()
            .with_title("Hotki Permissions")
            .with_visible(true)
            .with_decorations(true)
            .with_resizable(true)
            .with_transparent(false)
            .with_has_shadow(true)
            .with_inner_size(vec2(620.0, 360.0));

        ctx.show_viewport_immediate(self.id, builder, |vp_ui, _| {
            devtools::viewport_frame(devmcp, vp_ui, "permissions", "permissions.root", |vp_ui| {
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

                let status = self.current_status;
                self.render_body(vp_ui, &status);
            });
        });
    }

    /// Refresh status initially, after activation/override changes, or while help is visible.
    fn observe_status(&mut self, app_active: bool, help_visible: bool) {
        let reactivated = app_active && !self.app_was_active;
        self.app_was_active = app_active;
        if !self.observation_pending && !reactivated && !help_visible {
            return;
        }

        self.observation_pending = false;
        let observation = match self.status_override {
            Some(status) => PermissionObservation::devtools(status),
            None => PermissionObservation::system(check_permissions()),
        };
        self.current_status = observation.status();
        self.report_status_if_changed(observation);
    }

    /// Notify the runtime when macOS permission status changes.
    fn report_status_if_changed(&mut self, observation: PermissionObservation) {
        if self.last_reported_status == Some(observation) {
            return;
        }
        self.last_reported_status = Some(observation);
        let Some(tx) = self.tx_ctrl.as_ref() else {
            return;
        };
        if tx
            .send(ControlMsg::PermissionsChanged(observation))
            .is_err()
        {
            tracing::warn!("failed to send permissions status change");
        }
    }

    /// Render the permissions content inside the active viewport.
    fn render_body(&mut self, ui: &mut egui::Ui, status: &PermissionsStatus) {
        CentralPanel::default().show(ui, |ui| {
            container(ui, "permissions.panel", |ui| {
                ui.add_space(14.0);
                ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    self.render_intro(ui, status);
                    ui.add_space(12.0);
                    self.render_section(
                        ui,
                        PermissionSection {
                            id: "accessibility",
                            enabled: status.accessibility_ok(),
                            name: "Accessibility",
                            help: "Allow Hotki in System Settings → Privacy & Security → Accessibility.",
                            open_settings: open_accessibility_settings,
                            notice_text: "Opening Accessibility settings...",
                            open_intent: self.open_intents.accessibility,
                        },
                    );
                    ui.add_space(10.0);
                    self.render_section(
                        ui,
                        PermissionSection {
                            id: "input_monitoring",
                            enabled: status.input_ok(),
                            name: "Input Monitoring",
                            help: "Allow Hotki in System Settings → Privacy & Security → Input Monitoring.",
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
    fn render_intro(&self, ui: &mut egui::Ui, status: &PermissionsStatus) {
        let all_granted = status.accessibility_ok() && status.input_ok();
        let (summary, color) = if all_granted {
            (
                "All required permissions are granted.",
                Color32::from_rgb(64, 201, 99),
            )
        } else {
            (
                "Hotki cannot register hotkeys until these permissions are granted.",
                Color32::from_rgb(220, 50, 47),
            )
        };
        ui.heading(RichText::new("Permissions").strong());
        ui.add_space(6.0);
        ui.dev_label(
            "permissions.summary",
            RichText::new(summary).color(color).strong(),
        );
        ui.add_space(6.0);
        ui.dev_label(
            "permissions.description",
            "Hotki requires Accessibility and Input Monitoring permissions to register hotkeys and synthesize keystrokes.",
        );
    }

    /// Render one permission status row, help text, and opener button.
    fn render_section(&mut self, ui: &mut egui::Ui, section: PermissionSection<'_>) {
        let (color, status) = permission_status_parts(section.enabled);
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
        Frame::group(ui.style()).show(ui, |ui| {
            container(ui, format!("permissions.{}.section", section.id), |ui| {
                ui.horizontal(|ui| {
                    ui.dev_label(
                        format!("permissions.{}.icon", section.id),
                        RichText::new("●").color(color),
                    );
                    ui.dev_label(
                        format!("permissions.{}.name", section.id),
                        RichText::new(section.name).strong(),
                    );
                    ui.dev_label(
                        format!("permissions.{}.status", section.id),
                        RichText::new(status).color(color).strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .dev_button(
                                format!("permissions.{}.open_settings", section.id),
                                "Open Settings…",
                            )
                            .clicked()
                        {
                            self.open_settings(section);
                            self.send_settings_notice(section.name, section.notice_text);
                        }
                    });
                });
                ui.add_space(6.0);
                ui.dev_label(format!("permissions.{}.help", section.id), section.help);
            });
        });
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
        ui.dev_label(
            "permissions.tip.text",
            RichText::new(
                "If Hotki was updated or re-installed, remove the old Hotki entry in System \
                 Settings and add it again. Restart Hotki if hotkeys still do not respond.",
            )
            .weak(),
        );
    }
}

/// Presentation details for a permission status row.
fn permission_status_parts(enabled: bool) -> (Color32, &'static str) {
    if enabled {
        (Color32::from_rgb(64, 201, 99), "Granted")
    } else {
        (Color32::from_rgb(220, 50, 47), "Not granted")
    }
}

/// Query the current process permissions.
pub fn check_permissions() -> PermissionsStatus {
    ::permissions::check_permissions()
}

/// Return whether AppKit currently considers Hotki the active application.
fn app_is_active() -> bool {
    MainThreadMarker::new().is_some_and(|mtm| NSApplication::sharedApplication(mtm).isActive())
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

#[cfg(test)]
mod tests {
    use permissions::PermissionState;
    use tokio::sync::mpsc;

    use super::*;

    /// Build a deterministic permission snapshot for observer tests.
    fn status(granted: bool) -> PermissionsStatus {
        PermissionsStatus {
            accessibility: PermissionState::from(granted),
            input_monitoring: PermissionState::from(granted),
            screen_recording: PermissionState::Unknown,
        }
    }

    #[test]
    fn hidden_override_drives_permission_observer() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut help = PermissionsHelp::new();
        help.set_control_sender(tx);
        help.set_status_override(Some(status(false)));

        assert!(!help.visible);
        help.observe_status(false, false);
        assert!(matches!(
            rx.try_recv(),
            Ok(ControlMsg::PermissionsChanged(current))
                if current.status() == status(false) && current.is_overridden()
        ));

        help.observe_status(false, false);
        assert!(rx.try_recv().is_err());

        help.set_status_override(Some(status(true)));
        help.observe_status(false, false);
        assert!(matches!(
            rx.try_recv(),
            Ok(ControlMsg::PermissionsChanged(current))
                if current.status() == status(true) && current.is_overridden()
        ));
    }

    #[test]
    fn clearing_override_reports_system_provenance_even_when_status_matches() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut help = PermissionsHelp::new();
        help.set_control_sender(tx);
        help.set_status_override(Some(status(false)));
        help.observe_status(false, false);
        let _ = rx.try_recv().expect("override observation");

        help.set_status_override(None);
        help.current_status = status(false);
        help.report_status_if_changed(PermissionObservation::system(status(false)));

        assert!(matches!(
            rx.try_recv(),
            Ok(ControlMsg::PermissionsChanged(current)) if !current.is_overridden()
        ));
    }
}
