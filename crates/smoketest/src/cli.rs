//! Command-line interface definitions for smoketest.

use clap::{Parser, Subcommand, ValueEnum};
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

    /// Default timeout for UI readiness and waits in milliseconds
    #[arg(long, default_value_t = config::DEFAULTS.timeout_ms)]
    pub timeout: u64,

    /// Repeat the selected tests this many times
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    pub repeat: u32,

    /// Which subcommand to run
    #[command(subcommand)]
    pub command: Commands,
}

/// Named tests that can be run in sequence via `seq`.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum SeqTest {
    /// Verify full HUD appears and responds to keys
    #[value(name = "hud")]
    Hud,
    /// Verify mini HUD appears and responds to keys
    #[value(name = "mini")]
    Mini,
    /// Verify HUD placement on multi-display setups
    #[value(name = "displays")]
    Displays,
    /// Verify notification window placement
    #[value(name = "notifications")]
    Notifications,
}

impl SeqTest {
    /// Return the registry slug corresponding to this sequence entry.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Hud => "hud",
            Self::Mini => "mini",
            Self::Displays => "displays",
            Self::Notifications => "notifications",
        }
    }
}

/// CLI commands for the smoketest runner.
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run all smoketests
    #[command(name = "all")]
    All,

    /// Run a sequence of smoketests in order
    ///
    /// Example: smoketest seq hud mini displays
    #[command(name = "seq")]
    Seq {
        /// One or more test names to run in order
        #[arg(value_enum, value_name = "TEST", num_args = 1..)]
        tests: Vec<SeqTest>,
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
}

impl Commands {
    /// Return the case slug for a registry-backed command.
    pub fn case_slug(&self) -> Option<&'static str> {
        match self {
            Self::Hud => Some("hud"),
            Self::Mini => Some("mini"),
            Self::Displays => Some("displays"),
            Self::Notifications => Some("notifications"),
            Self::All | Self::Seq { .. } => None,
        }
    }
}
