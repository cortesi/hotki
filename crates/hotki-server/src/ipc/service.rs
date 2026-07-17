//! IPC service implementation for hotkey manager
//!
//! World read path
//! - `hotki-world` is authoritative for window/focus state. The service
//!   ensures a single forwarder instance per server and relays `WorldEvent`s
//!   to the UI stream, with snapshot-on-reconnect semantics.
//! - There are no CoreGraphics/AX focus fallbacks in the engine; physical
//!   dispatch waits for a world refresh before resolving contextual bindings.
//!
//! # Locking Strategy
//!
//! - Prefer Tokio locks inside async paths. The `clients` list uses
//!   `tokio::sync::Mutex` to avoid mixing where we `await` soon after.
//! - The event pipeline moves its receiver and task handles through one Tokio
//!   lifecycle mutex, releasing the guard before task joins or network work.
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
use hotki_protocol::rpc::{HotkeyMethod, RpcErrorCode, RpcFailure, ServerStatusLite};
use mrpc::{Connection as MrpcConnection, RpcError, RpcSender, Value};
pub(crate) use rpc::dec_inject_key_param;
use rpc::{
    build_snapshot_payload, enc_server_status, enc_world_snapshot, enc_world_status,
    inject_kind_to_event, string_param, typed_err,
};
use tokio::sync::OnceCell;
use tracing::{debug, info, trace, warn};
use workers::{DispatchResult, WorkerPool, WorkerRuntime};

use super::{IdleTimerSnapshot, IdleTimerState};
use crate::{
    loop_wake::{self, WakeEvent},
    shutdown::{ShutdownCoordinator, ShutdownReason},
};

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
    /// Shared idempotent shutdown transition.
    shutdown: ShutdownCoordinator,
    /// Active per-hotkey worker pool.
    workers: WorkerPool,
}

impl HotkeyService {
    pub fn new(
        manager: Arc<mac_hotkey::Manager>,
        shutdown: ShutdownCoordinator,
        idle_state: Arc<IdleTimerState>,
    ) -> Self {
        Self {
            engine: Arc::new(OnceCell::new()),
            manager: manager.clone(),
            events: EventPipeline::new(shutdown.flag(), manager),
            idle_state,
            shutdown,
            workers: WorkerPool::new(),
        }
    }

    /// Expose the shutdown flag for coordinated server shutdown.
    pub(crate) fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown.flag()
    }

    /// Expose the shared shutdown coordinator.
    pub(crate) fn shutdown(&self) -> ShutdownCoordinator {
        self.shutdown.clone()
    }

    /// Stop and join every shared server event task.
    pub(crate) async fn stop_event_pipeline(&self) {
        self.events.shutdown().await;
    }

    async fn engine(&self) -> &Engine {
        self.engine
            .get_or_init(|| async {
                let engine = Engine::new(self.manager.clone(), self.events.sender());
                let _ = self.events.start(engine.world()).await;
                engine
            })
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
            input: input_health(self.manager.status()),
        }
    }

    async fn handle_shutdown_request(&self) -> StdResult<Value, RpcError> {
        info!("Shutdown request received");
        self.shutdown.request(ShutdownReason::Rpc);
        self.stop_event_pipeline().await;
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
            return Err(typed_err(RpcFailure::new(
                RpcErrorCode::EngineSetConfig,
                err.to_string(),
            )));
        }
        Ok(Value::Boolean(true))
    }

    async fn handle_inject_key(&self, params: &[Value]) -> StdResult<Value, RpcError> {
        if params.is_empty() {
            return Err(typed_err(
                RpcFailure::new(
                    RpcErrorCode::MissingParams,
                    "inject_key requires an inject request",
                )
                .with_method(HotkeyMethod::InjectKey.as_str())
                .with_expected("inject request"),
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
            .dispatch_injected(&req.ident, inject_kind_to_event(req.kind), req.repeat)
            .await
        {
            Ok(true) => Ok(Value::Boolean(true)),
            Ok(false) => {
                tracing::debug!(
                    target: "hotki_server::ipc::service",
                    "InjectKey: ident not bound: {}",
                    req.ident
                );
                let message = format!("key is not bound: {}", req.ident);
                Err(typed_err(
                    RpcFailure::new(RpcErrorCode::KeyNotBound, message).with_ident(req.ident),
                ))
            }
            Err(err) => {
                tracing::warn!(
                    target: "hotki_server::ipc::service",
                    "InjectKey: dispatch failed ident={}: {}",
                    req.ident,
                    err
                );
                Err(typed_err(RpcFailure::new(
                    RpcErrorCode::EngineDispatch,
                    err.to_string(),
                )))
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
        Ok(enc_world_status(&engine.world_status()))
    }

    async fn handle_get_world_snapshot(&self) -> StdResult<Value, RpcError> {
        let engine = self.engine().await;
        let world = engine.world();
        let displays = world.displays();
        let focused_app = world.focus_snapshot();
        let payload = build_snapshot_payload(displays, focused_app);
        enc_world_snapshot(&payload)
            .map_err(|err| typed_err(RpcFailure::new(RpcErrorCode::InvalidType, err.to_string())))
    }

    async fn handle_get_server_status(&self) -> StdResult<Value, RpcError> {
        enc_server_status(&self.snapshot_server_status().await)
            .map_err(|err| typed_err(RpcFailure::new(RpcErrorCode::InvalidType, err.to_string())))
    }

    async fn route_request(
        &self,
        method: HotkeyMethod,
        params: &[Value],
    ) -> StdResult<Value, RpcError> {
        match method {
            HotkeyMethod::Shutdown => self.handle_shutdown_request().await,
            HotkeyMethod::SetConfigPath => self.handle_set_config_path(params).await,
            HotkeyMethod::InjectKey => self.handle_inject_key(params).await,
            HotkeyMethod::GetBindings => self.handle_get_bindings().await,
            HotkeyMethod::GetDepth => self.handle_get_depth().await,
            HotkeyMethod::GetWorldStatus => self.handle_get_world_status().await,
            HotkeyMethod::GetWorldSnapshot => self.handle_get_world_snapshot().await,
            HotkeyMethod::GetServerStatus => self.handle_get_server_status().await,
        }
    }
}

/// Convert manager-owned observations into the server's canonical protocol status.
fn input_health(status: mac_hotkey::ManagerStatus) -> hotki_protocol::InputHealth {
    let tap_mode = match status.tap_mode {
        mac_hotkey::TapMode::Physical => hotki_protocol::TapMode::Physical,
        mac_hotkey::TapMode::InjectionOnly => hotki_protocol::TapMode::InjectionOnly,
    };
    let tap_lifecycle = match status.tap_lifecycle {
        mac_hotkey::TapLifecycle::Starting => hotki_protocol::TapLifecycle::Starting,
        mac_hotkey::TapLifecycle::Running => hotki_protocol::TapLifecycle::Running,
        mac_hotkey::TapLifecycle::Stopped => hotki_protocol::TapLifecycle::Stopped,
    };
    let secure_input = match status.secure_input {
        mac_hotkey::SecureInputState::Unknown => hotki_protocol::SecureInputState::Unknown,
        mac_hotkey::SecureInputState::Inactive => hotki_protocol::SecureInputState::Inactive,
        mac_hotkey::SecureInputState::Active => hotki_protocol::SecureInputState::Active,
    };
    let blocked = secure_input == hotki_protocol::SecureInputState::Active
        && tap_mode == hotki_protocol::TapMode::Physical
        && tap_lifecycle == hotki_protocol::TapLifecycle::Running
        && status.registered_hotkeys > 0;
    hotki_protocol::InputHealth {
        tap_mode,
        tap_lifecycle,
        secure_input,
        secure_input_owner: status.secure_input_owner.map(|owner| {
            hotki_protocol::SecureInputOwner {
                pid: owner.pid,
                app_name: owner.app_name,
            }
        }),
        blocked,
        registered_hotkeys: status.registered_hotkeys,
        physical_event_count: status.physical_event_count,
        physical_event_age_ms: status.physical_event_age_ms,
        os_disable_count: status.os_disable_count,
        os_reenable_count: status.os_reenable_count,
        observed_at_ms: status.observed_at_ms,
        server_pid: status.server_pid,
    }
}

fn unknown_method(method: &str) -> RpcError {
    warn!("Unknown method: {}", method);
    typed_err(
        RpcFailure::new(
            RpcErrorCode::MethodNotFound,
            format!("method '{method}' not found"),
        )
        .with_method(method),
    )
}

#[async_trait]
impl MrpcConnection for HotkeyService {
    async fn connected(&self, client: RpcSender) -> StdResult<(), RpcError> {
        if self.shutdown.is_requested() {
            // Refuse new connections during shutdown
            return Err(typed_err(RpcFailure::new(
                RpcErrorCode::ShuttingDown,
                "server is shutting down",
            )));
        }
        debug!("Client connected via MRPC");

        self.events.register_client(client).await;
        let _ = self.engine().await;
        let _ = loop_wake::post_user_event(WakeEvent::ClientConnected);

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
        let id = ev.id;
        let kind = ev.kind;
        let engine = self.engine.clone();
        let manager = self.manager.clone();
        let event_tx = self.events.sender();
        let shutdown = self.shutdown_flag();
        let result = self.workers.dispatch(ev, || {
            WorkerRuntime::new(
                engine.clone(),
                manager.clone(),
                event_tx.clone(),
                shutdown.clone(),
            )
        });
        self.handle_worker_dispatch_result(result, id, kind);
    }

    fn handle_worker_dispatch_result(
        &self,
        result: DispatchResult,
        id: u32,
        kind: mac_hotkey::EventKind,
    ) {
        match result {
            DispatchResult::Queued => {}
            DispatchResult::QueueClosed => {
                warn!(
                    target: "hotki_server::ipc::service",
                    id,
                    kind = ?kind,
                    "replacement worker closed before hotkey event could be dispatched"
                );
            }
            DispatchResult::QueueFull => {
                warn!(
                    target: "hotki_server::ipc::service",
                    id,
                    kind = ?kind,
                    "worker queue full before hotkey event could be dispatched"
                );
            }
            DispatchResult::ReleaseQueued => {
                warn!(
                    target: "hotki_server::ipc::service",
                    id,
                    "worker queue saturated; retained ordered key release"
                )
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
    use super::*;

    #[test]
    fn unknown_method_returns_typed_service_error() {
        let RpcError::Service(service) = unknown_method("bogus") else {
            panic!("expected service error");
        };
        let failure = hotki_protocol::rpc::decode_rpc_failure(&service).expect("decode failure");
        assert_eq!(failure.code, RpcErrorCode::MethodNotFound);
        assert_eq!(failure.payload.message, "method 'bogus' not found");
        assert_eq!(failure.payload.fields.method.as_deref(), Some("bogus"));
    }

    fn manager_status(
        mode: mac_hotkey::TapMode,
        lifecycle: mac_hotkey::TapLifecycle,
        secure_input: mac_hotkey::SecureInputState,
        registered_hotkeys: usize,
    ) -> mac_hotkey::ManagerStatus {
        mac_hotkey::ManagerStatus {
            tap_mode: mode,
            tap_lifecycle: lifecycle,
            secure_input,
            secure_input_owner: None,
            registered_hotkeys,
            physical_event_count: 0,
            physical_event_age_ms: None,
            os_disable_count: 0,
            os_reenable_count: 0,
            observed_at_ms: None,
            server_pid: 1,
        }
    }

    #[test]
    fn blocked_requires_active_running_physical_tap_with_bindings() {
        let blocked = input_health(manager_status(
            mac_hotkey::TapMode::Physical,
            mac_hotkey::TapLifecycle::Running,
            mac_hotkey::SecureInputState::Active,
            1,
        ));
        assert!(blocked.blocked);

        for status in [
            manager_status(
                mac_hotkey::TapMode::InjectionOnly,
                mac_hotkey::TapLifecycle::Stopped,
                mac_hotkey::SecureInputState::Unknown,
                1,
            ),
            manager_status(
                mac_hotkey::TapMode::Physical,
                mac_hotkey::TapLifecycle::Stopped,
                mac_hotkey::SecureInputState::Active,
                1,
            ),
            manager_status(
                mac_hotkey::TapMode::Physical,
                mac_hotkey::TapLifecycle::Running,
                mac_hotkey::SecureInputState::Active,
                0,
            ),
            manager_status(
                mac_hotkey::TapMode::Physical,
                mac_hotkey::TapLifecycle::Running,
                mac_hotkey::SecureInputState::Inactive,
                1,
            ),
        ] {
            assert!(!input_health(status).blocked);
        }
    }
}
