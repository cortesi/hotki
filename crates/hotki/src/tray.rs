//! System tray icon and event wiring for the Hotki UI.
use std::thread;

use config::themes;
use egui::Context;
use tokio::sync::mpsc as tokio_mpsc;
use tray_icon::{
    Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, Submenu},
};

use crate::{app::AppEvent, runtime::ControlMsg};

/// Embed tray icon PNG: orange for dev builds, white for production.
static TRAY_ICON_PNG: &[u8] = if cfg!(debug_assertions) {
    include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/tray-icon-dev.png"
    ))
} else {
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/tray-icon.png"))
};

/// Decode the embedded tray icon image into a `tray_icon::Icon`.
fn tray_icon_image() -> Option<Icon> {
    match image::load_from_memory(TRAY_ICON_PNG) {
        Ok(im) => {
            let rgba = im.to_rgba8();
            let (w, h) = rgba.dimensions();
            Icon::from_rgba(rgba.to_vec(), w, h).ok()
        }
        Err(_) => None,
    }
}

/// Build the tray icon and spawn listeners for tray and menu events.
pub fn build_tray_and_listeners(
    tx: &tokio_mpsc::UnboundedSender<AppEvent>,
    tx_ctrl: &tokio_mpsc::UnboundedSender<ControlMsg>,
    egui_ctx: &Context,
) -> Option<TrayIcon> {
    let (menu, reload_id, help_id, quit_id, theme_ids) = {
        let menu = Menu::new();
        let reload = MenuItem::new("Reload Config", true, None);
        let help = MenuItem::new("Permissions Help", true, None);

        // Create themes submenu
        let themes_menu = Submenu::new("Themes", true);
        let theme_list = themes::list_themes();
        let mut theme_menu_ids = Vec::new();

        for theme_name in &theme_list {
            let theme_item = MenuItem::new(*theme_name, true, None);
            theme_menu_ids.push((theme_item.id().clone(), theme_name.to_string()));
            if let Err(e) = themes_menu.append(&theme_item) {
                tracing::warn!("failed to append theme item: {}", e);
            }
        }

        let quit = MenuItem::new("Quit", true, None);
        if let Err(e) = menu.append(&reload) {
            tracing::warn!("failed to append reload menu item: {}", e);
        }
        if let Err(e) = menu.append(&themes_menu) {
            tracing::warn!("failed to append themes submenu: {}", e);
        }
        if let Err(e) = menu.append(&help) {
            tracing::warn!("failed to append help menu item: {}", e);
        }
        if let Err(e) = menu.append(&quit) {
            tracing::warn!("failed to append quit menu item: {}", e);
        }
        (
            menu,
            reload.id().clone(),
            help.id().clone(),
            quit.id().clone(),
            theme_menu_ids,
        )
    };

    let tray_icon_opt: Option<TrayIcon> = {
        let mut builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false);
        if let Some(icon) = tray_icon_image() {
            builder = builder.with_icon(icon);
            // Use template rendering for release builds so macOS tints the icon
            // appropriately for Light/Dark modes. Keep dev icon colored.
            builder = builder.with_icon_as_template(!cfg!(debug_assertions));
        }
        match builder.with_tooltip("hotki").build() {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::error!("Failed to create tray icon: {}", e);
                None
            }
        }
    };

    if tray_icon_opt.is_some() {
        let tx = tx.clone();
        let egui_ctx = egui_ctx.clone();
        thread::spawn(move || {
            let rx_tray = TrayIconEvent::receiver();
            while let Ok(ev) = rx_tray.recv() {
                if matches!(
                    ev,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        ..
                    } | TrayIconEvent::DoubleClick { .. }
                ) {
                    if tx.send(AppEvent::ShowDetails).is_err() {
                        tracing::warn!("failed to send ShowDetails event: UI channel closed");
                    }
                    egui_ctx.request_repaint();
                }
            }
        });
    }

    if tray_icon_opt.is_some() {
        let tx_ctrl = tx_ctrl.clone();
        let egui_ctx = egui_ctx.clone();
        thread::spawn(move || {
            let menu_rx = MenuEvent::receiver();
            while let Ok(ev) = menu_rx.recv() {
                if ev.id == reload_id {
                    if tx_ctrl.send(ControlMsg::Reload).is_err() {
                        tracing::warn!("failed to send Reload control message");
                    }
                    egui_ctx.request_repaint();
                } else if ev.id == help_id {
                    if tx_ctrl.send(ControlMsg::OpenPermissionsHelp).is_err() {
                        tracing::warn!("failed to send OpenPermissionsHelp control message");
                    }
                    egui_ctx.request_repaint();
                } else if ev.id == quit_id {
                    // Request a graceful shutdown via the runtime control path
                    if tx_ctrl.send(ControlMsg::Shutdown).is_err() {
                        tracing::warn!("failed to send Shutdown control message");
                    }
                    egui_ctx.request_repaint();
                } else {
                    // Check if it's a theme selection
                    for (theme_id, theme_name) in &theme_ids {
                        if ev.id == *theme_id {
                            if tx_ctrl
                                .send(ControlMsg::SwitchTheme(theme_name.clone()))
                                .is_err()
                            {
                                tracing::warn!(
                                    "failed to send SwitchTheme control message for {}",
                                    theme_name
                                );
                            }
                            egui_ctx.request_repaint();
                            break;
                        }
                    }
                }
            }
        });
    }
    tray_icon_opt
}
