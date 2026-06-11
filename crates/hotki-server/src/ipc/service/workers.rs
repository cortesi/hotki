use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use hotki_engine::Engine;
use hotki_protocol::MsgToUI;
use parking_lot::Mutex;
use tokio::sync::{
    OnceCell,
    mpsc::{self, Sender},
};
use tracing::trace;

const DEFAULT_QUEUE_CAPACITY: usize = 64;
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// Runtime handles needed by a worker task to lazily initialize and dispatch through the engine.
#[derive(Clone)]
pub(super) struct WorkerRuntime {
    engine: Arc<OnceCell<Engine>>,
    manager: Arc<mac_hotkey::Manager>,
    event_tx: Sender<MsgToUI>,
    shutdown: Arc<AtomicBool>,
}

impl WorkerRuntime {
    /// Build runtime handles for newly spawned worker tasks.
    pub(super) fn new(
        engine: Arc<OnceCell<Engine>>,
        manager: Arc<mac_hotkey::Manager>,
        event_tx: Sender<MsgToUI>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            engine,
            manager,
            event_tx,
            shutdown,
        }
    }
}

/// Result of queueing an event for a per-ID worker.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum DispatchResult {
    /// Event was queued for dispatch.
    Queued,
    /// Event was dropped because the worker queue was full.
    QueueFull,
    /// Event was dropped because the worker channel had already closed.
    QueueClosed,
}

/// Per-hotkey worker pool that preserves ordering for each hotkey ID.
#[derive(Clone)]
pub(super) struct WorkerPool {
    workers: Arc<Mutex<HashMap<u32, Sender<mac_hotkey::Event>>>>,
    queue_capacity: usize,
    idle_timeout: Duration,
}

impl WorkerPool {
    /// Create a worker pool using the production queue and idle settings.
    pub(super) fn new() -> Self {
        Self {
            workers: Arc::new(Mutex::new(HashMap::new())),
            queue_capacity: DEFAULT_QUEUE_CAPACITY,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        }
    }

    /// Return the number of active per-ID workers.
    #[cfg(test)]
    pub(super) fn active_count(&self) -> usize {
        self.workers.lock().len()
    }

    /// Dispatch an event to its per-ID worker, spawning that worker if needed.
    pub(super) fn dispatch(
        &self,
        ev: mac_hotkey::Event,
        runtime: impl FnOnce() -> WorkerRuntime,
    ) -> DispatchResult {
        let id = ev.id;
        let mut workers = self.workers.lock();

        let tx = if let Some(tx) = workers.get(&id) {
            tx.clone()
        } else {
            let (tx, rx) = mpsc::channel(self.queue_capacity);
            workers.insert(id, tx.clone());
            self.spawn_worker(id, rx, tx.clone(), runtime());
            tx
        };

        drop(workers);

        match tx.try_send(ev) {
            Ok(()) => DispatchResult::Queued,
            Err(mpsc::error::TrySendError::Full(_)) => {
                trace!(id, "per_id_queue_full_drop");
                DispatchResult::QueueFull
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.remove_if_same(id, &tx);
                DispatchResult::QueueClosed
            }
        }
    }

    fn spawn_worker(
        &self,
        id: u32,
        mut rx: mpsc::Receiver<mac_hotkey::Event>,
        tx: Sender<mac_hotkey::Event>,
        runtime: WorkerRuntime,
    ) {
        let workers = self.workers.clone();
        let idle_timeout = self.idle_timeout;

        tokio::spawn(async move {
            let eng = runtime
                .engine
                .get_or_init(|| async {
                    Engine::new(runtime.manager.clone(), runtime.event_tx.clone())
                })
                .await;

            loop {
                if runtime.shutdown.load(Ordering::SeqCst) {
                    break;
                }

                match tokio::time::timeout(idle_timeout, rx.recv()).await {
                    Ok(Some(ev)) => {
                        if let Err(err) = eng.dispatch(ev.id, ev.kind, ev.repeat).await {
                            trace!(
                                target: "hotki_server::ipc::service",
                                "OS dispatch failed id={} kind={:?}: {}",
                                ev.id,
                                ev.kind,
                                err
                            );
                        }
                    }
                    Ok(None) | Err(_) => break,
                }
            }

            remove_if_same(&workers, id, &tx);
        });
    }

    fn remove_if_same(&self, id: u32, tx: &Sender<mac_hotkey::Event>) {
        remove_if_same(&self.workers, id, tx);
    }
}

impl Default for WorkerPool {
    fn default() -> Self {
        Self::new()
    }
}

fn remove_if_same(
    workers: &Arc<Mutex<HashMap<u32, Sender<mac_hotkey::Event>>>>,
    id: u32,
    tx: &Sender<mac_hotkey::Event>,
) {
    let mut guard = workers.lock();
    if let Some(current_tx) = guard.get(&id)
        && current_tx.same_channel(tx)
    {
        guard.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use mac_keycode::Key;
    use tokio::{sync::OnceCell, time::advance};
    use tracing::warn;

    use super::*;

    fn setup_runtime() -> Option<WorkerRuntime> {
        let manager = match mac_hotkey::Manager::new() {
            Ok(manager) => Arc::new(manager),
            Err(err) => {
                warn!(
                    "Skipping test: mac_hotkey::Manager failed to initialize: {:?}",
                    err
                );
                return None;
            }
        };
        let (event_tx, _event_rx) = hotki_protocol::ipc::ui_channel();
        Some(WorkerRuntime::new(
            Arc::new(OnceCell::new()),
            manager,
            event_tx,
            Arc::new(AtomicBool::new(false)),
        ))
    }

    fn pool() -> WorkerPool {
        WorkerPool {
            workers: Arc::new(Mutex::new(HashMap::new())),
            queue_capacity: 1,
            idle_timeout: Duration::from_secs(5),
        }
    }

    fn event(id: u32, key: Key) -> mac_hotkey::Event {
        mac_hotkey::Event {
            id,
            hotkey: mac_keycode::Chord {
                key,
                modifiers: HashSet::new(),
            },
            kind: mac_hotkey::EventKind::KeyDown,
            repeat: false,
        }
    }

    async fn advance_worker_idle_timeout() {
        tokio::task::yield_now().await;
        advance(DEFAULT_IDLE_TIMEOUT).await;
        tokio::task::yield_now().await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn worker_reaps_after_idle_timeout() {
        let Some(runtime) = setup_runtime() else {
            return;
        };
        let pool = WorkerPool::new();

        assert_eq!(pool.active_count(), 0);
        assert_eq!(
            pool.dispatch(event(42, Key::A), || runtime),
            DispatchResult::Queued
        );
        assert_eq!(pool.active_count(), 1);

        advance_worker_idle_timeout().await;
        assert_eq!(pool.active_count(), 0);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn worker_reactivates_after_idle_reap() {
        let Some(runtime) = setup_runtime() else {
            return;
        };
        let pool = WorkerPool::new();

        assert_eq!(
            pool.dispatch(event(42, Key::A), || runtime.clone()),
            DispatchResult::Queued
        );
        assert_eq!(pool.active_count(), 1);

        advance_worker_idle_timeout().await;
        assert_eq!(pool.active_count(), 0);

        assert_eq!(
            pool.dispatch(event(42, Key::A), || runtime),
            DispatchResult::Queued
        );
        assert_eq!(pool.active_count(), 1);

        advance_worker_idle_timeout().await;
        assert_eq!(pool.active_count(), 0);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn worker_reaps_on_shutdown() {
        let Some(runtime) = setup_runtime() else {
            return;
        };
        let shutdown = runtime.shutdown.clone();
        let pool = WorkerPool::new();

        assert_eq!(
            pool.dispatch(event(42, Key::A), || runtime.clone()),
            DispatchResult::Queued
        );
        assert_eq!(pool.active_count(), 1);

        shutdown.store(true, Ordering::SeqCst);
        assert_eq!(
            pool.dispatch(event(42, Key::A), || runtime),
            DispatchResult::Queued
        );

        tokio::task::yield_now().await;
        assert_eq!(pool.active_count(), 0);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn workers_are_isolated_by_hotkey_id() {
        let Some(runtime) = setup_runtime() else {
            return;
        };
        let pool = WorkerPool::new();

        assert_eq!(
            pool.dispatch(event(101, Key::A), || runtime.clone()),
            DispatchResult::Queued
        );
        assert_eq!(pool.active_count(), 1);
        assert_eq!(
            pool.dispatch(event(102, Key::B), || runtime),
            DispatchResult::Queued
        );
        assert_eq!(pool.active_count(), 2);

        advance_worker_idle_timeout().await;
        assert_eq!(pool.active_count(), 0);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn reaping_preserves_new_worker_for_same_id() {
        let Some(runtime) = setup_runtime() else {
            return;
        };
        let pool = WorkerPool::new();

        assert_eq!(
            pool.dispatch(event(999, Key::A), || runtime),
            DispatchResult::Queued
        );
        assert_eq!(pool.active_count(), 1);

        let (tx_b, _rx_b) = mpsc::channel(DEFAULT_QUEUE_CAPACITY);
        pool.workers.lock().insert(999, tx_b);

        advance_worker_idle_timeout().await;

        assert_eq!(pool.active_count(), 1);
        assert!(pool.workers.lock().contains_key(&999));
    }

    #[tokio::test]
    async fn dispatch_reports_full_worker_queue() {
        let pool = pool();
        let (tx, _rx) = mpsc::channel(1);
        tx.try_send(event(7, Key::A)).expect("prefill queue");
        pool.workers.lock().insert(7, tx);

        assert_eq!(
            pool.dispatch(event(7, Key::B), || panic!("worker already exists")),
            DispatchResult::QueueFull
        );
        assert_eq!(pool.active_count(), 1);
    }

    #[tokio::test]
    async fn dispatch_reports_and_removes_closed_worker_queue() {
        let pool = pool();
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        pool.workers.lock().insert(7, tx);

        assert_eq!(
            pool.dispatch(event(7, Key::B), || panic!("worker already exists")),
            DispatchResult::QueueClosed
        );
        assert_eq!(pool.active_count(), 0);
    }
}
