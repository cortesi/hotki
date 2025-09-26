use std::{cell::RefCell, rc::Rc, time::Duration};

use objc2_foundation::MainThreadMarker;
use winit::{
    application::ApplicationHandler,
    event_loop::EventLoop,
    platform::pump_events::{EventLoopExtPumpEvents, PumpStatus},
};

thread_local! {
    static SHARED_EVENT_LOOP: RefCell<Option<Rc<RefCell<EventLoop<()>>>>> =
        const { RefCell::new(None) };
}

/// Handle for the shared winit event loop used by mimic helpers and smoketests.
#[derive(Clone)]
pub struct EventLoopHandle(Rc<RefCell<EventLoop<()>>>);

impl EventLoopHandle {
    /// Pump application events with the provided timeout, returning the loop status.
    pub fn pump_app<A: ApplicationHandler>(
        &self,
        timeout: Option<Duration>,
        app: &mut A,
    ) -> PumpStatus {
        let mut event_loop = self.0.borrow_mut();
        event_loop.pump_app_events(timeout, app)
    }
}

/// Borrow the shared event loop, creating it on first use.
#[must_use]
pub fn shared_event_loop() -> EventLoopHandle {
    SHARED_EVENT_LOOP.with(|cell| {
        let mut borrowed = cell.borrow_mut();
        if let Some(existing) = borrowed.as_ref() {
            EventLoopHandle(existing.clone())
        } else {
            MainThreadMarker::new().expect("mimic helpers must run on the macOS main thread");
            let event_loop = Rc::new(RefCell::new(
                EventLoop::new().expect("failed to construct mimic event loop"),
            ));
            *borrowed = Some(event_loop.clone());
            EventLoopHandle(event_loop)
        }
    })
}
