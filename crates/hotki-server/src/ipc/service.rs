//! IPC service implementation for hotkey manager
//!
//! World read path
//! - `hotki-world` is authoritative for window/focus state. The service
//!   ensures a single forwarder instance per server and relays `WorldEvent`s
//!   to the UI stream, with snapshot-on-reconnect semantics.
//! - There are no CoreGraphics/AX focus fallbacks in the engine; actions rely
//!   on the world cache and nudge refresh via `hint_refresh()`.
//!
//! # Locking Strategy
//!
//! - Prefer Tokio locks inside async paths. The `clients` list uses
//!   `tokio::sync::Mutex` to avoid mixing where we `await` soon after.
//! - Use short-lived sync locks only at the edges (e.g., `event_tx`/`event_rx`),
//!   and release them before any `.await` or blocking work.
//! - Never hold any lock across network or file I/O; clone snapshots first.

mod events;
mod rpc;

use std::{
    collections::HashMap,
    path::PathBuf,
    result::Result as StdResult,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use events::EventPipeline;
use hotki_engine::Engine;
use hotki_protocol::rpc::{HotkeyMethod, ServerStatusLite};
use mrpc::{Connection as MrpcConnection, RpcError, RpcSender, Value};
use parking_lot::Mutex;
pub(crate) use rpc::dec_inject_key_param;
use rpc::{
    build_snapshot_payload, enc_server_status, enc_world_snapshot, enc_world_status,
    inject_kind_to_event, string_param, typed_err,
};
use tokio::sync::OnceCell;
use tracing::{debug, info, trace, warn};

use super::{IdleTimerSnapshot, IdleTimerState};
use crate::loop_wake;

/// IPC service that handles hotkey manager operations
#[derive(Clone)]
pub struct HotkeyService {
    /// The hotkey engine
    engine: Arc<OnceCell<Engine>>,
    /// Mac hotkey manager
    manager: Arc<mac_hotkey::Manager>,
    /// Event pipeline shared across client registration, broadcast, and forwarding tasks.
    events: EventPipeline,
    /// Shared idle timer state for status reporting.
    idle_state: Arc<IdleTimerState>,
    /// Notify handle for server shutdown.
    shutdown_notify: Arc<tokio::sync::Notify>,
    /// Active worker channels per hotkey ID.
    workers: Arc<Mutex<HashMap<u32, tokio::sync::mpsc::Sender<mac_hotkey::Event>>>>,
}

impl HotkeyService {
    pub fn new(
        manager: Arc<mac_hotkey::Manager>,
        shutdown: Arc<AtomicBool>,
        shutdown_notify: Arc<tokio::sync::Notify>,
        idle_state: Arc<IdleTimerState>,
    ) -> Self {
        Self {
            engine: Arc::new(OnceCell::new()),
            manager,
            events: EventPipeline::new(shutdown),
            idle_state,
            shutdown_notify,
            workers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Expose the shutdown flag for coordinated server shutdown.
    pub(crate) fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.events.shutdown_flag()
    }

    /// Expose the shutdown notify handle.
    pub(crate) fn shutdown_notify(&self) -> Arc<tokio::sync::Notify> {
        self.shutdown_notify.clone()
    }

    /// Expose the active worker count for diagnostics and testing.
    #[cfg(test)]
    pub(crate) fn active_workers_count(&self) -> usize {
        self.workers.lock().len()
    }

    async fn engine(&self) -> &Engine {
        self.engine
            .get_or_init(|| async { Engine::new(self.manager.clone(), self.events.sender()) })
            .await
    }

    /// Gather a lightweight server status snapshot for diagnostics.
    async fn snapshot_server_status(&self) -> ServerStatusLite {
        let clients_connected = self.events.client_count().await;
        let IdleTimerSnapshot {
            timeout_secs,
            armed,
            deadline_ms,
        } = self.idle_state.snapshot();
        ServerStatusLite {
            idle_timeout_secs: timeout_secs,
            idle_timer_armed: armed,
            idle_deadline_ms: deadline_ms,
            clients_connected,
        }
    }

    async fn handle_shutdown_request(&self) -> StdResult<Value, RpcError> {
        info!("Shutdown request received");
        self.shutdown_flag().store(true, Ordering::SeqCst);
        let _ = loop_wake::post_user_event();
        self.events.clear_for_shutdown().await;
        self.shutdown_notify.notify_waiters();
        Ok(Value::Boolean(true))
    }

    async fn handle_set_config_path(&self, params: &[Value]) -> StdResult<Value, RpcError> {
        let raw_path = string_param(
            params,
            HotkeyMethod::SetConfigPath.as_str(),
            "path",
            crate::error::RpcErrorCode::MissingParams,
        )?;
        let engine = self.engine().await;
        if let Err(err) = engine.set_config_path(PathBuf::from(raw_path)).await {
            return Err(typed_err(
                crate::error::RpcErrorCode::EngineSetConfig,
                &[("message", Value::String(err.to_string().into()))],
            ));
        }
        Ok(Value::Boolean(true))
    }

    async fn handle_set_theme(&self, params: &[Value]) -> StdResult<Value, RpcError> {
        let raw_name = string_param(
            params,
            HotkeyMethod::SetTheme.as_str(),
            "theme name",
            crate::error::RpcErrorCode::MissingParams,
        )?;
        let engine = self.engine().await;
        if let Err(err) = engine.set_theme(raw_name.as_str()).await {
            return Err(typed_err(
                crate::error::RpcErrorCode::EngineSetConfig,
                &[("message", Value::String(err.to_string().into()))],
            ));
        }
        Ok(Value::Boolean(true))
    }

    async fn handle_inject_key(&self, params: &[Value]) -> StdResult<Value, RpcError> {
        if params.is_empty() {
            return Err(typed_err(
                crate::error::RpcErrorCode::MissingParams,
                &[("expected", Value::String("inject request".into()))],
            ));
        }
        let req = dec_inject_key_param(&params[0])?;
        tracing::debug!(
            target: "hotki_server::ipc::service",
            "InjectKey: ident={} kind={:?} repeat={}",
            req.ident,
            req.kind,
            req.repeat
        );

        let engine = self.engine().await;
        let Some(id) = engine.resolve_id_for_ident(&req.ident).await else {
            tracing::debug!(
                target: "hotki_server::ipc::service",
                "InjectKey: ident not bound: {}",
                req.ident
            );
            return Err(typed_err(
                crate::error::RpcErrorCode::KeyNotBound,
                &[("ident", Value::String(req.ident.into()))],
            ));
        };
        tracing::debug!(
            target: "hotki_server::ipc::service",
            "InjectKey: resolved id={} for ident={} -> dispatch",
            id,
            req.ident
        );

        match engine
            .dispatch(id, inject_kind_to_event(req.kind), req.repeat)
            .await
        {
            Ok(()) => Ok(Value::Boolean(true)),
            Err(err) => {
                tracing::warn!(
                    target: "hotki_server::ipc::service",
                    "InjectKey: dispatch failed id={} ident={}: {}",
                    id,
                    req.ident,
                    err
                );
                Err(typed_err(
                    crate::error::RpcErrorCode::EngineDispatch,
                    &[("message", Value::String(err.to_string().into()))],
                ))
            }
        }
    }

    async fn handle_get_bindings(&self) -> StdResult<Value, RpcError> {
        let engine = self.engine().await;
        let mut idents: Vec<String> = engine
            .bindings_snapshot()
            .await
            .into_iter()
            .map(|(ident, _)| ident)
            .collect();
        idents.sort();
        Ok(Value::Array(
            idents
                .into_iter()
                .map(|ident| Value::String(ident.into()))
                .collect(),
        ))
    }

    async fn handle_get_depth(&self) -> StdResult<Value, RpcError> {
        let engine = self.engine().await;
        Ok(Value::Integer((engine.get_depth().await as u64).into()))
    }

    async fn handle_get_world_status(&self) -> StdResult<Value, RpcError> {
        let engine = self.engine().await;
        Ok(enc_world_status(&engine.world_status().await))
    }

    async fn handle_get_world_snapshot(&self) -> StdResult<Value, RpcError> {
        let engine = self.engine().await;
        let world = engine.world();
        let displays = world.displays().await;
        let focused_app = hotki_world::focused_snapshot(world.as_ref()).await;
        let payload = build_snapshot_payload(displays, focused_app);
        enc_world_snapshot(&payload).map_err(|err| {
            typed_err(
                crate::error::RpcErrorCode::InvalidType,
                &[("message", Value::String(err.to_string().into()))],
            )
        })
    }

    async fn handle_get_server_status(&self) -> StdResult<Value, RpcError> {
        enc_server_status(&self.snapshot_server_status().await).map_err(|err| {
            typed_err(
                crate::error::RpcErrorCode::InvalidType,
                &[("message", Value::String(err.to_string().into()))],
            )
        })
    }
}

#[async_trait]
impl MrpcConnection for HotkeyService {
    async fn connected(&self, client: RpcSender) -> StdResult<(), RpcError> {
        if self.shutdown_flag().load(Ordering::SeqCst) {
            // Refuse new connections during shutdown
            return Err(typed_err(
                crate::error::RpcErrorCode::ShuttingDown,
                &[("message", Value::String("Server is shutting down".into()))],
            ));
        }
        debug!("Client connected via MRPC");

        self.events.register_client(client).await;

        // Start event forwarding if not already started
        let event_rx = self.events.take_event_rx();
        if let Some(event_rx) = event_rx {
            let pipeline = self.events.clone();
            tokio::spawn(async move {
                pipeline.forward_events(event_rx).await;
            });
        }

        // Ensure engine and begin world forwarder if not already running.
        let world = self.engine().await.world();
        self.events.ensure_world_forwarder(world).await;

        // Set up log forwarding to this client
        self.events.bind_log_sink();
        self.events.ensure_heartbeat().await;

        Ok(())
    }

    async fn handle_request(
        &self,
        _client: RpcSender,
        method: &str,
        params: Vec<Value>,
    ) -> StdResult<Value, RpcError> {
        debug!("Handling request: {} with {} params", method, params.len());

        match HotkeyMethod::try_from_str(method) {
            Some(HotkeyMethod::Shutdown) => self.handle_shutdown_request().await,
            Some(HotkeyMethod::SetConfigPath) => self.handle_set_config_path(&params).await,
            Some(HotkeyMethod::SetTheme) => self.handle_set_theme(&params).await,
            Some(HotkeyMethod::InjectKey) => self.handle_inject_key(&params).await,
            Some(HotkeyMethod::GetBindings) => self.handle_get_bindings().await,
            Some(HotkeyMethod::GetDepth) => self.handle_get_depth().await,
            Some(HotkeyMethod::GetWorldStatus) => self.handle_get_world_status().await,
            Some(HotkeyMethod::GetWorldSnapshot) => self.handle_get_world_snapshot().await,
            Some(HotkeyMethod::GetServerStatus) => self.handle_get_server_status().await,

            None => {
                warn!("Unknown method: {}", method);
                Err(typed_err(
                    crate::error::RpcErrorCode::MethodNotFound,
                    &[("method", Value::String(method.into()))],
                ))
            }
        }
    }

    async fn handle_notification(
        &self,
        _client: RpcSender,
        method: &str,
        _params: Vec<Value>,
    ) -> StdResult<(), RpcError> {
        trace!("Received notification: {}", method);
        Ok(())
    }
}

impl HotkeyService {
    /// Dispatches a hotkey event to the appropriate per-ID worker task.
    /// If no worker task exists for the given ID, one is spawned.
    pub(crate) fn dispatch_event_to_worker(&self, ev: mac_hotkey::Event) {
        let id = ev.id;
        let mut workers_guard = self.workers.lock();

        let tx = if let Some(tx) = workers_guard.get(&id) {
            tx.clone()
        } else {
            const PER_ID_QUEUE_CAPACITY: usize = 64;
            let (tx, mut rx) =
                tokio::sync::mpsc::channel::<mac_hotkey::Event>(PER_ID_QUEUE_CAPACITY);
            workers_guard.insert(id, tx.clone());

            let engine = self.engine.clone();
            let manager = self.manager.clone();
            let event_tx = self.events.sender();
            let shutdown = self.shutdown_flag();
            let workers_clone = self.workers.clone();
            let my_tx = tx.clone();

            tokio::spawn(async move {
                let eng = engine
                    .get_or_init(|| async { Engine::new(manager, event_tx.clone()) })
                    .await;

                loop {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }

                    // Keep the worker task alive for at most 5 seconds of inactivity.
                    let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;

                    match msg {
                        Ok(Some(ev)) => {
                            if let Err(e) = eng.dispatch(ev.id, ev.kind, ev.repeat).await {
                                trace!(
                                    target: "hotki_server::ipc::service",
                                    "OS dispatch failed id={} kind={:?}: {}",
                                    ev.id,
                                    ev.kind,
                                    e
                                );
                            }
                        }
                        Ok(None) | Err(_) => {
                            break;
                        }
                    }
                }

                // Channel closed, idle timeout expired, or server shut down -> reap this worker
                let mut g = workers_clone.lock();
                if let Some(current_tx) = g.get(&id)
                    && current_tx.same_channel(&my_tx)
                {
                    g.remove(&id);
                }
            });

            tx
        };

        drop(workers_guard);

        match tx.try_send(ev) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                trace!(id, "per_id_queue_full_drop");
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                // Task has exited or is exiting; next event will spawn a new worker
            }
        }
    }

    /// Start the hotkey event dispatcher
    pub(crate) fn start_hotkey_dispatcher(&self) {
        let manager = self.manager.clone();
        let shutdown = self.shutdown_flag();

        // Bridge: dedicated OS thread blocks on crossbeam and forwards to Tokio mpsc
        let rx_cross = manager.events();
        let mut rx_ev = crate::util::bridge_crossbeam_to_tokio(rx_cross);

        let this = self.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx_ev.recv().await {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                this.dispatch_event_to_worker(ev);
            }
        });

        debug!("Hotkey dispatcher started with per-id ordering");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[tokio::test]
    async fn test_dispatcher_worker_reaping() {
        // Try creating the mac_hotkey::Manager. If accessibility/tap permissions are absent,
        // skip this test gracefully to ensure integration robustness on CI or headless environs.
        let manager_res = mac_hotkey::Manager::new();
        let manager = match manager_res {
            Ok(mgr) => Arc::new(mgr),
            Err(e) => {
                warn!(
                    "Skipping test_dispatcher_worker_reaping because mac_hotkey::Manager failed to initialize: {:?}",
                    e
                );
                return;
            }
        };

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_notify = Arc::new(tokio::sync::Notify::new());
        let idle_state = Arc::new(IdleTimerState::new(30));

        let service = HotkeyService::new(manager, shutdown, shutdown_notify, idle_state);

        assert_eq!(service.active_workers_count(), 0);

        // Dispatch an event to spawn a worker for hotkey ID 42
        let ev = mac_hotkey::Event {
            id: 42,
            hotkey: mac_keycode::Chord {
                key: mac_keycode::Key::A,
                modifiers: HashSet::new(),
            },
            kind: mac_hotkey::EventKind::KeyDown,
            repeat: false,
        };

        service.dispatch_event_to_worker(ev.clone());

        // Wait a brief moment for the spawned task to initialize and workers map to update
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(service.active_workers_count(), 1);

        // Sleep for slightly more than the 5-second inactivity timeout (e.g. 5.5 seconds)
        // to verify that the worker is reaped.
        tokio::time::sleep(tokio::time::Duration::from_millis(5500)).await;
        assert_eq!(service.active_workers_count(), 0);
    }

    #[tokio::test]
    async fn test_dispatcher_worker_reactivation() {
        let manager_res = mac_hotkey::Manager::new();
        let manager = match manager_res {
            Ok(mgr) => Arc::new(mgr),
            Err(e) => {
                warn!(
                    "Skipping test_dispatcher_worker_reactivation because mac_hotkey::Manager failed to initialize: {:?}",
                    e
                );
                return;
            }
        };

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_notify = Arc::new(tokio::sync::Notify::new());
        let idle_state = Arc::new(IdleTimerState::new(30));

        let service = HotkeyService::new(manager, shutdown, shutdown_notify, idle_state);

        assert_eq!(service.active_workers_count(), 0);

        let ev = mac_hotkey::Event {
            id: 42,
            hotkey: mac_keycode::Chord {
                key: mac_keycode::Key::A,
                modifiers: HashSet::new(),
            },
            kind: mac_hotkey::EventKind::KeyDown,
            repeat: false,
        };

        // 1. Spawn initial worker
        service.dispatch_event_to_worker(ev.clone());
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(service.active_workers_count(), 1);

        // 2. Wait for it to reap itself (5.5s)
        tokio::time::sleep(tokio::time::Duration::from_millis(5500)).await;
        assert_eq!(service.active_workers_count(), 0);

        // 3. Send a new event to the same ID and verify it reactivates a new worker successfully
        service.dispatch_event_to_worker(ev.clone());
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(service.active_workers_count(), 1);

        // Clean up: wait for reactivation worker to be reaped
        tokio::time::sleep(tokio::time::Duration::from_millis(5500)).await;
        assert_eq!(service.active_workers_count(), 0);
    }

    #[tokio::test]
    async fn test_dispatcher_worker_shutdown() {
        let manager_res = mac_hotkey::Manager::new();
        let manager = match manager_res {
            Ok(mgr) => Arc::new(mgr),
            Err(e) => {
                warn!(
                    "Skipping test_dispatcher_worker_shutdown because mac_hotkey::Manager failed to initialize: {:?}",
                    e
                );
                return;
            }
        };

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_notify = Arc::new(tokio::sync::Notify::new());
        let idle_state = Arc::new(IdleTimerState::new(30));

        let service = HotkeyService::new(manager, shutdown.clone(), shutdown_notify, idle_state);

        assert_eq!(service.active_workers_count(), 0);

        let ev = mac_hotkey::Event {
            id: 42,
            hotkey: mac_keycode::Chord {
                key: mac_keycode::Key::A,
                modifiers: HashSet::new(),
            },
            kind: mac_hotkey::EventKind::KeyDown,
            repeat: false,
        };

        // Spawn a worker
        service.dispatch_event_to_worker(ev.clone());
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(service.active_workers_count(), 1);

        // Trigger shutdown flag
        shutdown.store(true, Ordering::SeqCst);

        // Dispatching another event triggers the loop to wake up, check shutdown, break, and reap instantly.
        service.dispatch_event_to_worker(ev.clone());

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(service.active_workers_count(), 0);
    }

    #[tokio::test]
    async fn test_dispatcher_worker_isolation() {
        let manager_res = mac_hotkey::Manager::new();
        let manager = match manager_res {
            Ok(mgr) => Arc::new(mgr),
            Err(e) => {
                warn!(
                    "Skipping test_dispatcher_worker_isolation because mac_hotkey::Manager failed to initialize: {:?}",
                    e
                );
                return;
            }
        };

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_notify = Arc::new(tokio::sync::Notify::new());
        let idle_state = Arc::new(IdleTimerState::new(30));

        let service = HotkeyService::new(manager, shutdown, shutdown_notify, idle_state);

        assert_eq!(service.active_workers_count(), 0);

        let ev1 = mac_hotkey::Event {
            id: 101,
            hotkey: mac_keycode::Chord {
                key: mac_keycode::Key::A,
                modifiers: HashSet::new(),
            },
            kind: mac_hotkey::EventKind::KeyDown,
            repeat: false,
        };

        let ev2 = mac_hotkey::Event {
            id: 102,
            hotkey: mac_keycode::Chord {
                key: mac_keycode::Key::B,
                modifiers: HashSet::new(),
            },
            kind: mac_hotkey::EventKind::KeyDown,
            repeat: false,
        };

        // Dispatch first event
        service.dispatch_event_to_worker(ev1);
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(service.active_workers_count(), 1);

        // Dispatch second event (different ID)
        service.dispatch_event_to_worker(ev2);
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(service.active_workers_count(), 2);

        // Let them both idle timeout and reap themselves (5.5s)
        tokio::time::sleep(tokio::time::Duration::from_millis(5500)).await;
        assert_eq!(service.active_workers_count(), 0);
    }

    #[tokio::test]
    async fn test_dispatcher_same_channel_protection() {
        let manager_res = mac_hotkey::Manager::new();
        let manager = match manager_res {
            Ok(mgr) => Arc::new(mgr),
            Err(e) => {
                warn!(
                    "Skipping test_dispatcher_same_channel_protection because mac_hotkey::Manager failed to initialize: {:?}",
                    e
                );
                return;
            }
        };

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_notify = Arc::new(tokio::sync::Notify::new());
        let idle_state = Arc::new(IdleTimerState::new(30));

        let service = HotkeyService::new(manager, shutdown, shutdown_notify, idle_state);

        assert_eq!(service.active_workers_count(), 0);

        let ev = mac_hotkey::Event {
            id: 999,
            hotkey: mac_keycode::Chord {
                key: mac_keycode::Key::A,
                modifiers: HashSet::new(),
            },
            kind: mac_hotkey::EventKind::KeyDown,
            repeat: false,
        };

        // 1. Spawn Worker A
        service.dispatch_event_to_worker(ev.clone());
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(service.active_workers_count(), 1);

        // 2. Overtake the map entry with a new independent channel (simulating Worker B spawning)
        let (tx_b, _rx_b) = tokio::sync::mpsc::channel(64);
        {
            let mut g = service.workers.lock();
            g.insert(999, tx_b);
        }

        // 3. Wait for Worker A to timeout (5.5s)
        tokio::time::sleep(tokio::time::Duration::from_millis(5500)).await;

        // 4. Since the map entry was overtaken by a different channel, Worker A's timeout
        // should NOT have removed the entry. The active workers count must still be 1!
        assert_eq!(service.active_workers_count(), 1);

        // Verify the remaining sender is indeed our mock channel
        {
            let g = service.workers.lock();
            assert!(g.contains_key(&999));
        }
    }
}
