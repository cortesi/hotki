//! Background UI runtime glue: connects to the server, forwards events to the UI,
//! and applies configuration/overrides.

use std::{path::Path, thread};

use egui::Context;
use hotki_protocol::NotifyKind;
use hotki_server::smoketest_bridge::{BridgeCommandId, BridgeRequest, BridgeResponse};
use tokio::sync::{mpsc, oneshot};
use tracing::info;

use crate::{app::UiEvent, connection_driver::ConnectionDriver};

/// Control messages routed to the runtime event loop.
#[derive(Debug)]
pub enum ControlMsg {
    /// Reload from disk using `config_path`.
    Reload,
    /// Gracefully shut down the UI and exit the process.
    Shutdown,
    /// Request a server-side theme switch by name.
    SwitchTheme(String),
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
    /// Internal test bridge command (smoketest harness).
    Test(TestCommand),
}

/// Request/response pair used to service smoketest bridge commands.
#[derive(Debug)]
pub struct TestCommand {
    /// Identifier for the command being serviced.
    pub(crate) command_id: BridgeCommandId,
    /// The bridge request submitted by the smoketest harness.
    pub(crate) req: BridgeRequest,
    /// Channel used to deliver the bridge response back to the harness.
    pub(crate) respond_to: oneshot::Sender<BridgeResponse>,
}

/// Start background key runtime and server connection driver on a dedicated thread.
#[allow(clippy::too_many_arguments)]
pub fn spawn_key_runtime(
    config_path: &Path,
    tx_keys: &mpsc::UnboundedSender<UiEvent>,
    egui_ctx: &Context,
    tx_ctrl_runtime: &mpsc::UnboundedSender<ControlMsg>,
    rx_ctrl: mpsc::UnboundedReceiver<ControlMsg>,
    server_log_filter: Option<String>,
    dumpworld: bool,
) {
    let config_path = config_path.to_path_buf();
    let tx_keys = tx_keys.clone();
    let egui_ctx = egui_ctx.clone();
    let tx_ctrl_runtime = tx_ctrl_runtime.clone();
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
                tx_ctrl_runtime,
                dumpworld,
            );
            if let Some(mut client) = driver.connect().await {
                driver.drive_events(&mut client).await;
            }
        });
    });
}
