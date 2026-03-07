use std::{collections::VecDeque, fmt::Write as _, path::PathBuf};

/// Smoketest bridge buffering and request execution.
mod bridge;
/// UI event forwarding and repaint coordination.
mod ui_sink;

use bridge::BridgeState;
use hotki_protocol::{NotifyKind, ipc::heartbeat};
use hotki_server::{
    Client,
    smoketest_bridge::{BridgeEvent, BridgeResponse},
};
use tokio::{
    sync::{mpsc, oneshot},
    time::{Duration, Instant as TokioInstant, Sleep, sleep},
};
use tracing::{error, info};
use ui_sink::UiSink;

use crate::{
    app::UiEvent,
    logs,
    permissions::check_permissions,
    runtime::{ControlMsg, TestCommand},
};

/// Drives the MRPC connection for the UI: connect, process events, and apply config/overrides.
pub struct ConnectionDriver {
    /// Path to the on-disk Hotki config.
    config_path: PathBuf,
    /// Optional log filter for any auto-spawned server process.
    server_log_filter: Option<String>,
    /// Collapsed UI event forwarding and repaint handling.
    ui: UiSink,
    /// Receiver of control messages from tray/UI.
    rx_ctrl: mpsc::UnboundedReceiver<ControlMsg>,
    /// Sender for control messages back into the runtime.
    tx_ctrl_runtime: mpsc::UnboundedSender<ControlMsg>,
    /// Whether to log periodic world snapshots.
    dumpworld: bool,
    /// Smoketest bridge lifecycle and event buffering.
    bridge: BridgeState,
}

impl ConnectionDriver {
    /// Construct a new driver with channels and initial config.
    pub(crate) fn new(
        config_path: PathBuf,
        server_log_filter: Option<String>,
        tx_keys: mpsc::UnboundedSender<UiEvent>,
        egui_ctx: egui::Context,
        rx_ctrl: mpsc::UnboundedReceiver<ControlMsg>,
        tx_ctrl_runtime: mpsc::UnboundedSender<ControlMsg>,
        dumpworld: bool,
    ) -> Self {
        Self {
            config_path,
            server_log_filter,
            ui: UiSink::new(tx_keys, egui_ctx),
            rx_ctrl,
            tx_ctrl_runtime,
            dumpworld,
            bridge: BridgeState::new(),
        }
    }

    /// Handle control messages that do not require a connected server.
    fn handle_local_control(&mut self, msg: ControlMsg) {
        match msg {
            ControlMsg::OpenPermissionsHelp => self.ui.show_permissions_help(),
            ControlMsg::Notice { kind, title, text } => self.notify(kind, &title, &text),
            _ => {}
        }
    }

    /// Handle control messages that require an active server connection.
    async fn handle_connected_control(
        &mut self,
        conn: &mut hotki_server::Connection,
        msg: ControlMsg,
    ) -> bool {
        match msg {
            ControlMsg::Shutdown => {
                self.ui.trigger_graceful_shutdown(750);
                conn.shutdown().await.ok();
                true
            }
            ControlMsg::Reload => {
                self.reload_config(conn).await;
                false
            }
            ControlMsg::SwitchTheme(name) => {
                self.switch_theme(conn, &name).await;
                false
            }
            ControlMsg::Test(cmd) => {
                let TestCommand {
                    command_id: _command_id,
                    req,
                    respond_to,
                } = cmd;
                let response = self
                    .bridge
                    .handle_test_command(conn, req, &mut self.config_path, &self.ui)
                    .await;
                respond_to.send(response).ok();
                false
            }
            other => {
                self.handle_local_control(other);
                false
            }
        }
    }

    /// Route control messages to local or connected handlers, queueing as needed.
    async fn route_control_msg(
        &mut self,
        conn: Option<&mut hotki_server::Connection>,
        msg: ControlMsg,
        pending: &mut VecDeque<ControlMsg>,
    ) -> bool {
        match (conn, msg) {
            (Some(conn), msg) => self.handle_connected_control(conn, msg).await,
            (None, ControlMsg::Shutdown) => {
                self.ui.trigger_graceful_shutdown(750);
                true
            }
            (None, ControlMsg::Reload) => {
                pending.push_back(ControlMsg::Reload);
                false
            }
            (None, ControlMsg::SwitchTheme(name)) => {
                pending.push_back(ControlMsg::SwitchTheme(name));
                false
            }
            (None, ControlMsg::Test(cmd)) => {
                cmd.respond_to
                    .send(BridgeResponse::Err {
                        message: "bridge not ready".to_string(),
                    })
                    .ok();
                false
            }
            (None, other) => {
                self.handle_local_control(other);
                false
            }
        }
    }

    /// Record a notification and forward it to the UI.
    fn notify(&mut self, kind: NotifyKind, title: &str, text: &str) {
        self.bridge.record_notification(kind, title, text);
        self.ui.notify(kind, title, text);
    }

    /// Reload the current config path on the server and notify the UI.
    async fn reload_config(&mut self, conn: &mut hotki_server::Connection) {
        match conn
            .set_config_path(self.config_path.to_string_lossy().as_ref())
            .await
        {
            Ok(()) => {
                self.notify(NotifyKind::Success, "Config", "Reloaded successfully");
                self.ui.set_config_path(Some(self.config_path.clone()));
            }
            Err(err) => self.notify(NotifyKind::Error, "Config", &err.to_string()),
        }
    }

    /// Switch the server-side theme by name and request an updated HUD/style.
    async fn switch_theme(&mut self, conn: &mut hotki_server::Connection, name: &str) {
        if let Err(err) = conn.set_theme(name).await {
            self.notify(NotifyKind::Error, "Theme", &err.to_string());
        } else {
            self.ui.request_repaint();
        }
    }

    /// Process a message from the server and update the UI/bridge accordingly.
    async fn handle_server_msg(
        &mut self,
        _conn: &mut hotki_server::Connection,
        msg: hotki_protocol::MsgToUI,
    ) {
        match msg {
            hotki_protocol::MsgToUI::HudUpdate { hud, displays } => {
                self.ui.send_message(hotki_protocol::MsgToUI::HudUpdate {
                    hud: hud.clone(),
                    displays: displays.clone(),
                });
                self.bridge.emit_event(BridgeEvent::Hud { hud, displays });
            }
            hotki_protocol::MsgToUI::Notify { kind, title, text } => {
                self.notify(kind, &title, &text);
            }
            hotki_protocol::MsgToUI::ClearNotifications => {
                self.bridge.clear_notifications();
                self.ui
                    .send_message(hotki_protocol::MsgToUI::ClearNotifications);
            }
            hotki_protocol::MsgToUI::SelectorUpdate(snapshot) => {
                self.ui
                    .send_message(hotki_protocol::MsgToUI::SelectorUpdate(snapshot));
            }
            hotki_protocol::MsgToUI::SelectorHide => {
                self.ui.send_message(hotki_protocol::MsgToUI::SelectorHide);
            }
            hotki_protocol::MsgToUI::ShowDetails(arg) => {
                self.ui
                    .send_message(hotki_protocol::MsgToUI::ShowDetails(arg));
            }
            hotki_protocol::MsgToUI::HotkeyTriggered(_) => {}
            hotki_protocol::MsgToUI::Log {
                level,
                target,
                message,
            } => {
                logs::push_server(level, target, message);
                self.ui.request_repaint();
            }
            hotki_protocol::MsgToUI::Heartbeat(_) => {}
            hotki_protocol::MsgToUI::World(msg) => {
                self.bridge.handle_world_stream(self.dumpworld, msg);
            }
        }
    }

    /// Connect to the server, draining any queued control messages after connect.
    pub(crate) async fn connect(&mut self) -> Option<Client> {
        let perms = check_permissions();
        if !perms.accessibility_ok() || !perms.input_ok() {
            self.ui.show_permissions_help();
        }

        let mut rx_conn_ready = spawn_connect(self.server_log_filter.clone());
        let mut preconnect_queue: VecDeque<ControlMsg> = VecDeque::new();
        let mut client: Client = loop {
            tokio::select! {
                biased;
                res = &mut rx_conn_ready => {
                    match res {
                        Ok(client) => break client,
                        Err(_) => {
                            error!("Connect task canceled");
                            sleep(Duration::from_millis(300)).await;
                            rx_conn_ready = spawn_connect(self.server_log_filter.clone());
                        }
                    }
                }
                Some(msg) = self.rx_ctrl.recv() => {
                    if self.route_control_msg(None, msg, &mut preconnect_queue).await {
                        return None;
                    }
                }
            }
        };

        let conn = match client.connection() {
            Ok(conn) => conn,
            Err(err) => {
                error!("Failed to get client connection: {}", err);
                return None;
            }
        };
        match conn
            .set_config_path(self.config_path.to_string_lossy().as_ref())
            .await
        {
            Ok(()) => {}
            Err(err) => {
                error!("Failed to set config path on server: {}", err);
                return None;
            }
        };
        self.ui.set_config_path(Some(self.config_path.clone()));
        info!("Config path sent to server engine");

        self.bridge
            .ensure_listener(self.tx_ctrl_runtime.clone())
            .await;

        while let Some(msg) = preconnect_queue.pop_front() {
            if self
                .route_control_msg(Some(conn), msg, &mut preconnect_queue)
                .await
            {
                return None;
            }
        }

        Some(client)
    }

    /// Main UI event loop once connected: handles control, server events, and heartbeat.
    pub(crate) async fn drive_events(&mut self, client: &mut Client) {
        let conn = match client.connection() {
            Ok(conn) => conn,
            Err(err) => {
                error!("Failed to get client connection: {}", err);
                return;
            }
        };

        let hb_timer: Sleep = sleep(heartbeat::timeout());
        tokio::pin!(hb_timer);

        let dump_interval = Duration::from_secs(5);
        let dump_far_future = Duration::from_secs(3600);
        let dump_timer: Sleep = sleep(if self.dumpworld {
            dump_interval
        } else {
            dump_far_future
        });
        tokio::pin!(dump_timer);

        let mut control_sink: VecDeque<ControlMsg> = VecDeque::new();

        loop {
            tokio::select! {
                biased;
                _ = &mut hb_timer => {
                    error!("No IPC activity within heartbeat timeout; exiting UI event loop");
                    break;
                }
                Some(msg) = self.rx_ctrl.recv() => {
                    if self.route_control_msg(Some(conn), msg, &mut control_sink).await {
                        break;
                    }
                }
                resp = conn.recv_event() => {
                    match resp {
                        Ok(msg) => {
                            hb_timer.as_mut().reset(TokioInstant::now() + heartbeat::timeout());
                            self.handle_server_msg(conn, msg).await;
                        }
                        Err(err) => {
                            match err {
                                hotki_server::Error::Ipc(ref s) if s == "Event channel closed" => {
                                    tracing::info!("Event channel closed; exiting event loop");
                                }
                                _ => error!("Connection error receiving event: {}", err),
                            }
                            break;
                        }
                    }
                }
                _ = &mut dump_timer => {
                    let next = self.compute_dump_reset(conn, dump_interval, dump_far_future).await;
                    dump_timer.as_mut().reset(TokioInstant::now() + next);
                }
            }
        }
        info!("Exiting key loop");
        self.ui.trigger_graceful_shutdown(750);
    }

    /// Compute the next dump timer reset and optionally log a world snapshot.
    async fn compute_dump_reset(
        &self,
        conn: &mut hotki_server::Connection,
        dump_interval: Duration,
        dump_far_future: Duration,
    ) -> Duration {
        if self.dumpworld {
            if let Ok(snap) = conn.get_world_snapshot().await {
                let mut out = String::new();
                let focused_ctx = snap
                    .focused
                    .as_ref()
                    .map(|focused| {
                        format!("{} (pid={}) — {}", focused.app, focused.pid, focused.title)
                    })
                    .unwrap_or_else(|| "none".to_string());
                let display_count = snap.displays.displays.len();
                let active_disp = snap
                    .displays
                    .active
                    .as_ref()
                    .map(|display| display.id.to_string())
                    .unwrap_or_else(|| "-".into());
                if writeln!(
                    out,
                    "World: focused={} displays={} active_display={}",
                    focused_ctx, display_count, active_disp
                )
                .is_err()
                {
                    tracing::warn!("failed to format world dump line");
                }
                tracing::info!(target: "hotki::worlddump", "\n{}", out);
            }
            dump_interval
        } else {
            dump_far_future
        }
    }
}

/// Spawn a background connect task and return a oneshot receiver for its result.
fn spawn_connect(log_filter: Option<String>) -> oneshot::Receiver<Client> {
    let (tx_conn_ready, rx) = oneshot::channel::<Client>();
    tokio::spawn(async move {
        let client = if let Some(filter) = log_filter {
            Client::new()
                .with_auto_spawn_server()
                .with_server_log_filter(filter)
        } else {
            Client::new().with_auto_spawn_server()
        };
        match client.connect().await {
            Ok(client) => {
                if tx_conn_ready.send(client).is_err() {
                    tracing::warn!("connect-ready channel closed before send");
                }
            }
            Err(err) => {
                tracing::error!("Failed to connect to hotkey server: {}", err);
            }
        }
    });
    rx
}
