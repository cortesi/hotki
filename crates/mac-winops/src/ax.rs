use std::{cell::RefCell, collections::HashMap, ffi::c_void, ptr, thread_local};

use core_foundation::{
    base::{CFRelease, CFTypeRef, TCFType},
    boolean::{kCFBooleanFalse, kCFBooleanTrue},
    string::{CFString, CFStringRef},
};

use crate::{
    AXElem, WindowId,
    error::{Error, Result},
    geom::{CGPoint, CGSize},
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

thread_local! {
    static ATTR_STRINGS: RefCell<HashMap<&'static str, CFString>> = RefCell::new(HashMap::new());
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

pub fn ax_bool(element: *mut c_void, attr: CFStringRef) -> Result<Option<bool>> {
    let mut v: CFTypeRef = ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut v) };
    if err != 0 {
        if err == K_AX_ERROR_INVALID_UI_ELEMENT {
            return Err(Error::WindowGone);
        }
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

pub fn ax_set_bool(element: *mut c_void, attr: CFStringRef, value: bool) -> Result<()> {
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

pub fn ax_get_point(element: *mut c_void, attr: CFStringRef) -> Result<CGPoint> {
    let mut v: CFTypeRef = ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut v) };
    if err != 0 {
        if err == K_AX_ERROR_INVALID_UI_ELEMENT {
            return Err(Error::WindowGone);
        }
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

pub fn ax_get_size(element: *mut c_void, attr: CFStringRef) -> Result<CGSize> {
    let mut v: CFTypeRef = ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut v) };
    if err != 0 {
        if err == K_AX_ERROR_INVALID_UI_ELEMENT {
            return Err(Error::WindowGone);
        }
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

pub fn ax_get_string(element: *mut c_void, attr: CFStringRef) -> Option<String> {
    let mut v: CFTypeRef = ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut v) };
    if err != 0 || v.is_null() {
        return None;
    }
    let s = unsafe { CFString::wrap_under_create_rule(v as _) };
    Some(s.to_string())
}

pub fn ax_set_point(element: *mut c_void, attr: CFStringRef, p: CGPoint) -> Result<()> {
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

pub fn ax_set_size(element: *mut c_void, attr: CFStringRef, s: CGSize) -> Result<()> {
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
    let err =
        unsafe { AXUIElementCopyAttributeValue(app.as_ptr(), cfstr("AXWindows"), &mut wins_ref) };
    if err != 0 || wins_ref.is_null() {
        return Err(Error::AxCode(err));
    }
    let arr = unsafe {
        core_foundation::array::CFArray::<*const c_void>::wrap_under_create_rule(wins_ref as _)
    };
    let mut found: *mut c_void = std::ptr::null_mut();
    let mut fallback_first_window: *mut c_void = std::ptr::null_mut();
    for i in 0..unsafe { core_foundation::array::CFArrayGetCount(arr.as_concrete_TypeRef()) } {
        let wref =
            unsafe { core_foundation::array::CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) }
                as *mut c_void;
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
        let mut num_ref: CFTypeRef = std::ptr::null_mut();
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
fn ax_find_window_by_title(pid: i32, title: &str) -> Option<AXElem> {
    let Some(app) = (unsafe { crate::AXElem::from_create(AXUIElementCreateApplication(pid)) })
    else {
        return None;
    };

    let mut wins_ref: CFTypeRef = ptr::null_mut();
    let err =
        unsafe { AXUIElementCopyAttributeValue(app.as_ptr(), cfstr("AXWindows"), &mut wins_ref) };

    if err != 0 || wins_ref.is_null() {
        return None;
    }

    let arr = unsafe {
        core_foundation::array::CFArray::<*const c_void>::wrap_under_create_rule(wins_ref as _)
    };

    for i in 0..unsafe { core_foundation::array::CFArrayGetCount(arr.as_concrete_TypeRef()) } {
        let wref =
            unsafe { core_foundation::array::CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) };
        let w = wref as *mut c_void;
        if w.is_null() {
            continue;
        }

        let mut t_ref: CFTypeRef = ptr::null_mut();
        let terr = unsafe { AXUIElementCopyAttributeValue(w, cfstr("AXTitle"), &mut t_ref) };
        if terr != 0 || t_ref.is_null() {
            continue;
        }

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
