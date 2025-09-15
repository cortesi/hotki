//! UI runtime: connects to the server, forwards events to the UI, and applies
//! configuration/overrides. This module also handles permissions helpers and
//! convenience actions for opening macOS settings.
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    process::{self, Command},
    thread,
};

use config::themes;
use egui::Context;
use hotki_protocol::{NotifyKind, ipc::heartbeat};
use hotki_server::Client;
use tokio::{
    sync::{mpsc, oneshot},
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
}

impl ConnectionDriver {
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
        }
    }

    /// Handle control messages received before the server connection is ready.
    /// Returns true if a Shutdown was requested (caller should exit).
    async fn handle_preconnect_control(
        &self,
        msg: ControlMsg,
        preconnect_queue: &mut VecDeque<ControlMsg>,
    ) -> bool {
        match msg.clone() {
            ControlMsg::Shutdown => {
                if self.tx_keys.send(AppEvent::Shutdown).is_err() {
                    tracing::warn!("failed to send Shutdown UI event");
                }
                self.egui_ctx.request_repaint();
                sleep(Duration::from_millis(250)).await;
                return true;
            }
            ControlMsg::Reload | ControlMsg::SwitchTheme(_) => {
                preconnect_queue.push_back(msg);
            }
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
                if self
                    .tx_keys
                    .send(AppEvent::Notify { kind, title, text })
                    .is_err()
                {
                    tracing::warn!("failed to send Notify");
                }
                self.egui_ctx.request_repaint();
            }
        }
        false
    }

    /// Helper to send a UI notification.
    fn notify(&self, kind: NotifyKind, title: &str, text: &str) {
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

    /// Apply a UI override (theme or user style) to the current cursor and notify UI.
    fn apply_ui_override(&mut self, action: UiOverride) {
        match action {
            UiOverride::ThemeNext => {
                let cur = self
                    .current_cursor
                    .override_theme
                    .as_deref()
                    .unwrap_or("default");
                let next = themes::get_next_theme(cur);
                self.current_cursor.set_theme(Some(next));
                if self
                    .tx_keys
                    .send(AppEvent::UpdateCursor(self.current_cursor.clone()))
                    .is_err()
                {
                    tracing::warn!("failed to send UpdateCursor (next theme)");
                }
            }
            UiOverride::ThemePrev => {
                let cur = self
                    .current_cursor
                    .override_theme
                    .as_deref()
                    .unwrap_or("default");
                let prev = themes::get_prev_theme(cur);
                self.current_cursor.set_theme(Some(prev));
                if self
                    .tx_keys
                    .send(AppEvent::UpdateCursor(self.current_cursor.clone()))
                    .is_err()
                {
                    tracing::warn!("failed to send UpdateCursor (prev theme)");
                }
            }
            UiOverride::ThemeSet(name) => {
                if themes::theme_exists(&name) {
                    self.current_cursor.set_theme(Some(&name));
                    if self
                        .tx_keys
                        .send(AppEvent::UpdateCursor(self.current_cursor.clone()))
                        .is_err()
                    {
                        tracing::warn!("failed to send UpdateCursor (set theme)");
                    }
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
                }
                if self
                    .tx_keys
                    .send(AppEvent::UpdateCursor(self.current_cursor.clone()))
                    .is_err()
                {
                    tracing::warn!("failed to send UpdateCursor (user style)");
                }
            }
        }
        self.egui_ctx.request_repaint();
    }

    // Handle a control message while connected; may exit the process on Shutdown.
    /// Handle a control message while connected; may exit the process on Shutdown.
    async fn handle_runtime_control(
        &mut self,
        conn: &mut hotki_server::Connection,
        msg: ControlMsg,
    ) {
        match msg {
            ControlMsg::Shutdown => {
                if self.tx_keys.send(AppEvent::Shutdown).is_err() {
                    tracing::warn!("failed to send Shutdown UI event");
                }
                self.egui_ctx.request_repaint();
                sleep(Duration::from_millis(250)).await;
                process::exit(0);
            }
            ControlMsg::SwitchTheme(name) => {
                self.apply_ui_override(UiOverride::ThemeSet(name));
            }
            other => {
                handle_control_msg(
                    conn,
                    other,
                    &mut self.ui_config,
                    &self.config_path,
                    &self.tx_keys,
                    &self.egui_ctx,
                )
                .await;
            }
        }
    }

    /// Handle a single server-to-UI event received from the engine.
    async fn handle_server_msg(&mut self, msg: hotki_protocol::MsgToUI) {
        match msg {
            hotki_protocol::MsgToUI::HudUpdate { cursor } => {
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
                if self
                    .tx_keys
                    .send(AppEvent::KeyUpdate {
                        visible_keys,
                        depth,
                        cursor: self.current_cursor.clone(),
                        parent_title,
                    })
                    .is_err()
                {
                    tracing::warn!("failed to send KeyUpdate");
                }
                self.egui_ctx.request_repaint();
            }
            hotki_protocol::MsgToUI::Notify { kind, title, text } => {
                if self
                    .tx_keys
                    .send(AppEvent::Notify { kind, title, text })
                    .is_err()
                {
                    tracing::warn!("failed to send Notify");
                }
                self.egui_ctx.request_repaint();
            }
            hotki_protocol::MsgToUI::ReloadConfig => {
                if self.tx_ctrl_runtime.send(ControlMsg::Reload).is_err() {
                    tracing::warn!("failed to send Reload control");
                }
                self.egui_ctx.request_repaint();
            }
            hotki_protocol::MsgToUI::ClearNotifications => {
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
                if self.dumpworld {
                    debug!("World event: {:?}", msg);
                }
            }
        }
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
                    if self.handle_preconnect_control(msg, &mut preconnect_queue).await {
                        // Shutdown requested; exit early
                        process::exit(0);
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

        // Apply any queued preconnect messages now that we are connected.
        // Ensure theme switches use the same override path for consistency.
        while let Some(msg) = preconnect_queue.pop_front() {
            match msg {
                ControlMsg::SwitchTheme(name) => {
                    self.apply_ui_override(UiOverride::ThemeSet(name));
                }
                other => {
                    handle_control_msg(
                        conn,
                        other,
                        &mut self.ui_config,
                        &self.config_path,
                        &self.tx_keys,
                        &self.egui_ctx,
                    )
                    .await;
                }
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

        loop {
            tokio::select! {
                biased;
                // If the heartbeat timer fires, assume the backend is gone and exit gracefully
                _ = &mut hb_timer => {
                    error!("No IPC activity within heartbeat timeout; exiting UI event loop");
                    break;
                }
                Some(msg) = self.rx_ctrl.recv() => {
                    self.handle_runtime_control(conn, msg).await;
                }
                resp = conn.recv_event() => {
                    match resp {
                        Ok(msg) => {
                            // Any message indicates liveness; reset the heartbeat timer
                            hb_timer.as_mut().reset(TokioInstant::now() + heartbeat::timeout());
                            // Handle explicit backpressure recovery: request a world snapshot
                            // when the server signals that a resync is recommended.
                            if let hotki_protocol::MsgToUI::World(hotki_protocol::WorldStreamMsg::ResyncRecommended) = &msg {
                                // Briefly notify the user in Details → Notifications
                                if self.tx_keys.send(AppEvent::Notify {
                                    kind: NotifyKind::Info,
                                    title: "World".to_string(),
                                    text: "Syncing…".to_string(),
                                }).is_err() {
                                    tracing::warn!("failed to send world-sync start notification");
                                }
                                self.egui_ctx.request_repaint();

                                // Fetch a fresh snapshot from the server to realign state.
                                match conn.get_world_snapshot().await {
                                    Ok(_snap) => {
                                        // Snapshot fetched successfully; acknowledge with a subtle success.
                                        if self.tx_keys.send(AppEvent::Notify {
                                            kind: NotifyKind::Success,
                                            title: "World".to_string(),
                                            text: "Synced".to_string(),
                                        }).is_err() {
                                            tracing::warn!("failed to send world-sync success notification");
                                        }
                                        self.egui_ctx.request_repaint();
                                    }
                                    Err(e) => {
                                        // Surface the error so users have a clue if something goes wrong.
                                        if self.tx_keys.send(AppEvent::Notify {
                                            kind: NotifyKind::Error,
                                            title: "World".to_string(),
                                            text: format!("Sync failed: {}", e),
                                        }).is_err() {
                                            tracing::warn!("failed to send world-sync error notification");
                                        }
                                        self.egui_ctx.request_repaint();
                                    }
                                }
                            }
                            self.handle_server_msg(msg).await;
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
        // Terminate the process so any HUD windows are closed immediately.
        // Relying on UI message processing can stall if the UI isn't repainting.
        process::exit(0);
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
                if writeln!(
                    out,
                    "World: {} window(s); focused: {}",
                    snap.windows.len(),
                    focused_ctx
                )
                .is_err()
                {
                    tracing::debug!("failed to write world header line");
                }
                for w in snap.windows.iter() {
                    let mark = if w.focused { '*' } else { ' ' };
                    let disp = w
                        .display_id
                        .map(|d| d.to_string())
                        .unwrap_or_else(|| "-".into());
                    let title = if w.title.is_empty() {
                        "(no title)"
                    } else {
                        &w.title
                    };
                    if writeln!(
                        out,
                        "  {} z={:<2} pid={:<6} id={:<8} disp={:<3} app={:<16} title={}",
                        mark, w.z, w.pid, w.id, disp, w.app, title
                    )
                    .is_err()
                    {
                        tracing::debug!("failed to write world window line");
                    }
                }
                tracing::info!(target: "hotki::worlddump", "\n{}", out);
            }
            dump_interval
        } else {
            dump_far_future
        }
    }
}

#[derive(Debug, Clone)]
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
}

fn open_accessibility_settings() {
    if Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn()
        .is_err()
    {
        tracing::warn!("failed to open Accessibility settings");
    }
}

/// Open the system preferences pane for Input Monitoring.
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

/// Unified handler for all `ControlMsg` variants once a connection exists.
async fn handle_control_msg(
    conn: &mut hotki_server::Connection,
    msg: ControlMsg,
    ui_config: &mut config::Config,
    config_path: &Path,
    tx_keys: &mpsc::UnboundedSender<AppEvent>,
    egui_ctx: &Context,
) {
    match msg {
        ControlMsg::Reload => {
            reload_and_broadcast(conn, ui_config, config_path, tx_keys, egui_ctx).await
        }
        ControlMsg::Shutdown => {
            // Handled in the event loop branches; no action here.
        }
        ControlMsg::SwitchTheme(name) => {
            if themes::theme_exists(&name) {
                // Theme override now lives on Location; the live location is updated
                // inside the event loop when HudUpdate arrives. Here we just request UI refresh.
                apply_ui_config(ui_config, tx_keys, egui_ctx).await;
            } else {
                if tx_keys
                    .send(AppEvent::Notify {
                        kind: NotifyKind::Error,
                        title: "Theme".to_string(),
                        text: format!("Theme '{}' not found", name),
                    })
                    .is_err()
                {
                    tracing::warn!("failed to send theme-not-found notification");
                }
                egui_ctx.request_repaint();
            }
        }
        ControlMsg::Notice { kind, title, text } => {
            if tx_keys
                .send(AppEvent::Notify { kind, title, text })
                .is_err()
            {
                tracing::warn!("failed to send notification");
            }
            egui_ctx.request_repaint();
        }
        ControlMsg::OpenAccessibility => {
            open_accessibility_settings();
            if tx_keys
                .send(AppEvent::Notify {
                    kind: NotifyKind::Info,
                    title: "Accessibility".to_string(),
                    text: "Opening Accessibility settings...".to_string(),
                })
                .is_err()
            {
                tracing::warn!("failed to send accessibility notice");
            }
            egui_ctx.request_repaint();
        }
        ControlMsg::OpenInputMonitoring => {
            open_input_monitoring_settings();
            if tx_keys
                .send(AppEvent::Notify {
                    kind: NotifyKind::Info,
                    title: "Input Monitoring".to_string(),
                    text: "Opening Input Monitoring settings...".to_string(),
                })
                .is_err()
            {
                tracing::warn!("failed to send input monitoring notice");
            }
            egui_ctx.request_repaint();
        }
        ControlMsg::OpenPermissionsHelp => {
            if tx_keys.send(AppEvent::ShowPermissionsHelp).is_err() {
                tracing::warn!("failed to send permissions help event");
            }
            egui_ctx.request_repaint();
        }
    }
}

#[allow(clippy::too_many_arguments)]
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
