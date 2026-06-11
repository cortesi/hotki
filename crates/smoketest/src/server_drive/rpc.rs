//! Synchronous wrapper around the production server connection.

use std::time::Duration;

use hotki_protocol::{
    MsgToUI,
    rpc::{InjectKind, ServerStatusLite},
};
use hotki_server::{Client, Connection};
use tokio::{runtime::Runtime, time::timeout};

use super::{DriverError, DriverResult};

/// Synchronous facade for production RPCs used by the smoketest driver.
pub(super) struct ServerRpc<'a> {
    /// Runtime used to block on async MRPC methods.
    runtime: &'a Runtime,
    /// Active typed server connection.
    conn: &'a mut Connection,
}

impl<'a> ServerRpc<'a> {
    /// Wrap a runtime and active connection.
    pub(super) fn new(runtime: &'a Runtime, conn: &'a mut Connection) -> Self {
        Self { runtime, conn }
    }

    /// Wrap a runtime and the active connection inside a client.
    pub(super) fn from_client(runtime: &'a Runtime, client: &'a mut Client) -> DriverResult<Self> {
        let conn = client.connection().map_err(server_error)?;
        Ok(Self::new(runtime, conn))
    }

    /// Fetch server status.
    pub(super) fn server_status(&mut self) -> DriverResult<ServerStatusLite> {
        self.runtime
            .block_on(self.conn.get_server_status())
            .map_err(server_error)
    }

    /// Send the production shutdown RPC to the server.
    pub(super) fn shutdown(&mut self) -> DriverResult<()> {
        self.runtime
            .block_on(self.conn.shutdown())
            .map_err(server_error)
    }

    /// Inject one key event through the production RPC API.
    pub(super) fn inject_key(
        &mut self,
        ident: &str,
        kind: InjectKind,
        repeat: bool,
    ) -> DriverResult<()> {
        let result = match (kind, repeat) {
            (InjectKind::Down, true) => self.runtime.block_on(self.conn.inject_key_repeat(ident)),
            (InjectKind::Down, false) => self.runtime.block_on(self.conn.inject_key_down(ident)),
            (InjectKind::Up, _) => self.runtime.block_on(self.conn.inject_key_up(ident)),
        };
        result.map_err(server_error)
    }

    /// Fetch the binding identifiers currently reported by the server.
    pub(super) fn bindings(&mut self) -> DriverResult<Vec<String>> {
        self.runtime
            .block_on(self.conn.get_bindings())
            .map_err(server_error)
    }

    /// Fetch current mode-stack depth.
    #[cfg(test)]
    pub(super) fn depth(&mut self) -> DriverResult<usize> {
        self.runtime
            .block_on(self.conn.get_depth())
            .map_err(server_error)
    }

    /// Return one queued event without waiting.
    pub(super) fn try_recv_event(&mut self) -> DriverResult<Option<MsgToUI>> {
        self.conn.try_recv_event().map_err(server_error)
    }

    /// Wait for one event until `remaining` elapses.
    pub(super) fn recv_event_timeout(
        &mut self,
        remaining: Duration,
    ) -> DriverResult<Option<MsgToUI>> {
        match self
            .runtime
            .block_on(async { timeout(remaining, self.conn.recv_event()).await })
        {
            Ok(Ok(event)) => Ok(Some(event)),
            Ok(Err(err)) => Err(server_error(err)),
            Err(_) => Ok(None),
        }
    }
}

/// Convert a server crate error into a driver error.
fn server_error(err: hotki_server::Error) -> DriverError {
    match err {
        hotki_server::Error::Rpc { code, message, .. } => {
            DriverError::ServerRpcFailure { code, message }
        }
        other => DriverError::ServerFailure {
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use hotki_server::RpcErrorCode;

    use super::*;

    #[test]
    fn server_error_preserves_typed_rpc_code() {
        let err = hotki_server::Error::Rpc {
            method: "inject_key_down".to_string(),
            code: RpcErrorCode::KeyNotBound,
            message: "missing".to_string(),
        };

        assert!(matches!(
            server_error(err),
            DriverError::ServerRpcFailure {
                code: RpcErrorCode::KeyNotBound,
                ..
            }
        ));
    }
}
