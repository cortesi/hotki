//! mac-winops: macOS window operations for Hotki.
//!
//! Provides APIs to toggle/set native full screen (AppKit-managed Space)
//! and non‑native full screen (maximize to visible screen frame) on the
//! currently focused window of a given PID.
//!
//! All operations require Accessibility permission.

use std::{ffi::c_void, ptr};

use core_foundation::{
    array::{CFArray, CFArrayGetCount, CFArrayGetValueAtIndex},
    base::{CFRelease, CFTypeRef, TCFType},
    string::{CFString, CFStringRef},
};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSScreen};
use objc2_foundation::MainThreadMarker;
use tracing::{debug, warn};

mod ax;
mod cfutil;
mod error;
pub mod focus;
mod focus_dir;
mod frame_storage;
mod fullscreen;
mod geom;
mod hide;
mod main_thread_ops;
pub mod nswindow;
mod place;
mod raise;
pub mod screen;
mod screen_util;
mod window;

pub use error::{Error, Result};
pub use main_thread_ops::{
    MoveDir, request_activate_pid, request_focus_dir, request_fullscreen_native,
    request_fullscreen_nonnative, request_place_grid, request_place_grid_focused,
    request_place_move_grid, request_raise_window,
};
pub use raise::raise_window;
pub use window::{Pos, WindowInfo, frontmost_window, frontmost_window_for_pid, list_windows};

use ax::*;
pub use ax::{ax_window_frame, ax_window_position, ax_window_size};
pub use fullscreen::{fullscreen_native, fullscreen_nonnative};
use geom::{CGPoint, CGSize, approx_eq_eps};
pub use hide::{hide_bottom_left, hide_corner};
use main_thread_ops::{MAIN_OPS, MainOp};
pub use place::place_grid_focused;

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
pub(crate) struct AXElem(*mut c_void);
impl AXElem {
    #[inline]
    pub(crate) fn new(ptr: *mut c_void) -> Self {
        Self(ptr)
    }
    #[inline]
    pub(crate) fn as_ptr(&self) -> *mut c_void {
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

pub(crate) fn focused_window_for_pid(pid: i32) -> Result<*mut c_void> {
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
// moved to crate::fullscreen::fullscreen_native

/// Toggle or set non‑native full screen (maximize to visible frame in current Space)
/// for the focused window of `pid`.
///
/// Requires Accessibility permission and must run on the AppKit main thread
/// (uses `NSScreen::visibleFrame`).
// moved to crate::fullscreen::fullscreen_nonnative

// moved to screen_util::visible_frame_containing_point

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
                let _ = crate::place::place_grid(id, cols, rows, col, row);
            }
            MainOp::PlaceMoveGrid {
                id,
                cols,
                rows,
                dir,
            } => {
                let _ = crate::place::place_move_grid(id, cols, rows, dir);
            }
            MainOp::PlaceGridFocused {
                pid,
                cols,
                rows,
                col,
                row,
            } => {
                let _ = crate::place::place_grid_focused(pid, cols, rows, col, row);
            }
            MainOp::ActivatePid { pid } => {
                let _ = activate_pid(pid);
            }
            MainOp::RaiseWindow { pid, id } => {
                let _ = crate::raise::raise_window(pid, id);
            }
            MainOp::FocusDir { dir } => {
                let _ = crate::focus_dir::focus_dir(dir);
            }
        }
    }
}

// moved to crate::place::place_grid

/// Focus the next window in the given direction on the current screen within the
/// current Space. Uses CG for enumeration + AppKit for screen geometry and AX for
/// the origin window frame and final raise.
// moved to crate::focus_dir::focus_dir
// moved to focus_dir::focus_dir
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
// moved to crate::place::place_grid_focused

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
// moved to crate::hide::hide_corner

// moved to crate::hide::hide_bottom_left

// grid helpers now provided by crate::geom

// moved to crate::place::place_move_grid

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
