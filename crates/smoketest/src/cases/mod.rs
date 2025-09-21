//! Smoketest cases implemented with the mimic harness.
pub mod focus;
pub mod place;
pub mod repeat;
pub mod support;

pub use focus::{focus_nav, focus_tracking, raise};
pub use place::{place_animated_tween, place_async_delay, place_minimized_defer};
pub use repeat::{repeat_relay_throughput, repeat_shell_throughput, repeat_volume_throughput};
