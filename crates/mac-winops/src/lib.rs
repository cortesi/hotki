//! mac-winops: macOS window operations for Hotki.
//!
//! Provides APIs to toggle/set native full screen (AppKit-managed Space)
//! and non‑native full screen (maximize to visible screen frame) on the
//! currently focused window of a given PID.
//!
//! All operations require Accessibility permission.
//!
//! Window placement now flows through a shared `PlacementEngine` that accepts a
//! `PlacementContext` and yields verified outcomes with attempt timelines and
//! clamp diagnostics. Callers can fine-tune epsilon tolerances, retry budgets,
//! and safe-park hooks via `PlaceAttemptOptions`, supplied either directly to
//! placement functions (e.g., `place_grid_focused_opts`) or through the new
//! main-thread request APIs (`request_place_grid_opts`,
//! `request_place_grid_focused_opts`, `request_place_move_grid_opts`).
//!
//! All Accessibility setters are routed through the `AxAdapter` trait so that
//! production code uses the system adapter while deterministic tests install
//! an in-memory fake. The adapter exposes explicit entry points for
//! shrink→move→grow fallbacks, axis nudges, and safe parking, enabling the
//! smoketest harness and property tests to exercise the placement pipeline
//! without real windows.

use std::{
    collections::{HashMap, HashSet},
    ffi::c_void,
    ptr, thread,
    time::{Duration, Instant},
};

use core_foundation::{
    array::{CFArray, CFArrayGetCount, CFArrayGetValueAtIndex},
    base::{CFRelease, CFTypeRef, TCFType},
    string::{CFString, CFStringRef},
};
use core_graphics::{
    event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton},
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::CGPoint,
};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use objc2_foundation::MainThreadMarker;
use tracing::{debug, warn};

mod ax;
mod ax_observer;
mod ax_private;
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
pub mod wait;
mod window;

pub mod focus;
pub mod nswindow;
pub mod screen;
use std::sync::{Arc, RwLock};

use ax::*;
pub use ax::{
    AxProps, ax_get_bool_by_title, ax_is_window_minimized, ax_is_window_zoomed,
    ax_props_for_window_id, ax_set_bool_by_title, ax_set_window_minimized, ax_set_window_zoomed,
    ax_window_frame, ax_window_position, ax_window_size, cfstr,
};
pub use error::{Error, Result};
pub use fullscreen::{fullscreen_native, fullscreen_nonnative};
pub use geom::{Rect, approx_eq, approx_eq_eps, cell_rect};
pub use hide::{hide_bottom_left, hide_corner};
use main_thread_ops::{MAIN_OPS, MainOp};
pub use main_thread_ops::{
    MoveDir, request_activate_pid, request_focus_dir, request_fullscreen_native,
    request_fullscreen_nonnative, request_place_grid, request_place_grid_focused,
    request_place_grid_focused_opts, request_place_grid_opts, request_place_move_grid,
    request_place_move_grid_opts, request_raise_window,
};
use once_cell::sync::Lazy;
pub use place::{
    AttemptKind, AttemptOrder, AttemptRecord, AttemptTimeline, AxAdapterHandle, FakeApplyResponse,
    FakeAxAdapter, FakeOp, FakeWindowConfig, FallbackInvocation, FallbackTrigger,
    PlaceAttemptOptions, PlacementContext, PlacementCountersSnapshot, PlacementEngine,
    PlacementEngineConfig, PlacementGrid, PlacementOutcome, RetryLimits, place_grid_focused,
    place_grid_focused_opts, place_move_grid, placement_counters_reset,
    placement_counters_snapshot,
};
pub use raise::raise_window;
pub use window::{Pos, SpaceId, WindowInfo, active_space_ids};

// ===== Observer wiring =====
thread_local! {
    static OBSERVERS: std::cell::RefCell<Option<ax_observer::AxObserverRegistry>> =
        const { std::cell::RefCell::new(None) };
}

// Re-export AX observer event types so other crates can subscribe without
// depending on the private module path.
pub use ax_observer::{AxEvent, AxEventKind, WindowHint};

/// Ensure an AX observer exists for `pid` (idempotent).
pub fn ensure_ax_observer(pid: i32) {
    OBSERVERS.with(|cell| {
        let mut o = cell.borrow_mut();
        if o.is_none() {
            *o = Some(ax_observer::AxObserverRegistry::default());
        }
        if let Some(reg) = o.as_ref() {
            let _ = reg.ensure(pid);
        }
    });
}

/// Remove an AX observer for `pid` if present. Returns `true` if removed.
pub fn remove_ax_observer(pid: i32) -> bool {
    OBSERVERS.with(|cell| {
        let o = cell.borrow();
        if let Some(reg) = o.as_ref() {
            reg.remove(pid)
        } else {
            false
        }
    })
}

/// Set a global sender to receive `AxEvent`s from all installed observers.
///
/// If the observer registry has not been created yet, this function will
/// lazily initialize it.
pub fn set_ax_observer_sender(sender: crossbeam_channel::Sender<AxEvent>) {
    OBSERVERS.with(|cell| {
        let mut o = cell.borrow_mut();
        if o.is_none() {
            *o = Some(ax_observer::AxObserverRegistry::default());
        }
        if let Some(reg) = o.as_ref() {
            reg.set_sender(sender);
        }
    });
}

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
pub struct AXElem(*mut c_void);
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
    // Create AX application element for pid
    let Some(app) = (unsafe { AXElem::from_create(AXUIElementCreateApplication(pid)) }) else {
        return false;
    };
    // Fetch AXWindows array for the app
    let mut wins_ref: CFTypeRef = ptr::null_mut();
    // SAFETY: `app` is a valid AXUIElement and we pass an out‑param for the copy.
    let err =
        unsafe { AXUIElementCopyAttributeValue(app.as_ptr(), cfstr("AXWindows"), &mut wins_ref) };
    if err != 0 || wins_ref.is_null() {
        return false;
    }
    // SAFETY: `wins_ref` follows the Create rule from AX; wrap to transfer ownership.
    let arr = unsafe { CFArray::<*const c_void>::wrap_under_create_rule(wins_ref as _) };
    // SAFETY: CFArray access functions require the concrete CFArray ref; bounds checked by loop.
    let n = unsafe { CFArrayGetCount(arr.as_concrete_TypeRef()) };
    for i in 0..n {
        // SAFETY: Index < n; returns borrowed item pointer.
        let wref = unsafe { CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) } as *mut c_void;
        if wref.is_null() {
            continue;
        }
        let mut title_ref: CFTypeRef = ptr::null_mut();
        // SAFETY: `wref` is an AXUIElement; return copied CFString for title.
        let terr = unsafe { AXUIElementCopyAttributeValue(wref, cfstr("AXTitle"), &mut title_ref) };
        if terr != 0 || title_ref.is_null() {
            continue;
        }
        // SAFETY: Title CFString was returned under Create rule.
        let cfs = unsafe { CFString::wrap_under_create_rule(title_ref as CFStringRef) };
        let title = cfs.to_string();
        if title == expected_title {
            return true;
        }
    }
    false
}

fn frontmost_title_matches(pid: i32, expected_title: &str) -> bool {
    if let Some(win) = window::frontmost_window() {
        return win.pid == pid && win.title == expected_title;
    }
    false
}

fn post_mouse_event(point: CGPoint, event_type: CGEventType) -> bool {
    let source = match CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
        Ok(src) => src,
        Err(_) => return false,
    };
    let Ok(event) = CGEvent::new_mouse_event(source, event_type, point, CGMouseButton::Left) else {
        return false;
    };
    event.post(CGEventTapLocation::HID);
    true
}

fn nudge_frontmost_with_click(pid: i32, title: &str) -> bool {
    if !permissions::accessibility_ok() {
        return false;
    }
    let Some(((x, y), (w, h))) = ax_window_frame(pid, title) else {
        return false;
    };
    if w <= 0.0 || h <= 0.0 {
        return false;
    }
    let center = CGPoint::new(x + (w / 2.0), y + (h / 2.0));
    // Move momentarily to the window center, click, then restore.
    let moved = post_mouse_event(center, CGEventType::MouseMoved);
    let down = post_mouse_event(center, CGEventType::LeftMouseDown);
    let up = post_mouse_event(center, CGEventType::LeftMouseUp);
    if !(moved && down && up) {
        return false;
    }
    true
}

pub fn ensure_frontmost_by_title(pid: i32, title: &str, attempts: usize, delay_ms: u64) -> bool {
    if attempts == 0 {
        return false;
    }
    let step_ms = delay_ms.clamp(10, 40);
    let hold_target_ms = delay_ms.max(400);
    let mut cg_hold_ms: u64 = 0;
    let mut focus_hold_ms: u64 = 0;
    for attempt in 0..attempts {
        let mut cached_id = wait::find_window_id_ms(pid, title, delay_ms, 20);
        let mut last_nudge: Option<Instant> = None;
        if let Some(id) = cached_id {
            debug!(
                "ensure_frontmost_by_title: attempt={} pid={} id={} title='{}' (raise)",
                attempt + 1,
                pid,
                id,
                title
            );
            let _ = request_raise_window(pid, id);
        } else {
            debug!(
                "ensure_frontmost_by_title: attempt={} pid={} title='{}' (activate)",
                attempt + 1,
                pid,
                title
            );
            let _ = request_activate_pid(pid);
        }
        let mut waited_ms: u64 = 0;
        while waited_ms < hold_target_ms {
            thread::sleep(Duration::from_millis(step_ms));
            waited_ms += step_ms;
            let front = window::frontmost_window()
                .map(|w| format!("pid={} title='{}'", w.pid, w.title))
                .unwrap_or_else(|| "<none>".to_string());
            let cg_match = frontmost_title_matches(pid, title);
            let ax_match = ax_has_window_title(pid, title);
            let focus_snap = crate::focus::poll_now();
            let focus_pid_match = focus_snap.pid == pid && pid >= 0;
            let focus_title_match = focus_pid_match && focus_snap.title == title;
            let focus_window_match = focus_title_match || (focus_pid_match && ax_match);
            debug!(
                "ensure_frontmost_by_title: attempt={} poll frontmost={} cg_match={} ax_match={} focus_match={} cg_hold_ms={} focus_hold_ms={}",
                attempt + 1,
                front,
                cg_match,
                ax_match,
                focus_window_match,
                cg_hold_ms,
                focus_hold_ms
            );
            if cg_match {
                if !ax_match {
                    debug!(
                        "ensure_frontmost_by_title: cg matched but ax mismatch pid={} title='{}'",
                        pid, title
                    );
                }
                cg_hold_ms = (cg_hold_ms + step_ms).min(hold_target_ms);
                if focus_window_match {
                    focus_hold_ms = (focus_hold_ms + step_ms).min(hold_target_ms);
                } else if focus_hold_ms > 0 {
                    debug!(
                        "ensure_frontmost_by_title: focus hold reset after {} ms (loss observed)",
                        focus_hold_ms
                    );
                    focus_hold_ms = 0;
                }
                if cg_hold_ms >= hold_target_ms {
                    debug!(
                        "ensure_frontmost_by_title: stabilized after {} attempts (hold {} ms)",
                        attempt + 1,
                        cg_hold_ms
                    );
                    if nudge_frontmost_with_click(pid, title) {
                        debug!(
                            "ensure_frontmost_by_title: post-stabilize synthetic click pid={} title='{}'",
                            pid, title
                        );
                    } else {
                        debug!(
                            "ensure_frontmost_by_title: post-stabilize synthetic click skipped pid={} title='{}'",
                            pid, title
                        );
                    }
                    return true;
                }
                if focus_hold_ms >= hold_target_ms {
                    debug!(
                        "ensure_frontmost_by_title: stabilized via focus snapshot after {} attempts (hold {} ms)",
                        attempt + 1,
                        focus_hold_ms
                    );
                    return true;
                }
            } else {
                if focus_window_match {
                    focus_hold_ms = (focus_hold_ms + step_ms).min(hold_target_ms);
                    if focus_hold_ms >= hold_target_ms {
                        debug!(
                            "ensure_frontmost_by_title: stabilized via focus snapshot after {} attempts (hold {} ms)",
                            attempt + 1,
                            focus_hold_ms
                        );
                        return true;
                    }
                    cg_hold_ms = 0;
                    continue;
                }
                if cg_hold_ms > 0 {
                    debug!(
                        "ensure_frontmost_by_title: hold reset after {} ms (loss observed)",
                        cg_hold_ms
                    );
                    cg_hold_ms = 0;
                }
                if focus_hold_ms > 0 {
                    debug!(
                        "ensure_frontmost_by_title: focus hold reset after {} ms (loss observed)",
                        focus_hold_ms
                    );
                    focus_hold_ms = 0;
                }
                if ax_match {
                    let should_nudge = last_nudge
                        .map(|ts| ts.elapsed() >= Duration::from_millis(120))
                        .unwrap_or(true);
                    if should_nudge && nudge_frontmost_with_click(pid, title) {
                        last_nudge = Some(Instant::now());
                        debug!(
                            "ensure_frontmost_by_title: issued synthetic click nudge pid={} title='{}'",
                            pid, title
                        );
                        continue;
                    } else if should_nudge {
                        debug!(
                            "ensure_frontmost_by_title: synthetic click nudge failed pid={} title='{}'",
                            pid, title
                        );
                    }
                }
                if let Some(id) = cached_id {
                    debug!(
                        "ensure_frontmost_by_title: re-raising pid={} id={} title='{}' after loss",
                        pid, id, title
                    );
                    let _ = request_raise_window(pid, id);
                } else {
                    cached_id = wait::find_window_id_ms(pid, title, delay_ms, 20);
                    if let Some(id) = cached_id {
                        debug!(
                            "ensure_frontmost_by_title: resolved id={} on retry; raising",
                            id
                        );
                        let _ = request_raise_window(pid, id);
                    } else {
                        debug!(
                            "ensure_frontmost_by_title: id unresolved on retry; activating pid={}",
                            pid
                        );
                        let _ = request_activate_pid(pid);
                    }
                }
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
    // Opportunistically install an observer for the queried PID (no-op if disabled).
    ensure_ax_observer(pid);
    if !permissions::accessibility_ok() {
        return window::frontmost_window_for_pid(pid).map(|w| w.id);
    }
    // Create AX application element for pid
    let Some(app) = (unsafe { AXElem::from_create(AXUIElementCreateApplication(pid)) }) else {
        return window::frontmost_window_for_pid(pid).map(|w| w.id);
    };
    // Fetch AXWindows array for the app
    let mut wins_ref: CFTypeRef = std::ptr::null_mut();
    // SAFETY: `app` is valid and we pass an out‑param to receive a copied array.
    let err =
        unsafe { AXUIElementCopyAttributeValue(app.as_ptr(), cfstr("AXWindows"), &mut wins_ref) };
    if err != 0 || wins_ref.is_null() {
        return window::frontmost_window_for_pid(pid).map(|w| w.id);
    }
    // SAFETY: Wrap ownership of returned CFArray.
    let arr = unsafe { CFArray::<*const c_void>::wrap_under_create_rule(wins_ref as _) };
    // SAFETY: CFArray access requires concrete ref; bounds enforced by loop.
    let n = unsafe { CFArrayGetCount(arr.as_concrete_TypeRef()) };
    // Prefer AXFocused; then AXMain
    let mut chosen: *mut c_void = std::ptr::null_mut();
    for i in 0..n {
        // SAFETY: Index < n.
        let w = unsafe { CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) } as *mut c_void;
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
            // SAFETY: Index < n.
            let w = unsafe { CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) } as *mut c_void;
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
        return window::frontmost_window_for_pid(pid).map(|w| w.id);
    }
    // Prefer private API if available
    let wid = if let Some(idp) = ax_private::window_id_for_ax_element(chosen) {
        idp
    } else {
        let mut num_ref: CFTypeRef = std::ptr::null_mut();
        let nerr =
            unsafe { AXUIElementCopyAttributeValue(chosen, cfstr("AXWindowNumber"), &mut num_ref) };
        if nerr != 0 || num_ref.is_null() {
            return window::frontmost_window_for_pid(pid).map(|w| w.id);
        }
        let cfnum =
            unsafe { core_foundation::number::CFNumber::wrap_under_create_rule(num_ref as _) };
        cfnum.to_i64().unwrap_or(0) as u32
    };
    if wid == 0 {
        window::frontmost_window_for_pid(pid).map(|w| w.id)
    } else {
        Some(wid)
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
            match ax_bool(w, cfstr("AXFocused")) {
                Ok(Some(true)) => {
                    debug!("focused_window_for_pid: found window via AXFocused");
                    return AXElem::retain_from_borrowed(w).ok_or(Error::FocusedWindow);
                }
                Err(e) => {
                    debug!("focused_window_for_pid: AXFocused check error: {}", e);
                }
                _ => {}
            }
            match ax_bool(w, cfstr("AXMain")) {
                Ok(Some(true)) => {
                    debug!("focused_window_for_pid: found window via AXMain");
                    return AXElem::retain_from_borrowed(w).ok_or(Error::FocusedWindow);
                }
                Err(e) => {
                    debug!("focused_window_for_pid: AXMain check error: {}", e);
                }
                _ => {}
            }
        }
    }

    // Fallback: try mapping CG frontmost window for pid via AXWindowNumber.
    if let Some(info) = window::frontmost_window_for_pid(pid) {
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
    // A short deadline (25–40 ms) to collect and coalesce rapid bursts of
    // placement intents while still keeping UI latency low.
    const BUDGET_MS: u64 = 30;

    // Helper to actually apply a single op.
    fn apply_one(op: MainOp) {
        match op {
            MainOp::FullscreenNative { pid, desired } => {
                tracing::info!(
                    "MainOps: drain FullscreenNative pid={} desired={:?}",
                    pid,
                    desired
                );
                if let Err(e) = fullscreen_native(pid, desired) {
                    tracing::warn!("FullscreenNative failed: pid={} err={}", pid, e);
                }
            }
            MainOp::FullscreenNonNative { pid, desired } => {
                tracing::info!(
                    "MainOps: drain FullscreenNonNative pid={} desired={:?}",
                    pid,
                    desired
                );
                if let Err(e) = fullscreen_nonnative(pid, desired) {
                    tracing::warn!("FullscreenNonNative failed: pid={} err={}", pid, e);
                }
            }
            MainOp::PlaceGrid {
                id,
                cols,
                rows,
                col,
                row,
                opts,
            } => {
                if let Err(e) = placement().place_grid(id, cols, rows, col, row, opts) {
                    tracing::warn!(
                        "PlaceGrid failed: id={} cols={} rows={} col={} row={} err={}",
                        id,
                        cols,
                        rows,
                        col,
                        row,
                        e
                    );
                }
            }
            MainOp::PlaceMoveGrid {
                id,
                cols,
                rows,
                dir,
                opts,
            } => {
                if let Err(e) = placement().place_move_grid(id, cols, rows, dir, opts) {
                    tracing::warn!(
                        "PlaceMoveGrid failed: id={} cols={} rows={} dir={:?} err={}",
                        id,
                        cols,
                        rows,
                        dir,
                        e
                    );
                }
            }
            MainOp::PlaceGridFocused {
                pid,
                cols,
                rows,
                col,
                row,
                opts,
            } => {
                if let Err(e) = placement().place_grid_focused(pid, cols, rows, col, row, opts) {
                    tracing::warn!(
                        "PlaceGridFocused failed: pid={} cols={} rows={} col={} row={} err={}",
                        pid,
                        cols,
                        rows,
                        col,
                        row,
                        e
                    );
                }
            }
            MainOp::ActivatePid { pid } => {
                if let Err(e) = activate_pid(pid) {
                    tracing::warn!("ActivatePid failed: pid={} err={}", pid, e);
                }
            }
            MainOp::RaiseWindow { pid, id } => {
                if let Err(e) = crate::raise::raise_window(pid, id) {
                    tracing::warn!("RaiseWindow failed: pid={} id={} err={}", pid, id, e);
                }
            }
            MainOp::FocusDir { dir } => {
                if let Err(e) = crate::focus_dir::focus_dir(dir) {
                    tracing::warn!("FocusDir failed: dir={:?} err={}", dir, e);
                }
            }
            // Future-proofing: if a new variant is added to `MainOp` but not yet
            // handled here, avoid crashing in release builds. Log and drop.
            #[allow(unreachable_patterns)]
            _ => {
                tracing::warn!("MainOps: unhandled operation encountered; dropping");
            }
        }
    }

    // Local key to preserve last-writer order across id/pid groups.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
    enum Key {
        Id(WindowId),
        Pid(i32),
    }

    // Resolve pid for a given WindowId using best-effort strategies.
    fn resolve_pid_for_id(id: WindowId) -> Option<i32> {
        if let Some(pid) = overrides::lookup(id) {
            return Some(pid);
        }
        if let Some(w) = crate::window::list_windows()
            .into_iter()
            .find(|w| w.id == id)
        {
            return Some(w.pid);
        }
        if let Ok((_elem, pid)) = crate::ax::ax_window_for_id(id) {
            return Some(pid);
        }
        None
    }

    loop {
        let start = std::time::Instant::now();
        let mut any = false;

        // Non-placement ops are executed in FIFO order within the batch.
        let mut non_place: Vec<MainOp> = Vec::new();
        // Latest placement intents during the budget window.
        let mut latest_by_id: HashMap<WindowId, MainOp> = HashMap::new();
        let mut latest_by_pid: HashMap<i32, MainOp> = HashMap::new();
        let mut order: Vec<Key> = Vec::new();

        let budget = Duration::from_millis(BUDGET_MS);

        // Accumulate within the deadline or until the queue runs dry.
        loop {
            let op_opt = MAIN_OPS.lock().pop_front();
            let Some(op) = op_opt else { break };
            any = true;

            match op {
                MainOp::PlaceGrid {
                    id,
                    cols,
                    rows,
                    col,
                    row,
                    opts,
                } => {
                    latest_by_id.insert(
                        id,
                        MainOp::PlaceGrid {
                            id,
                            cols,
                            rows,
                            col,
                            row,
                            opts,
                        },
                    );
                    order.retain(|k| *k != Key::Id(id));
                    order.push(Key::Id(id));
                }
                MainOp::PlaceMoveGrid {
                    id,
                    cols,
                    rows,
                    dir,
                    opts,
                } => {
                    latest_by_id.insert(
                        id,
                        MainOp::PlaceMoveGrid {
                            id,
                            cols,
                            rows,
                            dir,
                            opts,
                        },
                    );
                    order.retain(|k| *k != Key::Id(id));
                    order.push(Key::Id(id));
                }
                MainOp::PlaceGridFocused {
                    pid,
                    cols,
                    rows,
                    col,
                    row,
                    opts,
                } => {
                    latest_by_pid.insert(
                        pid,
                        MainOp::PlaceGridFocused {
                            pid,
                            cols,
                            rows,
                            col,
                            row,
                            opts,
                        },
                    );
                    order.retain(|k| *k != Key::Pid(pid));
                    order.push(Key::Pid(pid));
                }
                other => non_place.push(other),
            }

            if start.elapsed() >= budget {
                break;
            }
        }

        if !any {
            break;
        }

        // Execute non-placement ops in-order first to preserve behavioral expectations.
        for op in non_place.into_iter() {
            apply_one(op);
        }

        // Cross-type stale-drop: if an id-specific placement maps to the same pid as a
        // focused placement within this batch, drop the focused placement.
        let mut pid_with_id: HashSet<i32> = HashSet::new();
        for (idk, _op) in latest_by_id.iter() {
            if let Some(pid) = resolve_pid_for_id(*idk) {
                pid_with_id.insert(pid);
            }
        }
        if !pid_with_id.is_empty() {
            latest_by_pid.retain(|pid, _| !pid_with_id.contains(pid));
            order.retain(|k| match k {
                Key::Pid(p) => !pid_with_id.contains(p),
                _ => true,
            });
        }

        // Then execute the coalesced placement intents using last-writer ordering.
        for k in order.into_iter() {
            match k {
                Key::Id(id) => {
                    if let Some(op) = latest_by_id.remove(&id) {
                        apply_one(op);
                    }
                }
                Key::Pid(pid) => {
                    if let Some(op) = latest_by_pid.remove(&pid) {
                        apply_one(op);
                    }
                }
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

        // Best-effort: if any windows are minimized, unminimize them so subsequent
        // raise/place operations have a visible target. Ignore AX failures.
        if permissions::accessibility_ok() {
            unsafe {
                let app_ax =
                    crate::AXElem::from_create(crate::ax::AXUIElementCreateApplication(pid));
                if let Some(app_ax) = app_ax {
                    let mut wins_ref: core_foundation::base::CFTypeRef = std::ptr::null_mut();
                    let err = crate::ax::AXUIElementCopyAttributeValue(
                        app_ax.as_ptr(),
                        crate::ax::cfstr("AXWindows"),
                        &mut wins_ref,
                    );
                    if err == 0 && !wins_ref.is_null() {
                        let arr = core_foundation::array::CFArray::<*const core::ffi::c_void>::wrap_under_create_rule(wins_ref as _);
                        let n = core_foundation::array::CFArrayGetCount(arr.as_concrete_TypeRef());
                        for i in 0..n {
                            let w = core_foundation::array::CFArrayGetValueAtIndex(
                                arr.as_concrete_TypeRef(),
                                i,
                            ) as *mut core::ffi::c_void;
                            if w.is_null() {
                                continue;
                            }
                            let _ = crate::ax::AXUIElementSetAttributeValue(
                                w,
                                crate::ax::cfstr("AXMinimized"),
                                core_foundation::boolean::kCFBooleanFalse
                                    as core_foundation::base::CFTypeRef,
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    } else {
        Err(Error::ActivationFailed)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use once_cell::sync::Lazy;

    use super::*;
    use crate::geom::Rect;

    static PLACE_ID_CALLS: AtomicUsize = AtomicUsize::new(0);
    static PLACE_FOCUSED_CALLS: AtomicUsize = AtomicUsize::new(0);
    static TEST_ID_PID: Lazy<Mutex<HashMap<WindowId, i32>>> =
        Lazy::new(|| Mutex::new(HashMap::new()));

    struct CountingPlacement;
    impl super::PlacementExecutor for CountingPlacement {
        fn place_grid(
            &self,
            _id: WindowId,
            _cols: u32,
            _rows: u32,
            _col: u32,
            _row: u32,
            _opts: PlaceAttemptOptions,
        ) -> super::Result<()> {
            PLACE_ID_CALLS.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn place_move_grid(
            &self,
            _id: WindowId,
            _cols: u32,
            _rows: u32,
            _dir: super::main_thread_ops::MoveDir,
            _opts: PlaceAttemptOptions,
        ) -> super::Result<()> {
            PLACE_ID_CALLS.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn place_grid_focused(
            &self,
            _pid: i32,
            _cols: u32,
            _rows: u32,
            _col: u32,
            _row: u32,
            _opts: PlaceAttemptOptions,
        ) -> super::Result<()> {
            PLACE_FOCUSED_CALLS.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }
    fn take_split_counts() -> (usize, usize) {
        (
            PLACE_ID_CALLS.swap(0, Ordering::Relaxed),
            PLACE_FOCUSED_CALLS.swap(0, Ordering::Relaxed),
        )
    }

    fn set_test_id_pid(id: WindowId, pid: i32) {
        TEST_ID_PID.lock().unwrap().insert(id, pid);
        super::overrides::set_pid_for_id(id, pid);
    }
    fn clear_test_id_pid() {
        TEST_ID_PID.lock().unwrap().clear();
        super::overrides::clear_pid_overrides();
    }

    #[test]
    fn cell_rect_corners_and_remainders() {
        // Visible frame 100x100, 3x2 grid -> tile 33x50 with remainders w:1, h:0
        let vf = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        };
        // top-left is (col 0, row 0)
        let r0 = vf.grid_cell(3, 2, 0, 0);
        assert_eq!((r0.x, r0.y, r0.w, r0.h), (0.0, 0.0, 33.0, 50.0));

        // top-right should absorb remainder width
        let r1 = vf.grid_cell(3, 2, 2, 0);
        assert_eq!((r1.x, r1.y, r1.w, r1.h), (66.0, 0.0, 34.0, 50.0));

        // bottom row (row 1) starts at y=50
        let r2 = vf.grid_cell(3, 2, 0, 1);
        assert_eq!(r2.y, 50.0);
    }

    #[test]
    fn batch_planner_coalesces_latest_per_target() {
        // Build a synthetic burst: mixed non-placement ops and multiple placements
        // for the same id/pid. Verify we keep FIFO for non-placement and only
        // the latest placement per target.
        let id = 1122u32;
        let pid = 7788i32;
        let ops = vec![
            MainOp::ActivatePid { pid: 1 },
            MainOp::PlaceGrid {
                id,
                cols: 3,
                rows: 2,
                col: 0,
                row: 0,
                opts: PlaceAttemptOptions::default(),
            },
            MainOp::FocusDir {
                dir: crate::main_thread_ops::MoveDir::Left,
            },
            MainOp::PlaceGrid {
                id,
                cols: 3,
                rows: 2,
                col: 2,
                row: 1,
                opts: PlaceAttemptOptions::default(),
            }, // should win for id
            MainOp::PlaceGridFocused {
                pid,
                cols: 2,
                rows: 2,
                col: 0,
                row: 0,
                opts: PlaceAttemptOptions::default(),
            },
            MainOp::PlaceGridFocused {
                pid,
                cols: 2,
                rows: 2,
                col: 1,
                row: 1,
                opts: PlaceAttemptOptions::default(),
            }, // should win for pid
        ];

        // Local planner that mirrors drain_main_ops batch logic without touching the global queue.
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        enum Key {
            Id(WindowId),
            Pid(i32),
        }
        let mut non_place: Vec<MainOp> = Vec::new();
        let mut latest_by_id: std::collections::HashMap<WindowId, MainOp> =
            std::collections::HashMap::new();
        let mut latest_by_pid: std::collections::HashMap<i32, MainOp> =
            std::collections::HashMap::new();
        let mut order: Vec<Key> = Vec::new();
        for op in ops.into_iter() {
            match op {
                MainOp::PlaceGrid {
                    id,
                    cols,
                    rows,
                    col,
                    row,
                    opts,
                } => {
                    latest_by_id.insert(
                        id,
                        MainOp::PlaceGrid {
                            id,
                            cols,
                            rows,
                            col,
                            row,
                            opts,
                        },
                    );
                    order.retain(|k| *k != Key::Id(id));
                    order.push(Key::Id(id));
                }
                MainOp::PlaceMoveGrid {
                    id,
                    cols,
                    rows,
                    dir,
                    opts,
                } => {
                    latest_by_id.insert(
                        id,
                        MainOp::PlaceMoveGrid {
                            id,
                            cols,
                            rows,
                            dir,
                            opts,
                        },
                    );
                    order.retain(|k| *k != Key::Id(id));
                    order.push(Key::Id(id));
                }
                MainOp::PlaceGridFocused {
                    pid,
                    cols,
                    rows,
                    col,
                    row,
                    opts,
                } => {
                    latest_by_pid.insert(
                        pid,
                        MainOp::PlaceGridFocused {
                            pid,
                            cols,
                            rows,
                            col,
                            row,
                            opts,
                        },
                    );
                    order.retain(|k| *k != Key::Pid(pid));
                    order.push(Key::Pid(pid));
                }
                other => non_place.push(other),
            }
        }

        // Execute ordering: non-placement FIFO then placements by last-writer order.
        let mut applied: Vec<String> = Vec::new();
        for op in non_place.into_iter() {
            match op {
                MainOp::ActivatePid { pid } => applied.push(format!("activate:{}", pid)),
                MainOp::FocusDir { .. } => applied.push("focus".into()),
                _ => unreachable!(),
            }
        }
        for k in order.into_iter() {
            match k {
                Key::Id(idk) => match latest_by_id.remove(&idk).unwrap() {
                    MainOp::PlaceGrid { col, row, .. } => {
                        applied.push(format!("place:id:{}-{},{}", idk, col, row))
                    }
                    MainOp::PlaceMoveGrid { .. } => applied.push(format!("move:id:{}", idk)),
                    _ => unreachable!(),
                },
                Key::Pid(pk) => match latest_by_pid.remove(&pk).unwrap() {
                    MainOp::PlaceGridFocused { col, row, .. } => {
                        applied.push(format!("place:pid:{}-{},{}", pk, col, row))
                    }
                    _ => unreachable!(),
                },
            }
        }

        // We expect: ActivatePid, FocusDir, then one id placement (2,1) and one pid placement (1,1).
        assert_eq!(
            applied,
            vec![
                "activate:1".to_string(),
                "focus".to_string(),
                format!("place:id:{}-2,1", id),
                format!("place:pid:{}-1,1", pid),
            ]
        );
    }

    #[test]
    fn cross_type_stale_drop_prefers_id_over_focused() {
        // A focused placement for pid followed by an id-specific placement
        // for a window with the same pid should drop the focused placement.
        let id = 9001u32;
        let pid = 1337i32;
        clear_test_id_pid();
        set_test_id_pid(id, pid);
        {
            let mut q = MAIN_OPS.lock();
            q.clear();
        }
        let _ = crate::main_thread_ops::request_place_grid_focused(pid, 2, 2, 0, 0);
        let _ = crate::main_thread_ops::request_place_grid(id, 2, 2, 1, 1);
        {
            let mut w = super::PLACEMENT_EXECUTOR.write().unwrap();
            *w = Arc::new(CountingPlacement);
        }
        drain_main_ops();
        let (id_calls, focused_calls) = take_split_counts();
        assert_eq!(id_calls, 1);
        assert_eq!(focused_calls, 0);
        clear_test_id_pid();
        {
            let mut w = super::PLACEMENT_EXECUTOR.write().unwrap();
            *w = Arc::new(super::RealPlacement);
        }
    }
}
// === Placement executor indirection (no cfg(test) behavior swaps) ===
pub trait PlacementExecutor: Send + Sync {
    fn place_grid(
        &self,
        id: WindowId,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        opts: PlaceAttemptOptions,
    ) -> Result<()>;
    fn place_move_grid(
        &self,
        id: WindowId,
        cols: u32,
        rows: u32,
        dir: main_thread_ops::MoveDir,
        opts: PlaceAttemptOptions,
    ) -> Result<()>;
    fn place_grid_focused(
        &self,
        pid: i32,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        opts: PlaceAttemptOptions,
    ) -> Result<()>;
}

pub(crate) struct RealPlacement;
impl PlacementExecutor for RealPlacement {
    fn place_grid(
        &self,
        id: WindowId,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        opts: PlaceAttemptOptions,
    ) -> Result<()> {
        crate::place::place_grid_opts(id, cols, rows, col, row, opts)
    }
    fn place_move_grid(
        &self,
        id: WindowId,
        cols: u32,
        rows: u32,
        dir: main_thread_ops::MoveDir,
        opts: PlaceAttemptOptions,
    ) -> Result<()> {
        crate::place::place_move_grid_opts(id, cols, rows, dir, opts)
    }
    fn place_grid_focused(
        &self,
        pid: i32,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        opts: PlaceAttemptOptions,
    ) -> Result<()> {
        crate::place::place_grid_focused_opts(pid, cols, rows, col, row, opts)
    }
}

static PLACEMENT_EXECUTOR: Lazy<RwLock<Arc<dyn PlacementExecutor>>> =
    Lazy::new(|| RwLock::new(Arc::new(RealPlacement)));

fn placement() -> Arc<dyn PlacementExecutor> {
    PLACEMENT_EXECUTOR
        .read()
        .map(|g| Arc::clone(&*g))
        .unwrap_or_else(|_| Arc::new(RealPlacement))
}

// Optional id->pid override mapping (used by tests; always compiled to avoid cfg magic).
mod overrides {
    use std::{collections::HashMap, sync::Mutex};

    use once_cell::sync::Lazy;

    use super::WindowId;

    static MAP: Lazy<Mutex<HashMap<WindowId, i32>>> = Lazy::new(|| Mutex::new(HashMap::new()));
    #[allow(dead_code)]
    pub(crate) fn set_pid_for_id(id: WindowId, pid: i32) {
        MAP.lock().unwrap().insert(id, pid);
    }
    #[allow(dead_code)]
    pub(crate) fn clear_pid_overrides() {
        MAP.lock().unwrap().clear();
    }
    pub(crate) fn lookup(id: WindowId) -> Option<i32> {
        MAP.lock().unwrap().get(&id).copied()
    }
}
