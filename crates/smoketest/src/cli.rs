//! Command-line interface definitions for smoketest.

use crate::config;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "smoketest", about = "Hotki smoketest tool", version)]
pub struct Cli {
    /// Enable logging to stdout/stderr at info level (respect RUST_LOG)
    #[arg(long)]
    pub logs: bool,

    /// Default duration for repeat tests in milliseconds
    #[arg(long, default_value_t = config::DEFAULT_DURATION_MS)]
    pub duration: u64,

    /// Default timeout for UI readiness and waits in milliseconds
    #[arg(long, default_value_t = config::DEFAULT_TIMEOUT_MS)]
    pub timeout: u64,

    #[command(subcommand)]
    pub command: Commands,
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
    #[command(name = "screenshots")]
    Screenshots {
        /// Theme name to apply before capturing (optional)
        #[arg(long)]
        theme: Option<String>,
        /// Output directory for PNG files
        dir: PathBuf,
    },

    /// Launch UI in mini HUD mode and cycle themes
    Minui,

    /// Toggle non-system (non-native) fullscreen on a helper window
    Fullscreen,
    // Preflight smoketest removed.
}
