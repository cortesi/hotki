use std::ffi::c_void;

use core_foundation::{
    array::{CFArray, CFArrayGetCount, CFArrayGetValueAtIndex},
    base::{CFTypeRef, TCFType},
    dictionary::CFDictionaryRef,
    string::CFString,
};
use core_graphics::window as cgw;
use tracing::{trace, warn};

use crate::{
    WindowId,
    cfutil::{
        dict_get_bool, dict_get_dict, dict_get_i32, dict_get_i64, dict_get_rect_i32,
        dict_get_string,
    },
};

#[allow(non_snake_case)]
unsafe extern "C" {
    fn CFGetTypeID(cf: CFTypeRef) -> u64;
    fn CFDictionaryGetTypeID() -> u64;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relativeToWindow: u32) -> CFTypeRef; // CFArrayRef
}

#[link(name = "SkyLight", kind = "framework")]
unsafe extern "C" {
    fn CGSMainConnectionID() -> i32;
    fn CGSCopyManagedDisplaySpaces(connection: i32) -> CFTypeRef; // CFArrayRef
}

const K_CG_WINDOW_LIST_OPTION_ALL: u32 = 0;
const K_CG_WINDOW_LIST_OPTION_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;

/// Identifier for a Mission Control space.
pub type SpaceId = i64;

/// Return the identifiers for the active Mission Control spaces (one per display).
pub fn active_space_ids() -> Vec<SpaceId> {
    trace!("active_space_ids");
    let mut out = Vec::new();
    unsafe {
        let conn = CGSMainConnectionID();
        let spaces_ref = CGSCopyManagedDisplaySpaces(conn);
        if spaces_ref.is_null() {
            warn!("active_space_ids: CGSCopyManagedDisplaySpaces returned null");
            return out;
        }
        let arr: CFArray<*const c_void> = CFArray::wrap_under_create_rule(spaces_ref as _);
        let key_current = CFString::from_static_string("Current Space");
        let key_id = CFString::from_static_string("ManagedSpaceID");
        for i in 0..CFArrayGetCount(arr.as_concrete_TypeRef()) {
            let item = CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) as CFTypeRef;
            if item.is_null() || CFGetTypeID(item) != CFDictionaryGetTypeID() {
                continue;
            }
            let d = item as CFDictionaryRef;
            if let Some(current) = dict_get_dict(d, key_current.as_concrete_TypeRef())
                && let Some(id) = dict_get_i64(current, key_id.as_concrete_TypeRef())
                && !out.contains(&id)
            {
                out.push(id);
            }
        }
    }
    trace!(?out, "active_space_ids_result");
    out
}

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
    pub space: Option<SpaceId>,
    /// CoreGraphics window layer (0 = standard app windows)
    pub layer: i32,
    /// True for the globally frontmost on-screen layer-0 window.
    pub focused: bool,
    /// True if CoreGraphics reports the window as currently on-screen.
    pub is_on_screen: bool,
    /// True if the window belongs to one of the active Mission Control spaces.
    pub on_active_space: bool,
}

pub fn list_windows() -> Vec<WindowInfo> {
    trace!("list_windows_active");
    let active_spaces = active_space_ids();
    list_windows_filtered(Some(&active_spaces), &active_spaces)
}

/// Enumerate windows limited to a specific set of Mission Control spaces.
///
/// Passing an empty `spaces` slice returns windows for all spaces (including
/// sticky/system windows). Sticky windows (negative space ids) and windows with
/// no reported space are always included, as they appear on every space.
pub fn list_windows_for_spaces(spaces: &[SpaceId]) -> Vec<WindowInfo> {
    trace!(?spaces, "list_windows_for_spaces");
    let active_spaces = active_space_ids();
    list_windows_filtered(Some(spaces), &active_spaces)
}

fn list_windows_filtered(
    filter_spaces: Option<&[SpaceId]>,
    active_spaces: &[SpaceId],
) -> Vec<WindowInfo> {
    let mut out = Vec::new();
    trace!(active_spaces = ?active_spaces, filter = ?filter_spaces, "list_windows_filtered_start");
    unsafe {
        let arr_ref = CGWindowListCopyWindowInfo(
            K_CG_WINDOW_LIST_OPTION_ALL | K_CG_WINDOW_LIST_OPTION_EXCLUDE_DESKTOP_ELEMENTS,
            0,
        );
        if arr_ref.is_null() {
            warn!("list_windows: CGWindowListCopyWindowInfo returned null");
            return out;
        }
        let arr: CFArray<*const c_void> = CFArray::wrap_under_create_rule(arr_ref as _);
        let mut frontmost_marked = false;
        let key_pid = cgw::kCGWindowOwnerPID;
        let key_layer = cgw::kCGWindowLayer;
        let key_num = cgw::kCGWindowNumber;
        let key_app = cgw::kCGWindowOwnerName;
        let key_title = cgw::kCGWindowName;
        let key_bounds = cgw::kCGWindowBounds; // optional
        let key_workspace = CFString::from_static_string("kCGWindowWorkspace"); // optional
        let key_is_onscreen = CFString::from_static_string("kCGWindowIsOnscreen");

        for i in 0..CFArrayGetCount(arr.as_concrete_TypeRef()) {
            let item = CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) as CFTypeRef;
            if item.is_null() || CFGetTypeID(item) != CFDictionaryGetTypeID() {
                continue;
            }
            let d = item as CFDictionaryRef;
            let pid = match dict_get_i32(d, key_pid) {
                Some(p) => p,
                None => continue,
            };
            let id = match dict_get_i32(d, key_num) {
                Some(n) if n > 0 => n as u32,
                _ => continue,
            };
            let space = dict_get_i64(d, key_workspace.as_concrete_TypeRef());
            let is_on_screen =
                dict_get_bool(d, key_is_onscreen.as_concrete_TypeRef()).unwrap_or(false);
            let on_active_space = match space {
                Some(id) if id >= 0 => active_spaces.contains(&id),
                Some(_) => true, // negative ids mean all spaces / sticky
                None => is_on_screen,
            };

            if !should_include_window(space, filter_spaces, on_active_space) {
                continue;
            }

            let app = dict_get_string(d, key_app).unwrap_or_default();
            let title = dict_get_string(d, key_title).unwrap_or_default();
            let pos = dict_get_rect_i32(d, key_bounds).map(|(x, y, w, h)| Pos {
                x,
                y,
                width: w,
                height: h,
            });
            let layer = dict_get_i32(d, key_layer).unwrap_or(0);
            let focused = if !frontmost_marked && layer == 0 && is_on_screen {
                frontmost_marked = true;
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
                layer,
                focused,
                is_on_screen,
                on_active_space,
            });
        }
    }
    out
}

fn should_include_window(
    space: Option<SpaceId>,
    filter_spaces: Option<&[SpaceId]>,
    on_active_space: bool,
) -> bool {
    match filter_spaces {
        None => on_active_space,
        Some([]) => true,
        Some(spaces) => match space {
            Some(id) if id >= 0 => spaces.contains(&id),
            Some(_) => true,
            None => true,
        },
    }
}

/// Convenience: return the frontmost on-screen window, if any.
pub fn frontmost_window() -> Option<WindowInfo> {
    let mut first_on_screen = None;
    let mut first_any = None;
    for w in list_windows().into_iter() {
        if crate::FOCUS_SKIP_APPS.iter().any(|s| *s == w.app) {
            if first_any.is_none() {
                first_any = Some(w.clone());
            }
            if first_on_screen.is_none() && w.is_on_screen {
                first_on_screen = Some(w.clone());
            }
            continue;
        }
        if w.layer == 0 && w.is_on_screen && w.on_active_space {
            return Some(w);
        }
        if first_on_screen.is_none() && w.is_on_screen {
            first_on_screen = Some(w.clone());
        }
        if first_any.is_none() {
            first_any = Some(w);
        }
    }
    first_on_screen.or(first_any)
}

/// Convenience: return the frontmost on-screen window owned by `pid`, if any.
pub fn frontmost_window_for_pid(pid: i32) -> Option<WindowInfo> {
    let mut first_on_active = None;
    let mut first_on_screen = None;
    let mut first_any = None;
    for w in list_windows().into_iter() {
        if w.pid != pid {
            continue;
        }
        if first_any.is_none() {
            first_any = Some(w.clone());
        }
        if w.layer == 0 && w.is_on_screen && w.on_active_space {
            return Some(w);
        }
        if first_on_active.is_none() && w.on_active_space {
            first_on_active = Some(w.clone());
        }
        if first_on_screen.is_none() && w.is_on_screen {
            first_on_screen = Some(w.clone());
        }
    }
    first_on_active.or(first_on_screen).or(first_any)
}
