//! Smoketest cases focused on UI and repeat flows (window ops removed).
pub mod repeat;
pub mod ui;

pub use repeat::{repeat_relay_throughput, repeat_shell_throughput, repeat_volume_throughput};
pub use ui::{ui_demo_mini, ui_demo_standard, ui_display_mapping};
