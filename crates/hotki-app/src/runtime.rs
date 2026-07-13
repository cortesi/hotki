//! Background UI runtime glue: connects to the server, forwards events to the UI,
//! and applies configuration/overrides.

use std::{path::Path, thread};

use egui::Context;
use hotki_protocol::NotifyKind;
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    connection_driver::ConnectionDriver, permissions::PermissionsStatus, ui_delivery::UiDeliveryTx,
};

/// Control messages routed to the runtime event loop.
#[derive(Debug)]
pub enum ControlMsg {
    /// Reload from disk using `config_path`.
    Reload,
    /// Gracefully shut down the UI and exit the process.
    Shutdown,
    /// Inject one complete key press through the connected server.
    InjectKey {
        /// Key chord identifier, for example `shift+cmd+0`.
        ident: String,
        /// Whether failed injection should be surfaced to the user.
        report_errors: bool,
    },
    /// Open the in-app permissions help view.
    OpenPermissionsHelp,
    /// Forward a user-facing notice into the app UI.
    Notice {
        /// Notice severity kind.
        kind: NotifyKind,
        /// Notice title text.
        title: String,
        /// Notice body text.
        text: String,
    },
    /// Current macOS permission status as observed by the UI.
    PermissionsChanged(PermissionsStatus),
}

/// Start background key runtime and server connection driver on a dedicated thread.
pub fn spawn_key_runtime(
    config_path: &Path,
    tx_keys: &UiDeliveryTx,
    egui_ctx: &Context,
    rx_ctrl: mpsc::UnboundedReceiver<ControlMsg>,
    server_log_filter: Option<String>,
    server_event_tap_enabled: bool,
    dumpworld: bool,
) {
    let config_path = config_path.to_path_buf();
    let tx_keys = tx_keys.clone();
    let egui_ctx = egui_ctx.clone();
    thread::spawn(move || {
        use tokio::runtime::Runtime;
        let rt = match Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("Failed to create Tokio runtime: {}", e);
                return;
            }
        };
        rt.block_on(async move {
            info!("Loaded mode; delegating to server engine");
            let mut driver = ConnectionDriver::new(
                config_path,
                server_log_filter,
                tx_keys,
                egui_ctx,
                rx_ctrl,
                server_event_tap_enabled,
                dumpworld,
            );
            driver.run().await;
        });
    });
}
