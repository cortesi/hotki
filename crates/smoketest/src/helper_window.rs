//! Helper window process management and fixtures used by smoketests.

use std::{
    env,
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

use crate::{
    config,
    error::{Error, Result},
    proc_registry,
};

/// Managed child process that cleans up on drop.
pub struct ManagedChild {
    /// Handle to the spawned child process.
    child: Option<Child>,
    /// Process ID of the managed child.
    pub pid: i32,
}

impl ManagedChild {
    /// Create a new managed child from a process.
    pub fn new(child: Child) -> Self {
        let pid = child.id() as i32;
        proc_registry::register(pid);
        Self {
            child: Some(child),
            pid,
        }
    }

    /// Kill the process and wait for it to exit.
    pub fn kill_and_wait(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            child.kill().map_err(Error::Io)?;
            child.wait().map_err(Error::Io)?;
        }
        proc_registry::unregister(self.pid);
        Ok(())
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        if let Err(_e) = self.kill_and_wait() {
            // best-effort cleanup on drop
        }
    }
}

/// Builder for spawning helper windows with common configurations.
pub struct HelperWindowBuilder {
    /// Window title for the helper.
    title: String,
    /// How long the helper runs before exiting (ms).
    time_ms: u64,
    /// Optional delay before applying system-set frames (ms).
    delay_setframe_ms: Option<u64>,
    /// Optional explicit delayed-apply time (ms).
    delay_apply_ms: Option<u64>,
    /// Optional tween duration for animated frame changes (ms).
    tween_ms: Option<u64>,
    /// Absolute frame to apply after delay `(x, y, w, h)`.
    apply_target: Option<(f64, f64, f64, f64)>,
    /// Grid target to apply after delay `(cols, rows, col, row)`.
    apply_grid: Option<(u32, u32, u32, u32)>,
    /// Immediate grid placement `(cols, rows, col, row)`.
    grid: Option<(u32, u32, u32, u32)>,
    /// Requested window size `(w, h)`.
    size: Option<(f64, f64)>,
    /// Requested window position `(x, y)`.
    pos: Option<(f64, f64)>,
    /// Optional label text rendered in the window.
    label_text: Option<String>,
    /// Minimum content size `(w, h)` enforced by the helper.
    min_size: Option<(f64, f64)>,
    /// Start minimized (miniaturized) if true.
    start_minimized: bool,
    /// Start zoomed (macOS zoom) if true.
    start_zoomed: bool,
    /// Make the window non-movable if true.
    nonmovable: bool,
    /// Make the window non-resizable if true.
    nonresizable: bool,
    /// Attach a sheet (AXRole=AXSheet) if true.
    attach_sheet: bool,
    /// Size increment rounding step `(w, h)`.
    step_size: Option<(f64, f64)>,
}

impl HelperWindowBuilder {
    /// Create a new helper window builder.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            time_ms: 30000, // Default 30 seconds
            delay_setframe_ms: None,
            delay_apply_ms: None,
            tween_ms: None,
            apply_target: None,
            apply_grid: None,
            grid: None,
            size: None,
            pos: None,
            label_text: None,
            min_size: None,
            start_minimized: false,
            start_zoomed: false,
            nonmovable: false,
            nonresizable: false,
            attach_sheet: false,
            step_size: None,
        }
    }

    /// Set the lifetime of the helper window in milliseconds.
    pub fn with_time_ms(mut self, ms: u64) -> Self {
        self.time_ms = ms;
        self
    }

    /// Place into an arbitrary grid cell (top-left origin)
    pub fn with_grid(mut self, cols: u32, rows: u32, col: u32, row: u32) -> Self {
        self.grid = Some((cols, rows, col, row));
        self
    }

    /// Set requested window size
    pub fn with_size(mut self, width: f64, height: f64) -> Self {
        self.size = Some((width, height));
        self
    }

    /// Set requested window position (x, y)
    pub fn with_position(mut self, x: f64, y: f64) -> Self {
        self.pos = Some((x, y));
        self
    }

    /// Explicit delayed-apply to a grid target `(cols,rows,col,row)` after `ms`.
    pub fn with_delay_apply_grid(
        mut self,
        ms: u64,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
    ) -> Self {
        self.delay_apply_ms = Some(ms);
        self.apply_grid = Some((cols, rows, col, row));
        self
    }

    /// Enable tweened apply: animate to the latest desired frame over `ms`.
    pub fn with_tween_ms(mut self, ms: u64) -> Self {
        self.tween_ms = Some(ms);
        self
    }

    /// Set explicit label text to display
    pub fn with_label_text(mut self, text: impl Into<String>) -> Self {
        self.label_text = Some(text.into());
        self
    }

    /// Enforce a minimum content size for the helper window.
    pub fn with_min_size(mut self, width: f64, height: f64) -> Self {
        self.min_size = Some((width.max(1.0), height.max(1.0)));
        self
    }

    /// Round requested sizes to nearest multiples of `(w, h)` in the helper.
    pub fn with_step_size(mut self, w: f64, h: f64) -> Self {
        self.step_size = Some((w, h));
        self
    }

    /// Start the helper minimized (miniaturized).
    pub fn with_start_minimized(mut self, v: bool) -> Self {
        self.start_minimized = v;
        self
    }

    /// Start the helper zoomed (macOS 'zoom' state).
    pub fn with_start_zoomed(mut self, v: bool) -> Self {
        self.start_zoomed = v;
        self
    }

    /// Make the helper window non-movable (sets NSWindow.movable=false).
    pub fn with_nonmovable(mut self, v: bool) -> Self {
        self.nonmovable = v;
        self
    }

    /// Make the helper window non-resizable.
    pub fn with_nonresizable(mut self, v: bool) -> Self {
        self.nonresizable = v;
        self
    }

    /// Attach a simple sheet to the helper window.
    pub fn with_attach_sheet(mut self, v: bool) -> Self {
        self.attach_sheet = v;
        self
    }

    fn configure_command(&self) -> Result<Command> {
        let exe = env::current_exe()?;
        let mut cmd = Command::new(exe);
        cmd.arg("focus-winhelper")
            .arg("--title")
            .arg(&self.title)
            .arg("--time")
            .arg(self.time_ms.to_string());
        if let Some(ms) = self.delay_setframe_ms {
            cmd.arg("--delay-setframe-ms").arg(ms.to_string());
        }
        if let Some(ms) = self.delay_apply_ms {
            cmd.arg("--delay-apply-ms").arg(ms.to_string());
        }
        if let Some(ms) = self.tween_ms {
            cmd.arg("--tween-ms").arg(ms.to_string());
        }
        if let Some((x, y, w, h)) = self.apply_target {
            cmd.arg("--apply-target").args([
                x.to_string(),
                y.to_string(),
                w.to_string(),
                h.to_string(),
            ]);
        }
        if let Some((c, r, col, row)) = self.apply_grid {
            cmd.arg("--apply-grid").args([
                c.to_string(),
                r.to_string(),
                col.to_string(),
                row.to_string(),
            ]);
        }
        if let Some((c, r, col, row)) = self.grid {
            cmd.arg("--grid").args([
                c.to_string(),
                r.to_string(),
                col.to_string(),
                row.to_string(),
            ]);
        }
        if let Some((w, h)) = self.size {
            cmd.arg("--size").args([w.to_string(), h.to_string()]);
        }
        if let Some((x, y)) = self.pos {
            cmd.arg("--pos").args([x.to_string(), y.to_string()]);
        }
        if let Some(ref txt) = self.label_text {
            cmd.arg("--label-text").arg(txt);
        }
        if let Some((w, h)) = self.min_size {
            cmd.arg("--min-size").args([w.to_string(), h.to_string()]);
        }
        if let Some((w, h)) = self.step_size {
            cmd.arg("--step-size").args([w.to_string(), h.to_string()]);
        }
        if self.start_minimized {
            cmd.arg("--start-minimized");
        }
        if self.start_zoomed {
            cmd.arg("--start-zoomed");
        }
        if self.nonmovable {
            cmd.arg("--panel-nonmovable");
        }
        if self.nonresizable {
            cmd.arg("--non-resizable");
        }
        if self.attach_sheet {
            cmd.arg("--attach-sheet");
        }
        Ok(cmd)
    }

    /// Spawn the helper window process.
    pub fn spawn(self) -> Result<ManagedChild> {
        let mut cmd = self.configure_command()?;
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        spawn_managed(cmd)
    }

    /// Spawn the helper window process with inherited stdout/stderr for debugging.
    pub fn spawn_inherit_io(self) -> Result<ManagedChild> {
        let mut cmd = self.configure_command()?;
        cmd.stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        spawn_managed(cmd)
    }
}

fn spawn_managed(mut cmd: Command) -> Result<ManagedChild> {
    let child = cmd.spawn().map_err(|e| Error::SpawnFailed(e.to_string()))?;
    Ok(ManagedChild::new(child))
}

/// Wait until the frontmost CG window has the given title.
pub fn wait_for_frontmost_title(expected: &str, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        if let Some(win) = mac_winops::frontmost_window()
            && win.title == expected
        {
            return true;
        }
        thread::sleep(config::ms(config::POLL_INTERVAL_MS));
    }
    false
}

/// Wait until a window with `(pid,title)` is visible via CG or AX.
pub fn wait_for_window_visible(pid: i32, title: &str, timeout_ms: u64, poll_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        let wins = mac_winops::list_windows();
        let cg_ok = wins.iter().any(|w| w.pid == pid && w.title == title);
        let ax_ok = mac_winops::ax_has_window_title(pid, title);
        if cg_ok || ax_ok {
            return true;
        }
        thread::sleep(config::ms(poll_ms));
    }
    false
}

/// Best-effort: bring the given window to the front by raising it or activating its PID.
pub fn ensure_frontmost(pid: i32, title: &str, attempts: usize, delay_ms: u64) {
    for _ in 0..attempts {
        if let Some(w) = mac_winops::list_windows()
            .into_iter()
            .find(|w| w.pid == pid && w.title == title)
        {
            drop(mac_winops::request_raise_window(pid, w.id));
        } else {
            drop(mac_winops::request_activate_pid(pid));
        }
        thread::sleep(config::ms(delay_ms));
        if wait_for_frontmost_title(title, delay_ms) {
            break;
        }
    }
}

/// Spawn a helper window with `title`, keep it alive for `lifetime_ms`, and
/// block until it’s visible (or return an error).
pub fn spawn_helper_visible(
    title: &str,
    lifetime_ms: u64,
    visible_timeout_ms: u64,
    poll_ms: u64,
    label_text: &str,
) -> Result<ManagedChild> {
    let helper = HelperWindowBuilder::new(title.to_string())
        .with_time_ms(lifetime_ms)
        .with_label_text(label_text)
        .spawn()?;
    if !wait_for_window_visible(helper.pid, title, visible_timeout_ms, poll_ms) {
        return Err(Error::FocusNotObserved {
            timeout_ms: visible_timeout_ms,
            expected: format!("helper window '{}' not visible", title),
        });
    }
    Ok(helper)
}

/// Variant allowing initial window state options.
pub fn spawn_helper_with_options(
    title: &str,
    lifetime_ms: u64,
    visible_timeout_ms: u64,
    poll_ms: u64,
    label_text: &str,
    start_minimized: bool,
    start_zoomed: bool,
) -> Result<ManagedChild> {
    let helper = HelperWindowBuilder::new(title.to_string())
        .with_time_ms(lifetime_ms)
        .with_label_text(label_text)
        .with_start_minimized(start_minimized)
        .with_start_zoomed(start_zoomed)
        .spawn()?;
    if !wait_for_window_visible(helper.pid, title, visible_timeout_ms, poll_ms) {
        return Err(Error::FocusNotObserved {
            timeout_ms: visible_timeout_ms,
            expected: format!("helper window '{}' not visible", title),
        });
    }
    Ok(helper)
}

/// RAII fixture for a helper window that ensures frontmost and cleans up on drop.
pub struct HelperWindow {
    /// Child process handle for the helper window.
    child: ManagedChild,
    /// Process identifier of the helper window.
    pub pid: i32,
}

impl HelperWindow {
    /// Spawn a helper window and ensure it becomes frontmost. Kills on drop.
    pub fn spawn_frontmost(
        title: &str,
        lifetime_ms: u64,
        visible_timeout_ms: u64,
        poll_ms: u64,
        label_text: &str,
    ) -> Result<Self> {
        let child =
            spawn_helper_visible(title, lifetime_ms, visible_timeout_ms, poll_ms, label_text)?;
        let pid = child.pid;
        ensure_frontmost(pid, title, 3, config::UI_ACTION_DELAY_MS);
        Ok(Self { child, pid })
    }

    /// Spawn using a preconfigured builder (for custom size/position), then ensure frontmost.
    pub fn spawn_frontmost_with_builder(
        builder: HelperWindowBuilder,
        expected_title: &str,
        visible_timeout_ms: u64,
        poll_ms: u64,
    ) -> Result<Self> {
        let child = builder.spawn()?;
        if !wait_for_window_visible(child.pid, expected_title, visible_timeout_ms, poll_ms) {
            return Err(Error::FocusNotObserved {
                timeout_ms: visible_timeout_ms,
                expected: format!("helper window '{}' not visible", expected_title),
            });
        }
        ensure_frontmost(child.pid, expected_title, 3, config::UI_ACTION_DELAY_MS);
        Ok(Self {
            pid: child.pid,
            child,
        })
    }

    /// Explicitly kill and wait for the helper process.
    pub fn kill_and_wait(&mut self) -> Result<()> {
        self.child.kill_and_wait()
    }
}
