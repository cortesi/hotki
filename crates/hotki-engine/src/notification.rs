use hotki_protocol::{DisplaysSnapshot, MsgToUI, NotifyKind};
use tokio::sync::mpsc::{Permit, Sender};

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

    /// Send a HUD update with the current rendered HUD state.
    pub fn send_hud_update(
        &self,
        hud: hotki_protocol::HudState,
        displays: DisplaysSnapshot,
    ) -> Result<()> {
        self.tx
            .try_send(MsgToUI::HudUpdate {
                hud: Box::new(hud),
                displays,
            })
            .map_err(|_| Error::ChannelClosed)
    }

    /// Reserve capacity for a UI message without publishing it yet.
    pub(crate) fn reserve_ui(&self) -> Result<Permit<'_, MsgToUI>> {
        self.tx.try_reserve().map_err(|_| Error::ChannelClosed)
    }

    /// Send a notification with the given kind, title, and text.
    pub fn send_notification(&self, kind: NotifyKind, title: String, text: String) -> Result<()> {
        log_notification(kind, &title, &text);
        self.tx
            .try_send(MsgToUI::Notify { kind, title, text })
            .map_err(|_| Error::ChannelClosed)
    }

    /// Try to send an arbitrary UI message without waiting for capacity.
    pub(crate) fn try_send_ui(&self, msg: MsgToUI) -> Result<()> {
        self.tx.try_send(msg).map_err(|_| Error::ChannelClosed)
    }

    /// Send an arbitrary UI message once channel capacity is available.
    pub(crate) async fn send_ui(&self, msg: MsgToUI) -> Result<()> {
        self.tx.send(msg).await.map_err(|_| Error::ChannelClosed)
    }

    /// Convenience helper to send an error notification.
    pub fn send_error(&self, title: &str, text: String) -> Result<()> {
        self.send_notification(NotifyKind::Error, title.to_string(), text)
    }
}

/// Emit a tracing record for a displayed notification using matching severity.
fn log_notification(kind: NotifyKind, title: &str, text: &str) {
    match kind {
        NotifyKind::Error => {
            tracing::error!(target: "hotki::notification", notification = "display", kind = ?kind, title = %title, text = %text);
        }
        NotifyKind::Warn => {
            tracing::warn!(target: "hotki::notification", notification = "display", kind = ?kind, title = %title, text = %text);
        }
        NotifyKind::Info | NotifyKind::Ignore | NotifyKind::Success => {
            tracing::info!(target: "hotki::notification", notification = "display", kind = ?kind, title = %title, text = %text);
        }
    }
}

#[cfg(test)]
mod tests {
    use hotki_protocol::MsgToUI;
    use tokio::sync::mpsc;

    use super::NotificationDispatcher;

    #[tokio::test]
    async fn reliable_send_waits_for_channel_capacity() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(MsgToUI::Heartbeat(1)).expect("fill channel");
        let dispatcher = NotificationDispatcher::new(tx);
        let send = tokio::spawn(async move { dispatcher.send_ui(MsgToUI::Heartbeat(2)).await });

        tokio::task::yield_now().await;
        assert!(!send.is_finished());
        assert_eq!(rx.recv().await, Some(MsgToUI::Heartbeat(1)));
        send.await.expect("send task").expect("reliable send");
        assert_eq!(rx.recv().await, Some(MsgToUI::Heartbeat(2)));
    }
}
