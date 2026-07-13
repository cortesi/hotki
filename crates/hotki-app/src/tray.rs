//! System tray icon and event wiring for the Hotki UI.
use std::{collections::HashMap, thread};

use egui::Context;
use hotki_protocol::{MsgToUI, Toggle};
use tokio::sync::mpsc as tokio_mpsc;
use tray_icon::{
    Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuId, MenuItem},
};

use crate::{
    app::UiEvent,
    health::{RetryState, RuntimeHealth, RuntimePhase},
    runtime::ControlMsg,
    ui_delivery::UiDeliveryTx,
};

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

#[derive(Clone, Copy)]
/// Menu actions emitted by the tray menu.
enum TrayAction {
    /// Show (or raise) the Details window.
    ShowWindow,
    /// Reload the current config file.
    Reload,
    /// Open the permissions helper window.
    OpenPermissionsHelp,
    /// Shut the application down.
    Quit,
}

/// Live tray icon and the menu rows that present runtime health.
pub struct Tray {
    /// Native tray icon handle.
    icon: TrayIcon,
    /// Disabled status row for the aggregate runtime phase.
    phase: MenuItem,
    /// Disabled status row for server connection state.
    connection: MenuItem,
    /// Disabled status row for the installed config.
    active_config: MenuItem,
    /// Disabled status row for the uncommitted config candidate.
    pending_config: MenuItem,
    /// Disabled status row for macOS permissions.
    permissions: MenuItem,
    /// Disabled status row for retry availability.
    retry: MenuItem,
    /// Existing action whose label follows retry availability.
    reload: MenuItem,
}

impl Tray {
    /// Update all tray health rows and the native tooltip from one snapshot.
    pub(crate) fn set_runtime_health(&self, health: &RuntimeHealth) {
        self.phase
            .set_text(format!("Status: {}", health.phase.display_label()));
        self.connection
            .set_text(format!("Server: {}", health.connection.display_label()));
        self.active_config
            .set_text(format!("Active config: {}", health.active_config_label()));
        self.pending_config
            .set_text(format!("Pending config: {}", health.pending_config_label()));
        self.permissions
            .set_text(format!("Permissions: {}", health.permissions_label()));
        self.retry
            .set_text(format!("Retry: {}", health.retry.display_label()));
        self.reload
            .set_text(if matches!(health.retry, RetryState::Available) {
                "Retry"
            } else {
                "Reload Config"
            });
        self.reload
            .set_enabled(!matches!(health.phase, RuntimePhase::ShuttingDown));

        let tooltip = format!("Hotki — {}", health.phase.display_label());
        if let Err(error) = self.icon.set_tooltip(Some(tooltip)) {
            tracing::warn!("failed to update tray tooltip: {error}");
        }
    }
}

/// Menu rows retained after insertion so their text can update in place.
struct TrayRows {
    /// Aggregate runtime phase.
    phase: MenuItem,
    /// Server connection state.
    connection: MenuItem,
    /// Installed config path.
    active_config: MenuItem,
    /// Pending candidate path.
    pending_config: MenuItem,
    /// Current macOS permissions.
    permissions: MenuItem,
    /// Current retry state.
    retry: MenuItem,
    /// Config reload or retry action.
    reload: MenuItem,
}

/// Build the tray menu and an id-to-action lookup table.
fn build_menu() -> (Menu, HashMap<MenuId, TrayAction>, TrayRows) {
    let menu = Menu::new();
    let phase = MenuItem::new("Status: Disconnected", false, None);
    let connection = MenuItem::new("Server: Disconnected", false, None);
    let active_config = MenuItem::new("Active config: None", false, None);
    let pending_config = MenuItem::new("Pending config: None", false, None);
    let permissions = MenuItem::new("Permissions: unknown", false, None);
    let retry = MenuItem::new("Retry: Not needed", false, None);
    let show_window = MenuItem::new("Open Hotki", true, None);
    let reload = MenuItem::new("Reload Config", true, None);
    let help = MenuItem::new("Permissions…", true, None);
    let quit = MenuItem::new("Quit", true, None);

    let mut actions = HashMap::new();
    register_base_actions(&mut actions, &show_window, &reload, &help, &quit);
    append_menu_item(&menu, &phase);
    append_menu_item(&menu, &connection);
    append_menu_item(&menu, &active_config);
    append_menu_item(&menu, &pending_config);
    append_menu_item(&menu, &permissions);
    append_menu_item(&menu, &retry);
    append_menu_item(&menu, &show_window);
    append_menu_item(&menu, &reload);
    append_menu_item(&menu, &help);
    append_menu_item(&menu, &quit);

    (
        menu,
        actions,
        TrayRows {
            phase,
            connection,
            active_config,
            pending_config,
            permissions,
            retry,
            reload,
        },
    )
}

/// Register fixed tray menu actions.
fn register_base_actions(
    actions: &mut HashMap<MenuId, TrayAction>,
    show_window: &MenuItem,
    reload: &MenuItem,
    help: &MenuItem,
    quit: &MenuItem,
) {
    actions.insert(show_window.id().clone(), TrayAction::ShowWindow);
    actions.insert(reload.id().clone(), TrayAction::Reload);
    actions.insert(help.id().clone(), TrayAction::OpenPermissionsHelp);
    actions.insert(quit.id().clone(), TrayAction::Quit);
}

/// Append one menu item and log failures.
fn append_menu_item(menu: &Menu, item: &MenuItem) {
    if let Err(error) = menu.append(item) {
        tracing::warn!("failed to append tray menu item: {}", error);
    }
}

/// Dispatch one tray action onto the runtime control channel or the UI channel.
fn dispatch_tray_action(
    tx: &UiDeliveryTx,
    tx_ctrl: &tokio_mpsc::UnboundedSender<ControlMsg>,
    action: TrayAction,
) {
    // `ShowWindow` targets the UI event channel directly; every other action
    // maps onto a runtime control message.
    let message = match action {
        TrayAction::ShowWindow => {
            if tx
                .send(UiEvent::Message(MsgToUI::ShowDetails(Toggle::On)))
                .is_err()
            {
                tracing::warn!("failed to send ShowDetails event: UI channel closed");
            }
            return;
        }
        TrayAction::Reload => ControlMsg::Reload,
        TrayAction::OpenPermissionsHelp => ControlMsg::OpenPermissionsHelp,
        TrayAction::Quit => ControlMsg::Shutdown,
    };

    if tx_ctrl.send(message).is_err() {
        tracing::warn!("failed to send tray control message");
    }
}

/// Build the tray icon and spawn listeners for tray and menu events.
pub fn build_tray_and_listeners(
    tx: &UiDeliveryTx,
    tx_ctrl: &tokio_mpsc::UnboundedSender<ControlMsg>,
    egui_ctx: &Context,
    health: &RuntimeHealth,
) -> Option<Tray> {
    let (menu, actions, rows) = build_menu();

    let tray_icon = {
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
    let tray = tray_icon.map(|icon| Tray {
        icon,
        phase: rows.phase,
        connection: rows.connection,
        active_config: rows.active_config,
        pending_config: rows.pending_config,
        permissions: rows.permissions,
        retry: rows.retry,
        reload: rows.reload,
    });
    if let Some(tray) = tray.as_ref() {
        tray.set_runtime_health(health);
    }

    if tray.is_some() {
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
                    egui_ctx.request_repaint();
                    if tx
                        .send(UiEvent::Message(MsgToUI::ShowDetails(Toggle::On)))
                        .is_err()
                    {
                        tracing::warn!("failed to send ShowDetails event: UI channel closed");
                    }
                }
            }
        });
    }

    if tray.is_some() {
        let tx = tx.clone();
        let tx_ctrl = tx_ctrl.clone();
        let egui_ctx = egui_ctx.clone();
        thread::spawn(move || {
            let menu_rx = MenuEvent::receiver();
            while let Ok(ev) = menu_rx.recv() {
                if let Some(action) = actions.get(&ev.id).cloned() {
                    egui_ctx.request_repaint();
                    dispatch_tray_action(&tx, &tx_ctrl, action);
                }
            }
        });
    }
    tray
}
