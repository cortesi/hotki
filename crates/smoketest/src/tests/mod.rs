//! Test implementations for smoketest.

pub mod fullscreen;
pub mod hide;
#[cfg(test)]
pub mod place_metrics;
pub mod repeat;
pub mod ui;
pub mod world_ax;
/// Simulated multi-space adoption smoketest utilities.
pub mod world_spaces;
pub mod world_status;

// Re-export the main test functions for easier access
pub use repeat::{repeat_relay, repeat_shell, repeat_volume};
pub mod fixtures;
