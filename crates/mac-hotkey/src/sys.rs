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
    collections::{HashMap, HashSet},
    ffi::c_void,
    process, ptr,
    sync::{
        Arc,
        atomic::{AtomicPtr, Ordering},
    },
};

use core_foundation::{
    base::{CFType, TCFType},
    dictionary::{CFDictionary, CFDictionaryRef},
    mach_port::CFMachPortRef,
    number::CFNumber,
    runloop::{CFRunLoop, kCFRunLoopCommonModes},
    string::CFString,
};
use core_graphics::event::{self as cge, CallbackResult};
use crossbeam_channel::Sender;
use mac_keycode::{Chord, Key, Modifier};
use objc2_app_kit::NSRunningApplication;
use parking_lot::Mutex;
use tracing::{debug, trace, warn};

use crate::{
    Event, EventKind, policy,
    status::{PlatformSample, StatusStore},
};

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> u8;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
}

const SESSION_SECURE_INPUT_PID: &str = "kCGSSessionSecureInputPID";

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

/// Logical registration accepted by one physical key press.
#[derive(Debug)]
struct LogicalPress {
    id: u32,
    hotkey: Chord,
}

/// Decision retained from physical key-down until its paired key-up.
#[derive(Debug)]
struct PressRecord {
    logical: Option<LogicalPress>,
    intercept: bool,
}

impl PressRecord {
    /// Recreate an event from the registration accepted at physical key-down.
    fn action(&self, kind: EventKind, repeat: bool, emit_logical: bool) -> TapAction {
        TapAction {
            event: if emit_logical {
                self.logical.as_ref().map(|logical| Event {
                    id: logical.id,
                    hotkey: logical.hotkey.clone(),
                    kind,
                    repeat,
                })
            } else {
                None
            },
            intercept: self.intercept,
        }
    }
}

fn classify_tap_event(
    inner: &crate::Inner,
    code: Key,
    mods: &HashSet<Modifier>,
    kind: EventKind,
    is_repeat: bool,
    presses: &mut HashMap<Key, PressRecord>,
) -> TapAction {
    if matches!(kind, EventKind::KeyUp) {
        return presses.remove(&code).map_or(
            TapAction {
                event: None,
                intercept: false,
            },
            |press| press.action(EventKind::KeyUp, false, true),
        );
    }

    if let Some(press) = presses.get(&code) {
        return press.action(EventKind::KeyDown, is_repeat, inner.suspend == 0);
    }

    let matched = crate::match_event(inner, code, mods);
    let matched_intercept = matched.as_ref().map(|(_, reg)| reg.intercept);
    let suspended = inner.suspend > 0;
    let capture = inner.capture_all > 0;
    let mut decision = policy::classify(suspended, matched_intercept);

    if !suspended && capture {
        decision.intercept = true;
        decision.emit = matched.is_some();
    }

    let logical = if decision.emit {
        matched.map(|(id, reg)| LogicalPress {
            id,
            hotkey: reg.hotkey,
        })
    } else {
        None
    };
    let press = PressRecord {
        logical,
        intercept: decision.intercept,
    };
    let action = press.action(EventKind::KeyDown, is_repeat, true);
    presses.insert(code, press);
    action
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
    status: Arc<StatusStore>,
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
    let status_cb = status.clone();
    let presses: RefCell<HashMap<Key, PressRecord>> = RefCell::new(HashMap::new());
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

            match etype {
                cge::CGEventType::KeyDown | cge::CGEventType::KeyUp => {
                    status_cb.record_physical_event();
                    let keycode =
                        event.get_integer_value_field(FIELD_KEYBOARD_EVENT_KEYCODE) as u16;
                    if let Ok(code) = Key::try_from(keycode) {
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
                            let mut presses = presses.borrow_mut();
                            classify_tap_event(
                                inner.as_ref(),
                                code,
                                &mods,
                                kind,
                                is_repeat,
                                &mut presses,
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
                    status_cb.record_disable();
                    let p = tap_port_ptr_cb.load(Ordering::SeqCst) as CFMachPortRef;
                    if !p.is_null() {
                        warn!("tap_disabled_by_os_reenabling");
                        // SAFETY: `p` is a valid CFMachPortRef because it was created by
                        // `CGEventTap::new` and its pointer was stored in `tap_port_ptr_cb`
                        // before the run loop started. It remains valid for the lifetime of
                        // the tap, which is tied to the thread running this callback.
                        unsafe { CGEventTapEnable(p, true) };
                        // SAFETY: `p` has the same tap-owned lifetime described above.
                        status_cb.record_reenable(unsafe { CGEventTapIsEnabled(p) });
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
    status.set_lifecycle(crate::TapLifecycle::Running);

    let _ = ready.send(Ok(()));
    debug!("event_tap_started_run_loop");

    CFRunLoop::run_current();

    status.set_lifecycle(crate::TapLifecycle::Stopped);
    debug!("event_tap_exited");
    Ok(())
}

pub(crate) fn sample_platform() -> PlatformSample {
    // SAFETY: Carbon documents this parameterless query for process-wide use.
    // `StatusStore` serializes every invocation because the function is not thread-safe.
    let active = unsafe { IsSecureEventInputEnabled() != 0 };
    PlatformSample {
        secure_input: if active {
            crate::SecureInputState::Active
        } else {
            crate::SecureInputState::Inactive
        },
        owner: active.then(secure_input_owner).flatten(),
    }
}

fn secure_input_owner() -> Option<crate::SecureInputOwner> {
    // SAFETY: A non-null result follows Core Foundation's create rule and is
    // transferred immediately to the owning wrapper.
    let raw = unsafe { CGSessionCopyCurrentDictionary() };
    if raw.is_null() {
        return None;
    }
    let dictionary: CFDictionary<CFString, CFType> =
        unsafe { TCFType::wrap_under_create_rule(raw) };
    let value = dictionary.find(CFString::from_static_string(SESSION_SECURE_INPUT_PID))?;
    let pid = value.downcast::<CFNumber>()?.to_i32()?;
    resolve_secure_input_owner(Some(pid), |pid| {
        let application = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
        if application.isTerminated() {
            return None;
        }
        application.localizedName().map(|name| name.to_string())
    })
}

fn resolve_secure_input_owner(
    pid: Option<i32>,
    resolve_app_name: impl FnOnce(i32) -> Option<String>,
) -> Option<crate::SecureInputOwner> {
    let pid = pid.filter(|pid| *pid > 0)?;
    Some(crate::SecureInputOwner {
        pid: u32::try_from(pid).ok()?,
        app_name: resolve_app_name(pid)?,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use mac_keycode::{Key, Modifier};

    use super::{TapAction, classify_tap_event, resolve_secure_input_owner};
    use crate::{EventKind, Inner, test_register};

    fn simulate(suspended: bool, matched: bool, intercept: bool) -> crate::policy::Decision {
        let matched_intercept = if matched { Some(intercept) } else { None };
        crate::policy::classify(suspended, matched_intercept)
    }

    #[test]
    fn secure_input_owner_resolution_handles_fallible_platform_data() {
        assert_eq!(resolve_secure_input_owner(None, |_| unreachable!()), None);
        assert_eq!(
            resolve_secure_input_owner(Some(-1), |_| unreachable!()),
            None
        );
        assert_eq!(resolve_secure_input_owner(Some(999), |_| None), None);

        let owner = resolve_secure_input_owner(Some(42), |_| Some("Terminal".to_string()))
            .expect("live PID with a name resolves");
        assert_eq!(owner.pid, 42);
        assert_eq!(owner.app_name, "Terminal");
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

    fn assert_event(action: &TapAction, id: u32, kind: EventKind, repeat: bool) {
        let event = action.event.as_ref().expect("logical event");
        assert_eq!(event.id, id);
        assert_eq!(event.hotkey.key, Key::H);
        assert_eq!(event.hotkey.modifiers, HashSet::from([Modifier::Control]));
        assert_eq!(event.kind, kind);
        assert_eq!(event.repeat, repeat);
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
    fn tap_repeat_uses_original_press_identity() {
        let (mut inner, mods, id) = registered_inner(true);
        let mut presses = HashMap::new();

        let down = classify_tap_event(
            &inner,
            Key::H,
            &mods,
            EventKind::KeyDown,
            false,
            &mut presses,
        );
        assert!(down.intercept);
        assert_event(&down, id, EventKind::KeyDown, false);
        assert!(presses.contains_key(&Key::H));

        assert!(inner.unregister(id));
        let repeat = classify_tap_event(
            &inner,
            Key::H,
            &HashSet::new(),
            EventKind::KeyDown,
            true,
            &mut presses,
        );
        assert!(repeat.intercept);
        assert_event(&repeat, id, EventKind::KeyDown, true);
        assert!(presses.contains_key(&Key::H));

        let up = classify_tap_event(
            &inner,
            Key::H,
            &HashSet::new(),
            EventKind::KeyUp,
            false,
            &mut presses,
        );
        assert!(up.intercept);
        assert_event(&up, id, EventKind::KeyUp, false);
        assert!(!presses.contains_key(&Key::H));
    }

    #[test]
    fn tap_duplicate_down_cannot_replace_original_press() {
        let (mut inner, mods, id) = registered_inner(true);
        let mut presses = HashMap::new();

        let down = classify_tap_event(
            &inner,
            Key::H,
            &mods,
            EventKind::KeyDown,
            false,
            &mut presses,
        );
        assert!(down.intercept);
        assert_event(&down, id, EventKind::KeyDown, false);

        assert!(inner.unregister(id));
        let replacement_id = test_register(
            &mut inner,
            mac_keycode::Chord {
                key: Key::H,
                modifiers: mods.clone(),
            },
            false,
        );
        let duplicate = classify_tap_event(
            &inner,
            Key::H,
            &mods,
            EventKind::KeyDown,
            false,
            &mut presses,
        );
        assert!(duplicate.intercept);
        assert_event(&duplicate, id, EventKind::KeyDown, false);
        assert_ne!(
            duplicate.event.as_ref().map(|event| event.id),
            Some(replacement_id)
        );

        let up = classify_tap_event(
            &inner,
            Key::H,
            &HashSet::new(),
            EventKind::KeyUp,
            false,
            &mut presses,
        );
        assert!(up.intercept);
        assert_event(&up, id, EventKind::KeyUp, false);
    }

    #[test]
    fn tap_release_uses_press_modifiers() {
        let (inner, mods, id) = registered_inner(true);
        let mut presses = HashMap::new();

        let down = classify_tap_event(
            &inner,
            Key::H,
            &mods,
            EventKind::KeyDown,
            false,
            &mut presses,
        );
        assert_event(&down, id, EventKind::KeyDown, false);

        let up = classify_tap_event(
            &inner,
            Key::H,
            &HashSet::new(),
            EventKind::KeyUp,
            false,
            &mut presses,
        );
        assert!(up.intercept);
        assert_event(&up, id, EventKind::KeyUp, false);
    }

    #[test]
    fn tap_release_survives_unregister_while_held() {
        let (mut inner, mods, id) = registered_inner(true);
        let mut presses = HashMap::new();

        let down = classify_tap_event(
            &inner,
            Key::H,
            &mods,
            EventKind::KeyDown,
            false,
            &mut presses,
        );
        assert_event(&down, id, EventKind::KeyDown, false);
        assert!(inner.unregister(id));

        let up = classify_tap_event(
            &inner,
            Key::H,
            &HashSet::new(),
            EventKind::KeyUp,
            false,
            &mut presses,
        );
        assert!(up.intercept);
        assert_event(&up, id, EventKind::KeyUp, false);
    }

    #[test]
    fn tap_release_survives_suspend_while_held() {
        let (mut inner, mods, id) = registered_inner(true);
        let mut presses = HashMap::new();

        let down = classify_tap_event(
            &inner,
            Key::H,
            &mods,
            EventKind::KeyDown,
            false,
            &mut presses,
        );
        assert_event(&down, id, EventKind::KeyDown, false);
        inner.suspend = 1;

        let repeat = classify_tap_event(
            &inner,
            Key::H,
            &HashSet::new(),
            EventKind::KeyDown,
            true,
            &mut presses,
        );
        assert!(repeat.intercept);
        assert!(repeat.event.is_none());

        let up = classify_tap_event(
            &inner,
            Key::H,
            &HashSet::new(),
            EventKind::KeyUp,
            false,
            &mut presses,
        );
        assert!(up.intercept);
        assert_event(&up, id, EventKind::KeyUp, false);
    }

    #[test]
    fn tap_capture_decision_lasts_for_unmatched_press() {
        let mut inner = Inner {
            capture_all: 1,
            ..Inner::default()
        };
        let mut presses = HashMap::new();

        let down = classify_tap_event(
            &inner,
            Key::J,
            &HashSet::new(),
            EventKind::KeyDown,
            false,
            &mut presses,
        );
        assert!(down.intercept);
        assert!(down.event.is_none());

        inner.capture_all = 0;
        let up = classify_tap_event(
            &inner,
            Key::J,
            &HashSet::new(),
            EventKind::KeyUp,
            false,
            &mut presses,
        );
        assert!(up.intercept);
        assert!(up.event.is_none());
    }

    #[test]
    fn tap_capture_interception_lasts_for_matched_press() {
        let (mut inner, mods, id) = registered_inner(false);
        inner.capture_all = 1;
        let mut presses = HashMap::new();

        let down = classify_tap_event(
            &inner,
            Key::H,
            &mods,
            EventKind::KeyDown,
            false,
            &mut presses,
        );
        assert!(down.intercept);
        assert_event(&down, id, EventKind::KeyDown, false);

        inner.capture_all = 0;
        let up = classify_tap_event(
            &inner,
            Key::H,
            &HashSet::new(),
            EventKind::KeyUp,
            false,
            &mut presses,
        );
        assert!(up.intercept);
        assert_event(&up, id, EventKind::KeyUp, false);
    }

    #[test]
    fn tap_capture_does_not_adopt_an_existing_press() {
        let mut inner = Inner::default();
        let mut presses = HashMap::new();

        let down = classify_tap_event(
            &inner,
            Key::J,
            &HashSet::new(),
            EventKind::KeyDown,
            false,
            &mut presses,
        );
        assert!(!down.intercept);
        assert!(down.event.is_none());

        inner.capture_all = 1;
        let up = classify_tap_event(
            &inner,
            Key::J,
            &HashSet::new(),
            EventKind::KeyUp,
            false,
            &mut presses,
        );
        assert!(!up.intercept);
        assert!(up.event.is_none());
    }
}
