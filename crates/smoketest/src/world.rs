//! Shared trait-backed window snapshot helpers for smoketests.

use std::sync::Arc;

use hotki_world::{World, WorldView, WorldWindow, view_util};
use mac_winops::{WindowInfo, ops::RealWinOps};
use once_cell::sync::OnceCell;

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
        space: None,
        layer: w.layer,
        focused: w.focused,
    }
}

/// Fetch a complete snapshot via the [`WorldView`].
pub fn list_windows() -> Result<Vec<WindowInfo>> {
    let world = ensure_world()?;
    world.hint_refresh();
    runtime::block_on(async move {
        let snap = view_util::list_windows(world.as_ref()).await;
        snap.into_iter().map(convert_window).collect::<Vec<_>>()
    })
}

/// Resolve a window snapshot or return an empty list if the world is unavailable.
pub fn list_windows_or_empty() -> Vec<WindowInfo> {
    list_windows().unwrap_or_default()
}

/// Resolve the frontmost window using focus preference, falling back to lowest `z`.
pub fn frontmost_window() -> Result<Option<WindowInfo>> {
    let world = ensure_world()?;
    world.hint_refresh();
    runtime::block_on(async move {
        view_util::frontmost_window(world.as_ref())
            .await
            .map(convert_window)
    })
}

/// Convenience helper that normalizes the optional result from [`frontmost_window`].
pub fn frontmost_window_opt() -> Option<WindowInfo> {
    frontmost_window().ok().flatten()
}
