//! Test implementations for smoketest.

pub mod fixtures;
pub mod fullscreen;
pub mod hide;
pub mod place;
pub mod place_animated;
pub mod place_async;
/// Fake placement harness exercising the adapter when GUI access is unavailable.
pub mod place_fake;
pub mod place_flex;
pub mod place_increments;
#[cfg(test)]
pub mod place_metrics;
pub mod place_skip;
pub mod place_state;
pub mod place_term;
pub mod repeat;
pub mod ui;
pub mod world_ax;
/// Simulated multi-space adoption smoketest utilities.
pub mod world_spaces;
pub mod world_status;

// Re-export the main test functions for easier access
pub use repeat::{repeat_relay, repeat_shell, repeat_volume};
