//! MRPC connection implementation for the hotkey server

use std::{result::Result as StdResult, sync::Arc};

use async_trait::async_trait;
use mrpc::{Client as MrpcClient, Connection as MrpcConnection, RpcError, RpcSender, Value};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, trace};

use hotki_protocol::MsgToUI;

use crate::{
    Error, Result,
    ipc::rpc::{HotkeyMethod, HotkeyNotification},
};

/// Active IPC connection.
///
/// Holds the MRPC client and an unbounded channel that carries
/// server→client notifications. Messages include HUD updates, log
/// forwarding, and a heartbeat for liveness.
pub struct Connection {
    client: MrpcClient<ClientHandler>,
    event_rx: UnboundedReceiver<MsgToUI>,
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

        Ok(Connection { client, event_rx })
    }

    /// Send shutdown request to server (typed convenience method).
    pub async fn shutdown(&mut self) -> Result<()> {
        debug!("Sending shutdown request");
        let response = self
            .client
            .send_request(HotkeyMethod::Shutdown.as_str(), &[])
            .await
            .map_err(|e| Error::Ipc(format!("Shutdown request failed: {}", e)))?;
        match response {
            Value::Boolean(true) => Ok(()),
            _ => Err(Error::Ipc("Unexpected shutdown response".into())),
        }
    }

    /// Set the full configuration (typed convenience method).
    pub async fn set_config(&mut self, cfg: config::Config) -> Result<()> {
        debug!("Sending set_config request");
        let response = self
            .client
            .send_request(
                HotkeyMethod::SetConfig.as_str(),
                &[super::rpc::enc_set_config(&cfg)],
            )
            .await
            .map_err(|e| Error::Ipc(format!("Set config request failed: {}", e)))?;
        match response {
            Value::Boolean(true) => Ok(()),
            _ => Err(Error::Ipc("Unexpected set_config response".into())),
        }
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
            "down" => crate::ipc::rpc::InjectKind::Down,
            "up" => crate::ipc::rpc::InjectKind::Up,
            other => return Err(Error::Ipc(format!("invalid kind: {}", other))),
        };
        let req = crate::ipc::rpc::InjectKeyReq {
            ident: ident.to_string(),
            kind: kind_enum,
            repeat,
        };
        let response = self
            .client
            .send_request(
                HotkeyMethod::InjectKey.as_str(),
                &[super::rpc::enc_inject_key(&req)],
            )
            .await
            .map_err(|e| Error::Ipc(format!("inject_key request failed: {}", e)))?;
        match response {
            Value::Boolean(true) => Ok(()),
            other => Err(Error::Ipc(format!(
                "Unexpected inject_key response: {:?}",
                other
            ))),
        }
    }

    /// Get a snapshot of currently bound identifiers (sorted).
    pub async fn get_bindings(&mut self) -> Result<Vec<String>> {
        let response = self
            .client
            .send_request(HotkeyMethod::GetBindings.as_str(), &[])
            .await
            .map_err(|e| Error::Ipc(format!("get_bindings request failed: {}", e)))?;
        match response {
            Value::Array(vals) => {
                let mut out = Vec::with_capacity(vals.len());
                for v in vals {
                    match v {
                        Value::String(s) => out.push(s.to_string()),
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
        let response = self
            .client
            .send_request(HotkeyMethod::GetDepth.as_str(), &[])
            .await
            .map_err(|e| Error::Ipc(format!("get_depth request failed: {}", e)))?;
        match response {
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
            match super::rpc::dec_event(params[0].clone()) {
                Ok(msg) => {
                    if let Err(e) = self.event_tx.send(msg) {
                        error!("Failed to send event to channel: {}", e);
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
