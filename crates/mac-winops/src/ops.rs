use hotki_world_ids::WorldWindowId;

use crate::{
    MoveDir, PlaceAttemptOptions, Result as WinResult, SpaceId, WindowId, WindowInfo,
    hide_bottom_left, request_activate_pid, request_focus_dir, request_fullscreen_native,
    request_fullscreen_nonnative, request_place_grid, request_place_grid_focused,
    request_place_grid_focused_opts, request_place_grid_opts, request_place_move_grid,
    request_place_move_grid_opts,
    window::{frontmost_window, frontmost_window_for_pid, list_windows, list_windows_for_spaces},
};

/// Trait abstraction over window operations to improve testability.
pub trait WinOps: Send + Sync {
    fn request_fullscreen_native(&self, pid: i32, desired: crate::Desired) -> WinResult<()>;
    fn request_fullscreen_nonnative(&self, pid: i32, desired: crate::Desired) -> WinResult<()>;
    fn request_place_grid(
        &self,
        target: WorldWindowId,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
    ) -> WinResult<()>;
    fn request_place_grid_opts(
        &self,
        target: WorldWindowId,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        opts: PlaceAttemptOptions,
    ) -> WinResult<()> {
        let _ = opts;
        self.request_place_grid(target, cols, rows, col, row)
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
        target: WorldWindowId,
        cols: u32,
        rows: u32,
        dir: MoveDir,
    ) -> WinResult<()>;
    fn request_place_move_grid_opts(
        &self,
        target: WorldWindowId,
        cols: u32,
        rows: u32,
        dir: MoveDir,
        opts: PlaceAttemptOptions,
    ) -> WinResult<()> {
        let _ = opts;
        self.request_place_move_grid(target, cols, rows, dir)
    }
    fn request_focus_dir(&self, dir: MoveDir) -> WinResult<()>;
    fn request_activate_pid(&self, pid: i32) -> WinResult<()>;
    fn request_raise_window(&self, pid: i32, id: WindowId) -> WinResult<()>;
    fn ensure_frontmost_by_title(
        &self,
        pid: i32,
        title: &str,
        attempts: usize,
        delay_ms: u64,
    ) -> bool;
    fn list_windows(&self) -> Vec<WindowInfo>;
    fn list_windows_for_spaces(&self, spaces: &[SpaceId]) -> Vec<WindowInfo>;
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
        target: WorldWindowId,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
    ) -> WinResult<()> {
        request_place_grid(target, cols, rows, col, row)
    }
    fn request_place_grid_opts(
        &self,
        target: WorldWindowId,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        opts: PlaceAttemptOptions,
    ) -> WinResult<()> {
        request_place_grid_opts(target, cols, rows, col, row, opts)
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
        target: WorldWindowId,
        cols: u32,
        rows: u32,
        dir: MoveDir,
    ) -> WinResult<()> {
        request_place_move_grid(target, cols, rows, dir)
    }
    fn request_place_move_grid_opts(
        &self,
        target: WorldWindowId,
        cols: u32,
        rows: u32,
        dir: MoveDir,
        opts: PlaceAttemptOptions,
    ) -> WinResult<()> {
        request_place_move_grid_opts(target, cols, rows, dir, opts)
    }
    fn request_focus_dir(&self, dir: MoveDir) -> WinResult<()> {
        request_focus_dir(dir)
    }
    fn request_activate_pid(&self, pid: i32) -> WinResult<()> {
        request_activate_pid(pid)
    }
    fn request_raise_window(&self, pid: i32, id: WindowId) -> WinResult<()> {
        crate::request_raise_window(pid, id)
    }
    fn ensure_frontmost_by_title(
        &self,
        pid: i32,
        title: &str,
        attempts: usize,
        delay_ms: u64,
    ) -> bool {
        crate::ensure_frontmost_by_title(pid, title, attempts, delay_ms)
    }
    fn list_windows(&self) -> Vec<WindowInfo> {
        list_windows()
    }
    fn list_windows_for_spaces(&self, spaces: &[SpaceId]) -> Vec<WindowInfo> {
        list_windows_for_spaces(spaces)
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
#[derive(Clone)]
pub struct MockWinOps {
    calls: Arc<Mutex<Vec<String>>>,
    front_for_pid: Arc<Mutex<Option<WI>>>,
    windows: Arc<Mutex<Vec<WI>>>,
    active_spaces: Arc<Mutex<Vec<SpaceId>>>,
    fail_focus_dir: Arc<AtomicBool>,
    frontmost: Arc<Mutex<Option<WI>>>,
    last_place_grid_pid: Arc<Mutex<Option<i32>>>,
    fail_fullscreen_native: Arc<AtomicBool>,
    fail_fullscreen_nonnative: Arc<AtomicBool>,
    fail_place_grid_focused: Arc<AtomicBool>,
    fail_place_move_grid: Arc<AtomicBool>,
    fail_activate_pid: Arc<AtomicBool>,
    fail_hide: Arc<AtomicBool>,
    fail_raise_window: Arc<AtomicBool>,
    ensure_return: Arc<AtomicBool>,
}

impl MockWinOps {
    pub fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            front_for_pid: Arc::new(Mutex::new(None)),
            windows: Arc::new(Mutex::new(Vec::new())),
            active_spaces: Arc::new(Mutex::new(Vec::new())),
            fail_focus_dir: Arc::new(AtomicBool::new(false)),
            frontmost: Arc::new(Mutex::new(None)),
            last_place_grid_pid: Arc::new(Mutex::new(None)),
            fail_fullscreen_native: Arc::new(AtomicBool::new(false)),
            fail_fullscreen_nonnative: Arc::new(AtomicBool::new(false)),
            fail_place_grid_focused: Arc::new(AtomicBool::new(false)),
            fail_place_move_grid: Arc::new(AtomicBool::new(false)),
            fail_activate_pid: Arc::new(AtomicBool::new(false)),
            fail_hide: Arc::new(AtomicBool::new(false)),
            fail_raise_window: Arc::new(AtomicBool::new(false)),
            ensure_return: Arc::new(AtomicBool::new(true)),
        }
    }
    pub fn set_frontmost_for_pid(&self, info: Option<WI>) {
        let mut g = self.front_for_pid.lock();
        *g = info;
    }
    pub fn set_windows(&self, wins: Vec<WI>) {
        {
            let mut g = self.windows.lock();
            *g = wins;
        }
        self.recompute_active_flags();
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
    pub fn set_fail_raise_window(&self, v: bool) {
        self.fail_raise_window.store(v, Ordering::SeqCst);
    }
    pub fn set_ensure_result(&self, v: bool) {
        self.ensure_return.store(v, Ordering::SeqCst);
    }
    fn note(&self, s: &str) {
        self.calls.lock().push(s.to_string());
    }
    /// Override the simulated active space ids for window filtering.
    pub fn set_active_spaces(&self, spaces: Vec<SpaceId>) {
        {
            let mut g = self.active_spaces.lock();
            *g = spaces;
        }
        self.recompute_active_flags();
    }
    fn recompute_active_flags(&self) {
        let active = {
            let g = self.active_spaces.lock();
            g.clone()
        };
        if active.is_empty() {
            return;
        }
        let mut wins = self.windows.lock();
        for w in wins.iter_mut() {
            w.on_active_space = matches_active_space(w.space, w.is_on_screen, &active);
        }
    }
    fn filter_windows(&self, filter: Option<&[SpaceId]>) -> Vec<WindowInfo> {
        let wins = self.windows.lock();
        wins.iter()
            .filter(|&w| should_include_mock_window(w, filter))
            .cloned()
            .collect()
    }
}

impl Default for MockWinOps {
    fn default() -> Self {
        Self::new()
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
        target: WorldWindowId,
        _cols: u32,
        _rows: u32,
        _col: u32,
        _row: u32,
    ) -> WinResult<()> {
        self.note("place_grid");
        {
            let mut g = self.last_place_grid_pid.lock();
            *g = Some(target.pid());
        }
        if self.fail_place_move_grid.load(Ordering::SeqCst) {
            return Err(crate::error::Error::MainThread);
        }
        Ok(())
    }
    fn request_place_grid_opts(
        &self,
        target: WorldWindowId,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        _opts: PlaceAttemptOptions,
    ) -> WinResult<()> {
        self.request_place_grid(target, cols, rows, col, row)
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
        _target: WorldWindowId,
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
        target: WorldWindowId,
        cols: u32,
        rows: u32,
        dir: MoveDir,
        _opts: PlaceAttemptOptions,
    ) -> WinResult<()> {
        self.request_place_move_grid(target, cols, rows, dir)
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
    fn request_raise_window(&self, _pid: i32, _id: WindowId) -> WinResult<()> {
        self.note("raise_window");
        if self.fail_raise_window.load(Ordering::SeqCst) {
            return Err(crate::error::Error::MainThread);
        }
        Ok(())
    }
    fn ensure_frontmost_by_title(
        &self,
        _pid: i32,
        _title: &str,
        _attempts: usize,
        _delay_ms: u64,
    ) -> bool {
        self.note("ensure_frontmost");
        self.note("raise_window");
        if self.fail_raise_window.load(Ordering::SeqCst) {
            return false;
        }
        self.ensure_return.load(Ordering::SeqCst)
    }
    fn list_windows(&self) -> Vec<WindowInfo> {
        self.filter_windows(None)
    }
    fn list_windows_for_spaces(&self, spaces: &[SpaceId]) -> Vec<WindowInfo> {
        self.filter_windows(Some(spaces))
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

fn matches_active_space(space: Option<SpaceId>, is_on_screen: bool, active: &[SpaceId]) -> bool {
    match space {
        Some(id) if id >= 0 => active.contains(&id),
        Some(_) => true,
        None => is_on_screen,
    }
}

fn should_include_mock_window(window: &WI, filter: Option<&[SpaceId]>) -> bool {
    match filter {
        None => window.on_active_space,
        Some([]) => true,
        Some(spaces) => match window.space {
            Some(id) if id >= 0 => spaces.contains(&id),
            Some(_) => true,
            None => true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{MockWinOps, SpaceId, WinOps, WindowInfo};

    fn win_with_space(
        pid: i32,
        id: u32,
        space: Option<SpaceId>,
        on_active_space: bool,
    ) -> WindowInfo {
        WindowInfo {
            app: format!("App{pid}"),
            title: format!("Title{id}"),
            pid,
            id,
            pos: None,
            space,
            layer: 0,
            focused: false,
            is_on_screen: true,
            on_active_space,
        }
    }

    #[test]
    fn list_windows_filters_to_active_flags() {
        let mock = MockWinOps::new();
        mock.set_windows(vec![
            win_with_space(1, 1, Some(1), true),
            win_with_space(2, 2, Some(2), false),
        ]);

        let active = mock.list_windows();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].pid, 1);

        let all = mock.list_windows_for_spaces(&[]);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn list_windows_for_specific_spaces_includes_matches() {
        let mock = MockWinOps::new();
        mock.set_windows(vec![
            win_with_space(1, 1, Some(1), true),
            win_with_space(2, 2, Some(2), false),
            win_with_space(3, 3, Some(-1), false),
        ]);

        let only_two = mock.list_windows_for_spaces(&[2_i64]);
        assert_eq!(only_two.len(), 2, "includes sticky window");
        assert!(only_two.iter().any(|w| w.pid == 2));
        assert!(only_two.iter().any(|w| w.pid == 3));
    }

    #[test]
    fn active_spaces_override_flags_when_configured() {
        let mock = MockWinOps::new();
        mock.set_windows(vec![win_with_space(4, 4, Some(5), false)]);
        assert!(mock.list_windows().is_empty());

        mock.set_active_spaces(vec![5_i64]);
        let active = mock.list_windows();
        assert_eq!(active.len(), 1);
        assert!(active[0].on_active_space);
    }
}
