use std::{ffi::c_void, ptr::null_mut};

use core_foundation::{
    array::{CFArray, CFArrayGetCount, CFArrayGetValueAtIndex},
    base::{CFTypeRef, TCFType},
    boolean::{kCFBooleanFalse, kCFBooleanTrue},
    string::CFStringRef,
};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use objc2_foundation::MainThreadMarker;
use tracing::{Level, debug, error, info, trace, warn};

use crate::{
    AXElem, WindowId,
    ax::{ax_check, ax_perform_action, cfstr},
    error::{Error, Result},
    status::{self, StatusKind},
    window::{frontmost_window, list_windows},
};

#[link(name = "SkyLight", kind = "framework")]
unsafe extern "C" {
    fn CGSMainConnectionID() -> i32;
    fn CGSOrderWindow(connection: i32, window: i32, place: i32, relative_to_window: i32) -> i32;
}

const K_CGS_ORDER_ABOVE: i32 = 1;

unsafe extern "C" {
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
    fn AXUIElementCopyActionNames(element: *mut c_void, names: *mut CFTypeRef) -> i32;
    fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> bool;
}

#[allow(clippy::missing_safety_doc)]
pub fn raise_window(pid: i32, id: WindowId) -> Result<()> {
    // Ensure Accessibility and main thread
    ax_check()?;
    let _mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;

    debug!(
        "raise_window: pid={} id={} (prefer window AXRaise)",
        pid, id
    );
    // Detect cross-app boundary (frontmost pid differs from target)
    let crossing_app_boundary = match frontmost_window() {
        Some(w) => w.pid != pid,
        None => true,
    };
    let windows = list_windows();
    // Validate via CG that the window exists and is on-screen before using AX.
    if !windows.iter().any(|w| w.pid == pid && w.id == id) {
        debug!(
            "raise_window: window pid={} id={} not found on-screen; skipping",
            pid, id
        );
        return Ok(());
    }

    debug!("raise_window: creating AX app element for pid={}", pid);
    let Some(app) = (unsafe { AXElem::from_create(AXUIElementCreateApplication(pid)) }) else {
        return Err(Error::AppElement);
    };

    let app_windows = copy_ax_windows(&app, pid)?;

    // Locate matching AX window by AXWindowNumber
    let mut found: Option<AXElem> = None;
    for (i, window) in app_windows.iter().enumerate() {
        let wref = window.as_ptr();
        if let Some(pid_id) = crate::ax_private::window_id_for_ax_element(wref) {
            debug!("raise_window: candidate i={} wid={} (private)", i, pid_id);
            if pid_id == id {
                debug!(
                    "raise_window: matched AX window at i={} wid={} (private)",
                    i, pid_id
                );
                found = Some(window.clone());
                break;
            }
            continue;
        }

        let mut num_ref: CFTypeRef = null_mut();
        let err =
            unsafe { AXUIElementCopyAttributeValue(wref, cfstr("AXWindowNumber"), &mut num_ref) };
        if err != 0 || num_ref.is_null() {
            continue;
        }
        let cfnum =
            unsafe { core_foundation::number::CFNumber::wrap_under_create_rule(num_ref as _) };
        let wid = cfnum.to_i64().unwrap_or(0) as u32;
        debug!("raise_window: candidate i={} wid={}", i, wid);
        if wid == id {
            debug!("raise_window: matched AX window at i={} wid={}", i, wid);
            found = Some(window.clone());
            break;
        }
    }

    if found.is_none() {
        debug!(
            "raise_window: did not find AX window with id={} for pid={}",
            id, pid
        );
        if let Some((title, elem)) = match_window_by_title(pid, id, &windows) {
            debug!("raise_window: matched by title via AX: '{}'", title);
            found = Some(elem);
        }
    }

    let mut raised = false;
    if let Some(window) = found.as_ref() {
        raised = raise_with_ax(&app, window);
        if !raised {
            raised = order_with_cgs(pid, id);
        }
    }

    if !raised {
        debug!(
            crossing_app_boundary = crossing_app_boundary,
            "raise_window: AX-based raise failed; activating app asynchronously for pid={}", pid
        );
        activate_app_async(pid);
    }

    Ok(())
}

fn copy_ax_windows(app: &AXElem, pid: i32) -> Result<Vec<AXElem>> {
    let mut wins_ref: CFTypeRef = null_mut();
    debug!("raise_window: copying AXWindows for pid={}", pid);
    let err =
        unsafe { AXUIElementCopyAttributeValue(app.as_ptr(), cfstr("AXWindows"), &mut wins_ref) };
    if err != 0 || wins_ref.is_null() {
        warn!("AXUIElementCopyAttributeValue(AXWindows) failed: {}", err);
        return Err(Error::AxCode(err));
    }
    debug!("raise_window: AXWindows copied");

    let arr: CFArray<*const c_void> = unsafe { CFArray::wrap_under_create_rule(wins_ref as _) };
    let count = unsafe { CFArrayGetCount(arr.as_concrete_TypeRef()) };
    let mut windows = Vec::with_capacity(count.max(0) as usize);
    for i in 0..count {
        let wref = unsafe { CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) } as *mut c_void;
        if let Some(elem) = AXElem::retain_from_borrowed(wref) {
            windows.push(elem);
        }
    }
    Ok(windows)
}

fn match_window_by_title(
    pid: i32,
    id: WindowId,
    windows: &[crate::window::WindowInfo],
) -> Option<(String, AXElem)> {
    let winfo = windows
        .iter()
        .find(|w| w.pid == pid && w.id == id && !w.title.is_empty())?;
    let elem = crate::ax::ax_find_window_by_title(pid, &winfo.title)?;
    Some((winfo.title.clone(), elem))
}

fn raise_with_ax(app: &AXElem, window: &AXElem) -> bool {
    unsafe {
        let _ = AXUIElementSetAttributeValue(
            window.as_ptr(),
            cfstr("AXMain"),
            kCFBooleanTrue as CFTypeRef,
        );
        let _ = AXUIElementSetAttributeValue(
            window.as_ptr(),
            cfstr("AXFocused"),
            kCFBooleanTrue as CFTypeRef,
        );
        let _ = AXUIElementSetAttributeValue(
            app.as_ptr(),
            cfstr("AXFocusedWindow"),
            window.as_ptr() as CFTypeRef,
        );
        let _ = AXUIElementSetAttributeValue(
            app.as_ptr(),
            cfstr("AXFrontmost"),
            kCFBooleanTrue as CFTypeRef,
        );
        let _ = AXUIElementSetAttributeValue(
            window.as_ptr(),
            cfstr("AXMinimized"),
            kCFBooleanFalse as CFTypeRef,
        );
    }

    let mut w_acts_ref: CFTypeRef = null_mut();
    let w_acts_err = unsafe { AXUIElementCopyActionNames(window.as_ptr(), &mut w_acts_ref) };
    if w_acts_err != 0 || w_acts_ref.is_null() {
        return false;
    }

    let arr = unsafe { CFArray::<*const c_void>::wrap_under_create_rule(w_acts_ref as _) };
    let mut supports_raise = false;
    unsafe {
        for j in 0..CFArrayGetCount(arr.as_concrete_TypeRef()) {
            let name = CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), j) as CFStringRef;
            if CFEqual(name as CFTypeRef, cfstr("AXRaise") as CFTypeRef) {
                supports_raise = true;
                break;
            }
        }
    }

    if !supports_raise {
        debug!("raise_window: window does not support AXRaise; skipping action");
        return false;
    }

    match ax_perform_action(window.as_ptr(), cfstr("AXRaise")) {
        Ok(()) => true,
        Err(err) => {
            warn!(?err, "AXRaise on window failed");
            false
        }
    }
}

fn order_with_cgs(pid: i32, id: WindowId) -> bool {
    let order_err =
        unsafe { CGSOrderWindow(CGSMainConnectionID(), id as i32, K_CGS_ORDER_ABOVE, 0) };
    if order_err == 0 {
        debug!(
            "raise_window: CGSOrderWindow promoted pid={} id={} above all",
            pid, id
        );
        return true;
    }

    if let Some(policy) = status::policy(StatusKind::CgsOrderWindow, order_err) {
        let msg = "raise_window: CGSOrderWindow returned expected error";
        match policy.level {
            Level::ERROR => error!(pid, id, err = order_err, note = policy.note, "{msg}"),
            Level::WARN => warn!(pid, id, err = order_err, note = policy.note, "{msg}"),
            Level::INFO => info!(pid, id, err = order_err, note = policy.note, "{msg}"),
            Level::DEBUG => debug!(pid, id, err = order_err, note = policy.note, "{msg}"),
            Level::TRACE => trace!(pid, id, err = order_err, note = policy.note, "{msg}"),
        }
    } else {
        warn!(
            "raise_window: CGSOrderWindow failed for pid={} id={} err={}",
            pid, id, order_err
        );
    }
    false
}

fn activate_app_async(pid: i32) {
    unsafe {
        if let Some(app) =
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid as libc::pid_t)
        {
            if !app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows) {
                warn!(
                    "NSRunningApplication.activateWithOptions(ActivateAllWindows) returned false for pid={}",
                    pid
                );
            } else {
                debug!("Queued async activation for pid={}", pid);
            }
        } else {
            warn!("NSRunningApplication not found for pid={}", pid);
        }
    }
}
