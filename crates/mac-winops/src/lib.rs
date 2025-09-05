//! mac-winops: macOS window operations for Hotki.
//!
//! Provides APIs to toggle/set native full screen (AppKit-managed Space)
//! and non‑native full screen (maximize to visible screen frame) on the
//! currently focused window of a given PID.
//!
//! All operations require Accessibility permission.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::c_void,
    ptr,
    sync::Mutex,
};

use core_foundation::{
    array::{CFArray, CFArrayGetCount, CFArrayGetValueAtIndex},
    base::{CFRelease, CFTypeRef, TCFType},
    boolean::{kCFBooleanFalse, kCFBooleanTrue},
    dictionary::CFDictionaryRef,
    string::{CFString, CFStringRef},
};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSScreen};
use objc2_foundation::MainThreadMarker;
use once_cell::sync::Lazy;
use tracing::{debug, trace, warn};

use mac_keycode::{Chord, Key, Modifier};
use relaykey::RelayKey;

mod cfutil;
pub mod focus;
mod raise;
pub use raise::raise_window;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
    fn AXUIElementCopyAttributeValue(
        element: *mut c_void,
        attr: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementSetAttributeValue(
        element: *mut c_void,
        attr: CFStringRef,
        value: CFTypeRef,
    ) -> i32;

    // AXValue helpers for CGPoint/CGSize
    fn AXValueCreate(theType: i32, valuePtr: *const c_void) -> CFTypeRef;
    fn AXValueGetValue(theValue: CFTypeRef, theType: i32, valuePtr: *mut c_void) -> bool;
}

// CFBooleanGetValue is part of CoreFoundation
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFBooleanGetValue(b: CFTypeRef) -> bool;
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
}

// AXValue type constants (per Apple docs)
const K_AX_VALUE_CGPOINT_TYPE: i32 = 1;
const K_AX_VALUE_CGSIZE_TYPE: i32 = 2;

/// Alias for CoreGraphics CGWindowID (kCGWindowNumber).
pub type WindowId = u32;

/// Desired state for operations that can turn on/off or toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desired {
    On,
    Off,
    Toggle,
}

/// Screen corner to place the window against so that a 1×1 px corner remains visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenCorner {
    BottomRight,
    BottomLeft,
    TopLeft,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Accessibility permission missing")]
    Permission,
    #[error("Failed to create AX application element")]
    AppElement,
    #[error("Focused window not available")]
    FocusedWindow,
    #[error("AX operation failed: code {0}")]
    AxCode(i32),
    #[error("Operation requires main thread")]
    MainThread,
    #[error("Unsupported attribute")]
    Unsupported,
    #[error("Main-thread queue poisoned or push failed")]
    QueuePoisoned,
    #[error("Invalid index")]
    InvalidIndex,
    #[error("Activation failed")]
    ActivationFailed,
}

type Result<T> = std::result::Result<T, Error>;

fn cfstr(name: &'static str) -> CFStringRef {
    // Use a non-owning CFString backed by a static &'static str; no release needed.
    CFString::from_static_string(name).as_concrete_TypeRef()
}

fn ax_check() -> Result<()> {
    if permissions::accessibility_ok() {
        Ok(())
    } else {
        Err(Error::Permission)
    }
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

fn ax_bool(element: *mut c_void, attr: CFStringRef) -> Result<Option<bool>> {
    let mut v: CFTypeRef = ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut v) };
    if err != 0 {
        // Not all windows expose AXFullScreen; treat as unsupported.
        return Err(Error::AxCode(err));
    }
    if v.is_null() {
        return Ok(None);
    }
    let b = unsafe { CFBooleanGetValue(v) };
    unsafe { CFRelease(v) };
    Ok(Some(b))
}

fn ax_set_bool(element: *mut c_void, attr: CFStringRef, value: bool) -> Result<()> {
    let val = unsafe {
        (if value {
            kCFBooleanTrue
        } else {
            kCFBooleanFalse
        }) as CFTypeRef
    };
    let err = unsafe { AXUIElementSetAttributeValue(element, attr, val) };
    if err != 0 {
        return Err(Error::AxCode(err));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CGPoint {
    x: f64,
    y: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct CGSize {
    width: f64,
    height: f64,
}

fn ax_get_point(element: *mut c_void, attr: CFStringRef) -> Result<CGPoint> {
    let mut v: CFTypeRef = ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut v) };
    if err != 0 {
        return Err(Error::AxCode(err));
    }
    if v.is_null() {
        return Err(Error::Unsupported);
    }
    let mut p = CGPoint { x: 0.0, y: 0.0 };
    let ok =
        unsafe { AXValueGetValue(v, K_AX_VALUE_CGPOINT_TYPE, &mut p as *mut _ as *mut c_void) };
    unsafe { CFRelease(v) };
    if !ok {
        return Err(Error::Unsupported);
    }
    Ok(p)
}

fn ax_get_size(element: *mut c_void, attr: CFStringRef) -> Result<CGSize> {
    let mut v: CFTypeRef = ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut v) };
    if err != 0 {
        return Err(Error::AxCode(err));
    }
    if v.is_null() {
        return Err(Error::Unsupported);
    }
    let mut s = CGSize {
        width: 0.0,
        height: 0.0,
    };
    let ok = unsafe { AXValueGetValue(v, K_AX_VALUE_CGSIZE_TYPE, &mut s as *mut _ as *mut c_void) };
    unsafe { CFRelease(v) };
    if !ok {
        return Err(Error::Unsupported);
    }
    Ok(s)
}

fn ax_get_string(element: *mut c_void, attr: CFStringRef) -> Option<String> {
    let mut v: CFTypeRef = ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut v) };
    if err != 0 || v.is_null() {
        return None;
    }
    let s = unsafe { CFString::wrap_under_create_rule(v as _) };
    Some(s.to_string())
}

fn ax_set_point(element: *mut c_void, attr: CFStringRef, p: CGPoint) -> Result<()> {
    let v = unsafe { AXValueCreate(K_AX_VALUE_CGPOINT_TYPE, &p as *const _ as *const c_void) };
    if v.is_null() {
        return Err(Error::Unsupported);
    }
    let err = unsafe { AXUIElementSetAttributeValue(element, attr, v) };
    unsafe { CFRelease(v) };
    if err != 0 {
        return Err(Error::AxCode(err));
    }
    Ok(())
}

fn ax_set_size(element: *mut c_void, attr: CFStringRef, s: CGSize) -> Result<()> {
    let v = unsafe { AXValueCreate(K_AX_VALUE_CGSIZE_TYPE, &s as *const _ as *const c_void) };
    if v.is_null() {
        return Err(Error::Unsupported);
    }
    let err = unsafe { AXUIElementSetAttributeValue(element, attr, v) };
    unsafe { CFRelease(v) };
    if err != 0 {
        return Err(Error::AxCode(err));
    }
    Ok(())
}

/// In-memory storage of pre-maximize frames to allow toggling back.
type FrameKey = (i32, WindowId);
type FrameVal = (CGPoint, CGSize);
static PREV_FRAMES: Lazy<Mutex<HashMap<FrameKey, FrameVal>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
const PREV_FRAMES_CAP: usize = 256;

/// Frames stored before hiding so we can restore on reveal
static HIDDEN_FRAMES: Lazy<Mutex<HashMap<FrameKey, FrameVal>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
const HIDDEN_FRAMES_CAP: usize = 512;

fn rect_eq(p1: CGPoint, s1: CGSize, p2: CGPoint, s2: CGSize) -> bool {
    approx_eq_eps(p1.x, p2.x, 1.0)
        && approx_eq_eps(p1.y, p2.y, 1.0)
        && approx_eq_eps(s1.width, s2.width, 1.0)
        && approx_eq_eps(s1.height, s2.height, 1.0)
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
    if let Ok(wid) = focused_window_id(pid) {
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

// Compute the visible frame rect of the screen containing `p_ax` in AX coordinates
// (top-left origin, y increases downward). Falls back to the main screen if none match.
// (no-op placeholder; conversion helper removed)

/// Queue of operations that must run on the AppKit main thread.
enum MainOp {
    FullscreenNonNative {
        pid: i32,
        desired: Desired,
    },
    PlaceGrid {
        pid: i32,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
    },
    PlaceMoveGrid {
        pid: i32,
        cols: u32,
        rows: u32,
        dir: MoveDir,
    },
    /// Best-effort app activation for a pid (fallback for raise).
    ActivatePid {
        pid: i32,
    },
    RaiseWindow {
        pid: i32,
        id: WindowId,
    },
}

static MAIN_OPS: Lazy<Mutex<VecDeque<MainOp>>> = Lazy::new(|| Mutex::new(VecDeque::new()));

/// Schedule a non‑native fullscreen operation to be executed on the AppKit main
/// thread and wake the Tao event loop.
pub fn request_fullscreen_nonnative(pid: i32, desired: Desired) -> Result<()> {
    if MAIN_OPS
        .lock()
        .map(|mut q| q.push_back(MainOp::FullscreenNonNative { pid, desired }))
        .is_err()
    {
        return Err(Error::QueuePoisoned);
    }
    // Wake the Tao main loop to handle user event and drain ops
    let _ = crate::focus::post_user_event();
    Ok(())
}

/// Schedule a window placement operation to snap the focused window into a
/// grid cell on the current screen's visible frame. Runs on the AppKit main
/// thread and wakes the Tao event loop.
pub fn request_place_grid(pid: i32, cols: u32, rows: u32, col: u32, row: u32) -> Result<()> {
    if cols == 0 || rows == 0 {
        return Err(Error::Unsupported);
    }
    if MAIN_OPS
        .lock()
        .map(|mut q| {
            q.push_back(MainOp::PlaceGrid {
                pid,
                cols,
                rows,
                col,
                row,
            })
        })
        .is_err()
    {
        return Err(Error::QueuePoisoned);
    }
    let _ = crate::focus::post_user_event();
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub enum MoveDir {
    Left,
    Right,
    Up,
    Down,
}

/// Schedule a window movement within a grid on the AppKit main thread.
pub fn request_place_move_grid(pid: i32, cols: u32, rows: u32, dir: MoveDir) -> Result<()> {
    if cols == 0 || rows == 0 {
        return Err(Error::Unsupported);
    }
    if MAIN_OPS
        .lock()
        .map(|mut q| {
            q.push_back(MainOp::PlaceMoveGrid {
                pid,
                cols,
                rows,
                dir,
            })
        })
        .is_err()
    {
        return Err(Error::QueuePoisoned);
    }
    let _ = crate::focus::post_user_event();
    Ok(())
}

/// Schedule a window raise by pid+id on the AppKit main thread.
pub fn request_raise_window(pid: i32, id: WindowId) -> Result<()> {
    if MAIN_OPS
        .lock()
        .map(|mut q| q.push_back(MainOp::RaiseWindow { pid, id }))
        .is_err()
    {
        return Err(Error::QueuePoisoned);
    }
    let _ = crate::focus::post_user_event();
    Ok(())
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
                pid,
                cols,
                rows,
                col,
                row,
            } => {
                let _ = place_grid(pid, cols, rows, col, row);
            }
            MainOp::PlaceMoveGrid {
                pid,
                cols,
                rows,
                dir,
            } => {
                let _ = place_move_grid(pid, cols, rows, dir);
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

/// Compute the visible frame for the screen containing the focused window and
/// place the window into the specified grid cell (top-left is (0,0)).
fn place_grid(pid: i32, cols: u32, rows: u32, col: u32, row: u32) -> Result<()> {
    ax_check()?;
    // For visibleFrame we need AppKit; require main thread.
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let win = focused_window_for_pid(pid)?;
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
/// remains visible on screen (bottom‑right), and restore it on reveal.
///
/// Strategy (public‑API friendly)
/// - Permissions: requires Accessibility trust; we no‑op if not trusted.
/// - Window resolution: resolve a top‑level `AXWindow` from the app’s
///   `AXWindows` list. We do not rely on having an AX focused window because
///   the HUD or transient sheets can steal focus.
/// - Identity for restore: we key stored frames by `(pid, CGWindowID)` obtained
///   from CoreGraphics (`CGWindowListCopyWindowInfo`), avoiding private APIs
///   while remaining stable across AX object re‑creations.
/// - Placement: when hiding, compute a bottom‑right target for the window’s
///   top‑left so that exactly 1 px is visible.
///   - Preferred: use `NSScreen::visibleFrame` of the screen containing the
///     current window and set position to `(right − 1, bottom − 1)`.
///   - Fallback (not on main thread): add a very large positive delta to both
///     X and Y (e.g., `+100_000`) and rely on WindowServer clamping to bottom‑right.
/// - Write order: set `AXSize` (no‑op) before `AXPosition`—some apps only
///   accept a move after any size set. If the large jump is ignored, apply a
///   small nudge (+40 px X) and retry.
/// - Reveal: restore the previously stored `(position,size)` for this window
///   if available.
///
/// Coordinate space & 1‑px rule
/// - AX window `kAXPositionAttribute`/`kAXSizeAttribute` use a top‑left origin
///   with Y increasing downward. `NSScreen::visibleFrame` provides the display’s
///   usable rect (excludes Dock/Menu Bar). Apple won’t allow a window to be
///   fully off‑screen, so a 1‑pixel sliver remains visible at the chosen edge.
///
/// Edge cases
/// - If a window is minimized (`kAXMinimizedAttribute`) or in native fullscreen
///   (`kAXFullScreenAttribute`), we skip reposition.
/// - On shutdown, callers should best‑effort restore any hidden frames.
/// - We avoid queuing to the AppKit main thread; if we can’t get a main‑thread
///   visibleFrame we rely on WindowServer clamping.
pub fn hide_right(pid: i32, desired: Desired) -> Result<()> {
    debug!("hide_right: entry pid={} desired={:?}", pid, desired);
    ax_check()?;
    debug!("hide_right: AX permission OK");
    // Best-effort: request activation to improve AXPosition success for some apps
    let _ = request_activate_pid(pid);
    // Resolve a top-level AXWindow from AXWindows (first entry with AXWindow role).
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
        // Retain chosen so it remains valid after arr drops
        if !chosen.is_null() {
            let _ = CFRetain(chosen as CFTypeRef);
        }
        CFRelease(app as CFTypeRef);
        if chosen.is_null() {
            return Err(Error::FocusedWindow);
        }
        chosen
    };
    debug!("hide_right: obtained top-level AX window for pid={}", pid);
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    // Current frame
    let cur_p = ax_get_point(win, attr_pos)?;
    let cur_s = ax_get_size(win, attr_size)?;
    debug!(
        "hide_right: current frame p=({:.1},{:.1}) s=({:.1},{:.1})",
        cur_p.x, cur_p.y, cur_s.width, cur_s.height
    );

    // Identify the window key for restore
    let key = focused_window_id(pid).ok().map(|id| (pid, id));
    let is_hidden = key
        .as_ref()
        .and_then(|k| HIDDEN_FRAMES.lock().ok().map(|m| m.contains_key(k)))
        .unwrap_or(false);

    // Determine operation
    let do_hide = match desired {
        Desired::On => true,
        Desired::Off => false,
        Desired::Toggle => !is_hidden,
    };

    debug!(
        "hide_right: pid={} is_hidden={} desired={:?}",
        pid, is_hidden, desired
    );
    if do_hide {
        // Store frame for restore
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

        // Compute target position so only the top-left 1×1 px of the window is visible
        // in the bottom-right corner of the screen that currently contains the window.
        // Preferred path: use NSScreen::visibleFrame on the AppKit main thread to get
        // the precise bottom-right coordinate, then place the window's top-left at
        // (right-1, bottom-1). Fallback path (not on main thread): add very large
        // deltas to both X and Y and rely on WindowServer clamping to bottom-right.
        let (target_x, target_y) = if let Some(mtm) = MainThreadMarker::new() {
            let (vf_x, vf_y, vf_w, vf_h) = visible_frame_containing_point(mtm, cur_p);
            ((vf_x + vf_w) - 1.0, (vf_y + vf_h) - 1.0)
        } else {
            (cur_p.x + 100_000.0, cur_p.y + 100_000.0)
        };
        debug!(
            "hide_right: cur_p=({:.1},{:.1}) cur_s=({:.1},{:.1}) -> target=({:.1},{:.1})",
            cur_p.x, cur_p.y, cur_s.width, cur_s.height, target_x, target_y
        );
        // Log basic window role info to aid diagnosis
        let role = ax_get_string(win, cfstr("AXRole")).unwrap_or_else(|| "<none>".into());
        let subrole = ax_get_string(win, cfstr("AXSubrole")).unwrap_or_else(|| "<none>".into());
        debug!("hide_right: target role='{}' subrole='{}'", role, subrole);

        // First attempt: large jump to the right (clamp by WindowServer).
        // Some apps accept AXPosition changes more reliably after any AXSize set.
        // Set size to current as a no-op before moving.
        let _ = ax_set_size(win, attr_size, cur_s);
        let first = ax_set_point(
            win,
            attr_pos,
            CGPoint {
                x: target_x,
                y: target_y,
            },
        );
        if let Err(e) = first {
            warn!("hide_right: AX set position failed: {:?}", e);
            unsafe { CFRelease(win as CFTypeRef) };
            return Err(e);
        }
        // Verify movement; if unchanged, try two-phase nudge then jump.
        match ax_get_point(win, attr_pos) {
            Ok(p1) if approx_eq_eps(p1.x, cur_p.x, 1.0) => {
                debug!("hide_right: no movement after first jump; applying nudge then retry");
                let _ = ax_set_point(
                    win,
                    attr_pos,
                    CGPoint {
                        x: cur_p.x + 40.0,
                        y: cur_p.y,
                    },
                );
                let _ = ax_set_point(
                    win,
                    attr_pos,
                    CGPoint {
                        x: target_x,
                        y: target_y,
                    },
                );
                // After retry, continue; caller will validate via HIDDEN_FRAMES on reveal
            }
            _ => {}
        }
        // no-op
    } else {
        // Reveal: restore size then position if we have a stored frame
        if let Some(k) = key
            && let Ok(mut map) = HIDDEN_FRAMES.lock()
            && let Some((p, s)) = map.remove(&k)
        {
            // Set size first to avoid AppKit clamping artifacts, then position
            let _ = ax_set_size(win, attr_size, s);
            let _ = ax_set_point(win, attr_pos, p);
            // No extra fallback needed; we already operate on a top-level AXWindow
        }
    }

    unsafe { CFRelease(win as CFTypeRef) };
    Ok(())
}

fn approx_eq_eps(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

/// Hide or reveal the focused window by sliding it so only a 1‑pixel corner
/// remains visible on screen for the given screen corner.
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

    let key = focused_window_id(pid).ok().map(|id| (pid, id));
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

/// Convenience wrappers for exploration
pub fn hide_bottom_left(pid: i32, desired: Desired) -> Result<()> {
    hide_corner(pid, desired, ScreenCorner::BottomLeft)
}

pub fn hide_top_left(pid: i32, desired: Desired) -> Result<()> {
    hide_corner(pid, desired, ScreenCorner::TopLeft)
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

#[cfg(test)]
mod tests {
    use super::{approx_eq_eps, cell_rect};

    #[test]
    fn approx_eq_eps_basic() {
        assert!(approx_eq_eps(1.0, 1.0, 0.0));
        assert!(approx_eq_eps(1.0, 1.000_5, 0.001));
        assert!(!approx_eq_eps(1.0, 1.01, 0.001));
    }

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

fn place_move_grid(pid: i32, cols: u32, rows: u32, dir: MoveDir) -> Result<()> {
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let win = focused_window_for_pid(pid)?;
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

// ===== CoreGraphics Window List and Display FFI =====

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relativeToWindow: u32) -> CFTypeRef; // CFArrayRef
}

// CoreGraphics window list options used by focused_window_id
const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
const K_CG_WINDOW_LIST_OPTION_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;

use crate::cfutil::{dict_get_bool, dict_get_f64, dict_get_i32, dict_get_string};
use core_graphics::window as cgw;

/// Convenience: resolve the focused top-level window’s CGWindowID for `pid`.
/// Best-effort: picks the first frontmost layer-0 on-screen window owned by pid.
pub fn focused_window_id(pid: i32) -> Result<WindowId> {
    ax_check()?;
    unsafe {
        let arr_ref = CGWindowListCopyWindowInfo(
            K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY
                | K_CG_WINDOW_LIST_OPTION_EXCLUDE_DESKTOP_ELEMENTS,
            0,
        );
        if arr_ref.is_null() {
            return Err(Error::FocusedWindow);
        }
        let arr: CFArray<*const c_void> = CFArray::wrap_under_create_rule(arr_ref as _);
        let key_pid = cgw::kCGWindowOwnerPID;
        let key_layer = cgw::kCGWindowLayer;
        let key_num = cgw::kCGWindowNumber;
        let key_onscreen = cgw::kCGWindowIsOnscreen;
        let key_alpha = cgw::kCGWindowAlpha;
        for i in 0..CFArrayGetCount(arr.as_concrete_TypeRef()) {
            let d = CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) as CFDictionaryRef;
            if d.is_null() {
                continue;
            }
            let dp = dict_get_i32(d, key_pid).unwrap_or(-1);
            if dp != pid {
                continue;
            }
            let layer = dict_get_i32(d, key_layer).unwrap_or(0) as i64;
            if layer != 0 {
                continue;
            }
            let onscreen = dict_get_bool(d, key_onscreen).unwrap_or(true);
            if !onscreen {
                continue;
            }
            let alpha = dict_get_f64(d, key_alpha).unwrap_or(1.0);
            if alpha <= 0.0 {
                continue;
            }
            if let Some(id) = dict_get_i32(d, key_num)
                && id > 0
            {
                return Ok(id as u32);
            }
        }
    }
    Err(Error::FocusedWindow)
}

/// Lightweight description of an on-screen window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub app: String,
    pub title: String,
    pub pid: i32,
    pub id: WindowId,
}

/// Return on-screen, layer-0 windows front-to-back.
pub fn list_windows() -> Vec<WindowInfo> {
    trace!("list_windows");
    let mut out = Vec::new();
    unsafe {
        let arr_ref = CGWindowListCopyWindowInfo(
            K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY
                | K_CG_WINDOW_LIST_OPTION_EXCLUDE_DESKTOP_ELEMENTS,
            0,
        );
        if arr_ref.is_null() {
            warn!("list_windows: CGWindowListCopyWindowInfo returned null");
            return out;
        }
        let arr: CFArray<*const c_void> = CFArray::wrap_under_create_rule(arr_ref as _);
        let key_pid = cgw::kCGWindowOwnerPID;
        let key_layer = cgw::kCGWindowLayer;
        let key_num = cgw::kCGWindowNumber;
        let key_onscreen = cgw::kCGWindowIsOnscreen;
        let key_alpha = cgw::kCGWindowAlpha;
        let key_app = cgw::kCGWindowOwnerName;
        let key_title = cgw::kCGWindowName;
        #[allow(non_snake_case)]
        unsafe extern "C" {
            fn CFGetTypeID(cf: CFTypeRef) -> u64;
            fn CFDictionaryGetTypeID() -> u64;
        }
        for i in 0..CFArrayGetCount(arr.as_concrete_TypeRef()) {
            let item = CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) as CFTypeRef;
            if item.is_null() {
                // Individual entry missing is not a fatal error; skip quietly.
                continue;
            }
            let is_dict = CFGetTypeID(item) == CFDictionaryGetTypeID();
            if !is_dict {
                // Unexpected type; skip without noisy logging.
                continue;
            }
            let d = item as CFDictionaryRef;
            let layer = dict_get_i32(d, key_layer).unwrap_or(0) as i64;
            if layer != 0 {
                continue;
            }
            let onscreen = dict_get_bool(d, key_onscreen).unwrap_or(true);
            if !onscreen {
                continue;
            }
            let alpha = dict_get_f64(d, key_alpha).unwrap_or(1.0);
            if alpha <= 0.0 {
                continue;
            }
            let pid = match dict_get_i32(d, key_pid) {
                Some(p) => p,
                None => continue,
            };
            let id = match dict_get_i32(d, key_num) {
                Some(n) if n > 0 => n as u32,
                _ => continue,
            };
            let app = dict_get_string(d, key_app).unwrap_or_default();
            let title = dict_get_string(d, key_title).unwrap_or_default();
            out.push(WindowInfo {
                app,
                title,
                pid,
                id,
            });
        }
    }
    out
}

/// Convenience: return the frontmost on-screen window, if any.
pub fn frontmost_window() -> Option<WindowInfo> {
    list_windows().into_iter().next()
}

/// Queue a best-effort activation of the application with `pid` on the AppKit main thread.
pub fn request_activate_pid(pid: i32) -> Result<()> {
    debug!("queue ActivatePid for pid={} on main thread", pid);
    if MAIN_OPS
        .lock()
        .map(|mut q| q.push_back(MainOp::ActivatePid { pid }))
        .is_err()
    {
        return Err(Error::QueuePoisoned);
    }
    let _ = crate::focus::post_user_event();
    Ok(())
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
