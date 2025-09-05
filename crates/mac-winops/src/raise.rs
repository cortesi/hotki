use core_foundation::{
    base::{CFTypeRef, TCFType},
    string::CFStringRef,
};
use std::ffi::c_void;
use std::ptr::null_mut;

use objc2_foundation::MainThreadMarker;
use tracing::{info, warn};

use crate::{Error, Result, WindowId, list_windows, request_activate_pid};

#[allow(clippy::missing_safety_doc)]
pub fn raise_window(pid: i32, id: WindowId) -> Result<()> {
    // Ensure Accessibility and main thread
    super::ax_check()?;
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
        fn AXUIElementIsAttributeSettable(
            element: *mut c_void,
            attr: CFStringRef,
            settable: *mut bool,
        ) -> i32;
        fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> bool;
    }

    info!("raise_window: creating AX app element for pid={}", pid);
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return Err(Error::AppElement);
    }

    let mut wins_ref: CFTypeRef = null_mut();
    info!("raise_window: copying AXWindows for pid={}", pid);
    let err =
        unsafe { AXUIElementCopyAttributeValue(app, super::cfstr("AXWindows"), &mut wins_ref) };
    if err != 0 || wins_ref.is_null() {
        warn!("AXUIElementCopyAttributeValue(AXWindows) failed: {}", err);
        unsafe { core_foundation::base::CFRelease(app as CFTypeRef) };
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
        let err = unsafe {
            AXUIElementCopyAttributeValue(wref, super::cfstr("AXWindowNumber"), &mut num_ref)
        };
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

    // Decide fallback only if we attempted focus hints
    let need_fallback: bool;
    if found.is_null() {
        info!(
            "raise_window: did not find AX window with id={} for pid={}",
            id, pid
        );
        need_fallback = true;
    } else {
        // Try setting AXFocusedWindow on the app
        info!("raise_window: setting AXFocusedWindow on app element");
        let mut settable = false;
        let can_set = unsafe {
            let rc = AXUIElementIsAttributeSettable(
                app,
                super::cfstr("AXFocusedWindow"),
                &mut settable,
            );
            if rc != 0 {
                warn!(
                    "AXUIElementIsAttributeSettable(AXFocusedWindow) failed: {}",
                    rc
                );
            }
            rc == 0 && settable
        };
        let mut step_failed = false;
        if can_set {
            let set_err = unsafe {
                AXUIElementSetAttributeValue(
                    app,
                    super::cfstr("AXFocusedWindow"),
                    found as CFTypeRef,
                )
            };
            if set_err != 0 {
                warn!(
                    "AXUIElementSetAttributeValue(AXFocusedWindow) failed: {}",
                    set_err
                );
                step_failed = true;
            }
        } else {
            info!("raise_window: AXFocusedWindow not settable; skipping set");
            step_failed = true;
        }
            if !step_failed {
                // Hint AXMain on the window (ignore error)
                let _ = unsafe {
                    AXUIElementSetAttributeValue(
                        found,
                        super::cfstr("AXMain"),
                        core_foundation::boolean::kCFBooleanTrue as CFTypeRef,
                    )
                };
                // Also try marking the window as AXFocused (ignore error)
                let _ = unsafe {
                    AXUIElementSetAttributeValue(
                        found,
                        super::cfstr("AXFocused"),
                        core_foundation::boolean::kCFBooleanTrue as CFTypeRef,
                    )
                };
                // Only call AXRaise if supported on the app element
            let mut acts_ref: CFTypeRef = null_mut();
            let mut can_raise = false;
            let acts_err = unsafe { AXUIElementCopyActionNames(app, &mut acts_ref) };
            if acts_err == 0 && !acts_ref.is_null() {
                let arr = unsafe {
                    core_foundation::array::CFArray::<*const c_void>::wrap_under_create_rule(
                        acts_ref as _,
                    )
                };
                unsafe {
                    for j in 0..core_foundation::array::CFArrayGetCount(arr.as_concrete_TypeRef()) {
                        let name = core_foundation::array::CFArrayGetValueAtIndex(
                            arr.as_concrete_TypeRef(),
                            j,
                        ) as CFStringRef;
                        if CFEqual(name as CFTypeRef, super::cfstr("AXRaise") as CFTypeRef) {
                            can_raise = true;
                            break;
                        }
                    }
                }
            }
            if can_raise {
                let app_raise = unsafe { AXUIElementPerformAction(app, super::cfstr("AXRaise")) };
                if app_raise != 0 {
                    warn!(
                        "AXUIElementPerformAction(app, AXRaise) failed: {}",
                        app_raise
                    );
                }
            } else {
                info!("raise_window: app does not support AXRaise; checking window actions");
                // Try AXRaise on the window element if available
                let mut w_acts_ref: CFTypeRef = null_mut();
                let mut w_can_raise = false;
                let w_acts_err = unsafe { AXUIElementCopyActionNames(found, &mut w_acts_ref) };
                if w_acts_err == 0 && !w_acts_ref.is_null() {
                    let arr = unsafe {
                        core_foundation::array::CFArray::<*const c_void>::wrap_under_create_rule(
                            w_acts_ref as _,
                        )
                    };
                    unsafe {
                        for j in 0..core_foundation::array::CFArrayGetCount(arr.as_concrete_TypeRef()) {
                            let name = core_foundation::array::CFArrayGetValueAtIndex(
                                arr.as_concrete_TypeRef(),
                                j,
                            ) as CFStringRef;
                            if CFEqual(name as CFTypeRef, super::cfstr("AXRaise") as CFTypeRef) {
                                w_can_raise = true;
                                break;
                            }
                        }
                    }
                }
                if w_can_raise {
                    let w_raise = unsafe { AXUIElementPerformAction(found, super::cfstr("AXRaise")) };
                    if w_raise != 0 {
                        warn!(
                            "AXUIElementPerformAction(window, AXRaise) failed: {}",
                            w_raise
                        );
                    }
                } else {
                    info!("raise_window: window does not support AXRaise; skipping action");
                }
            }
        }
        need_fallback = step_failed;
    }

    unsafe { core_foundation::base::CFRelease(app as CFTypeRef) };

    if need_fallback {
        info!(
            "raise_window: scheduling NSRunningApplication activation fallback for pid={}",
            pid
        );
        let _ = request_activate_pid(pid);
    }
    Ok(())
}
