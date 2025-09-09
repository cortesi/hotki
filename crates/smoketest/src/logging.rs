//! Logging and tracing utilities for smoketests.

use std::sync::OnceLock;
use tracing_subscriber::prelude::*;

/// Standard RUST_LOG configuration for tests with logging enabled.
const DEFAULT_LOG_CONFIG: &str = concat!(
    "info,",
    "hotki=info,",
    "hotki_server=info,",
    "hotki_engine=warn,",
    "mac_winops=info,",
    "mac_hotkey=info,",
    "mac_focus_watcher=info,",
    "mrpc::connection=off"
);

/// Global flag to track if logging has been initialized
static LOGGING_INITIALIZED: OnceLock<()> = OnceLock::new();

/// Initialize logging considering `logs` and `quiet` flags.
/// - When `quiet` is true: force WARN+ regardless of RUST_LOG.
/// - Else when `logs` is true: INFO+ using RUST_LOG or default filters.
/// - Else: do not initialize subscriber (no tracing output).
pub fn init_for(logs: bool, quiet: bool) {
    LOGGING_INITIALIZED.get_or_init(|| {
        if quiet {
            // Force warn+ without consulting environment
            let env_filter = tracing_subscriber::EnvFilter::new("warn");
            let _ = tracing_subscriber::registry()
                .with(env_filter)
                .with(tracing_subscriber::fmt::layer().without_time())
                .try_init();
            return;
        }
        if logs {
            // Create env filter, defaulting to our standard config if RUST_LOG not set
            let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| DEFAULT_LOG_CONFIG.into());

            // Add mrpc noise suppression if not already present
            let env_filter = if let Ok(directive) = "mrpc::connection=off".parse() {
                env_filter.add_directive(directive)
            } else {
                env_filter
            };

            // Initialize subscriber with no timestamps for cleaner test output
            let _ = tracing_subscriber::registry()
                .with(env_filter)
                .with(tracing_subscriber::fmt::layer().without_time())
                .try_init();
        }
    });
}

// no legacy initializer

/// Get the standard RUST_LOG configuration string for child processes.
///
/// This is used when spawning hotki sessions with logging enabled.
pub fn log_config_for_child() -> String {
    std::env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_LOG_CONFIG.to_string())
}

// (no structured test event helpers at the moment)
