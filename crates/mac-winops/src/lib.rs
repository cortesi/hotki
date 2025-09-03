//! mac-winops: macOS window operations for Hotki.
//!
//! Provides APIs to toggle/set native full screen (AppKit-managed Space)
//! and non‑native full screen (maximize to visible screen frame) on the
//! currently focused window of a given PID.
//!
//! All operations require Accessibility permission.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{CString, c_char, c_void};
use std::sync::Mutex;

use core_foundation::array::{
    CFArrayCreate, CFArrayGetCount, CFArrayGetValueAtIndex, kCFTypeArrayCallBacks,
};
use core_foundation::base::{CFRelease, CFType, CFTypeRef, TCFType};
use core_foundation::boolean::{kCFBooleanFalse, kCFBooleanTrue};
use core_foundation::dictionary::CFDictionaryRef;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use libloading::Library;
use mac_keycode::{Chord, Key, Modifier};
use objc2_app_kit::NSScreen;
use objc2_foundation::MainThreadMarker;
use once_cell::sync::Lazy;
use relaykey::RelayKey;
use tracing::debug;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
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
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        cStr: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFUUIDCreateString(alloc: *const c_void, uuid: *const c_void) -> CFStringRef;
}

// AXValue type constants (per Apple docs)
const K_AX_VALUE_CGPOINT_TYPE: i32 = 1;
const K_AX_VALUE_CGSIZE_TYPE: i32 = 2;

/// Alias for CoreGraphics CGWindowID (kCGWindowNumber).
pub type WindowId = u32;

/// Classification of a Space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceKind {
    User,
    Fullscreen,
    System,
    Unknown,
}

/// Info for a Space on a display.
#[derive(Debug, Clone)]
pub struct SpaceInfo {
    pub id: u64,
    pub index: u32,      // 0-based among user desktop spaces only
    pub kind: SpaceKind, // type classification
    pub is_current: bool,
}

/// Desired state for operations that can turn on/off or toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desired {
    On,
    Off,
    Toggle,
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
    #[error("Invalid index")]
    InvalidIndex,
}

type Result<T> = std::result::Result<T, Error>;

fn cfstr(name: &'static str) -> CFStringRef {
    const UTF8: u32 = 0x0800_0100;
    let cs = CString::new(name).expect("static str");
    unsafe { CFStringCreateWithCString(std::ptr::null(), cs.as_ptr(), UTF8) }
}

fn ax_check() -> Result<()> {
    if unsafe { AXIsProcessTrusted() } {
        Ok(())
    } else {
        Err(Error::Permission)
    }
}

fn focused_window_for_pid(pid: i32) -> Result<*mut c_void> {
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return Err(Error::AppElement);
    }
    let attr_focused_window = cfstr("AXFocusedWindow");
    let mut win: CFTypeRef = std::ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(app, attr_focused_window, &mut win) };
    unsafe { CFRelease(app as CFTypeRef) };
    if err != 0 {
        return Err(Error::AxCode(err));
    }
    if win.is_null() {
        return Err(Error::FocusedWindow);
    }
    Ok(win as *mut c_void)
}

fn ax_bool(element: *mut c_void, attr: CFStringRef) -> Result<Option<bool>> {
    let mut v: CFTypeRef = std::ptr::null_mut();
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
    let mut v: CFTypeRef = std::ptr::null_mut();
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
    let mut v: CFTypeRef = std::ptr::null_mut();
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

fn ax_window_title(element: *mut c_void) -> Option<String> {
    let attr_title = cfstr("AXTitle");
    let mut v: CFTypeRef = std::ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr_title, &mut v) };
    if err != 0 || v.is_null() {
        return None;
    }
    let s = unsafe { CFString::wrap_under_get_rule(v as CFStringRef) }.to_string();
    unsafe { CFRelease(v) };
    Some(s)
}

/// In-memory storage of pre-maximize frames to allow toggling back.
type FrameKey = (i32, String);
type FrameVal = (CGPoint, CGSize);
static PREV_FRAMES: Lazy<Mutex<HashMap<FrameKey, FrameVal>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < 1.0
}

fn rect_eq(p1: CGPoint, s1: CGSize, p2: CGPoint, s2: CGSize) -> bool {
    approx_eq(p1.x, p2.x)
        && approx_eq(p1.y, p2.y)
        && approx_eq(s1.width, s2.width)
        && approx_eq(s1.height, s2.height)
}

/// Toggle or set native full screen (AXFullScreen) for the focused window of `pid`.
///
/// Strategy: prefer AXFullScreen. If unsupported or fails, synthesize the
/// standard ⌃⌘F shortcut via `relaykey`.
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

    let mut prev_key: Option<(i32, String)> = None;
    let is_full = rect_eq(cur_p, cur_s, target_p, target_s);
    let do_set_to_full = match desired {
        Desired::On => true,
        Desired::Off => false,
        Desired::Toggle => !is_full,
    };

    // Identify window key for restore
    if let Some(title) = ax_window_title(win) {
        prev_key = Some((pid, title));
    }

    if do_set_to_full {
        // Store previous frame if we have a key and not already stored
        if let Some(k) = prev_key.clone()
            && let Ok(mut map) = PREV_FRAMES.lock()
        {
            map.entry(k).or_insert((cur_p, cur_s));
        }
        ax_set_point(win, attr_pos, target_p)?;
        ax_set_size(win, attr_size, target_s)?;
    } else {
        // Restore if available
        let restored = if let Some(k) = prev_key.clone() {
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
fn visible_frame_containing_point(mtm: MainThreadMarker, p: CGPoint) -> (f64, f64, f64, f64) {
    let screens = NSScreen::screens(mtm);
    let mut chosen = None;
    for s in screens.iter() {
        let fr = s.frame();
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
        scr.visibleFrame()
    } else {
        // Fallback to the first screen's frame if visible is unavailable
        NSScreen::screens(mtm)
            .iter()
            .next()
            .map(|s| s.visibleFrame())
            .unwrap_or_else(|| NSScreen::mainScreen(mtm).unwrap().visibleFrame())
    };
    (
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    )
}

// Compute the full frame (including menu bar and Dock areas) of the screen
// containing `p`. Falls back to main screen when not found.
fn frame_containing_point(mtm: MainThreadMarker, p: CGPoint) -> (f64, f64, f64, f64) {
    let screens = NSScreen::screens(mtm);
    let mut chosen = None;
    for s in screens.iter() {
        let fr = s.frame();
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
        scr.frame()
    } else {
        // Fallback to the first screen's frame if main screen unavailable
        NSScreen::screens(mtm)
            .iter()
            .next()
            .map(|s| s.frame())
            .unwrap_or_else(|| NSScreen::mainScreen(mtm).unwrap().frame())
    };
    (
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    )
}

/// Set the focused window's frame (position and size) for the given process id.
/// Units are AppKit points (pt).
pub fn set_window_frame(pid: i32, x: f64, y: f64, w: f64, h: f64) -> Result<()> {
    ax_check()?;
    let win = focused_window_for_pid(pid)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");
    let target_p = CGPoint { x, y };
    let target_s = CGSize {
        width: w,
        height: h,
    };

    // Adjust size first, then position to reduce post-resize drift.
    ax_set_size(win, attr_size, target_s)?;
    ax_set_point(win, attr_pos, target_p)?;

    unsafe { CFRelease(win as CFTypeRef) };
    Ok(())
}

/// Return the size (width, height) in points of the screen containing the
/// focused window for the given process id.
pub fn screen_size(pid: i32) -> Result<(f64, f64)> {
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let win = focused_window_for_pid(pid)?;
    let attr_pos = cfstr("AXPosition");
    let p = ax_get_point(win, attr_pos)?;
    let (_x, _y, w, h) = frame_containing_point(mtm, p);
    unsafe { CFRelease(win as CFTypeRef) };
    Ok((w, h))
}

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
}

static MAIN_OPS: Lazy<Mutex<VecDeque<MainOp>>> = Lazy::new(|| Mutex::new(VecDeque::new()));

/// Schedule a non‑native fullscreen operation to be executed on the main thread and
/// wake the Tao event loop via mac-focus-watcher.
pub fn request_fullscreen_nonnative(pid: i32, desired: Desired) -> Result<()> {
    if MAIN_OPS
        .lock()
        .map(|mut q| q.push_back(MainOp::FullscreenNonNative { pid, desired }))
        .is_err()
    {
        return Err(Error::Unsupported);
    }
    // Wake the Tao main loop to handle user event and drain ops
    let _ = mac_focus_watcher::wake_main_loop();
    Ok(())
}

/// Schedule a window placement operation to snap the focused window into a
/// grid cell on the current screen's visible frame. Runs on the AppKit main
/// thread and wakes the Tao event loop via mac-focus-watcher.
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
        return Err(Error::Unsupported);
    }
    let _ = mac_focus_watcher::wake_main_loop();
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub enum MoveDir {
    Left,
    Right,
    Up,
    Down,
}

/// Schedule a window movement within a grid.
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
        return Err(Error::Unsupported);
    }
    let _ = mac_focus_watcher::wake_main_loop();
    Ok(())
}

/// Drain and execute any pending main-thread operations. Must be called from the Tao main thread
/// (e.g., inside the Event::UserEvent handler in hotki-server).
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
    let vf = visible_frame_containing_point(mtm, cur_p);
    let vf_x = vf.0;
    let vf_y = vf.1;
    let vf_w = vf.2.max(1.0);
    let vf_h = vf.3.max(1.0);

    // Compute base tile sizes and remainders so last column/row absorb leftover pixels
    let c = cols.max(1) as f64;
    let r = rows.max(1) as f64;
    let tile_w = (vf_w / c).floor().max(1.0);
    let tile_h = (vf_h / r).floor().max(1.0);
    let rem_w = vf_w - tile_w * (cols as f64);
    let rem_h = vf_h - tile_h * (rows as f64);

    // Bounds clamp defensively; config/keymode should already validate
    let col = col.min(cols.saturating_sub(1));
    let row = row.min(rows.saturating_sub(1));

    // X/width
    let x = vf_x + tile_w * (col as f64);
    let w = if col == cols.saturating_sub(1) {
        tile_w + rem_w
    } else {
        tile_w
    };

    // Y/height: top-left is (0,0), macOS origin is bottom-left; bottom row gets remainder
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

fn approx_eq_eps(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
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
    fn CGGetActiveDisplayList(max: u32, active: *mut u32, count: *mut u32) -> i32;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> CGRectCGS;
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> *const c_void; // CFUUIDRef
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CGRectCGS {
    origin: CGPointCGS,
    size: CGSizeCGS,
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CGPointCGS {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CGSizeCGS {
    width: f64,
    height: f64,
}

const K_CG_WINDOW_LIST_OPTION_ALL: u32 = 0;
const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
const K_CG_WINDOW_LIST_OPTION_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;

fn cg_key(s: &'static str) -> CFStringRef {
    cfstr(s)
}
const K_CG_WINDOW_NUMBER: &str = "kCGWindowNumber";
const K_CG_WINDOW_OWNER_PID: &str = "kCGWindowOwnerPID";
const K_CG_WINDOW_LAYER: &str = "kCGWindowLayer";
const K_CG_WINDOW_BOUNDS: &str = "kCGWindowBounds";
const K_CG_WINDOW_IS_ONSCREEN: &str = "kCGWindowIsOnscreen";
const K_CG_WINDOW_ALPHA: &str = "kCGWindowAlpha";

fn display_uuid_cfstring_for_point(px: f64, py: f64) -> Option<CFString> {
    // Minimal unsafe: scope around each FFI call.
    let mut count: u32 = 0;
    unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut count) };
    if count == 0 {
        return None;
    }
    let mut buf = vec![0u32; count as usize];
    let rc = unsafe { CGGetActiveDisplayList(count, buf.as_mut_ptr(), &mut count) };
    if rc != 0 || count == 0 {
        return None;
    }
    let mut chosen = None;
    for &d in &buf {
        let r = unsafe { CGDisplayBounds(d) };
        let x = r.origin.x;
        let y = r.origin.y;
        let w = r.size.width;
        let h = r.size.height;
        if px >= x && px <= x + w && py >= y && py <= y + h {
            chosen = Some(d);
            break;
        }
    }
    let display = chosen.unwrap_or_else(|| unsafe { CGMainDisplayID() });
    let uuid = unsafe { CGDisplayCreateUUIDFromDisplayID(display) };
    if uuid.is_null() {
        return None;
    }
    let sref = unsafe { CFUUIDCreateString(std::ptr::null(), uuid) };
    if sref.is_null() {
        return None;
    }
    Some(unsafe { CFString::wrap_under_create_rule(sref) })
}

fn window_bounds_for_id(window_id: WindowId) -> Option<(f64, f64, f64, f64)> {
    unsafe {
        let arr_ref = CGWindowListCopyWindowInfo(
            K_CG_WINDOW_LIST_OPTION_EXCLUDE_DESKTOP_ELEMENTS | K_CG_WINDOW_LIST_OPTION_ALL,
            0,
        );
        if arr_ref.is_null() {
            return None;
        }
        let arr: core_foundation::array::CFArray<*const c_void> =
            core_foundation::array::CFArray::wrap_under_create_rule(arr_ref as _);
        let key_num = cg_key(K_CG_WINDOW_NUMBER);
        let key_bounds = cg_key(K_CG_WINDOW_BOUNDS);
        for i in 0..CFArrayGetCount(arr.as_concrete_TypeRef()) {
            let d = CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) as CFDictionaryRef;
            if d.is_null() {
                continue;
            }
            if let Some(n) = dict_get_cfnumber(d, key_num)
                && n.to_i64().unwrap_or(-1) as u32 == window_id
                && let Some(b) = dict_get_dictionary(d, key_bounds)
            {
                let x = dict_get_cfnumber(b, cfstr("X"))
                    .and_then(|n| n.to_f64())
                    .unwrap_or(0.0);
                let y = dict_get_cfnumber(b, cfstr("Y"))
                    .and_then(|n| n.to_f64())
                    .unwrap_or(0.0);
                let w = dict_get_cfnumber(b, cfstr("Width"))
                    .and_then(|n| n.to_f64())
                    .unwrap_or(0.0);
                let h = dict_get_cfnumber(b, cfstr("Height"))
                    .and_then(|n| n.to_f64())
                    .unwrap_or(0.0);
                return Some((x, y, w, h));
            }
        }
        None
    }
}

// ===== SkyLight dynamic bindings =====

struct SkyLight {
    _lib: Library,
    main_conn: unsafe extern "C" fn() -> i32,
    copy_managed_display_spaces: unsafe extern "C" fn(i32) -> CFTypeRef,
    managed_display_get_current_space: unsafe extern "C" fn(i32, CFStringRef) -> u64,
    move_windows_to_managed_space: unsafe extern "C" fn(i32, CFTypeRef, u64) -> i32,
    managed_display_set_current_space: Option<unsafe extern "C" fn(i32, CFStringRef, u64) -> i32>,
    copy_spaces_for_windows: Option<unsafe extern "C" fn(i32, u32, CFTypeRef) -> CFTypeRef>,
}

impl SkyLight {
    fn load() -> Result<&'static SkyLight> {
        static CELL: once_cell::sync::OnceCell<SkyLight> = once_cell::sync::OnceCell::new();
        CELL.get_or_try_init(|| unsafe {
            let lib = Library::new("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight")
                .map_err(|_| Error::Unsupported)?;
            let main_conn = *lib
                .get::<unsafe extern "C" fn() -> i32>(b"SLSMainConnectionID\0")
                .map_err(|_| Error::Unsupported)?;
            let copy_managed_display_spaces = *lib
                .get::<unsafe extern "C" fn(i32) -> CFTypeRef>(b"SLSCopyManagedDisplaySpaces\0")
                .map_err(|_| Error::Unsupported)?;
            let managed_display_get_current_space = *lib
                .get::<unsafe extern "C" fn(i32, CFStringRef) -> u64>(
                    b"SLSManagedDisplayGetCurrentSpace\0",
                )
                .map_err(|_| Error::Unsupported)?;
            let move_windows_to_managed_space = *lib
                .get::<unsafe extern "C" fn(i32, CFTypeRef, u64) -> i32>(
                    b"SLSMoveWindowsToManagedSpace\0",
                )
                .map_err(|_| Error::Unsupported)?;
            let managed_display_set_current_space = lib
                .get::<unsafe extern "C" fn(i32, CFStringRef, u64) -> i32>(
                    b"SLSManagedDisplaySetCurrentSpace\0",
                )
                .ok()
                .map(|s| *s);
            let copy_spaces_for_windows = lib
                .get::<unsafe extern "C" fn(i32, u32, CFTypeRef) -> CFTypeRef>(
                    b"SLSCopySpacesForWindows\0",
                )
                .ok()
                .map(|s| *s);
            Ok(SkyLight {
                _lib: lib,
                main_conn,
                copy_managed_display_spaces,
                managed_display_get_current_space,
                move_windows_to_managed_space,
                managed_display_set_current_space,
                copy_spaces_for_windows,
            })
        })
    }
}

fn cfstring_to_string(s: CFStringRef) -> String {
    unsafe { CFString::wrap_under_get_rule(s) }.to_string()
}

fn dict_get_cfstring(d: CFDictionaryRef, key: CFStringRef) -> Option<CFStringRef> {
    unsafe {
        let v = core_foundation::dictionary::CFDictionaryGetValue(d, key as *const _);
        if v.is_null() {
            return None;
        }
        Some(v as CFStringRef)
    }
}

fn dict_get_cfnumber(d: CFDictionaryRef, key: CFStringRef) -> Option<CFNumber> {
    unsafe {
        let v = core_foundation::dictionary::CFDictionaryGetValue(d, key as *const _);
        if v.is_null() {
            return None;
        }
        Some(CFNumber::wrap_under_get_rule(v as _))
    }
}

fn dict_get_cfbool(d: CFDictionaryRef, key: CFStringRef) -> Option<bool> {
    unsafe {
        let v = core_foundation::dictionary::CFDictionaryGetValue(d, key as *const _);
        if v.is_null() {
            return None;
        }
        Some(CFBooleanGetValue(v as _))
    }
}

fn dict_get_dictionary(d: CFDictionaryRef, key: CFStringRef) -> Option<CFDictionaryRef> {
    unsafe {
        let v = core_foundation::dictionary::CFDictionaryGetValue(d, key as *const _);
        if v.is_null() {
            return None;
        }
        Some(v as _)
    }
}

fn dict_get_array(d: CFDictionaryRef, key: CFStringRef) -> Option<core_foundation::array::CFArray> {
    unsafe {
        let v = core_foundation::dictionary::CFDictionaryGetValue(d, key as *const _);
        if v.is_null() {
            return None;
        }
        Some(core_foundation::array::CFArray::wrap_under_get_rule(v as _))
    }
}

fn parse_space_kind(ty: Option<i64>) -> SpaceKind {
    match ty {
        Some(4) | Some(16) => SpaceKind::Fullscreen,
        Some(2) => SpaceKind::System,
        // Treat unknown or missing types as user desktops by default.
        _ => SpaceKind::User,
    }
}

fn list_spaces_for_display_uuid(uuid: &CFString) -> Result<Vec<SpaceInfo>> {
    let sl = SkyLight::load()?;
    let conn = unsafe { (sl.main_conn)() };
    let arr_ref = unsafe { (sl.copy_managed_display_spaces)(conn) };
    if arr_ref.is_null() {
        return Err(Error::Unsupported);
    }
    let arr: core_foundation::array::CFArray<*const c_void> =
        unsafe { core_foundation::array::CFArray::wrap_under_create_rule(arr_ref as _) };

    let mut found_display: Option<CFDictionaryRef> = None;
    let key_display_identifier = cfstr("Display Identifier");
    for i in 0..unsafe { CFArrayGetCount(arr.as_concrete_TypeRef()) } {
        let d = unsafe { CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) } as CFDictionaryRef;
        if d.is_null() {
            continue;
        }
        if let Some(sref) = dict_get_cfstring(d, key_display_identifier) {
            let s = cfstring_to_string(sref);
            if s == *uuid {
                found_display = Some(d);
                break;
            }
        }
    }
    // Fallback for systems with global Spaces (no per-display UUID entry):
    let d = match found_display {
        Some(d) => d,
        None => {
            // Use the first managed display entry if present
            if unsafe { CFArrayGetCount(arr.as_concrete_TypeRef()) } > 0 {
                (unsafe { CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), 0) }) as CFDictionaryRef
            } else {
                return Err(Error::Unsupported);
            }
        }
    };

    let cur_id =
        unsafe { (sl.managed_display_get_current_space)(conn, uuid.as_concrete_TypeRef()) };
    let key_spaces = cfstr("Spaces");
    let key_type = cfstr("type");
    let key_managed_id = cfstr("ManagedSpaceID");
    let key_id64 = cfstr("id64");
    let spaces_arr = dict_get_array(d, key_spaces).ok_or(Error::Unsupported)?;
    let mut infos = Vec::new();
    let mut user_ix = 0u32;
    for i in 0..unsafe { CFArrayGetCount(spaces_arr.as_concrete_TypeRef()) } {
        let sd = unsafe { CFArrayGetValueAtIndex(spaces_arr.as_concrete_TypeRef(), i) }
            as CFDictionaryRef;
        if sd.is_null() {
            continue;
        }
        let id = if let Some(n) = dict_get_cfnumber(sd, key_id64) {
            n.to_i64().unwrap_or(0) as u64
        } else if let Some(n) = dict_get_cfnumber(sd, key_managed_id) {
            n.to_i64().unwrap_or(0) as u64
        } else {
            0
        };
        if id == 0 {
            continue;
        }
        let kind = parse_space_kind(dict_get_cfnumber(sd, key_type).and_then(|n| n.to_i64()));
        let is_user = matches!(kind, SpaceKind::User);
        let idx = if is_user {
            let c = user_ix;
            user_ix += 1;
            c
        } else {
            u32::MAX
        };
        infos.push(SpaceInfo {
            id,
            index: idx,
            kind,
            is_current: id == cur_id,
        });
    }
    // Normalize indices for user spaces 0..N-1
    let mut next = 0u32;
    for info in infos.iter_mut() {
        if matches!(info.kind, SpaceKind::User) {
            info.index = next;
            next += 1;
        } else {
            info.index = u32::MAX;
        }
    }
    Ok(infos)
}

fn display_uuid_for_window(window_id: WindowId) -> Result<CFString> {
    // First try SkyLight: WindowId -> SpaceId(s) -> Display UUID
    if let Ok(sl) = SkyLight::load()
        && let Some(copy_fn) = sl.copy_spaces_for_windows
    {
        let conn = unsafe { (sl.main_conn)() };
        let win_num = CFNumber::from(window_id as i64);
        let mut vals: [*const c_void; 1] = [win_num.as_CFTypeRef()];
        let wins = unsafe {
            CFArrayCreate(
                std::ptr::null(),
                vals.as_mut_ptr(),
                1,
                &kCFTypeArrayCallBacks,
            )
        };
        if !wins.is_null() {
            let spaces_ref = unsafe { copy_fn(conn, 0, wins as CFTypeRef) };
            // Release input array (created with Create)
            unsafe { CFRelease(wins as CFTypeRef) };
            if !spaces_ref.is_null() {
                let spaces: core_foundation::array::CFArray<*const c_void> = unsafe {
                    core_foundation::array::CFArray::wrap_under_create_rule(spaces_ref as _)
                };
                // Pull the managed display spaces to map space_id -> display UUID
                let arr_ref = unsafe { (sl.copy_managed_display_spaces)(conn) };
                if !arr_ref.is_null() {
                    let displays: core_foundation::array::CFArray<*const c_void> = unsafe {
                        core_foundation::array::CFArray::wrap_under_create_rule(arr_ref as _)
                    };
                    let key_disp_id = cfstr("Display Identifier");
                    let key_spaces = cfstr("Spaces");
                    let key_id64 = cfstr("id64");
                    let key_managed_id = cfstr("ManagedSpaceID");

                    // Build Vec<(space_id, display_uuid_string)>
                    let mut space_to_display: Vec<(u64, String)> = Vec::new();
                    for i in 0..unsafe { CFArrayGetCount(displays.as_concrete_TypeRef()) } {
                        let dd =
                            unsafe { CFArrayGetValueAtIndex(displays.as_concrete_TypeRef(), i) }
                                as CFDictionaryRef;
                        if dd.is_null() {
                            continue;
                        }
                        let disp_uuid = if let Some(s) = dict_get_cfstring(dd, key_disp_id) {
                            cfstring_to_string(s)
                        } else {
                            continue;
                        };
                        if let Some(sp_arr) = dict_get_array(dd, key_spaces) {
                            for j in 0..unsafe { CFArrayGetCount(sp_arr.as_concrete_TypeRef()) } {
                                let sd = unsafe {
                                    CFArrayGetValueAtIndex(sp_arr.as_concrete_TypeRef(), j)
                                } as CFDictionaryRef;
                                if sd.is_null() {
                                    continue;
                                }
                                let sid = if let Some(n) = dict_get_cfnumber(sd, key_id64) {
                                    n.to_i64().unwrap_or(0) as u64
                                } else if let Some(n) = dict_get_cfnumber(sd, key_managed_id) {
                                    n.to_i64().unwrap_or(0) as u64
                                } else {
                                    0
                                };
                                if sid != 0 {
                                    space_to_display.push((sid, disp_uuid.clone()));
                                }
                            }
                        }
                    }

                    // Read spaces returned for the window and map to display UUID
                    for i in 0..unsafe { CFArrayGetCount(spaces.as_concrete_TypeRef()) } {
                        let v = unsafe { CFArrayGetValueAtIndex(spaces.as_concrete_TypeRef(), i) };
                        if v.is_null() { continue; }
                        let any = unsafe { CFType::wrap_under_get_rule(v as _) };
                        // Try number first, then dictionary
                        let mut sid: u64 = 0;
                        if let Some(n) = any.downcast::<CFNumber>() {
                            sid = n.to_i64().unwrap_or(0) as u64;
                        } else if let Some(dict) = any.downcast::<core_foundation::dictionary::CFDictionary>() {
                            let sd = dict.as_concrete_TypeRef();
                            sid = if let Some(n) = dict_get_cfnumber(sd, key_id64) {
                                n.to_i64().unwrap_or(0) as u64
                            } else if let Some(n) = dict_get_cfnumber(sd, key_managed_id) {
                                n.to_i64().unwrap_or(0) as u64
                            } else { 0 };
                        }
                        if sid != 0
                            && let Some((_, disp)) = space_to_display.iter().find(|(k, _)| *k == sid) {
                                return Ok(CFString::new(disp));
                            }
                    }
                }
            }
        }
    }

    // Fallback: visible windows only, via bounds → display mapping.
    if let Some((x, y, w, h)) = window_bounds_for_id(window_id) {
        let cx = x + w / 2.0;
        let cy = y + h / 2.0;
        if let Some(uuid) = display_uuid_cfstring_for_point(cx, cy) {
            return Ok(uuid);
        }
    }
    Err(Error::Unsupported)
}

/// Enumerate spaces for the display that contains `window_id` (0-based user desktops).
pub fn list_spaces(window_id: WindowId) -> Result<Vec<SpaceInfo>> {
    let uuid = display_uuid_for_window(window_id)?;
    list_spaces_for_display_uuid(&uuid)
}

/// Return (space_id, 0-based user index) for the current user desktop on the display containing `window_id`.
pub fn current_space(window_id: WindowId) -> Result<(u64, u32)> {
    let uuid = display_uuid_for_window(window_id)?;
    let infos = list_spaces_for_display_uuid(&uuid)?;
    let mut cur_id = 0u64;
    let mut cur_user_ix = 0u32;
    let mut last_user_ix = 0u32;
    for info in &infos {
        if matches!(info.kind, SpaceKind::User) {
            last_user_ix = info.index;
        }
        if info.is_current {
            cur_id = info.id;
            cur_user_ix = if matches!(info.kind, SpaceKind::User) {
                info.index
            } else {
                last_user_ix
            };
        }
    }
    if cur_id == 0 {
        return Err(Error::Unsupported);
    }
    Ok((cur_id, cur_user_ix))
}

/// Move `window_id` to the user desktop with 0-based `index` on its display.
pub fn move_to_space(window_id: WindowId, index: u32) -> Result<()> {
    let uuid = display_uuid_for_window(window_id)?;
    let infos = list_spaces_for_display_uuid(&uuid)?;
    let mut users: Vec<&SpaceInfo> = infos
        .iter()
        .filter(|s| matches!(s.kind, SpaceKind::User))
        .collect();
    users.sort_by_key(|s| s.index);
    let target = users
        .into_iter()
        .find(|s| s.index == index)
        .ok_or(Error::InvalidIndex)?;

    let win_num = CFNumber::from(window_id as i64);
    let mut vals = vec![win_num.as_CFTypeRef()];
    let arr = unsafe {
        CFArrayCreate(
            std::ptr::null(),
            vals.as_mut_ptr(),
            1,
            &kCFTypeArrayCallBacks,
        )
    };
    if arr.is_null() {
        return Err(Error::Unsupported);
    }
    let sl = SkyLight::load()?;
    let conn = unsafe { (sl.main_conn)() };
    let rc = unsafe { (sl.move_windows_to_managed_space)(conn, arr as CFTypeRef, target.id) };
    unsafe { CFRelease(arr as CFTypeRef) };
    if rc != 0 {
        debug!(
            "move_to_space_failed rc={} target_id={} index={}",
            rc, target.id, index
        );
        return Err(Error::Unsupported);
    }
    Ok(())
}

/// Switch the user to the user desktop with 0-based `index` on the display that contains `window_id`.
/// Non-fatal on failure; returns Ok(()) when unsupported or fails.
pub fn switch_to_space(window_id: WindowId, index: u32) -> Result<()> {
    let uuid = display_uuid_for_window(window_id)?;
    let infos = list_spaces_for_display_uuid(&uuid)?;
    let mut users: Vec<&SpaceInfo> = infos
        .iter()
        .filter(|s| matches!(s.kind, SpaceKind::User))
        .collect();
    users.sort_by_key(|s| s.index);
    let target = match users.into_iter().find(|s| s.index == index) {
        Some(s) => s,
        None => return Err(Error::InvalidIndex),
    };
    if let Ok(sl) = SkyLight::load() {
        if let Some(set_fn) = sl.managed_display_set_current_space {
            let conn = unsafe { (sl.main_conn)() };
            let rc = unsafe { set_fn(conn, uuid.as_concrete_TypeRef(), target.id) };
            if rc != 0 {
                debug!(
                    "failed_to_switch_space rc={} target_id={} index={}",
                    rc, target.id, index
                );
            }
        } else {
            debug!("switch_space_unsupported_symbol_missing");
        }
    } else {
        debug!("switch_space_unsupported_loader");
    }
    Ok(())
}

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
        let arr: core_foundation::array::CFArray<*const c_void> =
            core_foundation::array::CFArray::wrap_under_create_rule(arr_ref as _);
        let key_pid = cg_key(K_CG_WINDOW_OWNER_PID);
        let key_layer = cg_key(K_CG_WINDOW_LAYER);
        let key_num = cg_key(K_CG_WINDOW_NUMBER);
        let key_onscreen = cg_key(K_CG_WINDOW_IS_ONSCREEN);
        let key_alpha = cg_key(K_CG_WINDOW_ALPHA);
        for i in 0..CFArrayGetCount(arr.as_concrete_TypeRef()) {
            let d = CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) as CFDictionaryRef;
            if d.is_null() {
                continue;
            }
            let dp = dict_get_cfnumber(d, key_pid)
                .and_then(|n| n.to_i64())
                .unwrap_or(-1) as i32;
            if dp != pid {
                continue;
            }
            let layer = dict_get_cfnumber(d, key_layer)
                .and_then(|n| n.to_i64())
                .unwrap_or(0);
            if layer != 0 {
                continue;
            }
            let onscreen = dict_get_cfbool(d, key_onscreen).unwrap_or(true);
            if !onscreen {
                continue;
            }
            let alpha = dict_get_cfnumber(d, key_alpha)
                .and_then(|n| n.to_f64())
                .unwrap_or(1.0);
            if alpha <= 0.0 {
                continue;
            }
            if let Some(n) = dict_get_cfnumber(d, key_num) {
                let id = n.to_i64().unwrap_or(-1);
                if id > 0 {
                    return Ok(id as u32);
                }
            }
        }
    }
    Err(Error::FocusedWindow)
}
