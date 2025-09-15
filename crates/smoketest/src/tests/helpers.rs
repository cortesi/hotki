//! Shared helpers for smoketests to keep tests concise.

use std::{
    thread,
    time::{Duration, Instant},
};

use crate::{
    config,
    error::{Error, Result},
    process::{HelperWindowBuilder, ManagedChild},
};
use crate::server_drive;

/// Approximate float equality within `eps`.
pub fn approx(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

/// Wait until the frontmost CG window has the given title.
pub fn wait_for_frontmost_title(expected: &str, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
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
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
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

/// Wait until all `(pid,title)` pairs are visible via CG or AX.
pub fn wait_for_windows_visible(entries: &[(i32, &str)], timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        let wins = mac_winops::list_windows();
        let all_found = entries.iter().all(|(pid, title)| {
            let cg_present = wins.iter().any(|w| w.pid == *pid && w.title == *title);
            let ax_present = mac_winops::ax_has_window_title(*pid, title);
            cg_present || ax_present
        });
        if all_found {
            return true;
        }
        thread::sleep(config::ms(config::POLL_INTERVAL_MS));
    }
    false
}

/// Wait until the backend-reported focused window title equals `expected_title`.
///
/// This uses the lightweight shared RPC driver if available; otherwise, it
/// returns `false` without side effects. Prefer `wait_for_frontmost_title`
/// for acceptance checks that must reflect the actual CG frontmost window.
pub fn wait_for_backend_focused_title(expected_title: &str, timeout_ms: u64) -> bool {
    if server_drive::is_ready() {
        server_drive::wait_for_focused_title(expected_title, timeout_ms)
    } else {
        false
    }
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
