use std::{
    cell::RefCell,
    ffi::{CStr, c_void},
};

use core_foundation::{
    base::{CFRelease, CFTypeRef, TCFType},
    runloop::{CFRunLoop, CFRunLoopSource, CFRunLoopSourceRef, kCFRunLoopDefaultMode},
    string::{CFString, CFStringRef},
};
use objc2_app_kit::NSRunningApplication;
use thiserror::Error;
use tokio::sync::mpsc::UnboundedSender;
use tracing::warn;

pub(crate) enum AxEvent {
    TitleChanged { title: String, pid: i32 },
}

/// Returns true if the process is trusted for Accessibility (AX) APIs.
pub(crate) fn ax_is_trusted() -> bool {
    permissions::accessibility_ok()
}

#[derive(Default)]
pub(crate) struct AXState {
    observer: Option<AXObserver>,
    app_elem: Option<crate::AXElem>,
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

/// RAII guard for AXObserverRef.
struct AXObserver(*mut c_void);
impl AXObserver {
    #[inline]
    fn from_create(ptr: *mut c_void) -> Option<Self> {
        if ptr.is_null() { None } else { Some(Self(ptr)) }
    }
    #[inline]
    fn as_ptr(&self) -> *mut c_void {
        self.0
    }
}
impl Drop for AXObserver {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0 as CFTypeRef) }
    }
}

impl AXState {
    pub(crate) fn detach(&mut self) {
        unsafe {
            if !self.ctx_ptr.is_null() {
                let _ = Box::<AXCtx>::from_raw(self.ctx_ptr as *mut AXCtx);
            }
        }
        // Drop RAII wrappers to release CF objects.
        self.observer = None;
        self.app_elem = None;
        self.have_source = false;
        self.ctx_ptr = std::ptr::null_mut();
    }

    pub(crate) fn attach(&mut self, pid: i32, tx: UnboundedSender<AxEvent>) -> Result<(), Error> {
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
            let mut observer_ptr: *mut c_void = std::ptr::null_mut();
            let err = AXObserverCreate(pid, ax_callback, &mut observer_ptr as *mut _);
            if err != 0 || observer_ptr.is_null() {
                return Err(Error::ObserverCreate(err));
            }
            let Some(app_elem) = crate::AXElem::from_create(AXUIElementCreateApplication(pid))
            else {
                return Err(Error::AppElementNull);
            };
            let Some(observer) = AXObserver::from_create(observer_ptr) else {
                return Err(Error::ObserverCreate(err));
            };

            // Create CFString constants (non-owning from 'static strs)
            let notif_focused_window_changed =
                CFString::from_static_string("AXFocusedWindowChanged");
            let notif_title_changed = CFString::from_static_string("AXTitleChanged");
            let attr_focused_window = CFString::from_static_string("AXFocusedWindow");
            let attr_title = CFString::from_static_string("AXTitle");

            // Create context used by callback (contains tx, app element, and CFStrings)
            let ctx = Box::new(AXCtx {
                tx,
                app_elem: app_elem.as_ptr(),
                notif_focused_window_changed,
                notif_title_changed,
                attr_focused_window,
                attr_title,
                pid,
                observed_window: RefCell::new(None),
            });
            // Capture notification ref before moving ctx into raw
            let notif = ctx.notif_focused_window_changed.as_concrete_TypeRef();
            let ctx_ptr = Box::into_raw(ctx) as *mut c_void;

            // Observe focused window changes on the app
            let err =
                AXObserverAddNotification(observer.as_ptr(), app_elem.as_ptr(), notif, ctx_ptr);
            if err != 0 {
                let _ = Box::<AXCtx>::from_raw(ctx_ptr as *mut AXCtx);
                return Err(if err == AX_ERR_NOTIFICATION_ALREADY_REGISTERED {
                    Error::AddNotificationAlreadyRegistered
                } else {
                    Error::AddNotification(err)
                });
            }

            let source = AXObserverGetRunLoopSource(observer.as_ptr());
            if source.is_null() {
                let _ = Box::<AXCtx>::from_raw(ctx_ptr as *mut AXCtx);
                return Err(Error::GetRunLoopSourceNull);
            }
            let rl = CFRunLoop::get_current();
            let mode = kCFRunLoopDefaultMode;
            let source_ref = source as CFRunLoopSourceRef;
            let source_obj = CFRunLoopSource::wrap_under_get_rule(source_ref);
            rl.add_source(&source_obj, mode);

            self.observer = Some(observer);
            self.app_elem = Some(app_elem);
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

/// Read the system-wide focused application and focused window title via AX.
/// Returns (app_name, window_title, pid) if available. The app_name may be
/// empty if an application name cannot be determined cheaply; callers should
/// primarily rely on pid + title for identity.
pub(crate) fn system_focus_snapshot() -> Option<(String, String, i32)> {
    if !ax_is_trusted() {
        return None;
    }
    unsafe extern "C" {
        fn AXUIElementCreateSystemWide() -> *mut c_void;
        fn AXUIElementCopyAttributeValue(
            element: *mut c_void,
            attr: CFStringRef,
            value: *mut CFTypeRef,
        ) -> i32;
        fn AXUIElementGetPid(element: *mut c_void, pid: *mut i32) -> i32;
    }
    unsafe {
        let sys_ptr = AXUIElementCreateSystemWide();
        if sys_ptr.is_null() {
            return None;
        }
        let sys = crate::AXElem::from_create(sys_ptr).expect("non-null just checked");
        let attr_focused_app = CFString::from_static_string("AXFocusedApplication");
        let attr_focused_window = CFString::from_static_string("AXFocusedWindow");
        let attr_title = CFString::from_static_string("AXTitle");

        let mut app_ref: CFTypeRef = std::ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(
            sys.as_ptr(),
            attr_focused_app.as_concrete_TypeRef(),
            &mut app_ref,
        );
        if err != 0 || app_ref.is_null() {
            return None;
        }
        // Wrap application element under create rule (Copy -> owned)
        let app_elem = crate::AXElem::from_create(app_ref as *mut c_void)?;
        // Resolve pid
        let mut pid_out: i32 = -1;
        let _ = AXUIElementGetPid(app_elem.as_ptr(), &mut pid_out as *mut i32);
        // Resolve application name via NSRunningApplication
        let app_name: String = match NSRunningApplication::runningApplicationWithProcessIdentifier(
            pid_out as libc::pid_t,
        ) {
            Some(app) => match app.localizedName() {
                Some(ns) => {
                    let ptr = ns.UTF8String();
                    if ptr.is_null() {
                        String::new()
                    } else {
                        CStr::from_ptr(ptr).to_string_lossy().into_owned()
                    }
                }
                None => String::new(),
            },
            None => String::new(),
        };

        // Focused window and its title
        let mut win_ref: CFTypeRef = std::ptr::null_mut();
        let werr = AXUIElementCopyAttributeValue(
            app_elem.as_ptr(),
            attr_focused_window.as_concrete_TypeRef(),
            &mut win_ref,
        );
        if werr != 0 || win_ref.is_null() {
            // No focused window; still return app and empty title
            return Some((app_name, String::new(), pid_out));
        }
        // Wrap focused window element under create rule
        let win_elem = match crate::AXElem::from_create(win_ref as *mut c_void) {
            Some(w) => w,
            None => return Some((app_name, String::new(), pid_out)),
        };
        let mut title_ref: CFTypeRef = std::ptr::null_mut();
        let terr = AXUIElementCopyAttributeValue(
            win_elem.as_ptr(),
            attr_title.as_concrete_TypeRef(),
            &mut title_ref,
        );
        if terr != 0 || title_ref.is_null() {
            return Some((app_name, String::new(), pid_out));
        }
        let cfs = core_foundation::string::CFString::wrap_under_create_rule(title_ref as _);
        let title = cfs.to_string();
        Some((app_name, title, pid_out))
    }
}

struct AXCtx {
    tx: UnboundedSender<AxEvent>,
    app_elem: *mut c_void,
    notif_focused_window_changed: CFString,
    notif_title_changed: CFString,
    attr_focused_window: CFString,
    attr_title: CFString,
    pid: i32,
    observed_window: RefCell<Option<crate::AXElem>>,
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
            fn AXObserverRemoveNotification(
                observer: *mut c_void,
                element: *mut c_void,
                notification: CFStringRef,
            ) -> i32;
            fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> bool;
        }
        use crate::cfutil::cfstring_to_string_copy;
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
                // Always remove previous title observer if present
                if let Some(prev) = self.observed_window.borrow_mut().take() {
                    let _ = AXObserverRemoveNotification(
                        observer,
                        prev.as_ptr(),
                        self.notif_title_changed.as_concrete_TypeRef(),
                    );
                    // prev dropped here
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
                        let s = cfstring_to_string_copy(title_ref as CFStringRef);
                        let _ = self.tx.send(AxEvent::TitleChanged {
                            title: s,
                            pid: self.pid,
                        });
                    }
                    // Begin observing title changes on this focused window as well
                    if let Some(new_win) = crate::AXElem::from_create(win_ref as *mut c_void) {
                        let err = AXObserverAddNotification(
                            observer,
                            new_win.as_ptr(),
                            self.notif_title_changed.as_concrete_TypeRef(),
                            self as *const _ as *mut c_void,
                        );
                        if err != 0 {
                            warn!("AXObserverAddNotification(title_changed) failed: {}", err);
                        } else {
                            *self.observed_window.borrow_mut() = Some(new_win);
                        }
                    } else {
                        // If wrapping failed, release the raw reference to avoid leaking
                        CFRelease(win_ref);
                    }
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
                    let s = cfstring_to_string_copy(title_ref as CFStringRef);
                    let _ = self.tx.send(AxEvent::TitleChanged {
                        title: s,
                        pid: self.pid,
                    });
                }
            }
        }
    }
}
