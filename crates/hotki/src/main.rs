use std::path::PathBuf;

use clap::Parser;
use eframe::NativeOptions;
use hotki_server::Server;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::MainThreadMarker;
use tokio::sync::mpsc as tokio_mpsc;
use tracing::{debug, error};
use tracing_subscriber::prelude::*;

mod app;
mod details;
mod fonts;
mod hud;
mod logs;
mod notification;
mod permissions;
mod runtime;
mod tray;

use crate::{
    app::{AppEvent, HotkiApp},
    details::Details,
    hud::Hud,
    notification::NotificationCenter,
};
use config::{Config, default_config_path, load_from_path};

#[derive(Parser, Debug)]
#[command(name = "hotki", about = "A macOS hotkey application", version)]
struct Cli {
    /// Run as server (headless)
    #[arg(long)]
    server: bool,

    /// Socket path for server mode
    #[arg(long)]
    socket: Option<String>,

    /// Server idle timeout in seconds (when running with --server)
    #[arg(long, value_name = "SECS")]
    server_idle_timeout: Option<u64>,

    /// Set global log level to trace
    #[arg(long, conflicts_with_all = ["debug", "log_level", "log_filter"])]
    trace: bool,

    /// Set global log level to debug
    #[arg(long, conflicts_with_all = ["trace", "log_level", "log_filter"])]
    debug: bool,

    /// Set a single global log level (error|warn|info|debug|trace)
    #[arg(long)]
    log_level: Option<String>,

    /// Set a tracing filter directive (e.g. "hotkey_manager=trace,mac_hotkey=trace,info")
    #[arg(long)]
    log_filter: Option<String>,

    /// Optional path to the config file
    config: Option<String>,
}

fn main() -> eframe::Result<()> {
    let cli = Cli::parse();

    // Determine filter precedence: log_filter > trace/debug/log_level > env > info
    let chosen_filter = if let Some(spec) = &cli.log_filter {
        Some(spec.clone())
    } else if cli.trace {
        Some("trace".to_string())
    } else if cli.debug {
        Some("debug".to_string())
    } else {
        cli.log_level.clone()
    };

    // Base filter from CLI/env (default to info)
    let mut env_filter =
        match &chosen_filter {
            Some(spec) => tracing_subscriber::EnvFilter::try_new(spec.as_str())
                .unwrap_or_else(|_| "info".into()),
            None => tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        };

    // Suppress expected disconnect noise from mrpc when clients exit cleanly.
    // These appear as errors from target "mrpc::connection" on shutdown.
    // We disable that specific target to keep exits clean while preserving
    // all other logs and user-provided filtering.
    if let Ok(d) = "mrpc::connection=off".parse() {
        env_filter = env_filter.add_directive(d);
    }

    // Install a single subscriber for both client and server modes, combining:
    // - Env filter (from CLI or env)
    // - Compact fmt output (no time)
    // - Client log buffer layer (records UI-side logs)
    // - Server forward layer (no-op on client; forwards logs to UI when server is running)
    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().without_time())
        .with(crate::logs::client_layer())
        .with(log_forward::layer())
        .try_init();
    // Build a filter string for any auto-spawned server process so it inherits
    // the same level plus our extra directive to silence mrpc disconnect noise.
    let server_filter: String = {
        let base = chosen_filter
            .clone()
            .or_else(|| std::env::var("RUST_LOG").ok())
            .unwrap_or_else(|| "info".to_string());
        if base.contains("mrpc::connection") {
            base
        } else {
            format!("{},mrpc::connection=off", base)
        }
    };

    if cli.server {
        debug!("Starting server mode");
        let mut server = if let Some(path) = cli.socket {
            debug!("Using socket path: {}", path);
            Server::new().with_socket_path(path)
        } else {
            Server::new()
        };

        if let Some(secs) = cli.server_idle_timeout {
            server = server.with_idle_timeout_secs(secs);
        }

        if let Err(e) = server.run() {
            error!("Server exited with error: {}", e);
        }
        return Ok(());
    }

    // Load config via config module; explicit path overrides default
    let (app_cfg, config_path): (Config, PathBuf) = {
        let path = if let Some(cfg) = &cli.config {
            PathBuf::from(cfg)
        } else {
            default_config_path()
        };
        match load_from_path(&path) {
            Ok(cfg) => (cfg, path),
            Err(e) => {
                error!("{}", e.pretty());
                std::process::exit(1);
            }
        }
    };

    let options: NativeOptions = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_visible(false)
            .with_transparent(true),
        ..Default::default()
    };

    let (tx, rx) = tokio_mpsc::unbounded_channel::<AppEvent>();
    let (tx_ctrl, rx_ctrl) = tokio_mpsc::unbounded_channel();

    eframe::run_native(
        "hotki",
        options,
        Box::new(move |cc| {
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::Visible(false));
            if let Some(mtm) = MainThreadMarker::new() {
                let app = NSApplication::sharedApplication(mtm);
                app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            }

            fonts::install_fonts(&cc.egui_ctx);

            runtime::spawn_key_runtime(
                &app_cfg,
                config_path.as_path(),
                &tx,
                &cc.egui_ctx,
                &tx_ctrl,
                rx_ctrl,
                Some(server_filter.clone()),
            );

            let tray_icon =
                tray::build_tray_and_listeners(tx.clone(), tx_ctrl.clone(), cc.egui_ctx.clone());

            let root_cursor = config::Cursor::default();
            let n = app_cfg.notify_config(&root_cursor);
            let theme = n.theme();
            let notifications = NotificationCenter::new(&n);

            let mut details = Details::new(theme);
            details.set_config_path(Some(config_path.clone()));
            details.set_control_sender(tx_ctrl.clone());

            let mut permissions = permissions::PermissionsHelp::new();
            permissions.set_control_sender(tx_ctrl.clone());

            Ok(Box::new(HotkiApp {
                rx,
                _tray: Some(tray_icon),
                hud: Hud::new(&app_cfg.hud(&root_cursor)),
                notifications,
                details,
                permissions,
                config: app_cfg.clone(),
                last_cursor: root_cursor,
            }))
        }),
    )
}
