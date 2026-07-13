//! Stateful synchronous driver for the production server connection.

use std::{collections::BTreeSet, mem, thread};

use hotki_protocol::{MsgToUI, rpc::InjectKind};
use hotki_server::Client;
use tokio::runtime::{Builder, Runtime};
use tracing::debug;

use super::{
    DriverEventRecord, DriverResult, HudSnapshot, ServerHandshake,
    deadline::Deadline,
    event_cache::EventCache,
    rpc::ServerRpc,
    types::{
        DriverError, DriverEventId, canonicalize_ident, describe_init_error,
        ensure_clean_handshake, is_key_not_bound,
    },
};
use crate::config;

/// Server driver backed directly by the production MRPC client and event stream.
pub struct ServerDriver {
    /// Current connection/readiness state. Dropped before the runtime.
    state: ConnectionState,
    /// Runtime used to drive async MRPC calls from the synchronous smoketest harness.
    runtime: Runtime,
    /// Server socket path used to connect to the UI-owned backend.
    socket_path: String,
    /// Server event cache and HUD index.
    events: EventCache,
}

/// Driver connection and readiness state.
enum ConnectionState {
    /// No active server connection.
    Disconnected,
    /// Connected to MRPC but handshake has not been validated.
    Connected {
        /// Active MRPC client.
        client: Client,
    },
    /// Connected and handshake-validated, waiting for the first server event.
    Handshake {
        /// Active MRPC client.
        client: Client,
        /// Validated server handshake.
        handshake: ServerHandshake,
    },
    /// Connected, handshake-validated, and event stream observed.
    EventStreamReady {
        /// Active MRPC client.
        client: Client,
        /// Validated server handshake.
        handshake: ServerHandshake,
    },
}

impl ConnectionState {
    /// Return true when the state owns an active client connection.
    fn has_client(&self) -> bool {
        !matches!(self, Self::Disconnected)
    }

    /// Return true when the event stream has produced at least one event.
    fn is_event_stream_ready(&self) -> bool {
        matches!(self, Self::EventStreamReady { .. })
    }

    /// Borrow the active client.
    fn client_mut(&mut self) -> DriverResult<&mut Client> {
        match self {
            Self::Connected { client }
            | Self::Handshake { client, .. }
            | Self::EventStreamReady { client, .. } => Ok(client),
            Self::Disconnected => Err(DriverError::NotInitialized),
        }
    }

    /// Replace the current state with a connected client.
    fn set_connected(&mut self, client: Client) {
        *self = Self::Connected { client };
    }

    /// Promote a connected state to handshake-validated.
    fn set_handshake(&mut self, handshake: ServerHandshake) -> DriverResult<()> {
        let client = match mem::replace(self, Self::Disconnected) {
            Self::Connected { client }
            | Self::Handshake { client, .. }
            | Self::EventStreamReady { client, .. } => client,
            Self::Disconnected => return Err(DriverError::NotInitialized),
        };
        *self = Self::Handshake { client, handshake };
        Ok(())
    }

    /// Promote a handshake-validated state after the first event arrives.
    fn mark_event_stream_ready(&mut self) -> DriverResult<()> {
        let (client, handshake) = match mem::replace(self, Self::Disconnected) {
            Self::Handshake { client, handshake }
            | Self::EventStreamReady { client, handshake } => (client, handshake),
            Self::Connected { client } => {
                *self = Self::Connected { client };
                return Err(DriverError::NotInitialized);
            }
            Self::Disconnected => return Err(DriverError::NotInitialized),
        };
        *self = Self::EventStreamReady { client, handshake };
        Ok(())
    }
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
            state: ConnectionState::Disconnected,
            runtime,
            socket_path: socket_path.into(),
            events: EventCache::new(Self::EVENT_BUFFER_CAPACITY),
        })
    }

    /// Drop the current server connection so the next operation reconnects from scratch.
    pub fn reset(&mut self) {
        self.state = ConnectionState::Disconnected;
        self.events.clear();
    }

    /// Ensure the server connection is initialized within `timeout_ms`.
    pub fn ensure_ready(&mut self, timeout_ms: u64) -> DriverResult<()> {
        if self.state.is_event_stream_ready() {
            return Ok(());
        }

        let deadline = Deadline::from_timeout(timeout_ms);
        let mut last_error: Option<String> = None;

        while !deadline.expired() {
            match self.connect_and_refresh_handshake(deadline) {
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
            let Some(remaining) = deadline.remaining() else {
                break;
            };
            thread::sleep(remaining.min(config::ms(config::RETRY.fast_delay_ms)));
        }

        Err(DriverError::InitTimeout {
            socket_path: self.socket_path.clone(),
            timeout_ms: deadline.timeout_ms(),
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
    pub fn inject_key(&mut self, seq: &str, timeout_ms: u64) -> DriverResult<()> {
        let ident = canonicalize_ident(seq);
        let deadline = Deadline::from_timeout(timeout_ms);
        let mut targets = BTreeSet::new();
        targets.insert(ident.clone());

        let remaining_ms = deadline
            .remaining_ms()
            .ok_or_else(|| DriverError::BindingTimeout {
                ident: ident.clone(),
                timeout_ms: deadline.timeout_ms(),
            })?;
        self.wait_for_hud_keys(&targets, remaining_ms)?;

        loop {
            let baseline = self.events.latest_hud_event_id();
            match self.inject_key_event(&ident, InjectKind::Down, false) {
                Ok(()) => {
                    self.drain_events()?;
                    let current = self.events.latest_hud_event_id();
                    debug!(?baseline, ?current, "injected_key_hud_progress");
                    break;
                }
                Err(err) if is_key_not_bound(&err) => {
                    let Some(remaining_ms) = deadline.remaining_ms() else {
                        return Err(DriverError::BindingTimeout {
                            ident,
                            timeout_ms: deadline.timeout_ms(),
                        });
                    };
                    self.wait_for_hud_keys(&targets, remaining_ms)?;
                }
                Err(err) => return Err(err),
            }
        }

        match self.inject_key_event(&ident, InjectKind::Up, false) {
            Ok(()) => Ok(()),
            Err(err) if is_key_not_bound(&err) => Ok(()),
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
        Ok(self.events.latest_hud())
    }

    /// Drain buffered server events for inspection.
    pub fn drain_events(&mut self) -> DriverResult<Vec<DriverEventRecord>> {
        self.require_connection()?;
        let mut observed = Vec::new();
        loop {
            let event = {
                let mut rpc = self.rpc()?;
                rpc.try_recv_event()?
            };
            let Some(event) = event else {
                break;
            };
            observed.push(self.record_event(event)?);
        }
        Ok(observed)
    }

    /// Return a cursor for matching subsequently observed server events.
    pub fn event_cursor(&self) -> DriverResult<DriverEventId> {
        self.require_connection()?;
        Ok(self.events.cursor())
    }

    /// Wait for a server UI event at or after `cursor` matching `predicate`.
    pub fn wait_for_message_since<F>(
        &mut self,
        cursor: DriverEventId,
        timeout_ms: u64,
        mut predicate: F,
    ) -> DriverResult<DriverEventRecord>
    where
        F: FnMut(&MsgToUI) -> bool,
    {
        let deadline = Deadline::from_timeout(timeout_ms);
        loop {
            if let Some(event) = self.events.find_since(cursor, &mut predicate) {
                return Ok(event);
            }

            for event in self.drain_events()? {
                if event.id >= cursor && predicate(&event.payload) {
                    return Ok(event);
                }
            }

            if deadline.expired() {
                return Err(DriverError::EventStreamTimeout {
                    socket_path: self.socket_path.clone(),
                    timeout_ms,
                });
            }

            let Some(remaining) = deadline.remaining() else {
                return Err(DriverError::EventStreamTimeout {
                    socket_path: self.socket_path.clone(),
                    timeout_ms,
                });
            };
            let event = {
                let mut rpc = self.rpc()?;
                rpc.recv_event_timeout(remaining)?
            };
            if let Some(event) = event {
                let record = self.record_event(event)?;
                if predicate(&record.payload) {
                    return Ok(record);
                }
            }
        }
    }

    /// Connect if needed, then refresh and validate the server handshake.
    fn connect_and_refresh_handshake(&mut self, deadline: Deadline) -> DriverResult<()> {
        if !self.state.has_client() {
            self.connect_client()?;
        }
        self.refresh_handshake()?;
        self.wait_for_event_stream(deadline)
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
        self.state.set_connected(client);
        Ok(())
    }

    /// Refresh cached server status and validate readiness invariants.
    fn refresh_handshake(&mut self) -> DriverResult<()> {
        self.events.clear();
        let status = {
            let mut rpc = self.rpc()?;
            rpc.server_status()?
        };
        let handshake = ServerHandshake { status };
        ensure_clean_handshake(&handshake)?;
        self.state.set_handshake(handshake)?;
        self.drain_events()?;
        Ok(())
    }

    /// Send the production shutdown RPC to the server.
    fn shutdown_server(&mut self) -> DriverResult<()> {
        self.rpc()?.shutdown()
    }

    /// Inject one key event through the production RPC API.
    fn inject_key_event(
        &mut self,
        ident: &str,
        kind: InjectKind,
        repeat: bool,
    ) -> DriverResult<()> {
        self.rpc()?.inject_key(ident, kind, repeat)?;
        self.drain_events()?;
        Ok(())
    }

    /// Fetch the binding identifiers currently reported by the server.
    fn call_bindings(&mut self) -> DriverResult<Vec<String>> {
        self.rpc()?.bindings()
    }

    #[cfg(test)]
    fn call_depth(&mut self) -> DriverResult<usize> {
        self.rpc()?.depth()
    }

    /// Ensure the driver has been initialized before inspecting cached state.
    fn require_connection(&self) -> DriverResult<()> {
        self.state
            .has_client()
            .then_some(())
            .ok_or(DriverError::NotInitialized)
    }

    /// Borrow the production RPC facade for the active connection.
    fn rpc(&mut self) -> DriverResult<ServerRpc<'_>> {
        ServerRpc::from_client(&self.runtime, self.state.client_mut()?)
    }

    /// Record one asynchronous server event into the local caches.
    fn record_event(
        &mut self,
        payload: hotki_protocol::MsgToUI,
    ) -> DriverResult<DriverEventRecord> {
        let record = self.events.record(payload);
        self.state.mark_event_stream_ready()?;
        Ok(record)
    }

    /// Check whether the cached HUD snapshot contains every requested identifier.
    fn hud_contains_all(&self, want: &BTreeSet<String>) -> bool {
        self.events.hud_contains_all(want)
    }

    /// Wait for the next asynchronous server event before `deadline`.
    fn wait_for_server_event(&mut self, deadline: Deadline) -> DriverResult<bool> {
        if !self.drain_events()?.is_empty() {
            return Ok(true);
        }
        if deadline.expired() {
            return Ok(false);
        }
        let Some(remaining) = deadline.remaining() else {
            return Ok(false);
        };

        let event = {
            let mut rpc = self.rpc()?;
            rpc.recv_event_timeout(remaining)?
        };

        if let Some(event) = event {
            self.record_event(event)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Wait until the driver's notification stream has delivered at least one event.
    fn wait_for_event_stream(&mut self, deadline: Deadline) -> DriverResult<()> {
        while !self.state.is_event_stream_ready() {
            if !self.wait_for_server_event(deadline)? {
                return Err(DriverError::EventStreamTimeout {
                    socket_path: self.socket_path.clone(),
                    timeout_ms: deadline.timeout_ms(),
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

        let deadline = Deadline::from_timeout(timeout_ms);
        let mut rpc_snapshot = None;

        loop {
            self.drain_events()?;
            if self.hud_contains_all(want) {
                return Ok(());
            }

            match self.call_bindings() {
                Ok(bindings) => {
                    if bindings_contain_all(&bindings, want) {
                        let elapsed_ms = deadline.elapsed_ms();
                        let hud_view = self
                            .events
                            .latest_hud_idents()
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>();
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

            if deadline.expired() {
                break;
            }
            let _ = self.wait_for_server_event(deadline)?;
        }

        if self.hud_contains_all(want) {
            return Ok(());
        }

        let current = self.events.latest_hud_idents();
        let rpc_view = rpc_snapshot
            .as_ref()
            .map(|bindings| canonicalize_bindings(bindings));

        let missing: Vec<String> = want.difference(&current).cloned().collect();
        let elapsed_ms = deadline.elapsed_ms();
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

#[cfg(test)]
mod tests {
    use hotki_protocol::rpc::ServerStatusLite;

    use super::*;

    fn handshake() -> ServerHandshake {
        ServerHandshake {
            status: ServerStatusLite {
                idle_timeout_secs: 5,
                idle_timer_armed: false,
                idle_deadline_ms: None,
                clients_connected: 2,
            },
        }
    }

    #[test]
    fn bindings_contain_all_canonicalizes_server_strings() {
        let bindings = vec!["\"shift+cmd+0\"".to_string(), "escape".to_string()];
        let want = BTreeSet::from([canonicalize_ident("shift+cmd+0")]);

        assert!(bindings_contain_all(&bindings, &want));
    }

    #[test]
    fn rpc_snapshot_can_satisfy_wait_when_hud_cache_is_empty() {
        let cache = EventCache::new(ServerDriver::EVENT_BUFFER_CAPACITY);
        let want = BTreeSet::from([canonicalize_ident("shift+cmd+0")]);
        let bindings = vec!["\"shift+cmd+0\"".to_string()];

        assert!(!cache.hud_contains_all(&want));
        assert!(bindings_contain_all(&bindings, &want));
    }

    #[test]
    fn connection_state_tracks_readiness_transitions() -> DriverResult<()> {
        let mut state = ConnectionState::Disconnected;
        assert!(!state.has_client());
        assert!(!state.is_event_stream_ready());

        state.set_connected(Client::new().with_connect_only());
        assert!(state.has_client());
        assert!(!state.is_event_stream_ready());

        state.set_handshake(handshake())?;
        assert!(state.has_client());
        assert!(!state.is_event_stream_ready());

        state.mark_event_stream_ready()?;
        assert!(state.has_client());
        assert!(state.is_event_stream_ready());
        Ok(())
    }
}
