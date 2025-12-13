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
    path::PathBuf,
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
use hotki_protocol::{
    App, MsgToUI, WorldStreamMsg,
    rpc::{
        HotkeyMethod, HotkeyNotification, InjectKeyReq, InjectKind, ServerStatusLite,
        WorldSnapshotLite,
    },
};
use mrpc::{Connection as MrpcConnection, RpcError, RpcSender, ServiceError, Value};
use parking_lot::Mutex;
use tokio::sync::{
    Mutex as AsyncMutex, OnceCell,
    mpsc::{Receiver, Sender},
};
use tracing::{debug, error, info, trace, warn};

use super::{IdleTimerSnapshot, IdleTimerState};
use crate::loop_wake;

/// IPC service that handles hotkey manager operations
#[derive(Clone)]
pub struct HotkeyService {
    /// The hotkey engine
    engine: Arc<OnceCell<Engine>>,
    /// Mac hotkey manager
    manager: Arc<mac_hotkey::Manager>,
    /// Event sender for UI messages (bounded)
    event_tx: Sender<MsgToUI>,
    /// Event receiver (taken when starting forwarder)
    event_rx: Arc<Mutex<Option<Receiver<MsgToUI>>>>,
    /// Connected clients; use Tokio mutex to reduce sync/async mixing.
    clients: Arc<AsyncMutex<Vec<RpcSender>>>,
    /// When set to true, the outer server event loop should exit.
    shutdown: Arc<AtomicBool>,
    /// Ensure we only spawn one heartbeat loop across clones.
    hb_running: Arc<AtomicBool>,
    world_forwarder_running: Arc<AtomicBool>,
    /// When true, auto-shutdown the server if no UI clients remain connected.
    auto_shutdown_on_empty: Arc<AtomicBool>,
    /// Shared idle timer state for status reporting.
    idle_state: Arc<IdleTimerState>,
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
    pub fn new(
        manager: Arc<mac_hotkey::Manager>,
        shutdown: Arc<AtomicBool>,
        idle_state: Arc<IdleTimerState>,
    ) -> Self {
        // Create bounded event channel
        let (event_tx, event_rx) = hotki_protocol::ipc::ui_channel();

        Self {
            engine: Arc::new(OnceCell::new()),
            manager,
            event_tx,
            event_rx: Arc::new(Mutex::new(Some(event_rx))),
            clients: Arc::new(AsyncMutex::new(Vec::new())),
            shutdown,
            hb_running: Arc::new(AtomicBool::new(false)),
            world_forwarder_running: Arc::new(AtomicBool::new(false)),
            auto_shutdown_on_empty: Arc::new(AtomicBool::new(false)),
            idle_state,
        }
    }

    /// Expose the shutdown flag for coordinated server shutdown.
    pub(crate) fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    async fn engine(&self) -> &Engine {
        self.engine
            .get_or_init(|| async { Engine::new(self.manager.clone(), self.event_tx.clone()) })
            .await
    }

    /// Gather a lightweight server status snapshot for diagnostics.
    async fn snapshot_server_status(&self) -> ServerStatusLite {
        let clients_connected = { self.clients.lock().await.len() };
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

    /// Forward events from the receiver to connected clients
    ///
    /// Log forwarding semantics: logs use a single global sink wired to the
    /// event channel (`logging::forward::set_sink(tx)`). Events are broadcast to all
    /// connected clients via `broadcast_event`; multi-client is supported, and
    /// logs are delivered through the same event pipeline as other messages.
    async fn forward_events(&self, mut event_rx: Receiver<MsgToUI>) {
        while let Some(event) = event_rx.recv().await {
            if self.shutdown.load(Ordering::SeqCst) {
                break;
            }
            self.broadcast_event(event).await;
        }
    }

    /// Start forwarding world events to the UI channel as MsgToUI::World.
    async fn start_world_forwarder(&self) {
        if self.world_forwarder_running.swap(true, Ordering::SeqCst) {
            return; // already running
        }
        let shutdown = self.shutdown.clone();
        let event_tx = self.event_tx.clone();
        let world = self.engine().await.world();
        tokio::spawn(async move {
            let mut cursor = world.subscribe();
            loop {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }

                let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
                let event = match world.next_event_until(&mut cursor, deadline).await {
                    Some(ev) => ev,
                    None => {
                        if cursor.is_closed() {
                            return;
                        }
                        continue;
                    }
                };

                let hotki_world::WorldEvent::FocusChanged(change) = event else {
                    continue;
                };

                let app = match (change.app, change.title, change.pid) {
                    (Some(app), Some(title), Some(pid)) => Some(App { app, title, pid }),
                    _ => world.focused_context().await.map(|(app, title, pid)| App {
                        app,
                        title,
                        pid,
                    }),
                };

                if let Err(err) =
                    event_tx.try_send(MsgToUI::World(WorldStreamMsg::FocusChanged(app)))
                {
                    match err {
                        tokio::sync::mpsc::error::TrySendError::Full(_) => {
                            // Drop silently; focus updates are best-effort.
                        }
                        tokio::sync::mpsc::error::TrySendError::Closed(_) => return,
                    }
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
        let value = match enc_event(&event) {
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

        // Ensure engine and begin world forwarder if not already running.
        let _ = self.engine().await;
        self.start_world_forwarder().await;

        // Set up log forwarding to this client
        // Bind the global log sink to the single event channel. Logs are then
        // forwarded through the standard event pipeline and broadcast to all
        // connected clients by `forward_events`.
        logging::forward::set_sink(self.event_tx.clone());

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
                                        let _ = loop_wake::post_user_event();
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
                let _ = loop_wake::post_user_event();

                // Stop forwarding any further logs/events to clients
                logging::forward::clear_sink();

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

                let cfg = dec_set_config_param(&params[0])?;
                debug!("Setting config via MRPC");

                let engine = self.engine().await;
                if let Err(e) = engine.set_config(cfg.clone()).await {
                    return Err(Self::typed_err(
                        crate::error::RpcErrorCode::EngineSetConfig,
                        &[("message", Value::String(e.to_string().into()))],
                    ));
                }

                // Update auto-shutdown flag from config if present.
                self.auto_shutdown_on_empty
                    .store(cfg.server().exit_if_no_clients, Ordering::SeqCst);

                Ok(Value::Boolean(true))
            }

            Some(HotkeyMethod::SetConfigPath) => {
                if params.is_empty() {
                    return Err(Self::typed_err(
                        crate::error::RpcErrorCode::MissingParams,
                        &[
                            (
                                "method",
                                Value::String(HotkeyMethod::SetConfigPath.as_str().into()),
                            ),
                            ("expected", Value::String("path".into())),
                        ],
                    ));
                }

                let raw_path = match &params[0] {
                    Value::String(s) => match s.as_str() {
                        Some(v) => v.to_string(),
                        None => {
                            return Err(Self::typed_err(
                                crate::error::RpcErrorCode::InvalidType,
                                &[("expected", Value::String("utf8 string path".into()))],
                            ));
                        }
                    },
                    _ => {
                        return Err(Self::typed_err(
                            crate::error::RpcErrorCode::InvalidType,
                            &[("expected", Value::String("string path".into()))],
                        ));
                    }
                };

                let path = PathBuf::from(raw_path.clone());
                let loaded = config::load_for_server_from_path(&path).map_err(|e| {
                    Self::typed_err(
                        crate::error::RpcErrorCode::InvalidConfig,
                        &[
                            ("path", Value::String(raw_path.clone().into())),
                            ("message", Value::String(e.pretty().into())),
                        ],
                    )
                })?;
                let cfg = loaded.config;
                let rhai = loaded.rhai;

                let engine = self.engine().await;
                if let Err(e) = engine.set_config_with_rhai(cfg.clone(), rhai).await {
                    return Err(Self::typed_err(
                        crate::error::RpcErrorCode::EngineSetConfig,
                        &[("message", Value::String(e.to_string().into()))],
                    ));
                }

                // Update auto-shutdown flag from config if present.
                self.auto_shutdown_on_empty
                    .store(cfg.server().exit_if_no_clients, Ordering::SeqCst);

                let bytes = rmp_serde::to_vec_named(&cfg).map_err(|e| {
                    Self::typed_err(
                        crate::error::RpcErrorCode::InvalidConfig,
                        &[("message", Value::String(e.to_string().into()))],
                    )
                })?;

                // Notify the UI of the loaded config for local rendering.
                self.event_tx
                    .try_send(MsgToUI::ConfigLoaded {
                        path: raw_path,
                        config: bytes.clone(),
                    })
                    .ok();

                Ok(Value::Binary(bytes))
            }

            Some(HotkeyMethod::InjectKey) => {
                // Expect a single Binary param with msgpack-encoded InjectKeyReq
                if params.is_empty() {
                    return Err(Self::typed_err(
                        crate::error::RpcErrorCode::MissingParams,
                        &[("expected", Value::String("inject request".into()))],
                    ));
                }
                let req = match dec_inject_key_param(&params[0]) {
                    Ok(r) => r,
                    Err(e) => return Err(e),
                };
                tracing::debug!(target: "hotki_server::ipc::service", "InjectKey: ident={} kind={:?} repeat={}", req.ident, req.kind, req.repeat);

                let eng = self.engine().await;

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
                match eng
                    .dispatch(id, inject_kind_to_event(req.kind), req.repeat)
                    .await
                {
                    Ok(_) => {
                        tracing::debug!(
                            target: "hotki_server::ipc::service",
                            "InjectKey: dispatch done for id={}",
                            id
                        );
                        Ok(Value::Boolean(true))
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "hotki_server::ipc::service",
                            "InjectKey: dispatch failed id={} ident={}: {}",
                            id,
                            req.ident,
                            e
                        );
                        return Err(Self::typed_err(
                            crate::error::RpcErrorCode::EngineDispatch,
                            &[("message", Value::String(e.to_string().into()))],
                        ));
                    }
                }
            }

            Some(HotkeyMethod::GetBindings) => {
                let eng = self.engine().await;
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
                let eng = self.engine().await;
                let depth = eng.get_depth().await as u64;
                Ok(Value::Integer(depth.into()))
            }

            Some(HotkeyMethod::GetWorldStatus) => {
                let eng = self.engine().await;
                let st = eng.world_status().await;
                Ok(enc_world_status(&st))
            }

            Some(HotkeyMethod::GetWorldSnapshot) => {
                let eng = self.engine().await;
                let world = eng.world();
                let displays = world.displays().await;
                let focused_app =
                    world
                        .focused_context()
                        .await
                        .map(|(app, title, pid)| App { app, title, pid });

                let payload = build_snapshot_payload(displays, focused_app);

                enc_world_snapshot(&payload).map_err(|e| {
                    Self::typed_err(
                        crate::error::RpcErrorCode::InvalidType,
                        &[("message", Value::String(e.to_string().into()))],
                    )
                })
            }

            Some(HotkeyMethod::GetServerStatus) => {
                enc_server_status(&self.snapshot_server_status().await).map_err(|e| {
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
        let event_tx = self.event_tx.clone();

        // Bridge: dedicated OS thread blocks on crossbeam and forwards to Tokio mpsc
        let rx_cross = manager.events();
        let mut rx_ev = crate::util::bridge_crossbeam_to_tokio(rx_cross);

        // Async task consumes Tokio channel and dispatches events with per-id ordering
        tokio::spawn(async move {
            const PER_ID_QUEUE_CAPACITY: usize = 64;
            let mut workers: HashMap<u32, tokio::sync::mpsc::Sender<mac_hotkey::Event>> =
                HashMap::new();

            while let Some(ev) = rx_ev.recv().await {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }

                let id = ev.id;
                let ev = if let Some(tx) = workers.get(&id) {
                    match tx.try_send(ev) {
                        Ok(()) => continue,
                        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                            trace!(id, "per_id_queue_full_drop");
                            continue;
                        }
                        Err(tokio::sync::mpsc::error::TrySendError::Closed(ev)) => {
                            workers.remove(&id);
                            ev
                        }
                    }
                } else {
                    ev
                };

                let (tx, mut rx) =
                    tokio::sync::mpsc::channel::<mac_hotkey::Event>(PER_ID_QUEUE_CAPACITY);
                let _ = tx.try_send(ev);
                workers.insert(id, tx.clone());

                let engine = engine.clone();
                let manager = manager.clone();
                let event_tx = event_tx.clone();
                let shutdown = shutdown.clone();
                tokio::spawn(async move {
                    let eng = engine
                        .get_or_init(|| async { Engine::new(manager, event_tx) })
                        .await;
                    while let Some(ev) = rx.recv().await {
                        if shutdown.load(Ordering::SeqCst) {
                            break;
                        }
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
                });
            }
        });

        debug!("Hotkey dispatcher started with per-id ordering");
    }
}

fn build_snapshot_payload(
    displays: hotki_world::DisplaysSnapshot,
    focused: Option<App>,
) -> WorldSnapshotLite {
    WorldSnapshotLite { focused, displays }
}

/// Encode world status into an MRPC value for transport.
fn enc_world_status(ws: &hotki_world::WorldStatus) -> Value {
    match rmp_serde::to_vec_named(ws) {
        Ok(bytes) => Value::Binary(bytes),
        Err(_) => Value::Nil,
    }
}

/// Decode `set_config` params.
pub(crate) fn dec_set_config_param(v: &Value) -> Result<config::Config, mrpc::RpcError> {
    match v {
        Value::Binary(bytes) => rmp_serde::from_slice::<config::Config>(bytes).map_err(|e| {
            mrpc::RpcError::Service(mrpc::ServiceError {
                name: crate::error::RpcErrorCode::InvalidConfig.to_string(),
                value: Value::String(e.to_string().into()),
            })
        }),
        _ => Err(mrpc::RpcError::Service(mrpc::ServiceError {
            name: crate::error::RpcErrorCode::InvalidType.to_string(),
            value: Value::String("expected binary msgpack".into()),
        })),
    }
}

/// Encode a generic UI event for notifications to clients.
pub(crate) fn enc_event(event: &hotki_protocol::MsgToUI) -> crate::Result<Value> {
    hotki_protocol::ipc::codec::msg_to_value(event)
        .map_err(|e| crate::Error::Serialization(e.to_string()))
}

/// Encode a server status snapshot to msgpack binary `Value`.
fn enc_server_status(status: &ServerStatusLite) -> crate::Result<Value> {
    let bytes = rmp_serde::to_vec_named(status)?;
    Ok(Value::Binary(bytes))
}

/// Encode a world snapshot to msgpack binary `Value`.
fn enc_world_snapshot(snap: &WorldSnapshotLite) -> crate::Result<Value> {
    let bytes = rmp_serde::to_vec_named(snap)?;
    Ok(Value::Binary(bytes))
}

/// Decode `inject_key` param from msgpack binary.
pub(crate) fn dec_inject_key_param(v: &Value) -> Result<InjectKeyReq, mrpc::RpcError> {
    match v {
        Value::Binary(bytes) => rmp_serde::from_slice::<InjectKeyReq>(bytes).map_err(|e| {
            mrpc::RpcError::Service(mrpc::ServiceError {
                name: crate::error::RpcErrorCode::InvalidConfig.to_string(),
                value: Value::String(e.to_string().into()),
            })
        }),
        _ => Err(mrpc::RpcError::Service(mrpc::ServiceError {
            name: crate::error::RpcErrorCode::InvalidType.to_string(),
            value: Value::String("expected binary msgpack".into()),
        })),
    }
}

/// Helper to convert protocol injection kind to internal event kind.
fn inject_kind_to_event(kind: InjectKind) -> mac_hotkey::EventKind {
    match kind {
        InjectKind::Down => mac_hotkey::EventKind::KeyDown,
        InjectKind::Up => mac_hotkey::EventKind::KeyUp,
    }
}
