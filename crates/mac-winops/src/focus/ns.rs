use std::sync::Mutex;

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
/// Must be called before requesting installation of the NSWorkspace observer.
pub(crate) fn set_ns_sink(tx: tokio::sync::mpsc::UnboundedSender<FocusEvent>) {
    if let Ok(mut guard) = NS_SINK.lock() {
        *guard = Some(tx);
    }
}

// Main-thread proxy to schedule installs safely on TAO event loop
static MAIN_PROXY: Lazy<Mutex<Option<EventLoopProxy<()>>>> = Lazy::new(|| Mutex::new(None));

/// Provide the Tao main thread proxy so we can request installation.
/// Call exactly once on the Tao main thread after creating the event loop.
pub fn set_main_proxy(proxy: EventLoopProxy<()>) {
    if let Ok(mut p) = MAIN_PROXY.lock() {
        *p = Some(proxy);
    }
}

/// Ask the Tao main thread to install the NSWorkspace observer.
pub(crate) fn request_ns_observer_install() -> Result<(), super::Error> {
    if let Ok(p) = MAIN_PROXY.lock() {
        if let Some(proxy) = &*p {
            proxy
                .send_event(())
                .map_err(|_| super::Error::PostEventFailed)?;
            return Ok(());
        }
        Err(super::Error::MainProxyNotSet)
    } else {
        Err(super::Error::MainProxyPoisoned)
    }
}

/// Wake the main loop; used by other modules to nudge event processing.
pub fn wake_main_loop() -> Result<(), super::Error> {
    request_ns_observer_install()
}

fn ns_emit_app_changed(name: String, pid: i32) {
    if let Ok(guard) = NS_SINK.lock()
        && let Some(tx) = &*guard
    {
        let _ = tx.send(FocusEvent::AppChanged { title: name, pid });
    }
}

/// Install the NSWorkspace activation observer on the current (main) thread.
/// Idempotent: safe to call multiple times; only the first call installs.
pub fn install_ns_workspace_observer() -> Result<(), super::Error> {
    use objc2::rc::Retained;
    use objc2_foundation::MainThreadMarker;

    static INSTALLED: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));

    if let Ok(mut installed) = INSTALLED.lock() {
        if *installed {
            return Ok(());
        }
        unsafe {
            let _mtm = MainThreadMarker::new().ok_or(super::Error::NsObserverPoisoned)?;
            let center = NSWorkspace::sharedWorkspace().notificationCenter();

            let block = StackBlock::new(|notification: std::ptr::NonNull<NSNotification>| {
                // notification.object() is NSRunningApplication*
                if let Some(obj) = notification.as_ref().object()
                    && let Some(app) = obj.downcast_ref::<NSRunningApplication>()
                {
                    let pid = app.processIdentifier();
                    let mut sent = false;
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
                        }
                    }
                }
            })
            .copy();
            let _: Retained<_> =
                center.addObserverForName_object_queue_usingBlock(None, None, None, &block);
            *installed = true;
            info!("NSWorkspace observer installed");
        }
        Ok(())
    } else {
        Err(super::Error::NsObserverPoisoned)
    }
}
