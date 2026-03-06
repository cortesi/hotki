use std::{path::PathBuf, process};

use hotki_protocol::{NotifyKind, WorldStreamMsg};
use hotki_server::{
    Connection,
    smoketest_bridge::{
        BridgeEvent, BridgeNotifications, BridgeRequest, BridgeResponse, control_socket_path,
        handshake_response,
    },
};
use tokio::sync::{broadcast, mpsc};
use tracing::debug;

use super::ui_sink::UiSink;
use crate::{runtime::ControlMsg, smoketest_bridge::init_test_bridge};

/// UI-side smoketest bridge state and event buffering.
pub(super) struct BridgeState {
    /// Pending socket path for the local bridge listener.
    test_bridge_path: Option<PathBuf>,
    /// Broadcast channel used to fan bridge events out to clients.
    events: broadcast::Sender<BridgeEvent>,
    /// Recent notifications retained for handshake snapshots.
    notifications: BridgeNotifications,
}

impl BridgeState {
    /// Max number of notifications retained for smoketest snapshots.
    pub(super) const MAX_NOTIFICATIONS: usize = 32;

    /// Create the bridge state for the current UI process.
    pub(super) fn new() -> Self {
        let server_socket = hotki_server::socket_path_for_pid(process::id());
        let test_bridge_path = Some(PathBuf::from(control_socket_path(&server_socket)));
        let (events, _rx) = broadcast::channel(128);
        Self {
            test_bridge_path,
            events,
            notifications: BridgeNotifications::new(Self::MAX_NOTIFICATIONS),
        }
    }

    /// Record a user-facing notification for future smoketest snapshots.
    pub(super) fn record_notification(&mut self, kind: NotifyKind, title: &str, text: &str) {
        self.notifications.record(kind, title, text);
    }

    /// Clear the retained bridge notifications.
    pub(super) fn clear_notifications(&mut self) {
        self.notifications.clear();
    }

    /// Broadcast a smoketest bridge event if listeners are present.
    pub(super) fn emit_event(&self, event: BridgeEvent) {
        self.events.send(event).ok();
    }

    /// Start the local smoketest bridge listener once the runtime is connected.
    pub(super) async fn ensure_listener(
        &mut self,
        tx_ctrl_runtime: mpsc::UnboundedSender<ControlMsg>,
    ) {
        if let Some(path) = self.test_bridge_path.take()
            && let Err(err) =
                init_test_bridge(path.clone(), tx_ctrl_runtime, self.events.clone()).await
        {
            tracing::warn!(?err, socket = %path.display(), "failed to initialize smoketest bridge");
            self.test_bridge_path = Some(path);
        }
    }

    /// Execute a smoketest bridge request against the live server.
    pub(super) async fn handle_test_command(
        &self,
        conn: &mut Connection,
        req: BridgeRequest,
        config_path: &mut PathBuf,
        ui: &UiSink,
    ) -> BridgeResponse {
        match req {
            BridgeRequest::Ping => match self.handshake_response(conn).await {
                Ok(response) => response,
                Err(err) => BridgeResponse::Err { message: err },
            },
            BridgeRequest::SetConfig { path } => {
                match (BridgeRequest::SetConfig { path: path.clone() })
                    .execute(conn)
                    .await
                {
                    Ok(BridgeResponse::Ok) => {
                        *config_path = PathBuf::from(&path);
                        ui.set_config_path(Some(config_path.clone()));
                        BridgeResponse::Ok
                    }
                    Ok(other) => other,
                    Err(err) => BridgeResponse::Err {
                        message: err.to_string(),
                    },
                }
            }
            other => match other.execute(conn).await {
                Ok(response) => response,
                Err(err) => BridgeResponse::Err {
                    message: err.to_string(),
                },
            },
        }
    }

    /// Forward world stream messages to the smoketest bridge.
    pub(super) fn handle_world_stream(&self, dumpworld: bool, msg: WorldStreamMsg) {
        if dumpworld {
            debug!("World event: {:?}", msg);
        }
        let WorldStreamMsg::FocusChanged(app) = msg;
        self.emit_event(BridgeEvent::Focus { app });
    }

    /// Build the handshake response from current server status and retained notifications.
    async fn handshake_response(&self, conn: &mut Connection) -> Result<BridgeResponse, String> {
        let status = conn
            .get_server_status()
            .await
            .map_err(|err| err.to_string())?;
        let notifications = self.notifications.snapshot();
        Ok(handshake_response(&status, notifications))
    }
}
