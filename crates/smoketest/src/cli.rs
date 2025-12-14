//! Command-line interface definitions for smoketest.

use clap::{Parser, Subcommand, ValueEnum};
use logging::LogArgs;

use crate::{
    config,
    suite::{CaseRunOpts, case_by_slug},
};

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
}

impl SeqTest {
    /// Return the registry slug corresponding to this sequence entry.
    pub fn slug(self) -> &'static str {
        let alias_value = self
            .to_possible_value()
            .expect("seq test must expose a clap alias");
        let alias = alias_value.get_name();
        case_by_slug(alias)
            .map(|entry| entry.name)
            .expect("seq test alias must map to a registered case")
    }
}

/// CLI commands for the smoketest runner.
#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
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
}

impl Commands {
    /// Return the case slug and run options for a command.
    pub fn case_info(&self, _fake_mode: bool) -> Option<(&'static str, CaseRunOpts)> {
        let default_opts = CaseRunOpts::default();

        let candidate = match self {
            Self::Hud => "hud",
            Self::Mini => "mini",
            Self::Displays => "displays",
            Self::All | Self::Seq { .. } => return None,
        };

        let entry = case_by_slug(candidate)?;
        Some((entry.name, default_opts))
    }
}
