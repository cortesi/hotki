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
mod geom;
mod main_thread_ops;
pub mod nswindow;
mod raise;
pub mod screen;
mod window;

pub use error::{Error, Result};
pub use main_thread_ops::{
    MoveDir, request_activate_pid, request_focus_dir, request_fullscreen_native,
    request_fullscreen_nonnative, request_place_grid, request_place_grid_focused,
    request_place_move_grid, request_raise_window,
};
pub use raise::raise_window;
pub use window::{Pos, WindowInfo, frontmost_window, frontmost_window_for_pid, list_windows};

use crate::geom::overshoot_target as geom_overshoot;
use ax::*;
pub use ax::{ax_window_frame, ax_window_position, ax_window_size};
use frame_storage::*;
use geom::{CGPoint, CGSize, approx_eq_eps, rect_eq};
use main_thread_ops::{MAIN_OPS, MainOp};

/// Applications to skip when determining focus/frontmost windows.
/// These are system or overlay processes that shouldn't count as focus owners.
pub const FOCUS_SKIP_APPS: &[&str] = &[
    "WindowManager",
    "Dock",
    "Control Center",
    "Spotlight",
    "Window Server",
    "hotki",
    "Hotki",
];

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
}

/// Alias for CoreGraphics CGWindowID (kCGWindowNumber).
pub type WindowId = u32;

/// RAII guard that releases a retained AX element on drop.
struct AXElem(*mut c_void);
impl AXElem {
    #[inline]
    fn new(ptr: *mut c_void) -> Self {
        Self(ptr)
    }
    #[inline]
    fn as_ptr(&self) -> *mut c_void {
        self.0
    }
}
impl Drop for AXElem {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0 as CFTypeRef) };
    }
}

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

    // Prefer scanning AXWindows for AXFocused/AXMain to avoid AXFocusedWindow crash on macOS 15.5.
    let mut wins_ref: CFTypeRef = ptr::null_mut();
    let err_w = unsafe { AXUIElementCopyAttributeValue(app, cfstr("AXWindows"), &mut wins_ref) };
    if err_w == 0 && !wins_ref.is_null() {
        let arr = unsafe { CFArray::<*const c_void>::wrap_under_create_rule(wins_ref as _) };
        let n = unsafe { CFArrayGetCount(arr.as_concrete_TypeRef()) };
        for i in 0..n {
            let w = unsafe { CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) } as *mut c_void;
            if w.is_null() {
                continue;
            }
            // Prefer AXFocused; fall back to AXMain
            if let Ok(Some(true)) = ax_bool(w, cfstr("AXFocused")) {
                unsafe { CFRetain(w as CFTypeRef) };
                unsafe { CFRelease(app as CFTypeRef) };
                debug!("focused_window_for_pid: found window via AXFocused");
                return Ok(w);
            }
            if let Ok(Some(true)) = ax_bool(w, cfstr("AXMain")) {
                unsafe { CFRetain(w as CFTypeRef) };
                unsafe { CFRelease(app as CFTypeRef) };
                debug!("focused_window_for_pid: found window via AXMain");
                return Ok(w);
            }
        }
    }

    // Fallback: try mapping CG frontmost window for pid via AXWindowNumber.
    if let Some(info) = frontmost_window_for_pid(pid) {
        // Reuse existing helper to resolve AX element by CGWindowID
        if let Ok((w, _pid)) = ax_window_for_id(info.id) {
            unsafe { CFRelease(app as CFTypeRef) };
            debug!("focused_window_for_pid: fallback via AXWindowNumber");
            return Ok(w);
        }
    }
    // Final fallback: choose the first top-level AXWindow from AXWindows list.
    unsafe {
        let mut wins_ref: CFTypeRef = ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(app, cfstr("AXWindows"), &mut wins_ref);
        if err == 0 && !wins_ref.is_null() {
            let arr = CFArray::<*const c_void>::wrap_under_create_rule(wins_ref as _);
            let n = CFArrayGetCount(arr.as_concrete_TypeRef());
            for i in 0..n {
                let w = CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) as *mut c_void;
                if w.is_null() {
                    continue;
                }
                let role = ax_get_string(w, cfstr("AXRole")).unwrap_or_default();
                if role == "AXWindow" {
                    CFRetain(w as CFTypeRef);
                    CFRelease(app as CFTypeRef);
                    debug!("focused_window_for_pid: fallback to first AXWindow entry");
                    return Ok(w);
                }
            }
        }
    }
    unsafe { CFRelease(app as CFTypeRef) };
    debug!("focused_window_for_pid: no focused window");
    Err(Error::FocusedWindow)
}

/// Toggle or set native full screen (AXFullScreen) for the focused window of `pid`.
///
/// Requires Accessibility permission. If AXFullScreen is not available, the
/// function synthesizes the standard ⌃⌘F shortcut as a fallback.
pub fn fullscreen_native(pid: i32, desired: Desired) -> Result<()> {
    tracing::info!(
        "WinOps: fullscreen_native enter pid={} desired={:?}",
        pid,
        desired
    );
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
            tracing::info!("WinOps: fullscreen_native fallback keystroke sent");
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
    tracing::info!(
        "WinOps: fullscreen_nonnative enter pid={} desired={:?}",
        pid,
        desired
    );
    ax_check()?;
    // For visibleFrame we need AppKit; require main thread.
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    tracing::debug!("WinOps: have MainThreadMarker");
    let win = focused_window_for_pid(pid)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    // Run body in a closure so we always release `win`.
    let result = (|| -> Result<()> {
        // Read current frame
        let cur_p = ax_get_point(win, attr_pos)?;
        let cur_s = ax_get_size(win, attr_size)?;

        // Determine target screen visible frame
        let vf = visible_frame_containing_point(mtm, cur_p);
        tracing::debug!(
            "WinOps: visible frame x={} y={} w={} h={}",
            vf.0,
            vf.1,
            vf.2,
            vf.3
        );
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
            tracing::debug!("WinOps: setting to non-native fullscreen frame");
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
            tracing::debug!("WinOps: restoring from previous frame if any");
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
        Ok(())
    })();
    unsafe { CFRelease(win as CFTypeRef) };
    tracing::info!("WinOps: fullscreen_nonnative exit");
    result
}

// Compute the visible frame (excluding menu bar and Dock) of the screen
// containing `p`. Falls back to main screen when not found.
fn visible_frame_containing_point(mtm: MainThreadMarker, p: CGPoint) -> (f64, f64, f64, f64) {
    // Try to find a screen containing the point.
    let mut chosen = None;
    for s in NSScreen::screens(mtm).iter() {
        let fr = s.visibleFrame();
        let r = geom::Rect {
            x: fr.origin.x,
            y: fr.origin.y,
            w: fr.size.width,
            h: fr.size.height,
        };
        if geom::point_in_rect(p.x, p.y, &r) {
            chosen = Some(s);
            break;
        }
    }
    // Prefer the chosen screen; otherwise try main, then first.
    if let Some(scr) = chosen.or_else(|| NSScreen::mainScreen(mtm)) {
        let r = scr.visibleFrame();
        return (r.origin.x, r.origin.y, r.size.width, r.size.height);
    }
    if let Some(s) = NSScreen::screens(mtm).iter().next() {
        let r = s.visibleFrame();
        return (r.origin.x, r.origin.y, r.size.width, r.size.height);
    }
    // As a last resort, return a zero rect to avoid panics.
    (0.0, 0.0, 0.0, 0.0)
}

/// Drain and execute any pending main-thread operations. Call from the Tao main thread
/// (e.g., in `Event::UserEvent`), after posting a user event via `focus::post_user_event()`.
pub fn drain_main_ops() {
    loop {
        let op_opt = MAIN_OPS.lock().ok().and_then(|mut q| q.pop_front());
        let Some(op) = op_opt else { break };
        match op {
            MainOp::FullscreenNative { pid, desired } => {
                tracing::info!(
                    "MainOps: drain FullscreenNative pid={} desired={:?}",
                    pid,
                    desired
                );
                let _ = fullscreen_native(pid, desired);
            }
            MainOp::FullscreenNonNative { pid, desired } => {
                tracing::info!(
                    "MainOps: drain FullscreenNonNative pid={} desired={:?}",
                    pid,
                    desired
                );
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
            MainOp::PlaceGridFocused {
                pid,
                cols,
                rows,
                col,
                row,
            } => {
                let _ = place_grid_focused(pid, cols, rows, col, row);
            }
            MainOp::ActivatePid { pid } => {
                let _ = activate_pid(pid);
            }
            MainOp::RaiseWindow { pid, id } => {
                let _ = crate::raise::raise_window(pid, id);
            }
            MainOp::FocusDir { dir } => {
                let _ = focus_dir(dir);
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

    let result = (|| -> Result<()> {
        // Read current frame to find screen containing the window's point
        let cur_p = ax_get_point(win, attr_pos)?;
        let (vf_x, vf_y, vf_w, vf_h) = visible_frame_containing_point(mtm, cur_p);
        // Clamp to grid bounds defensively
        let col = col.min(cols.saturating_sub(1));
        let row = row.min(rows.saturating_sub(1));
        let (x, y, w, h) = geom::grid_cell_rect(
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
        Ok(())
    })();
    unsafe { CFRelease(win as CFTypeRef) };
    result
}

/// Focus the next window in the given direction on the current screen within the
/// current Space. Uses CG for enumeration + AppKit for screen geometry and AX for
/// the origin window frame and final raise.
fn focus_dir(dir: MoveDir) -> Result<()> {
    ax_check()?;
    let _mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;

    // Determine origin via CG frontmost window (layer 0 preferred by frontmost_window)
    let origin = match frontmost_window() {
        Some(w) => w,
        None => return Err(Error::FocusedWindow),
    };

    (|| -> Result<()> {
        // Use AX for origin geometry (AppKit coords, bottom-left origin)
        let (ax_origin, _pid_for_id) = ax_window_for_id(origin.id)?;
        let o_pos = ax_get_point(ax_origin, cfstr("AXPosition"))?;
        let o_size = ax_get_size(ax_origin, cfstr("AXSize"))?;
        let o_rect = geom::Rect {
            x: o_pos.x,
            y: o_pos.y,
            w: o_size.width.max(1.0),
            h: o_size.height.max(1.0),
        };
        let o_cx = o_rect.cx();
        let o_cy = o_rect.cy();
        tracing::info!(
            "FocusDir origin(AX): pid={} id={} x={:.1} y={:.1} w={:.1} h={:.1}",
            origin.pid,
            origin.id,
            o_rect.x,
            o_rect.y,
            o_rect.w,
            o_rect.h
        );

        // Geometry helpers provided by crate::geom

        let eps = 16.0f64; // generous tolerance to absorb CG/AX rounding and menu bar offsets

        // Scan CG windows for candidates
        let all = list_windows();
        // Track best candidates with preference for same-app windows
        let mut primary_best_same: Option<(i32, f64, f64, i32, WindowId)> = None;
        let mut primary_best_other: Option<(i32, f64, f64, i32, WindowId)> = None;
        let mut fallback_best_same: Option<(i32, f64, i32, WindowId)> = None;
        let mut fallback_best_other: Option<(i32, f64, i32, WindowId)> = None;

        for w in all.into_iter() {
            if w.layer != 0 {
                continue;
            }
            if w.pid == origin.pid && w.id == origin.id {
                continue;
            }
            // Space filter: prefer matching origin's Space; allow None (unknown) to pass.
            if let Some(s) = origin.space
                && let Some(ws) = w.space
                && ws != s
            {
                continue;
            }
            // Retrieve candidate geometry via AX to stay in AppKit coordinate space
            let (cand_left, cand_bottom, cand_w, cand_h, id_match) = {
                match ax_window_for_id(w.id) {
                    Ok((cax, _)) => {
                        // determine if AXWindowNumber matches CG id (prefer these)
                        let mut id_match = false;
                        unsafe {
                            use core_foundation::base::TCFType;
                            let mut num_ref: CFTypeRef = ptr::null_mut();
                            let nerr = AXUIElementCopyAttributeValue(
                                cax,
                                cfstr("AXWindowNumber"),
                                &mut num_ref,
                            );
                            if nerr == 0 && !num_ref.is_null() {
                                let cfnum =
                                    core_foundation::number::CFNumber::wrap_under_create_rule(
                                        num_ref as _,
                                    );
                                let wid = cfnum.to_i64().unwrap_or(0) as u32;
                                if wid == w.id {
                                    id_match = true;
                                }
                            }
                        }
                        let p = match ax_get_point(cax, cfstr("AXPosition")) {
                            Ok(v) => v,
                            Err(_) => {
                                unsafe { CFRelease(cax as CFTypeRef) };
                                continue;
                            }
                        };
                        let s = match ax_get_size(cax, cfstr("AXSize")) {
                            Ok(v) => v,
                            Err(_) => {
                                unsafe { CFRelease(cax as CFTypeRef) };
                                continue;
                            }
                        };
                        unsafe { CFRelease(cax as CFTypeRef) };
                        (p.x, p.y, s.width.max(1.0), s.height.max(1.0), id_match)
                    }
                    Err(_) => continue,
                }
            };
            let c_rect = geom::Rect {
                x: cand_left,
                y: cand_bottom,
                w: cand_w,
                h: cand_h,
            };
            let cx = c_rect.cx();
            let cy = c_rect.cy();

            // (v1) Do not restrict to same screen to improve robustness.

            // Primary gating: edge-first selection within direction beam
            // Require substantial alignment on the orthogonal axis to avoid diagonal hops.
            let same_row = geom::same_row_by_overlap(&o_rect, &c_rect, 0.8);
            let same_col = geom::same_col_by_overlap(&o_rect, &c_rect, 0.8);
            let primary = match dir {
                MoveDir::Right => {
                    if c_rect.left() >= o_rect.right() - eps && same_row {
                        Some((c_rect.left() - o_rect.right(), (cy - o_cy).abs()))
                    } else {
                        None
                    }
                }
                MoveDir::Left => {
                    if c_rect.right() <= o_rect.left() + eps && same_row {
                        Some((o_rect.left() - c_rect.right(), (cy - o_cy).abs()))
                    } else {
                        None
                    }
                }
                MoveDir::Up => {
                    // Top-left origin semantics: above means candidate's top <= origin's bottom + eps
                    if c_rect.top() <= o_rect.bottom() + eps && same_col {
                        Some((o_rect.bottom() - c_rect.top(), (cx - o_cx).abs()))
                    } else {
                        None
                    }
                }
                MoveDir::Down => {
                    // Top-left origin semantics: below means candidate's bottom >= origin's top - eps
                    if c_rect.bottom() >= o_rect.top() - eps && same_col {
                        Some((c_rect.bottom() - o_rect.top(), (cx - o_cx).abs()))
                    } else {
                        None
                    }
                }
            };

            if let Some((axis_delta, tie)) = primary {
                let best_slot = if w.app == origin.app {
                    &mut primary_best_same
                } else {
                    &mut primary_best_other
                };
                let pref = if id_match { 0 } else { 1 };
                match best_slot {
                    None => *best_slot = Some((pref, axis_delta, tie, w.pid, w.id)),
                    Some((best_pref, best_axis, best_tie, _, _)) => {
                        if pref < *best_pref
                            || (pref == *best_pref
                                && (axis_delta < *best_axis
                                    || (approx_eq_eps(axis_delta, *best_axis, eps)
                                        && tie < *best_tie)))
                        {
                            *best_slot = Some((pref, axis_delta, tie, w.pid, w.id));
                        }
                    }
                }
                tracing::debug!(
                    "FocusDir primary cand pid={} id={} pref={} axis_delta={:.1} tie={:.1}",
                    w.pid,
                    w.id,
                    pref,
                    axis_delta,
                    tie
                );
                continue;
            }

            // Fallback: center-based directional gating with axis-biased distance
            let dx = cx - o_cx;
            let dy = cy - o_cy;
            let fallback_ok = match dir {
                MoveDir::Right => dx > eps,
                MoveDir::Left => dx < -eps,
                // Top-left origin semantics: moving up decreases Y, down increases Y
                MoveDir::Up => dy < -eps,
                MoveDir::Down => dy > eps,
            };
            if !fallback_ok {
                continue;
            }
            let score = match dir {
                MoveDir::Right | MoveDir::Left => {
                    let bias = if same_row { 0.25 } else { 1.0 };
                    let (primary, secondary) =
                        geom::center_distance_bias(&o_rect, &c_rect, geom::Axis::Horizontal);
                    primary * primary + (bias * secondary) * (bias * secondary)
                }
                MoveDir::Up | MoveDir::Down => {
                    let bias = if same_col { 0.25 } else { 1.0 };
                    let (primary, secondary) =
                        geom::center_distance_bias(&o_rect, &c_rect, geom::Axis::Vertical);
                    primary * primary + (bias * secondary) * (bias * secondary)
                }
            };
            let best_slot = if w.app == origin.app {
                &mut fallback_best_same
            } else {
                &mut fallback_best_other
            };
            let pref = if id_match { 0 } else { 1 };
            match best_slot {
                None => *best_slot = Some((pref, score, w.pid, w.id)),
                Some((best_pref, best, _, _)) => {
                    if pref < *best_pref || (pref == *best_pref && score < *best) {
                        *best_slot = Some((pref, score, w.pid, w.id));
                    }
                }
            }
            tracing::debug!(
                "FocusDir fallback cand pid={} id={} pref={} score={:.1}",
                w.pid,
                w.id,
                pref,
                score
            );
        }

        // Choose target
        let target = if let Some((_pref, _d, _t, pid, id)) = primary_best_same {
            Some((pid, id))
        } else if let Some((_pref, _d, _t, pid, id)) = primary_best_other {
            Some((pid, id))
        } else if let Some((_pref, _s, pid, id)) = fallback_best_same {
            Some((pid, id))
        } else if let Some((_pref, _s, pid, id)) = fallback_best_other {
            Some((pid, id))
        } else {
            None
        };
        if let Some((pid, id)) = target {
            tracing::info!("FocusDir target pid={} id={}", pid, id);
            let _ = crate::raise::raise_window(pid, id);
            let _ = request_activate_pid(pid);
        }
        Ok(())
    })()
}

/// Place the currently focused window of `pid` into the specified grid cell on its current screen.
///
/// This is a convenience variant of `place_grid` that resolves the window via Accessibility focus
/// rather than a CGWindowID and performs the move immediately (no main-op queueing).
#[allow(clippy::too_many_arguments)]
pub fn place_grid_focused(pid: i32, cols: u32, rows: u32, col: u32, row: u32) -> Result<()> {
    tracing::info!(
        "WinOps: place_grid_focused enter pid={} cols={} rows={} col={} row={}",
        pid,
        cols,
        rows,
        col,
        row
    );
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let win = focused_window_for_pid(pid)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");
    let result = (|| -> Result<()> {
        let cur_p = ax_get_point(win, attr_pos)?;
        let cur_s = ax_get_size(win, attr_size)?;
        let (vf_x, vf_y, vf_w, vf_h) = visible_frame_containing_point(mtm, cur_p);
        let col = col.min(cols.saturating_sub(1));
        let row = row.min(rows.saturating_sub(1));
        let (x, y, w, h) = geom::grid_cell_rect(
            vf_x,
            vf_y,
            vf_w.max(1.0),
            vf_h.max(1.0),
            cols,
            rows,
            col,
            row,
        );
        tracing::debug!(
            "WinOps: place_grid_focused current x={:.1} y={:.1} w={:.1} h={:.1} | target x={:.1} y={:.1} w={:.1} h={:.1}",
            cur_p.x,
            cur_p.y,
            cur_s.width,
            cur_s.height,
            x,
            y,
            w,
            h
        );
        ax_set_point(win, attr_pos, CGPoint { x, y })?;
        ax_set_size(
            win,
            attr_size,
            CGSize {
                width: w,
                height: h,
            },
        )?;
        Ok(())
    })();
    unsafe { CFRelease(win as CFTypeRef) };
    tracing::info!("WinOps: place_grid_focused exit");
    result
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
    let win_raw: *mut c_void = unsafe {
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
    let win = AXElem::new(win_raw);
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");
    let cur_p = ax_get_point(win.as_ptr(), attr_pos)?;
    let cur_s = ax_get_size(win.as_ptr(), attr_size)?;

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

        // Compute target for requested screen corner using a large overshoot; rely on
        // WindowServer clamping to achieve the tightest allowed placement.
        let tgt = geom_overshoot(cur_p, corner, 100_000.0);
        let (tx, ty) = (tgt.x, tgt.y);

        // Attempt move with size no-op first
        let _ = ax_set_size(win.as_ptr(), attr_size, cur_s);
        ax_set_point(win.as_ptr(), attr_pos, CGPoint { x: tx, y: ty })?;

        // If we didn't move horizontally, try a small nudge towards target and retry
        if let Ok(p1) = ax_get_point(win.as_ptr(), attr_pos)
            && approx_eq_eps(p1.x, cur_p.x, 1.0)
        {
            let nudge_x = if tx >= cur_p.x {
                cur_p.x + 40.0
            } else {
                cur_p.x - 40.0
            };
            let _ = ax_set_point(
                win.as_ptr(),
                attr_pos,
                CGPoint {
                    x: nudge_x,
                    y: cur_p.y,
                },
            );
            let _ = ax_set_point(win.as_ptr(), attr_pos, CGPoint { x: tx, y: ty });
        }

        // Tightening pass: iteratively nudge toward the requested corner with
        // a diminishing step to squeeze a few extra pixels off-screen.
        if let Ok(mut best) = ax_get_point(win.as_ptr(), attr_pos) {
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
                    let _ = ax_set_point(win.as_ptr(), attr_pos, cand);
                    if let Ok(np) = ax_get_point(win.as_ptr(), attr_pos) {
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
                    let _ = ax_set_point(win.as_ptr(), attr_pos, cand);
                    if let Ok(np) = ax_get_point(win.as_ptr(), attr_pos) {
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
        let _ = ax_set_size(win.as_ptr(), attr_size, s);
        let _ = ax_set_point(win.as_ptr(), attr_pos, p);
    }

    Ok(())
}

/// Hide or reveal the focused window at the bottom-left corner of the screen.
///
/// This is a convenience wrapper around `hide_corner` with `ScreenCorner::BottomLeft`.
pub fn hide_bottom_left(pid: i32, desired: Desired) -> Result<()> {
    hide_corner(pid, desired, ScreenCorner::BottomLeft)
}

// grid helpers now provided by crate::geom

fn place_move_grid(id: WindowId, cols: u32, rows: u32, dir: MoveDir) -> Result<()> {
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let (win, _pid_for_id) = ax_window_for_id(id)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    let result = (|| -> Result<()> {
        let cur_p = ax_get_point(win, attr_pos)?;
        let cur_s = ax_get_size(win, attr_size)?;
        let (vf_x, vf_y, vf_w, vf_h) = visible_frame_containing_point(mtm, cur_p);

        let eps = 2.0;
        let cur_cell = geom::grid_find_cell(vf_x, vf_y, vf_w, vf_h, cols, rows, cur_p, cur_s, eps);

        // First invocation from a non-aligned position places at visual top‑left (row 0).
        let (next_col, next_row) = match cur_cell {
            None => (0, 0),
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
                    // In bottom-left coords, moving up decreases the row index (towards 0)
                    MoveDir::Up => {
                        nr = nr.saturating_sub(1);
                    }
                    // Moving down increases the row index (towards rows-1)
                    MoveDir::Down => {
                        if nr + 1 < rows {
                            nr += 1;
                        }
                    }
                }
                (nc, nr)
            }
        };

        let (x, y, w, h) =
            geom::grid_cell_rect(vf_x, vf_y, vf_w, vf_h, cols, rows, next_col, next_row);
        ax_set_point(win, attr_pos, CGPoint { x, y })?;
        ax_set_size(
            win,
            attr_size,
            CGSize {
                width: w,
                height: h,
            },
        )?;
        Ok(())
    })();
    unsafe { CFRelease(win as CFTypeRef) };
    result
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
    let mut fallback_first_window: *mut c_void = ptr::null_mut();
    for i in 0..unsafe { CFArrayGetCount(arr.as_concrete_TypeRef()) } {
        let wref = unsafe { CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) } as *mut c_void;
        if wref.is_null() {
            continue;
        }
        // Remember the first top-level AXWindow as a fallback when AXWindowNumber is unavailable
        if fallback_first_window.is_null() {
            let role = ax_get_string(wref, cfstr("AXRole")).unwrap_or_default();
            if role == "AXWindow" {
                fallback_first_window = wref;
            }
        }
        let mut num_ref: CFTypeRef = ptr::null_mut();
        let nerr =
            unsafe { AXUIElementCopyAttributeValue(wref, cfstr("AXWindowNumber"), &mut num_ref) };
        if nerr == 0 && !num_ref.is_null() {
            let cfnum =
                unsafe { core_foundation::number::CFNumber::wrap_under_create_rule(num_ref as _) };
            let wid = cfnum.to_i64().unwrap_or(0) as u32;
            if wid == id {
                found = wref;
                break;
            }
        }
    }
    if found.is_null() {
        // Fallback: return the first AXWindow if available (useful for single-window apps)
        if !fallback_first_window.is_null() {
            unsafe { CFRetain(fallback_first_window as CFTypeRef) };
            unsafe { CFRelease(app as CFTypeRef) };
            return Ok((fallback_first_window, pid));
        }
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
    use crate::geom::grid_cell_rect as cell_rect;

    #[test]
    fn cell_rect_corners_and_remainders() {
        // Visible frame 100x100, 3x2 grid -> tile 33x50 with remainders w:1, h:0
        let (vf_x, vf_y, vf_w, vf_h) = (0.0, 0.0, 100.0, 100.0);
        // top-left is (col 0, row 0) in top-left origin mapping
        let (x0, y0, w0, h0) = cell_rect(vf_x, vf_y, vf_w, vf_h, 3, 2, 0, 0);
        assert_eq!((x0, y0, w0, h0), (0.0, 0.0, 33.0, 50.0));

        // top-right should absorb remainder width
        let (x1, y1, w1, h1) = cell_rect(vf_x, vf_y, vf_w, vf_h, 3, 2, 2, 0);
        assert_eq!((x1, y1, w1, h1), (66.0, 0.0, 34.0, 50.0));

        // bottom row (row 1) starts at y=50
        let (_x2, y2, _w2, _h2) = cell_rect(vf_x, vf_y, vf_w, vf_h, 3, 2, 0, 1);
        assert_eq!(y2, 50.0);
    }
}
