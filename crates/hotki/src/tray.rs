//! System tray icon and event wiring for the Hotki UI.
use std::{collections::HashMap, thread};

use config::themes;
use egui::Context;
use hotki_protocol::{MsgToUI, Toggle};
use tokio::sync::mpsc as tokio_mpsc;
use tray_icon::{
    Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuId, MenuItem, Submenu},
};

use crate::{app::UiEvent, runtime::ControlMsg};

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

#[derive(Clone)]
/// Menu actions emitted by the tray menu.
enum TrayAction {
    /// Reload the current config file.
    Reload,
    /// Open the permissions helper window.
    OpenPermissionsHelp,
    /// Shut the application down.
    Quit,
    /// Switch to the named theme.
    Theme(String),
}

/// Build the tray menu and an id-to-action lookup table.
fn build_menu() -> (Menu, HashMap<MenuId, TrayAction>) {
    let menu = Menu::new();
    let reload = MenuItem::new("Reload Config", true, None);
    let help = MenuItem::new("Permissions Help", true, None);
    let themes_menu = Submenu::new("Themes", true);
    let quit = MenuItem::new("Quit", true, None);

    let mut actions = HashMap::new();
    actions.insert(reload.id().clone(), TrayAction::Reload);
    actions.insert(help.id().clone(), TrayAction::OpenPermissionsHelp);
    actions.insert(quit.id().clone(), TrayAction::Quit);

    for theme_name in themes::list_themes() {
        let theme_item = MenuItem::new(theme_name, true, None);
        actions.insert(
            theme_item.id().clone(),
            TrayAction::Theme(theme_name.to_string()),
        );
        if let Err(error) = themes_menu.append(&theme_item) {
            tracing::warn!("failed to append theme item: {}", error);
        }
    }

    for item in [&reload] {
        if let Err(error) = menu.append(item) {
            tracing::warn!("failed to append tray menu item: {}", error);
        }
    }
    if let Err(error) = menu.append(&themes_menu) {
        tracing::warn!("failed to append themes submenu: {}", error);
    }
    for item in [&help, &quit] {
        if let Err(error) = menu.append(item) {
            tracing::warn!("failed to append tray menu item: {}", error);
        }
    }

    (menu, actions)
}

/// Dispatch one tray action onto the runtime control channel.
fn dispatch_tray_action(tx_ctrl: &tokio_mpsc::UnboundedSender<ControlMsg>, action: TrayAction) {
    let message = match action {
        TrayAction::Reload => ControlMsg::Reload,
        TrayAction::OpenPermissionsHelp => ControlMsg::OpenPermissionsHelp,
        TrayAction::Quit => ControlMsg::Shutdown,
        TrayAction::Theme(name) => ControlMsg::SwitchTheme(name),
    };

    if tx_ctrl.send(message).is_err() {
        tracing::warn!("failed to send tray control message");
    }
}

/// Build the tray icon and spawn listeners for tray and menu events.
pub fn build_tray_and_listeners(
    tx: &tokio_mpsc::UnboundedSender<UiEvent>,
    tx_ctrl: &tokio_mpsc::UnboundedSender<ControlMsg>,
    egui_ctx: &Context,
) -> Option<TrayIcon> {
    let (menu, actions) = build_menu();

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
                    if tx
                        .send(UiEvent::Message(MsgToUI::ShowDetails(Toggle::On)))
                        .is_err()
                    {
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
                if let Some(action) = actions.get(&ev.id).cloned() {
                    dispatch_tray_action(&tx_ctrl, action);
                    egui_ctx.request_repaint();
                }
            }
        });
    }
    tray_icon_opt
}
