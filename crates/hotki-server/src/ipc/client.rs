//! MRPC connection implementation for the hotkey server

use std::{result::Result as StdResult, sync::Arc};

use async_trait::async_trait;
use hotki_protocol::{
    MsgToUI,
    rpc::{
        HotkeyMethod, HotkeyNotification, InjectKeyReq, InjectKind, ServerStatusLite,
        WorldSnapshotLite,
    },
};
use mrpc::{Client as MrpcClient, Connection as MrpcConnection, RpcError, RpcSender, Value};
use serde::de::DeserializeOwned;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, trace};

use crate::{Error, Result};

/// Active IPC connection.
///
/// Holds the MRPC client and an unbounded channel that carries
/// server→client notifications. Messages include HUD updates, log
/// forwarding, and a heartbeat for liveness.
pub struct Connection {
    // Drop order matters: `client` must be released before `event_rx` so the
    // MRPC connection closes before we tear down the receive channel. Otherwise
    // in-flight notifications arrive after the receiver disappears, spamming
    // "Failed to send event to channel" errors during normal shutdown.
    event_rx: UnboundedReceiver<MsgToUI>,
    client: MrpcClient<ClientHandler>,
}

impl Connection {
    /// Connect to the server and return a connection handle
    pub async fn connect_unix(socket_path: &str) -> Result<Connection> {
        debug!("Connecting to MRPC server at: {}", socket_path);

        // Create event channel for receiving events from server
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        // Create client handler
        let handler = ClientHandler {
            event_tx: Arc::new(event_tx),
        };

        // Connect to server
        let client = MrpcClient::connect_unix(socket_path, handler)
            .await
            .map_err(|e| Error::Ipc(format!("Failed to connect: {}", e)))?;

        info!("IPC client connected");

        Ok(Connection { event_rx, client })
    }

    async fn request(&mut self, method: HotkeyMethod, params: &[Value]) -> Result<Value> {
        self.client
            .send_request(method.as_str(), params)
            .await
            .map_err(|e| Error::Ipc(format!("{} request failed: {}", method.as_str(), e)))
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
        match self.request(method, params).await? {
            Value::Binary(bytes) => {
                rmp_serde::from_slice::<T>(&bytes).map_err(|e| Error::Serialization(e.to_string()))
            }
            other => Err(Error::Ipc(format!(
                "Unexpected {} response: {:?}",
                method.as_str(),
                other
            ))),
        }
    }

    /// Send shutdown request to server (typed convenience method).
    pub async fn shutdown(&mut self) -> Result<()> {
        debug!("Sending shutdown request");
        self.request_ok(HotkeyMethod::Shutdown, &[]).await
    }

    /// Set the full configuration (typed convenience method).
    pub async fn set_config(&mut self, cfg: config::Config) -> Result<()> {
        debug!("Sending set_config request");
        let param = enc_set_config(&cfg)?;
        self.request_ok(HotkeyMethod::SetConfig, &[param]).await
    }

    /// Set the config file path (server loads config from disk).
    pub async fn set_config_path(&mut self, path: &str) -> Result<config::Config> {
        debug!("Sending set_config_path request");
        let param = Value::String(path.into());
        self.request_binary(HotkeyMethod::SetConfigPath, &[param])
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

    /// Inject a synthetic key down for a bound identifier.
    pub async fn inject_key_down(&mut self, ident: &str) -> Result<()> {
        self.inject_key(ident, "down", false).await
    }

    /// Inject a synthetic key up for a bound identifier.
    pub async fn inject_key_up(&mut self, ident: &str) -> Result<()> {
        self.inject_key(ident, "up", false).await
    }

    /// Inject a synthetic repeat key down for a bound identifier.
    pub async fn inject_key_repeat(&mut self, ident: &str) -> Result<()> {
        self.inject_key(ident, "down", true).await
    }

    async fn inject_key(&mut self, ident: &str, kind: &str, repeat: bool) -> Result<()> {
        // Build a typed request and encode it via serde to msgpack
        let kind_enum = match kind {
            "down" => InjectKind::Down,
            "up" => InjectKind::Up,
            other => return Err(Error::Ipc(format!("invalid kind: {}", other))),
        };
        let req = InjectKeyReq {
            ident: ident.to_string(),
            kind: kind_enum,
            repeat,
        };
        let param = enc_inject_key(&req)?;
        self.request_ok(HotkeyMethod::InjectKey, &[param]).await
    }

    /// Get a snapshot of currently bound identifiers (sorted).
    pub async fn get_bindings(&mut self) -> Result<Vec<String>> {
        match self.request(HotkeyMethod::GetBindings, &[]).await? {
            Value::Array(vals) => {
                let mut out = Vec::with_capacity(vals.len());
                for v in vals {
                    match v {
                        Value::String(s) => match s.as_str() {
                            Some(v) => out.push(v.to_string()),
                            None => {
                                return Err(Error::Ipc(
                                    "Unexpected non-utf8 string in get_bindings".into(),
                                ));
                            }
                        },
                        other => {
                            return Err(Error::Ipc(format!(
                                "Unexpected element in get_bindings: {:?}",
                                other
                            )));
                        }
                    }
                }
                Ok(out)
            }
            other => Err(Error::Ipc(format!(
                "Unexpected get_bindings response: {:?}",
                other
            ))),
        }
    }

    /// Get the current depth (0 = root).
    pub async fn get_depth(&mut self) -> Result<usize> {
        match self.request(HotkeyMethod::GetDepth, &[]).await? {
            Value::Integer(i) => match i.as_u64() {
                Some(u) => Ok(u as usize),
                None => Err(Error::Ipc("Invalid depth value".into())),
            },
            other => Err(Error::Ipc(format!(
                "Unexpected get_depth response: {:?}",
                other
            ))),
        }
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

/// Client-side connection handler for receiving events
#[derive(Clone)]
struct ClientHandler {
    event_tx: Arc<UnboundedSender<MsgToUI>>,
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
                Ok(msg) => {
                    if let Err(err) = self.event_tx.send(msg) {
                        if self.event_tx.is_closed() {
                            debug!("Dropping notify: client event receiver already closed");
                        } else {
                            error!("Failed to send event to channel: {}", err);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to parse event: {}, raw value: {:?}", e, params[0]);
                }
            }
        }

        Ok(())
    }
}

/// Encode `set_config` params.
pub(crate) fn enc_set_config(cfg: &config::Config) -> crate::Result<Value> {
    let bytes = rmp_serde::to_vec_named(cfg)?;
    Ok(Value::Binary(bytes))
}

/// Encode `inject_key` params as msgpack binary.
pub(crate) fn enc_inject_key(req: &InjectKeyReq) -> crate::Result<Value> {
    let bytes = rmp_serde::to_vec_named(req)?;
    Ok(Value::Binary(bytes))
}

/// Decode a generic UI event from a notification param value.
pub(crate) fn dec_event(v: Value) -> crate::Result<hotki_protocol::MsgToUI> {
    hotki_protocol::ipc::codec::value_to_msg(v)
        .map_err(|e| crate::Error::Serialization(e.to_string()))
}
