use std::path::Path;

use egui::Context;
use hotki_protocol::NotifyKind;
use tokio::sync::mpsc;

use crate::app::AppEvent;

/// Apply the current UI config: notify UI to reload and repaint.
pub async fn apply_ui_config(
    ui_config: &config::Config,
    tx_keys: &mpsc::UnboundedSender<AppEvent>,
    egui_ctx: &Context,
) {
    if tx_keys
        .send(AppEvent::ReloadUI(Box::new(ui_config.clone())))
        .is_err()
    {
        tracing::warn!("failed to send ReloadUI to app channel");
    }
    egui_ctx.request_repaint();
}

/// Single-source reload: load from disk, apply to UI + server, and notify success or error.
pub async fn reload_and_broadcast(
    conn: &mut hotki_server::Connection,
    ui_config: &mut config::Config,
    config_path: &Path,
    tx_keys: &mpsc::UnboundedSender<AppEvent>,
    egui_ctx: &Context,
) {
    match conn
        .set_config_path(config_path.to_string_lossy().as_ref())
        .await
    {
        Ok(new_cfg) => {
            *ui_config = new_cfg;
            if tx_keys
                .send(AppEvent::Notify {
                    kind: NotifyKind::Success,
                    title: "Config".to_string(),
                    text: "Reloaded successfully".to_string(),
                })
                .is_err()
            {
                tracing::warn!("failed to send reload success notification");
            }
            apply_ui_config(ui_config, tx_keys, egui_ctx).await;
        }
        Err(e) => {
            if tx_keys
                .send(AppEvent::Notify {
                    kind: NotifyKind::Error,
                    title: "Config".to_string(),
                    text: e.to_string(),
                })
                .is_err()
            {
                tracing::warn!("failed to send reload error notification");
            }
            egui_ctx.request_repaint();
        }
    };
}
