//! hotki-world: Window State Service (skeleton)
//!
//! Maintains types and constructor for the World service.
//! This stage provides the public API surface only; implementation arrives in later stages.

use std::sync::Arc;
use std::time::Instant;

use mac_winops::ops::WinOps;
use mac_winops::{Pos, WindowId};

/// Unique key for a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowKey {
    pub pid: i32,
    pub id: WindowId,
}

/// Opaque metadata attached to a window.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WindowMeta;

/// Identifier for a display.
pub type DisplayId = u32;

/// Snapshot of a single window.
#[derive(Clone, Debug)]
pub struct WorldWindow {
    pub app: String,
    pub title: String,
    pub pid: i32,
    pub id: WindowId,
    pub pos: Option<Pos>,
    pub layer: i32,
    pub z: u32,
    pub on_active_space: bool,
    pub display_id: Option<DisplayId>,
    pub focused: bool,
    pub meta: Vec<WindowMeta>,
    pub last_seen: Instant,
    pub seen_seq: u64,
}

/// Permission state for capabilities that affect data quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionState {
    Granted,
    Denied,
    Unknown,
}

impl Default for PermissionState {
    fn default() -> Self {
        Self::Unknown
    }
}

/// Reported capabilities and current permission state.
#[derive(Clone, Debug, Default)]
pub struct Capabilities {
    pub accessibility: PermissionState,
    pub screen_recording: PermissionState,
}

/// Configuration for the world service.
#[derive(Clone, Debug)]
pub struct WorldCfg {
    pub poll_ms_min: u64,
    pub poll_ms_max: u64,
    pub include_offscreen: bool,
    pub ax_watch_frontmost: bool,
}

impl Default for WorldCfg {
    fn default() -> Self {
        Self {
            poll_ms_min: 100,
            poll_ms_max: 1000,
            include_offscreen: false,
            ax_watch_frontmost: false,
        }
    }
}

/// Delta describing changed fields (placeholder for Stage 1).
#[derive(Clone, Debug, Default)]
pub struct WindowDelta;

/// World events stream payloads.
#[derive(Clone, Debug)]
pub enum WorldEvent {
    Added(WorldWindow),
    Removed(WindowKey),
    Updated(WindowKey, WindowDelta),
    MetaAdded(WindowKey, WindowMeta),
    MetaRemoved(WindowKey, WindowMeta),
    FocusChanged(Option<WindowKey>),
}

/// Cheap, clonable handle to the world service.
#[derive(Clone, Debug, Default)]
pub struct WorldHandle;

/// World constructor. Spawns the service and returns a handle.
///
/// Stage 1: define the API surface. Implementation is added in later stages.
pub struct World;

impl World {
    #[allow(unused_variables)]
    pub fn spawn(winops: Arc<dyn WinOps>, cfg: WorldCfg) -> WorldHandle {
        WorldHandle
    }
}
