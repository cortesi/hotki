//! Developer automation hooks for eguidev instrumentation.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use egui::{Context, Sense};
use eguidev::{
    DevMcp, DiagnosticError, FixtureCall, FixtureError, FixtureResponse, FixtureResult,
    FixtureSpec, FrameGuard, ViewportSel, WidgetRole, WidgetValue, frame_scope, id_with_meta,
    name_viewport,
};
use hotki_protocol::{
    DisplayFrame, DisplaysSnapshot, HudState, MsgToUI, NotifyKind, NotifyPos, Style, Toggle,
    rpc::InjectKind,
};
use permissions::{PermissionState, PermissionsStatus};
use serde_json::{Value, json};
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
    /// Latest app diagnostic snapshot recorded from the UI thread.
    diagnostics: Arc<Mutex<HotkiDiagnostics>>,
    /// Latest UI-thread idle state reported to eguidev settle waits.
    app_idle: Arc<AtomicBool>,
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

    /// Update the app diagnostic snapshot from the UI thread.
    pub fn update_diagnostics(
        &self,
        server_connected: bool,
        server_bindings: &[String],
        notification_stack: &[NotificationStackAlias],
    ) {
        if let Ok(mut diagnostics) = self.diagnostics.lock() {
            *diagnostics = HotkiDiagnostics::from_app_state(
                server_connected,
                server_bindings,
                notification_stack,
            );
        }
    }

    /// Update whether the UI has no finite animation work in progress.
    pub fn set_app_idle(&self, idle: bool) {
        self.app_idle.store(idle, Ordering::Relaxed);
    }

    /// Return the latest UI idle state for eguidev settle waits.
    fn is_app_idle(&self) -> bool {
        self.app_idle.load(Ordering::Relaxed)
    }

    /// Return the latest app diagnostic snapshot.
    fn diagnostic(&self) -> Result<Value, DiagnosticError> {
        let diagnostics = self
            .diagnostics
            .lock()
            .map_err(|err| DiagnosticError::new("diagnostic_lock", err.to_string()))?;
        Ok(diagnostics.to_json())
    }
}

#[derive(Clone, Default)]
/// App state exposed through `diagnostic("hotki.state")`.
struct HotkiDiagnostics {
    /// Whether the runtime/server lane is connected.
    server_connected: bool,
    /// Sorted server binding identifiers reported by the runtime.
    server_bindings: Vec<String>,
    /// Live notification stack aliases, newest first.
    notifications: Vec<NotificationDiagnostic>,
}

impl HotkiDiagnostics {
    /// Build a diagnostic snapshot from the currently rendered app state.
    fn from_app_state(
        server_connected: bool,
        server_bindings: &[String],
        notification_stack: &[NotificationStackAlias],
    ) -> Self {
        Self {
            server_connected,
            server_bindings: server_bindings.to_vec(),
            notifications: notification_stack
                .iter()
                .map(NotificationDiagnostic::from)
                .collect(),
        }
    }

    /// Convert the snapshot into script-visible JSON.
    fn to_json(&self) -> Value {
        let notifications = self
            .notifications
            .iter()
            .map(NotificationDiagnostic::to_json)
            .collect::<Vec<_>>();
        json!({
            "server": {
                "connected": self.server_connected,
                "binding_count": self.server_bindings.len(),
                "bindings": self.server_bindings,
            },
            "notifications": {
                "live_count": self.notifications.len(),
                "items": notifications,
            },
        })
    }
}

#[derive(Clone, Default)]
/// Diagnostic alias for one live notification viewport.
struct NotificationDiagnostic {
    /// Stack index, newest first.
    index: usize,
    /// Stable live notification id.
    live_id: String,
    /// Stable notification kind.
    kind: &'static str,
    /// Notification title.
    title: String,
}

impl From<&NotificationStackAlias> for NotificationDiagnostic {
    fn from(alias: &NotificationStackAlias) -> Self {
        Self {
            index: alias.index,
            live_id: alias.live_id.clone(),
            kind: alias.kind,
            title: alias.title.clone(),
        }
    }
}

impl NotificationDiagnostic {
    /// Convert the notification alias into script-visible JSON.
    fn to_json(&self) -> Value {
        json!({
            "index": self.index,
            "live_id": self.live_id,
            "kind": self.kind,
            "title": self.title,
        })
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
    let diagnostic_runtime = fixture_runtime.clone();
    let idle_runtime = fixture_runtime.clone();
    let devmcp = DevMcp::new()
        .fixtures(fixtures())
        .on_fixture_runtime(move |call| bridge.apply(call))
        .expect("hard-coded hotki fixture handler is unique")
        .diagnostic("hotki.state", move || diagnostic_runtime.diagnostic())
        .expect("hard-coded hotki diagnostic name is unique")
        .on_idle_ui(move |_| idle_runtime.is_app_idle())
        .expect("hard-coded hotki idle provider is unique");
    attach_runtime(devmcp, enable_runtime).map(|devmcp| (devmcp, fixture_runtime))
}

/// Stable fixture catalog advertised through eguidev.
fn fixtures() -> Vec<FixtureSpec> {
    vec![
        FixtureSpec::new(
            "hotki.basic.default",
            "UI-thread lane: open a clean Details window for baseline readiness.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_in("details.root", viewport_sel("details")),
        FixtureSpec::new("hotki.details", "UI-thread lane: open the Details window.")
            .anchor_value("app.ready", WidgetValue::Bool(true))
            .anchor_in("details.tab.notifications", viewport_sel("details")),
        FixtureSpec::new(
            "hotki.details.config",
            "UI-thread lane: open Details directly to the Config tab.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value_in(
            "details.active_tab",
            WidgetValue::Text("config".to_string()),
            viewport_sel("details"),
        )
        .anchor_in("details.config.reload", viewport_sel("details")),
        FixtureSpec::new(
            "hotki.details.logs",
            "UI-thread lane: open Details directly to the Logs tab.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value_in(
            "details.active_tab",
            WidgetValue::Text("logs".to_string()),
            viewport_sel("details"),
        )
        .anchor_in("details.logs.clear", viewport_sel("details")),
        FixtureSpec::new(
            "hotki.details.about",
            "UI-thread lane: open Details directly to the About tab.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value_in(
            "details.active_tab",
            WidgetValue::Text("about".to_string()),
            viewport_sel("details"),
        )
        .anchor_in("details.about.name", viewport_sel("details")),
        FixtureSpec::new(
            "hotki.permissions",
            "UI-thread lane: open the Permissions helper window.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_in("permissions.root", viewport_sel("permissions")),
        FixtureSpec::new(
            "hotki.permissions.all_granted",
            "UI-thread lane: open Permissions with deterministic granted status.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value_in(
            "permissions.accessibility.granted",
            WidgetValue::Bool(true),
            viewport_sel("permissions"),
        )
        .anchor_value_in(
            "permissions.input_monitoring.granted",
            WidgetValue::Bool(true),
            viewport_sel("permissions"),
        ),
        FixtureSpec::new(
            "hotki.permissions.none_granted",
            "UI-thread lane: open Permissions with deterministic missing status.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value_in(
            "permissions.accessibility.granted",
            WidgetValue::Bool(false),
            viewport_sel("permissions"),
        )
        .anchor_value_in(
            "permissions.input_monitoring.granted",
            WidgetValue::Bool(false),
            viewport_sel("permissions"),
        ),
        FixtureSpec::new(
            "hotki.permissions.mixed",
            "UI-thread lane: open Permissions with Accessibility granted and Input Monitoring missing.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value_in(
            "permissions.accessibility.granted",
            WidgetValue::Bool(true),
            viewport_sel("permissions"),
        )
        .anchor_value_in(
            "permissions.input_monitoring.granted",
            WidgetValue::Bool(false),
            viewport_sel("permissions"),
        ),
        FixtureSpec::new(
            "hotki.notifications",
            "UI-thread lane: create a deterministic notification and open Details history.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_in("details.notification.0.title", viewport_sel("details")),
        FixtureSpec::new(
            "hotki.notifications.variants",
            "UI-thread lane: create deterministic default-right notification variants.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true)),
        FixtureSpec::new(
            "hotki.notifications.left_variants",
            "UI-thread lane: create deterministic explicit-left notification variants.",
        )
        .anchor_value("app.ready", WidgetValue::Bool(true)),
        FixtureSpec::new(
            "hotki.notifications.truncated",
            "UI-thread lane: create a notification that must vertically truncate on a short display.",
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
        .anchor_in("hud.panel", viewport_sel("hud")),
        FixtureSpec::new(
            "hotki.hud.mini",
            "Runtime/server lane: enter the demo mini HUD submenu through server key injection.",
        )
        .precondition_value("app.server.connected", WidgetValue::Bool(true))
        .precondition_value("app.server.bindings.loaded", WidgetValue::Bool(true))
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value("app.server.connected", WidgetValue::Bool(true))
        .anchor_value_in(
            "hud.mode",
            WidgetValue::Text("mini".to_string()),
            viewport_sel("hud"),
        )
        .anchor_in("hud.mini.title", viewport_sel("hud")),
        FixtureSpec::new(
            "hotki.selector",
            "Runtime/server lane: open the demo selector through server key injection.",
        )
        .precondition_value("app.server.connected", WidgetValue::Bool(true))
        .precondition_value("app.server.bindings.loaded", WidgetValue::Bool(true))
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value("app.server.connected", WidgetValue::Bool(true))
        .anchor_in("selector.panel", viewport_sel("selector")),
        FixtureSpec::new(
            "hotki.selector.query",
            "Runtime/server lane: open the selector and type a deterministic query.",
        )
        .precondition_value("app.server.connected", WidgetValue::Bool(true))
        .precondition_value("app.server.bindings.loaded", WidgetValue::Bool(true))
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value("app.server.connected", WidgetValue::Bool(true))
        .anchor_in("selector.panel", viewport_sel("selector")),
        FixtureSpec::new(
            "hotki.selector.confirmed",
            "Runtime/server lane: confirm the currently open selector, then open Details history.",
        )
        .precondition_value("app.server.connected", WidgetValue::Bool(true))
        .precondition_value("app.server.bindings.loaded", WidgetValue::Bool(true))
        .precondition_in("selector.panel", viewport_sel("selector"))
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value("app.server.connected", WidgetValue::Bool(true))
        .anchor_in("details.notification.0.title", viewport_sel("details")),
        FixtureSpec::new(
            "hotki.selector.canceled",
            "Runtime/server lane: cancel the currently open selector, then open Details history.",
        )
        .precondition_value("app.server.connected", WidgetValue::Bool(true))
        .precondition_value("app.server.bindings.loaded", WidgetValue::Bool(true))
        .precondition_in("selector.panel", viewport_sel("selector"))
        .anchor_value("app.ready", WidgetValue::Bool(true))
        .anchor_value("app.server.connected", WidgetValue::Bool(true))
        .anchor_in("details.notification.0.title", viewport_sel("details")),
    ]
}

/// Build a checked semantic viewport selector for a hard-coded Hotki viewport.
fn viewport_sel(name: &'static str) -> ViewportSel {
    ViewportSel::name(name).expect("hard-coded hotki viewport name is valid")
}

/// Synthetic display small enough to force vertical notification truncation.
fn short_display_snapshot() -> DisplaysSnapshot {
    DisplaysSnapshot {
        global_top: 90.0,
        active: Some(DisplayFrame {
            id: 1,
            x: 0.0,
            y: 0.0,
            width: 500.0,
            height: 90.0,
        }),
        displays: Vec::new(),
    }
}

/// Fixture bridge into production UI and runtime channels.
#[derive(Clone)]
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
    fn apply(&self, call: &FixtureCall) -> FixtureResult {
        self.apply_name(&call.name)
            .map(|()| FixtureResponse::new())
            .map_err(|error| FixtureError::new("hotki_fixture", error))
    }

    /// Dispatch one named fixture onto the lane that owns the affected state.
    fn apply_name(&self, name: &str) -> Result<(), String> {
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
                self.reconfigure_notifications(NotifyPos::Right, DisplaysSnapshot::default())?;
                self.send_ui_message(MsgToUI::Notify {
                    kind: NotifyKind::Info,
                    title: "Eguidev".to_string(),
                    text: "Deterministic notification fixture".to_string(),
                })?;
                self.show_details()?;
            }
            "hotki.notifications.variants" => {
                self.clear_transient_ui()?;
                self.send_notification_variants(NotifyPos::Right)?;
            }
            "hotki.notifications.left_variants" => {
                self.clear_transient_ui()?;
                self.send_notification_variants(NotifyPos::Left)?;
            }
            "hotki.notifications.truncated" => {
                self.clear_transient_ui()?;
                self.send_truncated_notification()?;
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
    fn send_notification_variants(&self, pos: NotifyPos) -> Result<(), String> {
        self.reconfigure_notifications(pos, DisplaysSnapshot::default())?;
        for (kind, title, text) in [
            (
                NotifyKind::Info,
                "Information With A Long Title That Wraps Inside The Card",
                "This notification body is ordinary wrapped prose that should remain fully visible \
                 inside the measured card.",
            ),
            (NotifyKind::Warn, "Warning", "Warning notification fixture"),
            (
                NotifyKind::Error,
                "Error",
                "Error notification fixture: \
                 /Users/example/hotki/long-unbroken-path-that-must-wrap-inside-the-card-without-being-clipped",
            ),
            (
                NotifyKind::Success,
                "Success",
                "Success notification fixture",
            ),
        ] {
            self.send_ui_message(MsgToUI::Notify {
                kind,
                title: title.to_string(),
                text: text.to_string(),
            })?;
        }
        Ok(())
    }

    /// Create one notification with a deliberately over-height body.
    fn send_truncated_notification(&self) -> Result<(), String> {
        self.reconfigure_notifications(NotifyPos::Right, short_display_snapshot())?;
        self.send_ui_message(MsgToUI::Notify {
            kind: NotifyKind::Warn,
            title: "Tall Notification".to_string(),
            text: "This notification body is intentionally long. ".repeat(80),
        })
    }

    /// Reconfigure notification placement through the production HUD update path.
    fn reconfigure_notifications(
        &self,
        pos: NotifyPos,
        displays: DisplaysSnapshot,
    ) -> Result<(), String> {
        let mut style = Style::default();
        style.notify.pos = pos;
        self.send_ui_message(MsgToUI::HudUpdate {
            hud: Box::new(HudState {
                visible: false,
                rows: Vec::new(),
                depth: 0,
                breadcrumbs: Vec::new(),
                style,
                capture: false,
            }),
            displays,
        })
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
    viewport_name: impl Into<String>,
    container_id: impl Into<String>,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let viewport_name = viewport_name.into();
    frame_scope(devmcp, ui, container_id, |ui| {
        name_viewport(ui.ctx(), viewport_name);
        add_contents(ui)
    })
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
