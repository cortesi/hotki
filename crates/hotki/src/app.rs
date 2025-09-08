use tokio::sync::mpsc as tokio_mpsc;

use eframe::{App, Frame};
use egui::Context;
use tray_icon::TrayIcon;

use crate::{details::Details, hud::Hud, notification::NotificationCenter};

use config::Config;
use hotki_protocol::NotifyKind;

// Control messages moved to control.rs

pub enum AppEvent {
    Quit,
    Shutdown,
    ShowDetails,
    HideDetails,
    ShowPermissionsHelp,
    KeyUpdate {
        visible_keys: Vec<(String, String, bool)>,
        depth: usize,
        cursor: config::Cursor,
        parent_title: Option<String>,
    },
    Notify {
        kind: NotifyKind,
        title: String,
        text: String,
    },
    ClearNotifications,
    ToggleDetails,
    ReloadUI(Box<Config>),
    /// Update the current UI Location (used for theme/user-style flags now stored on Location)
    UpdateCursor(config::Cursor),
    // (no backend operations here)
}

pub struct HotkiApp {
    pub(crate) rx: tokio_mpsc::UnboundedReceiver<AppEvent>,
    pub(crate) _tray: Option<TrayIcon>,
    pub(crate) hud: Hud,
    pub(crate) notifications: NotificationCenter,
    pub(crate) details: Details,
    pub(crate) permissions: crate::permissions::PermissionsHelp,
    pub(crate) config: config::Config,
    pub(crate) last_cursor: config::Cursor,
}

impl App for HotkiApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::TRANSPARENT.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &Context, _frame: &mut Frame) {
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }

        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                AppEvent::Quit => {
                    std::process::exit(0);
                }
                AppEvent::Shutdown => {
                    // Hide all viewports and remove tray icon to allow a graceful exit
                    let (keys, _visible, parent_title) = self.hud.get_state();
                    // Ensure HUD viewport is hidden and stop rendering
                    self.hud.hide(ctx);
                    // Clear notifications and hide their windows
                    self.notifications.clear_all(ctx);
                    // Hide Details and Permissions windows
                    self.details.hide();
                    self.permissions.hide();
                    // Drop tray icon
                    self._tray = None;
                    // Preserve last cursor and config, but request a repaint to flush hides
                    // Optionally keep keys invisible
                    self.hud.set_keys(keys, false, parent_title);
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
                AppEvent::KeyUpdate {
                    visible_keys,
                    depth,
                    cursor,
                    parent_title,
                } => {
                    // Compute effective theme using config and location
                    let hud_theme = self.config.hud(&cursor);
                    let notify_cfg = self.config.notify_config(&cursor);
                    // Apply HUD theme and set new keys
                    self.hud = Hud::new(&hud_theme);
                    // Determine HUD visibility using current cursor and HUD mode
                    let visible = match hud_theme.mode {
                        config::Mode::Hide => false,
                        // Show in full HUD mode when root-view is active or depth>0
                        config::Mode::Hud => self.config.hud_visible(&cursor),
                        // Mini HUD remains visible only when inside a submode with a parent title
                        config::Mode::Mini => depth > 0 && parent_title.as_ref().is_some(),
                    };
                    self.hud.set_keys(visible_keys, visible, parent_title);
                    self.notifications.reconfigure(&notify_cfg);
                    self.details.update_theme(notify_cfg.theme());
                    // Remember cursor so theme switches reapply immediately
                    self.last_cursor = cursor;
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
                AppEvent::ReloadUI(cfg) => {
                    // Preserve HUD state before recreating
                    let (current_keys, current_visible, current_parent_title) =
                        self.hud.get_state();
                    // Replace config
                    self.config = *cfg;
                    // Compute effective theme at last location
                    let hud_theme = self.config.hud(&self.last_cursor);
                    let notify_cfg = self.config.notify_config(&self.last_cursor);
                    // Recreate HUD/notify with effective theme
                    self.hud = Hud::new(&hud_theme);
                    self.hud
                        .set_keys(current_keys, current_visible, current_parent_title);
                    self.notifications.reconfigure(&notify_cfg);
                    self.details.update_theme(notify_cfg.theme());
                    self.details.reload_config_contents();
                }
                AppEvent::UpdateCursor(loc) => {
                    // Preserve HUD state, then update theme/notify based on new location
                    let (current_keys, current_visible, current_parent_title) =
                        self.hud.get_state();
                    self.last_cursor = loc;
                    let hud_theme = self.config.hud(&self.last_cursor);
                    let notify_cfg = self.config.notify_config(&self.last_cursor);
                    self.hud = Hud::new(&hud_theme);
                    self.hud
                        .set_keys(current_keys, current_visible, current_parent_title);
                    self.notifications.reconfigure(&notify_cfg);
                    self.details.update_theme(notify_cfg.theme());
                }
            }
        }

        // Always render HUD and notifications
        self.hud.render(ctx);
        self.notifications.render(ctx);
        self.details.render(ctx, self.notifications.backlog());
        self.permissions.render(ctx);
    }
}
