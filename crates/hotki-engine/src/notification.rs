use tokio::sync::mpsc::UnboundedSender;

use hotki_protocol::{Focus, MsgToUI, NotifyKind};
use keymode::KeyResponse;

use crate::{Error, Result};

/// Sends HUD updates and notifications to the UI layer.
#[derive(Clone)]
pub struct NotificationDispatcher {
    tx: UnboundedSender<MsgToUI>,
}

impl NotificationDispatcher {
    /// Create a new dispatcher from a UI message channel.
    pub fn new(tx: UnboundedSender<MsgToUI>) -> Self {
        Self { tx }
    }

    /// Send a HUD update with the current cursor and focus snapshot.
    pub fn send_hud_update_cursor(
        &self,
        cursor: config::Cursor,
        app: String,
        title: String,
    ) -> Result<()> {
        self.tx
            .send(MsgToUI::HudUpdate {
                cursor,
                focus: Focus { app, title },
            })
            .map_err(|_| Error::ChannelClosed)
    }

    /// Send a notification with the given kind, title, and text.
    pub fn send_notification(&self, kind: NotifyKind, title: String, text: String) -> Result<()> {
        self.tx
            .send(MsgToUI::Notify { kind, title, text })
            .map_err(|_| Error::ChannelClosed)
    }

    /// Handle a `KeyResponse` by converting it to notifications/UI messages.
    pub fn handle_key_response(&self, response: KeyResponse) -> Result<()> {
        match response {
            KeyResponse::Ok => Ok(()),
            KeyResponse::Info { title, text } => {
                self.send_notification(NotifyKind::Info, title, text)
            }
            KeyResponse::Warn { title, text } => {
                self.send_notification(NotifyKind::Warn, title, text)
            }
            KeyResponse::Error { title, text } => {
                self.send_notification(NotifyKind::Error, title, text)
            }
            KeyResponse::Success { title, text } => {
                self.send_notification(NotifyKind::Success, title, text)
            }
            KeyResponse::ShellAsync { .. } => {
                // Engine repeater is responsible for execution.
                Ok(())
            }
            KeyResponse::Ui(msg) => self.tx.send(msg).map_err(|_| Error::ChannelClosed),
            KeyResponse::Relay { .. } => {
                // Relay is handled elsewhere (event handler / repeater).
                Ok(())
            }
            KeyResponse::Fullscreen { .. } => {
                // Engine handles fullscreen directly; no UI message.
                Ok(())
            }
            KeyResponse::Place { .. } => {
                // Engine handles placement directly; no UI message.
                Ok(())
            }
        }
    }

    /// Convenience helper to send an error notification.
    pub fn send_error(&self, title: &str, text: String) -> Result<()> {
        self.send_notification(NotifyKind::Error, title.to_string(), text)
    }
}
