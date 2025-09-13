//! Per‑PID Accessibility (AX) observer scaffolding.
#![allow(clippy::arc_with_non_send_sync)]
//!
//! Provides a minimal, safe wrapper around `AXObserver` that:
//! - Creates one observer per PID.
//! - Adds the observer's runloop source exactly once to the current run loop.
//! - Supports idempotent installation and clean removal on drop.
//!
//! Stage 4.1 focuses on lifecycle; richer events and consumers follow later.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    ffi::c_void,
    sync::Arc,
};

use core_foundation::{
    base::{CFRelease, CFTypeRef, TCFType},
    runloop::{CFRunLoopGetCurrent, CFRunLoopSourceRef, kCFRunLoopDefaultMode},
    string::{CFString, CFStringRef},
};
use parking_lot::Mutex;
use tracing::warn;

use crate::AXElem;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXObserverCreate(
        pid: i32,
        callback: extern "C" fn(*mut c_void, *mut c_void, CFStringRef, *mut c_void),
        out: *mut *mut c_void,
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
    fn AXObserverGetRunLoopSource(observer: *mut c_void) -> *mut c_void;

    fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> bool;
    fn CFRunLoopAddSource(rl: *mut c_void, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRemoveSource(rl: *mut c_void, source: CFRunLoopSourceRef, mode: CFStringRef);
}

/// CF-backed RAII for AXObserverRef.
struct AxObserver(*mut c_void);
impl AxObserver {
    #[inline]
    fn from_create(ptr: *mut c_void) -> Option<Self> {
        if ptr.is_null() { None } else { Some(Self(ptr)) }
    }
    #[inline]
    fn as_ptr(&self) -> *mut c_void {
        self.0
    }
}
impl Drop for AxObserver {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0 as CFTypeRef) }
    }
}

/// Richer event type for AX notifications.
#[derive(Debug, Clone)]
pub enum AxEventKind {
    Created,
    Destroyed,
    Focused,
    Moved,
    Resized,
    TitleChanged,
}

#[derive(Debug, Clone, Default)]
pub struct WindowHint {
    pub frame: Option<crate::geom::Rect>,
    pub title: Option<String>,
    pub role: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AxEvent {
    pub pid: i32,
    pub kind: AxEventKind,
    pub hint: WindowHint,
}

/// Internal context passed to the AX callback (holds strings, tx, and state).
struct Ctx {
    pid: i32,
    tx: Option<crossbeam_channel::Sender<AxEvent>>,
    // App element for attribute queries like AXFocusedWindow
    app_elem: *mut c_void,
    // Notifications
    notif_window_created: CFString,
    notif_ui_elem_destroyed: CFString,
    notif_focused_window_changed: CFString,
    notif_moved: CFString,
    notif_resized: CFString,
    notif_title_changed: CFString,
    // Attributes
    attr_focused_window: CFString,
    attr_title: CFString,
    attr_role: CFString,
    attr_subrole: CFString,
    attr_position: CFString,
    attr_size: CFString,
    // Track one window we attach to for moved/resized/title
    observed_window: RefCell<Option<AXElem>>,
}

extern "C" fn ax_callback(
    _observer: *mut c_void,
    element: *mut c_void,
    notification: CFStringRef,
    refcon: *mut c_void,
) {
    // Stage 4.2: translate a subset of notifications into AxEvent and send via channel.
    unsafe {
        if refcon.is_null() {
            return;
        }
        let ctx = &*(refcon as *mut Ctx);
        let send = |kind: AxEventKind, hint: WindowHint| {
            if let Some(tx) = &ctx.tx {
                let _ = tx.send(AxEvent {
                    pid: ctx.pid,
                    kind,
                    hint,
                });
            }
        };

        // Helper: collect a WindowHint from a given element
        let hint_for = |el: *mut c_void| -> WindowHint {
            let mut hint = WindowHint::default();
            if el.is_null() {
                return hint;
            }
            // Title
            hint.title = crate::ax::ax_get_string(el, ctx.attr_title.as_concrete_TypeRef());
            // Role/Subrole
            let role = crate::ax::ax_get_string(el, ctx.attr_role.as_concrete_TypeRef());
            let subrole = crate::ax::ax_get_string(el, ctx.attr_subrole.as_concrete_TypeRef());
            hint.role = match (role, subrole) {
                (Some(r), Some(sr)) if !sr.is_empty() => Some(format!("{r}:{sr}")),
                (Some(r), _) => Some(r),
                _ => None,
            };
            // Frame
            let p = crate::ax::ax_get_point(el, ctx.attr_position.as_concrete_TypeRef()).ok();
            let s = crate::ax::ax_get_size(el, ctx.attr_size.as_concrete_TypeRef()).ok();
            if let (Some(p), Some(s)) = (p, s) {
                hint.frame = Some(crate::geom::Rect {
                    x: p.x,
                    y: p.y,
                    w: s.width,
                    h: s.height,
                });
            }
            hint
        };

        // Equals helper
        let equals = |a: CFStringRef| -> bool { CFEqual(notification as _, a as _) };

        if equals(ctx.notif_focused_window_changed.as_concrete_TypeRef()) {
            // Get the new focused window and attach window-specific notifications
            let mut win_ref: CFTypeRef = std::ptr::null_mut();
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
            }
            let err = AXUIElementCopyAttributeValue(
                ctx.app_elem,
                ctx.attr_focused_window.as_concrete_TypeRef(),
                &mut win_ref,
            );
            // Detach previous window notifications
            if let Some(prev) = ctx.observed_window.borrow_mut().take() {
                let _ = AXObserverRemoveNotification(
                    _observer,
                    prev.as_ptr(),
                    ctx.notif_title_changed.as_concrete_TypeRef(),
                );
                let _ = AXObserverRemoveNotification(
                    _observer,
                    prev.as_ptr(),
                    ctx.notif_moved.as_concrete_TypeRef(),
                );
                let _ = AXObserverRemoveNotification(
                    _observer,
                    prev.as_ptr(),
                    ctx.notif_resized.as_concrete_TypeRef(),
                );
            }
            if err == 0 && !win_ref.is_null() {
                if let Some(win_elem) = AXElem::from_create(win_ref as *mut c_void) {
                    // Attach window-specific notifications
                    let _ = AXObserverAddNotification(
                        _observer,
                        win_elem.as_ptr(),
                        ctx.notif_title_changed.as_concrete_TypeRef(),
                        refcon,
                    );
                    let _ = AXObserverAddNotification(
                        _observer,
                        win_elem.as_ptr(),
                        ctx.notif_moved.as_concrete_TypeRef(),
                        refcon,
                    );
                    let _ = AXObserverAddNotification(
                        _observer,
                        win_elem.as_ptr(),
                        ctx.notif_resized.as_concrete_TypeRef(),
                        refcon,
                    );
                    *ctx.observed_window.borrow_mut() = Some(win_elem.clone());
                    send(AxEventKind::Focused, hint_for(win_elem.as_ptr()));
                }
            } else {
                // Focus cleared; still emit an event with empty hint
                send(AxEventKind::Focused, WindowHint::default());
            }
            return;
        }

        if equals(ctx.notif_title_changed.as_concrete_TypeRef()) {
            send(AxEventKind::TitleChanged, hint_for(element));
            return;
        }
        if equals(ctx.notif_moved.as_concrete_TypeRef()) {
            send(AxEventKind::Moved, hint_for(element));
            return;
        }
        if equals(ctx.notif_resized.as_concrete_TypeRef()) {
            send(AxEventKind::Resized, hint_for(element));
            return;
        }
        if equals(ctx.notif_window_created.as_concrete_TypeRef()) {
            send(AxEventKind::Created, hint_for(element));
            return;
        }
        if equals(ctx.notif_ui_elem_destroyed.as_concrete_TypeRef()) {
            send(AxEventKind::Destroyed, WindowHint::default());
        }
    }
}

/// One installed observer per PID with bookkeeping to ensure single runloop source.
struct PerPid {
    pid: i32,
    observer: AxObserver,
    app: AXElem,
    // Runloop bookkeeping
    rl: *mut c_void,            // CFRunLoopRef used for add/remove
    source: CFRunLoopSourceRef, // observer source
    source_added: bool,
    // Subscriptions we have installed on the app/window elements.
    subs: HashSet<&'static str>,
    // Opaque context pointer for callback; reclaimed on drop.
    ctx_ptr: *mut c_void,
}

impl PerPid {
    fn install(pid: i32, tx: Option<crossbeam_channel::Sender<AxEvent>>) -> Result<Self, i32> {
        unsafe {
            let mut obs_ptr: *mut c_void = std::ptr::null_mut();
            let err = AXObserverCreate(pid, ax_callback, &mut obs_ptr as *mut _);
            if err != 0 || obs_ptr.is_null() {
                return Err(err);
            }
            let Some(observer) = AxObserver::from_create(obs_ptr) else {
                return Err(err);
            };
            let Some(app) = AXElem::from_create(AXUIElementCreateApplication(pid)) else {
                return Err(-1);
            };
            // Runloop source
            let src = AXObserverGetRunLoopSource(observer.as_ptr()) as CFRunLoopSourceRef;
            if src.is_null() {
                return Err(-1);
            }
            // Create rich context for future use
            let ctx = Box::into_raw(Box::new(Ctx {
                pid,
                tx,
                app_elem: app.as_ptr(),
                notif_window_created: CFString::from_static_string("AXWindowCreated"),
                notif_ui_elem_destroyed: CFString::from_static_string("AXUIElementDestroyed"),
                notif_focused_window_changed: CFString::from_static_string(
                    "AXFocusedWindowChanged",
                ),
                notif_moved: CFString::from_static_string("AXMoved"),
                notif_resized: CFString::from_static_string("AXResized"),
                notif_title_changed: CFString::from_static_string("AXTitleChanged"),
                attr_focused_window: CFString::from_static_string("AXFocusedWindow"),
                attr_title: CFString::from_static_string("AXTitle"),
                attr_role: CFString::from_static_string("AXRole"),
                attr_subrole: CFString::from_static_string("AXSubrole"),
                attr_position: CFString::from_static_string("AXPosition"),
                attr_size: CFString::from_static_string("AXSize"),
                observed_window: RefCell::new(None),
            })) as *mut c_void;
            // Add source to current run loop exactly once
            let rl = CFRunLoopGetCurrent() as *mut c_void;
            let mode = kCFRunLoopDefaultMode;
            CFRunLoopAddSource(rl, src, mode);
            Ok(Self {
                pid,
                observer,
                app,
                rl,
                source: src,
                source_added: true,
                subs: HashSet::new(),
                ctx_ptr: ctx,
            })
        }
    }

    /// Returns true if the underlying process still appears to be the same
    /// and the AX application element equals the stored one.
    fn still_same_process(&self) -> bool {
        // Check liveness: kill(pid, 0) returns Ok if process exists.
        let alive = unsafe { libc::kill(self.pid as libc::pid_t, 0) == 0 };
        if !alive {
            return false;
        }
        // Create a fresh app element and compare equality with stored one.
        unsafe {
            let new_app = AXUIElementCreateApplication(self.pid);
            if new_app.is_null() {
                return false;
            }
            let equal = CFEqual(self.app.as_ptr() as CFTypeRef, new_app as CFTypeRef);
            CFRelease(new_app as CFTypeRef);
            equal
        }
    }

    fn subscribe_app(&mut self, name: &'static str) -> Result<(), i32> {
        if self.subs.contains(name) {
            return Ok(());
        }
        let cf = CFString::from_static_string(name);
        let err = unsafe {
            AXObserverAddNotification(
                self.observer.as_ptr(),
                self.app.as_ptr(),
                cf.as_concrete_TypeRef(),
                self.ctx_ptr,
            )
        };
        match err {
            0 | -25209 /* NotificationAlreadyRegistered */ => {
                self.subs.insert(name);
                Ok(())
            }
            e => Err(e),
        }
    }
}

impl Drop for PerPid {
    fn drop(&mut self) {
        unsafe {
            // Best-effort: detach any window-specific notifications
            if let Some(prev) = self
                .ctx_ptr
                .cast::<Ctx>()
                .as_ref()
                .and_then(|c| c.observed_window.borrow().clone())
            {
                let ctx = &*(self.ctx_ptr as *mut Ctx);
                let _ = AXObserverRemoveNotification(
                    self.observer.as_ptr(),
                    prev.as_ptr(),
                    ctx.notif_title_changed.as_concrete_TypeRef(),
                );
                let _ = AXObserverRemoveNotification(
                    self.observer.as_ptr(),
                    prev.as_ptr(),
                    ctx.notif_moved.as_concrete_TypeRef(),
                );
                let _ = AXObserverRemoveNotification(
                    self.observer.as_ptr(),
                    prev.as_ptr(),
                    ctx.notif_resized.as_concrete_TypeRef(),
                );
            }
            // Best-effort: unsubscribe app-level notifications we've tracked
            for name in self.subs.drain() {
                let cf = CFString::from_static_string(name);
                let _ = AXObserverRemoveNotification(
                    self.observer.as_ptr(),
                    self.app.as_ptr(),
                    cf.as_concrete_TypeRef(),
                );
            }
            if self.source_added {
                CFRunLoopRemoveSource(self.rl, self.source, kCFRunLoopDefaultMode);
            }
            if !self.ctx_ptr.is_null() {
                let _ = Box::<Ctx>::from_raw(self.ctx_ptr as *mut Ctx);
                self.ctx_ptr = std::ptr::null_mut();
            }
        }
    }
}

/// Thread-safe registry for per‑PID observers.
#[allow(clippy::arc_with_non_send_sync)]
pub struct AxObserverRegistry {
    inner: Arc<Mutex<HashMap<i32, PerPid>>>,
    tx: Arc<Mutex<Option<crossbeam_channel::Sender<AxEvent>>>>,
}

impl Default for AxObserverRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            tx: Arc::new(Mutex::new(None)),
        }
    }
}

impl AxObserverRegistry {
    /// Set a sender to receive `AxEvent`s for all observers created subsequently.
    pub fn set_sender(&self, sender: crossbeam_channel::Sender<AxEvent>) {
        *self.tx.lock() = Some(sender);
    }
    /// Ensure an observer exists for `pid`. Returns `true` if newly created.
    pub fn ensure(&self, pid: i32) -> Result<bool, i32> {
        let mut m = self.inner.lock();
        if let Some(existing) = m.get(&pid) {
            if existing.still_same_process() {
                return Ok(false);
            }
            // Stale or reused PID; drop the existing observer.
            m.remove(&pid);
        }
        let tx = self.tx.lock().clone();
        let mut p = PerPid::install(pid, tx)?;
        // Minimal app-level subscriptions; window-level added on focus change.
        for name in [
            "AXWindowCreated",
            "AXFocusedWindowChanged",
            "AXUIElementDestroyed",
        ] {
            if let Err(e) = p.subscribe_app(name) {
                warn!(
                    "AXObserverAddNotification({}, pid={}) failed: {}",
                    name, pid, e
                );
            }
        }
        m.insert(pid, p);
        Ok(true)
    }

    /// Remove and drop the observer for `pid` if present.
    pub fn remove(&self, pid: i32) -> bool {
        self.inner.lock().remove(&pid).is_some()
    }

    /// Number of installed observers (for tests/diagnostics).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: skip tests when Accessibility is not granted.
    fn ax_ok() -> bool {
        permissions::accessibility_ok()
    }

    #[test]
    fn install_idempotent_and_drop_removes() {
        // If no AX permission, this test is a no‑op to avoid spurious failures on CI.
        if !ax_ok() {
            eprintln!("skipping: Accessibility permission not granted");
            return;
        }
        let reg = AxObserverRegistry::default();
        // Hook a bounded channel to receive events; drop receiver immediately to avoid blocking
        let (tx, _rx) = crossbeam_channel::bounded::<AxEvent>(8);
        reg.set_sender(tx);
        let pid = std::process::id() as i32;
        let first = reg.ensure(pid).expect("ensure ok (first)");
        assert!(first, "first ensure creates the observer");
        let second = reg.ensure(pid).expect("ensure ok (second)");
        assert!(!second, "second ensure is idempotent");
        assert_eq!(reg.len(), 1, "exactly one observer installed");
        // Remove and ensure map is empty; Drop should remove the runloop source.
        assert!(reg.remove(pid));
        assert!(reg.is_empty());
    }
}
