#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

//! Minimal focus + display snapshot service for Hotki.
//!
//! Stage 3 of the WinOps removal collapses `hotki-world` into a read-only
//! provider. The service now tracks only:
//! - focused app/title/pid context (best-effort)
//! - a lightweight window list
//! - display geometry snapshots
//!
//! There are no mutating commands (place/hide/focus/raise); callers should use
//! external tooling for window control. The exported surface is intentionally
//! small and stable: [`WorldView`] for querying state, [`World`] helpers for
//! spawning, and the data carriers defined below.

mod events;
mod geometry;
mod platform;
mod polling;
mod state;
#[cfg(any(test, feature = "test-utils"))]
pub mod test_support;
#[cfg(any(test, feature = "test-utils"))]
mod test_world;
mod types;

use std::sync::Arc;

pub use events::EventCursor;
pub use hotki_protocol::{DisplayFrame, DisplaysSnapshot, FocusSnapshot};
pub use permissions::{PermissionState, PermissionsStatus as Capabilities};
use polling::PollingWorld;
#[cfg(any(test, feature = "test-utils"))]
pub use test_world::{TestApplication, TestWorld};
pub use types::{
    ApplicationResolution, FocusChange, WindowKey, WorldCfg, WorldEvent, WorldStatus, WorldView,
    WorldWindow, focus_snapshot, focus_snapshot_for_change, focused_snapshot, snapshot_for_key,
    subscribe_with_snapshot,
};

/// Public helpers to spawn world views.
pub struct World;

impl World {
    /// Spawn the default polling world view.
    #[must_use]
    pub fn spawn_default_view(cfg: WorldCfg) -> Arc<dyn WorldView> {
        PollingWorld::spawn(cfg)
    }
}

#[cfg(test)]
#[path = "../tests/basic.rs"]
mod integration_tests;
