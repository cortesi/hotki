use crate::{
    MoveDir, PlaceAttemptOptions, Result as WinResult, WindowId, WindowInfo, frontmost_window,
    frontmost_window_for_pid, hide_bottom_left, list_windows, request_activate_pid,
    request_focus_dir, request_fullscreen_native, request_fullscreen_nonnative, request_place_grid,
    request_place_grid_focused, request_place_grid_focused_opts, request_place_grid_opts,
    request_place_move_grid, request_place_move_grid_opts,
};

/// Trait abstraction over window operations to improve testability.
pub trait WinOps: Send + Sync {
    fn request_fullscreen_native(&self, pid: i32, desired: crate::Desired) -> WinResult<()>;
    fn request_fullscreen_nonnative(&self, pid: i32, desired: crate::Desired) -> WinResult<()>;
    fn request_place_grid(
        &self,
        id: WindowId,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
    ) -> WinResult<()>;
    fn request_place_grid_opts(
        &self,
        id: WindowId,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        opts: PlaceAttemptOptions,
    ) -> WinResult<()> {
        let _ = opts;
        self.request_place_grid(id, cols, rows, col, row)
    }
    fn request_place_grid_focused(
        &self,
        pid: i32,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
    ) -> WinResult<()>;
    fn request_place_grid_focused_opts(
        &self,
        pid: i32,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        opts: PlaceAttemptOptions,
    ) -> WinResult<()> {
        let _ = opts;
        self.request_place_grid_focused(pid, cols, rows, col, row)
    }
    fn request_place_move_grid(
        &self,
        id: WindowId,
        cols: u32,
        rows: u32,
        dir: MoveDir,
    ) -> WinResult<()>;
    fn request_place_move_grid_opts(
        &self,
        id: WindowId,
        cols: u32,
        rows: u32,
        dir: MoveDir,
        opts: PlaceAttemptOptions,
    ) -> WinResult<()> {
        let _ = opts;
        self.request_place_move_grid(id, cols, rows, dir)
    }
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
    fn request_place_grid(
        &self,
        id: WindowId,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
    ) -> WinResult<()> {
        request_place_grid(id, cols, rows, col, row)
    }
    fn request_place_grid_opts(
        &self,
        id: WindowId,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        opts: PlaceAttemptOptions,
    ) -> WinResult<()> {
        request_place_grid_opts(id, cols, rows, col, row, opts)
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
    fn request_place_grid_focused_opts(
        &self,
        pid: i32,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        opts: PlaceAttemptOptions,
    ) -> WinResult<()> {
        request_place_grid_focused_opts(pid, cols, rows, col, row, opts)
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
    fn request_place_move_grid_opts(
        &self,
        id: WindowId,
        cols: u32,
        rows: u32,
        dir: MoveDir,
        opts: PlaceAttemptOptions,
    ) -> WinResult<()> {
        request_place_move_grid_opts(id, cols, rows, dir, opts)
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

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use parking_lot::Mutex;

use crate::WindowInfo as WI;

/// Simple mock implementation for tests (enabled with `test-utils` feature).
#[derive(Clone, Default)]
pub struct MockWinOps {
    calls: Arc<Mutex<Vec<String>>>,
    front_for_pid: Arc<Mutex<Option<WI>>>,
    windows: Arc<Mutex<Vec<WI>>>,
    fail_focus_dir: Arc<AtomicBool>,
    frontmost: Arc<Mutex<Option<WI>>>,
    last_place_grid_pid: Arc<Mutex<Option<i32>>>,
    fail_fullscreen_native: Arc<AtomicBool>,
    fail_fullscreen_nonnative: Arc<AtomicBool>,
    fail_place_grid_focused: Arc<AtomicBool>,
    fail_place_move_grid: Arc<AtomicBool>,
    fail_activate_pid: Arc<AtomicBool>,
    fail_hide: Arc<AtomicBool>,
}

impl MockWinOps {
    pub fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            front_for_pid: Arc::new(Mutex::new(None)),
            windows: Arc::new(Mutex::new(Vec::new())),
            fail_focus_dir: Arc::new(AtomicBool::new(false)),
            frontmost: Arc::new(Mutex::new(None)),
            last_place_grid_pid: Arc::new(Mutex::new(None)),
            fail_fullscreen_native: Arc::new(AtomicBool::new(false)),
            fail_fullscreen_nonnative: Arc::new(AtomicBool::new(false)),
            fail_place_grid_focused: Arc::new(AtomicBool::new(false)),
            fail_place_move_grid: Arc::new(AtomicBool::new(false)),
            fail_activate_pid: Arc::new(AtomicBool::new(false)),
            fail_hide: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn set_frontmost_for_pid(&self, info: Option<WI>) {
        let mut g = self.front_for_pid.lock();
        *g = info;
    }
    pub fn set_windows(&self, wins: Vec<WI>) {
        let mut g = self.windows.lock();
        *g = wins;
    }
    pub fn calls_contains(&self, s: &str) -> bool {
        self.calls.lock().iter().any(|x| x == s)
    }
    pub fn set_fail_focus_dir(&self, v: bool) {
        self.fail_focus_dir.store(v, Ordering::SeqCst);
    }
    pub fn set_frontmost(&self, w: Option<WI>) {
        let mut g = self.frontmost.lock();
        *g = w;
    }
    pub fn last_place_grid_pid(&self) -> Option<i32> {
        *self.last_place_grid_pid.lock()
    }
    pub fn set_fail_fullscreen_native(&self, v: bool) {
        self.fail_fullscreen_native.store(v, Ordering::SeqCst);
    }
    pub fn set_fail_fullscreen_nonnative(&self, v: bool) {
        self.fail_fullscreen_nonnative.store(v, Ordering::SeqCst);
    }
    pub fn set_fail_place_grid_focused(&self, v: bool) {
        self.fail_place_grid_focused.store(v, Ordering::SeqCst);
    }
    pub fn set_fail_place_move_grid(&self, v: bool) {
        self.fail_place_move_grid.store(v, Ordering::SeqCst);
    }
    pub fn set_fail_activate_pid(&self, v: bool) {
        self.fail_activate_pid.store(v, Ordering::SeqCst);
    }
    pub fn set_fail_hide(&self, v: bool) {
        self.fail_hide.store(v, Ordering::SeqCst);
    }
    fn note(&self, s: &str) {
        self.calls.lock().push(s.to_string());
    }
}

impl WinOps for MockWinOps {
    fn request_fullscreen_native(&self, _pid: i32, _d: crate::Desired) -> WinResult<()> {
        self.note("fullscreen_native");
        if self.fail_fullscreen_native.load(Ordering::SeqCst) {
            return Err(crate::error::Error::MainThread);
        }
        Ok(())
    }
    fn request_fullscreen_nonnative(&self, _pid: i32, _d: crate::Desired) -> WinResult<()> {
        self.note("fullscreen_nonnative");
        if self.fail_fullscreen_nonnative.load(Ordering::SeqCst) {
            return Err(crate::error::Error::MainThread);
        }
        Ok(())
    }
    fn request_place_grid(
        &self,
        _id: WindowId,
        _cols: u32,
        _rows: u32,
        _col: u32,
        _row: u32,
    ) -> WinResult<()> {
        self.note("place_grid");
        if self.fail_place_move_grid.load(Ordering::SeqCst) {
            return Err(crate::error::Error::MainThread);
        }
        Ok(())
    }
    fn request_place_grid_opts(
        &self,
        id: WindowId,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        _opts: PlaceAttemptOptions,
    ) -> WinResult<()> {
        self.request_place_grid(id, cols, rows, col, row)
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
        {
            let mut g = self.last_place_grid_pid.lock();
            *g = Some(_pid);
        }
        if self.fail_place_grid_focused.load(Ordering::SeqCst) {
            return Err(crate::error::Error::MainThread);
        }
        Ok(())
    }
    fn request_place_grid_focused_opts(
        &self,
        pid: i32,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        _opts: PlaceAttemptOptions,
    ) -> WinResult<()> {
        self.request_place_grid_focused(pid, cols, rows, col, row)
    }
    fn request_place_move_grid(
        &self,
        _id: WindowId,
        _cols: u32,
        _rows: u32,
        _dir: MoveDir,
    ) -> WinResult<()> {
        self.note("place_move");
        if self.fail_place_move_grid.load(Ordering::SeqCst) {
            return Err(crate::error::Error::MainThread);
        }
        Ok(())
    }
    fn request_place_move_grid_opts(
        &self,
        id: WindowId,
        cols: u32,
        rows: u32,
        dir: MoveDir,
        _opts: PlaceAttemptOptions,
    ) -> WinResult<()> {
        self.request_place_move_grid(id, cols, rows, dir)
    }
    fn request_focus_dir(&self, _dir: MoveDir) -> WinResult<()> {
        self.note("focus_dir");
        if self.fail_focus_dir.load(Ordering::SeqCst) {
            return Err(crate::error::Error::MainThread);
        }
        Ok(())
    }
    fn request_activate_pid(&self, _pid: i32) -> WinResult<()> {
        self.note("activate_pid");
        if self.fail_activate_pid.load(Ordering::SeqCst) {
            return Err(crate::error::Error::MainThread);
        }
        Ok(())
    }
    fn list_windows(&self) -> Vec<WindowInfo> {
        self.windows.lock().clone()
    }
    fn frontmost_window(&self) -> Option<WindowInfo> {
        self.frontmost.lock().clone()
    }
    fn frontmost_window_for_pid(&self, _pid: i32) -> Option<WindowInfo> {
        self.front_for_pid.lock().clone()
    }
    fn hide_bottom_left(&self, _pid: i32, _desired: crate::Desired) -> WinResult<()> {
        self.note("hide");
        if self.fail_hide.load(Ordering::SeqCst) {
            return Err(crate::error::Error::MainThread);
        }
        Ok(())
    }
}
