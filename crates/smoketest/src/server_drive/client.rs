use std::{
    collections::{BTreeSet, VecDeque},
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    thread,
    time::{Duration, Instant},
};

use hotki_protocol::rpc::InjectKind;
use hotki_server::smoketest_bridge::{
    BridgeCommand, BridgeCommandId, BridgeEvent, BridgeReply, BridgeRequest, BridgeResponse,
    now_millis,
};
use tracing::debug;

use super::{
    BridgeEventRecord, BridgeHandshake, DriverError, DriverResult, HudSnapshot, canonicalize_ident,
    log_bindings_enabled,
    types::{ensure_clean_handshake, message_contains_key_not_bound},
};
use crate::config;

/// Blocking Unix-stream client that forwards commands to the UI bridge.
pub(super) struct BridgeClient {
    /// Reader half of the bridge socket.
    reader: BufReader<UnixStream>,
    /// Writer half of the bridge socket.
    writer: UnixStream,
    /// Path to the bridge socket, used for diagnostics.
    socket_path: String,
    /// Next command identifier to allocate.
    next_command_id: BridgeCommandId,
    /// Maximum time to wait for an acknowledgement.
    ack_timeout: Duration,
    /// Circular buffer of recent bridge events.
    event_buffer: VecDeque<BridgeEventRecord>,
    /// Latest HUD snapshot emitted by the bridge.
    latest_hud: Option<HudSnapshot>,
    /// Most recent handshake data captured during initialization.
    pub(super) handshake: Option<BridgeHandshake>,
}

impl BridgeClient {
    /// Maximum number of reconnection attempts per bridge call.
    const MAX_RECONNECT_ATTEMPTS: u32 = 3;
    /// Maximum number of bridge events retained in memory.
    const EVENT_BUFFER_CAPACITY: usize = 128;

    /// Establish a new bridge client connection to the given socket path.
    pub(super) fn connect(path: &str) -> DriverResult<Self> {
        let writer = UnixStream::connect(path).map_err(|source| DriverError::Connect {
            socket_path: path.to_string(),
            source,
        })?;
        writer.set_nonblocking(false).ok();
        let reader_stream = writer
            .try_clone()
            .map_err(|source| DriverError::Io { source })?;
        Ok(Self {
            reader: BufReader::new(reader_stream),
            writer,
            socket_path: path.to_string(),
            next_command_id: 0,
            ack_timeout: Duration::from_millis(config::BRIDGE.ack_timeout_ms),
            event_buffer: VecDeque::new(),
            latest_hud: None,
            handshake: None,
        })
    }

    /// Establish a connection and perform an initial handshake with invariant checks.
    pub(super) fn connect_with_handshake(path: &str) -> DriverResult<Self> {
        let mut client = Self::connect(path)?;
        client.refresh_handshake()?;
        Ok(client)
    }

    /// Perform the bridge handshake and cache the resulting snapshot.
    fn handshake(&mut self) -> DriverResult<BridgeHandshake> {
        match self.call(&BridgeRequest::Ping)? {
            BridgeResponse::Handshake {
                idle_timer,
                notifications,
            } => {
                let payload = BridgeHandshake {
                    idle_timer,
                    notifications,
                };
                ensure_clean_handshake(&payload)?;
                self.handshake = Some(payload.clone());
                Ok(payload)
            }
            BridgeResponse::Err { message } => Err(DriverError::BridgeFailure { message }),
            other => Err(DriverError::BridgeFailure {
                message: format!("unexpected handshake response: {:?}", other),
            }),
        }
    }

    /// Clear cached state and perform a fresh handshake.
    fn refresh_handshake(&mut self) -> DriverResult<BridgeHandshake> {
        self.clear_cached_state();
        self.handshake()
    }

    /// Send a bridge request and wait for its response.
    pub(super) fn call(&mut self, req: &BridgeRequest) -> DriverResult<BridgeResponse> {
        let request = req.clone();
        let mut attempt = 0;
        loop {
            let command_id = self.next_command_id;
            let command = BridgeCommand {
                command_id,
                issued_at_ms: now_millis(),
                request: request.clone(),
            };

            match self.send_command(&command) {
                Ok(()) => {}
                Err(DriverError::Io { source })
                    if connection_lost(&source) && attempt < Self::MAX_RECONNECT_ATTEMPTS =>
                {
                    attempt += 1;
                    self.reconnect_with_backoff(attempt)?;
                    continue;
                }
                Err(err @ DriverError::Io { .. }) => return Err(err),
                Err(err) => return Err(err),
            }

            let (acked, result) = self.await_ack_and_response(command_id);
            match result {
                Ok(resp) => {
                    if acked {
                        self.bump_command_id();
                    }
                    return Ok(resp);
                }
                Err(err @ DriverError::BridgeFailure { .. }) if acked => {
                    self.bump_command_id();
                    return Err(err);
                }
                Err(DriverError::Io { source })
                    if connection_lost(&source) && attempt < Self::MAX_RECONNECT_ATTEMPTS =>
                {
                    attempt += 1;
                    self.reconnect_with_backoff(attempt)?;
                    continue;
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Advance to the next command identifier.
    fn bump_command_id(&mut self) {
        self.next_command_id = self.next_command_id.wrapping_add(1);
    }

    /// Serialize and dispatch a command to the bridge socket.
    fn send_command(&mut self, command: &BridgeCommand) -> DriverResult<()> {
        let encoded = serde_json::to_string(command).map_err(|err| DriverError::BridgeFailure {
            message: err.to_string(),
        })?;
        self.writer
            .write_all(encoded.as_bytes())
            .map_err(|source| DriverError::Io { source })?;
        self.writer
            .write_all(b"\n")
            .map_err(|source| DriverError::Io { source })?;
        self.writer
            .flush()
            .map_err(|source| DriverError::Io { source })
    }

    /// Wait for the bridge to acknowledge the command and provide the final response.
    /// Returns whether the acknowledgement was accepted along with the outcome.
    fn await_ack_and_response(
        &mut self,
        command_id: BridgeCommandId,
    ) -> (bool, DriverResult<BridgeResponse>) {
        if let Err(source) = self
            .reader
            .get_ref()
            .set_read_timeout(Some(self.ack_timeout))
        {
            return (false, Err(DriverError::Io { source }));
        }
        loop {
            let ack_result = self.read_reply();
            match ack_result {
                Ok(reply) => {
                    if let BridgeResponse::Event { .. } = &reply.response {
                        self.record_event(reply);
                        continue;
                    }
                    let outcome = self.validate_ack(command_id, &reply);
                    if let Err(err) = self.reader.get_ref().set_read_timeout(None) {
                        debug!(?err, "failed to clear bridge read timeout");
                    }
                    match outcome {
                        Ok(()) => return (true, self.await_final_response(command_id)),
                        Err(err) => return (false, Err(err)),
                    }
                }
                Err(DriverError::Io { source })
                    if source.kind() == io::ErrorKind::WouldBlock
                        || source.kind() == io::ErrorKind::TimedOut =>
                {
                    if let Err(err) = self.reader.get_ref().set_read_timeout(None) {
                        debug!(?err, "failed to clear bridge read timeout");
                    }
                    return (
                        false,
                        Err(DriverError::AckTimeout {
                            command_id,
                            timeout_ms: self.ack_timeout.as_millis() as u64,
                        }),
                    );
                }
                Err(err) => {
                    if let Err(clear_err) = self.reader.get_ref().set_read_timeout(None) {
                        debug!(?clear_err, "failed to clear bridge read timeout");
                    }
                    return (false, Err(err));
                }
            }
        }
    }

    /// Validate that the acknowledgement matches the expected command id.
    fn validate_ack(&self, command_id: BridgeCommandId, ack: &BridgeReply) -> DriverResult<()> {
        if ack.command_id != command_id {
            return Err(DriverError::SequenceMismatch {
                expected: command_id,
                got: ack.command_id,
            });
        }
        match &ack.response {
            BridgeResponse::Ack { queued } => {
                debug!(command_id, queued, "bridge_ack");
                Ok(())
            }
            BridgeResponse::Err { message } => Err(DriverError::BridgeFailure {
                message: message.clone(),
            }),
            _ => Err(DriverError::AckMissing { command_id }),
        }
    }

    /// Read the final response frame for the supplied command id.
    fn await_final_response(
        &mut self,
        command_id: BridgeCommandId,
    ) -> DriverResult<BridgeResponse> {
        loop {
            let reply = self.read_reply()?;
            if let BridgeResponse::Event { .. } = &reply.response {
                self.record_event(reply);
                continue;
            }
            if reply.command_id != command_id {
                return Err(DriverError::SequenceMismatch {
                    expected: command_id,
                    got: reply.command_id,
                });
            }
            return match reply.response {
                BridgeResponse::Ack { .. } => Err(DriverError::AckMissing { command_id }),
                BridgeResponse::Err { message } => Err(DriverError::BridgeFailure { message }),
                other => Ok(other),
            };
        }
    }

    /// Read and deserialize the next reply frame from the bridge.
    fn read_reply(&mut self) -> DriverResult<BridgeReply> {
        let mut line = String::new();
        let bytes = self
            .reader
            .read_line(&mut line)
            .map_err(|source| DriverError::Io { source })?;
        if bytes == 0 {
            return Err(DriverError::BridgeFailure {
                message: format!("bridge socket '{}' closed", self.socket_path),
            });
        }
        let trimmed = line.trim_end_matches(['\n', '\r']);
        serde_json::from_str(trimmed).map_err(|err| DriverError::BridgeFailure {
            message: err.to_string(),
        })
    }

    /// Record an asynchronous event emitted by the bridge.
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

    /// Reset cached snapshots and buffered events.
    fn clear_cached_state(&mut self) {
        self.event_buffer.clear();
        self.latest_hud = None;
        self.handshake = None;
    }

    /// Drain the buffered bridge events.
    pub(super) fn drain_events(&mut self) -> Vec<BridgeEventRecord> {
        self.event_buffer.drain(..).collect()
    }

    /// Return the number of events observed so far.
    pub(super) fn event_buffer_len(&self) -> usize {
        self.event_buffer.len()
    }

    /// Ensure no additional events arrived after a baseline index.
    pub(super) fn assert_no_new_events_since(&self, baseline: usize) -> DriverResult<()> {
        if let Some(event) = self.event_buffer.get(baseline) {
            return Err(DriverError::PostShutdownMessage {
                message: format!("bridge event observed after shutdown: {:?}", event.payload),
            });
        }
        Ok(())
    }

    /// Access the latest HUD snapshot observed on the bridge.
    pub(super) fn latest_hud(&self) -> Option<HudSnapshot> {
        self.latest_hud.clone()
    }

    /// Return true when the current HUD snapshot contains all `want` identifiers.
    fn hud_contains_all(&self, want: &BTreeSet<String>) -> bool {
        if want.is_empty() {
            return true;
        }
        self.latest_hud
            .as_ref()
            .map(|snapshot| want.is_subset(&snapshot.idents))
            .unwrap_or(false)
    }

    /// Wait for the next bridge event until `deadline`, recording it when observed.
    /// Returns `true` if an event arrived before the deadline, or `false` on timeout.
    fn wait_for_bridge_event(&mut self, deadline: Instant) -> DriverResult<bool> {
        if Instant::now() >= deadline {
            return Ok(false);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        if let Err(source) = self.reader.get_ref().set_read_timeout(Some(remaining)) {
            return Err(DriverError::Io { source });
        }
        let outcome = self.read_reply();
        if let Err(err) = self.reader.get_ref().set_read_timeout(None) {
            debug!(?err, "failed to clear bridge read timeout");
        }
        match outcome {
            Ok(reply) => match reply.response {
                BridgeResponse::Event { .. } => {
                    self.record_event(reply);
                    Ok(true)
                }
                other => Err(DriverError::BridgeFailure {
                    message: format!(
                        "unexpected bridge reply while waiting for events: {:?}",
                        other
                    ),
                }),
            },
            Err(DriverError::Io { source })
                if source.kind() == io::ErrorKind::WouldBlock
                    || source.kind() == io::ErrorKind::TimedOut =>
            {
                Ok(false)
            }
            Err(err) => Err(err),
        }
    }

    /// Wait until the HUD snapshot contains all desired identifiers or the timeout elapses.
    pub(super) fn wait_for_hud_keys(
        &mut self,
        want: &BTreeSet<String>,
        timeout_ms: u64,
    ) -> DriverResult<()> {
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

    /// Wait for a HUD event newer than `baseline` within `timeout_ms` milliseconds.
    /// Returns `true` if a new HUD event arrived, or `false` if the wait timed out.
    fn wait_for_hud_progress_since(
        &mut self,
        baseline: Option<BridgeCommandId>,
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

    /// Inject a key chord by issuing down/up events once the HUD reports readiness.
    pub(super) fn inject_key(&mut self, seq: &str) -> DriverResult<()> {
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

    /// Attempt to re-establish the bridge connection with exponential backoff.
    fn reconnect_with_backoff(&mut self, attempt: u32) -> DriverResult<()> {
        let mut last_err: Option<io::Error> = None;
        let mut backoff_ms = config::RETRY.fast_delay_ms.saturating_mul(attempt as u64);
        let max_steps = 3;
        for _ in 0..max_steps {
            thread::sleep(Duration::from_millis(backoff_ms.max(1)));
            match UnixStream::connect(&self.socket_path) {
                Ok(writer) => {
                    writer.set_nonblocking(false).ok();
                    let reader_stream = writer
                        .try_clone()
                        .map_err(|source| DriverError::Io { source })?;
                    self.reader = BufReader::new(reader_stream);
                    self.writer = writer;
                    self.next_command_id = 0;
                    self.refresh_handshake()?;
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
            socket_path: self.socket_path.clone(),
            source,
        })
    }

    /// Send a bridge request that is expected to return `BridgeResponse::Ok`.
    pub(super) fn call_ok(&mut self, req: &BridgeRequest) -> DriverResult<()> {
        self.call(req)?
            .into_result()
            .map_err(|message| DriverError::BridgeFailure { message })
    }

    /// Retrieve the current bindings list via the bridge.
    fn call_bindings(&mut self) -> DriverResult<Vec<String>> {
        self.call(&BridgeRequest::GetBindings)?
            .into_bindings()
            .map_err(|message| DriverError::BridgeFailure { message })
    }

    /// Retrieve the current depth value via the bridge.
    #[cfg(test)]
    pub(super) fn call_depth(&mut self) -> DriverResult<usize> {
        self.call(&BridgeRequest::GetDepth)?
            .into_depth()
            .map_err(|message| DriverError::BridgeFailure { message })
    }
}

/// Return true when the provided I/O error indicates that the bridge connection dropped.
fn connection_lost(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
    )
}
