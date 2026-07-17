use egui::Context;
use hotki_protocol::{InputHealth, MsgToUI, NotifyKind};

use crate::{
    app::{UiCommand, UiEvent},
    health::RuntimeHealth,
    ui_delivery::{UiDeliveryOutcome, UiDeliveryTx},
};

/// Minimal UI event sink that owns repaint requests and channel forwarding.
pub(super) struct UiSink {
    /// Sender for UI-thread events.
    tx_keys: UiDeliveryTx,
    /// Egui context used for repaint and root-viewport commands.
    egui_ctx: Context,
}

impl UiSink {
    /// Build a sink for the UI channel and egui context.
    pub(super) fn new(tx_keys: UiDeliveryTx, egui_ctx: Context) -> Self {
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

    /// Ask the UI to show the permissions helper.
    pub(super) fn show_permissions_help(&self) {
        self.send_command(UiCommand::ShowPermissionsHelp);
    }

    /// Replace the complete UI-visible runtime health snapshot.
    pub(super) fn set_runtime_health(&self, health: RuntimeHealth) {
        self.send_command(UiCommand::SetRuntimeHealth(health));
    }

    /// Update the UI-visible server binding list.
    pub(super) fn set_server_bindings(&self, bindings: Vec<String>) {
        self.send_command(UiCommand::SetServerBindings(bindings));
    }

    /// Replace the full diagnostic input-health snapshot.
    pub(super) fn set_input_health(&self, input: InputHealth) {
        self.send_command(UiCommand::SetInputHealth(input));
    }

    /// Ask the UI to shut down after the runtime has finished its owned work.
    pub(super) fn finish_shutdown(&self) {
        self.send_command(UiCommand::Shutdown);
    }

    /// Request a repaint without sending a new event.
    pub(super) fn request_repaint(&self) {
        self.egui_ctx.request_repaint();
    }

    /// Forward a UI event and immediately request a repaint.
    fn emit(&self, event: UiEvent) {
        self.egui_ctx.request_repaint();
        match self.tx_keys.send(event) {
            Ok(UiDeliveryOutcome::Queued) => {}
            Ok(UiDeliveryOutcome::Coalesced) => {
                tracing::trace!("coalesced superseded UI snapshot");
            }
            Ok(UiDeliveryOutcome::DroppedLogFull) => {
                tracing::trace!("dropped UI log because bounded lane is full");
            }
            Err(error) => tracing::warn!("failed to forward UI event: {error}"),
        }
    }
}
