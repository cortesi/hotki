use std::{collections::VecDeque, sync::Mutex};

use once_cell::sync::Lazy;

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
    /// Best-effort app activation for a pid (fallback for raise).
    ActivatePid {
        pid: i32,
    },
    RaiseWindow {
        pid: i32,
        id: WindowId,
    },
}

pub static MAIN_OPS: Lazy<Mutex<VecDeque<MainOp>>> = Lazy::new(|| Mutex::new(VecDeque::new()));

/// Schedule a non‑native fullscreen operation to be executed on the AppKit main
/// thread and wake the Tao event loop.
pub fn request_fullscreen_nonnative(pid: i32, desired: Desired) -> Result<()> {
    tracing::info!(
        "MainOps: enqueue FullscreenNonNative pid={} desired={:?}",
        pid,
        desired
    );
    if MAIN_OPS
        .lock()
        .map(|mut q| q.push_back(MainOp::FullscreenNonNative { pid, desired }))
        .is_err()
    {
        return Err(Error::QueuePoisoned);
    }
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
    if MAIN_OPS
        .lock()
        .map(|mut q| q.push_back(MainOp::FullscreenNative { pid, desired }))
        .is_err()
    {
        return Err(Error::QueuePoisoned);
    }
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
    if MAIN_OPS
        .lock()
        .map(|mut q| {
            q.push_back(MainOp::PlaceGrid {
                id,
                cols,
                rows,
                col,
                row,
            })
        })
        .is_err()
    {
        return Err(Error::QueuePoisoned);
    }
    let _ = crate::focus::post_user_event();
    Ok(())
}

/// Schedule movement of a specific window (by `WindowId`) within a grid on the
/// AppKit main thread.
pub fn request_place_move_grid(id: WindowId, cols: u32, rows: u32, dir: MoveDir) -> Result<()> {
    if cols == 0 || rows == 0 {
        return Err(Error::Unsupported);
    }
    if MAIN_OPS
        .lock()
        .map(|mut q| {
            q.push_back(MainOp::PlaceMoveGrid {
                id,
                cols,
                rows,
                dir,
            })
        })
        .is_err()
    {
        return Err(Error::QueuePoisoned);
    }
    let _ = crate::focus::post_user_event();
    Ok(())
}

/// Schedule a window raise by pid+id on the AppKit main thread.
pub fn request_raise_window(pid: i32, id: WindowId) -> Result<()> {
    if MAIN_OPS
        .lock()
        .map(|mut q| q.push_back(MainOp::RaiseWindow { pid, id }))
        .is_err()
    {
        return Err(Error::QueuePoisoned);
    }
    let _ = crate::focus::post_user_event();
    Ok(())
}

/// Queue a best-effort activation of the application with `pid` on the AppKit main thread.
pub fn request_activate_pid(pid: i32) -> Result<()> {
    tracing::debug!("queue ActivatePid for pid={} on main thread", pid);
    if MAIN_OPS
        .lock()
        .map(|mut q| q.push_back(MainOp::ActivatePid { pid }))
        .is_err()
    {
        return Err(Error::QueuePoisoned);
    }
    let _ = crate::focus::post_user_event();
    Ok(())
}
