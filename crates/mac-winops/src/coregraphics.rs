use crate::{Error, Result};
use core_foundation::array::{CFArrayGetCount, CFArrayGetValueAtIndex};
use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::dictionary::CFDictionaryRef;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::c_void;

/// Alias for CoreGraphics CGWindowID (kCGWindowNumber).
pub type WindowId = u32;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relativeToWindow: u32) -> CFTypeRef; // CFArrayRef
}

// CoreGraphics window list options used by our queries
const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
const K_CG_WINDOW_LIST_OPTION_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_BELOW_WINDOW: u32 = 1 << 1; // used in front_app_title_pid

fn cfstr(name: &'static str) -> CFStringRef {
    // This duplicates crate::cfstr signature locally; kept private here to
    // avoid exposing CFStringRef across modules.
    use std::ffi::{CString, c_char};
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

fn cg_key(s: &'static str) -> CFStringRef {
    cfstr(s)
}

const K_CG_WINDOW_NUMBER: &str = "kCGWindowNumber";
const K_CG_WINDOW_OWNER_PID: &str = "kCGWindowOwnerPID";
const K_CG_WINDOW_LAYER: &str = "kCGWindowLayer";
const K_CG_WINDOW_IS_ONSCREEN: &str = "kCGWindowIsOnscreen";
const K_CG_WINDOW_ALPHA: &str = "kCGWindowAlpha";
const K_CG_WINDOW_OWNER_NAME: &str = "kCGWindowOwnerName";
const K_CG_WINDOW_NAME: &str = "kCGWindowName";

fn dict_get_cfnumber(d: CFDictionaryRef, key: CFStringRef) -> Option<CFNumber> {
    unsafe {
        let v = core_foundation::dictionary::CFDictionaryGetValue(d, key as *const _);
        if v.is_null() {
            return None;
        }
        Some(CFNumber::wrap_under_get_rule(v as _))
    }
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFBooleanGetValue(b: CFTypeRef) -> bool;
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

/// Resolve the focused top-level window’s CGWindowID for `pid`.
///
/// Best-effort: picks the first frontmost layer-0 on-screen window owned by pid.
pub(crate) fn focused_window_id(pid: i32) -> Result<WindowId> {
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

/// Return the frontmost app name, window title and owner PID using CGWindowList.
pub(crate) fn front_app_title_pid() -> (String, String, i32) {
    unsafe {
        let arr_ref = CGWindowListCopyWindowInfo(
            K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY | K_CG_WINDOW_LIST_OPTION_ON_SCREEN_BELOW_WINDOW,
            0,
        );
        if arr_ref.is_null() {
            return (String::new(), String::new(), -1);
        }
        let arr: core_foundation::array::CFArray<*const c_void> =
            core_foundation::array::CFArray::wrap_under_create_rule(arr_ref as _);
        let key_owner_name = cg_key(K_CG_WINDOW_OWNER_NAME);
        let key_name = cg_key(K_CG_WINDOW_NAME);
        let key_pid = cg_key(K_CG_WINDOW_OWNER_PID);
        let key_layer = cg_key(K_CG_WINDOW_LAYER);

        let mut app = String::new();
        let mut title = String::new();
        let mut pid: i32 = -1;

        for i in 0..CFArrayGetCount(arr.as_concrete_TypeRef()) {
            let d = CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) as CFDictionaryRef;
            if d.is_null() {
                continue;
            }
            let layer = dict_get_cfnumber(d, key_layer)
                .and_then(|n| n.to_i64())
                .unwrap_or(0);
            if layer != 0 {
                continue;
            }

            if app.is_empty() {
                let v = core_foundation::dictionary::CFDictionaryGetValue(
                    d,
                    key_owner_name as *const _,
                );
                if !v.is_null() {
                    let s = CFString::wrap_under_get_rule(v as CFStringRef);
                    app = s.to_string();
                }
                if let Some(n) = dict_get_cfnumber(d, key_pid) {
                    pid = n.to_i64().unwrap_or(-1) as i32;
                }
            }
            if title.is_empty() {
                let v = core_foundation::dictionary::CFDictionaryGetValue(d, key_name as *const _);
                if !v.is_null() {
                    let s = CFString::wrap_under_get_rule(v as CFStringRef);
                    title = s.to_string();
                }
            }
            if !app.is_empty() && !title.is_empty() && pid != -1 {
                break;
            }
        }
        CFRelease(arr_ref as CFTypeRef);
        (app, title, pid)
    }
}
