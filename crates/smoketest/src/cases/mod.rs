//! Smoketest cases implemented with the mimic harness.
pub mod focus;
pub mod fullscreen;
pub mod hide;
pub mod place;
pub mod repeat;
pub mod support;
pub mod ui;
pub mod world;

pub use focus::{focus_nav, focus_tracking, raise};
pub use fullscreen::fullscreen_toggle_nonnative;
pub use hide::hide_toggle_roundtrip;
pub use place::{
    place_animated_tween, place_async_delay, place_fake_adapter, place_flex_default,
    place_flex_force_size_pos, place_flex_smg, place_grid_cycle, place_increments_anchor,
    place_minimized_defer, place_move_min_anchor, place_move_nonresizable_anchor,
    place_skip_nonmovable, place_term_anchor, place_zoomed_normalize,
};
pub use repeat::{repeat_relay_throughput, repeat_shell_throughput, repeat_volume_throughput};
pub use ui::{ui_demo_mini, ui_demo_standard};
pub use world::{world_ax_focus_props, world_spaces_adoption, world_status_permissions};
