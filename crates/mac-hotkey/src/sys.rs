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
    ffi::c_void,
    process,
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
use parking_lot::Mutex;
use tracing::{debug, trace, warn};

use crate::{CallbackCtx, Event, EventKind, policy};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
}

// Minimal subset of CGEventField constants used by this module.
const FIELD_EVENT_SOURCE_UNIX_PROCESS_ID: u32 = 41;
const FIELD_EVENT_SOURCE_USER_DATA: u32 = 42;
const FIELD_KEYBOARD_EVENT_AUTOREPEAT: u32 = 8;
const FIELD_KEYBOARD_EVENT_KEYCODE: u32 = 9;

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
    cb_ctx: CallbackCtx,
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
    let tap_port_ptr: Arc<AtomicPtr<c_void>> = Arc::new(AtomicPtr::new(std::ptr::null_mut()));

    debug!("creating_event_tap");
    let tap_port_ptr_cb = tap_port_ptr.clone();
    let cb_ctx_cb = cb_ctx.clone();
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
                let inner = cb_ctx_cb.inner.lock();
                if inner.suspend > 0 {
                    trace!("tap_suspended_skipping_event");
                    return CallbackResult::Keep;
                }
            }

            match etype {
                cge::CGEventType::KeyDown | cge::CGEventType::KeyUp => {
                    let keycode =
                        event.get_integer_value_field(FIELD_KEYBOARD_EVENT_KEYCODE) as u16;
                    if let Some(code) = mac_keycode::Key::from_scancode(keycode) {
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

                        let intercept = {
                            let mut inner = cb_ctx_cb.inner.lock();
                            let matched =
                                crate::match_event(&inner, code, &mods).map(|(id, reg)| {
                                    (
                                        id,
                                        super::RegisterOptions {
                                            intercept: reg.intercept,
                                        },
                                    )
                                });
                            let suspended = inner.suspend > 0;
                            let capture = inner.capture_all > 0;
                            let mut d = policy::classify(suspended, matched, kind, is_repeat);

                            // Adjust for capture-all: swallow everything; emit only matched
                            if !suspended && capture {
                                // Force interception regardless of registration
                                d.intercept = true;
                                // Only emit when key matched a binding
                                d.emit = matched.is_some();
                            }

                            match kind {
                                EventKind::KeyDown => {
                                    if d.intercept && !is_repeat {
                                        inner.note_intercept_down(code);
                                    } else if is_repeat && inner.intercept_on_repeat(code) {
                                        d.intercept = true;
                                    }
                                }
                                EventKind::KeyUp => {
                                    if inner.intercept_on_keyup(code) {
                                        d.intercept = true;
                                    }
                                }
                            }

                            if d.emit
                                && let Some((id, reg)) = crate::match_event(&inner, code, &mods)
                            {
                                let ev = Event {
                                    id,
                                    hotkey: reg.hotkey.clone(),
                                    kind,
                                    repeat: is_repeat,
                                };
                                let _ = cb_ctx_cb.tx.send(ev);
                            }
                            d.intercept
                        };

                        if intercept {
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
    use mac_keycode::Key;

    use super::*;
    use crate::{RegisterOptions, test_register};

    fn simulate(
        suspended: bool,
        matched: bool,
        intercept: bool,
        kind: EventKind,
        is_repeat: bool,
    ) -> crate::policy::Decision {
        if matched {
            let _inner = crate::Inner::default();
            let m = Some((1u32, RegisterOptions { intercept }));
            crate::policy::classify(suspended, m, kind, is_repeat)
        } else {
            crate::policy::classify(suspended, None, kind, is_repeat)
        }
    }

    #[test]
    fn tap_policy_sim_repeat_intercepted_vs_forwarded() {
        let d = simulate(false, true, true, EventKind::KeyDown, true);
        assert!(d.emit);
        assert!(d.intercept);
        let d = simulate(false, true, false, EventKind::KeyDown, true);
        assert!(d.emit);
        assert!(!d.intercept);
    }

    #[test]
    fn tap_policy_sim_initial_emits_and_tracks_intercept() {
        let d = simulate(false, true, false, EventKind::KeyDown, false);
        assert!(d.emit);
        assert!(!d.intercept);
        let d = simulate(false, true, true, EventKind::KeyUp, false);
        assert!(d.emit);
        assert!(d.intercept);
    }

    #[test]
    fn tap_policy_sim_suspended_and_nomatch() {
        let d = simulate(true, true, true, EventKind::KeyDown, false);
        assert!(!d.emit);
        assert!(!d.intercept);
        let d = simulate(false, false, true, EventKind::KeyDown, false);
        assert!(!d.emit);
        assert!(!d.intercept);
    }

    #[test]
    fn end_to_end_match_then_emit_shape() {
        let mut inner = crate::Inner::default();
        let hk = mac_keycode::Chord {
            key: Key::H,
            modifiers: {
                let mut s = std::collections::HashSet::new();
                s.insert(mac_keycode::Modifier::Control);
                s
            },
        };
        let _id = test_register(&mut inner, hk, RegisterOptions { intercept: true });
        let mut mods = std::collections::HashSet::new();
        mods.insert(mac_keycode::Modifier::Control);
        let matched = crate::match_event(&inner, Key::H, &mods).map(|(id, reg)| {
            (
                id,
                RegisterOptions {
                    intercept: reg.intercept,
                },
            )
        });
        let d = crate::policy::classify(false, matched, EventKind::KeyDown, false);
        assert!(d.emit);
        assert!(d.intercept);
    }
}
