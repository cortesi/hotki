#![deny(clippy::disallowed_methods)]
//! Binary entrypoint for the Hotki macOS app.
use std::{
    path::{Path, PathBuf},
    process,
};

use clap::{Parser, Subcommand};
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

use config::{
    check_luau_config, load_dynamic_config, luau_api_markdown, luau_api_text, resolve_config_path,
    script::engine::{ModeCtx, ModeFrame, render_stack},
    themes,
};

use crate::app::{AppBootstrap, HotkiApp, UiEvent};

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

#[derive(Subcommand, Debug)]
/// Top-level CLI subcommands.
enum Command {
    /// Print the checked-in Luau scripting API definitions.
    Api {
        /// Wrap the output in a markdown code fence.
        #[arg(long)]
        markdown: bool,

        /// Restrict output to definition blocks matching this substring.
        #[arg(long, value_name = "TEXT")]
        filter: Option<String>,
    },
    /// Load and validate the configuration then exit.
    Check {
        /// Path to configuration file to check (defaults to ~/.hotki/config.luau)
        path: Option<String>,

        /// Dump the parsed configuration as JSON to stdout
        #[arg(long)]
        dump: bool,
    },
}

fn main() -> eframe::Result<()> {
    let cli = Cli::parse();
    themes::init_builtins();
    let server_filter = init_logging(&cli);

    if handle_subcommand(&cli) {
        return Ok(());
    }
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

/// Handle command-line subcommands that do not launch the UI.
fn handle_subcommand(cli: &Cli) -> bool {
    if let Some(command) = &cli.command {
        match command {
            Command::Api { markdown, filter } => {
                let output = if *markdown {
                    luau_api_markdown(filter.as_deref())
                } else {
                    luau_api_text(filter.as_deref())
                };
                print!("{output}");
                return true;
            }
            Command::Check { path, dump } => {
                run_check_command(path.as_deref(), cli.config.as_deref(), *dump);
                return true;
            }
        }
    }
    false
}

/// Validate a config file and optionally dump its resolved style.
fn run_check_command(path: Option<&str>, cli_config: Option<&Path>, dump: bool) {
    let explicit = path.map(Path::new).or(cli_config);
    let resolved = match resolve_config_path(explicit) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e.pretty());
            process::exit(1);
        }
    };
    let report = match check_luau_config(&resolved) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("{}", e.pretty());
            process::exit(1);
        }
    };
    if dump {
        dump_config_style(&resolved);
    } else {
        println!(
            "OK (validated {} imports, {} theme files)",
            report.imports, report.themes
        );
    }
}

/// Dump the base style for a validated config.
fn dump_config_style(resolved: &Path) {
    match load_dynamic_config(resolved) {
        Ok(cfg) => {
            let style = cfg.base_style(None);
            match serde_json::to_string_pretty(&style) {
                Ok(json) => println!("{json}"),
                Err(e) => {
                    eprintln!("Failed to serialize style: {e}");
                    process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("{}", e.pretty());
            process::exit(1);
        }
    }
}

/// Run hotki as the background server.
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
    let mut cfg = match load_dynamic_config(config_path) {
        Ok(cfg) => cfg,
        Err(err) => {
            error!("failed to load initial UI style: {}", err.pretty());
            return hotki_protocol::Style::default();
        }
    };
    let base_style = cfg.base_style(None);
    let mut stack = vec![ModeFrame {
        title: "root".to_string(),
        closure: cfg.root(),
        entered_via: None,
        rendered: Vec::new(),
        style: None,
        capture: false,
    }];
    let ctx = ModeCtx {
        app: String::new(),
        title: String::new(),
        pid: 0,
        hud: false,
        depth: 0,
    };

    match render_stack(&mut cfg, &mut stack, &ctx, &base_style) {
        Ok(output) => output.rendered.style,
        Err(err) => {
            error!("failed to render initial UI style: {}", err.pretty());
            base_style
        }
    }
}
