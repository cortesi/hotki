//! Command-line interface definitions for smoketest.

use crate::config;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(name = "smoketest", about = "Hotki smoketest tool", version)]
pub struct Cli {
    /// Enable logging to stdout/stderr at info level (respect RUST_LOG)
    #[arg(long)]
    pub logs: bool,

    /// Suppress headings and non-error output (used by orchestrated runs)
    #[arg(long)]
    pub quiet: bool,

    /// Default duration for repeat tests in milliseconds
    #[arg(long, default_value_t = config::DEFAULT_DURATION_MS)]
    pub duration: u64,

    /// Default timeout for UI readiness and waits in milliseconds
    #[arg(long, default_value_t = config::DEFAULT_TIMEOUT_MS)]
    pub timeout: u64,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum SeqTest {
    RepeatRelay,
    RepeatShell,
    RepeatVolume,
    Focus,
    Raise,
    Hide,
    Fullscreen,
    Ui,
    Minui,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum FsState {
    Toggle,
    On,
    Off,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Measure relay repeats posted to the focused window
    #[command(name = "repeat-relay")]
    Relay,

    /// Measure number of shell invocations when repeating a shell command
    #[command(name = "repeat-shell")]
    Shell,

    /// Measure repeats by incrementing system volume from zero
    #[command(name = "repeat-volume")]
    Volume,

    /// Run all smoketests (repeats + UI demos)
    #[command(name = "all")]
    All,

    /// Run a sequence of smoketests in order
    ///
    /// Example: smoketest seq repeat-relay focus ui
    #[command(name = "seq")]
    Seq {
        /// One or more test names to run in order
        #[arg(value_enum, value_name = "TEST", num_args = 1..)]
        tests: Vec<SeqTest>,
    },

    /// Verify raise(action) by switching focus between two titled windows
    Raise,

    /// Verify focus tracking by activating a test window
    Focus,

    /// Verify hide(toggle)/on/off by moving a helper window off/on screen right
    Hide,

    /// Internal helper: create a foreground window with a title for focus testing
    #[command(hide = true, name = "focus-winhelper")]
    FocusWinHelper {
        /// Title to set on the helper window
        #[arg(long)]
        title: String,
        /// How long to keep the window alive (ms)
        #[arg(long, default_value_t = config::DEFAULT_HELPER_WINDOW_TIME_MS)]
        time: u64,
    },

    /// Launch UI with test config and drive a short HUD + theme cycle
    Ui,

    /// Take HUD-only screenshots for a theme
    // Screenshots extracted to separate tool: hotki-shots

    /// Launch UI in mini HUD mode and cycle themes
    Minui,

    /// Control fullscreen on a helper window (toggle/on/off; native or non-native)
    Fullscreen {
        /// Desired state (toggle/on/off)
        #[arg(long, value_enum, default_value_t = FsState::Toggle)]
        state: FsState,
        /// Use native system fullscreen instead of non-native
        #[arg(long, default_value_t = false)]
        native: bool,
    },
    // Preflight smoketest removed.
}
