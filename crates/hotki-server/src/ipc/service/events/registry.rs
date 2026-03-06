use std::sync::Arc;

use mrpc::RpcSender;
use tokio::sync::Mutex as AsyncMutex;

/// Connected IPC clients tracked for event fanout.
#[derive(Clone, Default)]
pub(super) struct ClientRegistry {
    clients: Arc<AsyncMutex<Vec<RpcSender>>>,
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
        self.clients.lock().await.push(client);
    }

    /// Snapshot the current client list for concurrent broadcast.
    pub(super) async fn snapshot(&self) -> Vec<RpcSender> {
        self.clients.lock().await.clone()
    }

    /// Replace the client list after dropping disconnected peers.
    pub(super) async fn replace(&self, clients: Vec<RpcSender>) {
        *self.clients.lock().await = clients;
    }

    /// Clear all connected clients.
    pub(super) async fn clear(&self) {
        self.clients.lock().await.clear();
    }
}
