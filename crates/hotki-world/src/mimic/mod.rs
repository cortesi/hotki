#![allow(clippy::module_name_repetitions)]

mod config;
mod event_loop;
mod helper_app;
mod registry;
mod runtime;
mod scenario;
mod world;

pub use event_loop::{EventLoopHandle, shared_event_loop};
pub use registry::registry_snapshot;
pub use runtime::{
    MimicError, MimicHandle, active_count, kill_mimic, pump_active_mimics, request_shutdown_all,
    spawn_mimic, wait_until_idle,
};
pub use scenario::{HelperConfig, MimicDiagnostic, MimicScenario, MimicSpec, Quirk};
