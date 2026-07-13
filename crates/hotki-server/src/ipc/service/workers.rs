use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use hotki_engine::Engine;
use hotki_protocol::MsgToUI;
use parking_lot::Mutex;
use tokio::sync::{
    Mutex as AsyncMutex, OnceCell,
    mpsc::{self, Sender, UnboundedSender},
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
    /// A saturated queue retained an ordered synthetic release.
    ReleaseQueued,
    /// Event could not be queued after replacing a closed worker.
    QueueClosed,
}

/// One ordered message in a worker generation.
#[derive(Debug)]
enum WorkerMessage {
    /// A physical hotkey event.
    Event(mac_hotkey::Event),
    /// A synthetic release retained when the logical queue was saturated.
    ForcedRelease { id: u32, generation: u64 },
}

/// Sender and logical-capacity state for one worker generation.
#[derive(Clone)]
struct WorkerHandle {
    generation: u64,
    tx: UnboundedSender<WorkerMessage>,
    pending: Arc<AtomicUsize>,
    release_pending: Arc<AtomicBool>,
    handoff: Arc<AsyncMutex<()>>,
}

/// Per-hotkey worker pool that preserves ordering for each hotkey ID.
#[derive(Clone)]
pub(super) struct WorkerPool {
    workers: Arc<Mutex<HashMap<u32, WorkerHandle>>>,
    next_generation: Arc<AtomicU64>,
    queue_capacity: usize,
    idle_timeout: Duration,
}

impl WorkerPool {
    /// Create a worker pool using the production queue and idle settings.
    pub(super) fn new() -> Self {
        Self {
            workers: Arc::new(Mutex::new(HashMap::new())),
            next_generation: Arc::new(AtomicU64::new(1)),
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
        runtime: impl Fn() -> WorkerRuntime,
    ) -> DispatchResult {
        self.dispatch_with(ev, |id, handoff| self.spawn_worker(id, runtime(), handoff))
    }

    fn dispatch_with(
        &self,
        ev: mac_hotkey::Event,
        mut spawn: impl FnMut(u32, Option<Arc<AsyncMutex<()>>>) -> WorkerHandle,
    ) -> DispatchResult {
        let id = ev.id;
        for attempt in 0..2 {
            let handle = {
                let mut workers = self.workers.lock();
                if let Some(handle) = workers.get(&id) {
                    handle.clone()
                } else {
                    let handle = spawn(id, None);
                    workers.insert(id, handle.clone());
                    handle
                }
            };

            if handle.tx.is_closed() {
                if attempt == 1 {
                    self.remove_generation(id, handle.generation);
                    return DispatchResult::QueueClosed;
                }
                self.replace_closed_generation(id, &handle, &mut spawn);
                continue;
            }

            let (message, queued_result) =
                if handle.pending.load(Ordering::SeqCst) >= self.queue_capacity {
                    if matches!(ev.kind, mac_hotkey::EventKind::KeyUp)
                        && !handle.release_pending.swap(true, Ordering::SeqCst)
                    {
                        (
                            WorkerMessage::ForcedRelease {
                                id,
                                generation: handle.generation,
                            },
                            DispatchResult::ReleaseQueued,
                        )
                    } else {
                        trace!(id, generation = handle.generation, "per_id_queue_full_drop");
                        return DispatchResult::QueueFull;
                    }
                } else {
                    (WorkerMessage::Event(ev.clone()), DispatchResult::Queued)
                };

            handle.pending.fetch_add(1, Ordering::SeqCst);
            if handle.tx.send(message).is_ok() {
                return queued_result;
            }

            handle.pending.fetch_sub(1, Ordering::SeqCst);
            if matches!(queued_result, DispatchResult::ReleaseQueued) {
                handle.release_pending.store(false, Ordering::SeqCst);
            }
            if attempt == 1 {
                self.remove_generation(id, handle.generation);
                return DispatchResult::QueueClosed;
            }
            self.replace_closed_generation(id, &handle, &mut spawn);
        }
        DispatchResult::QueueClosed
    }

    fn replace_closed_generation(
        &self,
        id: u32,
        closed: &WorkerHandle,
        spawn: &mut impl FnMut(u32, Option<Arc<AsyncMutex<()>>>) -> WorkerHandle,
    ) {
        let mut workers = self.workers.lock();
        let needs_replacement = workers
            .get(&id)
            .is_none_or(|current| current.generation == closed.generation);
        if needs_replacement {
            let replacement = spawn(id, Some(closed.handoff.clone()));
            workers.insert(id, replacement);
        }
    }

    fn spawn_worker(
        &self,
        id: u32,
        runtime: WorkerRuntime,
        handoff: Option<Arc<AsyncMutex<()>>>,
    ) -> WorkerHandle {
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let pending = Arc::new(AtomicUsize::new(0));
        let release_pending = Arc::new(AtomicBool::new(false));
        let handoff = handoff.unwrap_or_else(|| Arc::new(AsyncMutex::new(())));
        let handle = WorkerHandle {
            generation,
            tx,
            pending: pending.clone(),
            release_pending: release_pending.clone(),
            handoff: handoff.clone(),
        };
        let workers = self.workers.clone();
        let idle_timeout = self.idle_timeout;

        tokio::spawn(async move {
            let _generation_guard = handoff.lock_owned().await;
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
                    Ok(Some(message)) => {
                        process_worker_message(
                            eng,
                            generation,
                            pending.as_ref(),
                            release_pending.as_ref(),
                            message,
                        )
                        .await;
                    }
                    Ok(None) | Err(_) => break,
                }
            }

            // Reject late sends before abandoning this receiver. Messages
            // accepted before closure are drained under the generation gate,
            // so a replacement cannot overtake them.
            rx.close();
            while let Some(message) = rx.recv().await {
                process_worker_message(
                    eng,
                    generation,
                    pending.as_ref(),
                    release_pending.as_ref(),
                    message,
                )
                .await;
            }
            remove_generation(&workers, id, generation);
        });
        handle
    }

    fn remove_generation(&self, id: u32, generation: u64) {
        remove_generation(&self.workers, id, generation);
    }
}

impl Default for WorkerPool {
    fn default() -> Self {
        Self::new()
    }
}

fn remove_generation(workers: &Arc<Mutex<HashMap<u32, WorkerHandle>>>, id: u32, generation: u64) {
    let mut guard = workers.lock();
    if let Some(current) = guard.get(&id)
        && current.generation == generation
    {
        guard.remove(&id);
    }
}

async fn process_worker_message(
    engine: &Engine,
    generation: u64,
    pending: &AtomicUsize,
    release_pending: &AtomicBool,
    message: WorkerMessage,
) {
    let (event_id, forced_release, result) = match message {
        WorkerMessage::Event(ev) => {
            let id = ev.id;
            (id, false, engine.dispatch(id, ev.kind, ev.repeat).await)
        }
        WorkerMessage::ForcedRelease {
            id,
            generation: message_generation,
        } => {
            debug_assert_eq!(generation, message_generation);
            (
                id,
                true,
                engine
                    .dispatch(id, mac_hotkey::EventKind::KeyUp, false)
                    .await,
            )
        }
    };
    pending.fetch_sub(1, Ordering::SeqCst);
    if forced_release {
        release_pending.store(false, Ordering::SeqCst);
    }
    if let Err(err) = result {
        trace!(
            target: "hotki_server::ipc::service",
            "ordered worker dispatch failed id={}: {}",
            event_id,
            err
        );
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
            next_generation: Arc::new(AtomicU64::new(1)),
            queue_capacity: 1,
            idle_timeout: Duration::from_secs(5),
        }
    }

    fn event_with_kind(id: u32, key: Key, kind: mac_hotkey::EventKind) -> mac_hotkey::Event {
        mac_hotkey::Event {
            id,
            hotkey: mac_keycode::Chord {
                key,
                modifiers: HashSet::new(),
            },
            kind,
            repeat: false,
        }
    }

    fn event(id: u32, key: Key) -> mac_hotkey::Event {
        event_with_kind(id, key, mac_hotkey::EventKind::KeyDown)
    }

    fn detached_handle(
        generation: u64,
        pending: usize,
    ) -> (WorkerHandle, mpsc::UnboundedReceiver<WorkerMessage>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            WorkerHandle {
                generation,
                tx,
                pending: Arc::new(AtomicUsize::new(pending)),
                release_pending: Arc::new(AtomicBool::new(false)),
                handoff: Arc::new(AsyncMutex::new(())),
            },
            rx,
        )
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
            pool.dispatch(event(42, Key::A), || runtime.clone()),
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
            pool.dispatch(event(42, Key::A), || runtime.clone()),
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
            pool.dispatch(event(42, Key::A), || runtime.clone()),
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
            pool.dispatch(event(102, Key::B), || runtime.clone()),
            DispatchResult::Queued
        );
        assert_eq!(pool.active_count(), 2);

        advance_worker_idle_timeout().await;
        assert_eq!(pool.active_count(), 0);
    }

    #[test]
    fn removal_preserves_new_worker_generation() {
        let pool = WorkerPool::new();

        let (first, _first_rx) = detached_handle(1, 0);
        let (replacement, _replacement_rx) = detached_handle(2, 0);
        pool.workers.lock().insert(999, first);
        pool.workers.lock().insert(999, replacement);

        pool.remove_generation(999, 1);

        assert_eq!(pool.active_count(), 1);
        assert_eq!(pool.workers.lock().get(&999).unwrap().generation, 2);
    }

    #[test]
    fn dispatch_reports_full_worker_queue() {
        let pool = pool();
        let (handle, _rx) = detached_handle(7, 1);
        pool.workers.lock().insert(7, handle);

        assert_eq!(
            pool.dispatch(event(7, Key::B), || panic!("worker already exists")),
            DispatchResult::QueueFull
        );
        assert_eq!(pool.active_count(), 1);
    }

    #[test]
    fn retiring_worker_drains_accepted_events_before_replacement_handoff() {
        let pool = pool();
        let (handle, mut old_rx) = detached_handle(77, 0);
        let old_handoff = handle.handoff.clone();
        let old_guard = old_handoff
            .clone()
            .try_lock_owned()
            .expect("old generation owns handoff");
        pool.workers.lock().insert(7, handle);

        assert_eq!(
            pool.dispatch(event(7, Key::A), || panic!("worker already exists")),
            DispatchResult::Queued
        );
        old_rx.close();

        let (replacement, mut replacement_rx) = detached_handle(88, 0);
        let mut replacement = Some(replacement);

        assert_eq!(
            pool.dispatch_with(event(7, Key::B), |_, handoff| {
                let handoff = handoff.expect("replacement inherits generation handoff");
                assert!(Arc::ptr_eq(&handoff, &old_handoff));
                let mut replacement = replacement.take().expect("one replacement worker");
                replacement.handoff = handoff;
                replacement
            }),
            DispatchResult::Queued
        );
        assert_eq!(pool.workers.lock().get(&7).unwrap().generation, 88);
        assert!(old_handoff.try_lock().is_err());
        assert!(matches!(
            old_rx.try_recv(),
            Ok(WorkerMessage::Event(mac_hotkey::Event { id: 7, .. }))
        ));
        assert!(matches!(
            old_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        ));
        assert!(matches!(
            replacement_rx.try_recv(),
            Ok(WorkerMessage::Event(mac_hotkey::Event { id: 7, .. }))
        ));
        drop(old_guard);
        assert!(old_handoff.try_lock().is_ok());
    }

    #[test]
    fn saturated_release_stays_ordered_in_one_generation() {
        let pool = pool();
        let (handle, mut rx) = detached_handle(7, 0);
        let release_pending = handle.release_pending.clone();
        pool.workers.lock().insert(7, handle);

        assert_eq!(
            pool.dispatch(event(7, Key::A), || panic!("worker already exists")),
            DispatchResult::Queued
        );
        assert_eq!(
            pool.dispatch(
                event_with_kind(7, Key::A, mac_hotkey::EventKind::KeyUp),
                || panic!("worker already exists")
            ),
            DispatchResult::ReleaseQueued
        );
        assert!(release_pending.load(Ordering::SeqCst));
        assert_eq!(
            pool.dispatch(
                event_with_kind(7, Key::A, mac_hotkey::EventKind::KeyUp),
                || panic!("worker already exists")
            ),
            DispatchResult::QueueFull
        );

        let first = rx.try_recv().expect("physical event queued first");
        assert!(matches!(
            first,
            WorkerMessage::Event(mac_hotkey::Event {
                kind: mac_hotkey::EventKind::KeyDown,
                ..
            })
        ));
        let second = rx.try_recv().expect("synthetic release queued second");
        assert!(matches!(
            second,
            WorkerMessage::ForcedRelease {
                id: 7,
                generation: 7
            }
        ));
    }
}
