use std::{ptr::NonNull, sync::Mutex};

use block2::StackBlock;
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_foundation::NSNotification;
use once_cell::sync::Lazy;
use tao::event_loop::EventLoopProxy;
use tracing::info;

use crate::event::FocusEvent;

// Global sink for NSWorkspace events (emitted from server main thread)
static NS_SINK: Lazy<Mutex<Option<tokio::sync::mpsc::UnboundedSender<FocusEvent>>>> =
    Lazy::new(|| Mutex::new(None));

/// Set the sink used by NSWorkspace notifications to forward focus events.
///
/// Must be called before requesting installation of the NSWorkspace observer.
pub(crate) fn set_ns_sink(tx: tokio::sync::mpsc::UnboundedSender<FocusEvent>) {
    if let Ok(mut guard) = NS_SINK.lock() {
        *guard = Some(tx);
    }
}

// Main-thread proxy to schedule installs safely on TAO event loop
static MAIN_PROXY: Lazy<Mutex<Option<EventLoopProxy<()>>>> = Lazy::new(|| Mutex::new(None));

/// Provide the Tao main-thread `EventLoopProxy<()>` for scheduling installation
/// of the NSWorkspace observer.
///
/// When to call:
/// - Call exactly once on the Tao main thread, after creating the event loop
///   and before calling [`crate::start_watcher`].
///
/// Why:
/// - The NSWorkspace observer must be installed on the main thread. This proxy
///   lets `start_watcher` post a user event to trigger
///   [`install_ns_workspace_observer`] from the event loop.
pub fn set_main_proxy(proxy: EventLoopProxy<()>) {
    if let Ok(mut g) = MAIN_PROXY.lock() {
        *g = Some(proxy);
    }
}

/// Request installation of the NSWorkspace observer on the main thread.
///
/// How it works:
/// - Posts a user event via the stored Tao `EventLoopProxy<()>` set by
///   [`set_main_proxy`].
/// - The Tao event loop should handle this user event and call
///   [`install_ns_workspace_observer`].
pub(crate) fn request_ns_observer_install() -> Result<(), crate::Error> {
    match MAIN_PROXY.lock() {
        Ok(guard) => match &*guard {
            Some(p) => {
                if p.send_event(()).is_err() {
                    return Err(crate::Error::PostEventFailed);
                }
                Ok(())
            }
            None => Err(crate::Error::MainProxyNotSet),
        },
        Err(_) => Err(crate::Error::MainProxyPoisoned),
    }
}

/// Post a generic Tao `UserEvent(())` to wake the main event loop.
///
/// This is useful to nudge the Tao loop when external threads flip control
/// flags (e.g., shutdown requests) and the loop is waiting (`ControlFlow::Wait`).
pub fn wake_main_loop() -> Result<(), crate::Error> {
    request_ns_observer_install()
}

/// Emit an AppChanged event into the NS sink; used by NSWorkspace callback.
pub(crate) fn ns_emit_app_changed(title: String, pid: i32) {
    if let Ok(guard) = NS_SINK.lock()
        && let Some(tx) = &*guard
    {
        let _ = tx.send(FocusEvent::AppChanged { title, pid });
    }
}

// Global token to keep NSWorkspace observer alive
static NS_OBS_TOKEN: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));

/// Install the NSWorkspace activation observer on the current (main) thread.
///
/// When to call:
/// - From the Tao event loop, in response to the single user event posted as
///   part of `start_watcher` (after you've called [`crate::set_main_proxy`]).
///   That user event is a `Tao::event::Event::UserEvent(())` that `start_watcher`
///   posts exactly once via the stored `EventLoopProxy<()>`.
///
/// Notes:
/// - Must run on the main thread.
/// - Idempotent: subsequent calls are no-ops; only the first call performs the install.
pub fn install_ns_workspace_observer() -> Result<(), crate::Error> {
    if let Ok(mut installed) = NS_OBS_TOKEN.lock() {
        if *installed {
            return Ok(());
        }
        unsafe {
            let ws = NSWorkspace::sharedWorkspace();
            let center = ws.notificationCenter();
            let block = StackBlock::new(move |notif: NonNull<NSNotification>| {
                let notif = notif.as_ref();
                let mut sent = false;
                if let Some(obj) = notif.object()
                    && let Some(app) = obj.downcast_ref::<NSRunningApplication>()
                {
                    let pid = app.processIdentifier();
                    if let Some(name) = app.localizedName() {
                        let c = name.UTF8String();
                        if !c.is_null()
                            && let Ok(s) = std::ffi::CStr::from_ptr(c).to_str()
                        {
                            ns_emit_app_changed(s.to_string(), pid);
                            sent = true;
                        }
                    }
                    if !sent && let Some(bid) = app.bundleIdentifier() {
                        let c = bid.UTF8String();
                        if !c.is_null()
                            && let Ok(s) = std::ffi::CStr::from_ptr(c).to_str()
                        {
                            ns_emit_app_changed(s.to_string(), pid);
                            sent = true;
                        }
                    }
                }
                if !sent {
                    ns_emit_app_changed(String::new(), -1);
                }
            })
            .copy();
            let _token =
                center.addObserverForName_object_queue_usingBlock(None, None, None, &block);
            // Keep process-global observer alive implicitly; center retains the block.
            *installed = true;
            info!("NSWorkspace observer installed");
        }
        Ok(())
    } else {
        Err(crate::Error::NsObserverPoisoned)
    }
}
