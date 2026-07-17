use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

use hotki_protocol::{Heartbeat, MsgToUI, WorldStreamMsg};
use hotki_world::WorldView;
use tokio::sync::mpsc::Sender;

use super::{LifecycleRun, broadcaster::broadcast_event, registry::ClientRegistry};

/// Spawn the shared focus-change forwarder.
pub(super) fn spawn_world_forwarder(
    shutdown: Arc<AtomicBool>,
    event_tx: Sender<MsgToUI>,
    world: Arc<dyn WorldView>,
    run: LifecycleRun,
) {
    tokio::spawn(async move {
        let _run = run;
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
    registry: ClientRegistry,
    manager: Arc<mac_hotkey::Manager>,
    run: LifecycleRun,
) {
    tokio::spawn(async move {
        let _run = run;
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
            tokio::time::sleep(interval).await;
        }
    });
}
