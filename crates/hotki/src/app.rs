//! App-level state and event handling for the Hotki UI.
use std::path::PathBuf;

use eframe::{App, Frame};
use egui::Context;
use hotki_protocol::{MsgToUI, Toggle};
use tokio::sync::mpsc as tokio_mpsc;
use tray_icon::TrayIcon;

use crate::{
    details::Details, display::DisplayMetrics, hud::Hud, notification::NotificationCenter,
    permissions::PermissionsHelp, selector::SelectorWindow,
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

impl App for HotkiApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::TRANSPARENT.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &Context, _frame: &mut Frame) {
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            if !self.shutdown_in_progress {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
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
}

impl HotkiApp {
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
