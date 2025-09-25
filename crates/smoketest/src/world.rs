//! Shared trait-backed window snapshot helpers for smoketests.

use std::{
    future::Future,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use hotki_world::{CommandError, RaiseIntent, WindowKey, World, WorldHandle, WorldWindow};
use hotki_world_ids::WorldWindowId;
use mac_winops::{self, active_space_ids, ops::RealWinOps};
use once_cell::sync::OnceCell;
use regex::Regex;
use tracing::{debug, info};

use crate::{
    error::{Error, Result},
    runtime,
};

/// Lazily constructed world handle shared across smoketest helpers.
static WORLD_HANDLE: OnceCell<WorldHandle> = OnceCell::new();

/// Ensure the shared world instance exists and return a cloned handle.
pub fn world_handle() -> Result<WorldHandle> {
    if let Some(handle) = WORLD_HANDLE.get() {
        return Ok(handle.clone());
    }
    let rt = runtime::shared_runtime()?;
    let runtime = rt.lock();
    let guard = runtime.enter();
    let handle = World::spawn(Arc::new(RealWinOps), hotki_world::WorldCfg::default());
    drop(guard);
    WORLD_HANDLE
        .set(handle.clone())
        .map_err(|_| Error::InvalidState("world already initialized".into()))?;
    Ok(handle)
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

/// Raise a window matching the provided title and best-effort ensure it is frontmost.
pub fn ensure_frontmost(pid: i32, title: &str, attempts: usize, delay_ms: u64) -> Result<()> {
    let regex = Regex::new(&format!("^{}$", regex::escape(title)))
        .map_err(|e| Error::InvalidState(format!("invalid title regex: {}", e)))?;
    let intent = RaiseIntent {
        app_regex: None,
        title_regex: Some(Arc::new(regex)),
    };

    for attempt in 0..attempts {
        let world = world_handle()?;
        let receipt = world_block_on(async { world.request_raise(intent.clone()).await })?;
        match receipt {
            Ok(receipt) => {
                if let Some(target) = receipt.target
                    && target.pid == pid
                    && target.title == title
                {
                    let key = WindowKey {
                        pid: target.pid,
                        id: target.id,
                    };
                    if let Ok(Some(window)) = world_block_on(async { world.get(key).await })
                        && window.on_active_space
                        && window.is_on_screen
                    {
                        return Ok(());
                    }
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

/// Attempt to raise a window without waiting for focus notifications.
pub fn smart_raise(target: WorldWindowId, title: &str, deadline: Duration) -> Result<()> {
    let pid = target.pid();
    let wid = target.window_id();
    let start = Instant::now();
    let mut click_attempted = false;
    let mut last_raise: Option<Instant> = None;

    while start.elapsed() < deadline {
        let now = Instant::now();
        let should_raise = last_raise
            .map(|ts| now.duration_since(ts) >= Duration::from_millis(160))
            .unwrap_or(true);
        if should_raise {
            match mac_winops::raise_window(pid, wid) {
                Ok(()) => {}
                Err(mac_winops::Error::MainThread) => {
                    mac_winops::request_raise_window(pid, wid).map_err(|err| {
                        Error::InvalidState(format!(
                            "smart raise queue failed for pid={} id={}: {}",
                            pid, wid, err
                        ))
                    })?;
                }
                Err(err) => {
                    debug!(pid, id = wid, error = %err, "smart_raise_raise_failed");
                }
            }
            last_raise = Some(now);
        }

        if let Ok(windows) = list_windows()
            && windows
                .iter()
                .any(|w| w.pid == pid && w.id == wid && w.is_on_screen && w.on_active_space)
        {
            return Ok(());
        }

        if !click_attempted && now.duration_since(start) >= Duration::from_millis(200) {
            click_attempted = mac_winops::click_window_center(pid, title);
            if click_attempted {
                debug!(pid, title, "smart_raise_click_issued");
            }
        }

        thread::sleep(Duration::from_millis(40));
    }

    Err(Error::InvalidState(format!(
        "smart raise timed out for pid={} title='{}'",
        pid, title
    )))
}

/// Fetch a complete snapshot via the [`WorldView`].
pub fn list_windows() -> Result<Vec<WorldWindow>> {
    let world = world_handle()?;
    let sweep_start = Instant::now();
    let active_spaces = active_space_ids();
    world.hint_refresh();
    let windows: Vec<WorldWindow> =
        runtime::block_on(async move { world.snapshot().await.into_iter().collect::<Vec<_>>() })?;
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
