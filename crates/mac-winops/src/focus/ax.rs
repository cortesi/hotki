use std::ffi::{CString, c_char, c_void};

use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::runloop::{
    CFRunLoop, CFRunLoopSource, CFRunLoopSourceRef, kCFRunLoopDefaultMode,
};
use core_foundation::string::CFStringRef;
use tokio::sync::mpsc::UnboundedSender;

use super::event::FocusEvent;

#[inline]
pub(crate) fn ax_is_trusted() -> bool {
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

#[derive(Default)]
pub(crate) struct AXState {
    pub(crate) observer: *mut c_void,
    pub(crate) app_elem: *mut c_void,
    pub(crate) have_source: bool,
    pub(crate) ctx_ptr: *mut c_void,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("AXObserverCreate failed: code {0}")]
    ObserverCreate(i32),
    #[error("AX UI element for app is null")]
    AppElementNull,
    #[error("AXObserverAddNotification failed: code {0}")]
    AddNotification(i32),
    #[error("AXObserverGetRunLoopSource returned null")]
    GetRunLoopSourceNull,
    #[error("CString creation failed for constant {0}")]
    CString(&'static str),
}

impl AXState {
    pub(crate) fn detach(&mut self) {
        unsafe {
            if !self.app_elem.is_null() {
                CFRelease(self.app_elem as CFTypeRef);
                self.app_elem = std::ptr::null_mut();
            }
            if !self.observer.is_null() {
                CFRelease(self.observer as CFTypeRef);
                self.observer = std::ptr::null_mut();
            }
            if !self.ctx_ptr.is_null() {
                let _ = Box::<AXCtx>::from_raw(self.ctx_ptr as *mut AXCtx);
                self.ctx_ptr = std::ptr::null_mut();
            }
            self.have_source = false;
        }
    }

    pub(crate) fn attach(
        &mut self,
        pid: i32,
        tx: UnboundedSender<FocusEvent>,
    ) -> Result<(), Error> {
        self.detach();
        unsafe extern "C" {
            fn AXObserverCreate(
                application: i32,
                callback: extern "C" fn(
                    observer: *mut c_void,
                    element: *mut c_void,
                    notification: CFStringRef,
                    refcon: *mut c_void,
                ),
                out: *mut *mut c_void,
            ) -> i32;
            fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
            fn AXObserverGetRunLoopSource(observer: *mut c_void) -> *mut c_void;
            fn AXObserverAddNotification(
                observer: *mut c_void,
                element: *mut c_void,
                notification: CFStringRef,
                refcon: *mut c_void,
            ) -> i32;
            fn CFStringCreateWithCString(
                alloc: *const c_void,
                cStr: *const c_char,
                encoding: u32,
            ) -> CFStringRef;
        }

        extern "C" fn ax_callback(
            _observer: *mut c_void,
            _element: *mut c_void,
            notification: CFStringRef,
            refcon: *mut c_void,
        ) {
            unsafe {
                let ctx = &*(refcon as *mut AXCtx);
                ctx.handle_notification(notification);
            }
        }

        unsafe {
            let mut observer: *mut c_void = std::ptr::null_mut();
            let err = AXObserverCreate(pid, ax_callback, &mut observer as *mut _);
            if err != 0 || observer.is_null() {
                return Err(Error::ObserverCreate(err));
            }
            let app_elem = AXUIElementCreateApplication(pid);
            if app_elem.is_null() {
                CFRelease(observer as CFTypeRef);
                return Err(Error::AppElementNull);
            }

            // Create CFString constants we need
            const UTF8: u32 = 0x0800_0100;
            let mk = |name: &'static str| -> Result<CFStringRef, Error> {
                let cs = CString::new(name).map_err(|_| Error::CString(name))?;
                Ok(CFStringCreateWithCString(
                    std::ptr::null(),
                    cs.as_ptr(),
                    UTF8,
                ))
            };
            let notif_focused_window_changed = mk("AXFocusedWindowChanged")?;
            let notif_title_changed = mk("AXTitleChanged")?;
            let attr_focused_window = mk("AXFocusedWindow")?;
            let attr_title = mk("AXTitle")?;

            let ctx = Box::new(AXCtx {
                tx,
                app_elem,
                notif_focused_window_changed,
                notif_title_changed,
                attr_focused_window,
                attr_title,
                pid,
            });
            let notif = ctx.notif_focused_window_changed;
            let ctx_ptr = Box::into_raw(ctx) as *mut c_void;

            let err = AXObserverAddNotification(observer, app_elem, notif, ctx_ptr);
            if err != 0 {
                CFRelease(app_elem as CFTypeRef);
                CFRelease(observer as CFTypeRef);
                let _ = Box::<AXCtx>::from_raw(ctx_ptr as *mut AXCtx);
                return Err(Error::AddNotification(err));
            }

            let source = AXObserverGetRunLoopSource(observer);
            if source.is_null() {
                CFRelease(app_elem as CFTypeRef);
                CFRelease(observer as CFTypeRef);
                let _ = Box::<AXCtx>::from_raw(ctx_ptr as *mut AXCtx);
                return Err(Error::GetRunLoopSourceNull);
            }
            let rl = CFRunLoop::get_current();
            let mode = kCFRunLoopDefaultMode;
            let source_ref = source as CFRunLoopSourceRef;
            let source_obj = CFRunLoopSource::wrap_under_get_rule(source_ref);
            rl.add_source(&source_obj, mode);

            self.observer = observer;
            self.app_elem = app_elem;
            self.have_source = true;
            self.ctx_ptr = ctx_ptr;
        }
        Ok(())
    }

    pub(crate) fn pump_runloop_once(&self) {
        if !self.have_source {
            return;
        }
        let mode = unsafe { kCFRunLoopDefaultMode };
        let _ = CFRunLoop::run_in_mode(mode, std::time::Duration::from_millis(10), true);
    }
}

struct AXCtx {
    tx: UnboundedSender<FocusEvent>,
    app_elem: *mut c_void,
    notif_focused_window_changed: CFStringRef,
    notif_title_changed: CFStringRef,
    attr_focused_window: CFStringRef,
    attr_title: CFStringRef,
    pid: i32,
}

impl AXCtx {
    fn handle_notification(&self, notification: CFStringRef) {
        unsafe extern "C" {
            fn AXUIElementCopyAttributeValue(
                element: *mut c_void,
                attr: CFStringRef,
                value: *mut CFTypeRef,
            ) -> i32;
        }
        unsafe {
            let notif_title_changed = self.notif_title_changed;
            if notification == notif_title_changed {
                let mut val: CFTypeRef = std::ptr::null_mut();
                let err = AXUIElementCopyAttributeValue(
                    self.app_elem,
                    self.attr_focused_window,
                    &mut val,
                );
                if err == 0 && !val.is_null() {
                    // Focused window changed; try to fetch title
                    let mut title_val: CFTypeRef = std::ptr::null_mut();
                    let err2 = AXUIElementCopyAttributeValue(
                        val as *mut c_void,
                        self.attr_title,
                        &mut title_val,
                    );
                    if err2 == 0 && !title_val.is_null() {
                        let s = core_foundation::string::CFString::wrap_under_get_rule(
                            title_val as CFStringRef,
                        )
                        .to_string();
                        let _ = self.tx.send(FocusEvent::TitleChanged {
                            title: s,
                            pid: self.pid,
                        });
                        CFRelease(title_val);
                    }
                    CFRelease(val);
                }
            }
        }
    }
}
