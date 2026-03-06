use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

mod broadcaster;
mod registry;
mod sources;

use hotki_protocol::MsgToUI;
use hotki_world::WorldView;
use parking_lot::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};

use self::{broadcaster::broadcast_event, registry::ClientRegistry, sources::*};

/// Shared event pipeline for broadcasting UI events to connected clients.
#[derive(Clone)]
pub(super) struct EventPipeline {
    /// Event sender for UI messages (bounded).
    event_tx: Sender<MsgToUI>,
    /// Event receiver, taken by the first connection that starts forwarding.
    event_rx: Arc<Mutex<Option<Receiver<MsgToUI>>>>,
    /// Connected clients.
    registry: ClientRegistry,
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
            registry: ClientRegistry::new(),
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
        self.registry.count().await
    }

    /// Register a new connected client.
    pub(super) async fn register_client(&self, client: mrpc::RpcSender) {
        self.registry.register(client).await;
    }

    /// Take ownership of the event receiver so forwarding starts exactly once.
    pub(super) fn take_event_rx(&self) -> Option<Receiver<MsgToUI>> {
        self.event_rx.lock().take()
    }

    /// Clear clients and close the local event pipeline for shutdown.
    pub(super) async fn clear_for_shutdown(&self) {
        logging::forward::clear_sink();
        self.registry.clear().await;
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
            broadcast_event(&self.registry, &self.shutdown, event).await;
        }
    }

    /// Ensure the focus-change forwarder is running.
    pub(super) async fn ensure_world_forwarder(&self, world: Arc<dyn WorldView>) {
        if self.world_forwarder_running.swap(true, Ordering::SeqCst) {
            return;
        }
        spawn_world_forwarder(self.shutdown.clone(), self.event_tx.clone(), world);
    }

    /// Start the single shared heartbeat loop if it is not already running.
    pub(super) async fn ensure_heartbeat(&self) {
        if self.hb_running.swap(true, Ordering::SeqCst) {
            return;
        }
        spawn_heartbeat(
            self.shutdown.clone(),
            self.event_tx.clone(),
            self.hb_running.clone(),
        );
    }
}
