//! App-level state and event handling for the Hotki UI.
use std::path::PathBuf;

use eframe::{App, CreationContext};
use egui::Context;
use hotki_protocol::{MsgToUI, Style, Toggle};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::MainThreadMarker;
use tokio::sync::mpsc as tokio_mpsc;
use tray_icon::TrayIcon;

use crate::{
    details::Details,
    display::DisplayMetrics,
    fonts,
    hud::Hud,
    notification::NotificationCenter,
    permissions::PermissionsHelp,
    runtime::{self, ControlMsg},
    selector::SelectorWindow,
    tray,
};

/// Commands local to the UI process that are not part of the protocol stream.
pub enum UiCommand {
    /// Request a graceful shutdown of all UI and background tasks.
    Shutdown,
    /// Update the config path displayed in Details.
    SetConfigPath(Option<PathBuf>),
    /// Show the permissions helper window.
    ShowPermissionsHelp,
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
    pub(crate) rx: tokio_mpsc::UnboundedReceiver<UiEvent>,
    /// Tray icon handle, kept to maintain tray lifetime.
    pub(crate) _tray: Option<TrayIcon>,
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
}

/// Inputs required to bootstrap the UI application and runtime.
pub struct AppBootstrap {
    /// Receiver for background runtime events.
    pub rx: tokio_mpsc::UnboundedReceiver<UiEvent>,
    /// Sender for local UI events.
    pub tx_ui: tokio_mpsc::UnboundedSender<UiEvent>,
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
    /// Whether world snapshots should be periodically dumped to logs.
    pub dumpworld: bool,
}

impl App for HotkiApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::TRANSPARENT.to_normalized_gamma_f32()
    }

    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.shutdown_in_progress {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }

        while let Ok(event) = self.rx.try_recv() {
            match event {
                UiEvent::Command(command) => self.handle_command(ctx, command),
                UiEvent::Message(message) => self.handle_message(ctx, message),
            }
        }

        self.hud.render(ctx);
        self.selector.render(ctx);
        self.notifications.render(ctx);
        self.details.render(ctx, self.notifications.backlog());
        self.permissions.render(ctx);
    }

    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
}

impl HotkiApp {
    /// Construct the full UI app, spawn the runtime thread, and wire the tray.
    pub fn new(cc: &CreationContext<'_>, bootstrap: AppBootstrap) -> Self {
        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        }

        // Disable the default Cmd+Q quit shortcut so it doesn't intercept
        // before hotki's own hotkey system can process the key.
        cc.egui_ctx.options_mut(|opts| opts.quit_shortcuts = vec![]);

        fonts::install_fonts(&cc.egui_ctx);

        runtime::spawn_key_runtime(
            bootstrap.config_path.as_path(),
            &bootstrap.tx_ui,
            &cc.egui_ctx,
            &bootstrap.tx_ctrl,
            bootstrap.rx_ctrl,
            bootstrap.server_log_filter,
            bootstrap.dumpworld,
        );

        let tray_icon =
            tray::build_tray_and_listeners(&bootstrap.tx_ui, &bootstrap.tx_ctrl, &cc.egui_ctx);

        let mut notifications = NotificationCenter::new(&bootstrap.initial_style.notify);
        let mut details = Details::new(bootstrap.initial_style.notify.theme.clone());
        details.set_config_path(Some(bootstrap.config_path.clone()));
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

        Self {
            rx: bootstrap.rx,
            _tray: tray_icon,
            hud,
            selector,
            notifications,
            details,
            permissions,
            shutdown_in_progress: false,
            display_metrics: metrics,
        }
    }

    /// Apply a local UI-only command emitted by the runtime or control layer.
    fn handle_command(&mut self, ctx: &Context, command: UiCommand) {
        match command {
            UiCommand::Shutdown => {
                self.shutdown_in_progress = true;
                self.hud.hide(ctx);
                self.selector.hide(ctx);
                self.notifications.clear_all(ctx);
                self.details.hide();
                self.permissions.hide();
                self._tray = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            UiCommand::SetConfigPath(path) => {
                self.details.set_config_path(path);
            }
            UiCommand::ShowPermissionsHelp => {
                self.permissions.show();
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

                self.hud.set_style(hud.style.hud.clone());
                self.hud
                    .set_state(hud.rows.clone(), hud.visible, hud.breadcrumbs.clone());

                self.selector.set_style(hud.style.selector.clone());

                self.notifications.reconfigure(&hud.style.notify);
                self.details.update_theme(hud.style.notify.theme.clone());
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
                self.notifications.clear_all(ctx);
            }
            MsgToUI::ShowDetails(toggle) => match toggle {
                Toggle::On => self.details.show(),
                Toggle::Off => self.details.hide(),
                Toggle::Toggle => self.details.toggle(),
            },
            MsgToUI::HotkeyTriggered(_)
            | MsgToUI::Log { .. }
            | MsgToUI::Heartbeat(_)
            | MsgToUI::World(_) => {}
        }
        ctx.request_repaint();
    }

    /// Propagate the cached display metrics to HUD, notifications, and details.
    fn sync_display_metrics(&mut self) {
        let metrics = self.display_metrics.clone();
        self.hud.set_display_metrics(metrics.clone());
        self.selector.set_display_metrics(metrics.clone());
        self.notifications.set_display_metrics(metrics.clone());
        self.details.set_display_metrics(metrics);
    }
}
