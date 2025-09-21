use std::{ffi::c_void, ptr::null_mut};

use core_foundation::{
    base::{CFTypeRef, TCFType},
    string::CFStringRef,
};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use objc2_foundation::MainThreadMarker;
use tracing::{Level, debug, error, info, trace, warn};

use crate::{
    WindowId,
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
    // Validate via CG that the window exists and is on-screen before using AX.
    if !list_windows()
        .into_iter()
        .any(|w| w.pid == pid && w.id == id)
    {
        debug!(
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

    debug!("raise_window: creating AX app element for pid={}", pid);
    let Some(app) = (unsafe { crate::AXElem::from_create(AXUIElementCreateApplication(pid)) })
    else {
        return Err(Error::AppElement);
    };

    let mut wins_ref: CFTypeRef = null_mut();
    debug!("raise_window: copying AXWindows for pid={}", pid);
    let err =
        unsafe { AXUIElementCopyAttributeValue(app.as_ptr(), cfstr("AXWindows"), &mut wins_ref) };
    if err != 0 || wins_ref.is_null() {
        warn!("AXUIElementCopyAttributeValue(AXWindows) failed: {}", err);
        return Err(Error::AxCode(err));
    }
    debug!("raise_window: AXWindows copied");

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
        // Prefer private id; fallback to AXWindowNumber.
        if let Some(pid_id) = crate::ax_private::window_id_for_ax_element(wref) {
            debug!("raise_window: candidate i={} wid={} (private)", i, pid_id);
            if pid_id == id {
                found = wref;
                debug!(
                    "raise_window: matched AX window at i={} wid={} (private)",
                    i, pid_id
                );
                break;
            }
        } else {
            let mut num_ref: CFTypeRef = null_mut();
            let err = unsafe {
                AXUIElementCopyAttributeValue(wref, cfstr("AXWindowNumber"), &mut num_ref)
            };
            if err != 0 || num_ref.is_null() {
                continue;
            }
            let cfnum =
                unsafe { core_foundation::number::CFNumber::wrap_under_create_rule(num_ref as _) };
            let wid = cfnum.to_i64().unwrap_or(0) as u32;
            debug!("raise_window: candidate i={} wid={}", i, wid);
            if wid == id {
                found = wref;
                debug!("raise_window: matched AX window at i={} wid={}", i, wid);
                break;
            }
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
                debug!("Activated app (all windows) for pid={}", pid);
            }
        } else {
            warn!("NSRunningApplication not found for pid={}", pid);
        }
    };

    // Prefer: if we are moving across apps, bring the app forward first, then raise.
    if crossing_app_boundary {
        debug!("raise_window: crossing app boundary → activating app first");
        activate_app(pid);
    }

    let mut raised = false;
    if found.is_null() {
        debug!(
            "raise_window: did not find AX window with id={} for pid={}",
            id, pid
        );
        // Fallback: try to match by title via AX if available.
        if let Some(winfo) = list_windows()
            .into_iter()
            .find(|w| w.pid == pid && w.id == id)
            && !winfo.title.is_empty()
            && let Some(elem) = crate::ax::ax_find_window_by_title(pid, &winfo.title)
        {
            debug!("raise_window: matched by title via AX: '{}'", winfo.title);
            found = elem.as_ptr();
            _found_guard = Some(elem);
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
        let _ = unsafe {
            AXUIElementSetAttributeValue(app.as_ptr(), cfstr("AXFocusedWindow"), found as CFTypeRef)
        };
        let _ = unsafe {
            AXUIElementSetAttributeValue(
                app.as_ptr(),
                cfstr("AXFrontmost"),
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

        let order_err =
            unsafe { CGSOrderWindow(CGSMainConnectionID(), id as i32, K_CGS_ORDER_ABOVE, 0) };
        if order_err == 0 {
            info!(
                "raise_window: CGSOrderWindow promoted pid={} id={} above all",
                pid, id
            );
        } else if let Some(policy) = status::policy(StatusKind::CgsOrderWindow, order_err) {
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
    }

    // If not raised (no AX support or not found), fall back to app activation.
    if !raised && !crossing_app_boundary {
        debug!(
            "raise_window: falling back to NSRunningApplication activation for pid={}",
            pid
        );
        activate_app(pid);
    }

    Ok(())
}
