use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

use hotki_protocol::{MsgToUI, WorldStreamMsg};
use hotki_world::WorldView;
use tokio::sync::mpsc::Sender;

/// Spawn the shared focus-change forwarder.
pub(super) fn spawn_world_forwarder(
    shutdown: Arc<AtomicBool>,
    event_tx: Sender<MsgToUI>,
    world: Arc<dyn WorldView>,
) {
    tokio::spawn(async move {
        let mut cursor = world.subscribe();
        loop {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }

            let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
            let event = match world.next_event_until(&mut cursor, deadline).await {
                Some(event) => event,
                None => {
                    if cursor.is_closed() {
                        return;
                    }
                    continue;
                }
            };

            let hotki_world::WorldEvent::FocusChanged(change) = event else {
                continue;
            };

            let app = hotki_world::focus_snapshot_for_change(world.as_ref(), &change).await;
            if let Err(err) = event_tx.try_send(MsgToUI::World(WorldStreamMsg::FocusChanged(app))) {
                match err {
                    tokio::sync::mpsc::error::TrySendError::Full(_) => {}
                    tokio::sync::mpsc::error::TrySendError::Closed(_) => return,
                }
            }
        }
    });
}

/// Spawn the shared heartbeat source that feeds the event queue.
pub(super) fn spawn_heartbeat(
    shutdown: Arc<AtomicBool>,
    event_tx: Sender<MsgToUI>,
    hb_running: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        let interval = hotki_protocol::ipc::heartbeat::interval();
        loop {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            let ts = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0);
            if let Err(err) = event_tx.try_send(MsgToUI::Heartbeat(ts)) {
                match err {
                    tokio::sync::mpsc::error::TrySendError::Full(_) => {}
                    tokio::sync::mpsc::error::TrySendError::Closed(_) => break,
                }
            }
            tokio::time::sleep(interval).await;
        }
        hb_running.store(false, Ordering::SeqCst);
    });
}
