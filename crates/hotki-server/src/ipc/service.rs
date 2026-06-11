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
mod workers;

use std::{
    path::PathBuf,
    result::Result as StdResult,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use events::EventPipeline;
use hotki_engine::Engine;
use hotki_protocol::rpc::{HotkeyMethod, ServerStatusLite};
use mrpc::{Connection as MrpcConnection, RpcError, RpcSender, Value};
pub(crate) use rpc::dec_inject_key_param;
use rpc::{
    build_snapshot_payload, enc_server_status, enc_world_snapshot, enc_world_status,
    inject_kind_to_event, string_param, typed_err,
};
use tokio::sync::OnceCell;
use tracing::{debug, info, trace, warn};
use workers::{WorkerPool, WorkerRuntime};

use super::{IdleTimerSnapshot, IdleTimerState};
use crate::{error::RpcErrorCode, loop_wake};

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
    /// Active per-hotkey worker pool.
    workers: WorkerPool,
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
            workers: WorkerPool::new(),
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
            RpcErrorCode::MissingParams,
        )?;
        let engine = self.engine().await;
        if let Err(err) = engine.set_config_path(PathBuf::from(raw_path)).await {
            return Err(typed_err(
                RpcErrorCode::EngineSetConfig,
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
            RpcErrorCode::MissingParams,
        )?;
        let engine = self.engine().await;
        if let Err(err) = engine.set_theme(raw_name.as_str()).await {
            return Err(typed_err(
                RpcErrorCode::EngineSetConfig,
                &[("message", Value::String(err.to_string().into()))],
            ));
        }
        Ok(Value::Boolean(true))
    }

    async fn handle_inject_key(&self, params: &[Value]) -> StdResult<Value, RpcError> {
        if params.is_empty() {
            return Err(typed_err(
                RpcErrorCode::MissingParams,
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
        match engine
            .dispatch_ident(&req.ident, inject_kind_to_event(req.kind), req.repeat)
            .await
        {
            Ok(true) => Ok(Value::Boolean(true)),
            Ok(false) => {
                tracing::debug!(
                    target: "hotki_server::ipc::service",
                    "InjectKey: ident not bound: {}",
                    req.ident
                );
                Err(typed_err(
                    RpcErrorCode::KeyNotBound,
                    &[("ident", Value::String(req.ident.into()))],
                ))
            }
            Err(err) => {
                tracing::warn!(
                    target: "hotki_server::ipc::service",
                    "InjectKey: dispatch failed ident={}: {}",
                    req.ident,
                    err
                );
                Err(typed_err(
                    RpcErrorCode::EngineDispatch,
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
                RpcErrorCode::InvalidType,
                &[("message", Value::String(err.to_string().into()))],
            )
        })
    }

    async fn handle_get_server_status(&self) -> StdResult<Value, RpcError> {
        enc_server_status(&self.snapshot_server_status().await).map_err(|err| {
            typed_err(
                RpcErrorCode::InvalidType,
                &[("message", Value::String(err.to_string().into()))],
            )
        })
    }

    async fn route_request(
        &self,
        method: HotkeyMethod,
        params: &[Value],
    ) -> StdResult<Value, RpcError> {
        match method {
            HotkeyMethod::Shutdown => self.handle_shutdown_request().await,
            HotkeyMethod::SetConfigPath => self.handle_set_config_path(params).await,
            HotkeyMethod::SetTheme => self.handle_set_theme(params).await,
            HotkeyMethod::InjectKey => self.handle_inject_key(params).await,
            HotkeyMethod::GetBindings => self.handle_get_bindings().await,
            HotkeyMethod::GetDepth => self.handle_get_depth().await,
            HotkeyMethod::GetWorldStatus => self.handle_get_world_status().await,
            HotkeyMethod::GetWorldSnapshot => self.handle_get_world_snapshot().await,
            HotkeyMethod::GetServerStatus => self.handle_get_server_status().await,
        }
    }
}

fn unknown_method(method: &str) -> RpcError {
    warn!("Unknown method: {}", method);
    typed_err(
        RpcErrorCode::MethodNotFound,
        &[("method", Value::String(method.into()))],
    )
}

#[async_trait]
impl MrpcConnection for HotkeyService {
    async fn connected(&self, client: RpcSender) -> StdResult<(), RpcError> {
        if self.shutdown_flag().load(Ordering::SeqCst) {
            // Refuse new connections during shutdown
            return Err(typed_err(
                RpcErrorCode::ShuttingDown,
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
            Some(method) => self.route_request(method, &params).await,
            None => Err(unknown_method(method)),
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
        let engine = self.engine.clone();
        let manager = self.manager.clone();
        let event_tx = self.events.sender();
        let shutdown = self.shutdown_flag();
        self.workers.dispatch(ev, || {
            WorkerRuntime::new(engine, manager, event_tx, shutdown)
        });
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
    use super::*;

    #[test]
    fn unknown_method_returns_typed_service_error() {
        let RpcError::Service(service) = unknown_method("bogus") else {
            panic!("expected service error");
        };
        assert_eq!(service.name, RpcErrorCode::MethodNotFound.to_string());
    }
}
