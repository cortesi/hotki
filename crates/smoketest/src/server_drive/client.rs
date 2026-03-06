use std::{
    collections::{BTreeSet, VecDeque},
    env, io,
    path::Path,
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

use hotki_protocol::rpc::InjectKind;
use hotki_server::smoketest_bridge::{
    BlockingBridgeClient, BridgeClientError, BridgeEvent, BridgeReply, BridgeRequest,
    BridgeResponse, control_socket_path,
};
use tracing::debug;

use super::{
    BridgeEventRecord, BridgeHandshake, DriverError, DriverResult, HudSnapshot,
    types::{
        canonicalize_ident, describe_init_error, ensure_clean_handshake,
        message_contains_key_not_bound,
    },
};
use crate::config;

/// Flag to enable verbose binding polling diagnostics.
static LOG_BINDINGS: OnceLock<bool> = OnceLock::new();

/// Return whether extra binding-polling diagnostics are enabled for this run.
fn log_bindings_enabled() -> bool {
    *LOG_BINDINGS.get_or_init(|| env::var_os("SMOKETEST_LOG_BINDINGS").is_some())
}

/// Blocking bridge client with lazy initialization and reconnect handling.
pub struct BridgeClient {
    /// Control socket path used to communicate with the UI bridge.
    control_socket: String,
    /// Active bridge transport, when initialized.
    transport: Option<BlockingBridgeClient>,
    /// Circular buffer of recent bridge events.
    event_buffer: VecDeque<BridgeEventRecord>,
    /// Latest HUD snapshot emitted by the bridge.
    latest_hud: Option<HudSnapshot>,
    /// Most recent handshake data captured during initialization.
    handshake: Option<BridgeHandshake>,
}

impl BridgeClient {
    /// Maximum number of reconnection attempts per bridge call.
    const MAX_RECONNECT_ATTEMPTS: u32 = 3;
    /// Maximum number of bridge events retained in memory.
    const EVENT_BUFFER_CAPACITY: usize = 128;

    /// Construct a client for the provided server socket path.
    #[must_use]
    pub fn new(server_socket: impl Into<String>) -> Self {
        let server_socket = server_socket.into();
        Self {
            control_socket: control_socket_path(&server_socket),
            transport: None,
            event_buffer: VecDeque::new(),
            latest_hud: None,
            handshake: None,
        }
    }

    /// Drop the current bridge connection so the next operation reconnects from scratch.
    pub fn reset(&mut self) {
        self.transport = None;
        self.clear_cached_state();
    }

    /// Ensure the bridge connection is initialized within `timeout_ms`.
    pub fn ensure_ready(&mut self, timeout_ms: u64) -> DriverResult<()> {
        if self.transport.is_some() {
            return Ok(());
        }

        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut last_error: Option<String> = None;

        while Instant::now() < deadline {
            match Self::connect_transport(&self.control_socket) {
                Ok(mut transport) => match self.refresh_handshake(&mut transport) {
                    Ok(()) => {
                        self.transport = Some(transport);
                        return Ok(());
                    }
                    Err(err) => {
                        last_error = Some(describe_init_error(&err));
                        debug!(
                            error = %last_error.as_ref().unwrap(),
                            socket = %self.control_socket,
                            "bridge initialization attempt failed"
                        );
                    }
                },
                Err(err) => {
                    last_error = Some(describe_init_error(&err));
                    debug!(
                        error = %last_error.as_ref().unwrap(),
                        socket = %self.control_socket,
                        "bridge initialization attempt failed"
                    );
                }
            }
            self.reset();
            thread::sleep(config::ms(config::RETRY.fast_delay_ms));
        }

        Err(DriverError::InitTimeout {
            socket_path: self.control_socket.clone(),
            timeout_ms,
            last_error: last_error
                .unwrap_or_else(|| "no connection attempts were made".to_string()),
        })
    }

    /// Attempt a graceful shutdown via the active bridge connection, if available.
    pub fn shutdown(&mut self) -> DriverResult<()> {
        self.require_transport()?;
        let baseline = self.event_buffer.len();
        self.call_ok(&BridgeRequest::Shutdown)?;
        match self.assert_no_new_events_since(baseline) {
            Ok(()) => {
                self.reset();
                Ok(())
            }
            Err(err) => {
                self.reset();
                Err(err)
            }
        }
    }

    /// Inject a single key press (down + up) via the bridge.
    pub fn inject_key(&mut self, seq: &str) -> DriverResult<()> {
        let ident = canonicalize_ident(seq);
        let gate_ms = config::BINDING_GATES.default_ms;
        let mut targets = BTreeSet::new();
        targets.insert(ident.clone());

        self.wait_for_hud_keys(&targets, gate_ms)?;

        let deadline = Instant::now() + Duration::from_millis(gate_ms);
        loop {
            let baseline = self.latest_hud.as_ref().map(|snapshot| snapshot.event_id);
            match self.call_ok(&BridgeRequest::InjectKey {
                ident: ident.clone(),
                kind: InjectKind::Down,
                repeat: false,
            }) {
                Ok(()) => {
                    let hud_wait_ms = config::INPUT_DELAYS.retry_delay_ms.max(10);
                    let _ = self.wait_for_hud_progress_since(baseline, hud_wait_ms)?;
                    break;
                }
                Err(DriverError::BridgeFailure { message })
                    if message_contains_key_not_bound(&message) =>
                {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(DriverError::BindingTimeout {
                            ident,
                            timeout_ms: gate_ms,
                        });
                    }
                    let remaining_ms = deadline.saturating_duration_since(now).as_millis() as u64;
                    if remaining_ms == 0 {
                        return Err(DriverError::BindingTimeout {
                            ident,
                            timeout_ms: gate_ms,
                        });
                    }
                    self.wait_for_hud_keys(&targets, remaining_ms)?;
                }
                Err(err) => return Err(err),
            }
        }

        match self.call_ok(&BridgeRequest::InjectKey {
            ident,
            kind: InjectKind::Up,
            repeat: false,
        }) {
            Ok(()) => Ok(()),
            Err(DriverError::BridgeFailure { message })
                if message_contains_key_not_bound(&message) =>
            {
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    /// Load a configuration from disk and apply it to the running server.
    pub fn set_config_from_path(&mut self, path: &Path) -> DriverResult<()> {
        let path_str = path.to_str().ok_or_else(|| DriverError::BridgeFailure {
            message: format!("non-UTF-8 config path: {}", path.display()),
        })?;
        self.call_ok(&BridgeRequest::SetConfig {
            path: path_str.to_string(),
        })
    }

    /// Wait until all identifiers are present in the current bindings.
    pub fn wait_for_idents(&mut self, idents: &[&str], timeout_ms: u64) -> DriverResult<()> {
        if idents.is_empty() {
            return Ok(());
        }

        let wanted: BTreeSet<String> = idents
            .iter()
            .map(|ident| canonicalize_ident(ident))
            .collect();
        self.wait_for_hud_keys(&wanted, timeout_ms)
    }

    /// Quick liveness probe against the backend via a lightweight bridge command.
    #[cfg(test)]
    pub fn check_alive(&mut self) -> DriverResult<()> {
        self.call_depth().map(|_| ())
    }

    /// Fetch the current depth reported by the bridge.
    #[cfg(test)]
    pub fn get_depth(&mut self) -> DriverResult<usize> {
        self.call_depth()
    }

    /// Retrieve the latest HUD snapshot observed on the bridge.
    pub fn latest_hud(&self) -> DriverResult<Option<HudSnapshot>> {
        self.require_transport()?;
        Ok(self.latest_hud.clone())
    }

    /// Drain buffered bridge events for inspection.
    pub fn drain_bridge_events(&mut self) -> DriverResult<Vec<BridgeEventRecord>> {
        self.transport_mut()?;
        Ok(self.event_buffer.drain(..).collect())
    }

    /// Retrieve the most recent handshake snapshot, if initialized.
    #[cfg(test)]
    pub fn handshake(&self) -> DriverResult<Option<BridgeHandshake>> {
        self.require_transport()?;
        Ok(self.handshake.clone())
    }

    /// Return the number of events currently buffered in the client.
    #[cfg(test)]
    pub fn event_buffer_len(&self) -> DriverResult<usize> {
        self.require_transport()?;
        Ok(self.event_buffer.len())
    }

    /// Connect a fresh transport to the bridge control socket.
    fn connect_transport(path: &str) -> DriverResult<BlockingBridgeClient> {
        BlockingBridgeClient::connect(path, Duration::from_millis(config::BRIDGE.ack_timeout_ms))
            .map_err(|source| DriverError::Connect {
                socket_path: path.to_string(),
                source,
            })
    }

    /// Refresh the initial handshake and reset cached bridge state.
    fn refresh_handshake(&mut self, transport: &mut BlockingBridgeClient) -> DriverResult<()> {
        self.clear_cached_state();
        let mut events = Vec::new();
        let response = transport
            .call(&BridgeRequest::Ping, |reply| events.push(reply))
            .map_err(DriverError::from)?;
        for reply in events {
            self.record_event(reply);
        }
        match response {
            BridgeResponse::Handshake {
                idle_timer,
                notifications,
            } => {
                let payload = BridgeHandshake {
                    idle_timer,
                    notifications,
                };
                ensure_clean_handshake(&payload)?;
                self.handshake = Some(payload);
                Ok(())
            }
            BridgeResponse::Err { message } => Err(DriverError::BridgeFailure { message }),
            other => Err(DriverError::BridgeFailure {
                message: format!("unexpected handshake response: {:?}", other),
            }),
        }
    }

    /// Execute one bridge request, reconnecting if the socket drops mid-call.
    fn call(&mut self, req: &BridgeRequest) -> DriverResult<BridgeResponse> {
        let request = req.clone();
        let mut attempt = 0;
        loop {
            let mut events = Vec::new();
            let result = {
                let transport = self.transport.as_mut().ok_or(DriverError::NotInitialized)?;
                transport.call(&request, |reply| events.push(reply))
            };
            for reply in events {
                self.record_event(reply);
            }
            match result {
                Ok(response) => return Ok(response),
                Err(BridgeClientError::Io { source })
                    if connection_lost(&source) && attempt < Self::MAX_RECONNECT_ATTEMPTS =>
                {
                    attempt += 1;
                    self.reconnect_with_backoff(attempt)?;
                }
                Err(err) => return Err(DriverError::from(err)),
            }
        }
    }

    /// Execute a request that should return a plain success response.
    fn call_ok(&mut self, req: &BridgeRequest) -> DriverResult<()> {
        self.call(req)?
            .into_result()
            .map_err(|message| DriverError::BridgeFailure { message })
    }

    /// Fetch the binding identifiers currently reported by the server.
    fn call_bindings(&mut self) -> DriverResult<Vec<String>> {
        self.call(&BridgeRequest::GetBindings)?
            .into_bindings()
            .map_err(|message| DriverError::BridgeFailure { message })
    }

    #[cfg(test)]
    fn call_depth(&mut self) -> DriverResult<usize> {
        self.call(&BridgeRequest::GetDepth)?
            .into_depth()
            .map_err(|message| DriverError::BridgeFailure { message })
    }

    /// Ensure the bridge has been initialized before inspecting cached state.
    fn require_transport(&self) -> DriverResult<()> {
        self.transport
            .as_ref()
            .map(|_| ())
            .ok_or(DriverError::NotInitialized)
    }

    /// Borrow the initialized transport mutably.
    fn transport_mut(&mut self) -> DriverResult<&mut BlockingBridgeClient> {
        self.transport.as_mut().ok_or(DriverError::NotInitialized)
    }

    /// Clear all cached bridge-derived state after a reset or reconnect.
    fn clear_cached_state(&mut self) {
        self.event_buffer.clear();
        self.latest_hud = None;
        self.handshake = None;
    }

    /// Verify that shutdown did not race with any late bridge events.
    fn assert_no_new_events_since(&self, baseline: usize) -> DriverResult<()> {
        if let Some(event) = self.event_buffer.get(baseline) {
            return Err(DriverError::PostShutdownMessage {
                message: format!("bridge event observed after shutdown: {:?}", event.payload),
            });
        }
        Ok(())
    }

    /// Record one asynchronous bridge event into the local caches.
    fn record_event(&mut self, reply: BridgeReply) {
        if let BridgeResponse::Event { event } = reply.response {
            let event = *event;
            if self.event_buffer.len() >= Self::EVENT_BUFFER_CAPACITY {
                self.event_buffer.pop_front();
            }
            if let BridgeEvent::Hud { hud, displays } = &event {
                let idents: BTreeSet<String> = hud
                    .rows
                    .iter()
                    .map(|row| canonicalize_ident(&row.chord.to_string()))
                    .collect();
                self.latest_hud = Some(HudSnapshot {
                    event_id: reply.command_id,
                    received_ms: reply.timestamp_ms,
                    hud: (**hud).clone(),
                    displays: displays.clone(),
                    idents,
                });
            }
            self.event_buffer.push_back(BridgeEventRecord {
                id: reply.command_id,
                timestamp_ms: reply.timestamp_ms,
                payload: event,
            });
        }
    }

    /// Check whether the cached HUD snapshot contains every requested identifier.
    fn hud_contains_all(&self, want: &BTreeSet<String>) -> bool {
        if want.is_empty() {
            return true;
        }
        self.latest_hud
            .as_ref()
            .map(|snapshot| want.is_subset(&snapshot.idents))
            .unwrap_or(false)
    }

    /// Wait for the next asynchronous bridge event before `deadline`.
    fn wait_for_bridge_event(&mut self, deadline: Instant) -> DriverResult<bool> {
        let mut events = Vec::new();
        let outcome = {
            let transport = self.transport_mut()?;
            transport.wait_for_event_until(deadline, |reply| events.push(reply))
        }
        .map_err(DriverError::from)?;
        for reply in events {
            self.record_event(reply);
        }
        Ok(outcome)
    }

    /// Wait until the HUD cache or RPC binding snapshot contains all requested identifiers.
    fn wait_for_hud_keys(&mut self, want: &BTreeSet<String>, timeout_ms: u64) -> DriverResult<()> {
        if want.is_empty() {
            return Ok(());
        }

        let start = Instant::now();
        let deadline = start + Duration::from_millis(timeout_ms);

        loop {
            if self.hud_contains_all(want) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                break;
            }
            let _ = self.wait_for_bridge_event(deadline)?;
        }

        if self.hud_contains_all(want) {
            return Ok(());
        }

        let rpc_snapshot = match self.call_bindings() {
            Ok(bindings) => Some(bindings),
            Err(err) => {
                debug!(?err, "failed to fetch bindings snapshot after HUD timeout");
                None
            }
        };

        let current = self
            .latest_hud
            .as_ref()
            .map(|snapshot| snapshot.idents.clone())
            .unwrap_or_default();
        let rpc_view = rpc_snapshot.as_ref().map(|bindings| {
            bindings
                .iter()
                .map(|raw| canonicalize_ident(raw.trim_matches('"')))
                .collect::<Vec<_>>()
        });

        if let Some(view) = &rpc_view {
            let rpc_idents = view.iter().cloned().collect::<BTreeSet<_>>();
            if want.is_subset(&rpc_idents) {
                if log_bindings_enabled() {
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    let hud_view = current.iter().cloned().collect::<Vec<_>>();
                    debug!(
                        elapsed_ms,
                        hud = ?hud_view,
                        rpc = ?view,
                        "wait_for_idents_rpc_match"
                    );
                }
                return Ok(());
            }
        }

        if self.hud_contains_all(want) {
            return Ok(());
        }

        let missing: Vec<String> = want.difference(&current).cloned().collect();

        if log_bindings_enabled() {
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let hud_view = current.iter().cloned().collect::<Vec<_>>();
            debug!(
                elapsed_ms,
                hud = ?hud_view,
                rpc = ?rpc_view,
                missing = ?missing,
                "wait_for_idents_timeout"
            );
        }

        Err(DriverError::BindingTimeout {
            ident: missing.join(", "),
            timeout_ms,
        })
    }

    /// Wait for the HUD snapshot to advance past the provided event baseline.
    fn wait_for_hud_progress_since(
        &mut self,
        baseline: Option<u64>,
        timeout_ms: u64,
    ) -> DriverResult<bool> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let current_id = self.latest_hud.as_ref().map(|snapshot| snapshot.event_id);
            let advanced =
                matches!((baseline, current_id), (_, Some(current)) if Some(current) != baseline);
            if advanced {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            let _ = self.wait_for_bridge_event(deadline)?;
        }
    }

    /// Reconnect the bridge with bounded backoff after a dropped socket.
    fn reconnect_with_backoff(&mut self, attempt: u32) -> DriverResult<()> {
        let mut last_err: Option<io::Error> = None;
        let mut backoff_ms = config::RETRY.fast_delay_ms.saturating_mul(attempt as u64);
        for _ in 0..3 {
            thread::sleep(Duration::from_millis(backoff_ms.max(1)));
            match BlockingBridgeClient::connect(
                &self.control_socket,
                Duration::from_millis(config::BRIDGE.ack_timeout_ms),
            ) {
                Ok(mut transport) => {
                    transport.reset_command_id();
                    self.refresh_handshake(&mut transport)?;
                    self.transport = Some(transport);
                    return Ok(());
                }
                Err(err) => {
                    last_err = Some(err);
                    backoff_ms = backoff_ms.saturating_mul(2);
                }
            }
        }
        let source = last_err.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "bridge reconnect attempts exhausted",
            )
        });
        Err(DriverError::Connect {
            socket_path: self.control_socket.clone(),
            source,
        })
    }
}

impl From<BridgeClientError> for DriverError {
    fn from(source: BridgeClientError) -> Self {
        match source {
            BridgeClientError::BridgeFailure { message } => Self::BridgeFailure { message },
            BridgeClientError::AckTimeout {
                command_id,
                timeout_ms,
            } => Self::AckTimeout {
                command_id,
                timeout_ms,
            },
            BridgeClientError::SequenceMismatch { expected, got } => {
                Self::SequenceMismatch { expected, got }
            }
            BridgeClientError::AckMissing { command_id } => Self::AckMissing { command_id },
            BridgeClientError::Io { source } => Self::Io { source },
        }
    }
}

/// Return whether an I/O error indicates the bridge connection was lost.
fn connection_lost(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::NotConnected
    )
}
