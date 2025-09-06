//! Configuration constants and defaults for smoketests.

use std::time::Duration;

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
pub const POLL_INTERVAL_MS: u64 = 50;

/// Short delay between key events.
pub const KEY_EVENT_DELAY_MS: u64 = 60;

/// Standard delay between UI actions.
pub const UI_ACTION_DELAY_MS: u64 = 120;

/// Delay for UI stabilization.
pub const UI_STABILIZE_DELAY_MS: u64 = 200;

/// Delay between retry attempts.
pub const RETRY_DELAY_MS: u64 = 300;

/// Delay when opening menus in sequence.
pub const MENU_OPEN_STAGGER_MS: u64 = 150;

/// Delay for window registration.
pub const WINDOW_REGISTRATION_DELAY_MS: u64 = 200;

/// Delay after menu operations to allow stabilization.
pub const MENU_STABILIZE_DELAY_MS: u64 = 250;

/// Delay between menu key presses.
pub const MENU_KEY_DELAY_MS: u64 = 120;

// ===== Wait Timeouts =====

/// Timeout for waiting for both windows to appear.
pub const WAIT_BOTH_WINDOWS_MS: u64 = 15000;

/// Timeout for waiting for the first window.
pub const WAIT_FIRST_WINDOW_MS: u64 = 6000;

/// Timeout for rechecking window presence.
pub const WAIT_WINDOW_RECHECK_MS: u64 = 1500;

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
pub const EVENT_CHECK_INTERVAL_MS: u64 = 300;

/// Delay after sending activation chord.
pub const ACTIVATION_CHORD_DELAY_MS: u64 = 80;

// ===== Window and UI Constants =====

/// Offset for window positioning tests.
pub const WINDOW_POSITION_OFFSET: f64 = 300.0;

/// Minimum timeout for hide operations (1/4 of main timeout).
pub const HIDE_MIN_TIMEOUT_MS: u64 = 800;

/// Minimum timeout for secondary hide operations (1/3 of main timeout).
pub const HIDE_SECONDARY_MIN_TIMEOUT_MS: u64 = 1000;

// ===== CoreGraphics Constants =====

// ===== Test Configuration Paths =====

/// Default test configuration file path relative to repo root.
pub const DEFAULT_TEST_CONFIG_PATH: &str = "examples/test.ron";

// ===== Window Title Templates =====

/// Generate a unique window title for focus tests.
pub fn focus_test_title(test_id: u128) -> String {
    format!("hotki smoketest: focus {}-{}", std::process::id(), test_id)
}

/// Generate a unique window title for hide tests.
pub fn hide_test_title(test_id: u128) -> String {
    format!("hotki smoketest: hide {}-{}", std::process::id(), test_id)
}

/// Base title for relay repeat test window.
pub const RELAY_TEST_TITLE: &str = "hotki smoketest: relayrepeat";

// ===== Logging Configuration =====

/// Standard RUST_LOG configuration for tests with logging enabled.
pub const TEST_LOG_CONFIG: &str = "info,hotki=info,hotki_server=info,hotki_engine=info,mac_winops=info,mac_hotkey=info,mac_focus_watcher=info,mrpc::connection=off";

// ===== Helper Functions =====

/// Convert milliseconds to Duration.
pub const fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}
