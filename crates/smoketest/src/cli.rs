//! Command-line interface definitions for smoketest.

use clap::{Parser, Subcommand};
use logging::LogArgs;

use crate::config;

/// Command-line interface arguments for the smoketest binary.
#[derive(Parser, Debug)]
#[command(name = "smoketest", about = "Hotki smoketest tool", version)]
pub struct Cli {
    /// Logging controls
    #[command(flatten)]
    pub log: LogArgs,

    /// Suppress headings and non-error output (used by orchestrated runs)
    #[arg(long)]
    pub quiet: bool,

    /// Disable the hands-off keyboard warning overlay
    #[arg(long)]
    pub no_warn: bool,

    /// Continue running the full `all` suite even if individual tests fail
    #[arg(long)]
    pub no_fail_fast: bool,

    /// Optional short info text to show in the warning overlay under the test title
    #[arg(long)]
    pub info: Option<String>,

    /// Wall-clock budget for each case, including startup, waits, and cleanup
    #[arg(long = "run-budget", default_value_t = config::DEFAULT_RUN_BUDGET_MS)]
    pub run_budget_ms: u64,

    /// Repeat the selected tests this many times
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    pub repeat: u32,

    /// Which subcommand to run
    #[command(subcommand)]
    pub command: Commands,
}

/// CLI commands for the smoketest runner.
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run all smoketests
    #[command(name = "all")]
    All,

    /// List registered smoketest cases
    #[command(name = "list")]
    List,

    /// Run a sequence of smoketests in order
    ///
    /// Example: smoketest seq hud mini displays
    #[command(name = "seq")]
    Seq {
        /// One or more test names to run in order
        #[arg(value_name = "TEST", num_args = 1..)]
        tests: Vec<String>,
    },

    /// Verify full HUD appears and responds to keys
    #[command(name = "hud")]
    Hud,

    /// Verify mini HUD appears and responds to keys
    #[command(name = "mini")]
    Mini,

    /// Verify HUD placement on multi-display setups
    #[command(name = "displays")]
    Displays,

    /// Verify notification window placement
    #[command(name = "notifications")]
    Notifications,

    /// Verify invalid startup config visibility and corrected activation
    #[command(name = "config-activation")]
    ConfigActivation,

    /// Verify the app reconnects after its server exits
    #[command(name = "reconnect")]
    Reconnect,
}

impl Commands {
    /// Return the case slug for a registry-backed command.
    pub fn case_slug(&self) -> Option<&'static str> {
        match self {
            Self::Hud => Some("hud"),
            Self::Mini => Some("mini"),
            Self::Displays => Some("displays"),
            Self::Notifications => Some("notifications"),
            Self::ConfigActivation => Some("config-activation"),
            Self::Reconnect => Some("reconnect"),
            Self::All | Self::List | Self::Seq { .. } => None,
        }
    }
}
