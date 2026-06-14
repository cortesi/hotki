use std::{collections::VecDeque, fmt::Write as _, path::PathBuf, pin::Pin};

/// UI event forwarding and repaint coordination.
mod ui_sink;

use hotki_protocol::{NotifyKind, ipc::heartbeat, rpc::InjectKind};
use hotki_server::{Client, Result as ServerResult};
use tokio::{
    sync::{mpsc, oneshot},
    time::{Duration, Instant as TokioInstant, Sleep, sleep},
};
use tracing::{debug, error, info, warn};
use ui_sink::UiSink;

use crate::{app::UiEvent, logs, permissions::check_permissions, runtime::ControlMsg};

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
    /// Whether the auto-spawned server should observe physical keyboard events.
    server_event_tap_enabled: bool,
    /// Whether to log periodic world snapshots.
    dumpworld: bool,
    /// Current server connection lifecycle.
    state: ConnectionState,
    /// Server-bound controls received before a connection is ready.
    pending_controls: VecDeque<ServerControl>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Server connection lifecycle visible to control routing.
enum ConnectionState {
    /// No server connection has been attempted yet, or the previous one ended.
    Disconnected,
    /// Initial connect is in progress.
    Connecting,
    /// Connected and ready to send server-bound control messages.
    Connected,
    /// Shutdown has been requested.
    ShuttingDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Result of processing a control message.
enum ControlOutcome {
    /// Continue connecting or driving events.
    Continue,
    /// Stop the current driver loop.
    Stop,
}

impl ControlOutcome {
    /// Whether the caller should stop the active loop.
    fn should_stop(self) -> bool {
        matches!(self, Self::Stop)
    }
}

/// Local UI control that does not require a server connection.
#[derive(Debug, PartialEq, Eq)]
enum LocalControl {
    /// Show the permissions helper window.
    OpenPermissionsHelp,
    /// Forward a user-facing notice into the app UI.
    Notice {
        /// Notice severity kind.
        kind: NotifyKind,
        /// Notice title text.
        title: String,
        /// Notice body text.
        text: String,
    },
}

/// Server-bound control command.
#[derive(Debug, PartialEq, Eq)]
enum ServerControl {
    /// Reload from disk using the configured config path.
    Reload,
    /// Request a server-side theme switch by name.
    SwitchTheme(String),
    /// Inject a synthetic key event through the connected server.
    InjectKey {
        /// Key chord identifier, for example `shift+cmd+0`.
        ident: String,
        /// Key event kind.
        kind: InjectKind,
        /// Whether this down event is a repeat.
        repeat: bool,
        /// Whether failed injection should be surfaced to the user.
        report_errors: bool,
    },
}

/// Routing class for a UI/runtime control message.
#[derive(Debug, PartialEq, Eq)]
enum ControlRoute {
    /// Control can be handled locally without a server connection.
    Local(LocalControl),
    /// Control must be delivered to the server.
    Server(ServerControl),
    /// Control requests app/server shutdown.
    Shutdown,
}

impl ControlRoute {
    /// Classify a control message by the connection it needs.
    fn from_msg(msg: ControlMsg) -> Self {
        match msg {
            ControlMsg::Shutdown => Self::Shutdown,
            ControlMsg::Reload => Self::Server(ServerControl::Reload),
            ControlMsg::SwitchTheme(name) => Self::Server(ServerControl::SwitchTheme(name)),
            ControlMsg::InjectKey {
                ident,
                kind,
                repeat,
                report_errors,
            } => Self::Server(ServerControl::InjectKey {
                ident,
                kind,
                repeat,
                report_errors,
            }),
            ControlMsg::OpenPermissionsHelp => Self::Local(LocalControl::OpenPermissionsHelp),
            ControlMsg::Notice { kind, title, text } => {
                Self::Local(LocalControl::Notice { kind, title, text })
            }
        }
    }
}

impl ConnectionDriver {
    /// Construct a new driver with channels and initial config.
    pub(crate) fn new(
        config_path: PathBuf,
        server_log_filter: Option<String>,
        tx_keys: mpsc::UnboundedSender<UiEvent>,
        egui_ctx: egui::Context,
        rx_ctrl: mpsc::UnboundedReceiver<ControlMsg>,
        server_event_tap_enabled: bool,
        dumpworld: bool,
    ) -> Self {
        Self {
            config_path,
            server_log_filter,
            ui: UiSink::new(tx_keys, egui_ctx),
            rx_ctrl,
            server_event_tap_enabled,
            dumpworld,
            state: ConnectionState::Disconnected,
            pending_controls: VecDeque::new(),
        }
    }

    /// Update connection lifecycle state when it changes.
    fn set_state(&mut self, state: ConnectionState) {
        if self.state != state {
            debug!(previous = ?self.state, next = ?state, "connection state changed");
            self.state = state;
            let connected = matches!(state, ConnectionState::Connected);
            self.ui.set_server_connected(connected);
            if !connected {
                self.ui.set_server_bindings(Vec::new());
            }
        }
    }

    /// Handle control messages that do not require a connected server.
    fn handle_local_control(&self, control: LocalControl) {
        match control {
            LocalControl::OpenPermissionsHelp => self.ui.show_permissions_help(),
            LocalControl::Notice { kind, title, text } => {
                self.notify_local(kind, &title, &text);
            }
        }
    }

    /// Handle control messages that require an active server connection.
    async fn handle_server_control(
        &self,
        conn: &mut hotki_server::Connection,
        control: ServerControl,
    ) {
        match control {
            ServerControl::Reload => {
                self.reload_config(conn).await;
            }
            ServerControl::SwitchTheme(name) => {
                self.switch_theme(conn, &name).await;
            }
            ServerControl::InjectKey {
                ident,
                kind,
                repeat,
                report_errors,
            } => {
                self.inject_key(conn, &ident, kind, repeat, report_errors)
                    .await;
            }
        }
    }

    /// Route control messages to local or connected handlers, queueing as needed.
    async fn route_control_msg(
        &mut self,
        conn: Option<&mut hotki_server::Connection>,
        msg: ControlMsg,
    ) -> ControlOutcome {
        match ControlRoute::from_msg(msg) {
            ControlRoute::Local(control) => {
                self.handle_local_control(control);
                ControlOutcome::Continue
            }
            ControlRoute::Server(control) => {
                if let Some(conn) = conn {
                    self.handle_server_control(conn, control).await;
                } else {
                    self.pending_controls.push_back(control);
                }
                ControlOutcome::Continue
            }
            ControlRoute::Shutdown => {
                self.begin_shutdown();
                if let Some(conn) = conn {
                    conn.shutdown().await.ok();
                }
                ControlOutcome::Stop
            }
        }
    }

    /// Trigger UI shutdown and record lifecycle state.
    fn begin_shutdown(&mut self) {
        self.set_state(ConnectionState::ShuttingDown);
        self.ui.trigger_graceful_shutdown(750);
    }

    /// Drain server-bound controls queued while connecting.
    async fn drain_pending_controls(&mut self, conn: &mut hotki_server::Connection) {
        while let Some(control) = self.pending_controls.pop_front() {
            self.handle_server_control(conn, control).await;
        }
    }

    /// Record and display a server-originated notification.
    fn notify_remote(&self, kind: NotifyKind, title: &str, text: &str) {
        self.ui.notify(kind, title, text);
    }

    /// Record, log, and display a client-originated notification.
    fn notify_local(&self, kind: NotifyKind, title: &str, text: &str) {
        logs::push_client_notification(kind, title, text);
        self.ui.notify(kind, title, text);
    }

    /// Show permission guidance and keep the server from being spawned without required grants.
    fn event_tap_permissions_missing(&self) -> bool {
        if !self.server_event_tap_enabled {
            return false;
        }

        let perms = check_permissions();
        if perms.accessibility_ok() && perms.input_ok() {
            return false;
        }

        self.ui.show_permissions_help();
        self.notify_local(
            NotifyKind::Error,
            "Permissions",
            "Grant Accessibility and Input Monitoring to Hotki, then restart Hotki.",
        );
        true
    }

    /// Reload the current config path on the server and notify the UI.
    async fn reload_config(&self, conn: &mut hotki_server::Connection) {
        match conn
            .set_config_path(self.config_path.to_string_lossy().as_ref())
            .await
        {
            Ok(()) => {
                self.notify_local(NotifyKind::Success, "Config", "Reloaded successfully");
                self.ui.set_config_path(Some(self.config_path.clone()));
                self.refresh_server_bindings(conn).await;
            }
            Err(err) => self.notify_local(NotifyKind::Error, "Config", &err.to_string()),
        }
    }

    /// Switch the server-side theme by name and request an updated HUD/style.
    async fn switch_theme(&self, conn: &mut hotki_server::Connection, name: &str) {
        if let Err(err) = conn.set_theme(name).await {
            self.notify_local(NotifyKind::Error, "Theme", &err.to_string());
        } else {
            self.ui.request_repaint();
        }
    }

    /// Inject a synthetic key event through the server-side test hook.
    async fn inject_key(
        &self,
        conn: &mut hotki_server::Connection,
        ident: &str,
        kind: InjectKind,
        repeat: bool,
        report_errors: bool,
    ) {
        let result = match (kind, repeat) {
            (InjectKind::Down, true) => conn.inject_key_repeat(ident).await,
            (InjectKind::Down, false) => conn.inject_key_down(ident).await,
            (InjectKind::Up, _) => conn.inject_key_up(ident).await,
        };
        if let Err(err) = result
            && report_errors
        {
            self.notify_local(
                NotifyKind::Error,
                "Devtools",
                &format!("Failed to inject {ident}: {err}"),
            );
        }
    }

    /// Process a message from the server and update the UI accordingly.
    async fn handle_server_msg(&self, msg: hotki_protocol::MsgToUI) {
        match msg {
            hotki_protocol::MsgToUI::HudUpdate { hud, displays } => {
                self.ui
                    .send_message(hotki_protocol::MsgToUI::HudUpdate { hud, displays });
            }
            hotki_protocol::MsgToUI::Notify { kind, title, text } => {
                self.notify_remote(kind, &title, &text);
            }
            hotki_protocol::MsgToUI::ClearNotifications => {
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
                if self.dumpworld {
                    debug!("World event: {:?}", msg);
                }
            }
        }
    }

    /// Connect to the server, draining any queued control messages after connect.
    pub(crate) async fn connect(&mut self) -> Option<Client> {
        if self.event_tap_permissions_missing() {
            self.set_state(ConnectionState::Disconnected);
            return None;
        }

        let mut rx_conn_ready = spawn_connect(
            self.server_log_filter.clone(),
            self.server_event_tap_enabled,
        );
        self.set_state(ConnectionState::Connecting);
        let mut client = self.wait_for_connected_client(&mut rx_conn_ready).await?;

        let conn = match client.connection() {
            Ok(conn) => conn,
            Err(err) => {
                error!("Failed to get client connection: {}", err);
                self.set_state(ConnectionState::Disconnected);
                return None;
            }
        };
        self.send_initial_config(conn).await?;
        self.refresh_server_bindings(conn).await;
        self.set_state(ConnectionState::Connected);
        self.ui.set_config_path(Some(self.config_path.clone()));
        info!("Config path sent to server engine");

        self.drain_pending_controls(conn).await;

        Some(client)
    }

    /// Wait for the background connect task while still accepting local controls.
    async fn wait_for_connected_client(
        &mut self,
        rx_conn_ready: &mut oneshot::Receiver<ServerResult<Client>>,
    ) -> Option<Client> {
        loop {
            tokio::select! {
                biased;
                res = &mut *rx_conn_ready => return self.handle_connect_result(res),
                Some(msg) = self.rx_ctrl.recv() => {
                    if self.route_control_msg(None, msg).await.should_stop() {
                        return None;
                    }
                }
            }
        }
    }

    /// Convert the background connect result into a client or user-facing error.
    fn handle_connect_result(
        &mut self,
        result: Result<ServerResult<Client>, oneshot::error::RecvError>,
    ) -> Option<Client> {
        match result {
            Ok(Ok(client)) => Some(client),
            Ok(Err(err)) => {
                error!("Failed to connect to hotkey server: {}", err);
                self.notify_local(
                    NotifyKind::Error,
                    "Connection",
                    &format!("Failed to start hotkey server: {err}"),
                );
                self.set_state(ConnectionState::Disconnected);
                None
            }
            Err(_) => {
                error!("Connect task canceled before reporting a result");
                self.set_state(ConnectionState::Disconnected);
                None
            }
        }
    }

    /// Send the active config path to the connected server.
    async fn send_initial_config(&mut self, conn: &mut hotki_server::Connection) -> Option<()> {
        if let Err(err) = conn
            .set_config_path(self.config_path.to_string_lossy().as_ref())
            .await
        {
            error!("Failed to set config path on server: {}", err);
            self.set_state(ConnectionState::Disconnected);
            return None;
        }
        Some(())
    }

    /// Refresh UI-visible server binding diagnostics.
    async fn refresh_server_bindings(&self, conn: &mut hotki_server::Connection) {
        match conn.get_bindings().await {
            Ok(bindings) => self.ui.set_server_bindings(bindings),
            Err(err) => warn!("failed to refresh server bindings: {err}"),
        }
    }

    /// Main UI event loop once connected: handles control, server events, and heartbeat.
    pub(crate) async fn drive_events(&mut self, client: &mut Client) {
        let conn = match client.connection() {
            Ok(conn) => conn,
            Err(err) => {
                error!("Failed to get client connection: {}", err);
                self.set_state(ConnectionState::Disconnected);
                return;
            }
        };
        self.set_state(ConnectionState::Connected);

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

        loop {
            if !self
                .drive_event_once(
                    conn,
                    &mut hb_timer,
                    &mut dump_timer,
                    dump_interval,
                    dump_far_future,
                )
                .await
            {
                break;
            }
        }
        info!("Exiting key loop");
        if self.state != ConnectionState::ShuttingDown {
            self.begin_shutdown();
        }
    }

    /// Drive one select iteration for the connected event loop.
    async fn drive_event_once(
        &mut self,
        conn: &mut hotki_server::Connection,
        hb_timer: &mut Pin<&mut Sleep>,
        dump_timer: &mut Pin<&mut Sleep>,
        dump_interval: Duration,
        dump_far_future: Duration,
    ) -> bool {
        tokio::select! {
            biased;
            _ = hb_timer.as_mut() => {
                error!("No IPC activity within heartbeat timeout; exiting UI event loop");
                false
            }
            Some(msg) = self.rx_ctrl.recv() => {
                !self.route_control_msg(Some(conn), msg).await.should_stop()
            }
            resp = conn.recv_event() => self.handle_recv_event_result(resp, hb_timer).await,
            _ = dump_timer.as_mut() => {
                let next = self.compute_dump_reset(conn, dump_interval, dump_far_future).await;
                dump_timer.as_mut().reset(TokioInstant::now() + next);
                true
            }
        }
    }

    /// Handle one server event receive result.
    async fn handle_recv_event_result(
        &self,
        resp: hotki_server::Result<hotki_protocol::MsgToUI>,
        hb_timer: &mut Pin<&mut Sleep>,
    ) -> bool {
        match resp {
            Ok(msg) => {
                hb_timer
                    .as_mut()
                    .reset(TokioInstant::now() + heartbeat::timeout());
                self.handle_server_msg(msg).await;
                true
            }
            Err(hotki_server::Error::Ipc(ref message)) if message == "Event channel closed" => {
                tracing::info!("Event channel closed; exiting event loop");
                false
            }
            Err(err) => {
                error!("Connection error receiving event: {}", err);
                false
            }
        }
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
fn spawn_connect(
    log_filter: Option<String>,
    server_event_tap_enabled: bool,
) -> oneshot::Receiver<ServerResult<Client>> {
    let (tx_conn_ready, rx) = oneshot::channel::<ServerResult<Client>>();
    tokio::spawn(async move {
        let mut client = Client::new()
            .with_auto_spawn_server()
            .with_server_event_tap_enabled(server_event_tap_enabled);
        if let Some(filter) = log_filter {
            client = client.with_server_log_filter(filter);
        }
        let result = client.connect().await;
        if tx_conn_ready.send(result).is_err() {
            tracing::warn!("connect-ready channel closed before send");
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use hotki_protocol::{NotifyKind, rpc::InjectKind};

    use super::{ConnectionState, ControlOutcome, ControlRoute, LocalControl, ServerControl};
    use crate::runtime::ControlMsg;

    #[test]
    fn control_route_separates_local_server_and_shutdown_messages() {
        assert_eq!(
            ControlRoute::from_msg(ControlMsg::OpenPermissionsHelp),
            ControlRoute::Local(LocalControl::OpenPermissionsHelp)
        );
        assert_eq!(
            ControlRoute::from_msg(ControlMsg::Notice {
                kind: NotifyKind::Info,
                title: "Config".to_string(),
                text: "Reloaded".to_string(),
            }),
            ControlRoute::Local(LocalControl::Notice {
                kind: NotifyKind::Info,
                title: "Config".to_string(),
                text: "Reloaded".to_string(),
            })
        );
        assert_eq!(
            ControlRoute::from_msg(ControlMsg::Reload),
            ControlRoute::Server(ServerControl::Reload)
        );
        assert_eq!(
            ControlRoute::from_msg(ControlMsg::SwitchTheme("dark".to_string())),
            ControlRoute::Server(ServerControl::SwitchTheme("dark".to_string()))
        );
        assert_eq!(
            ControlRoute::from_msg(ControlMsg::InjectKey {
                ident: "shift+cmd+0".to_string(),
                kind: InjectKind::Down,
                repeat: false,
                report_errors: true,
            }),
            ControlRoute::Server(ServerControl::InjectKey {
                ident: "shift+cmd+0".to_string(),
                kind: InjectKind::Down,
                repeat: false,
                report_errors: true,
            })
        );
        assert_eq!(
            ControlRoute::from_msg(ControlMsg::Shutdown),
            ControlRoute::Shutdown
        );
    }

    #[test]
    fn control_outcome_names_loop_exit_decision() {
        assert!(!ControlOutcome::Continue.should_stop());
        assert!(ControlOutcome::Stop.should_stop());
    }

    #[test]
    fn connection_state_covers_runtime_lifecycle() {
        let states = [
            ConnectionState::Disconnected,
            ConnectionState::Connecting,
            ConnectionState::Connected,
            ConnectionState::ShuttingDown,
        ];

        assert_eq!(states.len(), 4);
    }
}
