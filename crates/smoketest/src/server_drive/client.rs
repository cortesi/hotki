use std::{
    collections::{BTreeSet, VecDeque},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hotki_protocol::{MsgToUI, rpc::InjectKind};
use hotki_server::{Client, Connection};
use tokio::{
    runtime::{Builder, Runtime},
    time::timeout,
};
use tracing::debug;

use super::{
    DriverEventRecord, DriverResult, HudSnapshot, ServerHandshake,
    types::{
        DriverError, DriverEventId, canonicalize_ident, describe_init_error,
        ensure_clean_handshake, message_contains_key_not_bound,
    },
};
use crate::config;

/// Server driver backed directly by the production MRPC client and event stream.
pub struct ServerDriver {
    /// Active MRPC client, when initialized. Dropped before the runtime.
    client: Option<Client>,
    /// Runtime used to drive async MRPC calls from the synchronous smoketest harness.
    runtime: Runtime,
    /// Server socket path used to connect to the UI-owned backend.
    socket_path: String,
    /// Circular buffer of recent server events.
    event_buffer: VecDeque<DriverEventRecord>,
    /// Latest HUD snapshot emitted by the server.
    latest_hud: Option<HudSnapshot>,
    /// Most recent server handshake data captured during initialization.
    handshake: Option<ServerHandshake>,
    /// Whether this connection has received at least one server event.
    event_stream_ready: bool,
    /// Next local event id assigned to an observed server event.
    next_event_id: DriverEventId,
}

impl ServerDriver {
    /// Maximum number of server events retained in memory.
    const EVENT_BUFFER_CAPACITY: usize = 128;

    /// Construct a driver for the provided server socket path.
    pub fn new(socket_path: impl Into<String>) -> DriverResult<Self> {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| DriverError::Runtime {
                message: err.to_string(),
            })?;

        Ok(Self {
            client: None,
            runtime,
            socket_path: socket_path.into(),
            event_buffer: VecDeque::new(),
            latest_hud: None,
            handshake: None,
            event_stream_ready: false,
            next_event_id: 0,
        })
    }

    /// Drop the current server connection so the next operation reconnects from scratch.
    pub fn reset(&mut self) {
        self.client = None;
        self.clear_cached_state();
    }

    /// Ensure the server connection is initialized within `timeout_ms`.
    pub fn ensure_ready(&mut self, timeout_ms: u64) -> DriverResult<()> {
        if self.client.is_some() && self.handshake.is_some() && self.event_stream_ready {
            return Ok(());
        }

        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut last_error: Option<String> = None;

        while Instant::now() < deadline {
            match self.connect_and_refresh_handshake(deadline, timeout_ms) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_error = Some(describe_init_error(&err));
                    debug!(
                        error = %last_error.as_ref().unwrap(),
                        socket = %self.socket_path,
                        "server driver initialization attempt failed"
                    );
                    self.reset();
                }
            }
            thread::sleep(config::ms(config::RETRY.fast_delay_ms));
        }

        Err(DriverError::InitTimeout {
            socket_path: self.socket_path.clone(),
            timeout_ms,
            last_error: last_error
                .unwrap_or_else(|| "no connection attempts were made".to_string()),
        })
    }

    /// Attempt a graceful shutdown via the active server connection, if available.
    pub fn shutdown(&mut self) -> DriverResult<()> {
        self.require_connection()?;
        let result = self.shutdown_server();
        self.reset();
        result
    }

    /// Inject a single key press (down + up) via the production RPC API.
    pub fn inject_key(&mut self, seq: &str) -> DriverResult<()> {
        let ident = canonicalize_ident(seq);
        let gate_ms = config::BINDING_GATES.default_ms;
        let mut targets = BTreeSet::new();
        targets.insert(ident.clone());

        self.wait_for_hud_keys(&targets, gate_ms)?;

        let deadline = Instant::now() + Duration::from_millis(gate_ms);
        loop {
            let baseline = self.latest_hud.as_ref().map(|snapshot| snapshot.event_id);
            match self.inject_key_event(&ident, InjectKind::Down, false) {
                Ok(()) => {
                    let hud_wait_ms = config::INPUT_DELAYS.retry_delay_ms.max(10);
                    let _ = self.wait_for_hud_progress_since(baseline, hud_wait_ms)?;
                    break;
                }
                Err(DriverError::ServerFailure { message })
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

        match self.inject_key_event(&ident, InjectKind::Up, false) {
            Ok(()) => Ok(()),
            Err(DriverError::ServerFailure { message })
                if message_contains_key_not_bound(&message) =>
            {
                Ok(())
            }
            Err(err) => Err(err),
        }
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

    /// Quick liveness probe against the backend via a lightweight RPC.
    #[cfg(test)]
    pub fn check_alive(&mut self) -> DriverResult<()> {
        self.call_depth().map(|_| ())
    }

    /// Retrieve the latest HUD snapshot observed on the server event stream.
    pub fn latest_hud(&self) -> DriverResult<Option<HudSnapshot>> {
        self.require_connection()?;
        Ok(self.latest_hud.clone())
    }

    /// Drain buffered server events for inspection.
    pub fn drain_events(&mut self) -> DriverResult<Vec<DriverEventRecord>> {
        self.require_connection()?;
        let mut observed = Vec::new();
        loop {
            let event = {
                let client = self.client.as_mut().ok_or(DriverError::NotInitialized)?;
                let conn = connection(client)?;
                conn.try_recv_event().map_err(|err| server_error(&err))?
            };
            let Some(event) = event else {
                break;
            };
            observed.push(self.record_event(event));
        }
        Ok(observed)
    }

    /// Connect if needed, then refresh and validate the server handshake.
    fn connect_and_refresh_handshake(
        &mut self,
        deadline: Instant,
        timeout_ms: u64,
    ) -> DriverResult<()> {
        if self.client.is_none() {
            self.connect_client()?;
        }
        self.refresh_handshake()?;
        self.wait_for_event_stream(deadline, timeout_ms)
    }

    /// Establish a connect-only MRPC client to the UI-owned server socket.
    fn connect_client(&mut self) -> DriverResult<()> {
        let socket_path = self.socket_path.clone();
        let client = self
            .runtime
            .block_on(async {
                Client::new_with_socket(socket_path.clone())
                    .with_connect_only()
                    .connect()
                    .await
            })
            .map_err(|err| DriverError::Connect {
                socket_path,
                message: err.to_string(),
            })?;
        self.client = Some(client);
        Ok(())
    }

    /// Refresh cached server status and validate readiness invariants.
    fn refresh_handshake(&mut self) -> DriverResult<()> {
        self.clear_cached_state();
        let status = {
            let runtime = &self.runtime;
            let client = self.client.as_mut().ok_or(DriverError::NotInitialized)?;
            let conn = connection(client)?;
            runtime
                .block_on(conn.get_server_status())
                .map_err(|err| server_error(&err))?
        };
        let handshake = ServerHandshake { status };
        ensure_clean_handshake(&handshake)?;
        self.handshake = Some(handshake);
        self.drain_events()?;
        Ok(())
    }

    /// Send the production shutdown RPC to the server.
    fn shutdown_server(&mut self) -> DriverResult<()> {
        let runtime = &self.runtime;
        let client = self.client.as_mut().ok_or(DriverError::NotInitialized)?;
        let conn = connection(client)?;
        runtime
            .block_on(conn.shutdown())
            .map_err(|err| server_error(&err))
    }

    /// Inject one key event through the production RPC API.
    fn inject_key_event(
        &mut self,
        ident: &str,
        kind: InjectKind,
        repeat: bool,
    ) -> DriverResult<()> {
        let runtime = &self.runtime;
        let client = self.client.as_mut().ok_or(DriverError::NotInitialized)?;
        let conn = connection(client)?;
        let result = match (kind, repeat) {
            (InjectKind::Down, true) => runtime.block_on(conn.inject_key_repeat(ident)),
            (InjectKind::Down, false) => runtime.block_on(conn.inject_key_down(ident)),
            (InjectKind::Up, _) => runtime.block_on(conn.inject_key_up(ident)),
        };
        result.map_err(|err| server_error(&err))?;
        self.drain_events()?;
        Ok(())
    }

    /// Fetch the binding identifiers currently reported by the server.
    fn call_bindings(&mut self) -> DriverResult<Vec<String>> {
        let runtime = &self.runtime;
        let client = self.client.as_mut().ok_or(DriverError::NotInitialized)?;
        let conn = connection(client)?;
        runtime
            .block_on(conn.get_bindings())
            .map_err(|err| server_error(&err))
    }

    #[cfg(test)]
    fn call_depth(&mut self) -> DriverResult<usize> {
        let runtime = &self.runtime;
        let client = self.client.as_mut().ok_or(DriverError::NotInitialized)?;
        let conn = connection(client)?;
        runtime
            .block_on(conn.get_depth())
            .map_err(|err| server_error(&err))
    }

    /// Ensure the driver has been initialized before inspecting cached state.
    fn require_connection(&self) -> DriverResult<()> {
        self.client
            .as_ref()
            .map(|_| ())
            .ok_or(DriverError::NotInitialized)
    }

    /// Clear all cached server-derived state after a reset or reconnect.
    fn clear_cached_state(&mut self) {
        self.event_buffer.clear();
        self.latest_hud = None;
        self.handshake = None;
        self.event_stream_ready = false;
    }

    /// Record one asynchronous server event into the local caches.
    fn record_event(&mut self, payload: MsgToUI) -> DriverEventRecord {
        self.event_stream_ready = true;
        let id = self.next_event_id;
        self.next_event_id = self.next_event_id.wrapping_add(1);
        let timestamp_ms = now_millis();

        if let MsgToUI::HudUpdate { hud, displays } = &payload {
            let idents: BTreeSet<String> = hud
                .rows
                .iter()
                .map(|row| canonicalize_ident(&row.chord.to_string()))
                .collect();
            self.latest_hud = Some(HudSnapshot {
                event_id: id,
                received_ms: timestamp_ms,
                hud: (**hud).clone(),
                displays: displays.clone(),
                idents,
            });
        }

        if self.event_buffer.len() >= Self::EVENT_BUFFER_CAPACITY {
            self.event_buffer.pop_front();
        }
        let record = DriverEventRecord {
            id,
            timestamp_ms,
            payload,
        };
        self.event_buffer.push_back(record.clone());
        record
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

    /// Wait for the next asynchronous server event before `deadline`.
    fn wait_for_server_event(&mut self, deadline: Instant) -> DriverResult<bool> {
        if !self.drain_events()?.is_empty() {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(false);
        };

        let event = {
            let runtime = &self.runtime;
            let client = self.client.as_mut().ok_or(DriverError::NotInitialized)?;
            let conn = connection(client)?;
            match runtime.block_on(async { timeout(remaining, conn.recv_event()).await }) {
                Ok(Ok(event)) => Some(event),
                Ok(Err(err)) => return Err(server_error(&err)),
                Err(_) => None,
            }
        };

        if let Some(event) = event {
            self.record_event(event);
            return Ok(true);
        }
        Ok(false)
    }

    /// Wait until the driver's notification stream has delivered at least one event.
    fn wait_for_event_stream(&mut self, deadline: Instant, timeout_ms: u64) -> DriverResult<()> {
        while !self.event_stream_ready {
            if !self.wait_for_server_event(deadline)? {
                return Err(DriverError::EventStreamTimeout {
                    socket_path: self.socket_path.clone(),
                    timeout_ms,
                });
            }
        }
        Ok(())
    }

    /// Wait until the HUD cache or RPC binding snapshot contains all requested identifiers.
    fn wait_for_hud_keys(&mut self, want: &BTreeSet<String>, timeout_ms: u64) -> DriverResult<()> {
        if want.is_empty() {
            return Ok(());
        }

        let start = Instant::now();
        let deadline = start + Duration::from_millis(timeout_ms);
        let mut rpc_snapshot = None;

        loop {
            self.drain_events()?;
            if self.hud_contains_all(want) {
                return Ok(());
            }

            match self.call_bindings() {
                Ok(bindings) => {
                    if bindings_contain_all(&bindings, want) {
                        let elapsed_ms = start.elapsed().as_millis() as u64;
                        let hud_view = self
                            .latest_hud
                            .as_ref()
                            .map(|snapshot| snapshot.idents.iter().cloned().collect::<Vec<_>>())
                            .unwrap_or_default();
                        let rpc_view = canonicalize_bindings(&bindings);
                        debug!(
                            elapsed_ms,
                            hud = ?hud_view,
                            rpc = ?rpc_view,
                            "wait_for_idents_rpc_match"
                        );
                        return Ok(());
                    }
                    rpc_snapshot = Some(bindings);
                }
                Err(err) => {
                    debug!(?err, "failed to fetch bindings snapshot while waiting");
                }
            }

            if Instant::now() >= deadline {
                break;
            }
            let _ = self.wait_for_server_event(deadline)?;
        }

        if self.hud_contains_all(want) {
            return Ok(());
        }

        let current = self
            .latest_hud
            .as_ref()
            .map(|snapshot| snapshot.idents.clone())
            .unwrap_or_default();
        let rpc_view = rpc_snapshot
            .as_ref()
            .map(|bindings| canonicalize_bindings(bindings));

        if self.hud_contains_all(want) {
            return Ok(());
        }

        let missing: Vec<String> = want.difference(&current).cloned().collect();
        let elapsed_ms = start.elapsed().as_millis() as u64;
        let hud_view = current.iter().cloned().collect::<Vec<_>>();
        debug!(
            elapsed_ms,
            hud = ?hud_view,
            rpc = ?rpc_view,
            missing = ?missing,
            "wait_for_idents_timeout"
        );

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
            self.drain_events()?;
            let current_id = self.latest_hud.as_ref().map(|snapshot| snapshot.event_id);
            let advanced =
                matches!((baseline, current_id), (_, Some(current)) if Some(current) != baseline);
            if advanced {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            let _ = self.wait_for_server_event(deadline)?;
        }
    }
}

/// Borrow the active typed server connection from a client.
fn connection(client: &mut Client) -> DriverResult<&mut Connection> {
    client.connection().map_err(|err| server_error(&err))
}

/// Convert a server crate error into a driver error.
fn server_error(err: &hotki_server::Error) -> DriverError {
    DriverError::ServerFailure {
        message: err.to_string(),
    }
}

/// Canonicalize raw binding identifiers returned by the server.
fn canonicalize_bindings(bindings: &[String]) -> Vec<String> {
    bindings
        .iter()
        .map(|raw| canonicalize_ident(raw.trim_matches('"')))
        .collect()
}

/// Return whether a raw server binding snapshot contains every wanted identifier.
fn bindings_contain_all(bindings: &[String], want: &BTreeSet<String>) -> bool {
    let rpc_idents = canonicalize_bindings(bindings)
        .into_iter()
        .collect::<BTreeSet<_>>();
    want.is_subset(&rpc_idents)
}

/// Return the current wall-clock timestamp in milliseconds since the Unix epoch.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_event_returns_new_record_when_buffer_is_full() -> DriverResult<()> {
        let mut driver = ServerDriver::new("unused.sock")?;
        for tick in 0..ServerDriver::EVENT_BUFFER_CAPACITY {
            driver.record_event(MsgToUI::Heartbeat(tick as u64));
        }

        let record = driver.record_event(MsgToUI::Heartbeat(999));

        assert_eq!(record.id, ServerDriver::EVENT_BUFFER_CAPACITY as u64);
        assert_eq!(record.payload, MsgToUI::Heartbeat(999));
        assert_eq!(
            driver.event_buffer.len(),
            ServerDriver::EVENT_BUFFER_CAPACITY
        );
        assert_eq!(
            driver.event_buffer.back().map(|record| &record.payload),
            Some(&MsgToUI::Heartbeat(999))
        );
        Ok(())
    }

    #[test]
    fn record_event_marks_event_stream_ready() -> DriverResult<()> {
        let mut driver = ServerDriver::new("unused.sock")?;

        driver.record_event(MsgToUI::Heartbeat(1));

        assert!(driver.event_stream_ready);
        driver.clear_cached_state();
        assert!(!driver.event_stream_ready);
        Ok(())
    }

    #[test]
    fn bindings_contain_all_canonicalizes_server_strings() {
        let bindings = vec!["\"shift+cmd+0\"".to_string(), "escape".to_string()];
        let want = BTreeSet::from([canonicalize_ident("shift+cmd+0")]);

        assert!(bindings_contain_all(&bindings, &want));
    }
}
