//! Background UI runtime glue: connects to the server, forwards events to the UI,
//! and applies configuration/overrides.

use std::{path::Path, thread};

use egui::Context;
use tokio::sync::mpsc;
use tracing::info;

pub use crate::control::ControlMsg;
use crate::{app::AppEvent, connection_driver::ConnectionDriver};

/// Start background key runtime and server connection driver on a dedicated thread.
#[allow(clippy::too_many_arguments)]
pub fn spawn_key_runtime(
    cfg: &config::Config,
    config_path: &Path,
    tx_keys: &mpsc::UnboundedSender<AppEvent>,
    egui_ctx: &Context,
    tx_ctrl_runtime: &mpsc::UnboundedSender<ControlMsg>,
    rx_ctrl: mpsc::UnboundedReceiver<ControlMsg>,
    server_log_filter: Option<String>,
    dumpworld: bool,
) {
    let cfg = cfg.clone();
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
            if let Some(mut client) = driver.connect(cfg).await {
                driver.drive_events(&mut client).await;
            }
        });
    });
}
