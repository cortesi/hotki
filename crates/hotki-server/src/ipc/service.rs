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

use std::{
    collections::HashMap,
    result::Result as StdResult,
    slice,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use hotki_engine::Engine;
use hotki_protocol::{App, MsgToUI, WorldStreamMsg, WorldWindowLite};
use mrpc::{Connection as MrpcConnection, RpcError, RpcSender, ServiceError, Value};
use parking_lot::Mutex;
use tokio::sync::{Mutex as AsyncMutex, mpsc::UnboundedSender};
use tracing::{debug, error, info, trace, warn};

use crate::ipc::rpc::{HotkeyMethod, HotkeyNotification, enc_world_status};

/// IPC service that handles hotkey manager operations
#[derive(Clone)]
pub struct HotkeyService {
    /// The hotkey engine
    engine: Arc<tokio::sync::Mutex<Option<Engine>>>,
    /// Mac hotkey manager
    manager: Arc<mac_hotkey::Manager>,
    /// Event sender for UI messages
    event_tx: UnboundedSender<MsgToUI>,
    /// Event receiver (taken when starting forwarder)
    event_rx: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<MsgToUI>>>>,
    /// Connected clients; use Tokio mutex to reduce sync/async mixing.
    clients: Arc<AsyncMutex<Vec<RpcSender>>>,
    /// When set to true, the outer server event loop should exit.
    shutdown: Arc<AtomicBool>,
    /// Optional cap on per-id in-flight events (worker queue capacity)
    per_id_capacity: Option<usize>,
    /// Ensure we only spawn one heartbeat loop across clones.
    hb_running: Arc<AtomicBool>,
    world_forwarder_running: Arc<AtomicBool>,
    /// When true, auto-shutdown the server if no UI clients remain connected.
    auto_shutdown_on_empty: Arc<AtomicBool>,
}

impl HotkeyService {
    /// Construct a typed `RpcError::Service` with a stable `name` and structured fields.
    fn typed_err(code: crate::error::RpcErrorCode, fields: &[(&str, Value)]) -> RpcError {
        let map = fields
            .iter()
            .map(|(k, v)| (Value::String((*k).into()), v.clone()))
            .collect::<Vec<_>>();
        RpcError::Service(ServiceError {
            name: code.to_string(),
            value: Value::Map(map),
        })
    }
    pub fn new(manager: Arc<mac_hotkey::Manager>, shutdown: Arc<AtomicBool>) -> Self {
        // Create event channel
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();

        Self {
            engine: Arc::new(tokio::sync::Mutex::new(None)),
            manager,
            event_tx,
            event_rx: Arc::new(Mutex::new(Some(event_rx))),
            clients: Arc::new(AsyncMutex::new(Vec::new())),
            shutdown,
            per_id_capacity: None,
            hb_running: Arc::new(AtomicBool::new(false)),
            world_forwarder_running: Arc::new(AtomicBool::new(false)),
            auto_shutdown_on_empty: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a builder to configure and construct a `HotkeyService`.
    ///
    /// Use this when you need to tweak knobs (e.g., max in-flight events).
    pub fn builder(
        manager: Arc<mac_hotkey::Manager>,
        shutdown: Arc<AtomicBool>,
    ) -> HotkeyServiceBuilder {
        HotkeyServiceBuilder {
            manager,
            shutdown,
            per_id_capacity: None,
            auto_shutdown_on_empty: false,
        }
    }

    /// Expose the shutdown flag for coordinated server shutdown.
    pub(crate) fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    /// Initialize the engine (must be called within Tokio runtime)
    async fn ensure_engine_initialized(&self) -> crate::Result<()> {
        // First check if already initialized without holding lock long
        {
            let engine_guard = self.engine.lock().await;
            if engine_guard.is_some() {
                return Ok(());
            }
        }

        // Acquire sync lock first (following lock ordering: sync before async)
        let event_tx = self.event_tx.clone();

        // Now acquire async lock
        let mut engine_guard = self.engine.lock().await;
        // Double-check in case of race condition
        if engine_guard.is_none() {
            *engine_guard = Some(Engine::new(self.manager.clone(), event_tx));
        }
        Ok(())
    }

    /// Forward events from the receiver to connected clients
    ///
    /// Log forwarding semantics: logs use a single global sink wired to the
    /// event channel (`log_forward::set_sink(tx)`). Events are broadcast to all
    /// connected clients via `broadcast_event`; multi-client is supported, and
    /// logs are delivered through the same event pipeline as other messages.
    async fn forward_events(&self, mut event_rx: tokio::sync::mpsc::UnboundedReceiver<MsgToUI>) {
        while let Some(event) = event_rx.recv().await {
            if self.shutdown.load(Ordering::SeqCst) {
                break;
            }
            self.broadcast_event(event).await;
        }
    }

    /// Start forwarding world events to the UI channel as MsgToUI::World.
    fn start_world_forwarder(&self) {
        if self.world_forwarder_running.swap(true, Ordering::SeqCst) {
            return; // already running
        }
        let shutdown = self.shutdown.clone();
        let event_tx = self.event_tx.clone();
        let engine = self.engine.clone();
        tokio::spawn(async move {
            // Ensure engine exists and get world handle
            let _world = loop {
                if shutdown.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(eng) = engine.lock().await.as_ref() {
                    break eng.clone();
                }
                // engine not initialized yet; wait briefly
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            };
            let wh = {
                let eng = engine.lock().await;
                // safe to unwrap: loop above ensured Some
                eng.as_ref().unwrap().world_handle()
            };
            let mut rx = wh.subscribe();
            loop {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                match rx.recv().await {
                    Ok(ev) => {
                        let msg_opt: Option<WorldStreamMsg> = match ev {
                            hotki_world::WorldEvent::Added(w) => {
                                Some(WorldStreamMsg::Added(WorldWindowLite {
                                    app: w.app,
                                    title: w.title,
                                    pid: w.pid,
                                    id: w.id,
                                    z: w.z,
                                    focused: w.focused,
                                    display_id: w.display_id,
                                }))
                            }
                            hotki_world::WorldEvent::Removed(k) => Some(WorldStreamMsg::Removed {
                                pid: k.pid,
                                id: k.id,
                            }),
                            hotki_world::WorldEvent::Updated(k, _d) => {
                                Some(WorldStreamMsg::Updated {
                                    pid: k.pid,
                                    id: k.id,
                                })
                            }
                            hotki_world::WorldEvent::MetaAdded(_, _)
                            | hotki_world::WorldEvent::MetaRemoved(_, _) => None,
                            hotki_world::WorldEvent::FocusChanged(_k) => {
                                let ctx = wh.focused_context().await;
                                let app = ctx.map(|(app, title, pid)| App { app, title, pid });
                                Some(WorldStreamMsg::FocusChanged(app))
                            }
                        };
                        if let Some(msg) = msg_opt {
                            let _ = event_tx.send(MsgToUI::World(msg));
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let _ = event_tx.send(MsgToUI::World(WorldStreamMsg::ResyncRecommended));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// Broadcast an event to all connected clients
    async fn broadcast_event(&self, event: MsgToUI) {
        if self.shutdown.load(Ordering::SeqCst) {
            return;
        }
        // Clone the current client list for sending without holding the lock
        let clients_snapshot = { self.clients.lock().await.clone() };

        // Convert event to MRPC Value (binary serde payload)
        let value = match crate::ipc::rpc::enc_event(&event) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to encode event for broadcast: {}", e);
                return;
            }
        };

        // Send concurrently; retain only successful clients
        let mut survivors = Vec::with_capacity(clients_snapshot.len());
        let mut futs = FuturesUnordered::new();
        for client in clients_snapshot {
            let v = value.clone();
            futs.push(async move {
                (
                    client.clone(),
                    client
                        .send_notification(HotkeyNotification::Notify.as_str(), slice::from_ref(&v))
                        .await,
                )
            });
        }
        while let Some((client, res)) = futs.next().await {
            match res {
                Ok(_) => survivors.push(client),
                Err(e) => warn!("Dropping disconnected client (send failed): {:?}", e),
            }
        }

        // Replace the clients list with survivors
        *self.clients.lock().await = survivors;
    }
}

#[async_trait]
impl MrpcConnection for HotkeyService {
    async fn connected(&self, client: RpcSender) -> StdResult<(), RpcError> {
        if self.shutdown.load(Ordering::SeqCst) {
            // Refuse new connections during shutdown
            return Err(Self::typed_err(
                crate::error::RpcErrorCode::ShuttingDown,
                &[("message", Value::String("Server is shutting down".into()))],
            ));
        }
        debug!("Client connected via MRPC");

        // Add client to list for event broadcasting
        self.clients.lock().await.push(client.clone());

        // Start event forwarding if not already started
        let event_rx = { self.event_rx.lock().take() };
        if let Some(event_rx) = event_rx {
            let service_clone = self.clone();
            tokio::spawn(async move {
                service_clone.forward_events(event_rx).await;
            });
        }

        // Begin world forwarder if not already running
        self.start_world_forwarder();

        // Set up log forwarding to this client
        // Bind the global log sink to the single event channel. Logs are then
        // forwarded through the standard event pipeline and broadcast to all
        // connected clients by `forward_events`.
        log_forward::set_sink(self.event_tx.clone());

        // Proactively send an initial status snapshot to this client
        if let Err(e) = self.ensure_engine_initialized().await {
            return Err(Self::typed_err(
                crate::error::RpcErrorCode::EngineInit,
                &[("message", Value::String(e.to_string().into()))],
            ));
        }
        // No initial status snapshot; UI derives state from HudUpdate events.

        // Start a single heartbeat loop. The loop exits when shutdown is set.
        if !self.hb_running.swap(true, Ordering::SeqCst) {
            let svc = self.clone();
            tokio::spawn(async move {
                use std::time::SystemTime;
                let interval = hotki_protocol::ipc::heartbeat::interval();
                let mut empty_since: Option<std::time::Instant> = None;
                loop {
                    if svc.shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                    let ts = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    svc.broadcast_event(hotki_protocol::MsgToUI::Heartbeat(ts))
                        .await;
                    // If enabled via config, shut down when no clients remain for a short grace period.
                    if svc.auto_shutdown_on_empty.load(Ordering::SeqCst) {
                        let n = { svc.clients.lock().await.len() };
                        if n == 0 {
                            match empty_since {
                                None => empty_since = Some(std::time::Instant::now()),
                                Some(t0) => {
                                    if t0.elapsed() >= std::time::Duration::from_millis(750) {
                                        tracing::info!(
                                            "No UI clients; auto-shutdown enabled — stopping server"
                                        );
                                        svc.shutdown.store(true, Ordering::SeqCst);
                                        let _ = mac_winops::focus::post_user_event();
                                        break;
                                    }
                                }
                            }
                        } else {
                            empty_since = None;
                        }
                    }
                    tokio::time::sleep(interval).await;
                }
                svc.hb_running.store(false, Ordering::SeqCst);
            });
        }

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
            Some(HotkeyMethod::Shutdown) => {
                info!("Shutdown request received");
                // Flip shutdown flag (idempotent)
                self.shutdown.store(true, Ordering::SeqCst);

                // Wake the Tao event loop so it can observe shutdown promptly
                let _ = mac_winops::focus::post_user_event();

                // Stop forwarding any further logs/events to clients
                log_forward::clear_sink();

                // Drop all clients so no further notifications are attempted
                self.clients.lock().await.clear();

                // Close the UI event pipeline
                {
                    let mut r = self.event_rx.lock();
                    *r = None;
                }

                Ok(Value::Boolean(true))
            }

            Some(HotkeyMethod::SetConfig) => {
                if params.is_empty() {
                    return Err(Self::typed_err(
                        crate::error::RpcErrorCode::MissingParams,
                        &[
                            (
                                "method",
                                Value::String(HotkeyMethod::SetConfig.as_str().into()),
                            ),
                            ("expected", Value::String("config".into())),
                        ],
                    ));
                }

                let cfg = crate::ipc::rpc::dec_set_config_param(&params[0])?;
                debug!("Setting config via MRPC");

                // Ensure engine is initialized
                if let Err(e) = self.ensure_engine_initialized().await {
                    return Err(Self::typed_err(
                        crate::error::RpcErrorCode::EngineInit,
                        &[("message", Value::String(e.to_string().into()))],
                    ));
                }

                let mut engine_guard = self.engine.lock().await;
                let engine = match engine_guard.as_mut() {
                    Some(eng) => eng,
                    None => {
                        error!("Engine not initialized when setting config");
                        return Err(Self::typed_err(
                            crate::error::RpcErrorCode::EngineNotInitialized,
                            &[("message", Value::String("Engine not initialized".into()))],
                        ));
                    }
                };
                if let Err(e) = engine.set_config(cfg.clone()).await {
                    return Err(Self::typed_err(
                        crate::error::RpcErrorCode::EngineSetConfig,
                        &[("message", Value::String(e.to_string().into()))],
                    ));
                }

                // Update auto-shutdown flag from config if present.
                self.auto_shutdown_on_empty
                    .store(cfg.server().exit_if_no_clients, Ordering::SeqCst);

                drop(engine_guard);

                Ok(Value::Boolean(true))
            }

            Some(HotkeyMethod::InjectKey) => {
                // Expect a single Binary param with msgpack-encoded InjectKeyReq
                if params.is_empty() {
                    return Err(Self::typed_err(
                        crate::error::RpcErrorCode::MissingParams,
                        &[("expected", Value::String("inject request".into()))],
                    ));
                }
                let req = match crate::ipc::rpc::dec_inject_key_param(&params[0]) {
                    Ok(r) => r,
                    Err(e) => return Err(e),
                };
                tracing::debug!(target: "hotki_server::ipc::service", "InjectKey: ident={} kind={:?} repeat={}", req.ident, req.kind, req.repeat);

                // Ensure engine is initialized
                if let Err(e) = self.ensure_engine_initialized().await {
                    return Err(Self::typed_err(
                        crate::error::RpcErrorCode::EngineInit,
                        &[("message", Value::String(e.to_string().into()))],
                    ));
                }

                // Access engine and resolve ident → id
                let eng = match self.engine.lock().await.as_ref() {
                    Some(e) => e.clone(),
                    None => {
                        return Err(Self::typed_err(
                            crate::error::RpcErrorCode::EngineNotInitialized,
                            &[("message", Value::String("engine not initialized".into()))],
                        ));
                    }
                };

                let maybe_id = eng.resolve_id_for_ident(&req.ident).await;
                let id = match maybe_id {
                    Some(i) => i,
                    None => {
                        tracing::debug!(target: "hotki_server::ipc::service", "InjectKey: ident not bound: {}", req.ident);
                        return Err(Self::typed_err(
                            crate::error::RpcErrorCode::KeyNotBound,
                            &[("ident", Value::String(req.ident.into()))],
                        ));
                    }
                };
                tracing::debug!(target: "hotki_server::ipc::service", "InjectKey: resolved id={} for ident={} -> dispatch", id, req.ident);

                // Dispatch directly through the engine (same path as OS events)
                eng.dispatch(id, req.kind.to_event_kind(), req.repeat).await;
                tracing::debug!(target: "hotki_server::ipc::service", "InjectKey: dispatch done for id={}", id);
                Ok(Value::Boolean(true))
            }

            Some(HotkeyMethod::GetBindings) => {
                // Ensure engine
                if let Err(e) = self.ensure_engine_initialized().await {
                    return Err(Self::typed_err(
                        crate::error::RpcErrorCode::EngineInit,
                        &[("message", Value::String(e.to_string().into()))],
                    ));
                }
                let eng = match self.engine.lock().await.as_ref() {
                    Some(e) => e.clone(),
                    None => {
                        return Err(Self::typed_err(
                            crate::error::RpcErrorCode::EngineNotInitialized,
                            &[("message", Value::String("engine not initialized".into()))],
                        ));
                    }
                };
                let mut idents: Vec<String> = eng
                    .bindings_snapshot()
                    .await
                    .into_iter()
                    .map(|(ident, _)| ident)
                    .collect();
                // Keep ordering stable for consumers/tests
                idents.sort();
                // Return as Value::Array of Strings to avoid extra msgpack layer
                let arr = idents
                    .into_iter()
                    .map(|s| Value::String(s.into()))
                    .collect::<Vec<_>>();
                Ok(Value::Array(arr))
            }

            Some(HotkeyMethod::GetDepth) => {
                // Ensure engine
                if let Err(e) = self.ensure_engine_initialized().await {
                    return Err(Self::typed_err(
                        crate::error::RpcErrorCode::EngineInit,
                        &[("message", Value::String(e.to_string().into()))],
                    ));
                }
                let eng = match self.engine.lock().await.as_ref() {
                    Some(e) => e.clone(),
                    None => {
                        return Err(Self::typed_err(
                            crate::error::RpcErrorCode::EngineNotInitialized,
                            &[("message", Value::String("engine not initialized".into()))],
                        ));
                    }
                };
                let depth = eng.get_depth().await as u64;
                Ok(Value::Integer(depth.into()))
            }

            Some(HotkeyMethod::GetWorldStatus) => {
                // Ensure engine
                if let Err(e) = self.ensure_engine_initialized().await {
                    return Err(Self::typed_err(
                        crate::error::RpcErrorCode::EngineInit,
                        &[("message", Value::String(e.to_string().into()))],
                    ));
                }
                let eng = match self.engine.lock().await.as_ref() {
                    Some(e) => e.clone(),
                    None => {
                        return Err(Self::typed_err(
                            crate::error::RpcErrorCode::EngineNotInitialized,
                            &[("message", Value::String("engine not initialized".into()))],
                        ));
                    }
                };
                let st = eng.world_status().await;
                Ok(enc_world_status(&st))
            }

            Some(HotkeyMethod::GetWorldSnapshot) => {
                // Ensure engine
                if let Err(e) = self.ensure_engine_initialized().await {
                    return Err(Self::typed_err(
                        crate::error::RpcErrorCode::EngineInit,
                        &[("message", Value::String(e.to_string().into()))],
                    ));
                }
                let eng = match self.engine.lock().await.as_ref() {
                    Some(e) => e.clone(),
                    None => {
                        return Err(Self::typed_err(
                            crate::error::RpcErrorCode::EngineNotInitialized,
                            &[("message", Value::String("engine not initialized".into()))],
                        ));
                    }
                };
                let wh = eng.world_handle();
                // Obtain consistent snapshot + focused key
                let (_rx, snap, focused_key) = wh.subscribe_with_snapshot().await;
                // Convert to protocol types
                let mut wins: Vec<hotki_protocol::WorldWindowLite> = snap
                    .into_iter()
                    .map(|w| hotki_protocol::WorldWindowLite {
                        app: w.app,
                        title: w.title,
                        pid: w.pid,
                        id: w.id,
                        z: w.z,
                        focused: w.focused,
                        display_id: w.display_id,
                    })
                    .collect();
                wins.sort_by_key(|w| (w.z, w.pid, w.id));
                let focused = focused_key.and_then(|k| {
                    wins.iter()
                        .find(|w| w.pid == k.pid && w.id == k.id)
                        .map(|w| hotki_protocol::App {
                            app: w.app.clone(),
                            title: w.title.clone(),
                            pid: w.pid,
                        })
                });
                let payload = crate::ipc::rpc::WorldSnapshotLite {
                    windows: wins,
                    focused,
                };
                crate::ipc::rpc::enc_world_snapshot(&payload).map_err(|e| {
                    Self::typed_err(
                        crate::error::RpcErrorCode::InvalidType,
                        &[("message", Value::String(e.to_string().into()))],
                    )
                })
            }

            None => {
                warn!("Unknown method: {}", method);
                Err(Self::typed_err(
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
    /// Start the hotkey event dispatcher
    pub(crate) fn start_hotkey_dispatcher(&self) {
        let manager = self.manager.clone();
        let engine = self.engine.clone();
        let shutdown = self.shutdown.clone();

        // Bridge: dedicated OS thread blocks on crossbeam and forwards to Tokio mpsc
        let rx_cross = manager.events();
        let mut rx_ev = crate::util::bridge_crossbeam_to_tokio(rx_cross);

        // Async task consumes Tokio channel and dispatches events with per-id ordering
        let per_id_capacity = self.per_id_capacity;
        tokio::spawn(async move {
            // Store heterogeneous senders via a small enum so we can support
            // either bounded or unbounded queues per id.
            enum WorkerSender {
                Bounded(tokio::sync::mpsc::Sender<mac_hotkey::Event>),
                Unbounded(tokio::sync::mpsc::UnboundedSender<mac_hotkey::Event>),
            }

            impl WorkerSender {
                fn try_send(&self, ev: mac_hotkey::Event) -> bool {
                    match self {
                        WorkerSender::Unbounded(tx) => tx.send(ev).is_ok(),
                        WorkerSender::Bounded(tx) => tx.try_send(ev).is_ok(),
                    }
                }
            }

            let mut workers: HashMap<u32, WorkerSender> = HashMap::new();

            while let Some(ev) = rx_ev.recv().await {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }

                let id = ev.id;
                let ev_clone = ev.clone();
                if let Some(tx) = workers.get(&id)
                    && tx.try_send(ev)
                {
                    continue;
                }

                // Create a new per-id worker
                if let Some(cap) = per_id_capacity {
                    let (tx, mut rx) = tokio::sync::mpsc::channel::<mac_hotkey::Event>(cap);
                    let _ = tx.try_send(ev_clone);
                    workers.insert(id, WorkerSender::Bounded(tx));

                    let engine = engine.clone();
                    let shutdown = shutdown.clone();
                    tokio::spawn(async move {
                        while let Some(ev) = rx.recv().await {
                            if shutdown.load(Ordering::SeqCst) {
                                break;
                            }
                            let eng_guard = engine.lock().await;
                            if let Some(eng) = eng_guard.as_ref() {
                                eng.dispatch(ev.id, ev.kind, ev.repeat).await;
                            }
                        }
                    });
                } else {
                    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<mac_hotkey::Event>();
                    let _ = tx.send(ev_clone);
                    workers.insert(id, WorkerSender::Unbounded(tx));

                    let engine = engine.clone();
                    let shutdown = shutdown.clone();
                    tokio::spawn(async move {
                        while let Some(ev) = rx.recv().await {
                            if shutdown.load(Ordering::SeqCst) {
                                break;
                            }
                            let eng_guard = engine.lock().await;
                            if let Some(eng) = eng_guard.as_ref() {
                                eng.dispatch(ev.id, ev.kind, ev.repeat).await;
                            }
                        }
                    });
                }
            }
        });

        debug!("Hotkey dispatcher started with per-id ordering");
    }
}

/// Builder for `HotkeyService`.
pub struct HotkeyServiceBuilder {
    manager: Arc<mac_hotkey::Manager>,
    shutdown: Arc<AtomicBool>,
    per_id_capacity: Option<usize>,
    auto_shutdown_on_empty: bool,
}

impl HotkeyServiceBuilder {
    /// Limit in-flight events per key id. When set, queues are bounded
    /// and new events are dropped when the queue is full.
    pub fn max_in_flight_per_id(mut self, capacity: usize) -> Self {
        self.per_id_capacity = Some(capacity.max(1));
        self
    }

    /// Build the service with the configured options.
    pub fn build(self) -> HotkeyService {
        let mut svc = HotkeyService::new(self.manager, self.shutdown);
        svc.per_id_capacity = self.per_id_capacity;
        svc.auto_shutdown_on_empty
            .store(self.auto_shutdown_on_empty, Ordering::SeqCst);
        svc
    }
}
