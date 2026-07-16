//! App-level state and event handling for the Hotki UI.
use std::{
    mem,
    path::PathBuf,
    sync::mpsc::{Receiver, TryRecvError},
    time::Instant,
};

use eframe::{App, CreationContext};
use egui::Context;
use eguidev::DevMcp;
use hotki_protocol::{DisplaysSnapshot, HudState, MsgToUI, NotifyConfig, Style, Toggle};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::MainThreadMarker;
use tokio::sync::mpsc as tokio_mpsc;

use crate::{
    details::{Details, DetailsTab},
    devtools::{FixtureRuntime, PreparedDevMcp, render_app_anchors},
    display::DisplayMetrics,
    fonts,
    harness_control::{PresentationExpectation, PresentationRequest},
    health::RuntimeHealth,
    hud::Hud,
    notification::NotificationCenter,
    permissions::{PermissionsHelp, PermissionsStatus},
    runtime::{self, ControlMsg},
    selector::SelectorWindow,
    tray,
    ui_delivery::{UiDeliveryRx, UiDeliveryTx},
};

/// Commands local to the UI process that are not part of the protocol stream.
pub enum UiCommand {
    /// Request a graceful shutdown of all UI and background tasks.
    Shutdown,
    /// Show the permissions helper window.
    ShowPermissionsHelp,
    /// Hide the permissions helper window.
    HidePermissionsHelp,
    /// Show the Details window with a specific tab selected.
    ShowDetailsTab(DetailsTab),
    /// Replace the complete UI-visible runtime health snapshot.
    SetRuntimeHealth(RuntimeHealth),
    /// Update the server binding identifiers visible to devtools.
    SetServerBindings(Vec<String>),
    /// Override permission status for deterministic devtools fixtures.
    SetPermissionStatusOverride(Option<PermissionsStatus>),
    /// Override notification presentation for deterministic devtools fixtures.
    SetNotificationPresentationOverride(Option<Box<NotificationPresentation>>),
    /// Override HUD presentation for deterministic devtools fixtures.
    SetHudPresentationOverride(Option<Box<HudPresentation>>),
}

/// Notification presentation fixed by one deterministic devtools fixture.
pub struct NotificationPresentation {
    /// Notification configuration applied to fixture items.
    pub(crate) config: NotifyConfig,
    /// Synthetic display state used to measure and place fixture items.
    pub(crate) displays: DisplaysSnapshot,
}

/// HUD presentation fixed by one deterministic devtools fixture.
pub struct HudPresentation {
    /// Complete HUD state applied by the fixture.
    pub(crate) hud: Box<HudState>,
    /// Synthetic display state used to place the fixture HUD.
    pub(crate) displays: DisplaysSnapshot,
}

/// Events sent from the background runtime to the UI thread.
pub enum UiEvent {
    /// Protocol message from the server/runtime path.
    Message(MsgToUI),
    /// Local UI-only command.
    Command(UiCommand),
}

/// Top-level UI application state and channels.
pub struct HotkiApp {
    /// Receiver for events from the runtime thread.
    pub(crate) rx: UiDeliveryRx,
    /// Tray icon and its live health rows.
    pub(crate) tray: Option<tray::Tray>,
    /// Heads-up display for key hints.
    pub(crate) hud: Hud,
    /// Interactive selector popup.
    pub(crate) selector: SelectorWindow,
    /// In-app notifications manager.
    pub(crate) notifications: NotificationCenter,
    /// Details window state.
    pub(crate) details: Details,
    /// Permissions helper window state.
    pub(crate) permissions: PermissionsHelp,
    /// True when a graceful shutdown is in progress; allows window close.
    pub(crate) shutdown_in_progress: bool,
    /// Cached display metrics for HUD/notification placement.
    pub(crate) display_metrics: DisplayMetrics,
    /// Latest production notification configuration from the runtime.
    pub(crate) runtime_notify_config: NotifyConfig,
    /// Whether a devtools fixture owns notification presentation.
    pub(crate) notification_presentation_overridden: bool,
    /// Latest complete production HUD state from the runtime.
    pub(crate) runtime_hud_state: Option<Box<HudState>>,
    /// Whether a devtools fixture owns HUD presentation.
    pub(crate) hud_presentation_overridden: bool,
    /// Developer automation instrumentation handle.
    pub(crate) devmcp: DevMcp,
    /// Shared fixture and diagnostic state for developer automation.
    pub(crate) fixture_runtime: FixtureRuntime,
    /// Complete health state shared by every normal UI surface.
    pub(crate) runtime_health: RuntimeHealth,
    /// Sorted server binding identifiers, when the runtime has reported them.
    pub(crate) server_bindings: Vec<String>,
    /// App-local requests waiting for a specific UI state to be painted.
    pub(crate) harness_requests: Option<Receiver<PresentationRequest>>,
    /// Presentation requests that have not yet survived one complete rendered frame.
    pending_presentations: Vec<PendingPresentation>,
    /// Monotonic UI frame counter used by presentation barriers.
    rendered_frame: u64,
}

/// One presentation request being evaluated on the UI thread.
struct PendingPresentation {
    /// Request and acknowledgement sender supplied by the control listener.
    request: PresentationRequest,
    /// First rendered frame where the expected state matched.
    matched_frame: Option<u64>,
}

/// Inputs required to bootstrap the UI application and runtime.
pub struct AppBootstrap {
    /// Receiver for background runtime events.
    pub rx: UiDeliveryRx,
    /// Sender for local UI events.
    pub tx_ui: UiDeliveryTx,
    /// Sender for runtime control messages.
    pub tx_ctrl: tokio_mpsc::UnboundedSender<ControlMsg>,
    /// Receiver for runtime control messages.
    pub rx_ctrl: tokio_mpsc::UnboundedReceiver<ControlMsg>,
    /// Active config path to send to details and runtime.
    pub config_path: PathBuf,
    /// Initial style used before the first server HUD update.
    pub initial_style: Style,
    /// Optional log filter to propagate to an auto-spawned server.
    pub server_log_filter: Option<String>,
    /// Whether the auto-spawned server should observe physical keyboard events.
    pub server_event_tap_enabled: bool,
    /// Whether world snapshots should be periodically dumped to logs.
    pub dumpworld: bool,
    /// Developer automation instrumentation awaiting UI-thread attachment.
    pub prepared_devmcp: PreparedDevMcp,
    /// Fixture runtime state shared with the early MCP handler.
    pub fixture_runtime: FixtureRuntime,
    /// Optional local harness presentation request channel.
    pub harness_requests: Option<Receiver<PresentationRequest>>,
}

impl App for HotkiApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::TRANSPARENT.to_normalized_gamma_f32()
    }

    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.shutdown_in_progress {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }

        while let Some(event) = self.rx.try_recv() {
            match event {
                UiEvent::Command(command) => self.handle_command(ctx, command),
                UiEvent::Message(message) => self.handle_message(ctx, message),
            }
        }
        self.receive_presentation_requests();

        let notification_stack = self.notifications.stack_aliases();
        self.fixture_runtime.update_diagnostics(
            &self.runtime_health,
            &self.server_bindings,
            &notification_stack,
            self.rx.stats(),
        );
        render_app_anchors(
            &self.devmcp,
            ctx,
            &self.runtime_health,
            &self.server_bindings,
            &notification_stack,
        );
        self.hud.render(ctx, &self.devmcp);
        self.selector.render(ctx, &self.devmcp);
        let notifications_animating = self.notifications.render(ctx, &self.devmcp);
        self.fixture_runtime.set_app_idle(!notifications_animating);
        self.details
            .render(ctx, self.notifications.backlog(), &self.devmcp);
        self.permissions.render(ctx, &self.devmcp);
        self.acknowledge_presentations(ctx);
    }

    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
}

impl HotkiApp {
    /// Construct the full UI app, spawn the runtime thread, and wire the tray.
    pub fn new(cc: &CreationContext<'_>, bootstrap: AppBootstrap) -> Self {
        if !bootstrap.prepared_devmcp.automation_owns_presentation()
            && let Some(mtm) = MainThreadMarker::new()
        {
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        }

        // Disable the default Cmd+Q quit shortcut so it doesn't intercept
        // before hotki's own hotkey system can process the key.
        cc.egui_ctx.options_mut(|opts| opts.quit_shortcuts = vec![]);

        fonts::install_fonts(&cc.egui_ctx);

        let fixture_runtime = bootstrap.fixture_runtime.clone();
        fixture_runtime.set_context(cc.egui_ctx.clone());

        runtime::spawn_key_runtime(
            bootstrap.config_path.as_path(),
            &bootstrap.tx_ui,
            &cc.egui_ctx,
            bootstrap.rx_ctrl,
            bootstrap.server_log_filter,
            bootstrap.server_event_tap_enabled,
            bootstrap.dumpworld,
        );

        let runtime_health = RuntimeHealth::connecting(bootstrap.config_path.clone());
        let tray = tray::build_tray_and_listeners(
            &bootstrap.tx_ui,
            &bootstrap.tx_ctrl,
            &cc.egui_ctx,
            &runtime_health,
        );

        let runtime_notify_config = bootstrap.initial_style.notify.clone();
        let mut notifications = NotificationCenter::new(&runtime_notify_config);
        let mut details = Details::new(bootstrap.initial_style.notify.theme.clone());
        details.set_runtime_health(runtime_health.clone());
        details.set_control_sender(bootstrap.tx_ctrl.clone());

        let mut permissions = PermissionsHelp::new();
        permissions.set_control_sender(bootstrap.tx_ctrl.clone());

        let metrics = DisplayMetrics::default();
        let mut hud = Hud::new(&bootstrap.initial_style.hud);
        let mut selector = SelectorWindow::new(&bootstrap.initial_style.selector);
        hud.set_display_metrics(metrics.clone());
        selector.set_display_metrics(metrics.clone());
        notifications.set_display_metrics(metrics.clone());
        details.set_display_metrics(metrics.clone());

        let devmcp = bootstrap.prepared_devmcp.attach();

        Self {
            rx: bootstrap.rx,
            tray,
            hud,
            selector,
            notifications,
            details,
            permissions,
            shutdown_in_progress: false,
            display_metrics: metrics,
            runtime_notify_config,
            notification_presentation_overridden: false,
            runtime_hud_state: None,
            hud_presentation_overridden: false,
            devmcp,
            fixture_runtime,
            runtime_health,
            server_bindings: Vec::new(),
            harness_requests: bootstrap.harness_requests,
            pending_presentations: Vec::new(),
            rendered_frame: 0,
        }
    }

    /// Drain newly submitted harness barriers without blocking the UI thread.
    fn receive_presentation_requests(&mut self) {
        let Some(requests) = self.harness_requests.as_ref() else {
            return;
        };
        loop {
            match requests.try_recv() {
                Ok(request) => self.pending_presentations.push(PendingPresentation {
                    request,
                    matched_frame: None,
                }),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.harness_requests = None;
                    break;
                }
            }
        }
    }

    /// Acknowledge barriers only after matching state has completed a prior rendered frame.
    fn acknowledge_presentations(&mut self, ctx: &Context) {
        self.rendered_frame = self.rendered_frame.saturating_add(1);
        let frame = self.rendered_frame;
        let mut pending = Vec::new();
        for mut barrier in mem::take(&mut self.pending_presentations) {
            if !self.presentation_matches(&barrier.request.expectation) {
                barrier.matched_frame = None;
                pending.push(barrier);
                continue;
            }
            if barrier
                .matched_frame
                .is_some_and(|matched_frame| matched_frame < frame)
            {
                if let Err(error) = barrier.request.rendered.send(()) {
                    tracing::debug!(?error, "presentation barrier client disconnected");
                }
                continue;
            }
            barrier.matched_frame = Some(frame);
            pending.push(barrier);
            ctx.request_repaint();
        }
        self.pending_presentations = pending;
    }

    /// Test one harness expectation against current UI-thread presentation state.
    fn presentation_matches(&self, expectation: &PresentationExpectation) -> bool {
        match expectation {
            PresentationExpectation::Hud => self.hud.is_visible(),
            PresentationExpectation::Selector(query) => self.selector.query() == Some(query),
            PresentationExpectation::Notification(kind) => self.notifications.contains_kind(*kind),
        }
    }

    /// Apply a local UI-only command emitted by the runtime or control layer.
    fn handle_command(&mut self, ctx: &Context, command: UiCommand) {
        match command {
            UiCommand::Shutdown => {
                self.shutdown_in_progress = true;
                self.hud.hide(ctx);
                self.selector.hide(ctx);
                self.notifications.clear_all(ctx, &self.devmcp);
                self.details.hide();
                self.permissions.hide();
                self.tray = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            UiCommand::ShowPermissionsHelp => {
                self.permissions.show();
            }
            UiCommand::HidePermissionsHelp => {
                self.permissions.hide();
            }
            UiCommand::ShowDetailsTab(tab) => {
                self.details.show_tab(tab);
            }
            UiCommand::SetRuntimeHealth(health) => {
                self.runtime_health = health;
                self.apply_runtime_health();
            }
            UiCommand::SetServerBindings(bindings) => {
                self.server_bindings = bindings;
            }
            UiCommand::SetPermissionStatusOverride(status) => {
                self.permissions.set_status_override(status);
            }
            UiCommand::SetNotificationPresentationOverride(presentation) => {
                self.set_notification_presentation_override(presentation);
            }
            UiCommand::SetHudPresentationOverride(presentation) => {
                self.set_hud_presentation_override(presentation);
            }
        }
        ctx.request_repaint();
    }

    /// Apply a protocol message from the server to the local UI state.
    fn handle_message(&mut self, ctx: &Context, message: MsgToUI) {
        match message {
            MsgToUI::HudUpdate { hud, displays } => {
                self.display_metrics = DisplayMetrics::from_snapshot(&displays);
                self.sync_display_metrics();

                self.selector.set_style(hud.style.selector.clone());

                self.runtime_notify_config = hud.style.notify.clone();
                if !self.notification_presentation_overridden {
                    self.notifications.reconfigure(&self.runtime_notify_config);
                }
                self.details.update_theme(hud.style.notify.theme.clone());
                self.runtime_hud_state = Some(hud);
                if !self.hud_presentation_overridden {
                    self.apply_runtime_hud_state();
                }
            }
            MsgToUI::HudKeyState { chord, pressed } => {
                self.hud.set_key_state(&chord, pressed, Instant::now());
            }
            MsgToUI::SelectorUpdate(selector) => {
                self.selector.set_state(selector);
            }
            MsgToUI::SelectorHide => {
                self.selector.hide(ctx);
            }
            MsgToUI::Notify { kind, title, text } => {
                self.notifications.push(kind, title, text);
            }
            MsgToUI::ClearNotifications => {
                self.notifications.clear_all(ctx, &self.devmcp);
            }
            MsgToUI::ShowDetails(toggle) => match toggle {
                Toggle::On => self.details.show(),
                Toggle::Off => self.details.hide(),
                Toggle::Toggle => self.details.toggle(),
            },
            MsgToUI::Log { .. } | MsgToUI::Heartbeat(_) | MsgToUI::World(_) => {}
        }
        ctx.request_repaint();
    }

    /// Propagate the cached display metrics to HUD, notifications, and details.
    fn sync_display_metrics(&mut self) {
        let metrics = self.display_metrics.clone();
        if !self.hud_presentation_overridden {
            self.hud.set_display_metrics(metrics.clone());
        }
        self.selector.set_display_metrics(metrics.clone());
        if !self.notification_presentation_overridden {
            self.notifications.set_display_metrics(metrics.clone());
        }
        self.details.set_display_metrics(metrics);
    }

    /// Apply or clear deterministic notification presentation owned by devtools.
    fn set_notification_presentation_override(
        &mut self,
        presentation: Option<Box<NotificationPresentation>>,
    ) {
        if let Some(presentation) = presentation {
            self.notification_presentation_overridden = true;
            self.notifications.reconfigure(&presentation.config);
            self.notifications
                .set_display_metrics(DisplayMetrics::from_snapshot(&presentation.displays));
        } else {
            self.notification_presentation_overridden = false;
            self.notifications.reconfigure(&self.runtime_notify_config);
            self.notifications
                .set_display_metrics(self.display_metrics.clone());
        }
    }

    /// Apply or clear deterministic HUD presentation owned by devtools.
    fn set_hud_presentation_override(&mut self, presentation: Option<Box<HudPresentation>>) {
        self.hud.clear_key_state();
        if let Some(presentation) = presentation {
            self.hud_presentation_overridden = true;
            self.hud.set_style(presentation.hud.style.hud.clone());
            self.hud.set_state(
                presentation.hud.rows.clone(),
                presentation.hud.visible,
                presentation.hud.breadcrumbs.clone(),
            );
            self.hud
                .set_display_metrics(DisplayMetrics::from_snapshot(&presentation.displays));
        } else {
            self.hud_presentation_overridden = false;
            self.hud.set_display_metrics(self.display_metrics.clone());
            self.apply_runtime_hud_state();
        }
    }

    /// Restore the latest production HUD state after fixture ownership ends.
    fn apply_runtime_hud_state(&mut self) {
        let Some(hud) = self.runtime_hud_state.as_ref() else {
            return;
        };
        self.hud.set_style(hud.style.hud.clone());
        self.hud
            .set_state(hud.rows.clone(), hud.visible, hud.breadcrumbs.clone());
    }

    /// Propagate one runtime-health snapshot to every normal UI surface.
    fn apply_runtime_health(&mut self) {
        if !self.runtime_health.server_connected() {
            self.hud.clear_key_state();
        }
        self.details.set_runtime_health(self.runtime_health.clone());
        if let Some(tray) = self.tray.as_ref() {
            tray.set_runtime_health(&self.runtime_health);
        }
    }
}
