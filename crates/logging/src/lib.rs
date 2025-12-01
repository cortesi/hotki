#![warn(missing_docs)]

//! Shared logging helpers, CLI argument definitions, and tracing utilities for the hotki workspace.
//!
//! This crate consolidates logging infrastructure:
//! - [`fmt`]: Render tracing events to logfmt strings
//! - [`forward`]: Forward server logs to the UI layer
//! - CLI argument parsing for log level configuration

use std::env;

use clap::Args;
use tracing_subscriber::EnvFilter;

pub mod fmt;
pub mod forward;

/// Logging controls for CLI apps.
#[derive(Debug, Clone, Args)]
pub struct LogArgs {
    /// Set global log level to trace (our crates only)
    #[arg(long, conflicts_with_all = ["debug", "log_level", "log_filter"])]
    pub trace: bool,

    /// Set global log level to debug (our crates only)
    #[arg(long, conflicts_with_all = ["trace", "log_level", "log_filter"])]
    pub debug: bool,

    /// Set a single global log level for our crates (error|warn|info|debug|trace)
    #[arg(long)]
    pub log_level: Option<String>,

    /// Set an explicit tracing filter directive (overrides other flags)
    /// e.g. "hotki_engine=trace,hotki_server=debug"
    #[arg(long)]
    pub log_filter: Option<String>,
}

/// List of crate targets that constitute "our" logs.
pub fn our_crates() -> &'static [&'static str] {
    &[
        // Apps and core crates
        "hotki",
        "smoketest",
        "hotki_server",
        "hotki_engine",
        "hotki_world",
        "hotki_protocol",
        // macOS integration crates
        "mac_hotkey",
        "mac_keycode",
        // Utilities
        "permissions",
        "relaykey",
        "keymode",
        "eventtag",
        "dumpinput",
        "config",
        "logging",
    ]
}

/// Build a filter directive string that sets the same `level` for all of our crates.
///
/// Always includes `mrpc::connection=off` to suppress shutdown noise.
pub fn level_spec_for(level: &str) -> String {
    let lvl = level.to_ascii_lowercase();
    let mut parts: Vec<String> = our_crates()
        .iter()
        .map(|t| format!("{}={}", t, lvl))
        .collect();
    parts.push("mrpc::connection=off".to_string());
    parts.join(",")
}

/// Compute the final filter spec string with precedence:
/// - `log_filter`
/// - `trace`/`debug`/`log_level` (crate-scoped)
/// - `RUST_LOG` env (plus mrpc suppression if not present)
/// - default to crate-scoped `info`
pub fn compute_spec(
    trace: bool,
    debug: bool,
    log_level: Option<&str>,
    log_filter: Option<&str>,
) -> String {
    if let Some(spec) = log_filter {
        return spec.to_string();
    }
    if trace {
        return level_spec_for("trace");
    }
    if debug {
        return level_spec_for("debug");
    }
    if let Some(lvl) = log_level {
        return level_spec_for(lvl);
    }
    if let Ok(spec) = env::var("RUST_LOG") {
        if spec.contains("mrpc::connection") {
            spec
        } else {
            format!("{},mrpc::connection=off", spec)
        }
    } else {
        level_spec_for("info")
    }
}

/// Create an `EnvFilter` from a spec string.
pub fn env_filter_from_spec(spec: &str) -> EnvFilter {
    EnvFilter::new(spec)
}

/// Return the `RUST_LOG` value to use for child processes.
///
/// If the environment already specifies `RUST_LOG`, return that; otherwise return
/// a default crate-scoped `info` configuration (with mrpc suppression).
pub fn log_config_for_child() -> String {
    env::var("RUST_LOG").unwrap_or_else(|_| level_spec_for("info"))
}
