//! Test implementations for smoketest.

pub mod focus;
pub mod fullscreen;
pub mod hide;
pub mod raise;
pub mod repeat;
pub mod screenshot;
pub mod ui;

// Re-export the main test functions for easier access
pub use repeat::{repeat_relay, repeat_shell, repeat_volume};
