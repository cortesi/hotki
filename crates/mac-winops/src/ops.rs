use crate::{
    MoveDir, Result as WinResult, WindowId, WindowInfo,
    frontmost_window, frontmost_window_for_pid, hide_bottom_left, list_windows,
    request_activate_pid, request_focus_dir, request_fullscreen_native,
    request_fullscreen_nonnative, request_place_grid_focused, request_place_move_grid,
};

/// Trait abstraction over window operations to improve testability.
pub trait WinOps: Send + Sync {
    fn request_fullscreen_native(&self, pid: i32, desired: crate::Desired) -> WinResult<()>;
    fn request_fullscreen_nonnative(&self, pid: i32, desired: crate::Desired) -> WinResult<()>;
    fn request_place_grid_focused(
        &self,
        pid: i32,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
    ) -> WinResult<()>;
    fn request_place_move_grid(
        &self,
        id: WindowId,
        cols: u32,
        rows: u32,
        dir: MoveDir,
    ) -> WinResult<()>;
    fn request_focus_dir(&self, dir: MoveDir) -> WinResult<()>;
    fn request_activate_pid(&self, pid: i32) -> WinResult<()>;
    fn list_windows(&self) -> Vec<WindowInfo>;
    fn frontmost_window(&self) -> Option<WindowInfo>;
    fn frontmost_window_for_pid(&self, pid: i32) -> Option<WindowInfo>;
    fn hide_bottom_left(&self, pid: i32, desired: crate::Desired) -> WinResult<()>;
}

/// Production implementation of WinOps delegating to crate functions.
pub struct RealWinOps;

impl WinOps for RealWinOps {
    fn request_fullscreen_native(&self, pid: i32, desired: crate::Desired) -> WinResult<()> {
        request_fullscreen_native(pid, desired)
    }
    fn request_fullscreen_nonnative(&self, pid: i32, desired: crate::Desired) -> WinResult<()> {
        request_fullscreen_nonnative(pid, desired)
    }
    fn request_place_grid_focused(
        &self,
        pid: i32,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
    ) -> WinResult<()> {
        request_place_grid_focused(pid, cols, rows, col, row)
    }
    fn request_place_move_grid(
        &self,
        id: WindowId,
        cols: u32,
        rows: u32,
        dir: MoveDir,
    ) -> WinResult<()> {
        request_place_move_grid(id, cols, rows, dir)
    }
    fn request_focus_dir(&self, dir: MoveDir) -> WinResult<()> {
        request_focus_dir(dir)
    }
    fn request_activate_pid(&self, pid: i32) -> WinResult<()> {
        request_activate_pid(pid)
    }
    fn list_windows(&self) -> Vec<WindowInfo> {
        list_windows()
    }
    fn frontmost_window(&self) -> Option<WindowInfo> {
        frontmost_window()
    }
    fn frontmost_window_for_pid(&self, pid: i32) -> Option<WindowInfo> {
        frontmost_window_for_pid(pid)
    }
    fn hide_bottom_left(&self, pid: i32, desired: crate::Desired) -> WinResult<()> {
        hide_bottom_left(pid, desired)
    }
}

use std::sync::{Arc, Mutex};
use crate::WindowInfo as WI;

/// Simple mock implementation for tests (enabled with `test-utils` feature).
#[derive(Clone, Default)]
pub struct MockWinOps {
    calls: Arc<Mutex<Vec<String>>>,
    front_for_pid: Arc<Mutex<Option<WI>>>,
    windows: Arc<Mutex<Vec<WI>>>,
}

impl MockWinOps {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_frontmost_for_pid(&self, info: Option<WI>) {
        if let Ok(mut g) = self.front_for_pid.lock() {
            *g = info;
        }
    }
    pub fn set_windows(&self, wins: Vec<WI>) {
        if let Ok(mut g) = self.windows.lock() {
            *g = wins;
        }
    }
    pub fn calls_contains(&self, s: &str) -> bool {
        self.calls
            .lock()
            .map(|g| g.iter().any(|x| x == s))
            .unwrap_or(false)
    }
    fn note(&self, s: &str) {
        if let Ok(mut g) = self.calls.lock() {
            g.push(s.to_string());
        }
    }
}

impl WinOps for MockWinOps {
    fn request_fullscreen_native(&self, _pid: i32, _d: crate::Desired) -> WinResult<()> {
        self.note("fullscreen_native");
        Ok(())
    }
    fn request_fullscreen_nonnative(&self, _pid: i32, _d: crate::Desired) -> WinResult<()> {
        self.note("fullscreen_nonnative");
        Ok(())
    }
    fn request_place_grid_focused(
        &self,
        _pid: i32,
        _cols: u32,
        _rows: u32,
        _col: u32,
        _row: u32,
    ) -> WinResult<()> {
        self.note("place_grid_focused");
        Ok(())
    }
    fn request_place_move_grid(
        &self,
        _id: WindowId,
        _cols: u32,
        _rows: u32,
        _dir: MoveDir,
    ) -> WinResult<()> {
        self.note("place_move");
        Ok(())
    }
    fn request_focus_dir(&self, _dir: MoveDir) -> WinResult<()> {
        self.note("focus_dir");
        Ok(())
    }
    fn request_activate_pid(&self, _pid: i32) -> WinResult<()> {
        self.note("activate_pid");
        Ok(())
    }
    fn list_windows(&self) -> Vec<WindowInfo> {
        match self.windows.lock() {
            Ok(g) => g.clone(),
            Err(_) => Vec::new(),
        }
    }
    fn frontmost_window(&self) -> Option<WindowInfo> {
        None
    }
    fn frontmost_window_for_pid(&self, _pid: i32) -> Option<WindowInfo> {
        self.front_for_pid.lock().ok().and_then(|g| g.clone())
    }
    fn hide_bottom_left(&self, _pid: i32, _desired: crate::Desired) -> WinResult<()> {
        self.note("hide");
        Ok(())
    }
}
