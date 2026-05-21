use std::sync::Arc;

use mrpc::RpcSender;
use tokio::sync::Mutex as AsyncMutex;

/// Tracked client with an incrementing unique identifier.
#[derive(Clone)]
pub(super) struct TrackedClient {
    pub(super) id: u32,
    pub(super) sender: RpcSender,
}

/// Connected IPC clients tracked for event fanout.
#[derive(Clone, Default)]
pub(super) struct ClientRegistry {
    next_id: Arc<AsyncMutex<u32>>,
    clients: Arc<AsyncMutex<Vec<TrackedClient>>>,
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
        let mut next_id_guard = self.next_id.lock().await;
        let id = *next_id_guard;
        *next_id_guard += 1;
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
