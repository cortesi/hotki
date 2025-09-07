use std::time::Instant;

use crate::config;

pub(crate) fn run_focus_winhelper(title: &str, time_ms: u64) -> Result<(), String> {
    let event_loop = winit::event_loop::EventLoop::new().map_err(|e| e.to_string())?;

    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow};

    struct HelperApp {
        window: Option<winit::window::Window>,
        title: String,
        deadline: Instant,
    }

    impl ApplicationHandler for HelperApp {
        fn resumed(&mut self, elwt: &ActiveEventLoop) {
            if self.window.is_none() {
                use winit::dpi::{LogicalPosition, LogicalSize};
                let attrs = winit::window::Window::default_attributes()
                    .with_title(self.title.clone())
                    .with_visible(true)
                    // Small helper window; reduce visual intrusion.
                    .with_inner_size(LogicalSize::new(
                        crate::config::HELPER_WIN_WIDTH,
                        crate::config::HELPER_WIN_HEIGHT,
                    ));
                let win = elwt
                    .create_window(attrs)
                    .map_err(|e| e.to_string())
                    .expect("create window");
                if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                    unsafe { app.activate() };
                }
                // Place window at bottom-right corner of the main screen.
                if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                    use objc2_app_kit::NSScreen;
                    let margin: f64 = crate::config::HELPER_WIN_MARGIN;
                    if let Some(scr) = NSScreen::mainScreen(mtm) {
                        let vf = scr.visibleFrame();
                        let w = crate::config::HELPER_WIN_WIDTH;
                        let x = (vf.origin.x + vf.size.width - w - margin).max(0.0);
                        let y = (vf.origin.y + margin).max(0.0);
                        win.set_outer_position(LogicalPosition::new(x, y));
                    }
                }
                self.window = Some(win);
            }
        }
        fn window_event(
            &mut self,
            elwt: &ActiveEventLoop,
            _id: winit::window::WindowId,
            event: WindowEvent,
        ) {
            if let WindowEvent::CloseRequested = event {
                elwt.exit();
            }
        }
        fn about_to_wait(&mut self, elwt: &ActiveEventLoop) {
            if Instant::now() >= self.deadline {
                elwt.exit();
                return;
            }
            elwt.set_control_flow(ControlFlow::WaitUntil(self.deadline));
        }
    }

    let mut app = HelperApp {
        window: None,
        title: title.to_string(),
        deadline: Instant::now() + config::ms(time_ms.max(1000)),
    };
    let _ = event_loop.run_app(&mut app);
    Ok(())
}
