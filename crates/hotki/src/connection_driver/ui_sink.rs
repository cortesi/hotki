use std::{path::PathBuf, process};

use egui::Context;
use hotki_protocol::{MsgToUI, NotifyKind};
use tokio::{
    sync::mpsc,
    time::{Duration, sleep},
};

use crate::app::{UiCommand, UiEvent};

/// Minimal UI event sink that owns repaint requests and channel forwarding.
pub(super) struct UiSink {
    /// Sender for UI-thread events.
    tx_keys: mpsc::UnboundedSender<UiEvent>,
    /// Egui context used for repaint and root-viewport commands.
    egui_ctx: Context,
}

impl UiSink {
    /// Build a sink for the UI channel and egui context.
    pub(super) fn new(tx_keys: mpsc::UnboundedSender<UiEvent>, egui_ctx: Context) -> Self {
        Self { tx_keys, egui_ctx }
    }

    /// Enqueue a protocol message for the UI thread.
    pub(super) fn send_message(&self, message: MsgToUI) {
        self.emit(UiEvent::Message(message));
    }

    /// Enqueue a local command for the UI thread.
    pub(super) fn send_command(&self, command: UiCommand) {
        self.emit(UiEvent::Command(command));
    }

    /// Notify the user through the in-app notification path.
    pub(super) fn notify(&self, kind: NotifyKind, title: &str, text: &str) {
        self.send_message(MsgToUI::Notify {
            kind,
            title: title.to_string(),
            text: text.to_string(),
        });
    }

    /// Update the config path shown by the UI.
    pub(super) fn set_config_path(&self, path: Option<PathBuf>) {
        self.send_command(UiCommand::SetConfigPath(path));
    }

    /// Ask the UI to show the permissions helper.
    pub(super) fn show_permissions_help(&self) {
        self.send_command(UiCommand::ShowPermissionsHelp);
    }

    /// Ask the UI to shut down and close the root viewport.
    pub(super) fn trigger_graceful_shutdown(&self, fallback_ms: u64) {
        self.send_command(UiCommand::Shutdown);
        self.egui_ctx
            .send_viewport_cmd(egui::ViewportCommand::Close);
        self.request_repaint();
        tokio::spawn(async move {
            sleep(Duration::from_millis(fallback_ms)).await;
            process::exit(0);
        });
    }

    /// Request a repaint without sending a new event.
    pub(super) fn request_repaint(&self) {
        self.egui_ctx.request_repaint();
    }

    /// Forward a UI event and immediately request a repaint.
    fn emit(&self, event: UiEvent) {
        if self.tx_keys.send(event).is_err() {
            tracing::warn!("failed to forward UI event");
        }
        self.egui_ctx.request_repaint();
    }
}
