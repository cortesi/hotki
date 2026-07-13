//! Shared idempotent server shutdown coordination.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tokio::sync::Notify;

use crate::loop_wake::{self, WakeEvent};

/// Origin of a server shutdown request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShutdownReason {
    /// The shutdown RPC was invoked.
    Rpc,
    /// The UI parent process exited.
    ParentExited,
    /// The last-client idle deadline elapsed.
    IdleExpired,
    /// The IPC runtime could not start.
    IpcRuntimeFailed,
    /// The IPC server future ended.
    IpcStopped,
    /// Tao is destroying the event loop.
    EventLoopDestroyed,
    /// A test requested shutdown.
    #[cfg(test)]
    Test,
}

/// One shared shutdown transition for the Tao and Tokio server lanes.
#[derive(Clone)]
pub(crate) struct ShutdownCoordinator {
    requested: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl ShutdownCoordinator {
    /// Create an unset shutdown coordinator.
    pub(crate) fn new() -> Self {
        Self {
            requested: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Return the shared flag used by long-running worker loops.
    pub(crate) fn flag(&self) -> Arc<AtomicBool> {
        self.requested.clone()
    }

    /// Whether shutdown has already been requested.
    pub(crate) fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// Request shutdown once, then notify Tokio and wake Tao on every call.
    pub(crate) fn request(&self, reason: ShutdownReason) -> bool {
        let first = !self.requested.swap(true, Ordering::SeqCst);
        if first {
            tracing::debug!(?reason, "server shutdown requested");
        }
        self.notify.notify_waiters();
        let _ = loop_wake::post_user_event(WakeEvent::Shutdown);
        first
    }

    /// Wait until any owner requests shutdown without losing an early notification.
    pub(crate) async fn wait(&self) {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        let _ = notified.as_mut().enable();
        if self.is_requested() {
            return;
        }
        notified.await;
    }
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_transition_is_idempotent_and_wakes_waiters() {
        let shutdown = ShutdownCoordinator::new();
        let waiter = shutdown.clone();
        let task = tokio::spawn(async move {
            waiter.wait().await;
        });

        assert!(shutdown.request(ShutdownReason::Test));
        assert!(!shutdown.request(ShutdownReason::Test));
        task.await.expect("shutdown waiter");
        assert!(shutdown.is_requested());
    }

    #[tokio::test]
    async fn wait_observes_request_that_happened_first() {
        let shutdown = ShutdownCoordinator::new();
        shutdown.request(ShutdownReason::Test);

        shutdown.wait().await;
        assert!(shutdown.is_requested());
    }
}
