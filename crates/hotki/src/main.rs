#![deny(clippy::disallowed_methods)]
//! Binary entrypoint for the Hotki macOS app.
use std::{
    path::{Path, PathBuf},
    process,
};

use clap::{Parser, Subcommand};
use eframe::NativeOptions;
use hotki_server::Server;
use logging::{self as logshared, forward};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::MainThreadMarker;
use tokio::sync::mpsc as tokio_mpsc;
use tracing::{debug, error};
use tracing_subscriber::{fmt, prelude::*};

use crate::logs::client_layer;

/// Application state and event wiring.
mod app;
/// MRPC connection driver for the UI runtime.
mod connection_driver;
/// Control message types shared between UI and runtime.
mod control;
/// Details window (notifications/config/logs/about).
mod details;
/// Display geometry helpers.
mod display;
mod fonts;
mod hud;
mod logs;
mod notification;
mod nswindow;
/// Permissions UI helpers and checks.
mod permissions;
/// Background UI runtime glue (server connection + event loop).
mod runtime;
/// UI-side smoketest bridge listener and queue.
mod smoketest_bridge;
mod tray;

use config::{load_dynamic_config, resolve_config_path, themes};

use crate::{
    app::{AppEvent, HotkiApp},
    details::Details,
    display::DisplayMetrics,
    hud::Hud,
    notification::NotificationCenter,
};

#[derive(Parser, Debug)]
#[command(name = "hotki", about = "A macOS hotkey application", version)]
/// Command-line interface for the `hotki` binary.
struct Cli {
    /// Optional subcommand.
    #[command(subcommand)]
    command: Option<Command>,

    /// Run as server (headless)
    #[arg(long)]
    server: bool,

    /// Socket path for server mode
    #[arg(long)]
    socket: Option<String>,

    /// Server idle timeout in seconds (when running with --server)
    #[arg(long, value_name = "SECS")]
    server_idle_timeout: Option<u64>,

    /// Logging controls
    #[command(flatten)]
    log: logshared::LogArgs,

    /// Optional path to the config file
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Periodically dump a formatted world snapshot to logs (every ~5s)
    #[arg(long)]
    dumpworld: bool,
}

#[derive(Subcommand, Debug)]
/// Top-level CLI subcommands.
enum Command {
    /// Load and validate the configuration then exit.
    Check {
        /// Path to configuration file to check (defaults to ~/.hotki/config.rhai)
        path: Option<String>,

        /// Dump the parsed configuration as JSON to stdout
        #[arg(long)]
        dump: bool,
    },
}

fn main() -> eframe::Result<()> {
    let cli = Cli::parse();
    themes::init_builtins();

    // Compute final filter spec via shared helpers
    let final_spec: String = logshared::compute_spec(
        cli.log.trace,
        cli.log.debug,
        cli.log.log_level.as_deref(),
        cli.log.log_filter.as_deref(),
    );

    // Create EnvFilter from final spec
    let env_filter = logshared::env_filter_from_spec(&final_spec);

    // Install a single subscriber for both client and server modes, combining:
    // - Env filter (from CLI or env)
    // - Compact fmt output (no time)
    // - Client log buffer layer (records UI-side logs)
    // - Server forward layer (no-op on client; forwards logs to UI when server is running)
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().without_time())
        .with(client_layer())
        .with(forward::layer())
        .try_init()
        .ok();
    // Build a filter string for any auto-spawned server process so it inherits
    // the same level plus our extra directive to silence mrpc disconnect noise.
    let server_filter: String = final_spec;

    if let Some(Command::Check { path, dump }) = &cli.command {
        let explicit = path.as_deref().map(Path::new).or(cli.config.as_deref());
        let resolved = match resolve_config_path(explicit) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{}", e.pretty());
                process::exit(1);
            }
        };
        match load_dynamic_config(&resolved) {
            Ok(cfg) => {
                if *dump {
                    let style = cfg.base_style(None);
                    match serde_json::to_string_pretty(&style) {
                        Ok(json) => println!("{json}"),
                        Err(e) => {
                            eprintln!("Failed to serialize style: {e}");
                            process::exit(1);
                        }
                    }
                } else {
                    println!("OK");
                }
                return Ok(());
            }
            Err(e) => {
                eprintln!("{}", e.pretty());
                process::exit(1);
            }
        }
    }

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

    // Resolve config path (server loads and validates after connect).
    let config_path: PathBuf = {
        let explicit = cli.config.as_deref();
        match resolve_config_path(explicit) {
            Ok(p) => p,
            Err(e) => {
                error!("{}", e.pretty());
                process::exit(1);
            }
        }
    };
    let initial_style = hotki_protocol::Style::default();

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
                config_path.as_path(),
                &tx,
                &cc.egui_ctx,
                &tx_ctrl,
                rx_ctrl,
                Some(server_filter.clone()),
                cli.dumpworld,
            );

            let tray_icon = tray::build_tray_and_listeners(&tx, &tx_ctrl, &cc.egui_ctx);

            let mut notifications = NotificationCenter::new(&initial_style.notify);

            let mut details = Details::new(initial_style.notify.theme.clone());
            details.set_config_path(Some(config_path.clone()));
            details.set_control_sender(tx_ctrl.clone());

            let mut permissions = permissions::PermissionsHelp::new();
            permissions.set_control_sender(tx_ctrl.clone());

            let metrics = DisplayMetrics::default();
            let mut hud = Hud::new(&initial_style.hud);
            hud.set_display_metrics(metrics.clone());
            notifications.set_display_metrics(metrics.clone());
            details.set_display_metrics(metrics.clone());

            Ok(Box::new(HotkiApp {
                rx,
                _tray: tray_icon,
                hud,
                notifications,
                details,
                permissions,
                shutdown_in_progress: false,
                display_metrics: metrics,
            }))
        }),
    )
}
