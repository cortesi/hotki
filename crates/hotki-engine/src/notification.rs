use hotki_protocol::{DisplaysSnapshot, MsgToUI, NotifyKind};
use tokio::sync::mpsc::Sender;
use tracing::info;

use crate::{Error, Result};

/// Sends HUD updates and notifications to the UI layer.
#[derive(Clone)]
pub struct NotificationDispatcher {
    tx: Sender<MsgToUI>,
}

impl NotificationDispatcher {
    /// Create a new dispatcher from a UI message channel.
    pub fn new(tx: Sender<MsgToUI>) -> Self {
        Self { tx }
    }

    /// Send a HUD update with the current cursor and focus snapshot.
    pub fn send_hud_update_cursor(
        &self,
        cursor: config::Cursor,
        displays: DisplaysSnapshot,
    ) -> Result<()> {
        self.tx
            .try_send(MsgToUI::HudUpdate { cursor, displays })
            .map_err(|_| Error::ChannelClosed)
    }

    /// Send a notification with the given kind, title, and text.
    pub fn send_notification(&self, kind: NotifyKind, title: String, text: String) -> Result<()> {
        // Always log notification displays at info level, regardless of urgency.
        // Include kind, title and full text for traceability.
        info!(kind = ?kind, title = %title, text = %text, "notification_display");
        self.tx
            .try_send(MsgToUI::Notify { kind, title, text })
            .map_err(|_| Error::ChannelClosed)
    }

    /// Send an arbitrary UI message.
    pub(crate) fn send_ui(&self, msg: MsgToUI) -> Result<()> {
        self.tx.try_send(msg).map_err(|_| Error::ChannelClosed)
    }

    /// Convenience helper to send an error notification.
    pub fn send_error(&self, title: &str, text: String) -> Result<()> {
        self.send_notification(NotifyKind::Error, title.to_string(), text)
    }
}
