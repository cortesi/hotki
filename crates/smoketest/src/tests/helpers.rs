//! Shared helpers for smoketests to keep tests concise.

use std::time::{Duration, Instant};

use core_foundation::base::TCFType;

use crate::{
    config,
    error::{Error, Result},
    process::{HelperWindowBuilder, ManagedChild},
};

/// Approximate float equality within `eps`.
pub fn approx(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

/// Wait until the frontmost CG window has the given title.
pub fn wait_for_frontmost_title(expected: &str, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if let Some(win) = mac_winops::frontmost_window()
            && win.title == expected
        {
            return true;
        }
        std::thread::sleep(config::ms(config::POLL_INTERVAL_MS));
    }
    false
}

/// Wait until a window with `(pid,title)` is visible via CG or AX.
pub fn wait_for_window_visible(pid: i32, title: &str, timeout_ms: u64, poll_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        let wins = mac_winops::list_windows();
        let cg_ok = wins.iter().any(|w| w.pid == pid && w.title == title);
        let ax_ok = mac_winops::ax_has_window_title(pid, title);
        if cg_ok || ax_ok {
            return true;
        }
        std::thread::sleep(config::ms(poll_ms));
    }
    false
}

/// Wait until all `(pid,title)` pairs are visible via CG or AX.
pub fn wait_for_windows_visible(entries: &[(i32, &str)], timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        let wins = mac_winops::list_windows();
        let all_found = entries.iter().all(|(pid, title)| {
            let cg_present = wins.iter().any(|w| w.pid == *pid && w.title == *title);
            let ax_present = mac_winops::ax_has_window_title(*pid, title);
            cg_present || ax_present
        });
        if all_found {
            return true;
        }
        std::thread::sleep(config::ms(config::POLL_INTERVAL_MS));
    }
    false
}

/// Best-effort: bring the given window to the front by raising it or activating its PID.
pub fn ensure_frontmost(pid: i32, title: &str, attempts: usize, delay_ms: u64) {
    for _ in 0..attempts {
        if let Some(w) = mac_winops::list_windows()
            .into_iter()
            .find(|w| w.pid == pid && w.title == title)
        {
            let _ = mac_winops::request_raise_window(pid, w.id);
        } else {
            let _ = mac_winops::request_activate_pid(pid);
        }
        std::thread::sleep(config::ms(delay_ms));
        if wait_for_frontmost_title(title, delay_ms) {
            break;
        }
    }
}

/// Spawn a helper window with `title`, keep it alive for `lifetime_ms`, and
/// block until it’s visible (or return an error).
pub fn spawn_helper_visible(
    title: String,
    lifetime_ms: u64,
    visible_timeout_ms: u64,
    poll_ms: u64,
    label_text: &str,
) -> Result<ManagedChild> {
    let helper = HelperWindowBuilder::new(title.clone())
        .with_time_ms(lifetime_ms)
        .with_label_text(label_text)
        .spawn()?;
    if !wait_for_window_visible(helper.pid, &title, visible_timeout_ms, poll_ms) {
        return Err(Error::FocusNotObserved {
            timeout_ms: visible_timeout_ms,
            expected: format!("helper window '{}' not visible", title),
        });
    }
    Ok(helper)
}

/// Variant allowing initial window state options.
pub fn spawn_helper_with_options(
    title: String,
    lifetime_ms: u64,
    visible_timeout_ms: u64,
    poll_ms: u64,
    label_text: &str,
    start_minimized: bool,
    start_zoomed: bool,
) -> Result<ManagedChild> {
    let helper = HelperWindowBuilder::new(title.clone())
        .with_time_ms(lifetime_ms)
        .with_label_text(label_text)
        .with_start_minimized(start_minimized)
        .with_start_zoomed(start_zoomed)
        .spawn()?;
    if !wait_for_window_visible(helper.pid, &title, visible_timeout_ms, poll_ms) {
        return Err(Error::FocusNotObserved {
            timeout_ms: visible_timeout_ms,
            expected: format!("helper window '{}' not visible", title),
        });
    }
    Ok(helper)
}

// === AX helpers (local to smoketest) ===

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> *mut core::ffi::c_void;
    fn AXUIElementCopyAttributeValue(
        element: *mut core::ffi::c_void,
        attr: core_foundation::string::CFStringRef,
        value: *mut core_foundation::base::CFTypeRef,
    ) -> i32;
    fn AXUIElementSetAttributeValue(
        element: *mut core::ffi::c_void,
        attr: core_foundation::string::CFStringRef,
        value: core_foundation::base::CFTypeRef,
    ) -> i32;
    fn CFBooleanGetValue(b: core_foundation::base::CFTypeRef) -> bool;
    fn CFRetain(cf: core_foundation::base::CFTypeRef) -> core_foundation::base::CFTypeRef;
}

fn cfstr(name: &'static str) -> core_foundation::string::CFStringRef {
    use core_foundation::string::CFString;
    thread_local! {
        static S: std::cell::RefCell<std::collections::HashMap<&'static str, CFString>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    S.with(|cell| {
        let mut m = cell.borrow_mut();
        let s = m.entry(name).or_insert_with(|| CFString::new(name));
        s.as_concrete_TypeRef()
    })
}

fn ax_find_window_by_title(pid: i32, title: &str) -> Option<*mut core::ffi::c_void> {
    use core_foundation::{array::CFArray, base::TCFType};
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return None;
    }
    let mut wins_ref: core_foundation::base::CFTypeRef = std::ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(app, cfstr("AXWindows"), &mut wins_ref) };
    if err != 0 || wins_ref.is_null() {
        return None;
    }
    let arr = unsafe { CFArray::<*const core::ffi::c_void>::wrap_under_create_rule(wins_ref as _) };
    let n = unsafe { core_foundation::array::CFArrayGetCount(arr.as_concrete_TypeRef()) };
    for i in 0..n {
        let w =
            unsafe { core_foundation::array::CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) }
                as *mut core::ffi::c_void;
        if w.is_null() {
            continue;
        }
        let mut t_ref: core_foundation::base::CFTypeRef = std::ptr::null_mut();
        let terr = unsafe { AXUIElementCopyAttributeValue(w, cfstr("AXTitle"), &mut t_ref) };
        if terr != 0 || t_ref.is_null() {
            continue;
        }
        let cfs = unsafe { core_foundation::string::CFString::wrap_under_create_rule(t_ref as _) };
        if cfs == title {
            // Retain borrowed AX element so it remains valid beyond this CFArray scope
            unsafe { CFRetain(w as core_foundation::base::CFTypeRef) };
            return Some(w);
        }
    }
    None
}

pub fn ax_set_bool_by_title(pid: i32, title: &str, attr: &'static str, value: bool) -> bool {
    use core_foundation::boolean::{kCFBooleanFalse, kCFBooleanTrue};
    if let Some(w) = ax_find_window_by_title(pid, title) {
        let val = unsafe {
            if value {
                kCFBooleanTrue
            } else {
                kCFBooleanFalse
            }
        } as _;
        let err = unsafe { AXUIElementSetAttributeValue(w, cfstr(attr), val) };
        return err == 0;
    }
    false
}

pub fn ax_get_bool_by_title(pid: i32, title: &str, attr: &'static str) -> Option<bool> {
    if let Some(w) = ax_find_window_by_title(pid, title) {
        let mut v: core_foundation::base::CFTypeRef = std::ptr::null_mut();
        let err = unsafe { AXUIElementCopyAttributeValue(w, cfstr(attr), &mut v) };
        if err != 0 || v.is_null() {
            return None;
        }
        let b = unsafe { CFBooleanGetValue(v) };
        return Some(b);
    }
    None
}
