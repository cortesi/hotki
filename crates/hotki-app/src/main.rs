#![deny(clippy::disallowed_methods)]
//! Binary entrypoint for the Hotki macOS app.
use std::{
    path::{Path, PathBuf},
    process,
};

use clap::Parser;
use eframe::{NativeOptions, Renderer};
use hotki_server::Server;
use logging::{self as logshared, forward};
use tokio::sync::mpsc as tokio_mpsc;
use tracing::{debug, error};
use tracing_subscriber::{fmt, prelude::*};

use crate::logs::client_layer;

/// Application state and event wiring.
mod app;
/// MRPC connection driver for the UI runtime.
mod connection_driver;
/// Details window (notifications/config/logs/about).
mod details;
mod devtools;
/// Display geometry helpers.
mod display;
mod fonts;
mod hud;
mod logs;
mod notification;
mod nswindow;
/// Shared viewport mechanics for overlay windows.
mod overlay;
/// Permissions UI helpers and checks.
mod permissions;
/// Background UI runtime glue (server connection + event loop).
mod runtime;
mod selector;
mod tray;

use config::{load_dynamic_config, resolve_config_path};

use crate::app::{AppBootstrap, HotkiApp, UiEvent};

#[derive(Parser, Debug)]
#[command(name = "hotki-app", about = "Hotki macOS app runtime", version)]
/// Command-line interface for the `hotki-app` binary.
struct Cli {
    /// Run as server (headless)
    #[arg(long, hide = true)]
    server: bool,

    /// Socket path for server mode
    #[arg(long, hide = true)]
    socket: Option<String>,

    /// Server idle timeout in seconds (when running with --server)
    #[arg(long, value_name = "SECS", hide = true)]
    server_idle_timeout: Option<u64>,

    /// Parent PID to watch (server mode; shut down when this process exits)
    #[arg(long, value_name = "PID", hide = true)]
    parent_pid: Option<i32>,

    /// Logging controls
    #[command(flatten)]
    log: logshared::LogArgs,

    /// Optional path to the config file
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Periodically dump a formatted world snapshot to logs (every ~5s)
    #[arg(long)]
    dumpworld: bool,

    /// Disable the physical keyboard event tap for RPC-driven harnesses.
    #[arg(long, hide = true)]
    disable_event_tap: bool,

    /// Enable the embedded eguidev MCP runtime.
    #[arg(long, hide = true)]
    dev_mcp: bool,
}

fn main() -> eframe::Result<()> {
    let cli = Cli::parse();
    let server_filter = init_logging(&cli);

    if cli.server {
        run_server_mode(&cli);
        return Ok(());
    }
    run_ui_mode(&cli, server_filter)
}

/// Initialize tracing and return the filter string for an auto-spawned server.
fn init_logging(cli: &Cli) -> String {
    let final_spec = logshared::compute_spec(
        cli.log.trace,
        cli.log.debug,
        cli.log.log_level.as_deref(),
        cli.log.log_filter.as_deref(),
    );
    let env_filter = logshared::env_filter_from_spec(&final_spec);
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().without_time())
        .with(client_layer())
        .with(forward::layer())
        .try_init()
        .ok();
    final_spec
}

/// Run the app binary as the background server.
fn run_server_mode(cli: &Cli) {
    debug!("Starting server mode");
    let server = server_from_cli(cli);
    if let Err(e) = server.run() {
        error!("Server exited with error: {}", e);
        process::exit(1);
    }
}

/// Build a server from command-line options.
fn server_from_cli(cli: &Cli) -> Server {
    let mut server = server_with_socket(cli.socket.as_deref());
    if let Some(secs) = cli.server_idle_timeout {
        server = server.with_idle_timeout_secs(secs);
    }
    if let Some(pid) = cli.parent_pid {
        server = server.with_parent_pid(pid);
    }
    if cli.disable_event_tap {
        server = server.without_event_tap();
    }
    server
}

/// Create a server with an optional explicit socket path.
fn server_with_socket(socket: Option<&str>) -> Server {
    if let Some(path) = socket {
        debug!("Using socket path: {}", path);
        Server::new().with_socket_path(path)
    } else {
        Server::new()
    }
}

/// Run the Hotki egui application.
fn run_ui_mode(cli: &Cli, server_filter: String) -> eframe::Result<()> {
    let config_path = match resolve_config_path(cli.config.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            error!("{}", e.pretty());
            process::exit(1);
        }
    };
    let initial_style = initial_style_for_config(&config_path);
    let (tx, rx) = tokio_mpsc::unbounded_channel::<UiEvent>();
    let (tx_ctrl, rx_ctrl) = tokio_mpsc::unbounded_channel();

    let (devmcp, fixture_runtime) =
        match devtools::build_devmcp(cli.dev_mcp, tx.clone(), tx_ctrl.clone()) {
            Ok(devmcp) => devmcp,
            Err(message) => {
                eprintln!("{message}");
                process::exit(1);
            }
        };

    let mut options: NativeOptions = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_visible(false)
            .with_transparent(true)
            .with_decorations(false),
        ..Default::default()
    };
    if cli.dev_mcp {
        options.renderer = Renderer::Glow;
        options.viewport = options
            .viewport
            .with_visible(true)
            .with_inner_size(egui::vec2(1.0, 1.0));
    }
    let server_event_tap_enabled = !cli.disable_event_tap;
    let dumpworld = cli.dumpworld;

    eframe::run_native(
        "hotki",
        options,
        Box::new(move |cc| {
            Ok(Box::new(HotkiApp::new(
                cc,
                AppBootstrap {
                    rx,
                    tx_ui: tx,
                    tx_ctrl,
                    rx_ctrl,
                    config_path: config_path.clone(),
                    initial_style: initial_style.clone(),
                    server_log_filter: Some(server_filter.clone()),
                    server_event_tap_enabled,
                    dumpworld,
                    devmcp: devmcp.clone(),
                    fixture_runtime,
                },
            )))
        }),
    )
}

/// Render the root config style once so UI-local startup notices match user configuration.
fn initial_style_for_config(config_path: &Path) -> hotki_protocol::Style {
    let cfg = match load_dynamic_config(config_path) {
        Ok(cfg) => cfg,
        Err(err) => {
            error!("failed to load initial UI style: {}", err.pretty());
            return hotki_protocol::Style::default();
        }
    };
    cfg.base_style()
}
