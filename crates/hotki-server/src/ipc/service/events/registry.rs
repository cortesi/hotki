use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use mrpc::RpcSender;
use tokio::sync::Mutex as AsyncMutex;

/// Tracked client with an incrementing unique identifier.
#[derive(Clone)]
pub(super) struct TrackedClient {
    pub(super) id: u32,
    pub(super) sender: RpcSender,
}

/// Connected IPC clients tracked for event fanout.
#[derive(Clone)]
pub(super) struct ClientRegistry {
    next_id: Arc<AtomicU32>,
    clients: Arc<AsyncMutex<Vec<TrackedClient>>>,
}

impl Default for ClientRegistry {
    fn default() -> Self {
        Self {
            next_id: Arc::new(AtomicU32::new(0)),
            clients: Arc::new(AsyncMutex::new(Vec::new())),
        }
    }
}

impl ClientRegistry {
    /// Create an empty client registry.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Return the number of connected clients.
    pub(super) async fn count(&self) -> usize {
        self.clients.lock().await.len()
    }

    /// Register a newly connected client.
    pub(super) async fn register(&self, client: RpcSender) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.clients
            .lock()
            .await
            .push(TrackedClient { id, sender: client });
    }

    /// Snapshot the current client list for concurrent broadcast.
    pub(super) async fn snapshot(&self) -> Vec<TrackedClient> {
        self.clients.lock().await.clone()
    }

    /// Remove a client by ID if it disconnected.
    pub(super) async fn remove(&self, id: u32) {
        self.clients.lock().await.retain(|c| c.id != id);
    }

    /// Clear all connected clients.
    pub(super) async fn clear(&self) {
        self.clients.lock().await.clear();
    }
}
