use std::{ffi::c_void, ptr::null_mut};

use core_foundation::{
    base::{CFTypeRef, TCFType},
    string::CFStringRef,
};
use objc2_foundation::MainThreadMarker;
use tracing::{info, warn};

use crate::{
    WindowId,
    ax::{ax_check, cfstr},
    error::{Error, Result},
    list_windows,
    main_thread_ops::request_activate_pid,
};

#[allow(clippy::missing_safety_doc)]
pub fn raise_window(pid: i32, id: WindowId) -> Result<()> {
    // Ensure Accessibility and main thread
    ax_check()?;
    let _mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;

    info!("raise_window: pid={} id={} (attempt AX)", pid, id);
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
        fn AXUIElementPerformAction(element: *mut c_void, action: CFStringRef) -> i32;
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

    // Decide fallback if we cannot raise via AX
    let need_fallback: bool;
    if found.is_null() {
        info!(
            "raise_window: did not find AX window with id={} for pid={}",
            id, pid
        );
        need_fallback = true;
    } else {
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

        // Try AXRaise on the app first, then on the window.
        let mut raised = false;
        let mut acts_ref: CFTypeRef = null_mut();
        let acts_err = unsafe { AXUIElementCopyActionNames(app.as_ptr(), &mut acts_ref) };
        if acts_err == 0 && !acts_ref.is_null() {
            let arr = unsafe {
                core_foundation::array::CFArray::<*const c_void>::wrap_under_create_rule(
                    acts_ref as _,
                )
            };
            let mut can_raise = false;
            unsafe {
                for j in 0..core_foundation::array::CFArrayGetCount(arr.as_concrete_TypeRef()) {
                    let name = core_foundation::array::CFArrayGetValueAtIndex(
                        arr.as_concrete_TypeRef(),
                        j,
                    ) as CFStringRef;
                    if CFEqual(name as CFTypeRef, cfstr("AXRaise") as CFTypeRef) {
                        can_raise = true;
                        break;
                    }
                }
            }
            if can_raise {
                let app_raise = unsafe { AXUIElementPerformAction(app.as_ptr(), cfstr("AXRaise")) };
                if app_raise != 0 {
                    warn!(
                        "AXUIElementPerformAction(app, AXRaise) failed: {}",
                        app_raise
                    );
                } else {
                    raised = true;
                }
            }
        }

        if !raised {
            info!("raise_window: app does not support AXRaise or failed; checking window actions");
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
                    let w_raise = unsafe { AXUIElementPerformAction(found, cfstr("AXRaise")) };
                    if w_raise != 0 {
                        warn!(
                            "AXUIElementPerformAction(window, AXRaise) failed: {}",
                            w_raise
                        );
                    } else {
                        raised = true;
                    }
                } else {
                    info!("raise_window: window does not support AXRaise; skipping action");
                }
            }
        }
        need_fallback = !raised;
    }

    // `app` released by RAII on drop

    if need_fallback {
        info!(
            "raise_window: scheduling NSRunningApplication activation fallback for pid={}",
            pid
        );
        let _ = request_activate_pid(pid);
    }
    Ok(())
}
