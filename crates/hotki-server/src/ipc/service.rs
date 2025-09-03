//! IPC service implementation for hotkey manager
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
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use mrpc::{Connection as MrpcConnection, RpcError, RpcSender, ServiceError, Value};
use tokio::sync::{Mutex as AsyncMutex, mpsc::UnboundedSender};
use tracing::{debug, error, info, trace, warn};

use crate::ipc::rpc::{HotkeyMethod, HotkeyNotification};
use hotki_engine::Engine;
use hotki_protocol::MsgToUI;

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
    /// Ensure focus watcher is only started once per server lifetime
    watcher_started: Arc<AtomicBool>,
    /// Optional cap on per-id in-flight events (worker queue capacity)
    per_id_capacity: Option<usize>,
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
            watcher_started: Arc::new(AtomicBool::new(false)),
            per_id_capacity: None,
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

    /// Broadcast an event to all connected clients
    async fn broadcast_event(&self, event: MsgToUI) {
        if self.shutdown.load(Ordering::SeqCst) {
            return;
        }
        // Clone the current client list for sending without holding the lock
        let clients_snapshot = { self.clients.lock().await.clone() };

        // Convert event to MRPC Value (binary serde payload)
        let value = crate::ipc::rpc::enc_event(&event);

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
        info!("Client connected via MRPC");

        // Add client to list for event broadcasting
        self.clients.lock().await.push(client.clone());

        // Start event forwarding if not already started
        let event_rx = match self.event_rx.lock() {
            Ok(mut guard) => guard.take(),
            Err(e) => {
                error!("Failed to access event receiver: {}", e);
                None
            }
        };
        if let Some(event_rx) = event_rx {
            let service_clone = self.clone();
            tokio::spawn(async move {
                service_clone.forward_events(event_rx).await;
            });
        }

        // Set up log forwarding to this client
        // Temporarily disabled during investigation of a UI disconnect regression
        // observed after mac-winops consolidation. HUD + smoketests rely on
        // HudUpdate events and do not require server log forwarding.
        // log_forward::set_sink(self.event_tx.clone());

        // Proactively send an initial status snapshot to this client
        if let Err(e) = self.ensure_engine_initialized().await {
            return Err(Self::typed_err(
                crate::error::RpcErrorCode::EngineInit,
                &[("message", Value::String(e.to_string().into()))],
            ));
        }
        // No initial status snapshot; UI derives state from HudUpdate events.

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
                let _ = mac_winops::focus::wake_main_loop();

                // Stop forwarding any further logs/events to clients
                log_forward::clear_sink();

                // Drop all clients so no further notifications are attempted
                self.clients.lock().await.clear();

                // Close the UI event pipeline
                if let Ok(mut r) = self.event_rx.lock() {
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
                info!("Setting config via MRPC");

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
                if let Err(e) = engine.set_config(cfg).await {
                    return Err(Self::typed_err(
                        crate::error::RpcErrorCode::EngineSetConfig,
                        &[("message", Value::String(e.to_string().into()))],
                    ));
                }

                // Start focus watcher if needed
                let need_start = !self.watcher_started.swap(true, Ordering::SeqCst);
                drop(engine_guard);
                if need_start {
                    // Temporarily disable focus watcher during investigation to isolate HUD failure
                    // self.start_focus_watcher().await;
                }

                Ok(Value::Boolean(true))
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
    async fn start_focus_watcher(&self) {
        use mac_winops::focus::{FocusEvent, start_watcher};

        let (tx_focus, mut rx_focus) = tokio::sync::mpsc::unbounded_channel::<FocusEvent>();
        if let Err(e) = start_watcher(tx_focus) {
            tracing::warn!("Failed to start focus watcher: {}", e);

            // Surface a user-facing notification with actionable guidance.
            // Check current permissions to tailor the message.
            let perms = permissions::check_permissions();
            let mut hints: Vec<&str> = Vec::new();
            if !perms.accessibility_ok {
                hints.push("Accessibility");
            }
            if !perms.input_ok {
                hints.push("Input Monitoring");
            }
            let hint_str = if hints.is_empty() {
                "Ensure the app is not sandboxed improperly and restart Hotki."
            } else if hints.len() == 1 && hints[0] == "Accessibility" {
                "Grant Accessibility permission in System Settings → Privacy & Security → Accessibility, then restart Hotki."
            } else if hints.len() == 1 && hints[0] == "Input Monitoring" {
                "Grant Input Monitoring permission in System Settings → Privacy & Security → Input Monitoring, then restart Hotki."
            } else {
                "Grant Accessibility and Input Monitoring permissions in System Settings → Privacy & Security, then restart Hotki."
            };

            let msg = format!("Focus watcher failed to start ({}). {}", e, hint_str);
            let _ = self.event_tx.send(hotki_protocol::MsgToUI::Notify {
                kind: hotki_protocol::NotifyKind::Error,
                title: "Focus Watcher".to_string(),
                text: msg,
            });
        }

        let engine = self.engine.clone();
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx_focus.recv().await {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let mut eng_guard = engine.lock().await;
                if let Some(eng) = eng_guard.as_mut()
                    && let Err(e) = eng.on_focus_event(ev).await
                {
                    tracing::warn!("Engine focus update failed: {}", e);
                }
            }
        });

        info!("Focus watcher started");
    }

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
        svc
    }
}
