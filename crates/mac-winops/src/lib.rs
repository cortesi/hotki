//! mac-winops: macOS window operations for Hotki.
//!
//! Provides APIs to toggle/set native full screen (AppKit-managed Space)
//! and non‑native full screen (maximize to visible screen frame) on the
//! currently focused window of a given PID.
//!
//! All operations require Accessibility permission.

use std::ffi::{CString, c_char, c_void};
use std::sync::Mutex;

use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::boolean::{kCFBooleanFalse, kCFBooleanTrue};
use core_foundation::string::{CFString, CFStringRef};
use objc2_foundation::MainThreadMarker;
use once_cell::sync::Lazy;
use std::collections::HashMap;
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
}

// AXValue type constants (per Apple docs)
const K_AX_VALUE_CGPOINT_TYPE: i32 = 1;
const K_AX_VALUE_CGSIZE_TYPE: i32 = 2;

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
}

type Result<T> = std::result::Result<T, Error>;

fn cfstr(name: &'static str) -> CFStringRef {
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            cStr: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
    }
    const UTF8: u32 = 0x0800_0100;
    let cs = CString::new(name).expect("static str");
    unsafe { CFStringCreateWithCString(std::ptr::null(), cs.as_ptr(), UTF8) }
}

fn ax_check() -> Result<()> {
    unsafe {
        if AXIsProcessTrusted() {
            Ok(())
        } else {
            Err(Error::Permission)
        }
    }
}

fn focused_window_for_pid(pid: i32) -> Result<*mut c_void> {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Err(Error::AppElement);
        }
        let attr_focused_window = cfstr("AXFocusedWindow");
        let mut win: CFTypeRef = std::ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(app, attr_focused_window, &mut win);
        CFRelease(app as CFTypeRef);
        if err != 0 {
            return Err(Error::AxCode(err));
        }
        if win.is_null() {
            return Err(Error::FocusedWindow);
        }
        Ok(win as *mut c_void)
    }
}

fn ax_bool(element: *mut c_void, attr: CFStringRef) -> Result<Option<bool>> {
    unsafe {
        let mut v: CFTypeRef = std::ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(element, attr, &mut v);
        if err != 0 {
            // Not all windows expose AXFullScreen; treat as unsupported.
            return Err(Error::AxCode(err));
        }
        if v.is_null() {
            return Ok(None);
        }
        let b = CFBooleanGetValue(v);
        CFRelease(v);
        Ok(Some(b))
    }
}

fn ax_set_bool(element: *mut c_void, attr: CFStringRef, value: bool) -> Result<()> {
    unsafe {
        let val = if value {
            kCFBooleanTrue
        } else {
            kCFBooleanFalse
        } as CFTypeRef;
        let err = AXUIElementSetAttributeValue(element, attr, val);
        if err != 0 {
            return Err(Error::AxCode(err));
        }
        Ok(())
    }
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
    unsafe {
        let mut v: CFTypeRef = std::ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(element, attr, &mut v);
        if err != 0 {
            return Err(Error::AxCode(err));
        }
        if v.is_null() {
            return Err(Error::Unsupported);
        }
        let mut p = CGPoint { x: 0.0, y: 0.0 };
        let ok = AXValueGetValue(v, K_AX_VALUE_CGPOINT_TYPE, &mut p as *mut _ as *mut c_void);
        CFRelease(v);
        if !ok {
            return Err(Error::Unsupported);
        }
        Ok(p)
    }
}

fn ax_get_size(element: *mut c_void, attr: CFStringRef) -> Result<CGSize> {
    unsafe {
        let mut v: CFTypeRef = std::ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(element, attr, &mut v);
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
        let ok = AXValueGetValue(v, K_AX_VALUE_CGSIZE_TYPE, &mut s as *mut _ as *mut c_void);
        CFRelease(v);
        if !ok {
            return Err(Error::Unsupported);
        }
        Ok(s)
    }
}

fn ax_set_point(element: *mut c_void, attr: CFStringRef, p: CGPoint) -> Result<()> {
    unsafe {
        let v = AXValueCreate(K_AX_VALUE_CGPOINT_TYPE, &p as *const _ as *const c_void);
        if v.is_null() {
            return Err(Error::Unsupported);
        }
        let err = AXUIElementSetAttributeValue(element, attr, v);
        CFRelease(v);
        if err != 0 {
            return Err(Error::AxCode(err));
        }
        Ok(())
    }
}

fn ax_set_size(element: *mut c_void, attr: CFStringRef, s: CGSize) -> Result<()> {
    unsafe {
        let v = AXValueCreate(K_AX_VALUE_CGSIZE_TYPE, &s as *const _ as *const c_void);
        if v.is_null() {
            return Err(Error::Unsupported);
        }
        let err = AXUIElementSetAttributeValue(element, attr, v);
        CFRelease(v);
        if err != 0 {
            return Err(Error::AxCode(err));
        }
        Ok(())
    }
}

fn ax_window_title(element: *mut c_void) -> Option<String> {
    unsafe {
        let attr_title = cfstr("AXTitle");
        let mut v: CFTypeRef = std::ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(element, attr_title, &mut v);
        if err != 0 || v.is_null() {
            return None;
        }
        let s = CFString::wrap_under_get_rule(v as CFStringRef).to_string();
        CFRelease(v);
        Some(s)
    }
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
            use mac_keycode::{Chord, Key, Modifier};
            let mut mods = std::collections::HashSet::new();
            mods.insert(Modifier::Control);
            mods.insert(Modifier::Command);
            let chord = Chord {
                key: Key::F,
                modifiers: mods,
            };
            let rk = relaykey::RelayKey::new();
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
    use objc2_app_kit::NSScreen;
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
