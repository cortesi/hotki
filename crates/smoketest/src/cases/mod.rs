//! Smoketest cases implemented with the mimic harness.
pub mod focus;
pub mod hide;
pub mod place;
pub mod repeat;
pub mod support;
pub mod world;

pub use focus::{focus_nav, focus_tracking, raise};
pub use hide::hide_toggle_roundtrip;
pub use place::{
    place_animated_tween, place_async_delay, place_grid_cycle, place_increments_anchor,
    place_minimized_defer, place_move_min_anchor, place_move_nonresizable_anchor,
    place_skip_nonmovable, place_term_anchor,
};
pub use repeat::{repeat_relay_throughput, repeat_shell_throughput, repeat_volume_throughput};
pub use world::{world_ax_focus_props, world_status_permissions};
