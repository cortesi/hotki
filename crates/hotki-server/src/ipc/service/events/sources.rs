use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

use hotki_protocol::{Heartbeat, MsgToUI, WorldStreamMsg};
use hotki_world::{EventCursor, WorldView};
use tokio::{
    select,
    sync::{mpsc::Sender, watch},
};

use super::{broadcaster::broadcast_event, registry::ClientRegistry};

/// Forward shared focus changes until shutdown, cancellation, or cursor closure.
pub(super) async fn forward_world_events(
    shutdown: Arc<AtomicBool>,
    event_tx: Sender<MsgToUI>,
    world: Arc<dyn WorldView>,
    mut cursor: EventCursor,
    mut cancel: watch::Receiver<bool>,
) {
    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
        let event = select! {
            _ = cancel.changed() => break,
            event = world.next_event_until(&mut cursor, deadline) => event,
        };
        let event = match event {
            Some(event) => event,
            None => {
                if cursor.is_closed() {
                    break;
                }
                continue;
            }
        };

        let hotki_world::WorldEvent::FocusChanged(change) = event else {
            continue;
        };

        let app = match change {
            hotki_world::FocusChange::Focused(focus) => Some(focus),
            hotki_world::FocusChange::Cleared => None,
        };
        if let Err(err) = event_tx.try_send(MsgToUI::World(WorldStreamMsg::FocusChanged(app))) {
            match err {
                tokio::sync::mpsc::error::TrySendError::Full(_) => {}
                tokio::sync::mpsc::error::TrySendError::Closed(_) => break,
            }
        }
    }
}

/// Broadcast heartbeat snapshots until shutdown or cancellation.
pub(super) async fn broadcast_heartbeats(
    shutdown: Arc<AtomicBool>,
    registry: ClientRegistry,
    manager: Arc<mac_hotkey::Manager>,
    mut cancel: watch::Receiver<bool>,
) {
    let interval = hotki_protocol::ipc::heartbeat::interval();
    let mut previous = None;
    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        let ts = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let input = crate::ipc::service::input_health(manager.sample_status());
        let transition = (
            input.tap_mode,
            input.tap_lifecycle,
            input.secure_input,
            input.secure_input_owner.clone(),
            input.blocked,
        );
        if previous.as_ref() != Some(&transition) {
            tracing::info!(
                tap_mode = ?input.tap_mode,
                tap_lifecycle = ?input.tap_lifecycle,
                secure_input = ?input.secure_input,
                owner = ?input.secure_input_owner,
                blocked = input.blocked,
                "input_health_transition"
            );
            previous = Some(transition);
        }
        let heartbeat = Heartbeat {
            sent_at_ms: ts,
            input,
        };
        broadcast_event(&registry, &shutdown, MsgToUI::Heartbeat(heartbeat)).await;
        select! {
            _ = cancel.changed() => break,
            () = tokio::time::sleep(interval) => {}
        }
    }
}
