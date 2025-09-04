use std::ffi::c_void;

use core_foundation::{
    base::{CFRelease, CFTypeRef, TCFType},
    runloop::{CFRunLoop, CFRunLoopSource, CFRunLoopSourceRef, kCFRunLoopDefaultMode},
    string::{CFString, CFStringRef},
};
use thiserror::Error;
use tokio::sync::mpsc::UnboundedSender;
use tracing::warn;

use super::event::FocusEvent;

/// Returns true if the process is trusted for Accessibility (AX) APIs.
pub(crate) fn ax_is_trusted() -> bool {
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

#[derive(Default)]
pub(crate) struct AXState {
    observer: *mut c_void,
    app_elem: *mut c_void,
    pub(crate) have_source: bool,
    ctx_ptr: *mut c_void,
}

const AX_ERR_NOTIFICATION_ALREADY_REGISTERED: i32 = -25204;

#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("AXObserverCreate failed: {0}")]
    ObserverCreate(i32),
    #[error("AXUIElementCreateApplication returned null")]
    AppElementNull,
    #[error("AXObserverAddNotification: already registered")]
    AddNotificationAlreadyRegistered,
    #[error("AXObserverAddNotification failed: {0}")]
    AddNotification(i32),
    #[error("AXObserverGetRunLoopSource returned null")]
    GetRunLoopSourceNull,
}

impl AXState {
    pub(crate) fn detach(&mut self) {
        unsafe {
            if !self.app_elem.is_null() {
                CFRelease(self.app_elem as CFTypeRef);
            }
            if !self.observer.is_null() {
                CFRelease(self.observer as CFTypeRef);
            }
            if !self.ctx_ptr.is_null() {
                let _ = Box::<AXCtx>::from_raw(self.ctx_ptr as *mut AXCtx);
            }
        }
        self.observer = std::ptr::null_mut();
        self.app_elem = std::ptr::null_mut();
        self.have_source = false;
        self.ctx_ptr = std::ptr::null_mut();
    }

    pub(crate) fn attach(
        &mut self,
        pid: i32,
        tx: UnboundedSender<FocusEvent>,
    ) -> Result<(), Error> {
        self.detach();
        unsafe extern "C" {
            fn AXObserverCreate(
                pid: i32,
                callback: extern "C" fn(*mut c_void, *mut c_void, CFStringRef, *mut c_void),
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
        }

        extern "C" fn ax_callback(
            observer: *mut c_void,
            element: *mut c_void,
            notification: CFStringRef,
            refcon: *mut c_void,
        ) {
            unsafe {
                // SAFETY: refcon is Box<AXCtx> allocated in attach
                let ctx = &*(refcon as *mut AXCtx);
                ctx.handle_notification(observer, element, notification);
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

            // Create CFString constants (non-owning from 'static strs)
            let notif_focused_window_changed =
                CFString::from_static_string("AXFocusedWindowChanged");
            let notif_title_changed = CFString::from_static_string("AXTitleChanged");
            let attr_focused_window = CFString::from_static_string("AXFocusedWindow");
            let attr_title = CFString::from_static_string("AXTitle");

            // Create context used by callback (contains tx, app element, and CFStrings)
            let ctx = Box::new(AXCtx {
                tx,
                app_elem,
                notif_focused_window_changed,
                notif_title_changed,
                attr_focused_window,
                attr_title,
                pid,
            });
            // Capture notification ref before moving ctx into raw
            let notif = ctx.notif_focused_window_changed.as_concrete_TypeRef();
            let ctx_ptr = Box::into_raw(ctx) as *mut c_void;

            // Observe focused window changes on the app
            let err = AXObserverAddNotification(observer, app_elem, notif, ctx_ptr);
            if err != 0 {
                CFRelease(app_elem as CFTypeRef);
                CFRelease(observer as CFTypeRef);
                let _ = Box::<AXCtx>::from_raw(ctx_ptr as *mut AXCtx);
                return Err(if err == AX_ERR_NOTIFICATION_ALREADY_REGISTERED {
                    Error::AddNotificationAlreadyRegistered
                } else {
                    Error::AddNotification(err)
                });
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
    notif_focused_window_changed: CFString,
    notif_title_changed: CFString,
    attr_focused_window: CFString,
    attr_title: CFString,
    pid: i32,
}

impl AXCtx {
    fn handle_notification(
        &self,
        observer: *mut c_void,
        element: *mut c_void,
        notification: CFStringRef,
    ) {
        unsafe extern "C" {
            fn AXUIElementCopyAttributeValue(
                element: *mut c_void,
                attr: CFStringRef,
                value: *mut CFTypeRef,
            ) -> i32;
            fn AXObserverAddNotification(
                observer: *mut c_void,
                element: *mut c_void,
                notification: CFStringRef,
                refcon: *mut c_void,
            ) -> i32;
            fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> bool;
        }
        fn cfstring_to_string(s: CFStringRef) -> String {
            // SAFETY: CFStringRef obtained from system APIs per get rule
            let cf = unsafe { core_foundation::string::CFString::wrap_under_get_rule(s) };
            cf.to_string()
        }
        unsafe {
            // Determine notification type by equality against our CFStrings
            if CFEqual(
                notification as CFTypeRef,
                self.notif_focused_window_changed.as_concrete_TypeRef() as CFTypeRef,
            ) {
                // Get the focused window and its title
                let mut win_ref: CFTypeRef = std::ptr::null_mut();
                let err = AXUIElementCopyAttributeValue(
                    self.app_elem,
                    self.attr_focused_window.as_concrete_TypeRef(),
                    &mut win_ref,
                );
                if err != 0 {
                    warn!(
                        "AXUIElementCopyAttributeValue(focused_window) failed: {}",
                        err
                    );
                }
                if !win_ref.is_null() {
                    // Fetch title
                    let mut title_ref: CFTypeRef = std::ptr::null_mut();
                    let err = AXUIElementCopyAttributeValue(
                        win_ref as *mut c_void,
                        self.attr_title.as_concrete_TypeRef(),
                        &mut title_ref,
                    );
                    if err != 0 {
                        warn!("AXUIElementCopyAttributeValue(title) failed: {}", err);
                    }
                    if !title_ref.is_null() {
                        let s = cfstring_to_string(title_ref as CFStringRef);
                        let _ = self.tx.send(FocusEvent::TitleChanged {
                            title: s,
                            pid: self.pid,
                        });
                        CFRelease(title_ref);
                    }
                    // Begin observing title changes on this focused window as well
                    let err = AXObserverAddNotification(
                        observer,
                        win_ref as *mut c_void,
                        self.notif_title_changed.as_concrete_TypeRef(),
                        self as *const _ as *mut c_void,
                    );
                    if err != 0 {
                        warn!("AXObserverAddNotification(title_changed) failed: {}", err);
                    }
                    CFRelease(win_ref);
                }
            } else if CFEqual(
                notification as CFTypeRef,
                self.notif_title_changed.as_concrete_TypeRef() as CFTypeRef,
            ) {
                // Title changed for current window
                let mut title_ref: CFTypeRef = std::ptr::null_mut();
                let err = AXUIElementCopyAttributeValue(
                    element,
                    self.attr_title.as_concrete_TypeRef(),
                    &mut title_ref,
                );
                if err != 0 {
                    warn!("AXUIElementCopyAttributeValue(title) failed: {}", err);
                }
                if !title_ref.is_null() {
                    let s = cfstring_to_string(title_ref as CFStringRef);
                    let _ = self.tx.send(FocusEvent::TitleChanged {
                        title: s,
                        pid: self.pid,
                    });
                    CFRelease(title_ref);
                }
            }
        }
    }
}
