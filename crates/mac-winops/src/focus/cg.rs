use crate::cfutil::{dict_get_i32, dict_get_string};
use core_foundation::{
    base::{CFRelease, CFTypeRef},
    dictionary::CFDictionaryRef,
};
use core_graphics::window as cgw;

/// Query the frontmost app name, window title, and owner PID using CGWindowList.
pub(crate) fn front_app_title_pid() -> (String, String, i32) {
    unsafe {
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
            if let Some(layer) = dict_get_i32(dict, cgw::kCGWindowLayer)
                && layer != 0
            {
                continue;
            }
            if app.is_empty() {
                if let Some(name) = dict_get_string(dict, cgw::kCGWindowOwnerName) {
                    app = name;
                }
                if let Some(p) = dict_get_i32(dict, cgw::kCGWindowOwnerPID) {
                    pid = p;
                }
            }
            if title.is_empty()
                && let Some(name) = dict_get_string(dict, cgw::kCGWindowName)
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
