use std::{ptr::NonNull, sync::Mutex};

use block2::StackBlock;
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_foundation::NSNotification;
use once_cell::sync::Lazy;
use tao::event_loop::EventLoopProxy;
use tracing::info;

use super::event::FocusEvent;

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
pub fn set_main_proxy(proxy: EventLoopProxy<()>) {
    if let Ok(mut g) = MAIN_PROXY.lock() {
        *g = Some(proxy);
    }
}

/// Request installation of the NSWorkspace observer on the main thread.
pub(crate) fn request_ns_observer_install() -> Result<(), super::Error> {
    match MAIN_PROXY.lock() {
        Ok(guard) => match &*guard {
            Some(p) => {
                if p.send_event(()).is_err() {
                    return Err(super::Error::PostEventFailed);
                }
                Ok(())
            }
            None => Err(super::Error::MainProxyNotSet),
        },
        Err(_) => Err(super::Error::MainProxyPoisoned),
    }
}

/// Post a generic Tao `UserEvent(())` to wake the main event loop.
pub fn wake_main_loop() -> Result<(), super::Error> {
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
pub fn install_ns_workspace_observer() -> Result<(), super::Error> {
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
        Err(super::Error::NsObserverPoisoned)
    }
}
