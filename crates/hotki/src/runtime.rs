use std::{collections::VecDeque, path::Path, process::Command, thread};

use egui::Context;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info};

use crate::{app::AppEvent, logs, permissions::check_permissions};
use hotki_protocol::{MsgToUI, NotifyKind};
use hotki_server::Client;

#[derive(Debug, Clone)]
pub enum ControlMsg {
    /// Reload from disk using `config_path`
    Reload,
    OpenAccessibility,
    OpenInputMonitoring,
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
        let mut rx_ctrl = rx_ctrl;
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async move {
            info!("Loaded mode; delegating to server engine");

            // Kick off server connect in background, but keep servicing control messages
            fn spawn_connect(log_filter: Option<String>) -> oneshot::Receiver<hotki_server::Client> {
                let (tx_conn_ready, rx) = oneshot::channel::<hotki_server::Client>();
                tokio::spawn(async move {
                    let client = if let Some(f) = log_filter.clone() {
                        Client::new().with_auto_spawn_server().with_server_log_filter(f)
                    } else {
                        Client::new().with_auto_spawn_server()
                    };
                    match client.connect().await {
                        Ok(c) => { let _ = tx_conn_ready.send(c); }
                        Err(e) => { tracing::error!("Failed to connect to hotkey server: {}", e); }
                    }
                });
                rx
            }
            let mut rx_conn_ready = spawn_connect(server_log_filter.clone());

            // Show permissions help if either permission is missing
            let perms = check_permissions();
            if !perms.accessibility_ok || !perms.input_ok {
                let _ = tx_keys.send(AppEvent::ShowPermissionsHelp);
                egui_ctx.request_repaint();
            }

            // Single driver loop while connecting: queue connection-dependent messages
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
                                rx_conn_ready = spawn_connect(server_log_filter.clone());
                            }
                        }
                    }
                    Some(msg) = rx_ctrl.recv() => {
                        match msg {
                            ControlMsg::Reload | ControlMsg::SwitchTheme(_) => {
                                preconnect_queue.push_back(msg);
                            }
                            other => {
                                // Handle connection-independent messages immediately
                                match other {
                                    ControlMsg::OpenAccessibility => {
                                        open_accessibility_settings();
                                        let _ = tx_keys.send(AppEvent::Notify {
                                            kind: NotifyKind::Info,
                                            title: "Accessibility".to_string(),
                                            text: "Opening Accessibility settings...".to_string(),
                                        });
                                    }
                                    ControlMsg::OpenInputMonitoring => {
                                        open_input_monitoring_settings();
                                        let _ = tx_keys.send(AppEvent::Notify {
                                            kind: NotifyKind::Info,
                                            title: "Input Monitoring".to_string(),
                                            text: "Opening Input Monitoring settings...".to_string(),
                                        });
                                    }
                                    ControlMsg::OpenPermissionsHelp => {
                                        let _ = tx_keys.send(AppEvent::ShowPermissionsHelp);
                                    }
                                    ControlMsg::Notice { kind, title, text } => {
                                        let _ = tx_keys.send(AppEvent::Notify { kind, title, text });
                                    }
                                    _ => {}
                                }
                                egui_ctx.request_repaint();
                            }
                        }
                    }
                }
            };
            let conn = match client.connection() {
                Ok(c) => c,
                Err(e) => { error!("Failed to get client connection: {}", e); return; }
            };
            if let Err(e) = conn.set_config(cfg).await {
                error!("Failed to set config on server: {}", e);
                return;
            }
            info!("Config sent to server engine");

            // Load UI Config once; on Reload events the UI will refresh independently.
            let mut ui_config = match config::load_from_path(&config_path) {
                Ok(c) => c,
                Err(e) => {
                    error!("Failed to load UI config: {}", e.pretty());
                    config::Config::default()
                }
            };
            // Track current location for theme/user-style overrides now carried on Location
            let mut current_cursor = config::Cursor::default();
            // Apply any queued preconnect messages now that we are connected
            while let Some(msg) = preconnect_queue.pop_front() {
                handle_control_msg(conn, msg, &mut ui_config, &config_path, &tx_keys, &egui_ctx)
                    .await;
            }

            loop {

                tokio::select! {
                    biased;
                    Some(msg) = rx_ctrl.recv() => {
                        match msg {
                            ControlMsg::SwitchTheme(name) => {
                                if config::themes::theme_exists(&name) {
                                    current_cursor.set_theme(Some(&name));
                                    let _ = tx_keys.send(AppEvent::UpdateCursor(current_cursor.clone()));
                                } else {
                                    let _ = tx_keys.send(AppEvent::Notify { kind: NotifyKind::Error, title: "Theme".into(), text: "Theme not found".into() });
                                }
                                egui_ctx.request_repaint();
                            }
                            other => {
                                handle_control_msg(conn, other, &mut ui_config, &config_path, &tx_keys, &egui_ctx).await;
                            }
                        }
                    }
                    resp = conn.recv_event() => {
                        match resp {
                            Ok(MsgToUI::HudUpdate { cursor, focus }) => {
                                current_cursor = cursor.clone();
                                // Compute UI-facing fields from our Config
                                let vks = ui_config.hud_keys(&current_cursor, &focus.app, &focus.title);
                                let visible_keys: Vec<(String, String, bool)> = vks
                                    .into_iter()
                                    .filter(|(_, _, attrs, _)| !attrs.hide())
                                    .map(|(k, desc, _attrs, is_mode)| (k.to_string(), desc, is_mode))
                                    .collect();
                                let depth = ui_config.depth(&current_cursor);
                                let parent_title = ui_config.parent_title(&current_cursor).map(|s| s.to_string());
                                let _ = tx_keys.send(AppEvent::KeyUpdate { visible_keys, depth, cursor: current_cursor.clone(), parent_title });
                                egui_ctx.request_repaint();
                            }
                            Ok(MsgToUI::Notify { kind, title, text }) => {
                                let _ = tx_keys.send(AppEvent::Notify { kind, title, text });
                                egui_ctx.request_repaint();
                            }
                            Ok(MsgToUI::ReloadConfig) => {
                                let _ = tx_ctrl_runtime.send(ControlMsg::Reload);
                                egui_ctx.request_repaint();
                            }
                            Ok(MsgToUI::ClearNotifications) => {
                                let _ = tx_keys.send(AppEvent::ClearNotifications);
                                egui_ctx.request_repaint();
                            }
                            Ok(MsgToUI::ToggleDetails) => {
                                let _ = tx_keys.send(AppEvent::ToggleDetails);
                                egui_ctx.request_repaint();
                            }
                            Ok(MsgToUI::ThemeNext) => {
                                let current = current_cursor.override_theme.as_deref().unwrap_or("default");
                                let next = config::themes::get_next_theme(current);
                                current_cursor.set_theme(Some(next));
                                let _ = tx_keys.send(AppEvent::UpdateCursor(current_cursor.clone()));
                                egui_ctx.request_repaint();
                            }
                            Ok(MsgToUI::ThemePrev) => {
                                let current = current_cursor.override_theme.as_deref().unwrap_or("default");
                                let prev = config::themes::get_prev_theme(current);
                                current_cursor.set_theme(Some(prev));
                                let _ = tx_keys.send(AppEvent::UpdateCursor(current_cursor.clone()));
                                egui_ctx.request_repaint();
                            }
                            Ok(MsgToUI::ThemeSet(name)) => {
                                if config::themes::theme_exists(&name) {
                                    current_cursor.set_theme(Some(&name));
                                    let _ = tx_keys.send(AppEvent::UpdateCursor(current_cursor.clone()));
                                } else {
                                    let _ = tx_keys.send(AppEvent::Notify {
                                        kind: NotifyKind::Error,
                                        title: "Theme".to_string(),
                                        text: "Theme not found".to_string(),
                                    });
                                }
                                egui_ctx.request_repaint();
                            }
                            Ok(MsgToUI::UserStyle(arg)) => {
                                use config::Toggle as Tg;
                                match arg {
                                    Tg::On => current_cursor.set_user_style_enabled(true),
                                    Tg::Off => current_cursor.set_user_style_enabled(false),
                                    Tg::Toggle => current_cursor
                                        .set_user_style_enabled(!current_cursor.user_ui_disabled),
                                }
                                let _ = tx_keys.send(AppEvent::UpdateCursor(current_cursor.clone()));
                                egui_ctx.request_repaint();
                            }
                            Ok(MsgToUI::HotkeyTriggered(_)) => {}
                            Ok(MsgToUI::Log { level, target, message }) => {
                                logs::push_server(level, target, message);
                                egui_ctx.request_repaint();
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
        });
    });
}
