#![deny(clippy::disallowed_methods)]
//! Command-line entrypoint for Hotki tooling.

use std::{
    path::{Path, PathBuf},
    process,
};

use clap::{Parser, Subcommand, ValueEnum};
use config::{
    LuauApiSurface, check_luau_config, default_style_source, load_dynamic_config,
    luau_api_markdown, luau_api_text, resolve_config_path,
};
use tracing_subscriber::{fmt, prelude::*};

#[derive(Parser, Debug)]
#[command(
    name = "hotki",
    about = "Hotki command-line tools",
    version,
    arg_required_else_help = true
)]
/// Command-line interface for Hotki configuration and scripting tools.
struct Cli {
    /// Command to run.
    #[command(subcommand)]
    command: Command,

    /// Optional path to the config file.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Logging controls.
    #[command(flatten)]
    log: logging::LogArgs,
}

#[derive(Subcommand, Debug)]
/// Top-level CLI subcommands.
enum Command {
    /// Print the checked-in Luau scripting API definitions.
    Api {
        /// Which Luau API surface to print.
        #[arg(long, value_enum, default_value_t = ApiSurface::Config)]
        surface: ApiSurface,

        /// Wrap the output in a markdown code fence.
        #[arg(long)]
        markdown: bool,

        /// Restrict output to definition blocks matching this substring.
        #[arg(long, value_name = "TEXT")]
        filter: Option<String>,
    },
    /// Load and validate the configuration then exit.
    Check {
        /// Path to configuration file to check (defaults to ~/.hotki/config.luau).
        path: Option<String>,

        /// Dump the parsed configuration as JSON to stdout.
        #[arg(long)]
        dump: bool,
    },
    /// Print style source or the resolved effective style.
    Style {
        /// Path to configuration file (defaults to ~/.hotki/config.luau).
        path: Option<String>,

        /// Dump the embedded default style source.
        #[arg(long)]
        default: bool,
    },
}

/// Luau API surfaces exposed by `hotki api`.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum ApiSurface {
    /// Behavior config declarations.
    Config,
    /// Standalone `style.luau` declarations.
    Style,
    /// Combined declarations for tooling.
    All,
}

impl From<ApiSurface> for LuauApiSurface {
    fn from(surface: ApiSurface) -> Self {
        match surface {
            ApiSurface::Config => Self::Config,
            ApiSurface::Style => Self::Style,
            ApiSurface::All => Self::All,
        }
    }
}

fn main() {
    let cli = Cli::parse();
    init_logging(&cli);
    match &cli.command {
        Command::Api {
            surface,
            markdown,
            filter,
        } => run_api_command(*surface, *markdown, filter.as_deref()),
        Command::Check { path, dump } => {
            run_check_command(path.as_deref(), cli.config.as_deref(), *dump);
        }
        Command::Style { path, default } => {
            run_style_command(path.as_deref(), cli.config.as_deref(), *default);
        }
    }
}

/// Initialize tracing for command-line diagnostics.
fn init_logging(cli: &Cli) {
    let final_spec = logging::compute_spec(
        cli.log.trace,
        cli.log.debug,
        cli.log.log_level.as_deref(),
        cli.log.log_filter.as_deref(),
    );
    tracing_subscriber::registry()
        .with(logging::env_filter_from_spec(&final_spec))
        .with(fmt::layer().without_time())
        .try_init()
        .ok();
}

/// Print one Luau API surface.
fn run_api_command(surface: ApiSurface, markdown: bool, filter: Option<&str>) {
    let output = if markdown {
        luau_api_markdown(surface.into(), filter)
    } else {
        luau_api_text(surface.into(), filter)
    };
    print!("{output}");
}

/// Validate a config file and optionally dump its resolved style.
fn run_check_command(path: Option<&str>, cli_config: Option<&Path>, dump: bool) {
    let resolved = resolve_cli_config_path(path, cli_config);
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
            "OK (validated {} imports, style: {})",
            report.imports, report.style
        );
    }
}

/// Dump the embedded default style source or the resolved effective style.
fn run_style_command(path: Option<&str>, cli_config: Option<&Path>, default: bool) {
    if default {
        print!("{}", default_style_source());
        return;
    }
    let resolved = resolve_cli_config_path(path, cli_config);
    dump_config_style(&resolved);
}

/// Resolve a config path from command-specific and global CLI options.
fn resolve_cli_config_path(path: Option<&str>, cli_config: Option<&Path>) -> PathBuf {
    let explicit = path.map(Path::new).or(cli_config);
    match resolve_config_path(explicit) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("{}", e.pretty());
            process::exit(1);
        }
    }
}

/// Dump the resolved style for a validated config.
fn dump_config_style(resolved: &Path) {
    match load_dynamic_config(resolved) {
        Ok(cfg) => match serde_json::to_string_pretty(&cfg.resolved_style()) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("Failed to serialize style: {e}");
                process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("{}", e.pretty());
            process::exit(1);
        }
    }
}
