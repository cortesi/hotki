//! Developer automation hooks for eguidev instrumentation.

use std::sync::{Arc, Mutex};

use egui::{Context, Sense};
use eguidev::{DevMcp, FixtureSpec, FrameGuard, WidgetRole, WidgetValue, id_with_meta};
use hotki_protocol::{MsgToUI, NotifyKind, Toggle, rpc::InjectKind};
use permissions::{PermissionState, PermissionsStatus};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    app::{UiCommand, UiEvent},
    details::DetailsTab,
    notification::{NotificationStackAlias, render_stack_metadata},
    runtime::ControlMsg,
};

#[derive(Clone, Default)]
/// Shared fixture runtime state filled in as eframe finishes startup.
pub struct FixtureRuntime {
    /// Egui context becomes available only inside `HotkiApp::new`.
    ctx: Arc<Mutex<Option<Context>>>,
}

impl FixtureRuntime {
    /// Store the egui context used to request frames after fixture application.
    pub fn set_context(&self, ctx: Context) {
        if let Ok(mut stored) = self.ctx.lock() {
            *stored = Some(ctx);
        }
        self.request_repaint();
    }

    /// Request a repaint if eframe has already provided a context.
    fn request_repaint(&self) {
        if let Ok(stored) = self.ctx.lock()
            && let Some(ctx) = stored.as_ref()
        {
            ctx.request_repaint();
        }
    }
}

/// Build the DevMCP handle for this run.
pub fn build_devmcp(
    enable_runtime: bool,
    tx_ui: UnboundedSender<UiEvent>,
    tx_ctrl: UnboundedSender<ControlMsg>,
) -> Result<(DevMcp, FixtureRuntime), String> {
    let fixture_runtime = FixtureRuntime::default();
    let bridge = FixtureBridge {
        tx_ui,
        tx_ctrl,
        fixture_runtime: fixture_runtime.clone(),
    };
    let devmcp = DevMcp::new()
        .fixtures(fixtures())
        .on_fixture(move |name| bridge.apply(name));
    attach_runtime(devmcp, enable_runtime).map(|devmcp| (devmcp, fixture_runtime))
}

/// Stable fixture catalog advertised through eguidev.
fn fixtures() -> Vec<FixtureSpec> {
    let details = egui::ViewportId::from_hash_of("hotki_details");
    let hud = egui::ViewportId::from_hash_of("hotki_hud");
    let permissions = egui::ViewportId::from_hash_of("hotki_permissions");
    let selector = egui::ViewportId::from_hash_of("hotki_selector");
    vec![
        FixtureSpec::new(
            "hotki.basic.default",
            "UI-thread lane: open a clean Details window for baseline readiness.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_in("details.root", details),
        FixtureSpec::new("hotki.details", "UI-thread lane: open the Details window.")
            .anchor_value("app.ready", WidgetValue::Bool(true))
            .anchor_in("details.tab.notifications", details),
        FixtureSpec::new(
            "hotki.details.config",
            "UI-thread lane: open Details directly to the Config tab.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value_in(
            "details.active_tab",
            WidgetValue::Text("config".to_string()),
            details,
        )
        .anchor_in("details.config.reload", details),
        FixtureSpec::new(
            "hotki.details.logs",
            "UI-thread lane: open Details directly to the Logs tab.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value_in(
            "details.active_tab",
            WidgetValue::Text("logs".to_string()),
            details,
        )
        .anchor_in("details.logs.clear", details),
        FixtureSpec::new(
            "hotki.details.about",
            "UI-thread lane: open Details directly to the About tab.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value_in(
            "details.active_tab",
            WidgetValue::Text("about".to_string()),
            details,
        )
        .anchor_in("details.about.name", details),
        FixtureSpec::new(
            "hotki.permissions",
            "UI-thread lane: open the Permissions helper window.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_in("permissions.root", permissions),
        FixtureSpec::new(
            "hotki.permissions.all_granted",
            "UI-thread lane: open Permissions with deterministic granted status.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value_in(
            "permissions.accessibility.granted",
            WidgetValue::Bool(true),
            permissions,
        )
        .anchor_value_in(
            "permissions.input_monitoring.granted",
            WidgetValue::Bool(true),
            permissions,
        ),
        FixtureSpec::new(
            "hotki.permissions.none_granted",
            "UI-thread lane: open Permissions with deterministic missing status.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value_in(
            "permissions.accessibility.granted",
            WidgetValue::Bool(false),
            permissions,
        )
        .anchor_value_in(
            "permissions.input_monitoring.granted",
            WidgetValue::Bool(false),
            permissions,
        ),
        FixtureSpec::new(
            "hotki.permissions.mixed",
            "UI-thread lane: open Permissions with Accessibility granted and Input Monitoring missing.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value_in(
            "permissions.accessibility.granted",
            WidgetValue::Bool(true),
            permissions,
        )
        .anchor_value_in(
            "permissions.input_monitoring.granted",
            WidgetValue::Bool(false),
            permissions,
        ),
        FixtureSpec::new(
            "hotki.notifications",
            "UI-thread lane: create a deterministic notification and open Details history.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_in("details.notification.0.title", details),
        FixtureSpec::new(
            "hotki.notifications.variants",
            "UI-thread lane: create deterministic info, warning, error, and success notifications.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true)),
        FixtureSpec::new(
            "hotki.hud",
            "Runtime/server lane: open the demo HUD through server key injection.",
        )
        .precondition_value("app.server.connected", WidgetValue::Bool(true))
        .precondition_value("app.server.bindings.loaded", WidgetValue::Bool(true))
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value("app.server.connected", WidgetValue::Bool(true))
        .anchor_in("hud.panel", hud),
        FixtureSpec::new(
            "hotki.hud.mini",
            "Runtime/server lane: enter the demo mini HUD submenu through server key injection.",
        )
        .precondition_value("app.server.connected", WidgetValue::Bool(true))
        .precondition_value("app.server.bindings.loaded", WidgetValue::Bool(true))
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value("app.server.connected", WidgetValue::Bool(true))
        .anchor_value_in("hud.mode", WidgetValue::Text("mini".to_string()), hud)
        .anchor_in("hud.mini.title", hud),
        FixtureSpec::new(
            "hotki.selector",
            "Runtime/server lane: open the demo selector through server key injection.",
        )
        .precondition_value("app.server.connected", WidgetValue::Bool(true))
        .precondition_value("app.server.bindings.loaded", WidgetValue::Bool(true))
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value("app.server.connected", WidgetValue::Bool(true))
        .anchor_in("selector.panel", selector),
        FixtureSpec::new(
            "hotki.selector.query",
            "Runtime/server lane: open the selector and type a deterministic query.",
        )
        .precondition_value("app.server.connected", WidgetValue::Bool(true))
        .precondition_value("app.server.bindings.loaded", WidgetValue::Bool(true))
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value("app.server.connected", WidgetValue::Bool(true))
        .anchor_in("selector.panel", selector),
        FixtureSpec::new(
            "hotki.selector.confirmed",
            "Runtime/server lane: confirm the currently open selector, then open Details history.",
        )
        .precondition_value("app.server.connected", WidgetValue::Bool(true))
        .precondition_value("app.server.bindings.loaded", WidgetValue::Bool(true))
        .precondition_in("selector.panel", selector)
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value("app.server.connected", WidgetValue::Bool(true))
        .anchor_in("details.notification.0.title", details),
        FixtureSpec::new(
            "hotki.selector.canceled",
            "Runtime/server lane: cancel the currently open selector, then open Details history.",
        )
        .precondition_value("app.server.connected", WidgetValue::Bool(true))
        .precondition_value("app.server.bindings.loaded", WidgetValue::Bool(true))
        .precondition_in("selector.panel", selector)
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value("app.server.connected", WidgetValue::Bool(true))
        .anchor_in("details.notification.0.title", details),
    ]
}

#[derive(Clone)]
/// Fixture bridge into production UI and runtime channels.
struct FixtureBridge {
    /// Sender for UI-thread events.
    tx_ui: UnboundedSender<UiEvent>,
    /// Sender for runtime/server control events.
    tx_ctrl: UnboundedSender<ControlMsg>,
    /// Runtime state used after eframe provides its egui context.
    fixture_runtime: FixtureRuntime,
}

impl FixtureBridge {
    /// Apply a named fixture through Hotki's existing event lanes.
    fn apply(&self, name: &str) -> Result<(), String> {
        match name {
            "hotki.basic.default" | "hotki.details" => {
                self.clear_transient_ui()?;
                self.show_details()?;
            }
            "hotki.details.config" => {
                self.clear_transient_ui()?;
                self.show_details_tab(DetailsTab::Config)?;
            }
            "hotki.details.logs" => {
                self.clear_transient_ui()?;
                self.show_details_tab(DetailsTab::Logs)?;
            }
            "hotki.details.about" => {
                self.clear_transient_ui()?;
                self.show_details_tab(DetailsTab::About)?;
            }
            "hotki.permissions" => {
                self.clear_transient_ui()?;
                self.send_ui_command(UiCommand::ShowPermissionsHelp)?;
            }
            "hotki.permissions.all_granted" => {
                self.clear_transient_ui()?;
                self.set_permission_override(true, true)?;
                self.send_ui_command(UiCommand::ShowPermissionsHelp)?;
            }
            "hotki.permissions.none_granted" => {
                self.clear_transient_ui()?;
                self.set_permission_override(false, false)?;
                self.send_ui_command(UiCommand::ShowPermissionsHelp)?;
            }
            "hotki.permissions.mixed" => {
                self.clear_transient_ui()?;
                self.set_permission_override(true, false)?;
                self.send_ui_command(UiCommand::ShowPermissionsHelp)?;
            }
            "hotki.notifications" => {
                self.clear_transient_ui()?;
                self.send_ui_message(MsgToUI::Notify {
                    kind: NotifyKind::Info,
                    title: "Eguidev".to_string(),
                    text: "Deterministic notification fixture".to_string(),
                })?;
                self.show_details()?;
            }
            "hotki.notifications.variants" => {
                self.clear_transient_ui()?;
                self.send_notification_variants()?;
            }
            "hotki.hud" => {
                self.clear_transient_ui()?;
                self.inject_key_quiet("escape")?;
                self.inject_key("cmd+shift+0")?;
            }
            "hotki.hud.mini" => {
                self.clear_transient_ui()?;
                self.inject_key_quiet("escape")?;
                self.inject_keys(["cmd+shift+0", "t", "m"])?;
            }
            "hotki.selector" => {
                self.clear_transient_ui()?;
                self.inject_key_quiet("escape")?;
                self.inject_keys(["cmd+shift+0", "t", "s"])?;
            }
            "hotki.selector.query" => {
                self.clear_transient_ui()?;
                self.inject_key_quiet("escape")?;
                self.inject_keys(["cmd+shift+0", "t", "s", "b"])?;
            }
            "hotki.selector.confirmed" => {
                self.inject_key("return")?;
                self.show_details()?;
            }
            "hotki.selector.canceled" => {
                self.inject_key("escape")?;
                self.show_details()?;
            }
            _ => return Err(format!("unknown Hotki fixture: {name}")),
        }
        self.fixture_runtime.request_repaint();
        Ok(())
    }

    /// Create one visible notification for each practical display kind.
    fn send_notification_variants(&self) -> Result<(), String> {
        for (kind, title) in [
            (NotifyKind::Info, "Info"),
            (NotifyKind::Warn, "Warning"),
            (NotifyKind::Error, "Error"),
            (NotifyKind::Success, "Success"),
        ] {
            let text = if kind == NotifyKind::Error {
                "Error notification fixture: /Users/example/hotki/long-unbroken-path-that-must-wrap-inside-the-card-without-being-clipped".to_string()
            } else {
                format!("{title} notification fixture")
            };
            self.send_ui_message(MsgToUI::Notify {
                kind,
                title: title.to_string(),
                text,
            })?;
        }
        Ok(())
    }

    /// Clear UI-local transient state before applying a fixture.
    fn clear_transient_ui(&self) -> Result<(), String> {
        self.send_ui_message(MsgToUI::SelectorHide)?;
        self.send_ui_message(MsgToUI::ClearNotifications)?;
        self.send_ui_command(UiCommand::SetPermissionStatusOverride(None))?;
        self.send_ui_message(MsgToUI::ShowDetails(Toggle::Off))
    }

    /// Open Details through the same UI event consumed by normal actions.
    fn show_details(&self) -> Result<(), String> {
        self.send_ui_message(MsgToUI::ShowDetails(Toggle::On))
    }

    /// Open Details with a specific tab selected through the UI thread.
    fn show_details_tab(&self, tab: DetailsTab) -> Result<(), String> {
        self.send_ui_command(UiCommand::ShowDetailsTab(tab))
    }

    /// Override permission status for deterministic fixtures.
    fn set_permission_override(
        &self,
        accessibility: bool,
        input_monitoring: bool,
    ) -> Result<(), String> {
        self.send_ui_command(UiCommand::SetPermissionStatusOverride(Some(
            PermissionsStatus {
                accessibility: PermissionState::from(accessibility),
                input_monitoring: PermissionState::from(input_monitoring),
                screen_recording: PermissionState::Unknown,
            },
        )))
    }

    /// Send a local UI command through the production UI channel.
    fn send_ui_command(&self, command: UiCommand) -> Result<(), String> {
        self.tx_ui
            .send(UiEvent::Command(command))
            .map_err(|err| format!("failed to send UI command: {err}"))
    }

    /// Send a protocol UI message through the production UI channel.
    fn send_ui_message(&self, message: MsgToUI) -> Result<(), String> {
        self.tx_ui
            .send(UiEvent::Message(message))
            .map_err(|err| format!("failed to send UI message: {err}"))
    }

    /// Inject a complete key press through the runtime/server lane.
    fn inject_key(&self, ident: &str) -> Result<(), String> {
        self.inject_key_with_reporting(ident, true)
    }

    /// Attempt a cleanup key press without turning an unbound key into a UI notification.
    fn inject_key_quiet(&self, ident: &str) -> Result<(), String> {
        self.inject_key_with_reporting(ident, false)
    }

    /// Inject a complete key press through the runtime/server lane.
    fn inject_key_with_reporting(&self, ident: &str, report_errors: bool) -> Result<(), String> {
        self.send_control(ControlMsg::InjectKey {
            ident: ident.to_string(),
            kind: InjectKind::Down,
            repeat: false,
            report_errors,
        })?;
        self.send_control(ControlMsg::InjectKey {
            ident: ident.to_string(),
            kind: InjectKind::Up,
            repeat: false,
            report_errors: false,
        })
    }

    /// Inject a sequence of complete key presses through the runtime/server lane.
    fn inject_keys<const N: usize>(&self, idents: [&str; N]) -> Result<(), String> {
        for ident in idents {
            self.inject_key(ident)?;
        }
        Ok(())
    }

    /// Send a runtime control message.
    fn send_control(&self, message: ControlMsg) -> Result<(), String> {
        self.tx_ctrl
            .send(message)
            .map_err(|err| format!("failed to send runtime control: {err}"))
    }
}

#[cfg(feature = "devtools")]
/// Attach the native eguidev runtime when requested by the hidden devtools flag.
fn attach_runtime(devmcp: DevMcp, enable_runtime: bool) -> Result<DevMcp, String> {
    if enable_runtime {
        Ok(eguidev_runtime::attach(devmcp))
    } else {
        Ok(devmcp)
    }
}

#[cfg(not(feature = "devtools"))]
/// Reject runtime attachment when the binary was built without devtools support.
fn attach_runtime(devmcp: DevMcp, enable_runtime: bool) -> Result<DevMcp, String> {
    if enable_runtime {
        return Err("--dev-mcp requires building hotki with --features devtools".to_string());
    }
    Ok(devmcp)
}

/// Run an immediate viewport body under eguidev instrumentation.
pub fn viewport_frame<R>(
    devmcp: &DevMcp,
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let ctx = ui.ctx().clone();
    let _guard = FrameGuard::new(devmcp, &ctx);
    add_contents(ui)
}

/// Render root-viewport readiness anchors for scripts and fixture checks.
pub fn render_app_anchors(
    devmcp: &DevMcp,
    ctx: &Context,
    server_connected: bool,
    server_bindings: &[String],
    notification_stack: &[NotificationStackAlias],
) {
    if !devmcp.is_enabled() {
        return;
    }
    let _guard = FrameGuard::new(devmcp, ctx);
    egui::Area::new(egui::Id::new("hotki.devtools.anchors"))
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            readiness_anchor(ui, "app.ready", WidgetValue::Bool(true));
            readiness_anchor(
                ui,
                "app.server.connected",
                WidgetValue::Bool(server_connected),
            );
            readiness_anchor(
                ui,
                "app.server.bindings.loaded",
                WidgetValue::Bool(!server_bindings.is_empty()),
            );
            readiness_anchor(
                ui,
                "app.server.bindings",
                WidgetValue::Text(server_bindings.join(",")),
            );
            render_stack_metadata(ui, notification_stack);
        });
}

/// Record one tiny root-viewport readiness anchor.
fn readiness_anchor(ui: &mut egui::Ui, id: &'static str, value: WidgetValue) {
    value_anchor(ui, id, value);
}

/// Record a tiny metadata-only widget without changing visible content.
pub fn value_anchor(ui: &mut egui::Ui, id: impl Into<String>, value: WidgetValue) {
    let id = id.into();
    id_with_meta(
        ui,
        id.clone(),
        WidgetRole::Label,
        Some(id),
        Some(value),
        |ui| {
            let (_rect, response) = ui.allocate_exact_size(egui::Vec2::splat(1.0), Sense::hover());
            response
        },
    );
}
