//! mac-winops: macOS window operations for Hotki.
//!
//! Provides APIs to toggle/set native full screen (AppKit-managed Space)
//! and non‑native full screen (maximize to visible screen frame) on the
//! currently focused window of a given PID.
//!
//! All operations require Accessibility permission.

use std::{collections::HashSet, ffi::c_void, ptr};

use core_foundation::{
    array::{CFArray, CFArrayGetCount, CFArrayGetValueAtIndex},
    base::{CFRelease, CFTypeRef, TCFType},
    string::{CFString, CFStringRef},
};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSScreen};
use objc2_foundation::MainThreadMarker;
use tracing::{debug, warn};

use mac_keycode::{Chord, Key, Modifier};
use relaykey::RelayKey;

mod ax;
mod cfutil;
mod error;
pub mod focus;
mod frame_storage;
mod geometry;
mod main_thread_ops;
mod raise;
mod window;

pub use error::{Error, Result};
pub use main_thread_ops::{
    MoveDir, request_activate_pid, request_fullscreen_nonnative, request_place_grid,
    request_place_move_grid, request_raise_window,
};
pub use raise::raise_window;
pub use window::{Pos, WindowInfo, frontmost_window, frontmost_window_for_pid, list_windows};

use ax::*;
pub use ax::{ax_window_frame, ax_window_position, ax_window_size};
use frame_storage::*;
use geometry::{CGPoint, CGSize, approx_eq_eps, rect_eq};
use main_thread_ops::{MAIN_OPS, MainOp};

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
}

/// Alias for CoreGraphics CGWindowID (kCGWindowNumber).
pub type WindowId = u32;

/// Desired state for operations that can turn on/off or toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desired {
    /// Set the state to on/enabled.
    On,
    /// Set the state to off/disabled.
    Off,
    /// Toggle the current state.
    Toggle,
}

/// Screen corner to place the window against so that a 1×1 px corner remains visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenCorner {
    /// Bottom-right corner of the screen.
    BottomRight,
    /// Bottom-left corner of the screen.
    BottomLeft,
    /// Top-left corner of the screen.
    TopLeft,
}

/// Best-effort AX presence check: return true if `pid` has any AX window
/// whose title exactly matches `expected_title`.
///
/// Returns `false` on any AX error or if Accessibility permission is missing.
pub fn ax_has_window_title(pid: i32, expected_title: &str) -> bool {
    // Quick permission gate
    if !permissions::accessibility_ok() {
        return false;
    }
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return false;
        }
        let mut wins_ref: CFTypeRef = ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(app, cfstr("AXWindows"), &mut wins_ref);
        if err != 0 || wins_ref.is_null() {
            CFRelease(app as CFTypeRef);
            return false;
        }
        let arr = CFArray::<*const c_void>::wrap_under_create_rule(wins_ref as _);
        for i in 0..CFArrayGetCount(arr.as_concrete_TypeRef()) {
            let wref = CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) as *mut c_void;
            if wref.is_null() {
                continue;
            }
            let mut title_ref: CFTypeRef = ptr::null_mut();
            let terr = AXUIElementCopyAttributeValue(wref, cfstr("AXTitle"), &mut title_ref);
            if terr != 0 || title_ref.is_null() {
                continue;
            }
            let cfs = CFString::wrap_under_create_rule(title_ref as CFStringRef);
            let title = cfs.to_string();
            // CFString object from Copy is consumed by wrap_under_create_rule
            if title == expected_title {
                CFRelease(app as CFTypeRef);
                return true;
            }
        }
        CFRelease(app as CFTypeRef);
    }
    false
}

fn focused_window_for_pid(pid: i32) -> Result<*mut c_void> {
    debug!("focused_window_for_pid: pid={}", pid);
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        warn!("focused_window_for_pid: AXUIElementCreateApplication returned null");
        return Err(Error::AppElement);
    }
    let attr_focused_window = cfstr("AXFocusedWindow");
    let mut win: CFTypeRef = ptr::null_mut();
    debug!("focused_window_for_pid: calling AXUIElementCopyAttributeValue(AXFocusedWindow)");
    let err = unsafe { AXUIElementCopyAttributeValue(app, attr_focused_window, &mut win) };
    debug!(
        "focused_window_for_pid: AXUIElementCopyAttributeValue returned err={} ptr={:?}",
        err, win
    );
    unsafe { CFRelease(app as CFTypeRef) };
    debug!("focused_window_for_pid: released app element");
    if err != 0 {
        warn!(
            "focused_window_for_pid: AX copy focused window failed: code {}",
            err
        );
        return Err(Error::AxCode(err));
    }
    if win.is_null() {
        debug!("focused_window_for_pid: no focused window");
        return Err(Error::FocusedWindow);
    }
    debug!("focused_window_for_pid: got focused window");
    Ok(win as *mut c_void)
}

/// Toggle or set native full screen (AXFullScreen) for the focused window of `pid`.
///
/// Requires Accessibility permission. If AXFullScreen is not available, the
/// function synthesizes the standard ⌃⌘F shortcut as a fallback.
pub fn fullscreen_native(pid: i32, desired: Desired) -> Result<()> {
    ax_check()?;
    let win = focused_window_for_pid(pid)?;
    let attr_fullscreen = cfstr("AXFullScreen");
    match ax_bool(win, attr_fullscreen) {
        Ok(Some(cur)) => {
            let target = match desired {
                Desired::On => true,
                Desired::Off => false,
                Desired::Toggle => !cur,
            };
            if target != cur {
                ax_set_bool(win, attr_fullscreen, target)?;
            }
            // win retained by AX; release
            unsafe { CFRelease(win as CFTypeRef) };
            Ok(())
        }
        // If the attribute is missing/unsupported, fall back to keystroke.
        _ => {
            unsafe { CFRelease(win as CFTypeRef) };
            // Fallback only makes sense for Toggle or “turn on/off” when app supports it via menu.
            let mut mods = HashSet::new();
            mods.insert(Modifier::Control);
            mods.insert(Modifier::Command);
            let chord = Chord {
                key: Key::F,
                modifiers: mods,
            };
            let rk = RelayKey::new();
            rk.key_down(pid, chord.clone(), false);
            rk.key_up(pid, chord);
            Ok(())
        }
    }
}

/// Toggle or set non‑native full screen (maximize to visible frame in current Space)
/// for the focused window of `pid`.
///
/// Requires Accessibility permission and must run on the AppKit main thread
/// (uses `NSScreen::visibleFrame`).
pub fn fullscreen_nonnative(pid: i32, desired: Desired) -> Result<()> {
    ax_check()?;
    // For visibleFrame we need AppKit; require main thread.
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let win = focused_window_for_pid(pid)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    // Read current frame
    let cur_p = ax_get_point(win, attr_pos)?;
    let cur_s = ax_get_size(win, attr_size)?;

    // Determine target screen visible frame
    let vf = visible_frame_containing_point(mtm, cur_p);
    let target_p = CGPoint { x: vf.0, y: vf.1 };
    let target_s = CGSize {
        width: vf.2,
        height: vf.3,
    };

    let mut prev_key: Option<(i32, WindowId)> = None;
    let is_full = rect_eq(cur_p, cur_s, target_p, target_s);
    let do_set_to_full = match desired {
        Desired::On => true,
        Desired::Off => false,
        Desired::Toggle => !is_full,
    };

    // Identify window key for restore using stable CGWindowID
    if let Some(w) = frontmost_window_for_pid(pid) {
        let wid = w.id;
        prev_key = Some((pid, wid));
    }

    if do_set_to_full {
        // Store previous frame if we have a key and not already stored
        if let Some(k) = prev_key
            && let Ok(mut map) = PREV_FRAMES.lock()
        {
            if map.len() >= PREV_FRAMES_CAP
                && let Some(old_k) = map.keys().next().cloned()
            {
                let _ = map.remove(&old_k);
            }
            map.entry(k).or_insert((cur_p, cur_s));
        }
        ax_set_point(win, attr_pos, target_p)?;
        ax_set_size(win, attr_size, target_s)?;
    } else {
        // Restore if available
        let restored = if let Some(k) = prev_key {
            if let Ok(mut map) = PREV_FRAMES.lock() {
                if let Some((p, s)) = map.remove(&k) {
                    if !rect_eq(p, s, cur_p, cur_s) {
                        ax_set_point(win, attr_pos, p)?;
                        ax_set_size(win, attr_size, s)?;
                    }
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };
        if !restored {
            debug!("no previous frame to restore; non-native Off is a no-op");
        }
    }
    unsafe { CFRelease(win as CFTypeRef) };
    Ok(())
}

// Compute the visible frame (excluding menu bar and Dock) of the screen
// containing `p`. Falls back to main screen when not found.
#[allow(dead_code)]
enum FrameKind {
    Visible,
    Full,
}

fn frame_containing_point_with(
    mtm: MainThreadMarker,
    p: CGPoint,
    kind: FrameKind,
) -> (f64, f64, f64, f64) {
    let screens = NSScreen::screens(mtm);
    let mut chosen = None;
    for s in screens.iter() {
        let fr = match kind {
            FrameKind::Visible => s.visibleFrame(),
            FrameKind::Full => s.frame(),
        };
        let x = fr.origin.x;
        let y = fr.origin.y;
        let w = fr.size.width;
        let h = fr.size.height;
        if p.x >= x && p.x <= x + w && p.y >= y && p.y <= y + h {
            chosen = Some(s);
            break;
        }
    }
    let rect = if let Some(scr) = chosen.or_else(|| NSScreen::mainScreen(mtm)) {
        match kind {
            FrameKind::Visible => scr.visibleFrame(),
            FrameKind::Full => scr.frame(),
        }
    } else {
        // Fallback to the first screen
        match NSScreen::screens(mtm).iter().next() {
            Some(s) => match kind {
                FrameKind::Visible => s.visibleFrame(),
                FrameKind::Full => s.frame(),
            },
            None => match kind {
                FrameKind::Visible => NSScreen::mainScreen(mtm).unwrap().visibleFrame(),
                FrameKind::Full => NSScreen::mainScreen(mtm).unwrap().frame(),
            },
        }
    };
    (
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    )
}

fn visible_frame_containing_point(mtm: MainThreadMarker, p: CGPoint) -> (f64, f64, f64, f64) {
    frame_containing_point_with(mtm, p, FrameKind::Visible)
}

/// Drain and execute any pending main-thread operations. Call from the Tao main thread
/// (e.g., in `Event::UserEvent`), after posting a user event via `focus::post_user_event()`.
pub fn drain_main_ops() {
    loop {
        let op_opt = MAIN_OPS.lock().ok().and_then(|mut q| q.pop_front());
        let Some(op) = op_opt else { break };
        match op {
            MainOp::FullscreenNonNative { pid, desired } => {
                let _ = fullscreen_nonnative(pid, desired);
            }
            MainOp::PlaceGrid {
                id,
                cols,
                rows,
                col,
                row,
            } => {
                let _ = place_grid(id, cols, rows, col, row);
            }
            MainOp::PlaceMoveGrid {
                id,
                cols,
                rows,
                dir,
            } => {
                let _ = place_move_grid(id, cols, rows, dir);
            }
            MainOp::ActivatePid { pid } => {
                let _ = activate_pid(pid);
            }
            MainOp::RaiseWindow { pid, id } => {
                let _ = crate::raise::raise_window(pid, id);
            }
        }
    }
}

/// Compute the visible frame for the screen containing the given window and
/// place the window into the specified grid cell (top-left is (0,0)).
fn place_grid(id: WindowId, cols: u32, rows: u32, col: u32, row: u32) -> Result<()> {
    ax_check()?;
    // For visibleFrame we need AppKit; require main thread.
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let (win, _pid_for_id) = ax_window_for_id(id)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    // Read current frame to find screen containing the window's point
    let cur_p = ax_get_point(win, attr_pos)?;
    let (vf_x, vf_y, vf_w, vf_h) = visible_frame_containing_point(mtm, cur_p);
    // Clamp to grid bounds defensively
    let col = col.min(cols.saturating_sub(1));
    let row = row.min(rows.saturating_sub(1));
    let (x, y, w, h) = cell_rect(
        vf_x,
        vf_y,
        vf_w.max(1.0),
        vf_h.max(1.0),
        cols,
        rows,
        col,
        row,
    );

    // Set position first, then size to avoid initial height clamping by AppKit
    ax_set_point(win, attr_pos, CGPoint { x, y })?;
    ax_set_size(
        win,
        attr_size,
        CGSize {
            width: w,
            height: h,
        },
    )?;

    unsafe { CFRelease(win as CFTypeRef) };
    Ok(())
}

/// Hide or reveal the focused window by sliding it so only a 1‑pixel corner
/// remains visible at the requested screen corner.
///
/// Behavior
/// - Requires Accessibility permission; no‑ops otherwise.
/// - Resolves a stable top‑level AXWindow (not relying on AX focus).
/// - When hiding, we overshoot off‑screen toward the chosen corner and rely on
///   WindowServer clamping, then apply a short tightening pass (binary‑step
///   nudges along X and Y) to squeeze any extra allowable off‑screen movement.
///   This achieves the smallest visible sliver permitted by macOS in practice.
/// - When revealing, we restore the previously stored (position, size).
///
/// Notes
/// - Coordinates use a top‑left origin (y increases downward).
/// - macOS won’t allow windows to be fully off‑screen; titlebar chrome may be
///   kept visible depending on the app/window style. The tightening pass helps
///   minimize this but cannot bypass system constraints.
pub fn hide_corner(pid: i32, desired: Desired, corner: ScreenCorner) -> Result<()> {
    debug!(
        "hide_corner: entry pid={} desired={:?} corner={:?}",
        pid, desired, corner
    );
    ax_check()?;
    let _ = request_activate_pid(pid);
    // Resolve a top-level AXWindow
    let win: *mut c_void = unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Err(Error::AppElement);
        }
        let mut wins_ref: CFTypeRef = ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(app, cfstr("AXWindows"), &mut wins_ref);
        if err != 0 || wins_ref.is_null() {
            CFRelease(app as CFTypeRef);
            return Err(Error::FocusedWindow);
        }
        let arr = CFArray::<*const c_void>::wrap_under_create_rule(wins_ref as _);
        let n = CFArrayGetCount(arr.as_concrete_TypeRef());
        let mut chosen: *mut c_void = ptr::null_mut();
        for i in 0..n {
            let w = CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) as *mut c_void;
            if w.is_null() {
                continue;
            }
            let role = ax_get_string(w, cfstr("AXRole")).unwrap_or_default();
            if role == "AXWindow" {
                chosen = w;
                break;
            }
        }
        if !chosen.is_null() {
            let _ = CFRetain(chosen as CFTypeRef);
        }
        CFRelease(app as CFTypeRef);
        if chosen.is_null() {
            return Err(Error::FocusedWindow);
        }
        chosen
    };
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    let cur_p = ax_get_point(win, attr_pos)?;
    let cur_s = ax_get_size(win, attr_size)?;

    let key = frontmost_window_for_pid(pid).map(|w| (pid, w.id));
    let is_hidden = key
        .as_ref()
        .and_then(|k| HIDDEN_FRAMES.lock().ok().map(|m| m.contains_key(k)))
        .unwrap_or(false);
    let do_hide = match desired {
        Desired::On => true,
        Desired::Off => false,
        Desired::Toggle => !is_hidden,
    };

    if do_hide {
        if let Some(k) = key
            && let Ok(mut map) = HIDDEN_FRAMES.lock()
        {
            if map.len() >= HIDDEN_FRAMES_CAP
                && let Some(old_k) = map.keys().next().cloned()
            {
                let _ = map.remove(&old_k);
            }
            map.insert(k, (cur_p, cur_s));
        }

        // Compute target for requested screen corner. We intentionally overshoot off-screen
        // by a large delta and rely on WindowServer clamping to achieve the tightest allowed
        // placement. This is more aggressive than an exact 1px calculation and tends to
        // minimize visible area, especially on the left edge.
        let (tx, ty) = match corner {
            ScreenCorner::BottomRight => (cur_p.x + 100_000.0, cur_p.y + 100_000.0),
            ScreenCorner::BottomLeft => (cur_p.x - 100_000.0, cur_p.y + 100_000.0),
            ScreenCorner::TopLeft => (cur_p.x - 100_000.0, cur_p.y - 100_000.0),
        };

        // Attempt move with size no-op first
        let _ = ax_set_size(win, attr_size, cur_s);
        ax_set_point(win, attr_pos, CGPoint { x: tx, y: ty })?;

        // If we didn't move horizontally, try a small nudge towards target and retry
        if let Ok(p1) = ax_get_point(win, attr_pos)
            && approx_eq_eps(p1.x, cur_p.x, 1.0)
        {
            let nudge_x = if tx >= cur_p.x {
                cur_p.x + 40.0
            } else {
                cur_p.x - 40.0
            };
            let _ = ax_set_point(
                win,
                attr_pos,
                CGPoint {
                    x: nudge_x,
                    y: cur_p.y,
                },
            );
            let _ = ax_set_point(win, attr_pos, CGPoint { x: tx, y: ty });
        }

        // Tightening pass: iteratively nudge toward the requested corner with
        // a diminishing step to squeeze a few extra pixels off-screen.
        if let Ok(mut best) = ax_get_point(win, attr_pos) {
            let mut step = 128.0;
            for _ in 0..3 {
                // X axis drive
                let mut local = step;
                while local >= 1.0 {
                    let cand = match corner {
                        ScreenCorner::BottomRight => CGPoint {
                            x: best.x + local,
                            y: best.y,
                        },
                        ScreenCorner::BottomLeft | ScreenCorner::TopLeft => CGPoint {
                            x: best.x - local,
                            y: best.y,
                        },
                    };
                    let _ = ax_set_point(win, attr_pos, cand);
                    if let Ok(np) = ax_get_point(win, attr_pos) {
                        let improved = match corner {
                            ScreenCorner::BottomRight => np.x > best.x + 0.5,
                            ScreenCorner::BottomLeft | ScreenCorner::TopLeft => np.x < best.x - 0.5,
                        };
                        if improved {
                            best = np;
                            continue;
                        }
                    }
                    local /= 2.0;
                }

                // Y axis drive
                let mut local = step;
                while local >= 1.0 {
                    let cand = match corner {
                        ScreenCorner::BottomRight | ScreenCorner::BottomLeft => CGPoint {
                            x: best.x,
                            y: best.y + local,
                        },
                        ScreenCorner::TopLeft => CGPoint {
                            x: best.x,
                            y: best.y - local,
                        },
                    };
                    let _ = ax_set_point(win, attr_pos, cand);
                    if let Ok(np) = ax_get_point(win, attr_pos) {
                        let improved = match corner {
                            ScreenCorner::BottomRight | ScreenCorner::BottomLeft => {
                                np.y > best.y + 0.5
                            }
                            ScreenCorner::TopLeft => np.y < best.y - 0.5,
                        };
                        if improved {
                            best = np;
                            continue;
                        }
                    }
                    local /= 2.0;
                }
                step /= 2.0;
            }
        }
    } else if let Some(k) = key
        && let Ok(mut map) = HIDDEN_FRAMES.lock()
        && let Some((p, s)) = map.remove(&k)
    {
        let _ = ax_set_size(win, attr_size, s);
        let _ = ax_set_point(win, attr_pos, p);
    }

    unsafe { CFRelease(win as CFTypeRef) };
    Ok(())
}

/// Hide or reveal the focused window at the bottom-left corner of the screen.
///
/// This is a convenience wrapper around `hide_corner` with `ScreenCorner::BottomLeft`.
pub fn hide_bottom_left(pid: i32, desired: Desired) -> Result<()> {
    hide_corner(pid, desired, ScreenCorner::BottomLeft)
}

#[allow(clippy::too_many_arguments)]
fn cell_rect(
    vf_x: f64,
    vf_y: f64,
    vf_w: f64,
    vf_h: f64,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
) -> (f64, f64, f64, f64) {
    let c = cols.max(1) as f64;
    let r = rows.max(1) as f64;
    let tile_w = (vf_w / c).floor().max(1.0);
    let tile_h = (vf_h / r).floor().max(1.0);
    let rem_w = vf_w - tile_w * (cols as f64);
    let rem_h = vf_h - tile_h * (rows as f64);

    let x = vf_x + tile_w * (col as f64);
    let w = if col == cols.saturating_sub(1) {
        tile_w + rem_w
    } else {
        tile_w
    };
    let y = if row == rows.saturating_sub(1) {
        vf_y
    } else {
        vf_y + rem_h + tile_h * ((rows - 1 - row) as f64)
    };
    let h = if row == rows.saturating_sub(1) {
        tile_h + rem_h
    } else {
        tile_h
    };
    (x, y, w, h)
}

#[allow(clippy::too_many_arguments)]
fn find_cell_for_window(
    vf_x: f64,
    vf_y: f64,
    vf_w: f64,
    vf_h: f64,
    cols: u32,
    rows: u32,
    pos: CGPoint,
    size: CGSize,
    eps: f64,
) -> Option<(u32, u32)> {
    for row in 0..rows {
        for col in 0..cols {
            let (x, y, w, h) = cell_rect(vf_x, vf_y, vf_w, vf_h, cols, rows, col, row);
            if approx_eq_eps(pos.x, x, eps)
                && approx_eq_eps(pos.y, y, eps)
                && approx_eq_eps(size.width, w, eps)
                && approx_eq_eps(size.height, h, eps)
            {
                return Some((col, row));
            }
        }
    }
    None
}

fn place_move_grid(id: WindowId, cols: u32, rows: u32, dir: MoveDir) -> Result<()> {
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let (win, _pid_for_id) = ax_window_for_id(id)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    let cur_p = ax_get_point(win, attr_pos)?;
    let cur_s = ax_get_size(win, attr_size)?;
    let (vf_x, vf_y, vf_w, vf_h) = visible_frame_containing_point(mtm, cur_p);

    let eps = 2.0;
    let cur_cell = find_cell_for_window(vf_x, vf_y, vf_w, vf_h, cols, rows, cur_p, cur_s, eps);

    // First invocation from a non-aligned position places at visual top‑left.
    // With our row indexing, some environments report coordinates such that
    // choosing row 0 may map to the bottom visually; prefer the row that
    // aligns to the topmost cell for first placement.
    let (next_col, next_row) = match cur_cell {
        None => (0, rows.saturating_sub(1)),
        Some((c, r)) => {
            let (mut nc, mut nr) = (c, r);
            match dir {
                MoveDir::Left => {
                    nc = nc.saturating_sub(1);
                }
                MoveDir::Right => {
                    if nc + 1 < cols {
                        nc += 1;
                    }
                }
                // Up decreases visual Y (moves down one row index in top-left origin)
                MoveDir::Up => {
                    if nr + 1 < rows {
                        nr += 1;
                    }
                }
                // Down increases visual Y (moves up one row index)
                MoveDir::Down => {
                    nr = nr.saturating_sub(1);
                }
            }
            (nc, nr)
        }
    };

    let (x, y, w, h) = cell_rect(vf_x, vf_y, vf_w, vf_h, cols, rows, next_col, next_row);
    ax_set_point(win, attr_pos, CGPoint { x, y })?;
    ax_set_size(
        win,
        attr_size,
        CGSize {
            width: w,
            height: h,
        },
    )?;
    unsafe { CFRelease(win as CFTypeRef) };
    Ok(())
}

/// Resolve an AX window element for a given CG `WindowId`. Returns the AX element and owning PID.
fn ax_window_for_id(id: WindowId) -> Result<(*mut c_void, i32)> {
    // Look up pid via CG, then match AXWindowNumber.
    let info = list_windows()
        .into_iter()
        .find(|w| w.id == id)
        .ok_or(Error::FocusedWindow)?;
    let pid = info.pid;
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return Err(Error::AppElement);
    }
    let mut wins_ref: CFTypeRef = ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(app, cfstr("AXWindows"), &mut wins_ref) };
    if err != 0 || wins_ref.is_null() {
        unsafe { CFRelease(app as CFTypeRef) };
        return Err(Error::AxCode(err));
    }
    let arr = unsafe {
        core_foundation::array::CFArray::<*const c_void>::wrap_under_create_rule(wins_ref as _)
    };
    let mut found: *mut c_void = ptr::null_mut();
    for i in 0..unsafe { CFArrayGetCount(arr.as_concrete_TypeRef()) } {
        let wref = unsafe { CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) } as *mut c_void;
        if wref.is_null() {
            continue;
        }
        let mut num_ref: CFTypeRef = ptr::null_mut();
        let nerr =
            unsafe { AXUIElementCopyAttributeValue(wref, cfstr("AXWindowNumber"), &mut num_ref) };
        if nerr != 0 || num_ref.is_null() {
            continue;
        }
        let cfnum =
            unsafe { core_foundation::number::CFNumber::wrap_under_create_rule(num_ref as _) };
        let wid = cfnum.to_i64().unwrap_or(0) as u32;
        if wid == id {
            found = wref;
            break;
        }
    }
    if found.is_null() {
        unsafe { CFRelease(app as CFTypeRef) };
        return Err(Error::FocusedWindow);
    }
    unsafe { CFRetain(found as CFTypeRef) };
    unsafe { CFRelease(app as CFTypeRef) };
    Ok((found, pid))
}

/// Perform activation of an app by pid using NSRunningApplication. Main-thread only.
fn activate_pid(pid: i32) -> Result<()> {
    let _mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    // SAFETY: Objective-C calls are performed with typed wrappers.
    let app = unsafe {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid as libc::pid_t)
    };
    if let Some(app) = app {
        // Prefer bringing all windows forward.
        let ok =
            unsafe { app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows) };
        if !ok {
            warn!(
                "NSRunningApplication.activateWithOptions returned false for pid={}",
                pid
            );
        } else {
            debug!("Activated app via NSRunningApplication for pid={}", pid);
        }
        Ok(())
    } else {
        Err(Error::ActivationFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::cell_rect;

    #[test]
    fn cell_rect_corners_and_remainders() {
        // Visible frame 100x100, 3x2 grid -> tile 33x50 with remainders w:1, h:0
        let (vf_x, vf_y, vf_w, vf_h) = (0.0, 0.0, 100.0, 100.0);
        // top-left (col 0, row 1 in top-left origin mapping)
        let (x0, y0, w0, h0) = cell_rect(vf_x, vf_y, vf_w, vf_h, 3, 2, 0, 1);
        assert_eq!((x0, y0, w0, h0), (0.0, 0.0, 33.0, 50.0));

        // top-right should absorb remainder width
        let (x1, y1, w1, h1) = cell_rect(vf_x, vf_y, vf_w, vf_h, 3, 2, 2, 1);
        assert_eq!((x1, y1, w1, h1), (66.0, 0.0, 34.0, 50.0));

        // bottom row (row 0) gets full tile height; top row (row 1) as above
        let (_x2, y2, _w2, _h2) = cell_rect(vf_x, vf_y, vf_w, vf_h, 3, 2, 0, 0);
        assert_eq!(y2, 50.0);
    }
}
