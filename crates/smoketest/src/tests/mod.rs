//! Test implementations for smoketest.

pub mod focus;
pub mod hide;
pub mod raise;
pub mod repeat;
pub mod screenshot;
pub mod ui;

// Re-export the main test functions for easier access
pub use focus::run_focus_test;
pub use hide::run_hide_test;
pub use raise::run_raise_test;
pub use repeat::{count_relay, count_shell, count_volume, repeat_relay, repeat_shell, repeat_volume};
pub use screenshot::run_screenshots;
pub use ui::{run_ui_demo, run_minui_demo};
