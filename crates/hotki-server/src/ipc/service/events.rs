use std::{
    slice,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

use futures::stream::{FuturesUnordered, StreamExt};
use hotki_protocol::{MsgToUI, WorldStreamMsg};
use hotki_world::WorldView;
use mrpc::RpcSender;
use parking_lot::Mutex;
use tokio::sync::{
    Mutex as AsyncMutex,
    mpsc::{Receiver, Sender},
};
use tracing::{error, warn};

use super::rpc::enc_event;

/// Shared event pipeline for broadcasting UI events to connected clients.
#[derive(Clone)]
pub(super) struct EventPipeline {
    /// Event sender for UI messages (bounded).
    event_tx: Sender<MsgToUI>,
    /// Event receiver, taken by the first connection that starts forwarding.
    event_rx: Arc<Mutex<Option<Receiver<MsgToUI>>>>,
    /// Connected clients.
    clients: Arc<AsyncMutex<Vec<RpcSender>>>,
    /// Global shutdown flag shared with the Tao loop.
    shutdown: Arc<AtomicBool>,
    /// Ensure only one heartbeat loop is active.
    hb_running: Arc<AtomicBool>,
    /// Ensure only one world forwarder loop is active.
    world_forwarder_running: Arc<AtomicBool>,
}

impl EventPipeline {
    /// Create a new event pipeline with a fresh UI message channel.
    pub(super) fn new(shutdown: Arc<AtomicBool>) -> Self {
        let (event_tx, event_rx) = hotki_protocol::ipc::ui_channel();
        Self {
            event_tx,
            event_rx: Arc::new(Mutex::new(Some(event_rx))),
            clients: Arc::new(AsyncMutex::new(Vec::new())),
            shutdown,
            hb_running: Arc::new(AtomicBool::new(false)),
            world_forwarder_running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Clone the sender used by the engine and logging pipeline.
    pub(super) fn sender(&self) -> Sender<MsgToUI> {
        self.event_tx.clone()
    }

    /// Return the shutdown flag.
    pub(super) fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    /// Return the number of connected clients.
    pub(super) async fn client_count(&self) -> usize {
        self.clients.lock().await.len()
    }

    /// Register a new connected client.
    pub(super) async fn register_client(&self, client: RpcSender) {
        self.clients.lock().await.push(client);
    }

    /// Take ownership of the event receiver so forwarding starts exactly once.
    pub(super) fn take_event_rx(&self) -> Option<Receiver<MsgToUI>> {
        self.event_rx.lock().take()
    }

    /// Clear clients and close the local event pipeline for shutdown.
    pub(super) async fn clear_for_shutdown(&self) {
        logging::forward::clear_sink();
        self.clients.lock().await.clear();
        *self.event_rx.lock() = None;
    }

    /// Bind the global log sink to the shared UI event channel.
    pub(super) fn bind_log_sink(&self) {
        logging::forward::set_sink(self.event_tx.clone());
    }

    /// Forward queued events from the receiver to all connected clients.
    pub(super) async fn forward_events(&self, mut event_rx: Receiver<MsgToUI>) {
        while let Some(event) = event_rx.recv().await {
            if self.shutdown.load(Ordering::SeqCst) {
                break;
            }
            self.broadcast_event(event).await;
        }
    }

    /// Ensure the focus-change forwarder is running.
    pub(super) async fn ensure_world_forwarder(&self, world: Arc<dyn WorldView>) {
        if self.world_forwarder_running.swap(true, Ordering::SeqCst) {
            return;
        }
        let shutdown = self.shutdown.clone();
        let event_tx = self.event_tx.clone();
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
                if let Err(err) =
                    event_tx.try_send(MsgToUI::World(WorldStreamMsg::FocusChanged(app)))
                {
                    match err {
                        tokio::sync::mpsc::error::TrySendError::Full(_) => {}
                        tokio::sync::mpsc::error::TrySendError::Closed(_) => return,
                    }
                }
            }
        });
    }

    /// Start the single shared heartbeat loop if it is not already running.
    pub(super) async fn ensure_heartbeat(&self) {
        if self.hb_running.swap(true, Ordering::SeqCst) {
            return;
        }
        let pipeline = self.clone();
        tokio::spawn(async move {
            let interval = hotki_protocol::ipc::heartbeat::interval();
            loop {
                if pipeline.shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let ts = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as u64)
                    .unwrap_or(0);
                pipeline.broadcast_event(MsgToUI::Heartbeat(ts)).await;
                tokio::time::sleep(interval).await;
            }
            pipeline.hb_running.store(false, Ordering::SeqCst);
        });
    }

    async fn broadcast_event(&self, event: MsgToUI) {
        if self.shutdown.load(Ordering::SeqCst) {
            return;
        }
        let clients_snapshot = self.clients.lock().await.clone();
        let value = match enc_event(&event) {
            Ok(value) => value,
            Err(err) => {
                error!("Failed to encode event for broadcast: {}", err);
                return;
            }
        };

        let mut survivors = Vec::with_capacity(clients_snapshot.len());
        let mut sends = FuturesUnordered::new();
        for client in clients_snapshot {
            let value = value.clone();
            sends.push(async move {
                (
                    client.clone(),
                    client
                        .send_notification(
                            hotki_protocol::rpc::HotkeyNotification::Notify.as_str(),
                            slice::from_ref(&value),
                        )
                        .await,
                )
            });
        }
        while let Some((client, result)) = sends.next().await {
            match result {
                Ok(()) => survivors.push(client),
                Err(err) => warn!("Dropping disconnected client (send failed): {:?}", err),
            }
        }
        *self.clients.lock().await = survivors;
    }
}
