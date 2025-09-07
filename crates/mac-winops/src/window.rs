use std::ffi::c_void;

use core_foundation::{
    array::{CFArray, CFArrayGetCount, CFArrayGetValueAtIndex},
    base::{CFTypeRef, TCFType},
    dictionary::CFDictionaryRef,
    string::CFString,
};
use core_graphics::window as cgw;
use tracing::{trace, warn};

use crate::WindowId;
use crate::cfutil::{dict_get_i32, dict_get_rect_i32, dict_get_string};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relativeToWindow: u32) -> CFTypeRef; // CFArrayRef
}

const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
const K_CG_WINDOW_LIST_OPTION_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub app: String,
    pub title: String,
    pub pid: i32,
    pub id: WindowId,
    pub pos: Option<Pos>,
    pub space: Option<i32>,
    /// True for the globally frontmost on-screen layer-0 window.
    pub focused: bool,
}

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
        // PID of the globally frontmost layer-0 window (first encountered)
        let mut frontmost_pid: Option<i32> = None;
        let mut focused_marked = false;
        let key_pid = cgw::kCGWindowOwnerPID;
        let _key_layer = cgw::kCGWindowLayer;
        let key_num = cgw::kCGWindowNumber;
        let key_app = cgw::kCGWindowOwnerName;
        let key_title = cgw::kCGWindowName;
        let key_bounds = cgw::kCGWindowBounds; // optional
        let key_workspace = CFString::from_static_string("kCGWindowWorkspace"); // optional
        #[allow(non_snake_case)]
        unsafe extern "C" {
            fn CFGetTypeID(cf: CFTypeRef) -> u64;
            fn CFDictionaryGetTypeID() -> u64;
        }
        for i in 0..CFArrayGetCount(arr.as_concrete_TypeRef()) {
            let item = CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) as CFTypeRef;
            if item.is_null() || CFGetTypeID(item) != CFDictionaryGetTypeID() {
                continue;
            }
            let d = item as CFDictionaryRef;
            // Include all layers; some app windows (HUD/notifications) use non-zero layers.
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
            let pos = dict_get_rect_i32(d, key_bounds).map(|(x, y, w, h)| Pos {
                x,
                y,
                width: w,
                height: h,
            });
            let space = dict_get_i32(d, key_workspace.as_concrete_TypeRef());
            if frontmost_pid.is_none() {
                frontmost_pid = Some(pid);
            }
            let focused = if !focused_marked && frontmost_pid == Some(pid) {
                focused_marked = true;
                true
            } else {
                false
            };
            out.push(WindowInfo {
                app,
                title,
                pid,
                id,
                pos,
                space,
                focused,
            });
        }
    }
    out
}

/// Convenience: return the frontmost on-screen window, if any.
pub fn frontmost_window() -> Option<WindowInfo> {
    list_windows().into_iter().next()
}

/// Convenience: return the frontmost on-screen window owned by `pid`, if any.
pub fn frontmost_window_for_pid(pid: i32) -> Option<WindowInfo> {
    list_windows().into_iter().find(|w| w.pid == pid)
}
