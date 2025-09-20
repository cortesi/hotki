use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{CStr, c_void},
    ptr, thread_local,
};

use core_foundation::{
    base::{CFRelease, CFTypeRef, TCFType},
    boolean::{kCFBooleanFalse, kCFBooleanTrue},
    string::{CFString, CFStringRef},
};
use objc2_app_kit::NSRunningApplication;
#[cfg(debug_assertions)]
use objc2_foundation::MainThreadMarker;
use tracing::{Level, debug, enabled, info};

use crate::{
    AXElem, WindowId,
    error::{Error, Result},
    geom::{Point, Size},
    window,
};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
    pub fn AXUIElementCopyAttributeValue(
        element: *mut c_void,
        attr: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    pub fn AXUIElementSetAttributeValue(
        element: *mut c_void,
        attr: CFStringRef,
        value: CFTypeRef,
    ) -> i32;
    pub fn AXUIElementGetPid(element: *mut c_void, pid: *mut i32) -> i32;
    pub fn AXUIElementIsAttributeSettable(
        element: *mut c_void,
        attr: CFStringRef,
        out_settable: *mut bool,
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
// AX error for invalid UI element (window closed / stale reference)
const K_AX_ERROR_INVALID_UI_ELEMENT: i32 = -25202;
// AX error returned when messaging cannot complete within the timeout window.
pub(crate) const K_AX_ERROR_CANNOT_COMPLETE: i32 = -25204;

#[inline]
pub(crate) fn ax_error_name(code: i32) -> &'static str {
    match code {
        0 => "Success",
        -25200 => "Failure",
        -25201 => "IllegalArgument",
        -25202 => "InvalidUIElement",
        -25203 => "InvalidObserver",
        -25204 => "CannotComplete",
        -25205 => "AttributeUnsupported",
        -25206 => "ActionUnsupported",
        -25207 => "NotificationUnsupported",
        -25208 => "NotImplemented",
        -25209 => "NotificationAlreadyRegistered",
        -25210 => "NotificationNotRegistered",
        -25211 => "APIDisabled",
        -25212 => "NoValue",
        -25213 => "ParameterizedAttributeUnsupported",
        -25214 => "NotEnoughPrecision",
        _ => "Unknown",
    }
}

thread_local! {
    static ATTR_STRINGS: RefCell<HashMap<&'static str, CFString>> = RefCell::new(HashMap::new());
}

// Cache AXIsAttributeSettable results per AX window element for position/size.
// Keyed by the AX element pointer address; values store cached booleans for
// AXPosition and AXSize respectively. We intentionally keep this thread-local
// because AX usage happens on the main thread in this crate.
type SettablePair = (Option<bool>, Option<bool>); // (AXPosition, AXSize)
thread_local! {
    static SETTABLE_CACHE: RefCell<HashMap<usize, SettablePair>> =
        RefCell::new(HashMap::new());
}

pub fn cfstr(name: &'static str) -> CFStringRef {
    // Return a stable CFStringRef for known attribute/action names. This avoids
    // relying on toll‑free bridging of static strings, which can trip pointer
    // authentication on recent macOS versions when CoreFoundation treats the
    // input as an Objective‑C NSString internally.
    ATTR_STRINGS.with(|cell| {
        let mut m = cell.borrow_mut();
        let s = m.entry(name).or_insert_with(|| CFString::new(name));
        s.as_concrete_TypeRef()
    })
}

pub fn ax_check() -> Result<()> {
    if permissions::accessibility_ok() {
        Ok(())
    } else {
        Err(Error::Permission)
    }
}

#[inline]
fn assert_main_thread_debug() {
    #[cfg(debug_assertions)]
    {
        let is_main = MainThreadMarker::new().is_some();
        debug_assert!(is_main, "AX mutation must run on the AppKit main thread");
    }
}

pub fn ax_bool(element: *mut c_void, attr: CFStringRef) -> Result<Option<bool>> {
    let mut v: CFTypeRef = ptr::null_mut();
    // SAFETY: `element` is a valid AXUIElement pointer and `&mut v` is an out‑param
    // that will receive a retained CFTypeRef if the call succeeds.
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut v) };
    if err != 0 {
        if err == K_AX_ERROR_INVALID_UI_ELEMENT {
            debug!(
                "AXUIElementCopyAttributeValue(bool) -> {} ({})",
                ax_error_name(err),
                err
            );
            return Err(Error::WindowGone);
        }
        debug!(
            "AXUIElementCopyAttributeValue(bool) failed: code={} ({})",
            err,
            ax_error_name(err)
        );
        // Not all windows expose this attribute; surface the code upstream.
        return Err(Error::AxCode(err));
    }
    if v.is_null() {
        return Ok(None);
    }
    // SAFETY: `v` is either kCFBooleanTrue/False or a CFBoolean* returned by AX and
    // owned by us; we must release it after reading.
    let b = unsafe { CFBooleanGetValue(v) };
    unsafe { CFRelease(v) };
    Ok(Some(b))
}

pub fn ax_set_bool(element: *mut c_void, attr: CFStringRef, value: bool) -> Result<()> {
    assert_main_thread_debug();
    // SAFETY: kCFBooleanTrue/False are immortal singletons; casting to CFTypeRef is fine.
    let val = unsafe {
        (if value {
            kCFBooleanTrue
        } else {
            kCFBooleanFalse
        }) as CFTypeRef
    };
    // SAFETY: Valid AX element and attribute; CFBoolean* is non-null.
    let err = unsafe { AXUIElementSetAttributeValue(element, attr, val) };
    if err != 0 {
        return Err(Error::AxCode(err));
    }
    Ok(())
}

/// A lightweight snapshot of key AX properties/capabilities for a window.
#[derive(Debug, Clone)]
pub struct AxProps {
    /// Accessibility role (e.g., "AXWindow", "AXSheet").
    pub role: Option<String>,
    /// Accessibility subrole (e.g., "AXStandardWindow", "AXDialog").
    pub subrole: Option<String>,
    /// Whether `AXPosition` appears settable for this window (cached).
    pub can_set_pos: Option<bool>,
    /// Whether `AXSize` appears settable for this window (cached).
    pub can_set_size: Option<bool>,
    /// Reported AX frame `(x,y,w,h)` in global coordinates when available.
    pub frame: Option<crate::geom::Rect>,
    /// Current `AXMinimized` state when exposed by the window.
    pub minimized: Option<bool>,
    /// Current `AXFullScreen` state when exposed by the window.
    pub fullscreen: Option<bool>,
    /// Current `AXVisible` state when exposed by the window.
    pub visible: Option<bool>,
    /// Current `AXZoomed` state when exposed by the window.
    pub zoomed: Option<bool>,
}

/// Resolve `AxProps` for a CoreGraphics `WindowId` (kCGWindowNumber).
/// Uses the per-window AXIsAttributeSettable cache for `AXPosition`/`AXSize`.
pub fn ax_props_for_window_id(id: WindowId) -> Result<AxProps> {
    let (win, _pid) = ax_window_for_id(id)?;
    let role = ax_get_string(win.as_ptr(), cfstr("AXRole"));
    let subrole = ax_get_string(win.as_ptr(), cfstr("AXSubrole"));
    let (can_set_pos, can_set_size) = ax_settable_pos_size(win.as_ptr());

    let frame = match (
        ax_get_point(win.as_ptr(), cfstr("AXPosition")),
        ax_get_size(win.as_ptr(), cfstr("AXSize")),
    ) {
        (Ok(pos), Ok(size)) => Some(crate::geom::Rect {
            x: pos.x,
            y: pos.y,
            w: size.width,
            h: size.height,
        }),
        _ => None,
    };

    let minimized = ax_bool(win.as_ptr(), cfstr("AXMinimized")).ok().flatten();
    let fullscreen = ax_bool(win.as_ptr(), cfstr("AXFullScreen")).ok().flatten();
    let visible = ax_bool(win.as_ptr(), cfstr("AXVisible")).ok().flatten();
    let zoomed = ax_bool(win.as_ptr(), cfstr("AXZoomed")).ok().flatten();

    Ok(AxProps {
        role,
        subrole,
        can_set_pos,
        can_set_size,
        frame,
        minimized,
        fullscreen,
        visible,
        zoomed,
    })
}

pub fn ax_get_point(element: *mut c_void, attr: CFStringRef) -> Result<Point> {
    let mut v: CFTypeRef = ptr::null_mut();
    // SAFETY: See note in `ax_bool` for out‑param contract.
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut v) };
    if err != 0 {
        if err == K_AX_ERROR_INVALID_UI_ELEMENT {
            debug!(
                "AXUIElementCopyAttributeValue(CGPoint) -> {} ({})",
                ax_error_name(err),
                err
            );
            return Err(Error::WindowGone);
        }
        debug!(
            "AXUIElementCopyAttributeValue(CGPoint) failed: code={} ({})",
            err,
            ax_error_name(err)
        );
        return Err(Error::AxCode(err));
    }
    if v.is_null() {
        return Err(Error::Unsupported);
    }
    let mut p = Point { x: 0.0, y: 0.0 };
    // SAFETY: `v` is an AXValue for CGPoint; out‑ptr is properly aligned and valid.
    let ok =
        unsafe { AXValueGetValue(v, K_AX_VALUE_CGPOINT_TYPE, &mut p as *mut _ as *mut c_void) };
    // SAFETY: We own `v` via Create rule.
    unsafe { CFRelease(v) };
    if !ok {
        debug!("AXValueGetValue(CGPoint) returned false (unsupported type)");
        return Err(Error::Unsupported);
    }
    Ok(p)
}

pub fn ax_get_size(element: *mut c_void, attr: CFStringRef) -> Result<Size> {
    let mut v: CFTypeRef = ptr::null_mut();
    // SAFETY: See note in `ax_bool` for out‑param contract.
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut v) };
    if err != 0 {
        if err == K_AX_ERROR_INVALID_UI_ELEMENT {
            debug!(
                "AXUIElementCopyAttributeValue(CGSize) -> {} ({})",
                ax_error_name(err),
                err
            );
            return Err(Error::WindowGone);
        }
        debug!(
            "AXUIElementCopyAttributeValue(CGSize) failed: code={} ({})",
            err,
            ax_error_name(err)
        );
        return Err(Error::AxCode(err));
    }
    if v.is_null() {
        return Err(Error::Unsupported);
    }
    let mut s = Size {
        width: 0.0,
        height: 0.0,
    };
    // SAFETY: `v` is an AXValue for CGSize; out‑ptr is properly aligned and valid.
    let ok = unsafe { AXValueGetValue(v, K_AX_VALUE_CGSIZE_TYPE, &mut s as *mut _ as *mut c_void) };
    // SAFETY: We own `v` via Create rule.
    unsafe { CFRelease(v) };
    if !ok {
        debug!("AXValueGetValue(CGSize) returned false (unsupported type)");
        return Err(Error::Unsupported);
    }
    Ok(s)
}

pub fn ax_get_string(element: *mut c_void, attr: CFStringRef) -> Option<String> {
    let mut v: CFTypeRef = ptr::null_mut();
    // SAFETY: See note in `ax_bool` for out‑param contract.
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut v) };
    if err != 0 || v.is_null() {
        return None;
    }
    // SAFETY: AX returned a CFString under the Create rule; wrap to transfer ownership.
    let s = unsafe { CFString::wrap_under_create_rule(v as _) };
    Some(s.to_string())
}

pub fn ax_set_point(element: *mut c_void, attr: CFStringRef, p: Point) -> Result<()> {
    assert_main_thread_debug();
    // SAFETY: `&p` points to a valid CGPoint; AXValueCreate copies the bytes.
    let v = unsafe { AXValueCreate(K_AX_VALUE_CGPOINT_TYPE, &p as *const _ as *const c_void) };
    if v.is_null() {
        return Err(Error::Unsupported);
    }
    // Optionally surface settable state in debug builds.
    if enabled!(Level::DEBUG) {
        match ax_is_attribute_settable_cached(element, attr) {
            Some(settable) => debug!("AXIsAttributeSettable(CGPoint) -> settable={}", settable),
            None => debug!("AXIsAttributeSettable(CGPoint) -> unknown (attr)"),
        }
    }
    // SAFETY: Valid element, attribute, and AXValue*; we own `v` and must release.
    let err = unsafe { AXUIElementSetAttributeValue(element, attr, v) };
    unsafe { CFRelease(v) };
    if err != 0 {
        debug!(
            "AXUIElementSetAttributeValue(CGPoint) failed: code={} ({})",
            err,
            ax_error_name(err)
        );
        if enabled!(Level::DEBUG) {
            let role = ax_get_string(element, cfstr("AXRole")).unwrap_or_default();
            let subrole = ax_get_string(element, cfstr("AXSubrole")).unwrap_or_default();
            let title = ax_get_string(element, cfstr("AXTitle")).unwrap_or_default();
            debug!(
                "AX context: role='{}' subrole='{}' title='{}' for CGPoint set",
                role, subrole, title
            );
        }
        return Err(Error::AxCode(err));
    }
    Ok(())
}

pub fn ax_set_size(element: *mut c_void, attr: CFStringRef, s: Size) -> Result<()> {
    assert_main_thread_debug();
    // SAFETY: `&s` points to a valid CGSize; AXValueCreate copies the bytes.
    let v = unsafe { AXValueCreate(K_AX_VALUE_CGSIZE_TYPE, &s as *const _ as *const c_void) };
    if v.is_null() {
        return Err(Error::Unsupported);
    }
    // Optionally surface settable state in debug builds.
    if enabled!(Level::DEBUG) {
        match ax_is_attribute_settable_cached(element, attr) {
            Some(settable) => debug!("AXIsAttributeSettable(CGSize) -> settable={}", settable),
            None => debug!("AXIsAttributeSettable(CGSize) -> unknown (attr)"),
        }
    }
    // SAFETY: Valid element, attribute, and AXValue*; we own `v` and must release.
    let err = unsafe { AXUIElementSetAttributeValue(element, attr, v) };
    unsafe { CFRelease(v) };
    if err != 0 {
        debug!(
            "AXUIElementSetAttributeValue(CGSize) failed: code={} ({})",
            err,
            ax_error_name(err)
        );
        if enabled!(Level::DEBUG) {
            let role = ax_get_string(element, cfstr("AXRole")).unwrap_or_default();
            let subrole = ax_get_string(element, cfstr("AXSubrole")).unwrap_or_default();
            let title = ax_get_string(element, cfstr("AXTitle")).unwrap_or_default();
            debug!(
                "AX context: role='{}' subrole='{}' title='{}' for CGSize set",
                role, subrole, title
            );
        }
        return Err(Error::AxCode(err));
    }
    Ok(())
}

/// Perform an AX action on an element, with a debug-only main-thread assertion.
pub fn ax_perform_action(element: *mut c_void, action: CFStringRef) -> Result<()> {
    assert_main_thread_debug();
    unsafe extern "C" {
        fn AXUIElementPerformAction(element: *mut c_void, action: CFStringRef) -> i32;
    }
    let err = unsafe { AXUIElementPerformAction(element, action) };
    if err != 0 {
        return Err(Error::AxCode(err));
    }
    Ok(())
}

/// Resolve owning pid for an AX element using AXUIElementGetPid.
pub fn ax_element_pid(element: *mut c_void) -> Option<i32> {
    let mut pid = -1;
    let err = unsafe { AXUIElementGetPid(element, &mut pid as *mut i32) };
    if err == 0 && pid > 0 { Some(pid) } else { None }
}

/// Resolve bundle identifier for a pid.
pub fn bundle_id_for_pid(pid: i32) -> Option<String> {
    unsafe {
        if let Some(app) =
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid as libc::pid_t)
        {
            if let Some(bid) = app.bundleIdentifier() {
                let c = bid.UTF8String();
                if !c.is_null() {
                    return Some(CStr::from_ptr(c).to_string_lossy().into_owned());
                }
            }
            if let Some(name) = app.localizedName() {
                let c = name.UTF8String();
                if !c.is_null() {
                    return Some(CStr::from_ptr(c).to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

// One-shot per-bundle warnings for non-settable attributes (AXPosition/AXSize)
thread_local! {
    static WARNED_BUNDLES: RefCell<HashMap<String, u8>> = RefCell::new(HashMap::new());
}

/// Log a warning once per bundle for non-settable AX attributes.
/// `can_pos`/`can_size`: Some(false) indicates the attribute is not settable; others are ignored.
pub fn warn_once_nonsettable(pid: i32, can_pos: Option<bool>, can_size: Option<bool>) {
    let mut bits: u8 = 0;
    if can_pos == Some(false) {
        bits |= 0b01;
    }
    if can_size == Some(false) {
        bits |= 0b10;
    }
    if bits == 0 {
        return;
    }
    let key = bundle_id_for_pid(pid).unwrap_or_else(|| format!("pid:{}", pid));
    WARNED_BUNDLES.with(|cell| {
        let mut m = cell.borrow_mut();
        let prev = m.get(&key).copied().unwrap_or(0);
        let new_bits = bits & !prev;
        if new_bits != 0 {
            let s_pos = match can_pos {
                Some(false) => "false",
                Some(true) => "true",
                None => "unknown",
            };
            let s_size = match can_size {
                Some(false) => "false",
                Some(true) => "true",
                None => "unknown",
            };
            info!(
                "AX non-settable attributes for {}: AXPosition={} AXSize={}",
                key, s_pos, s_size
            );
            m.insert(key, prev | new_bits);
        }
    });
}

/// Resolve an AX window element for a given CG `WindowId`. Returns the AX element and owning PID.
pub(crate) fn ax_window_for_id(id: WindowId) -> Result<(AXElem, i32)> {
    // Look up pid via CG, then match AXWindowNumber.
    let info = window::list_windows()
        .into_iter()
        .find(|w| w.id == id)
        .ok_or(Error::FocusedWindow)?;
    let pid = info.pid;
    let Some(app) = (unsafe { crate::AXElem::from_create(AXUIElementCreateApplication(pid)) })
    else {
        return Err(Error::AppElement);
    };
    let mut wins_ref: CFTypeRef = std::ptr::null_mut();
    // SAFETY: `app` is valid; out‑param receives a retained array.
    let err =
        unsafe { AXUIElementCopyAttributeValue(app.as_ptr(), cfstr("AXWindows"), &mut wins_ref) };
    if err != 0 || wins_ref.is_null() {
        return Err(Error::AxCode(err));
    }
    // SAFETY: Wrap ownership of returned CFArray.
    let arr = unsafe {
        core_foundation::array::CFArray::<*const c_void>::wrap_under_create_rule(wins_ref as _)
    };
    let mut found: *mut c_void = std::ptr::null_mut();
    let mut fallback_first_window: *mut c_void = std::ptr::null_mut();
    // SAFETY: Bounds checked by loop range.
    for i in 0..unsafe { core_foundation::array::CFArrayGetCount(arr.as_concrete_TypeRef()) } {
        // SAFETY: Index < len; returns borrowed pointer.
        let wref =
            unsafe { core_foundation::array::CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) }
                as *mut c_void;
        if wref.is_null() {
            continue;
        }
        // Remember the first top-level AXWindow as a fallback when id lookup fails
        if fallback_first_window.is_null() {
            let role = ax_get_string(wref, cfstr("AXRole")).unwrap_or_default();
            if role == "AXWindow" {
                fallback_first_window = wref;
            }
        }
        // Prefer private API for id resolution
        if let Some(pid_id) = crate::ax_private::window_id_for_ax_element(wref) {
            if pid_id == id {
                found = wref;
                break;
            }
        } else {
            // Fallback to AXWindowNumber attribute
            let mut num_ref: CFTypeRef = std::ptr::null_mut();
            let nerr = unsafe {
                AXUIElementCopyAttributeValue(wref, cfstr("AXWindowNumber"), &mut num_ref)
            };
            if nerr == 0 && !num_ref.is_null() {
                let cfnum = unsafe {
                    core_foundation::number::CFNumber::wrap_under_create_rule(num_ref as _)
                };
                let wid = cfnum.to_i64().unwrap_or(0) as u32;
                if wid == id {
                    found = wref;
                    break;
                }
            }
        }
    }
    if found.is_null() {
        if !fallback_first_window.is_null() {
            let elem =
                AXElem::retain_from_borrowed(fallback_first_window).ok_or(Error::FocusedWindow)?;
            return Ok((elem, pid));
        }
        return Err(Error::FocusedWindow);
    }
    let elem = AXElem::retain_from_borrowed(found).ok_or(Error::FocusedWindow)?;
    Ok((elem, pid))
}
/// Get the position of a window via Accessibility API.
/// Returns None if the window is not found or permission is denied.
pub fn ax_window_position(pid: i32, title: &str) -> Option<(f64, f64)> {
    let window = ax_find_window_by_title(pid, title)?;

    ax_get_point(window.as_ptr(), cfstr("AXPosition"))
        .ok()
        .map(|pos| (pos.x, pos.y))
}

/// Get the size of a window via Accessibility API.
/// Returns None if the window is not found or permission is denied.
pub fn ax_window_size(pid: i32, title: &str) -> Option<(f64, f64)> {
    let window = ax_find_window_by_title(pid, title)?;

    ax_get_size(window.as_ptr(), cfstr("AXSize"))
        .ok()
        .map(|s| (s.width, s.height))
}

/// Set a boolean AX attribute on the window identified by `(pid, title)`.
///
/// Returns `Ok(())` if the attribute could be set, or an `Error` if the
/// application/window could not be resolved or AX returned a failure code.
pub fn ax_set_bool_by_title(
    pid: i32,
    title: &str,
    attr_name: &'static str,
    value: bool,
) -> Result<()> {
    let Some(win) = ax_find_window_by_title(pid, title) else {
        return Err(Error::FocusedWindow);
    };
    ax_set_bool(win.as_ptr(), cfstr(attr_name), value)
}

/// Get a boolean AX attribute from the window identified by `(pid, title)`.
///
/// Returns `Ok(Some(v))` when present, `Ok(None)` when the window/attribute
/// is not available, or an error for AX failures.
pub fn ax_get_bool_by_title(
    pid: i32,
    title: &str,
    attr_name: &'static str,
) -> Result<Option<bool>> {
    let Some(win) = ax_find_window_by_title(pid, title) else {
        return Ok(None);
    };
    ax_bool(win.as_ptr(), cfstr(attr_name))
}

/// Convenience: set the minimized state on a window by `(pid, title)`.
pub fn ax_set_window_minimized(pid: i32, title: &str, minimized: bool) -> Result<()> {
    ax_set_bool_by_title(pid, title, "AXMinimized", minimized)
}

/// Convenience: query the minimized state on a window by `(pid, title)`.
pub fn ax_is_window_minimized(pid: i32, title: &str) -> Result<Option<bool>> {
    ax_get_bool_by_title(pid, title, "AXMinimized")
}

/// Convenience: set the zoomed state on a window by `(pid, title)`.
pub fn ax_set_window_zoomed(pid: i32, title: &str, zoomed: bool) -> Result<()> {
    ax_set_bool_by_title(pid, title, "AXZoomed", zoomed)
}

/// Convenience: query the zoomed state on a window by `(pid, title)`.
pub fn ax_is_window_zoomed(pid: i32, title: &str) -> Result<Option<bool>> {
    ax_get_bool_by_title(pid, title, "AXZoomed")
}

/// Get the frame (position and size) of a window via Accessibility API.
/// Returns None if the window is not found or permission is denied.
pub fn ax_window_frame(pid: i32, title: &str) -> Option<((f64, f64), (f64, f64))> {
    let window = ax_find_window_by_title(pid, title)?;

    match (
        ax_get_point(window.as_ptr(), cfstr("AXPosition")).ok(),
        ax_get_size(window.as_ptr(), cfstr("AXSize")).ok(),
    ) {
        (Some(pos), Some(size)) => Some(((pos.x, pos.y), (size.width, size.height))),
        _ => None,
    }
}

/// Find a window by title using Accessibility API.
/// Returns the AXUIElement pointer for the window, or None if not found.
pub(crate) fn ax_find_window_by_title(pid: i32, title: &str) -> Option<AXElem> {
    let app = (unsafe { crate::AXElem::from_create(AXUIElementCreateApplication(pid)) })?;

    let mut wins_ref: CFTypeRef = ptr::null_mut();
    // SAFETY: `app` is valid; out‑param receives a retained array.
    let err =
        unsafe { AXUIElementCopyAttributeValue(app.as_ptr(), cfstr("AXWindows"), &mut wins_ref) };

    if err != 0 || wins_ref.is_null() {
        return None;
    }

    // SAFETY: Wrap ownership of returned CFArray.
    let arr = unsafe {
        core_foundation::array::CFArray::<*const c_void>::wrap_under_create_rule(wins_ref as _)
    };

    // SAFETY: Bounds checked by range.
    for i in 0..unsafe { core_foundation::array::CFArrayGetCount(arr.as_concrete_TypeRef()) } {
        // SAFETY: Index < len; returns borrowed pointer.
        let wref =
            unsafe { core_foundation::array::CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) };
        let w = wref as *mut c_void;
        if w.is_null() {
            continue;
        }

        let mut t_ref: CFTypeRef = ptr::null_mut();
        // SAFETY: Valid element; out‑param receives retained CFString title.
        let terr = unsafe { AXUIElementCopyAttributeValue(w, cfstr("AXTitle"), &mut t_ref) };
        if terr != 0 || t_ref.is_null() {
            continue;
        }

        // SAFETY: Title CFString returned under Create rule; wrap to transfer ownership.
        let cfs = unsafe { CFString::wrap_under_create_rule(t_ref as CFStringRef) };
        let t = cfs.to_string();
        if t == title {
            // Retain the AX window element so it remains valid after `arr` is released.
            if let Some(elem) = AXElem::retain_from_borrowed(w) {
                return Some(elem);
            }
        }
    }
    None
}

// Determine whether the given attribute is one we cache (AXPosition/AXSize).
#[inline]
fn cached_attr_kind(attr: CFStringRef) -> Option<usize> {
    // Index 0 => AXPosition, 1 => AXSize
    let pos = cfstr("AXPosition");
    if attr == pos {
        return Some(0);
    }
    let size = cfstr("AXSize");
    if attr == size {
        return Some(1);
    }
    None
}

/// Query AXIsAttributeSettable with a per-window cache for AXPosition/AXSize.
/// Returns Some(true/false) when the attribute is one of the cached kinds and
/// the query succeeded; returns None when the attribute is not cached or when
/// the system query failed.
fn ax_is_attribute_settable_cached(element: *mut c_void, attr: CFStringRef) -> Option<bool> {
    let kind = cached_attr_kind(attr)?;
    let key = element as usize;
    // Fast path: return cached value if present
    if let Some(hit) = SETTABLE_CACHE.with(|cell| {
        let m = cell.borrow();
        m.get(&key).and_then(|(p, s)| match kind {
            0 => *p,
            _ => *s,
        })
    }) {
        return Some(hit);
    }
    // Miss: query AX and populate cache slot
    let mut settable = false;
    let serr = unsafe { AXUIElementIsAttributeSettable(element, attr, &mut settable) };
    if serr != 0 {
        // Leave as None so callers can decide behavior; we still insert an entry
        // with the missing slot untouched so a subsequent successful query can fill it.
        SETTABLE_CACHE.with(|cell| {
            let mut m = cell.borrow_mut();
            let entry = m.entry(key).or_insert((None, None));
            match kind {
                0 => {
                    // leave position as None
                    let _ = entry;
                }
                _ => {
                    // leave size as None
                    let _ = entry;
                }
            }
        });
        return None;
    }
    SETTABLE_CACHE.with(|cell| {
        let mut m = cell.borrow_mut();
        let entry = m.entry(key).or_insert((None, None));
        match kind {
            0 => entry.0 = Some(settable),
            _ => entry.1 = Some(settable),
        }
    });
    Some(settable)
}

/// Return cached or freshly-queried settable flags for AXPosition and AXSize.
/// Values are returned as `(can_set_pos, can_set_size)`, where each item is
/// `Some(true|false)` when known, or `None` if the attribute is unsupported or
/// the underlying query failed. Results are cached per AX element.
pub fn ax_settable_pos_size(element: *mut c_void) -> (Option<bool>, Option<bool>) {
    let pos = ax_is_attribute_settable_cached(element, cfstr("AXPosition"));
    let size = ax_is_attribute_settable_cached(element, cfstr("AXSize"));
    (pos, size)
}

/// Clear cached AXIsAttributeSettable results for a given AX window element.
/// This should be called after state changes like toggling `AXZoomed`/`AXMinimized`
/// that can affect whether `AXPosition`/`AXSize` are settable.
pub fn ax_clear_settable_cache(element: *mut c_void) {
    let key = element as usize;
    SETTABLE_CACHE.with(|cell| {
        cell.borrow_mut().remove(&key);
    });
}
