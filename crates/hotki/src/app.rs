//! App-level state and event handling for the Hotki UI.
use std::path::PathBuf;

use eframe::{App, Frame};
use egui::Context;
use hotki_protocol::{DisplaysSnapshot, HudState, NotifyKind, SelectorSnapshot};
use tokio::sync::mpsc as tokio_mpsc;
use tray_icon::TrayIcon;

use crate::{
    details::Details, display::DisplayMetrics, hud::Hud, notification::NotificationCenter,
    permissions::PermissionsHelp, selector::SelectorWindow,
};

/// Events sent from the background runtime to the UI thread.
pub enum AppEvent {
    /// Request a graceful shutdown of all UI and background tasks.
    Shutdown,
    /// Update the config path displayed in Details.
    SetConfigPath(Option<PathBuf>),
    /// Show the Details window.
    ShowDetails,
    /// Hide the Details window.
    HideDetails,
    /// Show the permissions helper window.
    ShowPermissionsHelp,
    /// Replace HUD state from the server.
    HudUpdate {
        /// Fully rendered HUD snapshot.
        hud: Box<HudState>,
        /// Display geometry snapshot for HUD/notification placement.
        displays: DisplaysSnapshot,
    },
    /// Show/update selector popup.
    SelectorUpdate {
        /// Fully rendered selector snapshot.
        selector: Box<SelectorSnapshot>,
    },
    /// Hide selector popup.
    SelectorHide,
    /// Show an in-app notification.
    Notify {
        /// Notification kind (info, warn, error, success).
        kind: NotifyKind,
        /// Title of the notification.
        title: String,
        /// Body text of the notification.
        text: String,
    },
    /// Clear all on-screen notifications.
    ClearNotifications,
    /// Toggle the Details window.
    ToggleDetails,
}

/// Top-level UI application state and channels.
pub struct HotkiApp {
    /// Receiver for events from the runtime thread.
    pub(crate) rx: tokio_mpsc::UnboundedReceiver<AppEvent>,
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
            // Always request hide; optionally cancel the close below.
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            if !self.shutdown_in_progress {
                // Default behavior: hide instead of closing the app window.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }

        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                AppEvent::Shutdown => {
                    self.shutdown_in_progress = true;
                    // Hide all viewports and remove tray icon to allow a graceful exit
                    self.hud.hide(ctx);
                    self.selector.hide(ctx);
                    // Clear notifications and hide their windows
                    self.notifications.clear_all(ctx);
                    // Hide Details and Permissions windows
                    self.details.hide();
                    self.permissions.hide();
                    // Drop tray icon
                    self._tray = None;
                    // Ask the native window to close; runtime has a fallback timer.
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    ctx.request_repaint();
                }
                AppEvent::SetConfigPath(path) => {
                    self.details.set_config_path(path);
                    ctx.request_repaint();
                }
                AppEvent::ShowDetails => {
                    self.details.show();
                    ctx.request_repaint();
                }
                AppEvent::HideDetails => {
                    self.details.hide();
                    ctx.request_repaint();
                }
                AppEvent::ShowPermissionsHelp => {
                    self.permissions.show();
                    ctx.request_repaint();
                }
                AppEvent::HudUpdate { hud, displays } => {
                    self.display_metrics = DisplayMetrics::from_snapshot(&displays);
                    self.sync_display_metrics();

                    self.hud.set_style(hud.style.hud.clone());
                    self.hud
                        .set_state(hud.rows.clone(), hud.visible, hud.breadcrumbs.clone());

                    self.selector.set_style(hud.style.selector.clone());

                    self.notifications.reconfigure(&hud.style.notify);
                    self.details.update_theme(hud.style.notify.theme.clone());

                    ctx.request_repaint();
                }
                AppEvent::SelectorUpdate { selector } => {
                    self.selector.set_state(*selector);
                    ctx.request_repaint();
                }
                AppEvent::SelectorHide => {
                    self.selector.hide(ctx);
                    ctx.request_repaint();
                }
                AppEvent::Notify { kind, title, text } => {
                    self.notifications.push(kind, title, text);
                    ctx.request_repaint();
                }
                AppEvent::ClearNotifications => {
                    self.notifications.clear_all(ctx);
                    ctx.request_repaint();
                }
                AppEvent::ToggleDetails => {
                    self.details.toggle();
                    ctx.request_repaint();
                }
            }
        }

        // Always render HUD and notifications
        self.hud.render(ctx);
        self.selector.render(ctx);
        self.notifications.render(ctx);
        self.details.render(ctx, self.notifications.backlog());
        self.permissions.render(ctx);
    }
}

impl HotkiApp {
    /// Propagate the cached display metrics to HUD, notifications, and details.
    fn sync_display_metrics(&mut self) {
        let metrics = self.display_metrics.clone();
        self.hud.set_display_metrics(metrics.clone());
        self.selector.set_display_metrics(metrics.clone());
        self.notifications.set_display_metrics(metrics.clone());
        self.details.set_display_metrics(metrics);
    }
}
