use std::ptr::NonNull;

use block2::StackBlock;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::NSNotification;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tao::event_loop::EventLoopProxy;
use tracing::debug;

// Legacy NS sink removed; observer remains for potential main-thread tasks.

// Main-thread proxy to schedule installs safely on TAO event loop
static MAIN_PROXY: Lazy<Mutex<Option<EventLoopProxy<()>>>> = Lazy::new(|| Mutex::new(None));

/// Provide the Tao main-thread `EventLoopProxy<()>` for scheduling installation
/// of the NSWorkspace observer.
pub fn set_main_proxy(proxy: EventLoopProxy<()>) {
    let mut g = MAIN_PROXY.lock();
    *g = Some(proxy);
}

/// Request installation of the NSWorkspace observer on the main thread.
pub(crate) fn request_ns_observer_install() -> Result<(), super::Error> {
    let guard = MAIN_PROXY.lock();
    match &*guard {
        Some(p) => {
            if p.send_event(()).is_err() {
                return Err(super::Error::PostEventFailed);
            }
            Ok(())
        }
        None => Err(super::Error::MainProxyNotSet),
    }
}

/// Post a generic Tao `UserEvent(())` to wake the main event loop.
pub fn post_user_event() -> Result<(), super::Error> {
    request_ns_observer_install()
}

// No-op emitter retained for future use.

// Global token to keep NSWorkspace observer alive
static NS_OBS_TOKEN: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));

/// Install the NSWorkspace activation observer on the current (main) thread.
pub fn install_ns_workspace_observer() -> Result<(), super::Error> {
    let mut installed = NS_OBS_TOKEN.lock();
    if *installed {
        return Ok(());
    }
    unsafe {
        let ws = NSWorkspace::sharedWorkspace();
        let center = ws.notificationCenter();
        let block = StackBlock::new(move |_notif: NonNull<NSNotification>| {}).copy();
        let _token = center.addObserverForName_object_queue_usingBlock(None, None, None, &block);
        *installed = true;
        debug!("NSWorkspace observer installed");
    }
    Ok(())
}
