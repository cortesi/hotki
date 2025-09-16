//! Configuration constants and defaults for smoketests.

use std::{
    process,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

// ===== Default Test Parameters =====

/// Default duration for repeat tests in milliseconds.
pub const DEFAULT_DURATION_MS: u64 = 1000;

/// Default timeout for UI readiness and waits in milliseconds.
pub const DEFAULT_TIMEOUT_MS: u64 = 10000;

/// Default time to keep helper windows alive in milliseconds.
pub const DEFAULT_HELPER_WINDOW_TIME_MS: u64 = 30000;

/// Minimum duration for volume tests to reduce flakiness.
pub const MIN_VOLUME_TEST_DURATION_MS: u64 = 2000;

// ===== Timing Constants for UI Interactions =====

/// Polling interval for checking conditions.
pub const POLL_INTERVAL_MS: u64 = 10;

/// Short delay between key events.
pub const KEY_EVENT_DELAY_MS: u64 = 25;

/// Standard delay between UI actions.
pub const UI_ACTION_DELAY_MS: u64 = 50;

/// Delay between retry attempts.
pub const RETRY_DELAY_MS: u64 = 80;

/// Delay for window registration.
pub const WINDOW_REGISTRATION_DELAY_MS: u64 = 80;

// ===== Wait Timeouts =====

/// Timeout for waiting for both windows to appear.
pub const WAIT_BOTH_WINDOWS_MS: u64 = 15000;

/// Timeout for waiting for the first window.
pub const WAIT_FIRST_WINDOW_MS: u64 = 6000;

// (legacy per-test menu timing constants have been removed; use test-specific
// tunables below instead.)

/// Extra time to add to helper window lifetime.
pub const HELPER_WINDOW_EXTRA_TIME_MS: u64 = 5000;

/// Extra time for hide test helper windows.
pub const HIDE_HELPER_EXTRA_TIME_MS: u64 = 8000;

// ===== Retry and Connection Settings =====

/// Initial delay for connection retry (increases after first attempts).
pub const INITIAL_RETRY_DELAY_MS: u64 = 200;

/// Fast retry delay after initial attempts.
pub const FAST_RETRY_DELAY_MS: u64 = 50;

/// Number of initial connection attempts before switching to fast retry.
pub const INITIAL_RETRY_ATTEMPTS: u32 = 3;

/// Delay for waiting between event checks.
pub const EVENT_CHECK_INTERVAL_MS: u64 = 80;

// ===== Window and UI Constants =====

/// Offset for window positioning tests.
pub const WINDOW_POSITION_OFFSET: f64 = 300.0;

/// Minimum timeout for hide operations (1/4 of main timeout).
pub const HIDE_MIN_TIMEOUT_MS: u64 = 800;

/// Minimum timeout for secondary hide operations (1/3 of main timeout).
pub const HIDE_SECONDARY_MIN_TIMEOUT_MS: u64 = 1000;

/// Default binding readiness gate for non-raise tests (RPC mode).
pub const BINDING_GATE_DEFAULT_MS: u64 = 2000;

// ===== Helper Window Defaults =====

/// Default helper window width in pixels for test helpers.
pub const HELPER_WIN_WIDTH: f64 = 280.0;
/// Default helper window height in pixels for test helpers.
pub const HELPER_WIN_HEIGHT: f64 = 180.0;
/// Margin from screen edge when placing helper windows.
pub const HELPER_WIN_MARGIN: f64 = 8.0;

// ===== Place Test Tunables =====
/// Default number of columns for placement grid.
pub const PLACE_COLS: u32 = 3;
/// Default number of rows for placement grid.
pub const PLACE_ROWS: u32 = 2;
/// Epsilon in pixels for frame comparisons in placement checks.
pub const PLACE_EPS: f64 = 2.0;
/// Poll interval while waiting for placement to settle.
pub const PLACE_POLL_MS: u64 = 50;
/// Per-cell timeout while waiting for the expected frame.
pub const PLACE_STEP_TIMEOUT_MS: u64 = 3000;
// ===== Raise Test Tunables =====

/// Extra lifetime added to helper windows beyond overall timeout.
pub const RAISE_HELPER_EXTRA_MS: u64 = 2500;
/// Max wait for the first helper window to appear.
pub const RAISE_FIRST_WINDOW_MAX_MS: u64 = 2000;
// (legacy per-step delays for raise have been removed; active gating is used instead.)
/// Binding gate timeout for RPC identifier readiness.
pub const RAISE_BINDING_GATE_MS: u64 = 1200;

// ===== Focus-Nav Test Tunables =====
/// Per-step wait for focus-nav frontmost checks and transitions.
pub const FOCUS_NAV_STEP_TIMEOUT_MS: u64 = 2000;

// ===== Session Tunables =====

/// Interval to resend activation chord while waiting for HUD.
pub const ACTIVATION_RESEND_INTERVAL_MS: u64 = 500;

// ===== Test Configuration Paths =====

/// Default test configuration file path relative to repo root.
pub const DEFAULT_TEST_CONFIG_PATH: &str = "examples/test.ron";

// ===== Window Title Templates =====

/// Generate a unique window title with a simple prefix.
pub fn test_title(prefix: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("hotki smoketest: {} {}-{}", prefix, process::id(), now)
}

/// Base title for relay repeat test window.
pub const RELAY_TEST_TITLE: &str = "hotki smoketest: relayrepeat";

// ===== Fullscreen Test Tunables =====

/// Delay after launching the fullscreen helper before measuring (milliseconds).
pub const FULLSCREEN_HELPER_SHOW_DELAY_MS: u64 = 300;
// Post-toggle delay removed in favor of actively waiting for frame changes.
/// Maximum time to wait for fullscreen frame stabilization (milliseconds).
pub const FULLSCREEN_WAIT_TOTAL_MS: u64 = 1000;
/// Poll interval while waiting for fullscreen frame changes (milliseconds).
pub const FULLSCREEN_WAIT_POLL_MS: u64 = 50;

// ===== Hide Test Tunables =====

/// Max wait for the helper window to appear initially.
pub const HIDE_FIRST_WINDOW_MAX_MS: u64 = 2000;
/// Poll interval for hide position/frame checks.
pub const HIDE_POLL_MS: u64 = 50;
/// Delay after activation before sending next hide keys.
pub const HIDE_ACTIVATE_POST_DELAY_MS: u64 = 100;
/// Max wait for the window to restore frame on hide(off).
pub const HIDE_RESTORE_MAX_MS: u64 = 1200;

// ===== Focus Test Tunables =====

/// Poll interval for receiving focus HudUpdate events.
pub const FOCUS_EVENT_POLL_MS: u64 = 150;
/// Poll interval for the outer wait loop.
pub const FOCUS_POLL_MS: u64 = 100;

// ===== Warning Overlay Tunables =====
/// Initial delay before starting tests after showing the hands-off overlay.
pub const WARN_OVERLAY_INITIAL_DELAY_MS: u64 = 2000;
/// Default size for the hands-off overlay window.
pub const WARN_OVERLAY_WIDTH: f64 = 520.0;
/// Default height of the warning overlay window.
pub const WARN_OVERLAY_HEIGHT: f64 = 140.0;

// ===== Helper Functions =====

/// Convert milliseconds to Duration.
pub const fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}
