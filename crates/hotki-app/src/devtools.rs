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
    DisplayFrame, DisplaysSnapshot, HudRow, HudState, Mode, MsgToUI, NotifyKind, NotifyPos, Style,
    Toggle,
};
use mac_keycode::Chord;
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

/// Stable fixture ids advertised through eguidev and dispatched by `FixtureBridge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotkiFixture {
    /// Baseline Details readiness fixture.
    BasicDefault,
    /// Details window fixture.
    Details,
    /// Details Config tab fixture.
    DetailsConfig,
    /// Details Logs tab fixture.
    DetailsLogs,
    /// Details About tab fixture.
    DetailsAbout,
    /// Permissions helper fixture.
    Permissions,
    /// Permissions fixture with all required grants present.
    PermissionsAllGranted,
    /// Permissions fixture with all required grants missing.
    PermissionsNoneGranted,
    /// Permissions fixture with mixed grant state.
    PermissionsMixed,
    /// Single notification plus Details history fixture.
    Notifications,
    /// Default-right notification variants fixture.
    NotificationVariants,
    /// Explicit-left notification variants fixture.
    NotificationLeftVariants,
    /// Vertically truncated notification fixture.
    NotificationTruncated,
    /// Server-driven HUD fixture.
    Hud,
    /// Server-driven Demo submenu fixture.
    HudDemo,
    /// Direct tall HUD fixture.
    HudTall,
    /// Server-driven mini HUD fixture.
    HudMini,
    /// Server-driven selector fixture.
    Selector,
    /// Server-driven selector query fixture.
    SelectorQuery,
    /// Selector confirmation fixture.
    SelectorConfirmed,
    /// Selector cancellation fixture.
    SelectorCanceled,
}

/// Catalog metadata for a fixture id.
#[derive(Debug, Clone, Copy)]
struct FixtureDef {
    /// Typed fixture id used for dispatch.
    fixture: HotkiFixture,
    /// Stable fixture id exposed over eguidev.
    name: &'static str,
    /// Human-readable fixture description.
    description: &'static str,
}

impl FixtureDef {
    /// Construct a fixture definition.
    const fn new(fixture: HotkiFixture, name: &'static str, description: &'static str) -> Self {
        Self {
            fixture,
            name,
            description,
        }
    }

    /// Fixture catalog entry advertised through eguidev.
    fn spec(self) -> FixtureSpec {
        self.fixture.spec(self.name, self.description)
    }
}

/// Fixtures in the order advertised to eguidev.
const HOTKI_FIXTURES: &[FixtureDef] = &[
    FixtureDef::new(
        HotkiFixture::BasicDefault,
        "hotki.basic.default",
        "UI-thread lane: open a clean Details window for baseline readiness.",
    ),
    FixtureDef::new(
        HotkiFixture::Details,
        "hotki.details",
        "UI-thread lane: open the Details window.",
    ),
    FixtureDef::new(
        HotkiFixture::DetailsConfig,
        "hotki.details.config",
        "UI-thread lane: open Details directly to the Config tab.",
    ),
    FixtureDef::new(
        HotkiFixture::DetailsLogs,
        "hotki.details.logs",
        "UI-thread lane: open Details directly to the Logs tab.",
    ),
    FixtureDef::new(
        HotkiFixture::DetailsAbout,
        "hotki.details.about",
        "UI-thread lane: open Details directly to the About tab.",
    ),
    FixtureDef::new(
        HotkiFixture::Permissions,
        "hotki.permissions",
        "UI-thread lane: open the Permissions helper window.",
    ),
    FixtureDef::new(
        HotkiFixture::PermissionsAllGranted,
        "hotki.permissions.all_granted",
        "UI-thread lane: open Permissions with deterministic granted status.",
    ),
    FixtureDef::new(
        HotkiFixture::PermissionsNoneGranted,
        "hotki.permissions.none_granted",
        "UI-thread lane: open Permissions with deterministic missing status.",
    ),
    FixtureDef::new(
        HotkiFixture::PermissionsMixed,
        "hotki.permissions.mixed",
        "UI-thread lane: open Permissions with Accessibility granted and Input Monitoring missing.",
    ),
    FixtureDef::new(
        HotkiFixture::Notifications,
        "hotki.notifications",
        "UI-thread lane: create a deterministic notification and open Details history.",
    ),
    FixtureDef::new(
        HotkiFixture::NotificationVariants,
        "hotki.notifications.variants",
        "UI-thread lane: create deterministic default-right notification variants.",
    ),
    FixtureDef::new(
        HotkiFixture::NotificationLeftVariants,
        "hotki.notifications.left_variants",
        "UI-thread lane: create deterministic explicit-left notification variants.",
    ),
    FixtureDef::new(
        HotkiFixture::NotificationTruncated,
        "hotki.notifications.truncated",
        "UI-thread lane: create a notification that must vertically truncate on a short display.",
    ),
    FixtureDef::new(
        HotkiFixture::Hud,
        "hotki.hud",
        "Runtime/server lane: open the demo HUD through server key injection.",
    ),
    FixtureDef::new(
        HotkiFixture::HudDemo,
        "hotki.hud.demo",
        "Runtime/server lane: enter the Demo submenu through server key injection.",
    ),
    FixtureDef::new(
        HotkiFixture::HudTall,
        "hotki.hud.tall",
        "UI-thread lane: render a tall HUD that should fit without clipping.",
    ),
    FixtureDef::new(
        HotkiFixture::HudMini,
        "hotki.hud.mini",
        "UI-thread lane: render the deterministic mini HUD style.",
    ),
    FixtureDef::new(
        HotkiFixture::Selector,
        "hotki.selector",
        "Runtime/server lane: open the demo selector through server key injection.",
    ),
    FixtureDef::new(
        HotkiFixture::SelectorQuery,
        "hotki.selector.query",
        "Runtime/server lane: open the selector and type a deterministic query.",
    ),
    FixtureDef::new(
        HotkiFixture::SelectorConfirmed,
        "hotki.selector.confirmed",
        "Runtime/server lane: confirm the currently open selector, then open Details history.",
    ),
    FixtureDef::new(
        HotkiFixture::SelectorCanceled,
        "hotki.selector.canceled",
        "Runtime/server lane: cancel the currently open selector, then open Details history.",
    ),
];

impl HotkiFixture {
    /// Look up a fixture by its stable eguidev id.
    fn from_name(name: &str) -> Option<Self> {
        HOTKI_FIXTURES
            .iter()
            .find(|def| def.name == name)
            .map(|def| def.fixture)
    }

    /// Fixture catalog entry advertised through eguidev.
    fn spec(self, name: &'static str, description: &'static str) -> FixtureSpec {
        let spec = FixtureSpec::new(name, description);
        match self {
            Self::BasicDefault => {
                app_ready(spec).anchor_in("details.root", viewport_sel("details"))
            }
            Self::Details => {
                app_ready(spec).anchor_in("details.tab.notifications", viewport_sel("details"))
            }
            Self::DetailsConfig => details_tab_spec(spec, "config", "details.config.reload"),
            Self::DetailsLogs => details_tab_spec(spec, "logs", "details.logs.clear"),
            Self::DetailsAbout => details_tab_spec(spec, "about", "details.about.name"),
            Self::Permissions => {
                app_ready(spec).anchor_in("permissions.root", viewport_sel("permissions"))
            }
            Self::PermissionsAllGranted => permission_status_spec(spec, true, true),
            Self::PermissionsNoneGranted => permission_status_spec(spec, false, false),
            Self::PermissionsMixed => permission_status_spec(spec, true, false),
            Self::Notifications => {
                app_ready(spec).anchor_in("details.notification.0.title", viewport_sel("details"))
            }
            Self::NotificationVariants
            | Self::NotificationLeftVariants
            | Self::NotificationTruncated => app_ready(spec),
            Self::Hud => runtime_ready(spec)
                .anchor_value_in(
                    "hud.row.0.chord",
                    WidgetValue::Text("d".to_string()),
                    viewport_sel("hud"),
                )
                .anchor_in("hud.panel", viewport_sel("hud")),
            Self::HudDemo => runtime_ready(spec)
                .precondition_value_in(
                    "hud.row.0.chord",
                    WidgetValue::Text("d".to_string()),
                    viewport_sel("hud"),
                )
                .anchor_value_in(
                    "hud.row.0.chord",
                    WidgetValue::Text("n".to_string()),
                    viewport_sel("hud"),
                ),
            Self::HudTall => app_ready(spec).anchor_in("hud.row.21.desc", viewport_sel("hud")),
            Self::HudMini => app_ready(spec)
                .anchor_value_in(
                    "hud.mode",
                    WidgetValue::Text("mini".to_string()),
                    viewport_sel("hud"),
                )
                .anchor_in("hud.mini.title", viewport_sel("hud")),
            Self::Selector => runtime_ready(spec)
                .precondition_value_in(
                    "hud.row.0.chord",
                    WidgetValue::Text("n".to_string()),
                    viewport_sel("hud"),
                )
                .anchor_in("selector.panel", viewport_sel("selector")),
            Self::SelectorQuery => runtime_ready(spec)
                .precondition_in("selector.panel", viewport_sel("selector"))
                .anchor_value_in(
                    "selector.item.0.label",
                    WidgetValue::Text("Beta".to_string()),
                    viewport_sel("selector"),
                ),
            Self::SelectorConfirmed | Self::SelectorCanceled => runtime_ready(spec)
                .precondition_in("selector.panel", viewport_sel("selector"))
                .anchor_in("details.notification.0.title", viewport_sel("details")),
        }
    }
}

/// Stable fixture catalog advertised through eguidev.
fn fixtures() -> Vec<FixtureSpec> {
    HOTKI_FIXTURES
        .iter()
        .copied()
        .map(FixtureDef::spec)
        .collect()
}

/// Add the baseline app-ready anchor common to every UI fixture.
fn app_ready(spec: FixtureSpec) -> FixtureSpec {
    spec.anchor_value("app.ready", WidgetValue::Bool(true))
}

/// Add runtime/server preconditions and anchors common to server-driven fixtures.
fn runtime_ready(spec: FixtureSpec) -> FixtureSpec {
    app_ready(spec)
        .precondition_value("app.server.connected", WidgetValue::Bool(true))
        .precondition_value("app.server.bindings.loaded", WidgetValue::Bool(true))
        .anchor_value("app.server.connected", WidgetValue::Bool(true))
}

/// Build a Details tab fixture with the shared active-tab anchor.
fn details_tab_spec(spec: FixtureSpec, tab: &'static str, anchor: &'static str) -> FixtureSpec {
    app_ready(spec)
        .anchor_value_in(
            "details.active_tab",
            WidgetValue::Text(tab.to_string()),
            viewport_sel("details"),
        )
        .anchor_in(anchor, viewport_sel("details"))
}

/// Build a Permissions fixture with deterministic grant-state anchors.
fn permission_status_spec(
    spec: FixtureSpec,
    accessibility: bool,
    input_monitoring: bool,
) -> FixtureSpec {
    app_ready(spec)
        .anchor_value_in(
            "permissions.accessibility.granted",
            WidgetValue::Bool(accessibility),
            viewport_sel("permissions"),
        )
        .anchor_value_in(
            "permissions.input_monitoring.granted",
            WidgetValue::Bool(input_monitoring),
            viewport_sel("permissions"),
        )
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

/// Synthetic display used for tall HUD layout coverage.
fn tall_hud_display_snapshot() -> DisplaysSnapshot {
    DisplaysSnapshot {
        global_top: 900.0,
        active: Some(DisplayFrame {
            id: 1,
            x: 0.0,
            y: 0.0,
            width: 1200.0,
            height: 900.0,
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
        let fixture = HotkiFixture::from_name(name)
            .ok_or_else(|| format!("unknown Hotki fixture: {name}"))?;
        self.apply_fixture(fixture)?;
        self.fixture_runtime.request_repaint();
        Ok(())
    }

    /// Dispatch one typed fixture onto the lane that owns the affected state.
    fn apply_fixture(&self, fixture: HotkiFixture) -> Result<(), String> {
        match fixture {
            HotkiFixture::BasicDefault | HotkiFixture::Details => {
                self.clear_transient_ui()?;
                self.show_details()?;
            }
            HotkiFixture::DetailsConfig => {
                self.clear_transient_ui()?;
                self.show_details_tab(DetailsTab::Config)?;
            }
            HotkiFixture::DetailsLogs => {
                self.clear_transient_ui()?;
                self.show_details_tab(DetailsTab::Logs)?;
            }
            HotkiFixture::DetailsAbout => {
                self.clear_transient_ui()?;
                self.show_details_tab(DetailsTab::About)?;
            }
            HotkiFixture::Permissions => {
                self.clear_transient_ui()?;
                self.send_ui_command(UiCommand::ShowPermissionsHelp)?;
            }
            HotkiFixture::PermissionsAllGranted => {
                self.clear_transient_ui()?;
                self.set_permission_override(true, true)?;
                self.send_ui_command(UiCommand::ShowPermissionsHelp)?;
            }
            HotkiFixture::PermissionsNoneGranted => {
                self.clear_transient_ui()?;
                self.set_permission_override(false, false)?;
                self.send_ui_command(UiCommand::ShowPermissionsHelp)?;
            }
            HotkiFixture::PermissionsMixed => {
                self.clear_transient_ui()?;
                self.set_permission_override(true, false)?;
                self.send_ui_command(UiCommand::ShowPermissionsHelp)?;
            }
            HotkiFixture::Notifications => {
                self.clear_transient_ui()?;
                self.reconfigure_notifications(NotifyPos::Right, DisplaysSnapshot::default())?;
                self.send_ui_message(MsgToUI::Notify {
                    kind: NotifyKind::Info,
                    title: "Eguidev".to_string(),
                    text: "Deterministic notification fixture".to_string(),
                })?;
                self.show_details()?;
            }
            HotkiFixture::NotificationVariants => {
                self.clear_transient_ui()?;
                self.send_notification_variants(NotifyPos::Right)?;
            }
            HotkiFixture::NotificationLeftVariants => {
                self.clear_transient_ui()?;
                self.send_notification_variants(NotifyPos::Left)?;
            }
            HotkiFixture::NotificationTruncated => {
                self.clear_transient_ui()?;
                self.send_truncated_notification()?;
            }
            HotkiFixture::Hud => {
                self.clear_transient_ui()?;
                self.send_control(ControlMsg::Reload)?;
                self.inject_key("cmd+shift+0")?;
            }
            HotkiFixture::HudDemo => {
                self.inject_key("d")?;
            }
            HotkiFixture::HudTall => {
                self.clear_transient_ui()?;
                self.send_tall_hud()?;
            }
            HotkiFixture::HudMini => {
                self.clear_transient_ui()?;
                self.send_fixture_hud(Mode::Mini, "Mini", Vec::new())?;
            }
            HotkiFixture::Selector => {
                self.clear_transient_ui()?;
                self.inject_key("s")?;
            }
            HotkiFixture::SelectorQuery => {
                self.inject_key("b")?;
            }
            HotkiFixture::SelectorConfirmed => {
                self.inject_key("return")?;
                self.show_details()?;
            }
            HotkiFixture::SelectorCanceled => {
                self.inject_key("escape")?;
                self.show_details()?;
            }
        }
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

    /// Render one deterministic HUD variant through the production UI message path.
    fn send_fixture_hud(&self, mode: Mode, title: &str, rows: Vec<HudRow>) -> Result<(), String> {
        let mut style = Style::default();
        style.hud.mode = mode;
        self.send_ui_message(MsgToUI::HudUpdate {
            hud: Box::new(HudState {
                visible: true,
                rows,
                depth: 1,
                breadcrumbs: vec![title.to_string()],
                style,
                capture: false,
            }),
            displays: DisplaysSnapshot::default(),
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

    /// Render a tall HUD through the same UI update path used by the runtime.
    fn send_tall_hud(&self) -> Result<(), String> {
        self.send_ui_message(MsgToUI::HudUpdate {
            hud: Box::new(HudState {
                visible: true,
                rows: tall_hud_rows()?,
                depth: 1,
                breadcrumbs: vec!["Tall HUD".to_string()],
                style: Style::default(),
                capture: false,
            }),
            displays: tall_hud_display_snapshot(),
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

    /// Inject a complete key press through the runtime/server lane.
    fn inject_key_with_reporting(&self, ident: &str, report_errors: bool) -> Result<(), String> {
        self.send_control(ControlMsg::InjectKey {
            ident: ident.to_string(),
            report_errors,
        })
    }

    /// Send a runtime control message.
    fn send_control(&self, message: ControlMsg) -> Result<(), String> {
        self.tx_ctrl
            .send(message)
            .map_err(|err| format!("failed to send runtime control: {err}"))
    }
}

/// Rows used by the tall HUD fixture.
fn tall_hud_rows() -> Result<Vec<HudRow>, String> {
    (0..22)
        .map(|index| {
            Ok(HudRow {
                chord: Chord::parse(tall_hud_chord(index))
                    .ok_or_else(|| format!("invalid tall HUD chord {index}"))?,
                desc: format!("Tall HUD command {index:02} with enough text to measure"),
                is_mode: index % 4 == 0,
                style: None,
            })
        })
        .collect()
}

/// Deterministic chord list for the tall HUD fixture.
fn tall_hud_chord(index: usize) -> &'static str {
    const CHORDS: &[&str] = &[
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r",
        "s", "t", "u", "v",
    ];
    CHORDS[index % CHORDS.len()]
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn fixture_catalog_round_trips_stable_names() {
        let specs = fixtures();
        let expected_names = HOTKI_FIXTURES
            .iter()
            .map(|def| def.name)
            .collect::<Vec<_>>();
        let actual_names = specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(actual_names, expected_names);

        let mut seen = HashSet::new();
        for def in HOTKI_FIXTURES {
            assert!(seen.insert(def.name));
            assert_eq!(HotkiFixture::from_name(def.name), Some(def.fixture));
        }
        assert_eq!(specs.len(), seen.len());
        assert_eq!(HotkiFixture::from_name("hotki.unknown"), None);
    }
}
