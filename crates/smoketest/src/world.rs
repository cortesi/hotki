//! Shared trait-backed window snapshot helpers for smoketests.

use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use hotki_world::{
    CommandError, CommandReceipt, MoveDirection, PlaceAttemptOptions, RaiseIntent, World,
    WorldView, WorldWindow,
};
use hotki_world_ids::WorldWindowId;
use mac_winops::{self, WindowInfo, active_space_ids, ops::RealWinOps};
use once_cell::sync::OnceCell;
use regex::Regex;
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

/// Convert a [`CommandError`] into a smoketest [`Error`] with context.
fn map_world_error(op: &str, err: &CommandError) -> Error {
    Error::InvalidState(format!("world {op} command failed: {err}"))
}

/// Execute a future on the shared runtime used by smoketests.
fn world_block_on<F, T>(fut: F) -> Result<T>
where
    F: Future<Output = T>,
{
    runtime::block_on(fut)
}

/// Request placement for a specific window via the world service.
pub fn place_window(
    target: WorldWindowId,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
    options: Option<PlaceAttemptOptions>,
) -> Result<CommandReceipt> {
    let world = ensure_world()?;
    let receipt = world_block_on(async move {
        world
            .request_place_for_window(target, cols, rows, col, row, options)
            .await
    })?;
    let receipt = receipt.map_err(|err| map_world_error("place_window", &err))?;
    mac_winops::drain_main_ops();
    Ok(receipt)
}

/// Request a grid-relative move for a specific window via the world service.
pub fn move_window(
    target: WorldWindowId,
    cols: u32,
    rows: u32,
    dir: MoveDirection,
    options: Option<PlaceAttemptOptions>,
) -> Result<CommandReceipt> {
    let world = ensure_world()?;
    let receipt = world_block_on(async move {
        world
            .request_place_move_for_window(target, cols, rows, dir, options)
            .await
    })?;
    let receipt = receipt.map_err(|err| map_world_error("move_window", &err))?;
    mac_winops::drain_main_ops();
    Ok(receipt)
}

/// Raise a window matching the provided title and best-effort ensure it is frontmost.
pub fn ensure_frontmost(pid: i32, title: &str, attempts: usize, delay_ms: u64) -> Result<()> {
    let regex = Regex::new(&format!("^{}$", regex::escape(title)))
        .map_err(|e| Error::InvalidState(format!("invalid title regex: {}", e)))?;
    let intent = RaiseIntent {
        app_regex: None,
        title_regex: Some(Arc::new(regex)),
    };

    for attempt in 0..attempts {
        let world = ensure_world()?;
        let receipt = world_block_on(async { world.request_raise(intent.clone()).await })?;
        match receipt {
            Ok(receipt) => {
                if let Some(target) = receipt.target
                    && target.pid == pid
                    && target.title == title
                {
                    return Ok(());
                }
            }
            Err(err) => {
                return Err(map_world_error("ensure_frontmost", &err));
            }
        }

        if attempt + 1 < attempts {
            thread::sleep(Duration::from_millis(delay_ms));
        }
    }

    Err(Error::InvalidState(format!(
        "failed to raise window pid={} title='{}' after {} attempts",
        pid, title, attempts
    )))
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
