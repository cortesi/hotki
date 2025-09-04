use core_foundation::{
    base::{CFRelease, CFTypeRef, TCFType},
    dictionary::CFDictionaryRef,
    number::CFNumberRef,
    string::{CFString, CFStringRef},
};
use core_graphics::window as cgw;

/// Query the frontmost app name, window title, and owner PID using CGWindowList.
pub(crate) fn front_app_title_pid() -> (String, String, i32) {
    unsafe {
        fn cfstring_to_string(s: CFStringRef) -> String {
            // SAFETY: CFStringRef obtained from system APIs per get rule
            let cf = unsafe { CFString::wrap_under_get_rule(s) };
            cf.to_string()
        }
        unsafe fn get_string(dict: CFDictionaryRef, key: CFStringRef) -> Option<String> {
            let value = unsafe {
                core_foundation::dictionary::CFDictionaryGetValue(
                    dict,
                    key as *const core::ffi::c_void,
                )
            };
            if value.is_null() {
                return None;
            }
            let sref = value as CFStringRef;
            Some(cfstring_to_string(sref))
        }
        unsafe fn get_number(dict: CFDictionaryRef, key: CFStringRef) -> Option<i32> {
            let value = unsafe {
                core_foundation::dictionary::CFDictionaryGetValue(
                    dict,
                    key as *const core::ffi::c_void,
                )
            };
            if value.is_null() {
                return None;
            }
            let nref = value as CFNumberRef;
            let mut out: i32 = 0;
            let ok = unsafe {
                core_foundation::number::CFNumberGetValue(
                    nref,
                    9,
                    &mut out as *mut i32 as *mut core::ffi::c_void,
                )
            };
            if ok { Some(out) } else { None }
        }

        let options: cgw::CGWindowListOption =
            cgw::kCGWindowListOptionOnScreenOnly | cgw::kCGWindowListOptionOnScreenBelowWindow;
        let arr = cgw::CGWindowListCopyWindowInfo(options, cgw::kCGNullWindowID);
        if arr.is_null() {
            return (String::new(), String::new(), -1);
        }
        let count = core_foundation::array::CFArrayGetCount(arr);
        let mut app = String::new();
        let mut title = String::new();
        let mut pid: i32 = -1;
        for i in 0..count {
            let item = core_foundation::array::CFArrayGetValueAtIndex(arr, i);
            if item.is_null() {
                continue;
            }
            let dict = item as CFDictionaryRef;
            if let Some(layer) = get_number(dict, cgw::kCGWindowLayer)
                && layer != 0
            {
                continue;
            }
            if app.is_empty() {
                if let Some(name) = get_string(dict, cgw::kCGWindowOwnerName) {
                    app = name;
                }
                if let Some(p) = get_number(dict, cgw::kCGWindowOwnerPID) {
                    pid = p;
                }
            }
            if title.is_empty()
                && let Some(name) = get_string(dict, cgw::kCGWindowName)
            {
                title = name;
            }
            if !app.is_empty() && !title.is_empty() && pid != -1 {
                break;
            }
        }
        CFRelease(arr as CFTypeRef);
        (app, title, pid)
    }
}
