use std::{collections::VecDeque, fmt::Write as _, path::PathBuf, pin::Pin};

/// UI event forwarding and repaint coordination.
mod ui_sink;

use hotki_protocol::{NotifyKind, ipc::heartbeat, rpc::InjectKind};
use hotki_server::{Client, Result as ServerResult};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{Duration, Instant as TokioInstant, Sleep, sleep},
};
use tracing::{debug, error, info, warn};
use ui_sink::UiSink;

use crate::{
    health::{ConnectionStatus, InputProjection, RetryState, RuntimeHealth, RuntimePhase},
    logs,
    permissions::{PermissionObservation, PermissionsStatus, check_permissions},
    runtime::ControlMsg,
    ui_delivery::UiDeliveryTx,
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
    /// Whether the auto-spawned server should observe physical keyboard events.
    server_event_tap_enabled: bool,
    /// Whether to log periodic world snapshots.
    dumpworld: bool,
    /// Complete app/runtime health published atomically to every UI surface.
    health: RuntimeHealth,
    /// Deterministic permission state supplied by devtools, when active.
    permission_override: Option<PermissionsStatus>,
    /// Server-bound controls received before a connection is ready.
    pending_controls: VecDeque<ServerControl>,
    /// Whether this app session already warned for the current active observation run.
    secure_input_warning_sent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Result of processing a control message.
enum ControlOutcome {
    /// Continue connecting or driving events.
    Continue,
    /// Retry the server connection.
    RetryConnect,
    /// Pause an in-flight connection until required permissions are granted.
    PauseConnect,
    /// Stop the current driver loop.
    Stop,
}

impl ControlOutcome {
    /// Whether the caller should stop the active loop.
    fn should_stop(self) -> bool {
        matches!(self, Self::Stop)
    }

    /// Whether the caller should retry the server connection.
    fn should_retry(self) -> bool {
        matches!(self, Self::RetryConnect)
    }

    /// Whether an in-flight connect task must be canceled.
    fn should_pause_connect(self) -> bool {
        matches!(self, Self::PauseConnect)
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
    /// Current permission status observed by the UI.
    PermissionsChanged(PermissionObservation),
}

/// Server-bound control command.
#[derive(Debug, PartialEq, Eq)]
enum ServerControl {
    /// Reload from disk using the configured config path.
    Reload,
    /// Inject one complete key press through the connected server.
    InjectKey {
        /// Key chord identifier, for example `shift+cmd+0`.
        ident: String,
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
            ControlMsg::InjectKey {
                ident,
                report_errors,
            } => Self::Server(ServerControl::InjectKey {
                ident,
                report_errors,
            }),
            ControlMsg::OpenPermissionsHelp => Self::Local(LocalControl::OpenPermissionsHelp),
            ControlMsg::Notice { kind, title, text } => {
                Self::Local(LocalControl::Notice { kind, title, text })
            }
            ControlMsg::PermissionsChanged(status) => {
                Self::Local(LocalControl::PermissionsChanged(status))
            }
        }
    }
}

impl ConnectionDriver {
    /// Construct a new driver with channels and initial config.
    pub(crate) fn new(
        config_path: PathBuf,
        server_log_filter: Option<String>,
        tx_keys: UiDeliveryTx,
        egui_ctx: egui::Context,
        rx_ctrl: mpsc::UnboundedReceiver<ControlMsg>,
        server_event_tap_enabled: bool,
        dumpworld: bool,
    ) -> Self {
        let permissions = check_permissions();
        let mut health = RuntimeHealth::connecting(config_path.clone());
        health.permissions = permissions;
        let driver = Self {
            config_path,
            server_log_filter,
            ui: UiSink::new(tx_keys, egui_ctx),
            rx_ctrl,
            server_event_tap_enabled,
            dumpworld,
            health,
            permission_override: None,
            pending_controls: VecDeque::new(),
            secure_input_warning_sent: false,
        };
        driver.ui.set_runtime_health(driver.health.clone());
        driver
    }

    /// Publish the complete health snapshot after a state transition.
    fn publish_health(&self) {
        self.ui.set_runtime_health(self.health.clone());
    }

    /// Record a connection failure that can be retried.
    fn mark_disconnected(&mut self, message: impl Into<String>) {
        self.health.phase = RuntimePhase::Disconnected;
        self.health.connection = ConnectionStatus::Disconnected;
        self.health.retry = RetryState::Available;
        self.health.message = Some(message.into());
        self.ui.set_server_bindings(Vec::new());
        self.publish_health();
    }

    /// Begin the first connection or a reconnect attempt.
    fn mark_connecting(&mut self) {
        let retrying = self.health.phase == RuntimePhase::Retrying
            || self.health.retry == RetryState::Available
            || self.health.message.is_some();
        self.health.phase = if retrying {
            RuntimePhase::Retrying
        } else {
            RuntimePhase::Connecting
        };
        self.health.connection = ConnectionStatus::Connecting;
        self.health.pending_config = Some(self.config_path.clone());
        self.health.retry = RetryState::InProgress;
        self.health.message = None;
        self.publish_health();
    }

    /// Record that required macOS permissions are still missing.
    fn mark_waiting_for_permissions(&mut self) {
        self.health.phase = RuntimePhase::WaitingPermissions;
        if !self.health.server_connected() {
            self.health.connection = ConnectionStatus::Disconnected;
        }
        self.health.retry = RetryState::Available;
        self.health.message =
            Some("Grant Accessibility and Input Monitoring to continue.".to_string());
        self.publish_health();
    }

    /// Record an acknowledged active config and ready server.
    fn mark_ready(&mut self) {
        self.health.phase = RuntimePhase::Ready;
        self.health.connection = ConnectionStatus::Connected;
        self.health.active_config = Some(self.config_path.clone());
        self.health.pending_config = None;
        self.health.retry = RetryState::Idle;
        self.health.message = None;
        self.publish_health();
    }

    /// Record a rejected config while keeping any prior active config explicit.
    fn mark_invalid_config(&mut self, message: impl Into<String>) {
        self.health.phase = RuntimePhase::InvalidConfig;
        self.health.connection = ConnectionStatus::Connected;
        self.health.pending_config = Some(self.config_path.clone());
        self.health.retry = RetryState::Available;
        self.health.message = Some(message.into());
        self.publish_health();
    }

    /// Handle control messages that do not require a connected server.
    fn handle_local_control(&mut self, control: LocalControl) -> ControlOutcome {
        match control {
            LocalControl::OpenPermissionsHelp => {
                self.ui.show_permissions_help();
                ControlOutcome::Continue
            }
            LocalControl::Notice { kind, title, text } => {
                self.notify_local(kind, &title, &text);
                ControlOutcome::Continue
            }
            LocalControl::PermissionsChanged(observation) => {
                let status = observation.status();
                self.permission_override = observation.is_overridden().then_some(status);
                self.health.permissions = status;
                if !Self::observed_permissions_granted(status) {
                    if self.health.server_connected()
                        && self.health.phase == RuntimePhase::InvalidConfig
                    {
                        self.publish_health();
                    } else {
                        self.mark_waiting_for_permissions();
                    }
                    if self.server_event_tap_enabled && !self.health.server_connected() {
                        ControlOutcome::PauseConnect
                    } else {
                        ControlOutcome::Continue
                    }
                } else if self.health.server_connected()
                    && self.health.phase == RuntimePhase::WaitingPermissions
                {
                    self.mark_ready();
                    ControlOutcome::Continue
                } else if matches!(
                    self.health.phase,
                    RuntimePhase::Disconnected
                        | RuntimePhase::Connecting
                        | RuntimePhase::WaitingPermissions
                ) {
                    self.health.phase = RuntimePhase::Retrying;
                    self.health.retry = RetryState::InProgress;
                    self.health.message = None;
                    self.publish_health();
                    ControlOutcome::RetryConnect
                } else {
                    self.publish_health();
                    ControlOutcome::Continue
                }
            }
        }
    }

    /// Handle control messages that require an active server connection.
    async fn handle_server_control(
        &mut self,
        conn: &mut hotki_server::Connection,
        control: ServerControl,
    ) {
        match control {
            ServerControl::Reload => {
                if !self.event_tap_permissions_missing() {
                    self.reload_config(conn).await;
                }
            }
            ServerControl::InjectKey {
                ident,
                report_errors,
            } => {
                if self
                    .inject_key(conn, &ident, InjectKind::Down, false, report_errors)
                    .await
                {
                    self.inject_key(conn, &ident, InjectKind::Up, false, false)
                        .await;
                }
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
            ControlRoute::Local(control) => self.handle_local_control(control),
            ControlRoute::Server(control) => {
                if let Some(conn) = conn {
                    self.handle_server_control(conn, control).await;
                    ControlOutcome::Continue
                } else {
                    self.pending_controls.push_back(control);
                    self.health.phase = RuntimePhase::Retrying;
                    self.health.pending_config = Some(self.config_path.clone());
                    self.health.retry = RetryState::InProgress;
                    self.health.message = None;
                    self.publish_health();
                    ControlOutcome::RetryConnect
                }
            }
            ControlRoute::Shutdown => {
                self.begin_shutdown();
                if let Some(conn) = conn {
                    match conn.shutdown().await {
                        Ok(()) => info!("server acknowledged shutdown"),
                        Err(err) => warn!("server shutdown was not acknowledged: {err}"),
                    }
                }
                ControlOutcome::Stop
            }
        }
    }

    /// Record shutdown before waiting for owned runtime work to finish.
    fn begin_shutdown(&mut self) {
        self.health.phase = RuntimePhase::ShuttingDown;
        self.health.connection = if self.health.server_connected() {
            ConnectionStatus::Closing
        } else {
            ConnectionStatus::Disconnected
        };
        self.health.retry = RetryState::Idle;
        self.health.message = None;
        self.publish_health();
    }

    /// Close the UI after server and connection work has finished.
    fn finish_shutdown(&self) {
        self.ui.finish_shutdown();
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

    /// Whether the current permission snapshot allows the server event tap to start.
    fn required_permissions_granted(&self, perms: PermissionsStatus) -> bool {
        !self.server_event_tap_enabled || Self::observed_permissions_granted(perms)
    }

    /// Whether the latest observation contains both permissions used by hotkeys.
    fn observed_permissions_granted(perms: PermissionsStatus) -> bool {
        perms.accessibility_ok() && perms.input_ok()
    }

    /// Show permission guidance and keep the server from being spawned without required grants.
    fn event_tap_permissions_missing(&mut self) -> bool {
        let perms = self.permission_override.unwrap_or_else(check_permissions);
        if self.health.permissions != perms {
            self.health.permissions = perms;
            self.publish_health();
        }
        if self.required_permissions_granted(perms) {
            return false;
        }

        self.mark_waiting_for_permissions();
        self.ui.show_permissions_help();
        self.notify_local(
            NotifyKind::Error,
            "Permissions",
            "Grant Accessibility and Input Monitoring to Hotki. It will retry automatically.",
        );
        true
    }

    /// Reload the current config path on the server and notify the UI.
    async fn reload_config(&mut self, conn: &mut hotki_server::Connection) {
        self.health.phase = RuntimePhase::Retrying;
        self.health.connection = ConnectionStatus::Connected;
        self.health.pending_config = Some(self.config_path.clone());
        self.health.retry = RetryState::InProgress;
        self.health.message = None;
        self.publish_health();

        match self.activate_config(conn).await {
            Ok(()) => {
                self.notify_local(
                    NotifyKind::Success,
                    "Config reloaded",
                    "The new configuration is active.",
                );
                self.refresh_server_bindings(conn).await;
            }
            Err(err) => {
                self.notify_local(NotifyKind::Error, "Config", &err.to_string());
            }
        }
    }

    /// Ask the server to activate the configured path and publish the exact outcome.
    async fn activate_config(
        &mut self,
        conn: &mut hotki_server::Connection,
    ) -> hotki_server::Result<()> {
        match conn
            .set_config_path(self.config_path.to_string_lossy().as_ref())
            .await
        {
            Ok(()) => {
                self.mark_ready();
                Ok(())
            }
            Err(err) => {
                let message = err.to_string();
                error!(error = %err, "server rejected config candidate");
                self.mark_invalid_config(message);
                Err(err)
            }
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
    ) -> bool {
        let result = match (kind, repeat) {
            (InjectKind::Down, true) => conn.inject_key_repeat(ident).await,
            (InjectKind::Down, false) => conn.inject_key_down(ident).await,
            (InjectKind::Up, _) => conn.inject_key_up(ident).await,
        };
        match result {
            Ok(()) => true,
            Err(err) => {
                if report_errors {
                    self.notify_local(
                        NotifyKind::Error,
                        "Devtools",
                        &format!("Failed to inject {ident}: {err}"),
                    );
                }
                false
            }
        }
    }

    /// Process a message from the server and update the UI accordingly.
    async fn handle_server_msg(&mut self, msg: hotki_protocol::MsgToUI) {
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
            hotki_protocol::MsgToUI::ShowMainWindow(arg) => {
                self.ui
                    .send_message(hotki_protocol::MsgToUI::ShowMainWindow(arg));
            }
            message @ hotki_protocol::MsgToUI::HudKeyState { .. } => {
                self.ui.send_message(message);
            }
            hotki_protocol::MsgToUI::Log {
                level,
                target,
                message,
            } => {
                logs::push_server(level, target, message);
                self.ui.request_repaint();
            }
            hotki_protocol::MsgToUI::Heartbeat(heartbeat) => {
                self.handle_input_health(&heartbeat.input);
            }
            hotki_protocol::MsgToUI::World(msg) => {
                if self.dumpworld {
                    debug!("World event: {:?}", msg);
                }
            }
        }
    }

    /// Publish full diagnostics and transition-stable presentation from one heartbeat.
    fn handle_input_health(&mut self, input: &hotki_protocol::InputHealth) {
        let projection = InputProjection::from(input);
        self.ui.set_input_health(input.clone());
        if self.health.input != projection {
            self.health.input = projection;
            self.publish_health();
        }

        if input.secure_input == hotki_protocol::SecureInputState::Inactive {
            if self.secure_input_warning_sent {
                self.notify_local(
                    NotifyKind::Success,
                    "Secure Input ended",
                    "Physical hotkeys have resumed.",
                );
                self.secure_input_warning_sent = false;
            }
        } else if input.blocked && !self.secure_input_warning_sent {
            self.notify_local(
                NotifyKind::Warn,
                "Hotkeys paused by Secure Input",
                "Physical hotkeys resume automatically when Secure Input ends.",
            );
            self.secure_input_warning_sent = true;
        }
    }

    /// Connect to the server, draining any queued control messages after connect.
    pub(crate) async fn connect(&mut self) -> Option<Client> {
        if self.event_tap_permissions_missing() {
            return None;
        }

        self.mark_connecting();
        let mut connect_task = spawn_connect(
            self.server_log_filter.clone(),
            self.server_event_tap_enabled,
        );
        let mut client = self.wait_for_connected_client(&mut connect_task).await?;

        if self.event_tap_permissions_missing() {
            return None;
        }

        let conn = match client.connection() {
            Ok(conn) => conn,
            Err(err) => {
                error!("Failed to get client connection: {}", err);
                self.mark_disconnected(format!("Failed to open server connection: {err}"));
                return None;
            }
        };
        if let Err(err) = self.activate_config(conn).await {
            self.notify_local(NotifyKind::Error, "Config", &err.to_string());
        } else {
            self.refresh_server_bindings(conn).await;
            info!("Config path sent to server engine");
        }

        self.drain_pending_controls(conn).await;

        Some(client)
    }

    /// Run the connection lifecycle until shutdown.
    pub(crate) async fn run(&mut self) {
        loop {
            if self.health.phase == RuntimePhase::ShuttingDown {
                break;
            }

            if let Some(mut client) = self.connect().await {
                self.drive_events(&mut client).await;
                continue;
            }

            if self.health.phase == RuntimePhase::ShuttingDown {
                break;
            }

            if !self.wait_for_retry_signal().await {
                break;
            }
        }

        if self.health.phase == RuntimePhase::ShuttingDown {
            self.finish_shutdown();
        }
    }

    /// Wait for a local control, permission change, or queued server command that should retry.
    async fn wait_for_retry_signal(&mut self) -> bool {
        while let Some(msg) = self.rx_ctrl.recv().await {
            let outcome = self.route_control_msg(None, msg).await;
            if outcome.should_stop() {
                return false;
            }
            if outcome.should_retry() {
                return true;
            }
        }
        false
    }

    /// Wait for the background connect task while still accepting local controls.
    async fn wait_for_connected_client(
        &mut self,
        connect_task: &mut ConnectTask,
    ) -> Option<Client> {
        loop {
            tokio::select! {
                biased;
                res = &mut connect_task.result => {
                    connect_task.join().await;
                    return self.handle_connect_result(res);
                }
                Some(msg) = self.rx_ctrl.recv() => {
                    let outcome = self.route_control_msg(None, msg).await;
                    if outcome.should_stop() || outcome.should_pause_connect() {
                        connect_task.cancel().await;
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
                self.mark_disconnected(format!("Failed to start hotkey server: {err}"));
                None
            }
            Err(_) => {
                error!("Connect task canceled before reporting a result");
                if self.health.phase != RuntimePhase::ShuttingDown {
                    self.mark_disconnected("Server connection attempt was canceled");
                }
                None
            }
        }
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
                self.mark_disconnected(format!("Failed to open server connection: {err}"));
                return;
            }
        };

        let hb_timer: Sleep = sleep(heartbeat::timeout());
        tokio::pin!(hb_timer);

        let dump_interval = Duration::from_secs(5);
        let dump_timer: Sleep = sleep(dump_interval);
        tokio::pin!(dump_timer);

        loop {
            if !self
                .drive_event_once(conn, &mut hb_timer, &mut dump_timer, dump_interval)
                .await
            {
                break;
            }
        }
        info!("Exiting key loop");
        if self.health.phase != RuntimePhase::ShuttingDown {
            self.mark_disconnected("Server connection lost; reconnecting");
        }
    }

    /// Drive one select iteration for the connected event loop.
    async fn drive_event_once(
        &mut self,
        conn: &mut hotki_server::Connection,
        hb_timer: &mut Pin<&mut Sleep>,
        dump_timer: &mut Pin<&mut Sleep>,
        dump_interval: Duration,
    ) -> bool {
        let dumpworld = self.dumpworld;
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
            _ = dump_timer.as_mut(), if dumpworld => {
                self.dump_world_snapshot(conn).await;
                dump_timer.as_mut().reset(TokioInstant::now() + dump_interval);
                true
            }
        }
    }

    /// Handle one server event receive result.
    async fn handle_recv_event_result(
        &mut self,
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

    /// Log a world snapshot for dumpworld diagnostics.
    async fn dump_world_snapshot(&self, conn: &mut hotki_server::Connection) {
        let Ok(snap) = conn.get_world_snapshot().await else {
            return;
        };

        let mut out = String::new();
        let focused_ctx = snap
            .focused
            .as_ref()
            .map(|focused| format!("{} (pid={}) — {}", focused.app, focused.pid, focused.title))
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
}

/// Owned asynchronous server-connect attempt.
struct ConnectTask {
    /// Delivers the connection attempt's typed result.
    result: oneshot::Receiver<ServerResult<Client>>,
    /// Owned task that must finish or be canceled before shutdown proceeds.
    handle: JoinHandle<()>,
}

impl ConnectTask {
    /// Wait for the task to finish after its result becomes available.
    async fn join(&mut self) {
        if let Err(err) = (&mut self.handle).await
            && !err.is_cancelled()
        {
            warn!("server connect task failed: {err}");
        }
    }

    /// Cancel an in-flight connect and wait until it has stopped.
    async fn cancel(&mut self) {
        self.handle.abort();
        self.join().await;
    }
}

/// Spawn an owned background connect task.
fn spawn_connect(log_filter: Option<String>, server_event_tap_enabled: bool) -> ConnectTask {
    let (tx_conn_ready, rx) = oneshot::channel::<ServerResult<Client>>();
    let handle = tokio::spawn(async move {
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
    ConnectTask { result: rx, handle }
}

#[cfg(test)]
mod tests {
    use std::{future, iter};

    use hotki_protocol::{
        InputHealth, NotifyKind, SecureInputOwner, SecureInputState, TapLifecycle, TapMode,
    };
    use permissions::PermissionState;
    use tokio::sync::{mpsc::unbounded_channel, oneshot};

    use super::{
        ConnectTask, ConnectionDriver, ControlOutcome, ControlRoute, LocalControl, ServerControl,
    };
    use crate::{
        app::{UiCommand, UiEvent},
        health::{ConnectionStatus, RetryState, RuntimePhase},
        permissions::{PermissionObservation, PermissionsStatus},
        runtime::ControlMsg,
        ui_delivery::{UiDeliveryRx, ui_delivery_channel},
    };

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
            ControlRoute::from_msg(ControlMsg::InjectKey {
                ident: "shift+cmd+0".to_string(),
                report_errors: true,
            }),
            ControlRoute::Server(ServerControl::InjectKey {
                ident: "shift+cmd+0".to_string(),
                report_errors: true,
            })
        );
        assert_eq!(
            ControlRoute::from_msg(ControlMsg::Shutdown),
            ControlRoute::Shutdown
        );
        let status = PermissionsStatus {
            accessibility: PermissionState::Granted,
            input_monitoring: PermissionState::Granted,
            screen_recording: PermissionState::Denied,
        };
        assert_eq!(
            ControlRoute::from_msg(ControlMsg::PermissionsChanged(
                PermissionObservation::system(status),
            )),
            ControlRoute::Local(LocalControl::PermissionsChanged(
                PermissionObservation::system(status),
            ))
        );
    }

    #[test]
    fn control_outcome_names_loop_exit_decision() {
        assert!(!ControlOutcome::Continue.should_stop());
        assert!(!ControlOutcome::RetryConnect.should_stop());
        assert!(!ControlOutcome::PauseConnect.should_stop());
        assert!(ControlOutcome::Stop.should_stop());
        assert!(!ControlOutcome::Continue.should_retry());
        assert!(ControlOutcome::RetryConnect.should_retry());
        assert!(!ControlOutcome::PauseConnect.should_retry());
        assert!(!ControlOutcome::Stop.should_retry());
        assert!(!ControlOutcome::Continue.should_pause_connect());
        assert!(ControlOutcome::PauseConnect.should_pause_connect());
    }

    #[test]
    fn permission_status_retries_when_disconnected_and_ready() {
        let (tx_ui, rx_ui) = ui_delivery_channel();
        let (_tx_ctrl, rx_ctrl) = unbounded_channel();
        let ctx = egui::Context::default();
        let mut driver =
            ConnectionDriver::new("config.luau".into(), None, tx_ui, ctx, rx_ctrl, true, false);
        drop(rx_ui);

        let ready = PermissionsStatus {
            accessibility: PermissionState::Granted,
            input_monitoring: PermissionState::Granted,
            screen_recording: PermissionState::Denied,
        };
        assert_eq!(
            driver.handle_local_control(LocalControl::PermissionsChanged(
                PermissionObservation::system(ready),
            )),
            ControlOutcome::RetryConnect
        );

        driver.health.phase = RuntimePhase::Ready;
        driver.health.connection = ConnectionStatus::Connected;
        driver.health.retry = RetryState::Idle;
        assert_eq!(
            driver.handle_local_control(LocalControl::PermissionsChanged(
                PermissionObservation::system(ready),
            )),
            ControlOutcome::Continue
        );
    }

    #[test]
    fn permission_loss_pauses_connect_and_fixture_override_has_explicit_lifetime() {
        let (tx_ui, _rx_ui) = ui_delivery_channel();
        let (_tx_ctrl, rx_ctrl) = unbounded_channel();
        let ctx = egui::Context::default();
        let mut driver =
            ConnectionDriver::new("config.luau".into(), None, tx_ui, ctx, rx_ctrl, true, false);
        let denied = PermissionsStatus {
            accessibility: PermissionState::Denied,
            input_monitoring: PermissionState::Denied,
            screen_recording: PermissionState::Denied,
        };
        let ready = PermissionsStatus {
            accessibility: PermissionState::Granted,
            input_monitoring: PermissionState::Granted,
            screen_recording: PermissionState::Denied,
        };
        driver.health.phase = RuntimePhase::Connecting;
        driver.health.connection = ConnectionStatus::Connecting;

        assert_eq!(
            driver.handle_local_control(LocalControl::PermissionsChanged(
                PermissionObservation::devtools(denied),
            )),
            ControlOutcome::PauseConnect
        );
        assert_eq!(driver.permission_override, Some(denied));
        assert_eq!(driver.health.phase, RuntimePhase::WaitingPermissions);

        assert_eq!(
            driver.handle_local_control(LocalControl::PermissionsChanged(
                PermissionObservation::system(ready),
            )),
            ControlOutcome::RetryConnect
        );
        assert_eq!(driver.permission_override, None);
    }

    #[tokio::test]
    async fn server_control_without_connection_is_queued_retry() {
        let (tx_ui, _rx_ui) = ui_delivery_channel();
        let (_tx_ctrl, rx_ctrl) = unbounded_channel();
        let ctx = egui::Context::default();
        let mut driver = ConnectionDriver::new(
            "config.luau".into(),
            None,
            tx_ui,
            ctx,
            rx_ctrl,
            false,
            false,
        );

        let outcome = driver.route_control_msg(None, ControlMsg::Reload).await;

        assert_eq!(outcome, ControlOutcome::RetryConnect);
        assert_eq!(driver.pending_controls.len(), 1);
    }

    #[tokio::test]
    async fn shutdown_closes_ui_only_after_driver_finishes() {
        let (tx_ui, rx_ui) = ui_delivery_channel();
        let (_tx_ctrl, rx_ctrl) = unbounded_channel();
        let ctx = egui::Context::default();
        let mut driver = ConnectionDriver::new(
            "config.luau".into(),
            None,
            tx_ui,
            ctx,
            rx_ctrl,
            false,
            false,
        );

        let outcome = driver.route_control_msg(None, ControlMsg::Shutdown).await;

        assert_eq!(outcome, ControlOutcome::Stop);
        assert_eq!(driver.health.phase, RuntimePhase::ShuttingDown);
        assert!(!drain_for_shutdown(&rx_ui));

        driver.finish_shutdown();

        assert!(drain_for_shutdown(&rx_ui));
    }

    #[test]
    fn invalid_candidate_preserves_active_config_and_exposes_retry() {
        let (tx_ui, _rx_ui) = ui_delivery_channel();
        let (_tx_ctrl, rx_ctrl) = unbounded_channel();
        let ctx = egui::Context::default();
        let mut driver = ConnectionDriver::new(
            "candidate.luau".into(),
            None,
            tx_ui,
            ctx,
            rx_ctrl,
            false,
            false,
        );
        driver.health.active_config = Some("active.luau".into());

        driver.mark_invalid_config("candidate rejected");

        assert_eq!(driver.health.phase, RuntimePhase::InvalidConfig);
        assert_eq!(driver.health.connection, ConnectionStatus::Connected);
        assert_eq!(driver.health.active_config, Some("active.luau".into()));
        assert_eq!(driver.health.pending_config, Some("candidate.luau".into()));
        assert_eq!(driver.health.retry, RetryState::Available);
        assert_eq!(driver.health.message.as_deref(), Some("candidate rejected"));
    }

    #[tokio::test]
    async fn cancel_connect_task_waits_for_owned_task() {
        let (result_tx, result) = oneshot::channel();
        let (started_tx, started_rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            let _result_tx = result_tx;
            started_tx.send(()).expect("signal task start");
            future::pending::<()>().await;
        });
        started_rx.await.expect("connect task started");
        let mut task = ConnectTask { result, handle };

        task.cancel().await;

        assert!(task.handle.is_finished());
    }

    fn drain_for_shutdown(rx: &UiDeliveryRx) -> bool {
        let mut found = false;
        while let Some(event) = rx.try_recv() {
            found |= matches!(event, UiEvent::Command(UiCommand::Shutdown));
        }
        found
    }

    fn input_health(state: SecureInputState, blocked: bool, count: u64) -> InputHealth {
        InputHealth {
            tap_mode: TapMode::Physical,
            tap_lifecycle: TapLifecycle::Running,
            secure_input: state,
            secure_input_owner: (state == SecureInputState::Active).then(|| SecureInputOwner {
                pid: 42,
                app_name: "Terminal".to_string(),
            }),
            blocked,
            registered_hotkeys: usize::from(blocked),
            physical_event_count: count,
            server_pid: 7,
            ..InputHealth::default()
        }
    }

    #[test]
    fn secure_input_warning_latches_until_inactive_observation() {
        let (tx_ui, rx_ui) = ui_delivery_channel();
        let (_tx_ctrl, rx_ctrl) = unbounded_channel();
        let mut driver = ConnectionDriver::new(
            "config.luau".into(),
            None,
            tx_ui,
            egui::Context::default(),
            rx_ctrl,
            false,
            false,
        );
        while rx_ui.try_recv().is_some() {}

        driver.handle_input_health(&input_health(SecureInputState::Active, true, 1));
        driver.mark_disconnected("test reconnect");
        driver.handle_input_health(&input_health(SecureInputState::Active, true, 2));
        driver.handle_input_health(&input_health(SecureInputState::Active, false, 3));
        driver.handle_input_health(&input_health(SecureInputState::Inactive, false, 4));

        let mut warning_count = 0;
        let mut recovery_count = 0;
        while let Some(event) = rx_ui.try_recv() {
            if let UiEvent::Message(hotki_protocol::MsgToUI::Notify { title, .. }) = event {
                warning_count += usize::from(title == "Hotkeys paused by Secure Input");
                recovery_count += usize::from(title == "Secure Input ended");
            }
        }
        assert_eq!(warning_count, 1);
        assert_eq!(recovery_count, 1);
        assert!(!driver.secure_input_warning_sent);
    }

    #[test]
    fn volatile_input_counters_do_not_republish_runtime_health() {
        let (tx_ui, rx_ui) = ui_delivery_channel();
        let (_tx_ctrl, rx_ctrl) = unbounded_channel();
        let mut driver = ConnectionDriver::new(
            "config.luau".into(),
            None,
            tx_ui,
            egui::Context::default(),
            rx_ctrl,
            false,
            false,
        );
        while rx_ui.try_recv().is_some() {}

        driver.handle_input_health(&input_health(SecureInputState::Inactive, false, 1));
        while rx_ui.try_recv().is_some() {}
        driver.handle_input_health(&input_health(SecureInputState::Inactive, false, 2));

        let events = iter::from_fn(|| rx_ui.try_recv()).collect::<Vec<_>>();
        assert!(events.iter().any(|event| matches!(
            event,
            UiEvent::Command(UiCommand::SetInputHealth(input))
                if input.physical_event_count == 2
        )));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, UiEvent::Command(UiCommand::SetRuntimeHealth(_))))
        );
    }
}
