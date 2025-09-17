//! Command-line interface definitions for hotki-tester.

use std::{path::PathBuf, time::Duration};

use clap::{Args, Parser, Subcommand};
use logging::LogArgs;

/// Command-line interface for the `hotki-tester` binary.
#[derive(Parser, Debug)]
#[command(
    name = "hotki-tester",
    about = "Real-world diagnostics for Hotki",
    version
)]
pub struct Cli {
    /// Logging controls shared across hotki binaries.
    #[command(flatten)]
    pub log: LogArgs,

    /// Which diagnostic scenario to run.
    #[command(subcommand)]
    pub command: Commands,
}

/// Top-level tester commands.
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Diagnose placement behaviour for the currently focused window.
    Place(PlaceArgs),
}

/// Arguments for the `place` subcommand.
#[derive(Args, Debug, Clone)]
pub struct PlaceArgs {
    /// One or more place directives in RON syntax, e.g. `place(grid(3, 2), at(1, 0))`.
    #[arg(value_name = "PLACE_SPEC", num_args = 1..)]
    pub specs: Vec<String>,

    /// Duration to wait before capturing the pre-placement snapshot.
    #[arg(
        long,
        value_parser = humantime::parse_duration,
        default_value = "5s",
        value_name = "DURATION"
    )]
    pub snapshot_after: Duration,

    /// Duration to wait for the backend server to accept connections.
    #[arg(
        long,
        value_parser = humantime::parse_duration,
        default_value = "10s",
        value_name = "DURATION"
    )]
    pub ready_timeout: Duration,

    /// Poll interval while waiting for the backend socket to appear.
    #[arg(
        long,
        value_parser = humantime::parse_duration,
        default_value = "150ms",
        value_name = "DURATION"
    )]
    pub ready_poll: Duration,

    /// Duration to wait after placement before collecting the final snapshot.
    #[arg(
        long,
        value_parser = humantime::parse_duration,
        default_value = "800ms",
        value_name = "DURATION"
    )]
    pub settle_after: Duration,

    /// Optional path to a Hotki configuration file (RON) to load into the backend.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Optional override for the Hotki binary to spawn in `--server` mode.
    #[arg(long, value_name = "PATH")]
    pub hotki_bin: Option<PathBuf>,

    /// Inherit stdout/stderr from the backend so its logs appear inline.
    #[arg(long)]
    pub server_logs: bool,
}
