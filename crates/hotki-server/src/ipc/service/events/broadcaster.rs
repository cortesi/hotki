use std::{
    slice,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use futures::stream::{FuturesUnordered, StreamExt};
use hotki_protocol::MsgToUI;
use tracing::{error, warn};

use super::registry::ClientRegistry;
use crate::ipc::service::rpc::enc_event;

/// Fan out one queued UI event to all currently connected clients.
pub(super) async fn broadcast_event(
    registry: &ClientRegistry,
    shutdown: &Arc<AtomicBool>,
    event: MsgToUI,
) {
    if shutdown.load(Ordering::SeqCst) {
        return;
    }

    let clients_snapshot = registry.snapshot().await;
    let value = match enc_event(&event) {
        Ok(value) => value,
        Err(err) => {
            error!("Failed to encode event for broadcast: {}", err);
            return;
        }
    };

    let mut sends = FuturesUnordered::new();
    for client in clients_snapshot {
        let value = value.clone();
        sends.push(async move {
            (
                client.id,
                client
                    .sender
                    .send_notification(
                        hotki_protocol::rpc::HotkeyNotification::Notify.as_str(),
                        slice::from_ref(&value),
                    )
                    .await,
            )
        });
    }

    while let Some((client_id, result)) = sends.next().await {
        if let Err(err) = result {
            warn!("Dropping disconnected client (send failed): {:?}", err);
            registry.remove(client_id).await;
        }
    }
}
