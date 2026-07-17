use std::{
    mem,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

mod broadcaster;
mod registry;
mod sources;

use hotki_protocol::MsgToUI;
use hotki_world::WorldView;
use tokio::{
    select,
    sync::{
        Mutex,
        mpsc::{Receiver, Sender},
        watch,
    },
    task::JoinHandle,
};

use self::{
    broadcaster::broadcast_event,
    registry::ClientRegistry,
    sources::{broadcast_heartbeats, forward_world_events},
};

/// Owned tasks and cancellation signal for one running event pipeline.
struct PipelineTasks {
    /// Wakes every task during explicit pipeline shutdown.
    cancel: watch::Sender<bool>,
    /// Queued UI event fanout task.
    fanout: JoinHandle<()>,
    /// World focus forwarding task.
    world: JoinHandle<()>,
    /// Direct heartbeat broadcasting task.
    heartbeat: JoinHandle<()>,
}

impl PipelineTasks {
    /// Cancel every task and await its completion.
    async fn shutdown(self) {
        let _ = self.cancel.send(true);
        join_task("fanout", self.fanout).await;
        join_task("world", self.world).await;
        join_task("heartbeat", self.heartbeat).await;
    }
}

/// Await an owned pipeline task and report unexpected task failure.
async fn join_task(name: &str, task: JoinHandle<()>) {
    if let Err(error) = task.await {
        tracing::warn!(task = name, ?error, "event_pipeline_task_failed");
    }
}

/// Lifecycle of the single event-pipeline task group.
enum PipelineLifecycle {
    /// Receiver retained until the lazy engine exposes its world.
    Ready(Receiver<MsgToUI>),
    /// All event sources and forwarding tasks are running.
    Running(PipelineTasks),
    /// Pipeline has been shut down and cannot restart.
    Stopped,
}

/// Shared event pipeline for broadcasting UI events to connected clients.
#[derive(Clone)]
pub(super) struct EventPipeline {
    /// Event sender for UI messages (bounded).
    event_tx: Sender<MsgToUI>,
    /// Connected clients.
    registry: ClientRegistry,
    /// Global shutdown flag shared with the Tao loop.
    shutdown: Arc<AtomicBool>,
    /// Manager sampled by the single heartbeat source.
    manager: Arc<mac_hotkey::Manager>,
    /// Receiver and owned task group for the one pipeline run.
    lifecycle: Arc<Mutex<PipelineLifecycle>>,
}

impl EventPipeline {
    /// Create a new event pipeline with a fresh UI message channel.
    pub(super) fn new(shutdown: Arc<AtomicBool>, manager: Arc<mac_hotkey::Manager>) -> Self {
        let (event_tx, event_rx) = hotki_protocol::ipc::ui_channel();
        Self {
            event_tx,
            registry: ClientRegistry::new(),
            shutdown,
            manager,
            lifecycle: Arc::new(Mutex::new(PipelineLifecycle::Ready(event_rx))),
        }
    }

    /// Clone the sender used by the engine and logging pipeline.
    pub(super) fn sender(&self) -> Sender<MsgToUI> {
        self.event_tx.clone()
    }

    /// Return the number of connected clients.
    pub(super) async fn client_count(&self) -> usize {
        self.registry.count().await
    }

    /// Register a new connected client.
    pub(super) async fn register_client(&self, client: mrpc::RpcSender) {
        self.registry.register(client).await;
    }

    /// Start the one owned task group after the lazy engine exposes its world.
    ///
    /// Returns true only for the first successful start.
    pub(super) async fn start(&self, world: Arc<dyn WorldView>) -> bool {
        let mut lifecycle = self.lifecycle.lock().await;
        let event_rx = match mem::replace(&mut *lifecycle, PipelineLifecycle::Stopped) {
            PipelineLifecycle::Ready(event_rx) => event_rx,
            state => {
                *lifecycle = state;
                return false;
            }
        };

        logging::forward::set_sink(self.event_tx.clone());
        let (cancel, _) = watch::channel(false);
        let fanout = tokio::spawn(forward_events(
            event_rx,
            self.registry.clone(),
            self.shutdown.clone(),
            cancel.subscribe(),
        ));
        let cursor = world.subscribe();
        let world = tokio::spawn(forward_world_events(
            self.shutdown.clone(),
            self.event_tx.clone(),
            world,
            cursor,
            cancel.subscribe(),
        ));
        let heartbeat = tokio::spawn(broadcast_heartbeats(
            self.shutdown.clone(),
            self.registry.clone(),
            self.manager.clone(),
            cancel.subscribe(),
        ));
        *lifecycle = PipelineLifecycle::Running(PipelineTasks {
            cancel,
            fanout,
            world,
            heartbeat,
        });
        true
    }

    /// Clear clients, close event sources, and join the owned task group.
    pub(super) async fn shutdown(&self) {
        logging::forward::clear_sink();
        self.registry.clear().await;
        let tasks = {
            let mut lifecycle = self.lifecycle.lock().await;
            match mem::replace(&mut *lifecycle, PipelineLifecycle::Stopped) {
                PipelineLifecycle::Running(tasks) => Some(tasks),
                PipelineLifecycle::Ready(event_rx) => {
                    drop(event_rx);
                    None
                }
                PipelineLifecycle::Stopped => None,
            }
        };
        if let Some(tasks) = tasks {
            tasks.shutdown().await;
        }
    }
}

/// Forward queued UI events until shutdown, cancellation, or sender closure.
async fn forward_events(
    mut event_rx: Receiver<MsgToUI>,
    registry: ClientRegistry,
    shutdown: Arc<AtomicBool>,
    mut cancel: watch::Receiver<bool>,
) {
    loop {
        let event = select! {
            _ = cancel.changed() => break,
            event = event_rx.recv() => event,
        };
        let Some(event) = event else {
            break;
        };
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        broadcast_event(&registry, &shutdown, event).await;
    }
}

#[cfg(test)]
mod tests {
    use hotki_world::{FocusChange, TestWorld, WorldEvent, WorldView};

    use super::*;

    fn test_pipeline() -> EventPipeline {
        EventPipeline::new(
            Arc::new(AtomicBool::new(false)),
            Arc::new(mac_hotkey::Manager::without_event_tap()),
        )
    }

    #[tokio::test]
    async fn pipeline_starts_one_task_group_and_joins_it_on_shutdown() {
        let pipeline = test_pipeline();
        let world = Arc::new(TestWorld::new());

        assert!(pipeline.start(world.clone()).await);
        assert!(!pipeline.start(world).await);
        {
            let lifecycle = pipeline.lifecycle.lock().await;
            let PipelineLifecycle::Running(tasks) = &*lifecycle else {
                panic!("pipeline did not retain its running task group");
            };
            assert!(!tasks.fanout.is_finished());
            assert!(!tasks.world.is_finished());
            assert!(!tasks.heartbeat.is_finished());
        }

        pipeline.shutdown().await;

        assert!(matches!(
            &*pipeline.lifecycle.lock().await,
            PipelineLifecycle::Stopped
        ));
    }

    #[tokio::test]
    async fn queued_event_forwarder_finishes_when_its_sender_closes() {
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(1);
        let (_cancel, cancel_rx) = watch::channel(false);
        drop(event_tx);

        forward_events(
            event_rx,
            ClientRegistry::new(),
            Arc::new(AtomicBool::new(false)),
            cancel_rx,
        )
        .await;
    }

    #[tokio::test]
    async fn world_forwarder_finishes_when_the_event_queue_closes() {
        let world = Arc::new(TestWorld::new());
        let cursor = world.subscribe();
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(1);
        let (_cancel, cancel_rx) = watch::channel(false);
        drop(event_rx);
        let task = tokio::spawn(forward_world_events(
            Arc::new(AtomicBool::new(false)),
            event_tx,
            world.clone(),
            cursor,
            cancel_rx,
        ));

        world.push_event(WorldEvent::FocusChanged(FocusChange::Cleared));

        task.await.expect("world forwarder task");
    }
}
