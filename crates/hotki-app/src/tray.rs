//! System tray icon and event wiring for the Hotki UI.

use std::{collections::HashMap, ptr, thread};

use egui::Context;
use hotki_protocol::{MsgToUI, Toggle};
use objc2::{AnyThread, msg_send, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{
    NSAboutPanelOptionApplicationIcon, NSAboutPanelOptionApplicationName,
    NSAboutPanelOptionApplicationVersion, NSAboutPanelOptionCredits, NSApplication, NSImage,
};
use objc2_foundation::{MainThreadMarker, NSAttributedString, NSData, NSDictionary, NSString};
use tokio::sync::mpsc as tokio_mpsc;
use tray_icon::{
    Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
};

use crate::{
    app::{UiCommand, UiEvent},
    health::{RuntimeHealth, RuntimePhase},
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

/// Decode the embedded pixels once for either native icon type.
fn tray_icon_rgba() -> Option<(Vec<u8>, u32, u32)> {
    let image = image::load_from_memory(TRAY_ICON_PNG).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    Some((image.to_vec(), width, height))
}

/// Decode the status-item icon.
fn tray_icon_image() -> Option<Icon> {
    let (rgba, width, height) = tray_icon_rgba()?;
    Icon::from_rgba(rgba, width, height).ok()
}

/// Menu actions emitted by ordinary command items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayAction {
    /// Show or raise the main window.
    ShowMainWindow,
    /// Reload the current config file.
    Reload,
    /// Open the permissions helper window.
    OpenPermissions,
    /// Open the standard macOS About panel.
    About,
    /// Shut the application down.
    Quit,
}

/// Stable native menu items reused whenever the optional notice changes.
struct TrayItems {
    /// Disabled non-ready notice.
    notice: MenuItem,
    /// Open the main window.
    open: MenuItem,
    /// Reload the configuration.
    reload: MenuItem,
    /// Open required-permission guidance.
    permissions: MenuItem,
    /// Standard macOS About command.
    about: MenuItem,
    /// Quit Hotki.
    quit: MenuItem,
}

impl TrayItems {
    /// Construct the complete stable command set.
    fn new() -> Self {
        Self {
            notice: MenuItem::new("Starting Hotki…", false, None),
            open: MenuItem::new("Open Hotki", true, None),
            reload: MenuItem::new("Reload Config", true, None),
            permissions: MenuItem::new("Permissions", true, None),
            about: MenuItem::new("About Hotki", true, None),
            quit: MenuItem::new("Quit Hotki", true, None),
        }
    }
}

/// Plain-language copy derived from the shared runtime presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TrayCopy {
    /// Optional non-ready notice title.
    notice: Option<&'static str>,
    /// Status-item tooltip.
    tooltip: String,
}

impl TrayCopy {
    /// Derive tray copy without exposing runtime diagnostic fields.
    fn from_health(health: &RuntimeHealth) -> Self {
        let presentation = health.presentation();
        let notice = presentation.notice.map(|notice| notice.title);
        let tooltip = notice.map_or_else(|| "Hotki".to_string(), str::to_string);
        Self { notice, tooltip }
    }
}

/// Live status item and its stable menu command set.
pub struct Tray {
    /// Native tray icon handle.
    icon: TrayIcon,
    /// Stable items used to rebuild menus when notice visibility changes.
    items: TrayItems,
}

impl Tray {
    /// Update the optional notice, tooltip, and command availability.
    pub(crate) fn set_runtime_health(&self, health: &RuntimeHealth) {
        let copy = TrayCopy::from_health(health);
        if let Some(notice) = copy.notice {
            self.items.notice.set_text(notice);
        }
        self.items
            .reload
            .set_enabled(!matches!(health.phase, RuntimePhase::ShuttingDown));
        self.icon.set_menu(Some(Box::new(build_menu(
            &self.items,
            copy.notice.is_some(),
        ))));
        if let Err(error) = self.icon.set_tooltip(Some(copy.tooltip)) {
            tracing::warn!(?error, "failed to update tray tooltip");
        }
    }
}

/// Build the current grouped menu around stable command items.
fn build_menu(items: &TrayItems, show_notice: bool) -> Menu {
    let menu = Menu::new();
    if show_notice {
        append_menu_item(&menu, &items.notice);
        append_menu_item(&menu, &PredefinedMenuItem::separator());
    }
    for item in [
        &items.open as &dyn IsMenuItem,
        &items.reload,
        &items.permissions,
        &items.about,
    ] {
        append_menu_item(&menu, item);
    }
    append_menu_item(&menu, &PredefinedMenuItem::separator());
    append_menu_item(&menu, &items.quit);
    menu
}

/// Map stable command IDs to their runtime actions.
fn action_map(items: &TrayItems) -> HashMap<MenuId, TrayAction> {
    HashMap::from([
        (items.open.id().clone(), TrayAction::ShowMainWindow),
        (items.reload.id().clone(), TrayAction::Reload),
        (items.permissions.id().clone(), TrayAction::OpenPermissions),
        (items.about.id().clone(), TrayAction::About),
        (items.quit.id().clone(), TrayAction::Quit),
    ])
}

/// Append one menu item and log native failures.
fn append_menu_item(menu: &Menu, item: &dyn IsMenuItem) {
    if let Err(error) = menu.append(item) {
        tracing::warn!(?error, "failed to append tray menu item");
    }
}

/// Request main-thread activation for Accessory-owned panels and viewports.
fn activate_hotki(tx: &UiDeliveryTx) {
    if tx.send(UiEvent::Command(UiCommand::ActivateApp)).is_err() {
        tracing::warn!("failed to activate Hotki: UI channel closed");
    }
}

/// Activate Hotki and open the standard About panel with package metadata.
pub fn show_about_panel() -> bool {
    let Some(marker) = MainThreadMarker::new() else {
        tracing::warn!("failed to show About panel off the main thread");
        return false;
    };
    let application = NSApplication::sharedApplication(marker);
    let existing_windows = application.windows();
    force_activate(&application);

    let mut keys: Vec<&NSString> = Vec::new();
    let mut objects: Vec<Retained<AnyObject>> = Vec::new();

    keys.push(unsafe { NSAboutPanelOptionApplicationName });
    objects.push(Retained::into_super(Retained::into_super(
        NSString::from_str("Hotki"),
    )));

    keys.push(unsafe { NSAboutPanelOptionApplicationVersion });
    objects.push(Retained::into_super(Retained::into_super(
        NSString::from_str(env!("CARGO_PKG_VERSION")),
    )));

    if let Some(icon) = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(TRAY_ICON_PNG))
    {
        keys.push(unsafe { NSAboutPanelOptionApplicationIcon });
        objects.push(Retained::into_super(Retained::into_super(icon)));
    }

    let credits = format!(
        "{}\n{}",
        env!("CARGO_PKG_DESCRIPTION"),
        env!("CARGO_PKG_REPOSITORY")
    );
    keys.push(unsafe { NSAboutPanelOptionCredits });
    objects.push(Retained::into_super(Retained::into_super(
        NSAttributedString::from_nsstring(&NSString::from_str(&credits)),
    )));

    let options = NSDictionary::from_retained_objects(&keys, &objects);
    unsafe { application.orderFrontStandardAboutPanelWithOptions(&options) };
    let windows = application.windows();
    let Some(panel) = windows
        .iter()
        .find(|window| {
            !existing_windows
                .iter()
                .any(|existing| ptr::eq(&*existing, &**window))
        })
        .or_else(|| application.keyWindow().filter(|window| window.isVisible()))
    else {
        return false;
    };
    panel.makeKeyAndOrderFront(None);
    force_activate(&application);
    application.isActive() && panel.isKeyWindow()
}

/// Force activation for an Accessory app after a user invokes its status item.
fn force_activate(application: &NSApplication) {
    unsafe {
        let _: () = msg_send![application, activateIgnoringOtherApps: true];
    }
}

/// Bring Accessory-owned panels and viewports above the currently focused app.
pub fn activate_app() {
    let Some(marker) = MainThreadMarker::new() else {
        tracing::warn!("failed to activate Hotki off the main thread");
        return;
    };
    force_activate(&NSApplication::sharedApplication(marker));
}

/// Send the main-window show event through the normal protocol lane.
fn show_main_window(tx: &UiDeliveryTx) {
    if tx
        .send(UiEvent::Message(MsgToUI::ShowMainWindow(Toggle::On)))
        .is_err()
    {
        tracing::warn!("failed to show main window: UI channel closed");
    }
}

/// Dispatch one tray command to the UI or runtime control lane.
fn dispatch_tray_action(
    tx: &UiDeliveryTx,
    tx_ctrl: &tokio_mpsc::UnboundedSender<ControlMsg>,
    action: TrayAction,
) {
    let control = match action {
        TrayAction::ShowMainWindow => {
            show_main_window(tx);
            return;
        }
        TrayAction::Reload => ControlMsg::Reload,
        TrayAction::OpenPermissions => ControlMsg::OpenPermissionsHelp,
        TrayAction::About => {
            if tx.send(UiEvent::Command(UiCommand::ShowAbout)).is_err() {
                tracing::warn!("failed to show About panel: UI channel closed");
            }
            return;
        }
        TrayAction::Quit => ControlMsg::Shutdown,
    };
    if tx_ctrl.send(control).is_err() {
        tracing::warn!("failed to send tray control message");
    }
}

/// Build the status item and spawn its native event listeners.
pub fn build_tray_and_listeners(
    tx: &UiDeliveryTx,
    tx_ctrl: &tokio_mpsc::UnboundedSender<ControlMsg>,
    egui_ctx: &Context,
    health: &RuntimeHealth,
) -> Option<Tray> {
    let items = TrayItems::new();
    let actions = action_map(&items);
    let show_notice = health.presentation().notice.is_some();
    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(build_menu(&items, show_notice)))
        .with_menu_on_left_click(false);
    if let Some(icon) = tray_icon_image() {
        builder = builder
            .with_icon(icon)
            .with_icon_as_template(!cfg!(debug_assertions));
    }
    let icon = match builder.with_tooltip("Hotki").build() {
        Ok(icon) => icon,
        Err(error) => {
            tracing::error!(?error, "failed to create tray icon");
            return None;
        }
    };
    let tray = Tray { icon, items };
    tray.set_runtime_health(health);

    let click_tx = tx.clone();
    let click_ctx = egui_ctx.clone();
    thread::spawn(move || {
        let receiver = TrayIconEvent::receiver();
        while let Ok(event) = receiver.recv() {
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    ..
                }
                | TrayIconEvent::DoubleClick { .. } => {
                    click_ctx.request_repaint();
                    activate_hotki(&click_tx);
                    show_main_window(&click_tx);
                }
                TrayIconEvent::Click {
                    button: MouseButton::Right,
                    ..
                } => activate_hotki(&click_tx),
                _ => {}
            }
        }
    });

    let menu_tx = tx.clone();
    let menu_ctrl = tx_ctrl.clone();
    let menu_ctx = egui_ctx.clone();
    thread::spawn(move || {
        let receiver = MenuEvent::receiver();
        while let Ok(event) = receiver.recv() {
            activate_hotki(&menu_tx);
            menu_ctx.request_repaint();
            if let Some(action) = actions.get(&event.id).copied() {
                dispatch_tray_action(&menu_tx, &menu_ctrl, action);
            }
        }
    });
    Some(tray)
}

#[cfg(test)]
mod tests {
    use super::TrayCopy;
    use crate::health::{RetryState, RuntimeHealth, RuntimePhase};

    #[test]
    fn ready_tray_has_no_notice_and_plain_tooltip() {
        let copy = TrayCopy::from_health(&RuntimeHealth {
            phase: RuntimePhase::Ready,
            ..RuntimeHealth::default()
        });

        assert_eq!(copy.notice, None);
        assert_eq!(copy.tooltip, "Hotki");
    }

    #[test]
    fn non_ready_tray_uses_shared_notice_title() {
        let health = RuntimeHealth {
            phase: RuntimePhase::Disconnected,
            retry: RetryState::Available,
            ..RuntimeHealth::default()
        };
        let copy = TrayCopy::from_health(&health);

        assert_eq!(copy.notice, Some("Hotki isn't running"));
        assert_eq!(copy.tooltip, "Hotki isn't running");
        assert_eq!(
            copy.notice,
            health.presentation().notice.map(|notice| notice.title)
        );
    }
}
