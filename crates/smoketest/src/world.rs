//! Shared trait-backed window snapshot helpers for smoketests.

use std::{sync::Arc, time::Instant};

use hotki_world::{World, WorldView, WorldWindow};
use mac_winops::{WindowInfo, active_space_ids, ops::RealWinOps};
use once_cell::sync::OnceCell;
use tracing::info;

use crate::{
    error::{Error, Result},
    runtime,
};

/// Lazily constructed world view shared across smoketest helpers.
static WORLD: OnceCell<Arc<dyn WorldView>> = OnceCell::new();

/// Ensure the shared world instance exists and return a cloned handle.
fn ensure_world() -> Result<Arc<dyn WorldView>> {
    if let Some(w) = WORLD.get() {
        return Ok(w.clone());
    }
    let rt = runtime::shared_runtime()?;
    let runtime = rt.lock();
    let guard = runtime.enter();
    let world = World::spawn_view(Arc::new(RealWinOps), hotki_world::WorldCfg::default());
    drop(guard);
    WORLD
        .set(world.clone())
        .map_err(|_| Error::InvalidState("world already initialized".into()))?;
    Ok(world)
}

/// Convert a `WorldWindow` into the `mac_winops` data structure used by tests.
fn convert_window(w: WorldWindow) -> WindowInfo {
    WindowInfo {
        app: w.app,
        title: w.title,
        pid: w.pid,
        id: w.id,
        pos: w.pos,
        space: w.space,
        layer: w.layer,
        focused: w.focused,
        is_on_screen: w.is_on_screen,
        on_active_space: w.on_active_space,
    }
}

/// Fetch a complete snapshot via the [`WorldView`].
pub fn list_windows() -> Result<Vec<WindowInfo>> {
    let world = ensure_world()?;
    let sweep_start = Instant::now();
    let active_spaces = active_space_ids();
    world.hint_refresh();
    let windows: Vec<WindowInfo> = runtime::block_on(async move {
        world
            .list_windows()
            .await
            .into_iter()
            .map(convert_window)
            .collect::<Vec<_>>()
    })?;
    let elapsed = sweep_start.elapsed();
    let active_count = windows.iter().filter(|w| w.on_active_space).count();
    let total = windows.len();
    info!(
        target: "smoketest::world",
        sweep_ms = elapsed.as_secs_f64() * 1000.0,
        total_windows = total,
        active_windows = active_count,
        offspace_windows = total.saturating_sub(active_count),
        active_spaces = ?active_spaces,
        "world_snapshot_metrics"
    );
    Ok(windows)
}

/// Resolve a window snapshot or return an empty list if the world is unavailable.
pub fn list_windows_or_empty() -> Vec<WindowInfo> {
    list_windows().unwrap_or_default()
}
