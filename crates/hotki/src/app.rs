use tokio::sync::mpsc as tokio_mpsc;

use eframe::{App, Frame};
use egui::Context;
use tray_icon::TrayIcon;

use crate::{details::Details, hud::Hud, notification::NotificationCenter};

use config::Config;
use hotki_protocol::NotifyKind;

pub enum AppEvent {
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
                    self.apply_effective_theme(
                        &cursor,
                        KeysState::FromUpdate {
                            keys: visible_keys,
                            depth,
                            parent_title,
                        },
                    );
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
                    // Replace config and reapply theme at last known cursor while preserving HUD keys
                    self.config = *cfg;
                    let cur = self.last_cursor.clone();
                    self.apply_effective_theme(&cur, KeysState::PreserveCurrent);
                    self.details.reload_config_contents();
                }
                AppEvent::UpdateCursor(loc) => {
                    // Preserve HUD state, then update theme/notify based on new location
                    self.last_cursor = loc;
                    let cur = self.last_cursor.clone();
                    self.apply_effective_theme(&cur, KeysState::PreserveCurrent);
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

enum KeysState {
    FromUpdate {
        keys: Vec<(String, String, bool)>,
        depth: usize,
        parent_title: Option<String>,
    },
    PreserveCurrent,
}

impl HotkiApp {
    /// Compute and apply the effective UI theme (HUD + notifications + details) for a cursor.
    /// When `keys_state` is `Some`, it sets those keys/visibility/parent title on the HUD;
    /// when `None`, it preserves existing HUD keys state.
    fn apply_effective_theme(&mut self, cursor: &config::Cursor, keys_state: KeysState) {
        let hud_theme = self.config.hud(cursor);
        let notify_cfg = self.config.notify_config(cursor);
        // Preserve prior keys/visibility before replacing HUD
        let (cur_keys, cur_visible, cur_parent_title) = self.hud.get_state();
        // Recreate HUD with new theme
        self.hud = Hud::new(&hud_theme);
        match keys_state {
            KeysState::FromUpdate {
                keys,
                depth,
                parent_title,
            } => {
                // Derive visibility from depth and parent title for Mini/Hud modes
                let visible = match hud_theme.mode {
                    config::Mode::Hide => false,
                    config::Mode::Hud => self.config.hud_visible(cursor),
                    config::Mode::Mini => depth > 0 && parent_title.as_ref().is_some(),
                };
                self.hud.set_keys(keys, visible, parent_title);
            }
            KeysState::PreserveCurrent => {
                self.hud.set_keys(cur_keys, cur_visible, cur_parent_title);
            }
        }
        self.notifications.reconfigure(&notify_cfg);
        self.details.update_theme(notify_cfg.theme());
    }
}
