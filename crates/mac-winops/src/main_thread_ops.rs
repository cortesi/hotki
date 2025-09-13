use std::collections::VecDeque;

use once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::{
    Desired, WindowId,
    error::{Error, Result},
};

/// Direction for moving a window within a grid layout.
#[derive(Clone, Copy, Debug)]
pub enum MoveDir {
    /// Move the window to the left grid cell.
    Left,
    /// Move the window to the right grid cell.
    Right,
    /// Move the window to the upper grid cell.
    Up,
    /// Move the window to the lower grid cell.
    Down,
}

/// Queue of operations that must run on the AppKit main thread.
pub enum MainOp {
    FullscreenNative {
        pid: i32,
        desired: Desired,
    },
    FullscreenNonNative {
        pid: i32,
        desired: Desired,
    },
    PlaceGrid {
        id: WindowId,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
    },
    PlaceMoveGrid {
        id: WindowId,
        cols: u32,
        rows: u32,
        dir: MoveDir,
    },
    PlaceGridFocused {
        pid: i32,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
    },
    /// Best-effort app activation for a pid (fallback for raise).
    ActivatePid {
        pid: i32,
    },
    RaiseWindow {
        pid: i32,
        id: WindowId,
    },
    /// Focus navigation: move focus to the next window in the given direction
    /// on the current screen within the current Space.
    FocusDir {
        dir: MoveDir,
    },
}

pub static MAIN_OPS: Lazy<Mutex<VecDeque<MainOp>>> = Lazy::new(|| Mutex::new(VecDeque::new()));

/// Return true if `existing` and `incoming` target the same logical window and
/// should be coalesced (i.e., keep only the `incoming`).
fn should_coalesce(existing: &MainOp, incoming: &MainOp) -> bool {
    match (existing, incoming) {
        // Coalesce per-WindowId for id-specific placements (keep latest intent)
        (
            MainOp::PlaceGrid { id: a, .. } | MainOp::PlaceMoveGrid { id: a, .. },
            MainOp::PlaceGrid { id: b, .. } | MainOp::PlaceMoveGrid { id: b, .. },
        ) => a == b,

        // Coalesce focused placements per pid (we don't know WindowId yet)
        (MainOp::PlaceGridFocused { pid: a, .. }, MainOp::PlaceGridFocused { pid: b, .. }) => {
            a == b
        }

        // Other operations are not coalesced here
        _ => false,
    }
}

/// Enqueue `op` into `MAIN_OPS` with simple coalescing rules so that rapid
/// consecutive placements for the same target window (or focused pid) collapse
/// to the latest intent before the main thread drains the queue.
fn enqueue_with_coalescing(op: MainOp) {
    let mut q = MAIN_OPS.lock();
    if matches!(
        op,
        MainOp::PlaceGrid { .. } | MainOp::PlaceMoveGrid { .. } | MainOp::PlaceGridFocused { .. }
    ) {
        // Rebuild the queue without older ops targeting the same window/pid
        let mut new_q = VecDeque::with_capacity(q.len());
        let mut dropped = 0usize;
        while let Some(existing) = q.pop_front() {
            if should_coalesce(&existing, &op) {
                dropped += 1;
            } else {
                new_q.push_back(existing);
            }
        }
        if dropped > 0 {
            tracing::debug!(
                "MainOps: coalesced {} prior placement op(s) before enqueue",
                dropped
            );
        }
        *q = new_q;
    }
    q.push_back(op);
}

/// Schedule a non‑native fullscreen operation to be executed on the AppKit main
/// thread and wake the Tao event loop.
pub fn request_fullscreen_nonnative(pid: i32, desired: Desired) -> Result<()> {
    tracing::info!(
        "MainOps: enqueue FullscreenNonNative pid={} desired={:?}",
        pid,
        desired
    );
    enqueue_with_coalescing(MainOp::FullscreenNonNative { pid, desired });
    // Wake the Tao main loop to handle user event and drain ops
    let _ = crate::focus::post_user_event();
    Ok(())
}

/// Schedule a native fullscreen operation (AXFullScreen) on the AppKit main thread.
pub fn request_fullscreen_native(pid: i32, desired: Desired) -> Result<()> {
    tracing::info!(
        "MainOps: enqueue FullscreenNative pid={} desired={:?}",
        pid,
        desired
    );
    enqueue_with_coalescing(MainOp::FullscreenNative { pid, desired });
    let _ = crate::focus::post_user_event();
    Ok(())
}

/// Schedule placement of a specific window (by `WindowId`) into a grid cell on
/// its current screen's visible frame. Runs on the AppKit main thread and
/// wakes the Tao event loop.
pub fn request_place_grid(id: WindowId, cols: u32, rows: u32, col: u32, row: u32) -> Result<()> {
    if cols == 0 || rows == 0 {
        return Err(Error::Unsupported);
    }
    enqueue_with_coalescing(MainOp::PlaceGrid {
        id,
        cols,
        rows,
        col,
        row,
    });
    let _ = crate::focus::post_user_event();
    Ok(())
}

/// Schedule movement of a specific window (by `WindowId`) within a grid on the
/// AppKit main thread.
pub fn request_place_move_grid(id: WindowId, cols: u32, rows: u32, dir: MoveDir) -> Result<()> {
    if cols == 0 || rows == 0 {
        return Err(Error::Unsupported);
    }
    enqueue_with_coalescing(MainOp::PlaceMoveGrid {
        id,
        cols,
        rows,
        dir,
    });
    let _ = crate::focus::post_user_event();
    Ok(())
}

/// Schedule placement of the focused window for `pid` into a grid cell on the AppKit main thread.
pub fn request_place_grid_focused(
    pid: i32,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
) -> Result<()> {
    if cols == 0 || rows == 0 {
        return Err(Error::Unsupported);
    }
    enqueue_with_coalescing(MainOp::PlaceGridFocused {
        pid,
        cols,
        rows,
        col,
        row,
    });
    let _ = crate::focus::post_user_event();
    Ok(())
}

/// Schedule a window raise by pid+id on the AppKit main thread.
pub fn request_raise_window(pid: i32, id: WindowId) -> Result<()> {
    enqueue_with_coalescing(MainOp::RaiseWindow { pid, id });
    let _ = crate::focus::post_user_event();
    Ok(())
}

/// Queue a best-effort activation of the application with `pid` on the AppKit main thread.
pub fn request_activate_pid(pid: i32) -> Result<()> {
    tracing::debug!("queue ActivatePid for pid={} on main thread", pid);
    enqueue_with_coalescing(MainOp::ActivatePid { pid });
    let _ = crate::focus::post_user_event();
    Ok(())
}

/// Schedule a directional focus change on the AppKit main thread.
pub fn request_focus_dir(dir: MoveDir) -> Result<()> {
    enqueue_with_coalescing(MainOp::FocusDir { dir });
    let _ = crate::focus::post_user_event();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clear_queue() {
        MAIN_OPS.lock().clear();
    }

    #[test]
    fn coalesce_id_specific_latest_wins() {
        clear_queue();
        let id = 42u32;
        // enqueue several ops for same id
        let _ = request_place_grid(id, 3, 2, 0, 0);
        let _ = request_place_move_grid(id, 3, 2, MoveDir::Right);
        let _ = request_place_grid(id, 3, 2, 2, 1); // latest should win

        let q = MAIN_OPS.lock();
        assert_eq!(q.len(), 1);
        match q.front().unwrap() {
            MainOp::PlaceGrid {
                id: got, col, row, ..
            } => {
                assert_eq!(*got, id);
                assert_eq!((*col, *row), (2, 1));
            }
            _ => panic!("unexpected op in queue"),
        }
    }

    #[test]
    fn coalesce_focused_by_pid() {
        clear_queue();
        let pid = 12345;
        let _ = request_place_grid_focused(pid, 2, 2, 0, 0);
        let _ = request_place_grid_focused(pid, 2, 2, 1, 1); // coalesce previous focused for pid

        let q = MAIN_OPS.lock();
        assert_eq!(q.len(), 1);
        match q.front().unwrap() {
            MainOp::PlaceGridFocused {
                pid: got, col, row, ..
            } => {
                assert_eq!(*got, pid);
                assert_eq!((*col, *row), (1, 1));
            }
            _ => panic!("unexpected op in queue"),
        }
    }

    #[test]
    fn unrelated_ops_not_coalesced() {
        clear_queue();
        let _ = request_activate_pid(777);
        let _ = request_focus_dir(MoveDir::Left);
        let _ = request_raise_window(7, 9);
        // Different id gets its own entry
        let _ = request_place_grid(1, 2, 2, 0, 0);
        let _ = request_place_grid(2, 2, 2, 1, 1);
        let q = MAIN_OPS.lock();
        assert_eq!(q.len(), 5);
    }
}
