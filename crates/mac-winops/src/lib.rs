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
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use objc2_foundation::MainThreadMarker;
use tracing::{debug, warn};

mod ax;
mod cfutil;
mod error;
mod focus_dir;
mod frame_storage;
mod fullscreen;
mod geom;
mod hide;
mod main_thread_ops;
pub mod ops;
mod place;
mod raise;
mod screen_util;
mod window;

pub mod focus;
pub mod nswindow;
pub mod screen;
use ax::*;
pub use ax::{ax_window_frame, ax_window_position, ax_window_size};
pub use error::{Error, Result};
pub use fullscreen::{fullscreen_native, fullscreen_nonnative};
pub use hide::{hide_bottom_left, hide_corner};
use main_thread_ops::{MAIN_OPS, MainOp};
pub use main_thread_ops::{
    MoveDir, request_activate_pid, request_focus_dir, request_fullscreen_native,
    request_fullscreen_nonnative, request_place_grid, request_place_grid_focused,
    request_place_move_grid, request_raise_window,
};
pub use place::place_grid_focused;
pub use raise::raise_window;
pub use window::{Pos, WindowInfo, frontmost_window, frontmost_window_for_pid, list_windows};

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

/// RAII guard that releases an AX element on drop.
pub(crate) struct AXElem(*mut c_void);
impl AXElem {
    /// Wrap an AX pointer we own under the Create rule. Returns None if null.
    #[inline]
    pub(crate) fn from_create(ptr: *mut c_void) -> Option<Self> {
        if ptr.is_null() { None } else { Some(Self(ptr)) }
    }
    /// Retain a borrowed AX pointer and wrap it. Returns None if null.
    #[inline]
    pub(crate) fn retain_from_borrowed(ptr: *mut c_void) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            unsafe { CFRetain(ptr as CFTypeRef) };
            Some(Self(ptr))
        }
    }
    /// Expose the raw pointer for AX calls.
    #[inline]
    pub(crate) fn as_ptr(&self) -> *mut c_void {
        self.0
    }
}
impl Clone for AXElem {
    fn clone(&self) -> Self {
        unsafe { CFRetain(self.0 as CFTypeRef) };
        Self(self.0)
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
        let Some(app) = AXElem::from_create(AXUIElementCreateApplication(pid)) else {
            return false;
        };
        let mut wins_ref: CFTypeRef = ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(app.as_ptr(), cfstr("AXWindows"), &mut wins_ref);
        if err != 0 || wins_ref.is_null() {
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
                return true;
            }
        }
    }
    false
}

/// Best-effort: return the focused window's CG `WindowId` for a given `pid` using AX semantics.
///
/// Tries `AXFocused` first, then `AXMain`, and falls back to CG's frontmost window for the pid.
/// Returns `None` if nothing is found or AX is unavailable.
pub fn ax_focused_window_id_for_pid(pid: i32) -> Option<WindowId> {
    if !permissions::accessibility_ok() {
        return frontmost_window_for_pid(pid).map(|w| w.id);
    }
    unsafe {
        let Some(app) = AXElem::from_create(AXUIElementCreateApplication(pid)) else {
            return frontmost_window_for_pid(pid).map(|w| w.id);
        };
        let mut wins_ref: CFTypeRef = std::ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(app.as_ptr(), cfstr("AXWindows"), &mut wins_ref);
        if err != 0 || wins_ref.is_null() {
            return frontmost_window_for_pid(pid).map(|w| w.id);
        }
        let arr = CFArray::<*const c_void>::wrap_under_create_rule(wins_ref as _);
        let n = CFArrayGetCount(arr.as_concrete_TypeRef());
        // Prefer AXFocused; then AXMain
        let mut chosen: *mut c_void = std::ptr::null_mut();
        for i in 0..n {
            let w = CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) as *mut c_void;
            if w.is_null() {
                continue;
            }
            if let Ok(Some(true)) = ax_bool(w, cfstr("AXFocused")) {
                chosen = w;
                break;
            }
        }
        if chosen.is_null() {
            for i in 0..n {
                let w = CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) as *mut c_void;
                if w.is_null() {
                    continue;
                }
                if let Ok(Some(true)) = ax_bool(w, cfstr("AXMain")) {
                    chosen = w;
                    break;
                }
            }
        }
        if chosen.is_null() {
            return frontmost_window_for_pid(pid).map(|w| w.id);
        }
        let mut num_ref: CFTypeRef = std::ptr::null_mut();
        let nerr = AXUIElementCopyAttributeValue(chosen, cfstr("AXWindowNumber"), &mut num_ref);
        if nerr != 0 || num_ref.is_null() {
            return frontmost_window_for_pid(pid).map(|w| w.id);
        }
        let cfnum = core_foundation::number::CFNumber::wrap_under_create_rule(num_ref as _);
        let wid = cfnum.to_i64().unwrap_or(0) as u32;
        if wid == 0 {
            frontmost_window_for_pid(pid).map(|w| w.id)
        } else {
            Some(wid)
        }
    }
}

/// Best-effort: return the AX title for a given CG `WindowId`.
/// Returns `None` if AX is unavailable or the window cannot be resolved.
pub fn ax_title_for_window_id(id: WindowId) -> Option<String> {
    if !permissions::accessibility_ok() {
        return None;
    }
    match ax_window_for_id(id) {
        Ok((w, _pid)) => ax_get_string(w.as_ptr(), cfstr("AXTitle")),
        Err(_) => None,
    }
}

pub(crate) fn focused_window_for_pid(pid: i32) -> Result<AXElem> {
    debug!("focused_window_for_pid: pid={}", pid);
    let Some(app) = (unsafe { AXElem::from_create(AXUIElementCreateApplication(pid)) }) else {
        warn!("focused_window_for_pid: AXUIElementCreateApplication returned null");
        return Err(Error::AppElement);
    };

    // Prefer scanning AXWindows for AXFocused/AXMain to avoid AXFocusedWindow crash on macOS 15.5.
    let mut wins_ref: CFTypeRef = ptr::null_mut();
    let err_w =
        unsafe { AXUIElementCopyAttributeValue(app.as_ptr(), cfstr("AXWindows"), &mut wins_ref) };
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
                debug!("focused_window_for_pid: found window via AXFocused");
                return AXElem::retain_from_borrowed(w).ok_or(Error::FocusedWindow);
            }
            if let Ok(Some(true)) = ax_bool(w, cfstr("AXMain")) {
                debug!("focused_window_for_pid: found window via AXMain");
                return AXElem::retain_from_borrowed(w).ok_or(Error::FocusedWindow);
            }
        }
    }

    // Fallback: try mapping CG frontmost window for pid via AXWindowNumber.
    if let Some(info) = frontmost_window_for_pid(pid) {
        // Reuse existing helper to resolve AX element by CGWindowID
        if let Ok((w, _pid)) = ax_window_for_id(info.id) {
            debug!("focused_window_for_pid: fallback via AXWindowNumber");
            return Ok(w);
        }
    }
    // Final fallback: choose the first top-level AXWindow from AXWindows list.
    unsafe {
        let mut wins_ref: CFTypeRef = ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(app.as_ptr(), cfstr("AXWindows"), &mut wins_ref);
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
                    debug!("focused_window_for_pid: fallback to first AXWindow entry");
                    return AXElem::retain_from_borrowed(w).ok_or(Error::FocusedWindow);
                }
            }
        }
    }
    debug!("focused_window_for_pid: no focused window");
    Err(Error::FocusedWindow)
}

// fullscreen and screen helpers are defined in their modules

/// Drain and execute any pending main-thread operations. Call from the Tao main thread
/// (e.g., in `Event::UserEvent`), after posting a user event via `focus::post_user_event()`.
pub fn drain_main_ops() {
    loop {
        let op_opt = MAIN_OPS.lock().pop_front();
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

//

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
