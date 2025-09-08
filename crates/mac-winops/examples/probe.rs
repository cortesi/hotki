use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::number::CFBooleanGetValue;
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::c_void;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
    fn AXUIElementCopyAttributeValue(
        element: *mut c_void,
        attr: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
}

fn cfstr(s: &'static str) -> CFStringRef {
    CFString::from_static_string(s).as_concrete_TypeRef()
}

fn main() {
    let arg_pid = std::env::args().nth(1).and_then(|s| s.parse::<i32>().ok());
    let ok = permissions::accessibility_ok();
    println!("[probe] accessibility_ok: {}", ok);
    if !ok {
        eprintln!("no accessibility; exiting");
        std::process::exit(1);
    }
    let pid = arg_pid
        .or_else(|| mac_winops::frontmost_window().map(|w| w.pid))
        .unwrap_or(-1);
    println!("[probe] pid: {}", pid);
    assert!(pid > 0, "no frontmost window pid");
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        assert!(!app.is_null());
        let mut v: CFTypeRef = std::ptr::null_mut();
        println!("[probe] calling AXUIElementCopyAttributeValue(AXWindows)...");
        let err_wins = AXUIElementCopyAttributeValue(app, cfstr("AXWindows"), &mut v);
        println!(
            "[probe] AXWindows err={}, v.is_null={}",
            err_wins,
            v.is_null()
        );
        if !v.is_null() {
            // Try scanning for focused/main window without using AXFocusedWindow
            let arr = core_foundation::array::CFArray::<*const c_void>::wrap_under_create_rule(
                v as *const core_foundation::array::__CFArray,
            );
            let n = core_foundation::array::CFArrayGetCount(arr.as_concrete_TypeRef());
            for i in 0..n {
                let w = core_foundation::array::CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i)
                    as *mut c_void;
                if w.is_null() {
                    continue;
                }
                let mut bv: CFTypeRef = std::ptr::null_mut();
                let e1 = AXUIElementCopyAttributeValue(w, cfstr("AXFocused"), &mut bv);
                let focused = if e1 == 0 && !bv.is_null() {
                    let b = unsafe { CFBooleanGetValue(bv as *const _) };
                    CFRelease(bv);
                    b
                } else {
                    false
                };
                let mut mv: CFTypeRef = std::ptr::null_mut();
                let e2 = AXUIElementCopyAttributeValue(w, cfstr("AXMain"), &mut mv);
                let main = if e2 == 0 && !mv.is_null() {
                    let b = unsafe { CFBooleanGetValue(mv as *const _) };
                    CFRelease(mv);
                    b
                } else {
                    false
                };
                println!("[probe] window[{}]: focused={}, main={}", i, focused, main);
            }
        }
        // v consumed by wrap_under_create_rule
        v = std::ptr::null_mut();
        println!("[probe] calling AXUIElementCopyAttributeValue(AXFocusedWindow)...");
        let err_focus = AXUIElementCopyAttributeValue(app, cfstr("AXFocusedWindow"), &mut v);
        println!("[probe] AXFocusedWindow err={}, v={:?}", err_focus, v);
        if !v.is_null() {
            CFRelease(v);
        }
        CFRelease(app as CFTypeRef);
    }
}
