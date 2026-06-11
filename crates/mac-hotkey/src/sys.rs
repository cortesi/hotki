//! macOS event tap (CoreGraphics) integration for hotkey interception.
//!
//! Why we use `core-graphics` for event taps:
//! - Some wrappers expose a Rust callback like `FnMut(..) -> Option<CGEvent>`,
//!   where returning `None` is meant to “swallow” the event. If the wrapper maps
//!   `None` to the original `CGEventRef` (instead of a NULL), the OS still delivers
//!   the keystroke. CoreGraphics only suppresses delivery if the tap returns NULL.
//! - The `core-graphics` crate’s `CGEventTap` uses a `CallbackResult` where `Drop`
//!   maps to a NULL `CGEventRef` at the C boundary, matching CoreGraphics’ contract.
//!   We return `CallbackResult::Drop` for intercepted events so they never reach the
//!   foreground app.

use std::{
    cell::RefCell,
    collections::HashSet,
    ffi::c_void,
    process, ptr,
    sync::{
        Arc,
        atomic::{AtomicPtr, Ordering},
    },
};

use core_foundation::{
    base::TCFType,
    mach_port::CFMachPortRef,
    runloop::{CFRunLoop, kCFRunLoopCommonModes},
};
use core_graphics::event::{self as cge, CallbackResult};
use crossbeam_channel::Sender;
use mac_keycode::{Key, Modifier};
use parking_lot::Mutex;
use tracing::{debug, trace, warn};

use crate::{Event, EventKind, policy};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
}

// Minimal subset of CGEventField constants used by this module.
const FIELD_EVENT_SOURCE_UNIX_PROCESS_ID: u32 = 41;
const FIELD_EVENT_SOURCE_USER_DATA: u32 = 42;
const FIELD_KEYBOARD_EVENT_AUTOREPEAT: u32 = 8;
const FIELD_KEYBOARD_EVENT_KEYCODE: u32 = 9;

#[derive(Debug)]
struct TapAction {
    event: Option<Event>,
    intercept: bool,
}

fn classify_tap_event(
    inner: &crate::Inner,
    code: Key,
    mods: &HashSet<Modifier>,
    kind: EventKind,
    is_repeat: bool,
    held_intercepts: &mut HashSet<Key>,
) -> TapAction {
    let matched = crate::match_event(inner, code, mods);
    let matched_intercept = matched.as_ref().map(|(_, reg)| reg.intercept);
    let suspended = inner.suspend > 0;
    let capture = inner.capture_all > 0;
    let mut decision = policy::classify(suspended, matched_intercept);

    if !suspended && capture {
        decision.intercept = true;
        decision.emit = matched.is_some();
    }

    match kind {
        EventKind::KeyDown => {
            if decision.intercept && !is_repeat {
                held_intercepts.insert(code);
            } else if is_repeat && held_intercepts.contains(&code) {
                decision.intercept = true;
            }
        }
        EventKind::KeyUp => {
            if held_intercepts.remove(&code) {
                decision.intercept = true;
            }
        }
    }

    let event = if decision.emit {
        matched.map(|(id, reg)| Event {
            id,
            hotkey: reg.hotkey,
            kind,
            repeat: is_repeat,
        })
    } else {
        None
    };

    TapAction {
        event,
        intercept: decision.intercept,
    }
}

// Shared control handle to stop the run loop from other threads.
pub(crate) struct SysControl {
    rl: Mutex<Option<CFRunLoop>>,
}

impl SysControl {
    pub(crate) fn new() -> Self {
        Self {
            rl: Mutex::new(None),
        }
    }

    pub(crate) fn set_rl(&self, rl: CFRunLoop) {
        let mut g = self.rl.lock();
        *g = Some(rl);
    }

    pub(crate) fn stop(&self) {
        let mut g = self.rl.lock();
        if let Some(rl) = g.take() {
            rl.stop();
        }
    }
}

pub fn run_event_loop(
    inner_state: Arc<arc_swap::ArcSwap<crate::Inner>>,
    tx: Sender<Event>,
    ready: Sender<crate::Result<()>>,
    ctrl: Arc<SysControl>,
) -> crate::Result<()> {
    // Preflight Input Monitoring permission.
    if !permissions::input_monitoring_ok() {
        warn!("input_monitoring_permission_missing");
        let _ = ready.send(Err(crate::Error::PermissionDenied("Input Monitoring")));
        return Err(crate::Error::PermissionDenied("Input Monitoring"));
    }

    // Capture for re-enabling the tap from inside the closure.
    let tap_port_ptr: Arc<AtomicPtr<c_void>> = Arc::new(AtomicPtr::new(ptr::null_mut()));

    debug!("creating_event_tap");
    let tap_port_ptr_cb = tap_port_ptr.clone();
    let held_intercepts: RefCell<HashSet<Key>> = RefCell::new(HashSet::new());
    let tap = match cge::CGEventTap::new(
        cge::CGEventTapLocation::HID,
        cge::CGEventTapPlacement::HeadInsertEventTap,
        cge::CGEventTapOptions::Default,
        vec![cge::CGEventType::KeyDown, cge::CGEventType::KeyUp],
        move |_proxy, etype, event| {
            // Ignore events we injected ourselves either by PID or by custom tag.
            let src_pid = event.get_integer_value_field(FIELD_EVENT_SOURCE_UNIX_PROCESS_ID) as u32;
            let user_tag = event.get_integer_value_field(FIELD_EVENT_SOURCE_USER_DATA);
            if user_tag == crate::HOTK_TAG || src_pid == process::id() {
                trace!(src_pid, user_tag, "ignoring_synthetic_event");
                return CallbackResult::Keep;
            }

            {
                let inner = inner_state.load();
                if inner.suspend > 0 {
                    trace!("tap_suspended_skipping_event");
                    return CallbackResult::Keep;
                }
            }

            match etype {
                cge::CGEventType::KeyDown | cge::CGEventType::KeyUp => {
                    let keycode =
                        event.get_integer_value_field(FIELD_KEYBOARD_EVENT_KEYCODE) as u16;
                    if let Some(code) = Key::from_scancode(keycode) {
                        let flags = event.get_flags().bits();
                        let mods = mac_keycode::modifiers_from_cg_flags(flags);
                        let kind = if matches!(etype, cge::CGEventType::KeyDown) {
                            EventKind::KeyDown
                        } else {
                            EventKind::KeyUp
                        };
                        let is_repeat = matches!(etype, cge::CGEventType::KeyDown)
                            && event.get_integer_value_field(FIELD_KEYBOARD_EVENT_AUTOREPEAT) != 0;

                        trace!(
                            scancode = keycode,
                            flags,
                            code = ?code,
                            mods = ?mods,
                            ?kind,
                            is_repeat,
                            src_pid,
                            "tap_event"
                        );

                        let action = {
                            let inner = inner_state.load();
                            let mut held = held_intercepts.borrow_mut();
                            classify_tap_event(
                                inner.as_ref(),
                                code,
                                &mods,
                                kind,
                                is_repeat,
                                &mut held,
                            )
                        };

                        if let Some(event) = action.event {
                            let _ = tx.send(event);
                        }

                        if action.intercept {
                            trace!("intercepting_event");
                            return CallbackResult::Drop;
                        }
                    }
                    CallbackResult::Keep
                }
                cge::CGEventType::TapDisabledByTimeout
                | cge::CGEventType::TapDisabledByUserInput => {
                    let p = tap_port_ptr_cb.load(Ordering::SeqCst) as CFMachPortRef;
                    if !p.is_null() {
                        warn!("tap_disabled_by_os_reenabling");
                        // SAFETY: `p` is a valid CFMachPortRef because it was created by
                        // `CGEventTap::new` and its pointer was stored in `tap_port_ptr_cb`
                        // before the run loop started. It remains valid for the lifetime of
                        // the tap, which is tied to the thread running this callback.
                        unsafe { CGEventTapEnable(p, true) };
                    }
                    CallbackResult::Keep
                }
                _ => CallbackResult::Keep,
            }
        },
    ) {
        Ok(t) => t,
        Err(_) => {
            if !permissions::accessibility_ok() {
                warn!("accessibility_permission_missing_for_event_tap");
                let _ = ready.send(Err(crate::Error::PermissionDenied("Accessibility")));
                return Err(crate::Error::PermissionDenied("Accessibility"));
            }
            warn!("event_tap_create_failed");
            let _ = ready.send(Err(crate::Error::EventTapStart));
            return Err(crate::Error::EventTapStart);
        }
    };

    // Share the CFMachPort for re-enabling inside the callback.
    tap_port_ptr.store(
        tap.mach_port().as_concrete_TypeRef() as *mut c_void,
        Ordering::SeqCst,
    );

    // Create a runloop source and start the tap on this thread's runloop.
    let source = match tap.mach_port().create_runloop_source(0) {
        Ok(s) => s,
        Err(_) => {
            warn!("run_loop_source_create_failed");
            let _ = ready.send(Err(crate::Error::EventTapStart));
            return Err(crate::Error::EventTapStart);
        }
    };

    let rl = CFRunLoop::get_current();
    ctrl.set_rl(rl.clone());
    // SAFETY: `kCFRunLoopCommonModes` is a process-lifetime CoreFoundation mode
    // constant. We borrow it only long enough to register this source.
    let mode = unsafe { kCFRunLoopCommonModes };
    rl.add_source(&source, mode);

    // Enable the tap and run the loop.
    tap.enable();

    let _ = ready.send(Ok(()));
    debug!("event_tap_started_run_loop");

    CFRunLoop::run_current();

    debug!("event_tap_exited");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use mac_keycode::{Key, Modifier};

    use super::classify_tap_event;
    use crate::{EventKind, test_register};

    fn simulate(suspended: bool, matched: bool, intercept: bool) -> crate::policy::Decision {
        let matched_intercept = if matched { Some(intercept) } else { None };
        crate::policy::classify(suspended, matched_intercept)
    }

    #[test]
    fn tap_policy_sim_intercept_tracks_option() {
        let d = simulate(false, true, true);
        assert!(d.emit);
        assert!(d.intercept);
        let d = simulate(false, true, false);
        assert!(d.emit);
        assert!(!d.intercept);
    }

    #[test]
    fn tap_policy_sim_suspended_and_nomatch() {
        let d = simulate(true, true, true);
        assert!(!d.emit);
        assert!(!d.intercept);
        let d = simulate(false, false, true);
        assert!(!d.emit);
        assert!(!d.intercept);
    }

    fn registered_inner(intercept: bool) -> (crate::Inner, HashSet<Modifier>, u32) {
        let mut inner = crate::Inner::default();
        let modifiers = HashSet::from([Modifier::Control]);
        let hotkey = mac_keycode::Chord {
            key: Key::H,
            modifiers: modifiers.clone(),
        };
        let id = test_register(&mut inner, hotkey, intercept);
        (inner, modifiers, id)
    }

    #[test]
    fn end_to_end_match_then_emit_shape() {
        let (inner, mods, _id) = registered_inner(true);
        let matched = crate::match_event(&inner, Key::H, &mods).map(|(_, reg)| reg.intercept);
        let d = crate::policy::classify(false, matched);
        assert!(d.emit);
        assert!(d.intercept);
    }

    #[test]
    fn tap_decision_tracks_intercepted_repeat_and_keyup() {
        let (inner, mods, id) = registered_inner(true);
        let mut held = HashSet::new();

        let down = classify_tap_event(&inner, Key::H, &mods, EventKind::KeyDown, false, &mut held);
        assert!(down.intercept);
        assert_eq!(down.event.as_ref().map(|event| event.id), Some(id));
        assert!(held.contains(&Key::H));

        let repeat = classify_tap_event(&inner, Key::H, &mods, EventKind::KeyDown, true, &mut held);
        assert!(repeat.intercept);
        assert_eq!(repeat.event.as_ref().map(|event| event.repeat), Some(true));

        let up = classify_tap_event(&inner, Key::H, &mods, EventKind::KeyUp, false, &mut held);
        assert!(up.intercept);
        assert!(!held.contains(&Key::H));
    }

    #[test]
    fn tap_decision_capture_intercepts_unmatched_without_emit() {
        let (mut inner, mods, _id) = registered_inner(false);
        inner.capture_all = 1;
        let mut held = HashSet::new();

        let action =
            classify_tap_event(&inner, Key::J, &mods, EventKind::KeyDown, false, &mut held);

        assert!(action.intercept);
        assert!(action.event.is_none());
    }
}
