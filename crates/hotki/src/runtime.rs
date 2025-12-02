//! UI runtime: connects to the server, forwards events to the UI, and applies
//! configuration/overrides. This module also handles permissions helpers and
//! convenience actions for opening macOS settings.
use std::{
    collections::VecDeque,
    convert::TryInto,
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    process::{self, Command},
    thread,
    time::{Duration as StdDuration, SystemTime, UNIX_EPOCH},
};

use config::themes;
use egui::Context;
use hotki_protocol::{NotifyKind, WorldStreamMsg, ipc::heartbeat};
use hotki_server::{
    Client,
    smoketest_bridge::{
        BridgeCommand, BridgeCommandId, BridgeEvent, BridgeHudKey, BridgeKeyKind,
        BridgeNotifications, BridgeReply, BridgeRequest, BridgeResponse, BridgeTimestampMs,
        control_socket_path, drain_bridge_events, handshake_response,
    },
};
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
    net::{UnixListener, UnixStream, unix::OwnedWriteHalf},
    sync::{broadcast, mpsc, oneshot},
    time::{Duration, Instant as TokioInstant, Sleep, sleep},
};
use tracing::{debug, error, info};

use crate::{app::AppEvent, logs, permissions::check_permissions};

/// Actions that adjust UI overrides on the current cursor (theme and user style).
#[derive(Debug, Clone)]
enum UiOverride {
    /// Switch to the next theme.
    ThemeNext,
    /// Switch to the previous theme.
    ThemePrev,
    /// Set the theme to the given name.
    ThemeSet(String),
    /// Toggle or set the user style state.
    UserStyle(config::Toggle),
}

/// Drives the MRPC connection for the UI: connect, process events, and apply config/overrides.
struct ConnectionDriver {
    /// Path to the config file used for reloads.
    config_path: PathBuf,
    /// Optional server log filter passed to the child server process.
    server_log_filter: Option<String>,
    /// Channel to send UI app events.
    tx_keys: mpsc::UnboundedSender<AppEvent>,
    /// egui context used to request repaints.
    egui_ctx: Context,
    /// Control channel from UI widgets and tray.
    rx_ctrl: mpsc::UnboundedReceiver<ControlMsg>,
    /// Control channel back into the runtime (self-directed messages).
    tx_ctrl_runtime: mpsc::UnboundedSender<ControlMsg>,

    /// Current UI configuration.
    ui_config: config::Config,
    /// Current cursor context used to evaluate UI state.
    current_cursor: config::Cursor,
    /// When true, periodically dump the world snapshot to logs.
    dumpworld: bool,
    /// Optional smoketest bridge socket path to expose test commands.
    test_bridge_path: Option<PathBuf>,
    /// Broadcast channel used to stream bridge updates to the smoketest harness.
    bridge_events: broadcast::Sender<BridgeEvent>,
    /// Pending notifications tracked for smoketest handshake responses.
    bridge_notifications: BridgeNotifications,
}

impl ConnectionDriver {
    /// Maximum number of notifications tracked for smoketest handshake payloads.
    const MAX_BRIDGE_NOTIFICATIONS: usize = 32;
    /// Cap on the number of events drained during shutdown handshake preparation.
    const MAX_SHUTDOWN_DRAIN_EVENTS: usize = 128;

    /// Publish a bridge event to any smoketest subscribers.
    fn emit_bridge_event(&self, event: BridgeEvent) {
        if self.bridge_events.send(event).is_err() {
            // No active smoketest subscribers; ignore the backpressure signal.
        }
    }

    /// Build the handshake payload returned to the smoketest harness.
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

    /// Record the latest world reconcile sequence and wake pending waiters.
    /// Construct a new driver instance with initial configuration and channels.
    fn new(
        config_path: PathBuf,
        server_log_filter: Option<String>,
        tx_keys: mpsc::UnboundedSender<AppEvent>,
        egui_ctx: Context,
        rx_ctrl: mpsc::UnboundedReceiver<ControlMsg>,
        tx_ctrl_runtime: mpsc::UnboundedSender<ControlMsg>,
        dumpworld: bool,
    ) -> Self {
        // Load UI Config once; on Reload events the UI will refresh independently.
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

    /// Initialize the smoketest bridge listener if it hasn't been started yet.
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
            // If initialization failed, allow later retries by restoring the path
            self.test_bridge_path = Some(path);
        }
    }

    /// Request a graceful app shutdown: notify UI, ask the native
    /// window to close, and arm a short fast-exit fallback.
    fn trigger_graceful_shutdown(&self, fallback_ms: u64) {
        if self.tx_keys.send(AppEvent::Shutdown).is_err() {
            tracing::warn!("failed to send Shutdown UI event");
        }
        self.egui_ctx
            .send_viewport_cmd(egui::ViewportCommand::Close);
        self.egui_ctx.request_repaint();
        tokio::spawn(async move {
            sleep(Duration::from_millis(fallback_ms)).await;
            process::exit(0);
        });
    }

    /// Handle control messages that do not require a server connection.
    fn handle_local_control(&mut self, msg: ControlMsg) {
        match msg {
            ControlMsg::OpenAccessibility => {
                open_accessibility_settings();
                self.notify(
                    NotifyKind::Info,
                    "Accessibility",
                    "Opening Accessibility settings...",
                );
            }
            ControlMsg::OpenInputMonitoring => {
                open_input_monitoring_settings();
                self.notify(
                    NotifyKind::Info,
                    "Input Monitoring",
                    "Opening Input Monitoring settings...",
                );
            }
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

    /// Handle a control message once a server connection is available.
    /// Returns true if shutdown was requested.
    async fn handle_connected_control(
        &mut self,
        conn: &mut hotki_server::Connection,
        msg: ControlMsg,
    ) -> bool {
        match msg {
            ControlMsg::Shutdown => {
                self.trigger_graceful_shutdown(750);
                let _res = conn.shutdown().await;
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
                    command_id,
                    req,
                    respond_to,
                } = cmd;
                let response = self.handle_test_command(conn, req).await;
                if respond_to.send(response).is_err() {
                    tracing::debug!(
                        command_id,
                        "bridge responder dropped while delivering response"
                    );
                }
                false
            }
            other => {
                self.handle_local_control(other);
                false
            }
        }
    }

    /// Route a control message, queuing when disconnected. Returns true on shutdown.
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
                if cmd
                    .respond_to
                    .send(BridgeResponse::Err {
                        message: "bridge not ready".to_string(),
                    })
                    .is_err()
                {
                    tracing::debug!("bridge responder dropped before connection readiness");
                }
                false
            }
            (None, other) => {
                self.handle_local_control(other);
                false
            }
        }
    }

    /// Helper to send a UI notification.
    fn notify(&mut self, kind: NotifyKind, title: &str, text: &str) {
        self.bridge_notifications.record(kind, title, text);
        if self
            .tx_keys
            .send(AppEvent::Notify {
                kind,
                title: title.to_string(),
                text: text.to_string(),
            })
            .is_err()
        {
            tracing::warn!("failed to send Notify");
        }
        self.egui_ctx.request_repaint();
    }

    /// Send the current cursor to the UI and repaint, logging context on failure.
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

    /// Apply a UI override (theme or user style) to the current cursor and notify UI.
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
                    config::Toggle::Toggle => self
                        .current_cursor
                        .set_user_style_enabled(!self.current_cursor.user_ui_disabled),
                };
                update_reason = Some("user style");
            }
        }
        if let Some(reason) = update_reason {
            self.push_cursor_update(reason);
        }
    }

    /// Handle an individual smoketest bridge request using the active connection.
    async fn handle_test_command(
        &mut self,
        conn: &mut hotki_server::Connection,
        req: BridgeRequest,
    ) -> BridgeResponse {
        match req {
            BridgeRequest::Ping => match self.make_handshake_response(conn).await {
                Ok(resp) => resp,
                Err(message) => BridgeResponse::Err { message },
            },
            BridgeRequest::SetConfig { path } => match config::load_from_path(Path::new(&path)) {
                Ok(cfg) => match conn.set_config(cfg.clone()).await {
                    Ok(()) => {
                        self.ui_config = cfg;
                        apply_ui_config(&self.ui_config, &self.tx_keys, &self.egui_ctx).await;
                        BridgeResponse::Ok
                    }
                    Err(err) => BridgeResponse::Err {
                        message: err.to_string(),
                    },
                },
                Err(err) => BridgeResponse::Err {
                    message: err.pretty(),
                },
            },
            BridgeRequest::InjectKey {
                ident,
                kind,
                repeat,
            } => {
                let result = match (kind, repeat) {
                    (BridgeKeyKind::Down, true) => conn.inject_key_repeat(&ident).await,
                    (BridgeKeyKind::Down, false) => conn.inject_key_down(&ident).await,
                    (BridgeKeyKind::Up, _) => conn.inject_key_up(&ident).await,
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

    /// Handle a single server-to-UI event received from the engine.
    async fn handle_server_msg(
        &mut self,
        conn: &mut hotki_server::Connection,
        msg: hotki_protocol::MsgToUI,
    ) {
        match msg {
            hotki_protocol::MsgToUI::HudUpdate { cursor, displays } => {
                let displays_clone = displays.clone();
                self.current_cursor = cursor;
                let vks = self.ui_config.hud_keys_ctx(&self.current_cursor);
                let visible_keys: Vec<(String, String, bool)> = vks
                    .into_iter()
                    .filter(|(_, _, attrs, _)| !attrs.hide())
                    .map(|(k, desc, _attrs, is_mode)| (k.to_string(), desc, is_mode))
                    .collect();
                let depth = self.ui_config.depth(&self.current_cursor);
                let parent_title = self
                    .ui_config
                    .parent_title(&self.current_cursor)
                    .map(|s| s.to_string());
                let bridge_keys: Vec<BridgeHudKey> = visible_keys
                    .iter()
                    .map(|(ident, desc, is_mode)| BridgeHudKey {
                        ident: ident.clone(),
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
                        displays: displays_clone,
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
                if self.tx_ctrl_runtime.send(ControlMsg::Reload).is_err() {
                    tracing::warn!("failed to send Reload control");
                }
                self.egui_ctx.request_repaint();
            }
            hotki_protocol::MsgToUI::ClearNotifications => {
                self.bridge_notifications.clear();
                if self.tx_keys.send(AppEvent::ClearNotifications).is_err() {
                    tracing::warn!("failed to send ClearNotifications");
                }
                self.egui_ctx.request_repaint();
            }
            hotki_protocol::MsgToUI::ShowDetails(arg) => {
                match arg {
                    config::Toggle::On => {
                        if self.tx_keys.send(AppEvent::ShowDetails).is_err() {
                            tracing::warn!("failed to send ShowDetails");
                        }
                    }
                    config::Toggle::Off => {
                        if self.tx_keys.send(AppEvent::HideDetails).is_err() {
                            tracing::warn!("failed to send HideDetails");
                        }
                    }
                    config::Toggle::Toggle => {
                        if self.tx_keys.send(AppEvent::ToggleDetails).is_err() {
                            tracing::warn!("failed to send ToggleDetails");
                        }
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
            hotki_protocol::MsgToUI::Heartbeat(_) => {
                // No-op beyond heartbeat timer reset in the caller
            }
            hotki_protocol::MsgToUI::World(msg) => {
                self.handle_world_stream(conn, msg).await;
            }
        }
    }

    /// Process a streamed world event: update reconcile metrics and emit focus updates.
    async fn handle_world_stream(&self, _conn: &mut hotki_server::Connection, msg: WorldStreamMsg) {
        if self.dumpworld {
            debug!("World event: {:?}", msg);
        }
        let WorldStreamMsg::FocusChanged(app) = msg;
        self.emit_bridge_event(BridgeEvent::Focus { app });
    }

    /// Background connect with a preconnect control-message queue. Returns an open connection.
    async fn connect(&mut self, initial_cfg: config::Config) -> Option<hotki_server::Client> {
        // Kick off server connect in background, but keep servicing control messages

        // Show permissions help if either permission is missing
        let perms = check_permissions();
        if !perms.accessibility_ok || !perms.input_ok {
            if self.tx_keys.send(AppEvent::ShowPermissionsHelp).is_err() {
                tracing::warn!("failed to send permissions help event");
            }
            self.egui_ctx.request_repaint();
        }

        let mut rx_conn_ready = spawn_connect(self.server_log_filter.clone());
        let mut preconnect_queue: VecDeque<ControlMsg> = VecDeque::new();
        let mut client: hotki_server::Client = loop {
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
                    if self
                        .route_control_msg(None, msg, &mut preconnect_queue)
                        .await
                    {
                        // Shutdown requested; exit early (graceful close in progress)
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

        // Apply any queued preconnect messages now that we are connected.
        // Ensure theme switches use the same override path for consistency.
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

    /// Main event loop: process control messages, server events, and heartbeat.
    async fn drive_events(&mut self, client: &mut hotki_server::Client) {
        // Borrow the connection once for the duration of the loop
        let conn = match client.connection() {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to get client connection: {}", e);
                return;
            }
        };
        // Heartbeat: if we don't receive any server message within timeout, exit.
        let hb_timer: Sleep = sleep(heartbeat::timeout());
        tokio::pin!(hb_timer);
        // Optional world dump timer
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
                // If the heartbeat timer fires, assume the backend is gone and exit gracefully
                _ = &mut hb_timer => {
                    error!("No IPC activity within heartbeat timeout; exiting UI event loop");
                    break;
                }
                Some(msg) = self.rx_ctrl.recv() => {
                    if self
                        .route_control_msg(Some(conn), msg, &mut control_sink)
                        .await
                    {
                        break;
                    }
                }
                resp = conn.recv_event() => {
                    match resp {
                        Ok(msg) => {
                            // Any message indicates liveness; reset the heartbeat timer
                            hb_timer.as_mut().reset(TokioInstant::now() + heartbeat::timeout());
                            self.handle_server_msg(conn, msg).await;
                        }
                        Err(e) => {
                            match e {
                                // Channel closed is expected on shutdown; log at info level
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
                // Periodic world dump (disabled when flag not set)
                _ = &mut dump_timer => {
                    let next = self.compute_dump_reset(conn, dump_interval, dump_far_future).await;
                    dump_timer.as_mut().reset(TokioInstant::now() + next);
                }
            }
        }
        info!("Exiting key loop");
        // Ask the app to close and rely on fallback if needed.
        self.trigger_graceful_shutdown(750);
    }

    /// Compute the next dump timer reset duration and optionally log a snapshot.
    async fn compute_dump_reset(
        &self,
        conn: &mut hotki_server::Connection,
        dump_interval: Duration,
        dump_far_future: Duration,
    ) -> Duration {
        if self.dumpworld {
            if let Ok(snap) = conn.get_world_snapshot().await {
                use std::fmt::Write as _;
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
                    tracing::debug!("failed to write world header line");
                }
                tracing::info!(target: "hotki::worlddump", "\n{}", out);
            }
            dump_interval
        } else {
            dump_far_future
        }
    }
}

/// Spawn the UI-side listener that proxies smoketest bridge requests.
async fn init_test_bridge(
    path: PathBuf,
    tx_ctrl_runtime: mpsc::UnboundedSender<ControlMsg>,
    events: broadcast::Sender<BridgeEvent>,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    if let Err(err) = fs::remove_file(&path).await
        && err.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(?err, socket = %path.display(), "failed to remove stale test bridge socket");
    }
    let listener = UnixListener::bind(&path)?;
    let cleanup_path = path.clone();
    tokio::spawn(async move {
        if let Err(err) = run_test_bridge(listener, tx_ctrl_runtime, events).await {
            tracing::debug!(?err, "smoketest bridge listener exited");
        }
        if let Err(err) = fs::remove_file(&cleanup_path).await
            && err.kind() != io::ErrorKind::NotFound
        {
            tracing::debug!(?err, "failed to remove smoketest bridge socket on shutdown");
        }
    });
    Ok(())
}

/// Accept incoming bridge clients and spawn per-connection handlers.
async fn run_test_bridge(
    listener: UnixListener,
    tx_ctrl_runtime: mpsc::UnboundedSender<ControlMsg>,
    events: broadcast::Sender<BridgeEvent>,
) -> io::Result<()> {
    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let tx = tx_ctrl_runtime.clone();
                let rx = events.subscribe();
                tokio::spawn(async move {
                    if let Err(err) = handle_test_bridge_client(stream, tx, rx).await {
                        tracing::debug!(?err, "smoketest bridge client disconnected");
                    }
                });
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {
                continue;
            }
            Err(err) => {
                return Err(err);
            }
        }
    }
}

/// Future that resolves with the command id and final bridge response.
type ProcessingFuture = Pin<Box<dyn Future<Output = (BridgeCommandId, BridgeResponse)> + Send>>;

/// Process commands from a single smoketest bridge client connection.
async fn handle_test_bridge_client(
    stream: UnixStream,
    tx_ctrl_runtime: mpsc::UnboundedSender<ControlMsg>,
    mut event_rx: broadcast::Receiver<BridgeEvent>,
) -> io::Result<()> {
    let (reader, writer) = stream.into_split();
    let reader = BufReader::new(reader);
    let mut writer = BufWriter::new(writer);
    let mut lines = reader.lines();

    let mut pending: VecDeque<BridgeCommand> = VecDeque::new();
    let mut processing: Option<ProcessingFuture> = None;
    let mut expected_command: BridgeCommandId = 0;
    let mut next_event_id: BridgeCommandId = 1 << 63;

    loop {
        tokio::select! {
            maybe_line = lines.next_line() => {
                match maybe_line? {
                    Some(line) => {
                        handle_bridge_line(
                            line,
                            &mut writer,
                            &mut pending,
                            &mut processing,
                            &tx_ctrl_runtime,
                            &mut expected_command,
                        ).await?;
                    }
                    None => break,
                }
            }
            event = event_rx.recv() => {
                match event {
                    Ok(event) => {
                        let reply = BridgeReply {
                            command_id: next_event_id,
                            timestamp_ms: now_millis(),
                            response: BridgeResponse::Event { event },
                        };
                        write_bridge_reply(&mut writer, reply).await?;
                        next_event_id = next_event_id.wrapping_add(1);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            result = async {
                if let Some(fut) = processing.as_mut() {
                    Some(fut.await)
                } else {
                    None
                }
            }, if processing.is_some() => {
                if let Some((command_id, response)) = result {
                    let reply = BridgeReply {
                        command_id,
                        timestamp_ms: now_millis(),
                        response,
                    };
                    write_bridge_reply(&mut writer, reply).await?;
                    processing = None;
                    drive_queue(&mut pending, &mut processing, &tx_ctrl_runtime, &mut writer).await?;
                }
            }
        }
    }

    writer.flush().await?;
    Ok(())
}

/// Process a single inbound bridge line: validate sequence, enqueue, and ACK.
async fn handle_bridge_line(
    line: String,
    writer: &mut BufWriter<OwnedWriteHalf>,
    pending: &mut VecDeque<BridgeCommand>,
    processing: &mut Option<ProcessingFuture>,
    tx_ctrl_runtime: &mpsc::UnboundedSender<ControlMsg>,
    expected_command: &mut BridgeCommandId,
) -> io::Result<()> {
    if line.trim().is_empty() {
        return Ok(());
    }

    let command: BridgeCommand = match serde_json::from_str(&line) {
        Ok(cmd) => cmd,
        Err(err) => {
            let reply = BridgeReply {
                command_id: *expected_command,
                timestamp_ms: now_millis(),
                response: BridgeResponse::Err {
                    message: format!("invalid request: {}", err),
                },
            };
            write_bridge_reply(writer, reply).await?;
            return Ok(());
        }
    };

    if command.command_id != *expected_command {
        let reply = BridgeReply {
            command_id: command.command_id,
            timestamp_ms: now_millis(),
            response: BridgeResponse::Err {
                message: format!(
                    "unexpected command id: expected {}, got {}",
                    *expected_command, command.command_id
                ),
            },
        };
        write_bridge_reply(writer, reply).await?;
        return Ok(());
    }

    let next = (*expected_command).wrapping_add(1);
    *expected_command = next;
    let command_id = command.command_id;
    pending.push_back(command);

    let queued = pending.len() + if processing.is_some() { 1 } else { 0 };
    let ack = BridgeReply {
        command_id,
        timestamp_ms: now_millis(),
        response: BridgeResponse::Ack { queued },
    };
    write_bridge_reply(writer, ack).await?;

    drive_queue(pending, processing, tx_ctrl_runtime, writer).await
}

/// Drive the queued commands, ensuring only one runtime request executes at a time.
async fn drive_queue(
    pending: &mut VecDeque<BridgeCommand>,
    processing: &mut Option<ProcessingFuture>,
    tx_ctrl_runtime: &mpsc::UnboundedSender<ControlMsg>,
    writer: &mut BufWriter<OwnedWriteHalf>,
) -> io::Result<()> {
    while processing.is_none() {
        let Some(command) = pending.pop_front() else {
            break;
        };
        let BridgeCommand {
            command_id,
            request,
            ..
        } = command;
        let (reply_tx, reply_rx) = oneshot::channel();
        if tx_ctrl_runtime
            .send(ControlMsg::Test(TestCommand {
                command_id,
                req: request,
                respond_to: reply_tx,
            }))
            .is_err()
        {
            let reply = BridgeReply {
                command_id,
                timestamp_ms: now_millis(),
                response: BridgeResponse::Err {
                    message: "runtime control channel closed".to_string(),
                },
            };
            write_bridge_reply(writer, reply).await?;
            continue;
        }

        let fut = Box::pin(async move {
            let response = match reply_rx.await {
                Ok(resp) => resp,
                Err(_canceled) => BridgeResponse::Err {
                    message: "runtime dropped bridge response".to_string(),
                },
            };
            (command_id, response)
        });
        *processing = Some(fut);
    }
    Ok(())
}

/// Serialize a bridge reply to the client stream.
/// Serialize a bridge reply to the client stream.
async fn write_bridge_reply(
    writer: &mut BufWriter<OwnedWriteHalf>,
    reply: BridgeReply,
) -> io::Result<()> {
    let encoded = serde_json::to_string(&reply).map_err(io::Error::other)?;
    writer.write_all(encoded.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

/// Return the current wall-clock timestamp in milliseconds since the Unix epoch.
fn now_millis() -> BridgeTimestampMs {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| StdDuration::from_secs(0))
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// Control messages routed to the runtime event loop.
#[derive(Debug)]
pub enum ControlMsg {
    /// Reload from disk using `config_path`
    Reload,
    /// Open macOS Accessibility privacy settings.
    OpenAccessibility,
    /// Open macOS Input Monitoring privacy settings.
    OpenInputMonitoring,
    /// Gracefully shut down the UI and exit the process
    Shutdown,
    /// Request a theme switch by name (handled here on the live Config)
    SwitchTheme(String),
    /// Open the in-app permissions help view.
    OpenPermissionsHelp,
    /// Forward a user-facing notice into the app UI
    Notice {
        /// Notice severity kind.
        kind: NotifyKind,
        /// Notice title text.
        title: String,
        /// Notice body text.
        text: String,
    },
    /// Internal test bridge command (smoketest harness).
    Test(TestCommand),
}

/// Request/response pair used to service smoketest bridge commands.
#[derive(Debug)]
pub struct TestCommand {
    /// Identifier for the command being serviced.
    command_id: BridgeCommandId,
    /// The bridge request submitted by the smoketest harness.
    req: BridgeRequest,
    /// Channel used to deliver the bridge response back to the harness.
    respond_to: oneshot::Sender<BridgeResponse>,
}

/// Open the macOS Accessibility privacy pane.
fn open_accessibility_settings() {
    if Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn()
        .is_err()
    {
        tracing::warn!("failed to open Accessibility settings");
    }
}

/// Open the macOS Input Monitoring privacy pane.
fn open_input_monitoring_settings() {
    if Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
        .spawn()
        .is_err()
    {
        tracing::warn!("failed to open Input Monitoring settings");
    }
}

/// Apply the current UI config: notify UI to reload and send config to server, then repaint.
async fn apply_ui_config(
    ui_config: &config::Config,
    tx_keys: &mpsc::UnboundedSender<AppEvent>,
    egui_ctx: &Context,
) {
    // UI refresh request; sending config to server only necessary when config changed.
    if tx_keys
        .send(AppEvent::ReloadUI(Box::new(ui_config.clone())))
        .is_err()
    {
        tracing::warn!("failed to send ReloadUI to app channel");
    }
    egui_ctx.request_repaint();
}

/// Single-source reload: load from disk, apply to UI + server, and notify success or error.
async fn reload_and_broadcast(
    conn: &mut hotki_server::Connection,
    ui_config: &mut config::Config,
    config_path: &Path,
    tx_keys: &mpsc::UnboundedSender<AppEvent>,
    egui_ctx: &Context,
) {
    match config::load_from_path(config_path) {
        Ok(new_cfg) => {
            *ui_config = new_cfg.clone();
            if tx_keys
                .send(AppEvent::Notify {
                    kind: NotifyKind::Success,
                    title: "Config".to_string(),
                    text: "Reloaded successfully".to_string(),
                })
                .is_err()
            {
                tracing::warn!("failed to send reload success notification");
            }
            // For reload, push the new config to the server engine, then refresh UI
            if conn.set_config(ui_config.clone()).await.is_err() {
                tracing::warn!("failed to push config to server on reload");
            }
            apply_ui_config(ui_config, tx_keys, egui_ctx).await;
        }
        Err(e) => {
            if tx_keys
                .send(AppEvent::Notify {
                    kind: NotifyKind::Error,
                    title: "Config".to_string(),
                    text: e.pretty(),
                })
                .is_err()
            {
                tracing::warn!("failed to send reload error notification");
            }
            egui_ctx.request_repaint();
        }
    }
}

// Spawn a background task to establish a server connection and return a oneshot
// which resolves when the connection is ready.
/// Spawn background task to establish a server connection and return a oneshot
/// which resolves when the connection is ready.
fn spawn_connect(log_filter: Option<String>) -> oneshot::Receiver<hotki_server::Client> {
    let (tx_conn_ready, rx) = oneshot::channel::<hotki_server::Client>();
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

/// Start background key runtime and server connection driver on a dedicated thread.
#[allow(clippy::too_many_arguments)]
pub fn spawn_key_runtime(
    cfg: &config::Config,
    config_path: &Path,
    tx_keys: &mpsc::UnboundedSender<AppEvent>,
    egui_ctx: &Context,
    tx_ctrl_runtime: &mpsc::UnboundedSender<ControlMsg>,
    rx_ctrl: mpsc::UnboundedReceiver<ControlMsg>,
    server_log_filter: Option<String>,
    dumpworld: bool,
) {
    // Take cheap clones here to own values in the thread
    let cfg = cfg.clone();
    let config_path = config_path.to_path_buf();
    let tx_keys = tx_keys.clone();
    let egui_ctx = egui_ctx.clone();
    let tx_ctrl_runtime = tx_ctrl_runtime.clone();
    thread::spawn(move || {
        use tokio::runtime::Runtime;
        let rt = match Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("Failed to create Tokio runtime: {}", e);
                return;
            }
        };
        rt.block_on(async move {
            info!("Loaded mode; delegating to server engine");
            let mut driver = ConnectionDriver::new(
                config_path,
                server_log_filter,
                tx_keys,
                egui_ctx,
                rx_ctrl,
                tx_ctrl_runtime,
                dumpworld,
            );
            if let Some(mut client) = driver.connect(cfg).await {
                driver.drive_events(&mut client).await;
            }
        });
    });
}
