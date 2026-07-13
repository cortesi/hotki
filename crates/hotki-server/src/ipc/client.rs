//! MRPC connection implementation for the hotkey server

use std::{
    result::Result as StdResult,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use hotki_protocol::{
    MsgToUI,
    rpc::{
        HotkeyMethod, HotkeyNotification, InjectKeyReq, InjectKind, ServerStatusLite,
        WorldSnapshotLite,
    },
};
use mrpc::{Client as MrpcClient, Connection as MrpcConnection, RpcError, RpcSender, Value};
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use tokio::sync::{
    Notify,
    mpsc::{self, Receiver, Sender, error::TryRecvError},
};
use tracing::{debug, error, info, trace};

use crate::{Error, Result, error::RpcErrorCode, ipc::value};

const CLIENT_EVENT_CAPACITY: usize = 256;

/// Cumulative pressure statistics for one client event delivery lane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeliveryStats {
    /// Log records dropped because the bounded ordered lane was full.
    pub dropped_logs: u64,
    /// Snapshot messages replaced by a newer snapshot before delivery.
    pub coalesced_snapshots: u64,
}

#[derive(Default)]
struct DeliveryCounters {
    dropped_logs: AtomicU64,
    coalesced_snapshots: AtomicU64,
}

impl DeliveryCounters {
    fn snapshot(&self) -> DeliveryStats {
        DeliveryStats {
            dropped_logs: self.dropped_logs.load(Ordering::Relaxed),
            coalesced_snapshots: self.coalesced_snapshots.load(Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
struct PendingSnapshots {
    hud: Option<DeliveryEntry>,
    selector: Option<DeliveryEntry>,
    heartbeat: Option<DeliveryEntry>,
    world: Option<DeliveryEntry>,
}

impl PendingSnapshots {
    fn slot(&mut self, entry: &DeliveryEntry) -> Option<&mut Option<DeliveryEntry>> {
        match &entry.message {
            MsgToUI::HudUpdate { .. } => Some(&mut self.hud),
            MsgToUI::SelectorUpdate(_) | MsgToUI::SelectorHide => Some(&mut self.selector),
            MsgToUI::Heartbeat(_) => Some(&mut self.heartbeat),
            MsgToUI::World(_) => Some(&mut self.world),
            _ => None,
        }
    }

    fn next_sequence(&self) -> Option<u64> {
        [
            self.hud.as_ref(),
            self.selector.as_ref(),
            self.heartbeat.as_ref(),
            self.world.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(|entry| entry.sequence)
        .min()
    }

    fn take(&mut self, sequence: u64) -> Option<DeliveryEntry> {
        for slot in [
            &mut self.hud,
            &mut self.selector,
            &mut self.heartbeat,
            &mut self.world,
        ] {
            if slot
                .as_ref()
                .is_some_and(|entry| entry.sequence == sequence)
            {
                return slot.take();
            }
        }
        None
    }
}

struct DeliveryEntry {
    sequence: u64,
    message: MsgToUI,
}

#[derive(Clone)]
struct EventDeliveryTx {
    ordered: Sender<DeliveryEntry>,
    snapshots: Arc<Mutex<PendingSnapshots>>,
    snapshot_ready: Arc<Notify>,
    counters: Arc<DeliveryCounters>,
    next_sequence: Arc<AtomicU64>,
}

enum DeliveryOutcome {
    Queued,
    Coalesced,
    DroppedLogFull,
    Closed,
}

impl EventDeliveryTx {
    async fn send(&self, message: MsgToUI) -> DeliveryOutcome {
        let snapshot = matches!(
            &message,
            MsgToUI::HudUpdate { .. }
                | MsgToUI::SelectorUpdate(_)
                | MsgToUI::SelectorHide
                | MsgToUI::Heartbeat(_)
                | MsgToUI::World(_)
        );
        if snapshot {
            let entry = DeliveryEntry {
                sequence: self.next_sequence.fetch_add(1, Ordering::Relaxed),
                message,
            };
            let mut snapshots = self.snapshots.lock();
            let slot = snapshots
                .slot(&entry)
                .expect("snapshot message has a delivery slot");
            let coalesced = slot.replace(entry).is_some();
            if coalesced {
                self.counters
                    .coalesced_snapshots
                    .fetch_add(1, Ordering::Relaxed);
            }
            drop(snapshots);
            self.snapshot_ready.notify_one();
            return if coalesced {
                DeliveryOutcome::Coalesced
            } else {
                DeliveryOutcome::Queued
            };
        }

        if matches!(&message, MsgToUI::Log { .. }) {
            return match self.ordered.try_reserve() {
                Ok(permit) => {
                    permit.send(DeliveryEntry {
                        sequence: self.next_sequence.fetch_add(1, Ordering::Relaxed),
                        message,
                    });
                    DeliveryOutcome::Queued
                }
                Err(mpsc::error::TrySendError::Full(())) => {
                    self.counters.dropped_logs.fetch_add(1, Ordering::Relaxed);
                    DeliveryOutcome::DroppedLogFull
                }
                Err(mpsc::error::TrySendError::Closed(())) => DeliveryOutcome::Closed,
            };
        }

        match self.ordered.reserve().await {
            Ok(permit) => {
                permit.send(DeliveryEntry {
                    sequence: self.next_sequence.fetch_add(1, Ordering::Relaxed),
                    message,
                });
                DeliveryOutcome::Queued
            }
            Err(_) => DeliveryOutcome::Closed,
        }
    }
}

struct EventDeliveryRx {
    ordered: Receiver<DeliveryEntry>,
    snapshots: Arc<Mutex<PendingSnapshots>>,
    snapshot_ready: Arc<Notify>,
    counters: Arc<DeliveryCounters>,
    reported_dropped_logs: u64,
    buffered_ordered: Option<DeliveryEntry>,
    ordered_closed: bool,
}

impl EventDeliveryRx {
    fn dropped_log_summary(&mut self) -> Option<MsgToUI> {
        let dropped = self.counters.dropped_logs.load(Ordering::Relaxed);
        let newly_dropped = dropped.saturating_sub(self.reported_dropped_logs);
        if newly_dropped == 0 {
            return None;
        }
        self.reported_dropped_logs = dropped;
        Some(MsgToUI::Log {
            level: "warn".to_string(),
            target: "hotki_server::delivery".to_string(),
            message: format!("dropped {newly_dropped} server log messages under queue pressure"),
        })
    }

    async fn recv(&mut self) -> Option<MsgToUI> {
        loop {
            match self.try_recv() {
                Ok(Some(message)) => return Some(message),
                Err(TryRecvError::Disconnected) => return None,
                Ok(None) | Err(TryRecvError::Empty) => {}
            }
            tokio::select! {
                entry = self.ordered.recv() => {
                    if let Some(entry) = entry {
                        self.buffered_ordered = Some(entry);
                    } else {
                        self.ordered_closed = true;
                    }
                }
                _ = self.snapshot_ready.notified() => {}
            }
        }
    }

    fn try_recv(&mut self) -> StdResult<Option<MsgToUI>, TryRecvError> {
        if let Some(summary) = self.dropped_log_summary() {
            return Ok(Some(summary));
        }
        loop {
            if self.buffered_ordered.is_none() && !self.ordered_closed {
                match self.ordered.try_recv() {
                    Ok(entry) => self.buffered_ordered = Some(entry),
                    Err(TryRecvError::Disconnected) => self.ordered_closed = true,
                    Err(TryRecvError::Empty) => {}
                }
            }

            let ordered_sequence = self.buffered_ordered.as_ref().map(|entry| entry.sequence);
            let snapshot_sequence = self.snapshots.lock().next_sequence();
            if let Some(sequence) = snapshot_sequence
                && ordered_sequence.is_none_or(|ordered| sequence < ordered)
            {
                if let Some(entry) = self.snapshots.lock().take(sequence) {
                    return Ok(Some(entry.message));
                }
                continue;
            }
            if let Some(entry) = self.buffered_ordered.take() {
                return Ok(Some(entry.message));
            }
            return if self.ordered_closed {
                Err(TryRecvError::Disconnected)
            } else {
                Ok(None)
            };
        }
    }
}

fn event_delivery_channel() -> (EventDeliveryTx, EventDeliveryRx) {
    let (ordered, ordered_rx) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let snapshots = Arc::new(Mutex::new(PendingSnapshots::default()));
    let snapshot_ready = Arc::new(Notify::new());
    let counters = Arc::new(DeliveryCounters::default());
    let next_sequence = Arc::new(AtomicU64::new(0));
    (
        EventDeliveryTx {
            ordered,
            snapshots: snapshots.clone(),
            snapshot_ready: snapshot_ready.clone(),
            counters: counters.clone(),
            next_sequence,
        },
        EventDeliveryRx {
            ordered: ordered_rx,
            snapshots,
            snapshot_ready,
            counters,
            reported_dropped_logs: 0,
            buffered_ordered: None,
            ordered_closed: false,
        },
    )
}

/// Active IPC connection.
///
/// Holds the MRPC client and a bounded, message-aware delivery lane for
/// server→client notifications.
pub struct Connection {
    // Drop order matters: `client` must be released before `event_rx` so the
    // MRPC connection closes before we tear down the receive channel. Otherwise
    // in-flight notifications arrive after the receiver disappears, spamming
    // "Failed to send event to channel" errors during normal shutdown.
    client: MrpcClient,
    event_rx: EventDeliveryRx,
}

impl Connection {
    /// Connect to the server and return a connection handle
    pub async fn connect_unix(socket_path: &str) -> Result<Connection> {
        debug!("Connecting to MRPC server at: {}", socket_path);

        // Create event channel for receiving events from server
        let (event_tx, event_rx) = event_delivery_channel();

        // Create client handler
        let handler = ClientHandler { event_tx };

        // Connect to server
        let client = MrpcClient::connect_unix(socket_path, handler)
            .await
            .map_err(|e| Error::Ipc(format!("Failed to connect: {}", e)))?;

        info!("IPC client connected");

        Ok(Connection { client, event_rx })
    }

    async fn request(&mut self, method: HotkeyMethod, params: &[Value]) -> Result<Value> {
        self.client
            .send_request(method.as_str(), params)
            .await
            .map_err(|err| request_error(method, err))
    }

    async fn request_ok(&mut self, method: HotkeyMethod, params: &[Value]) -> Result<()> {
        match self.request(method, params).await? {
            Value::Boolean(true) => Ok(()),
            other => Err(Error::Ipc(format!(
                "Unexpected {} response: {:?}",
                method.as_str(),
                other
            ))),
        }
    }

    async fn request_binary<T: DeserializeOwned>(
        &mut self,
        method: HotkeyMethod,
        params: &[Value],
    ) -> Result<T> {
        value::binary_response(self.request(method, params).await?, method.as_str())
    }

    /// Send shutdown request to server (typed convenience method).
    pub async fn shutdown(&mut self) -> Result<()> {
        debug!("Sending shutdown request");
        self.request_ok(HotkeyMethod::Shutdown, &[]).await
    }

    /// Close this client connection without sending a server shutdown request.
    pub async fn close(self) -> Result<()> {
        self.client
            .close()
            .await
            .map_err(|err| Error::Ipc(format!("Failed to close connection: {err}")))
    }

    /// Set the config file path (server loads config from disk).
    pub async fn set_config_path(&mut self, path: &str) -> Result<()> {
        debug!("Sending set_config_path request");
        self.request_ok(HotkeyMethod::SetConfigPath, &[value::string_param(path)])
            .await
    }

    /// Receive the next UI/log event from the server.
    ///
    /// Returns a `MsgToUI` value. Keep polling this to avoid backpressure on
    /// the server’s event forwarder; disconnects are detected when the channel
    /// closes.
    pub async fn recv_event(&mut self) -> Result<MsgToUI> {
        self.event_rx
            .recv()
            .await
            .ok_or_else(|| Error::Ipc("Event channel closed".into()))
    }

    /// Return cumulative event delivery pressure counters.
    pub fn delivery_stats(&self) -> DeliveryStats {
        self.event_rx.counters.snapshot()
    }

    /// Return the next queued UI/log event without waiting.
    pub fn try_recv_event(&mut self) -> Result<Option<MsgToUI>> {
        match self.event_rx.try_recv() {
            Ok(event) => Ok(event),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(Error::Ipc("Event channel closed".into())),
        }
    }

    /// Inject a synthetic key down for a bound identifier.
    pub async fn inject_key_down(&mut self, ident: &str) -> Result<()> {
        self.inject_key(ident, InjectKind::Down, false).await
    }

    /// Inject a synthetic key up for a bound identifier.
    pub async fn inject_key_up(&mut self, ident: &str) -> Result<()> {
        self.inject_key(ident, InjectKind::Up, false).await
    }

    /// Inject a synthetic repeat key down for a bound identifier.
    pub async fn inject_key_repeat(&mut self, ident: &str) -> Result<()> {
        self.inject_key(ident, InjectKind::Down, true).await
    }

    async fn inject_key(&mut self, ident: &str, kind: InjectKind, repeat: bool) -> Result<()> {
        let req = InjectKeyReq {
            ident: ident.to_string(),
            kind,
            repeat,
        };
        let param = enc_inject_key(&req)?;
        self.request_ok(HotkeyMethod::InjectKey, &[param]).await
    }

    /// Get a snapshot of currently bound identifiers (sorted).
    pub async fn get_bindings(&mut self) -> Result<Vec<String>> {
        value::string_vec_response(
            self.request(HotkeyMethod::GetBindings, &[]).await?,
            HotkeyMethod::GetBindings.as_str(),
        )
    }

    /// Get the current depth (0 = root).
    pub async fn get_depth(&mut self) -> Result<usize> {
        value::usize_response(
            self.request(HotkeyMethod::GetDepth, &[]).await?,
            HotkeyMethod::GetDepth.as_str(),
        )
    }

    /// Get a diagnostic world status snapshot.
    pub async fn get_world_status(&mut self) -> Result<hotki_world::WorldStatus> {
        self.request_binary(HotkeyMethod::GetWorldStatus, &[]).await
    }

    /// Retrieve the current server status snapshot.
    pub async fn get_server_status(&mut self) -> Result<ServerStatusLite> {
        self.request_binary(HotkeyMethod::GetServerStatus, &[])
            .await
    }

    /// Get a lightweight world snapshot (windows + focused context).
    pub async fn get_world_snapshot(&mut self) -> Result<WorldSnapshotLite> {
        self.request_binary(HotkeyMethod::GetWorldSnapshot, &[])
            .await
    }
}

/// Convert an MRPC request failure into the server crate's error shape.
fn request_error(method: HotkeyMethod, err: RpcError) -> Error {
    match err {
        RpcError::Service(service) => match RpcErrorCode::from_service_name(&service.name) {
            Some(code) => Error::Rpc {
                method: method.as_str().to_string(),
                code,
                message: service_value_message(&service.value),
            },
            None => Error::Ipc(format!(
                "{} request failed: service error {}: {}",
                method.as_str(),
                service.name,
                service_value_message(&service.value)
            )),
        },
        other => Error::Ipc(format!("{} request failed: {}", method.as_str(), other)),
    }
}

/// Render the service error payload without exposing MessagePack debug noise for strings.
fn service_value_message(value: &Value) -> String {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("{value:?}"))
}

/// Client-side connection handler for receiving events
#[derive(Clone)]
struct ClientHandler {
    event_tx: EventDeliveryTx,
}

#[async_trait]
impl MrpcConnection for ClientHandler {
    async fn connected(&self, _client: RpcSender) -> StdResult<(), RpcError> {
        trace!("Client handler connected");
        Ok(())
    }

    async fn handle_request(
        &self,
        _client: RpcSender,
        method: &str,
        _params: Vec<Value>,
    ) -> StdResult<Value, RpcError> {
        // Client doesn't handle requests from server
        error!("Unexpected request from server: {}", method);
        Err(RpcError::Service(mrpc::ServiceError {
            name: "not_implemented".into(),
            value: Value::String("Client doesn't handle requests".into()),
        }))
    }

    async fn handle_notification(
        &self,
        _client: RpcSender,
        method: &str,
        params: Vec<Value>,
    ) -> StdResult<(), RpcError> {
        trace!("Received notification: {}", method);

        if method == HotkeyNotification::Notify.as_str() && !params.is_empty() {
            // Parse event and send to channel
            match dec_event(params[0].clone()) {
                Ok(msg) => match self.event_tx.send(msg).await {
                    DeliveryOutcome::Queued => {}
                    DeliveryOutcome::Coalesced => {
                        trace!("coalesced superseded client snapshot")
                    }
                    DeliveryOutcome::DroppedLogFull => {
                        trace!("dropped client log because bounded queue is full")
                    }
                    DeliveryOutcome::Closed => {
                        debug!("Dropping notify: client event receiver already closed")
                    }
                },
                Err(e) => {
                    error!("Failed to parse event: {}, raw value: {:?}", e, params[0]);
                }
            }
        }

        Ok(())
    }
}

/// Encode `inject_key` params as msgpack binary.
pub(crate) fn enc_inject_key(req: &InjectKeyReq) -> crate::Result<Value> {
    value::binary_param(req)
}

/// Decode a generic UI event from a notification param value.
pub(crate) fn dec_event(v: Value) -> crate::Result<hotki_protocol::MsgToUI> {
    hotki_protocol::ipc::codec::value_to_msg(v)
        .map_err(|e| crate::Error::Serialization(e.to_string()))
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        io::Write,
        os::unix::net::UnixListener,
        path::PathBuf,
        process, thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    use mrpc::{Message, Response};

    use super::*;

    fn log_message(index: usize) -> MsgToUI {
        MsgToUI::Log {
            level: "info".to_string(),
            target: "test".to_string(),
            message: format!("log {index}"),
        }
    }

    #[tokio::test]
    async fn client_delivery_coalesces_snapshots_and_summarizes_dropped_logs() {
        let (tx, mut rx) = event_delivery_channel();
        assert!(matches!(
            tx.send(MsgToUI::Heartbeat(1)).await,
            DeliveryOutcome::Queued
        ));
        assert!(matches!(
            tx.send(MsgToUI::Heartbeat(2)).await,
            DeliveryOutcome::Coalesced
        ));
        assert!(matches!(rx.try_recv(), Ok(Some(MsgToUI::Heartbeat(2)))));

        for index in 0..CLIENT_EVENT_CAPACITY {
            assert!(matches!(
                tx.send(log_message(index)).await,
                DeliveryOutcome::Queued
            ));
        }
        assert!(matches!(
            tx.send(log_message(CLIENT_EVENT_CAPACITY)).await,
            DeliveryOutcome::DroppedLogFull
        ));
        assert_eq!(tx.counters.snapshot().dropped_logs, 1);
        assert!(matches!(
            rx.try_recv(),
            Ok(Some(MsgToUI::Log { message, .. })) if message.contains("dropped 1")
        ));
    }

    #[tokio::test]
    async fn client_delivery_preserves_sequence_across_message_classes() {
        let (tx, mut rx) = event_delivery_channel();
        tx.send(MsgToUI::Notify {
            kind: hotki_protocol::NotifyKind::Info,
            title: "first".to_string(),
            text: "ordered".to_string(),
        })
        .await;
        tx.send(MsgToUI::Heartbeat(1)).await;

        assert!(matches!(
            rx.try_recv(),
            Ok(Some(MsgToUI::Notify { title, .. })) if title == "first"
        ));
        assert!(matches!(rx.try_recv(), Ok(Some(MsgToUI::Heartbeat(1)))));
    }

    fn tmp_socket_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        env::temp_dir().join(format!(
            "hotki-shutdown-{process_id}-{unique}.sock",
            process_id = process::id()
        ))
    }

    #[tokio::test]
    async fn shutdown_succeeds_when_peer_closes_after_ack() {
        let socket_path = tmp_socket_path();
        let listener = UnixListener::bind(&socket_path).expect("bind test socket");

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept shutdown client");
            let Message::Request(request) =
                Message::decode(&mut stream).expect("decode shutdown request")
            else {
                panic!("expected shutdown request");
            };
            assert_eq!(request.method, HotkeyMethod::Shutdown.as_str());

            Message::Response(Response {
                id: request.id,
                result: Ok(Value::Boolean(true)),
            })
            .encode(&mut stream)
            .expect("encode shutdown response");
            stream.flush().expect("flush shutdown response");
        });

        let mut connection = Connection::connect_unix(socket_path.to_str().expect("utf8 socket"))
            .await
            .expect("connect to test socket");

        connection.shutdown().await.expect("shutdown ack");
        server.join().expect("server thread");
        let _ = fs::remove_file(socket_path);
    }
}
