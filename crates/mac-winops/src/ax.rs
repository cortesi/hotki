use std::{ffi::c_void, ptr};

use core_foundation::{
    base::{CFRelease, CFTypeRef, TCFType},
    boolean::{kCFBooleanFalse, kCFBooleanTrue},
    string::{CFString, CFStringRef},
};

use crate::error::{Error, Result};
use crate::geometry::{CGPoint, CGSize};

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

pub fn cfstr(name: &'static str) -> CFStringRef {
    // Use a non-owning CFString backed by a static &'static str; no release needed.
    CFString::from_static_string(name).as_concrete_TypeRef()
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
