use std::{
    collections::VecDeque,
    fmt::Write as _,
    path::{Path, PathBuf},
    process::{self},
};

use config::themes;
use egui::Context;
use hotki_protocol::{NotifyKind, WorldStreamMsg, ipc::heartbeat, rpc::InjectKind};
use hotki_server::{
    Client,
    smoketest_bridge::{
        BridgeEvent, BridgeHudKey, BridgeNotifications, BridgeRequest, BridgeResponse,
        control_socket_path, drain_bridge_events, handshake_response,
    },
};
use mac_keycode::Chord;
use tokio::{
    sync::{broadcast, mpsc, oneshot},
    time::{Duration, Instant as TokioInstant, Sleep, sleep},
};
use tracing::{debug, error, info};

use crate::{
    app::AppEvent,
    control::{ControlMsg, TestCommand},
    logs,
    permissions::check_permissions,
    settings::{apply_ui_config, reload_and_broadcast},
    smoketest_bridge::init_test_bridge,
};

/// Actions that adjust UI overrides on the current cursor (theme and user style).
#[derive(Debug, Clone)]
enum UiOverride {
    /// Cycle to the next theme in order.
    ThemeNext,
    /// Cycle to the previous theme in order.
    ThemePrev,
    /// Set the theme override to an explicit name.
    ThemeSet(String),
    /// Enable, disable, or toggle the user-style UI override.
    UserStyle(config::Toggle),
}

/// Drives the MRPC connection for the UI: connect, process events, and apply config/overrides.
pub struct ConnectionDriver {
    /// Path to the on-disk Hotki config.
    config_path: PathBuf,
    /// Optional log filter for any auto-spawned server process.
    server_log_filter: Option<String>,
    /// Sender for UI events to the app thread.
    tx_keys: mpsc::UnboundedSender<AppEvent>,
    /// Egui context used for repaint and viewport commands.
    egui_ctx: Context,
    /// Receiver of control messages from tray/UI.
    rx_ctrl: mpsc::UnboundedReceiver<ControlMsg>,
    /// Sender for control messages back into the runtime.
    tx_ctrl_runtime: mpsc::UnboundedSender<ControlMsg>,

    /// Last loaded UI configuration.
    ui_config: config::Config,
    /// Current focus and UI override cursor.
    current_cursor: config::Cursor,
    /// Whether to log periodic world snapshots.
    dumpworld: bool,
    /// Pending smoketest bridge socket to initialize after connect.
    test_bridge_path: Option<PathBuf>,
    /// Broadcast channel for smoketest bridge events.
    bridge_events: broadcast::Sender<BridgeEvent>,
    /// Recent notifications buffered for smoketest snapshots.
    bridge_notifications: BridgeNotifications,
}

impl ConnectionDriver {
    /// Max number of notifications retained for smoketest snapshots.
    const MAX_BRIDGE_NOTIFICATIONS: usize = 32;
    /// Max number of bridge events to drain on shutdown.
    const MAX_SHUTDOWN_DRAIN_EVENTS: usize = 128;

    /// Send a smoketest bridge event if listeners are present.
    fn emit_bridge_event(&self, event: BridgeEvent) {
        self.bridge_events.send(event).ok();
    }

    /// Build a bridge handshake response from current server status and notifications.
    async fn make_handshake_response(
        &self,
        conn: &mut hotki_server::Connection,
    ) -> Result<BridgeResponse, String> {
        let status = conn
            .get_server_status()
            .await
            .map_err(|err| err.to_string())?;
        let notifications = self.bridge_notifications.snapshot();
        Ok(handshake_response(&status, notifications))
    }

    /// Construct a new driver with channels and initial config.
    pub(crate) fn new(
        config_path: PathBuf,
        server_log_filter: Option<String>,
        tx_keys: mpsc::UnboundedSender<AppEvent>,
        egui_ctx: Context,
        rx_ctrl: mpsc::UnboundedReceiver<ControlMsg>,
        tx_ctrl_runtime: mpsc::UnboundedSender<ControlMsg>,
        dumpworld: bool,
    ) -> Self {
        let ui_config = match config::load_from_path(&config_path) {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to load UI config: {}", e.pretty());
                config::Config::default()
            }
        };
        let server_socket = hotki_server::socket_path_for_pid(process::id());
        let test_bridge_path = Some(PathBuf::from(control_socket_path(&server_socket)));
        let (bridge_events, _rx) = broadcast::channel(128);

        Self {
            config_path,
            server_log_filter,
            tx_keys,
            egui_ctx,
            rx_ctrl,
            tx_ctrl_runtime,
            ui_config,
            current_cursor: config::Cursor::default(),
            dumpworld,
            test_bridge_path,
            bridge_events,
            bridge_notifications: BridgeNotifications::new(Self::MAX_BRIDGE_NOTIFICATIONS),
        }
    }

    /// Ensure the smoketest bridge listener is running, retrying later on error.
    async fn ensure_test_bridge(&mut self) {
        if let Some(path) = self.test_bridge_path.take()
            && let Err(err) = init_test_bridge(
                path.clone(),
                self.tx_ctrl_runtime.clone(),
                self.bridge_events.clone(),
            )
            .await
        {
            tracing::warn!(?err, socket = %path.display(), "failed to initialize smoketest bridge");
            self.test_bridge_path = Some(path);
        }
    }

    /// Request a graceful UI shutdown, falling back to hard exit after `fallback_ms`.
    fn trigger_graceful_shutdown(&self, fallback_ms: u64) {
        if self.tx_keys.send(AppEvent::Shutdown).is_err() {
            tracing::warn!("failed to send Shutdown to app channel");
        }
        self.egui_ctx
            .send_viewport_cmd(egui::ViewportCommand::Close);
        self.egui_ctx.request_repaint();
        tokio::spawn(async move {
            sleep(Duration::from_millis(fallback_ms)).await;
            process::exit(0);
        });
    }

    /// Handle control messages that do not require a connected server.
    fn handle_local_control(&mut self, msg: ControlMsg) {
        match msg {
            ControlMsg::OpenPermissionsHelp => {
                if self.tx_keys.send(AppEvent::ShowPermissionsHelp).is_err() {
                    tracing::warn!("failed to send permissions help event");
                }
                self.egui_ctx.request_repaint();
            }
            ControlMsg::Notice { kind, title, text } => {
                self.notify(kind, &title, &text);
            }
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
                self.trigger_graceful_shutdown(750);
                conn.shutdown().await.ok();
                true
            }
            ControlMsg::Reload => {
                reload_and_broadcast(
                    conn,
                    &mut self.ui_config,
                    &self.config_path,
                    &self.tx_keys,
                    &self.egui_ctx,
                )
                .await;
                false
            }
            ControlMsg::SwitchTheme(name) => {
                self.apply_ui_override(UiOverride::ThemeSet(name));
                false
            }
            ControlMsg::Test(cmd) => {
                let TestCommand {
                    command_id: _command_id,
                    req,
                    respond_to,
                } = cmd;
                let response = self.handle_test_command(conn, req).await;
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
                self.trigger_graceful_shutdown(750);
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
        self.bridge_notifications.record(kind, title, text);
        self.tx_keys
            .send(AppEvent::Notify {
                kind,
                title: title.to_string(),
                text: text.to_string(),
            })
            .ok();
        self.egui_ctx.request_repaint();
    }

    /// Push an updated cursor to the UI and request repaint.
    fn push_cursor_update(&self, context: &str) {
        if self
            .tx_keys
            .send(AppEvent::UpdateCursor(self.current_cursor.clone()))
            .is_err()
        {
            tracing::warn!("failed to send UpdateCursor ({})", context);
        }
        self.egui_ctx.request_repaint();
    }

    /// Apply a theme/user-style override to the current cursor.
    fn apply_ui_override(&mut self, action: UiOverride) {
        let mut update_reason: Option<&str> = None;
        match action {
            UiOverride::ThemeNext => {
                let cur = self
                    .current_cursor
                    .override_theme
                    .as_deref()
                    .unwrap_or("default");
                let next = themes::get_next_theme(cur);
                self.current_cursor.set_theme(Some(next));
                update_reason = Some("next theme");
            }
            UiOverride::ThemePrev => {
                let cur = self
                    .current_cursor
                    .override_theme
                    .as_deref()
                    .unwrap_or("default");
                let prev = themes::get_prev_theme(cur);
                self.current_cursor.set_theme(Some(prev));
                update_reason = Some("prev theme");
            }
            UiOverride::ThemeSet(name) => {
                if themes::theme_exists(&name) {
                    self.current_cursor.set_theme(Some(&name));
                    update_reason = Some("set theme");
                } else {
                    self.notify(NotifyKind::Error, "Theme", "Theme not found");
                }
            }
            UiOverride::UserStyle(tg) => {
                match tg {
                    config::Toggle::On => self.current_cursor.set_user_style_enabled(true),
                    config::Toggle::Off => self.current_cursor.set_user_style_enabled(false),
                    config::Toggle::Toggle => {
                        let currently_enabled = !self.current_cursor.user_ui_disabled;
                        self.current_cursor
                            .set_user_style_enabled(!currently_enabled);
                    }
                }
                update_reason = Some("user style toggle");
            }
        }

        if let Some(reason) = update_reason {
            debug!(%reason, "apply_ui_override");
            self.push_cursor_update(reason);
        }
    }

    /// Handle a smoketest bridge request against the live server.
    async fn handle_test_command(
        &mut self,
        conn: &mut hotki_server::Connection,
        req: BridgeRequest,
    ) -> BridgeResponse {
        match req {
            BridgeRequest::Ping => match self.make_handshake_response(conn).await {
                Ok(r) => r,
                Err(err) => BridgeResponse::Err { message: err },
            },
            BridgeRequest::SetConfig { path } => match config::load_from_path(Path::new(&path)) {
                Ok(new_cfg) => {
                    self.ui_config = new_cfg.clone();
                    if conn.set_config(new_cfg.clone()).await.is_err() {
                        tracing::warn!("failed to push config to server on bridge set_config");
                    }
                    apply_ui_config(&new_cfg, &self.tx_keys, &self.egui_ctx).await;
                    BridgeResponse::Ok
                }
                Err(e) => BridgeResponse::Err {
                    message: e.pretty(),
                },
            },
            BridgeRequest::InjectKey {
                ident,
                kind,
                repeat,
            } => {
                let result = match (kind, repeat) {
                    (InjectKind::Down, true) => conn.inject_key_repeat(&ident).await,
                    (InjectKind::Down, false) => conn.inject_key_down(&ident).await,
                    (InjectKind::Up, _) => conn.inject_key_up(&ident).await,
                };
                match result {
                    Ok(()) => BridgeResponse::Ok,
                    Err(err) => BridgeResponse::Err {
                        message: err.to_string(),
                    },
                }
            }
            BridgeRequest::GetBindings => match conn.get_bindings().await {
                Ok(bindings) => BridgeResponse::Bindings { bindings },
                Err(err) => BridgeResponse::Err {
                    message: err.to_string(),
                },
            },
            BridgeRequest::GetDepth => match conn.get_depth().await {
                Ok(depth) => BridgeResponse::Depth { depth },
                Err(err) => BridgeResponse::Err {
                    message: err.to_string(),
                },
            },
            BridgeRequest::Shutdown => match conn.shutdown().await {
                Ok(()) => {
                    drain_bridge_events(
                        conn,
                        Self::MAX_SHUTDOWN_DRAIN_EVENTS,
                        Duration::from_secs(1),
                    )
                    .await;
                    BridgeResponse::Ok
                }
                Err(err) => BridgeResponse::Err {
                    message: err.to_string(),
                },
            },
        }
    }

    /// Process a message from the server and update the UI/bridge accordingly.
    async fn handle_server_msg(
        &mut self,
        conn: &mut hotki_server::Connection,
        msg: hotki_protocol::MsgToUI,
    ) {
        match msg {
            hotki_protocol::MsgToUI::HudUpdate { cursor, displays } => {
                self.current_cursor = cursor;
                let vks = self.ui_config.hud_keys_ctx(&self.current_cursor);
                let visible_keys: Vec<(Chord, String, bool)> = vks
                    .into_iter()
                    .filter(|(_, _, attrs, _)| !attrs.hide())
                    .map(|(k, desc, _attrs, is_mode)| (k, desc, is_mode))
                    .collect();
                let depth = self.current_cursor.depth();
                let parent_title = self
                    .ui_config
                    .parent_title(&self.current_cursor)
                    .map(|s| s.to_string());

                let bridge_keys: Vec<BridgeHudKey> = visible_keys
                    .iter()
                    .map(|(chord, desc, is_mode)| BridgeHudKey {
                        ident: chord.to_string(),
                        description: desc.clone(),
                        is_mode: *is_mode,
                    })
                    .collect();
                let bridge_parent_title = parent_title.clone();

                if self
                    .tx_keys
                    .send(AppEvent::KeyUpdate {
                        visible_keys,
                        depth,
                        cursor: self.current_cursor.clone(),
                        parent_title,
                        displays: displays.clone(),
                    })
                    .is_err()
                {
                    tracing::warn!("failed to send KeyUpdate");
                }
                self.egui_ctx.request_repaint();
                self.emit_bridge_event(BridgeEvent::Hud {
                    cursor: self.current_cursor.clone(),
                    depth,
                    parent_title: bridge_parent_title,
                    keys: bridge_keys,
                    displays,
                });
                self.emit_bridge_event(BridgeEvent::Focus {
                    app: self.current_cursor.app.clone(),
                });
            }
            hotki_protocol::MsgToUI::Notify { kind, title, text } => {
                self.notify(kind, &title, &text);
            }
            hotki_protocol::MsgToUI::ReloadConfig => {
                self.tx_ctrl_runtime.send(ControlMsg::Reload).ok();
                self.egui_ctx.request_repaint();
            }
            hotki_protocol::MsgToUI::ClearNotifications => {
                self.bridge_notifications.clear();
                self.tx_keys.send(AppEvent::ClearNotifications).ok();
                self.egui_ctx.request_repaint();
            }
            hotki_protocol::MsgToUI::ShowDetails(arg) => {
                match arg {
                    config::Toggle::On => {
                        self.tx_keys.send(AppEvent::ShowDetails).ok();
                    }
                    config::Toggle::Off => {
                        self.tx_keys.send(AppEvent::HideDetails).ok();
                    }
                    config::Toggle::Toggle => {
                        self.tx_keys.send(AppEvent::ToggleDetails).ok();
                    }
                }
                self.egui_ctx.request_repaint();
            }
            hotki_protocol::MsgToUI::ThemeNext => {
                self.apply_ui_override(UiOverride::ThemeNext);
            }
            hotki_protocol::MsgToUI::ThemePrev => {
                self.apply_ui_override(UiOverride::ThemePrev);
            }
            hotki_protocol::MsgToUI::ThemeSet(name) => {
                self.apply_ui_override(UiOverride::ThemeSet(name));
            }
            hotki_protocol::MsgToUI::UserStyle(arg) => {
                self.apply_ui_override(UiOverride::UserStyle(arg));
            }
            hotki_protocol::MsgToUI::HotkeyTriggered(_) => {}
            hotki_protocol::MsgToUI::Log {
                level,
                target,
                message,
            } => {
                logs::push_server(level, target, message);
                self.egui_ctx.request_repaint();
            }
            hotki_protocol::MsgToUI::Heartbeat(_) => {}
            hotki_protocol::MsgToUI::World(msg) => {
                self.handle_world_stream(conn, msg).await;
            }
        }
    }

    /// Forward world-stream messages to the smoketest bridge.
    async fn handle_world_stream(&self, _conn: &mut hotki_server::Connection, msg: WorldStreamMsg) {
        if self.dumpworld {
            debug!("World event: {:?}", msg);
        }
        let WorldStreamMsg::FocusChanged(app) = msg;
        self.emit_bridge_event(BridgeEvent::Focus { app });
    }

    /// Connect to the server, draining any queued control messages after connect.
    pub(crate) async fn connect(&mut self, initial_cfg: config::Config) -> Option<Client> {
        let perms = check_permissions();
        if !perms.accessibility_ok || !perms.input_ok {
            self.tx_keys.send(AppEvent::ShowPermissionsHelp).ok();
            self.egui_ctx.request_repaint();
        }

        let mut rx_conn_ready = spawn_connect(self.server_log_filter.clone());
        let mut preconnect_queue: VecDeque<ControlMsg> = VecDeque::new();
        let mut client: Client = loop {
            tokio::select! {
                biased;
                res = &mut rx_conn_ready => {
                    match res {
                        Ok(c) => break c,
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
            Ok(c) => c,
            Err(e) => {
                error!("Failed to get client connection: {}", e);
                return None;
            }
        };
        if let Err(e) = conn.set_config(initial_cfg).await {
            error!("Failed to set config on server: {}", e);
            return None;
        }
        debug!("Config sent to server engine");

        self.ensure_test_bridge().await;

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
            Ok(c) => c,
            Err(e) => {
                error!("Failed to get client connection: {}", e);
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
                        Err(e) => {
                            match e {
                                hotki_server::Error::Ipc(ref s) if s == "Event channel closed" => {
                                    tracing::info!("Event channel closed; exiting event loop");
                                }
                                _ => {
                                    error!("Connection error receiving event: {}", e);
                                }
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
        self.trigger_graceful_shutdown(750);
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
                    .map(|f| format!("{} (pid={}) — {}", f.app, f.pid, f.title))
                    .unwrap_or_else(|| "none".to_string());
                let display_count = snap.displays.displays.len();
                let active_disp = snap
                    .displays
                    .active
                    .as_ref()
                    .map(|d| d.id.to_string())
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
        let client = if let Some(f) = log_filter.clone() {
            Client::new()
                .with_auto_spawn_server()
                .with_server_log_filter(f)
        } else {
            Client::new().with_auto_spawn_server()
        };
        match client.connect().await {
            Ok(c) => {
                if tx_conn_ready.send(c).is_err() {
                    tracing::warn!("connect-ready channel closed before send");
                }
            }
            Err(e) => {
                tracing::error!("Failed to connect to hotkey server: {}", e);
            }
        }
    });
    rx
}
