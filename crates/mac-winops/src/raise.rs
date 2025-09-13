use std::{ffi::c_void, ptr::null_mut};

use core_foundation::{
    base::{CFTypeRef, TCFType},
    string::CFStringRef,
};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use objc2_foundation::MainThreadMarker;
use tracing::{info, warn};

use crate::{
    WindowId,
    ax::{ax_check, ax_perform_action, cfstr},
    error::{Error, Result},
    frontmost_window, list_windows,
};

#[allow(clippy::missing_safety_doc)]
pub fn raise_window(pid: i32, id: WindowId) -> Result<()> {
    // Ensure Accessibility and main thread
    ax_check()?;
    let _mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;

    info!(
        "raise_window: pid={} id={} (prefer window AXRaise)",
        pid, id
    );
    // Detect cross-app boundary (frontmost pid differs from target)
    let crossing_app_boundary = match frontmost_window() {
        Some(w) => w.pid != pid,
        None => true,
    };
    // Validate via CG that the window exists and is on-screen before using AX.
    if !list_windows()
        .into_iter()
        .any(|w| w.pid == pid && w.id == id)
    {
        info!(
            "raise_window: window pid={} id={} not found on-screen; skipping",
            pid, id
        );
        return Ok(());
    }

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

    info!("raise_window: creating AX app element for pid={}", pid);
    let Some(app) = (unsafe { crate::AXElem::from_create(AXUIElementCreateApplication(pid)) })
    else {
        return Err(Error::AppElement);
    };

    let mut wins_ref: CFTypeRef = null_mut();
    info!("raise_window: copying AXWindows for pid={}", pid);
    let err =
        unsafe { AXUIElementCopyAttributeValue(app.as_ptr(), cfstr("AXWindows"), &mut wins_ref) };
    if err != 0 || wins_ref.is_null() {
        warn!("AXUIElementCopyAttributeValue(AXWindows) failed: {}", err);
        return Err(Error::AxCode(err));
    }
    info!("raise_window: AXWindows copied");

    // Locate matching AX window by AXWindowNumber
    let mut found: *mut c_void = null_mut();
    let mut _found_guard: Option<crate::AXElem> = None;
    let arr: core_foundation::array::CFArray<*const c_void> =
        unsafe { core_foundation::array::CFArray::wrap_under_create_rule(wins_ref as _) };
    for i in 0..unsafe { core_foundation::array::CFArrayGetCount(arr.as_concrete_TypeRef()) } {
        let wref =
            unsafe { core_foundation::array::CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) }
                as *mut c_void;
        if wref.is_null() {
            continue;
        }
        // Try AXWindowNumber; if unsupported, skip.
        let mut num_ref: CFTypeRef = null_mut();
        let err =
            unsafe { AXUIElementCopyAttributeValue(wref, cfstr("AXWindowNumber"), &mut num_ref) };
        if err != 0 || num_ref.is_null() {
            continue;
        }
        let cfnum =
            unsafe { core_foundation::number::CFNumber::wrap_under_create_rule(num_ref as _) };
        let wid = cfnum.to_i64().unwrap_or(0) as u32;
        info!("raise_window: candidate i={} wid={}", i, wid);
        if wid == id {
            found = wref;
            info!("raise_window: matched AX window at i={} wid={}", i, wid);
            break;
        }
    }

    // Helper to perform synchronous app activation on the main thread.
    let activate_app = |pid: i32| unsafe {
        if let Some(app) =
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid as libc::pid_t)
        {
            let ok = app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
            if !ok {
                warn!(
                    "NSRunningApplication.activateWithOptions(ActivateAllWindows) returned false for pid={}",
                    pid
                );
            } else {
                info!("Activated app (all windows) for pid={}", pid);
            }
        } else {
            warn!("NSRunningApplication not found for pid={}", pid);
        }
    };

    // Prefer: if we are moving across apps, bring the app forward first, then raise.
    if crossing_app_boundary {
        info!("raise_window: crossing app boundary → activating app first");
        activate_app(pid);
    }

    let mut raised = false;
    if found.is_null() {
        info!(
            "raise_window: did not find AX window with id={} for pid={}",
            id, pid
        );
        // Fallback: try to match by title via AX if available.
        if let Some(winfo) = list_windows()
            .into_iter()
            .find(|w| w.pid == pid && w.id == id)
        {
            if !winfo.title.is_empty() {
                if let Some(elem) = crate::ax::ax_find_window_by_title(pid, &winfo.title) {
                    info!("raise_window: matched by title via AX: '{}'", winfo.title);
                    found = elem.as_ptr();
                    _found_guard = Some(elem);
                }
            }
        }
    }

    // If we now have a candidate window, apply focus hints and attempt AXRaise.
    if !found.is_null() {
        // Best-effort hints on the window itself; avoid AXFocusedWindow entirely.
        let _ = unsafe {
            AXUIElementSetAttributeValue(
                found,
                cfstr("AXMain"),
                core_foundation::boolean::kCFBooleanTrue as CFTypeRef,
            )
        };
        let _ = unsafe {
            AXUIElementSetAttributeValue(
                found,
                cfstr("AXFocused"),
                core_foundation::boolean::kCFBooleanTrue as CFTypeRef,
            )
        };
        // Best-effort: unminimize before attempting raise to make focus effective.
        let _ = unsafe {
            AXUIElementSetAttributeValue(
                found,
                cfstr("AXMinimized"),
                core_foundation::boolean::kCFBooleanFalse as CFTypeRef,
            )
        };

        // Prefer window-level AXRaise if supported.
        let mut w_acts_ref: CFTypeRef = null_mut();
        let w_acts_err = unsafe { AXUIElementCopyActionNames(found, &mut w_acts_ref) };
        if w_acts_err == 0 && !w_acts_ref.is_null() {
            let arr = unsafe {
                core_foundation::array::CFArray::<*const c_void>::wrap_under_create_rule(
                    w_acts_ref as _,
                )
            };
            let mut w_can_raise = false;
            unsafe {
                for j in 0..core_foundation::array::CFArrayGetCount(arr.as_concrete_TypeRef()) {
                    let name = core_foundation::array::CFArrayGetValueAtIndex(
                        arr.as_concrete_TypeRef(),
                        j,
                    ) as CFStringRef;
                    if CFEqual(name as CFTypeRef, cfstr("AXRaise") as CFTypeRef) {
                        w_can_raise = true;
                        break;
                    }
                }
            }
            if w_can_raise {
                if ax_perform_action(found, cfstr("AXRaise")).is_ok() {
                    raised = true;
                } else {
                    warn!("AXRaise on window failed");
                }
            } else {
                info!("raise_window: window does not support AXRaise; skipping action");
            }
        }
    }

    // If not raised (no AX support or not found), fall back to app activation.
    if !raised && !crossing_app_boundary {
        info!(
            "raise_window: falling back to NSRunningApplication activation for pid={}",
            pid
        );
        activate_app(pid);
    }

    Ok(())
}
