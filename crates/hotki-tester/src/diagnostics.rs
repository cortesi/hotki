//! Logging helpers for placement diagnostics.

use hotki_server::WorldSnapshotLite;
use mac_winops::{AxProps, Rect};
use tracing::info;

/// Snapshot of a window combining world metadata and Accessibility details.
#[derive(Debug, Clone)]
pub struct WindowSnapshot {
    /// Application name from world snapshot.
    pub app: String,
    /// Window title from world snapshot.
    pub title: String,
    /// Process identifier.
    pub pid: i32,
    /// Core Graphics window identifier.
    pub window_id: u32,
    /// Display identifier with the greatest overlap, if known.
    pub display_id: Option<u32>,
    /// Z-order index (0 = frontmost).
    pub z: u32,
    /// Accessibility properties such as role/subrole and settable state.
    pub ax: Option<AxProps>,
    /// Window frame in screen coordinates, if available.
    pub frame: Option<Rect>,
}

/// Log a summary of the current permissions state.
pub fn log_permissions(accessibility_ok: bool, screen_ok: bool) {
    info!("Accessibility permission: {}", bool_label(accessibility_ok));
    info!("Screen Recording permission: {}", bool_label(screen_ok));
}

/// Log the contents of a world snapshot with a contextual label.
pub fn log_world_snapshot(label: &str, snapshot: &WorldSnapshotLite) {
    info!(
        label,
        windows = snapshot.windows.len(),
        focused_pid = snapshot.focused.as_ref().map(|a| a.pid),
        "World snapshot"
    );
    for win in &snapshot.windows {
        info!(
            label,
            z = win.z,
            focused = win.focused,
            pid = win.pid,
            id = win.id,
            app = win.app,
            title = win.title,
            display = win.display_id,
            "Window"
        );
    }
}

/// Log pre/post window state derived from snapshots.
pub fn log_window_snapshot(label: &str, snapshot: &WindowSnapshot) {
    let frame = snapshot
        .frame
        .map(format_rect)
        .unwrap_or_else(|| "<none>".into());
    info!(
        context = label,
        window.pid = snapshot.pid,
        window.id = snapshot.window_id,
        window.z = snapshot.z,
        window.display = snapshot.display_id,
        window.frame = %frame,
        window.app = snapshot.app,
        window.title = snapshot.title,
        "Window snapshot (world)"
    );
    if let Some(ax) = &snapshot.ax {
        info!(
            context = label,
            ax.role = ax.role.as_deref().unwrap_or("<unknown>"),
            ax.subrole = ax.subrole.as_deref().unwrap_or("<unknown>"),
            ax.can_set_pos = ax.can_set_pos,
            ax.can_set_size = ax.can_set_size,
            "Window snapshot (AX)"
        );
    } else {
        info!(context = label, "Window snapshot (AX unavailable)");
    }
}

/// Log the delta between two window snapshots when geometry was captured.
pub fn log_window_delta(
    label: &str,
    before: Option<&WindowSnapshot>,
    after: Option<&WindowSnapshot>,
) {
    match (before.and_then(|w| w.frame), after.and_then(|w| w.frame)) {
        (Some(prev), Some(next)) => {
            info!(
                context = label,
                delta.x = next.x - prev.x,
                delta.y = next.y - prev.y,
                delta.width = next.w - prev.w,
                delta.height = next.h - prev.h,
                "Window geometry delta"
            );
        }
        (None, Some(_)) => {
            info!(
                context = label,
                "Window geometry delta: previous frame missing; captured new frame"
            );
        }
        (Some(_), None) => {
            info!(
                context = label,
                "Window geometry delta: new frame missing after placement"
            );
        }
        (None, None) => {
            info!(
                context = label,
                "Window geometry delta: no geometry available before or after"
            );
        }
    }
}

/// Convert a boolean permission state into a human-readable label.
fn bool_label(v: bool) -> &'static str {
    if v { "granted" } else { "missing" }
}

/// Render a rectangle as a readable `(x, y) [w x h]` string with one decimal precision.
fn format_rect(Rect { x, y, w, h }: Rect) -> String {
    format!("({:.1}, {:.1}) [{:.1} x {:.1}]", x, y, w, h)
}
