use std::{collections::VecDeque, path::Path, process::Command, thread};

use egui::Context;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Duration;
use tokio::time::Instant as TokioInstant;
use tokio::time::Sleep;
use tracing::{debug, error, info};

use crate::{app::AppEvent, logs, permissions::check_permissions};
use hotki_protocol::{MsgToUI, NotifyKind};
use hotki_server::Client;

/// Actions that adjust UI overrides on the current cursor (theme and user style).
#[derive(Debug, Clone)]
enum UiOverride {
    ThemeNext,
    ThemePrev,
    ThemeSet(String),
    UserStyle(config::Toggle),
}

/// Drives the MRPC connection for the UI: connect, process events, and apply config/overrides.
struct ConnectionDriver {
    // Static inputs
    config_path: std::path::PathBuf,
    server_log_filter: Option<String>,
    tx_keys: mpsc::UnboundedSender<AppEvent>,
    egui_ctx: Context,
    rx_ctrl: mpsc::UnboundedReceiver<ControlMsg>,
    tx_ctrl_runtime: mpsc::UnboundedSender<ControlMsg>,

    // Mutable session state
    ui_config: config::Config,
    current_cursor: config::Cursor,
}

impl ConnectionDriver {
    fn new(
        config_path: std::path::PathBuf,
        server_log_filter: Option<String>,
        tx_keys: mpsc::UnboundedSender<AppEvent>,
        egui_ctx: Context,
        rx_ctrl: mpsc::UnboundedReceiver<ControlMsg>,
        tx_ctrl_runtime: mpsc::UnboundedSender<ControlMsg>,
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
        }
    }

    /// Helper to send a UI notification.
    fn notify(&self, kind: NotifyKind, title: &str, text: &str) {
        let _ = self.tx_keys.send(AppEvent::Notify {
            kind,
            title: title.to_string(),
            text: text.to_string(),
        });
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
                let next = config::themes::get_next_theme(cur);
                self.current_cursor.set_theme(Some(next));
                let _ = self
                    .tx_keys
                    .send(AppEvent::UpdateCursor(self.current_cursor.clone()));
            }
            UiOverride::ThemePrev => {
                let cur = self
                    .current_cursor
                    .override_theme
                    .as_deref()
                    .unwrap_or("default");
                let prev = config::themes::get_prev_theme(cur);
                self.current_cursor.set_theme(Some(prev));
                let _ = self
                    .tx_keys
                    .send(AppEvent::UpdateCursor(self.current_cursor.clone()));
            }
            UiOverride::ThemeSet(name) => {
                if config::themes::theme_exists(&name) {
                    self.current_cursor.set_theme(Some(&name));
                    let _ = self
                        .tx_keys
                        .send(AppEvent::UpdateCursor(self.current_cursor.clone()));
                } else {
                    self.notify(NotifyKind::Error, "Theme", "Theme not found");
                }
            }
            UiOverride::UserStyle(tg) => {
                use config::Toggle as Tg;
                match tg {
                    Tg::On => self.current_cursor.set_user_style_enabled(true),
                    Tg::Off => self.current_cursor.set_user_style_enabled(false),
                    Tg::Toggle => self
                        .current_cursor
                        .set_user_style_enabled(!self.current_cursor.user_ui_disabled),
                }
                let _ = self
                    .tx_keys
                    .send(AppEvent::UpdateCursor(self.current_cursor.clone()));
            }
        }
        self.egui_ctx.request_repaint();
    }

    /// Background connect with a preconnect control-message queue. Returns an open connection.
    async fn connect(&mut self, initial_cfg: config::Config) -> Option<hotki_server::Client> {
        // Kick off server connect in background, but keep servicing control messages
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
                        let _ = tx_conn_ready.send(c);
                    }
                    Err(e) => {
                        tracing::error!("Failed to connect to hotkey server: {}", e);
                    }
                }
            });
            rx
        }

        // Show permissions help if either permission is missing
        let perms = check_permissions();
        if !perms.accessibility_ok || !perms.input_ok {
            let _ = self.tx_keys.send(AppEvent::ShowPermissionsHelp);
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
                            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                            rx_conn_ready = spawn_connect(self.server_log_filter.clone());
                        }
                    }
                }
                Some(msg) = self.rx_ctrl.recv() => {
                    match msg.clone() {
                        ControlMsg::Shutdown => {
                            // Ask UI to hide everything, allow a brief window to process, then exit
                            let _ = self.tx_keys.send(AppEvent::Shutdown);
                            self.egui_ctx.request_repaint();
                            tokio::time::sleep(Duration::from_millis(250)).await;
                            std::process::exit(0);
                        }
                        ControlMsg::Reload | ControlMsg::SwitchTheme(_) => {
                            preconnect_queue.push_back(msg);
                        }
                        ControlMsg::OpenAccessibility => {
                            open_accessibility_settings();
                            self.notify(NotifyKind::Info, "Accessibility", "Opening Accessibility settings...");
                        }
                        ControlMsg::OpenInputMonitoring => {
                            open_input_monitoring_settings();
                            self.notify(NotifyKind::Info, "Input Monitoring", "Opening Input Monitoring settings...");
                        }
                        ControlMsg::OpenPermissionsHelp => {
                            let _ = self.tx_keys.send(AppEvent::ShowPermissionsHelp);
                            self.egui_ctx.request_repaint();
                        }
                        ControlMsg::Notice { kind, title, text } => {
                            let _ = self.tx_keys.send(AppEvent::Notify { kind, title, text });
                            self.egui_ctx.request_repaint();
                        }
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

        // Apply any queued preconnect messages now that we are connected
        while let Some(msg) = preconnect_queue.pop_front() {
            handle_control_msg(
                conn,
                msg,
                &mut self.ui_config,
                &self.config_path,
                &self.tx_keys,
                &self.egui_ctx,
            )
            .await;
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
        let hb_timer: Sleep = tokio::time::sleep(hotki_protocol::ipc::heartbeat::timeout());
        tokio::pin!(hb_timer);

        loop {
            tokio::select! {
                biased;
                // If the heartbeat timer fires, assume the backend is gone and exit gracefully
                _ = &mut hb_timer => {
                    error!("No IPC activity within heartbeat timeout; exiting UI event loop");
                    break;
                }
                Some(msg) = self.rx_ctrl.recv() => {
                    match msg {
                        ControlMsg::Shutdown => {
                            let _ = self.tx_keys.send(AppEvent::Shutdown);
                            self.egui_ctx.request_repaint();
                            tokio::time::sleep(Duration::from_millis(250)).await;
                            std::process::exit(0);
                        }
                        ControlMsg::SwitchTheme(name) => {
                            // Theme override now lives on Location; update and refresh UI
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
                            ).await;
                        }
                    }
                }
                resp = conn.recv_event() => {
                    match resp {
                        Ok(MsgToUI::HudUpdate { cursor }) => {
                            // Any message indicates liveness; reset the heartbeat timer
                            hb_timer.as_mut().reset(TokioInstant::now() + hotki_protocol::ipc::heartbeat::timeout());
                            self.current_cursor = cursor.clone();
                            // Compute UI-facing fields from our Config using cursor context
                            let vks = self.ui_config.hud_keys_ctx(&self.current_cursor);
                            let visible_keys: Vec<(String, String, bool)> = vks
                                .into_iter()
                                .filter(|(_, _, attrs, _)| !attrs.hide())
                                .map(|(k, desc, _attrs, is_mode)| (k.to_string(), desc, is_mode))
                                .collect();
                            let depth = self.ui_config.depth(&self.current_cursor);
                            let parent_title = self.ui_config.parent_title(&self.current_cursor).map(|s| s.to_string());
                            let _ = self.tx_keys.send(AppEvent::KeyUpdate { visible_keys, depth, cursor: self.current_cursor.clone(), parent_title });
                            self.egui_ctx.request_repaint();
                        }
                        Ok(MsgToUI::Notify { kind, title, text }) => {
                            hb_timer.as_mut().reset(TokioInstant::now() + hotki_protocol::ipc::heartbeat::timeout());
                            let _ = self.tx_keys.send(AppEvent::Notify { kind, title, text });
                            self.egui_ctx.request_repaint();
                        }
                        Ok(MsgToUI::ReloadConfig) => {
                            hb_timer.as_mut().reset(TokioInstant::now() + hotki_protocol::ipc::heartbeat::timeout());
                            let _ = self.tx_ctrl_runtime.send(ControlMsg::Reload);
                            self.egui_ctx.request_repaint();
                        }
                        Ok(MsgToUI::ClearNotifications) => {
                            hb_timer.as_mut().reset(TokioInstant::now() + hotki_protocol::ipc::heartbeat::timeout());
                            let _ = self.tx_keys.send(AppEvent::ClearNotifications);
                            self.egui_ctx.request_repaint();
                        }
                        Ok(MsgToUI::ShowDetails(arg)) => {
                            hb_timer.as_mut().reset(TokioInstant::now() + hotki_protocol::ipc::heartbeat::timeout());
                            use config::Toggle as Tg;
                            match arg {
                                Tg::On => { let _ = self.tx_keys.send(AppEvent::ShowDetails); }
                                Tg::Off => { let _ = self.tx_keys.send(AppEvent::HideDetails); }
                                Tg::Toggle => { let _ = self.tx_keys.send(AppEvent::ToggleDetails); }
                            }
                            self.egui_ctx.request_repaint();
                        }
                        Ok(MsgToUI::ThemeNext) => {
                            hb_timer.as_mut().reset(TokioInstant::now() + hotki_protocol::ipc::heartbeat::timeout());
                            self.apply_ui_override(UiOverride::ThemeNext);
                        }
                        Ok(MsgToUI::ThemePrev) => {
                            hb_timer.as_mut().reset(TokioInstant::now() + hotki_protocol::ipc::heartbeat::timeout());
                            self.apply_ui_override(UiOverride::ThemePrev);
                        }
                        Ok(MsgToUI::ThemeSet(name)) => {
                            hb_timer.as_mut().reset(TokioInstant::now() + hotki_protocol::ipc::heartbeat::timeout());
                            self.apply_ui_override(UiOverride::ThemeSet(name));
                        }
                        Ok(MsgToUI::UserStyle(arg)) => {
                            hb_timer.as_mut().reset(TokioInstant::now() + hotki_protocol::ipc::heartbeat::timeout());
                            self.apply_ui_override(UiOverride::UserStyle(arg));
                        }
                        Ok(MsgToUI::HotkeyTriggered(_)) => {}
                        Ok(MsgToUI::Log { level, target, message }) => {
                            hb_timer.as_mut().reset(TokioInstant::now() + hotki_protocol::ipc::heartbeat::timeout());
                            logs::push_server(level, target, message);
                            self.egui_ctx.request_repaint();
                        }
                        Ok(MsgToUI::Heartbeat(_)) => {
                            // Liveness tick; reset timer and do nothing else.
                            hb_timer.as_mut().reset(TokioInstant::now() + hotki_protocol::ipc::heartbeat::timeout());
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
            }
        }
        info!("Exiting key loop");
        // Terminate the process so any HUD windows are closed immediately.
        // Relying on UI message processing can stall if the UI isn't repainting.
        std::process::exit(0);
    }
}

#[derive(Debug, Clone)]
pub enum ControlMsg {
    /// Reload from disk using `config_path`
    Reload,
    OpenAccessibility,
    OpenInputMonitoring,
    /// Gracefully shut down the UI and exit the process
    Shutdown,
    /// Request a theme switch by name (handled here on the live Config)
    SwitchTheme(String),
    OpenPermissionsHelp,
    /// Forward a user-facing notice into the app UI
    Notice {
        kind: NotifyKind,
        title: String,
        text: String,
    },
}

pub(crate) fn open_accessibility_settings() {
    let _ = Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

pub(crate) fn open_input_monitoring_settings() {
    let _ = Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
        .spawn();
}

// Apply the current UI config: notify UI to reload and send config to server, then repaint
async fn apply_ui_config(
    ui_config: &config::Config,
    tx_keys: &mpsc::UnboundedSender<AppEvent>,
    egui_ctx: &Context,
) {
    // UI refresh request; sending config to server only necessary when config changed.
    let _ = tx_keys.send(AppEvent::ReloadUI(Box::new(ui_config.clone())));
    egui_ctx.request_repaint();
}

// Single-source reload: load from disk, apply to UI + server, and notify success or error.
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
            let _ = tx_keys.send(AppEvent::Notify {
                kind: NotifyKind::Success,
                title: "Config".to_string(),
                text: "Reloaded successfully".to_string(),
            });
            // For reload, push the new config to the server engine, then refresh UI
            let _ = conn.set_config(ui_config.clone()).await;
            apply_ui_config(ui_config, tx_keys, egui_ctx).await;
        }
        Err(e) => {
            let _ = tx_keys.send(AppEvent::Notify {
                kind: NotifyKind::Error,
                title: "Config".to_string(),
                text: e.pretty(),
            });
            egui_ctx.request_repaint();
        }
    }
}

// Unified handler for all ControlMsg variants once a connection exists.
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
            if config::themes::theme_exists(&name) {
                // Theme override now lives on Location; the live location is updated
                // inside the event loop when HudUpdate arrives. Here we just request UI refresh.
                apply_ui_config(ui_config, tx_keys, egui_ctx).await;
            } else {
                let _ = tx_keys.send(AppEvent::Notify {
                    kind: NotifyKind::Error,
                    title: "Theme".to_string(),
                    text: format!("Theme '{}' not found", name),
                });
                egui_ctx.request_repaint();
            }
        }
        ControlMsg::Notice { kind, title, text } => {
            let _ = tx_keys.send(AppEvent::Notify { kind, title, text });
            egui_ctx.request_repaint();
        }
        ControlMsg::OpenAccessibility => {
            open_accessibility_settings();
            let _ = tx_keys.send(AppEvent::Notify {
                kind: NotifyKind::Info,
                title: "Accessibility".to_string(),
                text: "Opening Accessibility settings...".to_string(),
            });
            egui_ctx.request_repaint();
        }
        ControlMsg::OpenInputMonitoring => {
            open_input_monitoring_settings();
            let _ = tx_keys.send(AppEvent::Notify {
                kind: NotifyKind::Info,
                title: "Input Monitoring".to_string(),
                text: "Opening Input Monitoring settings...".to_string(),
            });
            egui_ctx.request_repaint();
        }
        ControlMsg::OpenPermissionsHelp => {
            let _ = tx_keys.send(AppEvent::ShowPermissionsHelp);
            egui_ctx.request_repaint();
        }
    }
}

pub fn spawn_key_runtime(
    cfg: &config::Config,
    config_path: &Path,
    tx_keys: &mpsc::UnboundedSender<AppEvent>,
    egui_ctx: &Context,
    tx_ctrl_runtime: &mpsc::UnboundedSender<ControlMsg>,
    rx_ctrl: mpsc::UnboundedReceiver<ControlMsg>,
    server_log_filter: Option<String>,
) {
    // Take cheap clones here to own values in the thread
    let cfg = cfg.clone();
    let config_path = config_path.to_path_buf();
    let tx_keys = tx_keys.clone();
    let egui_ctx = egui_ctx.clone();
    let tx_ctrl_runtime = tx_ctrl_runtime.clone();
    thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async move {
            info!("Loaded mode; delegating to server engine");
            let mut driver = ConnectionDriver::new(
                config_path,
                server_log_filter,
                tx_keys,
                egui_ctx,
                rx_ctrl,
                tx_ctrl_runtime,
            );
            if let Some(mut client) = driver.connect(cfg).await {
                driver.drive_events(&mut client).await;
            }
        });
    });
}
