use crate::config;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

/// Run a borderless, always-on-top overlay window instructing the user to avoid typing.
/// The window stays up until the process is killed by the parent orchestrator.
pub fn run_warn_overlay() -> Result<(), String> {
    // Create winit event loop; do not explicitly activate the app to avoid stealing focus.
    let event_loop = winit::event_loop::EventLoop::new().map_err(|e| e.to_string())?;

    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow};

    struct OverlayApp {
        window: Option<winit::window::Window>,
    }

    impl ApplicationHandler for OverlayApp {
        fn resumed(&mut self, elwt: &ActiveEventLoop) {
            if self.window.is_none() {
                use winit::dpi::LogicalSize;
                let attrs = winit::window::Window::default_attributes()
                    .with_title("hotki smoketest: hands-off")
                    .with_visible(true)
                    .with_decorations(false)
                    .with_resizable(false)
                    .with_inner_size(LogicalSize::new(
                        config::WARN_OVERLAY_WIDTH,
                        config::WARN_OVERLAY_HEIGHT,
                    ));
                let win = elwt.create_window(attrs).expect("create overlay window");
                // Ensure always-on-top using runtime API if available
                #[allow(unused_imports)]
                use winit::window::WindowLevel;
                #[allow(unused_must_use)]
                {
                    // Not all platforms/versions support this; ignore if missing at runtime.
                    win.set_window_level(WindowLevel::AlwaysOnTop);
                }

                // Place centered on the main screen without activating the app
                if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                    // Reduce activation prominence to avoid focus changes.
                    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                    app.setActivationPolicy(
                        objc2_app_kit::NSApplicationActivationPolicy::Accessory,
                    );
                    if let Some(scr) = objc2_app_kit::NSScreen::mainScreen(mtm) {
                        let vf = scr.visibleFrame();
                        let x = vf.origin.x + (vf.size.width - config::WARN_OVERLAY_WIDTH) / 2.0;
                        let y = vf.origin.y + (vf.size.height - config::WARN_OVERLAY_HEIGHT) / 2.0;
                        use winit::dpi::LogicalPosition;
                        win.set_outer_position(LogicalPosition::new(x.max(0.0), y.max(0.0)));

                        // Add a simple label so the user sees instructions
                        // Lookup the NSWindow by title and attach an NSTextField
                        let windows = app.windows();
                        for w in windows.iter() {
                            let title = w.title();
                            let is_match = objc2::rc::autoreleasepool(|pool| unsafe {
                                title.to_str(pool) == "hotki smoketest: hands-off"
                            });
                            if is_match {
                                if let Some(view) = w.contentView() {
                                    use objc2_app_kit::NSTextField;
                                    let text = NSString::from_str(
                                        "Hands off the keyboard while smoketests run.",
                                    );
                                    let label = unsafe { NSTextField::labelWithString(&text, mtm) };
                                    // Set a slightly larger system font
                                    use objc2_app_kit::{NSFont, NSTextAlignment};
                                    let font = unsafe { NSFont::boldSystemFontOfSize(16.0) };
                                    unsafe { label.setFont(Some(&font)) };
                                    unsafe { label.setAlignment(NSTextAlignment::Center) };
                                    // Size and center vertically within the window
                                    let margin_x: f64 = 12.0;
                                    let lw =
                                        (config::WARN_OVERLAY_WIDTH - 2.0 * margin_x).max(10.0);
                                    let lh: f64 = 26.0;
                                    let ly = (config::WARN_OVERLAY_HEIGHT - lh) / 2.0;
                                    let frame = NSRect::new(
                                        NSPoint::new(margin_x, ly),
                                        NSSize::new(lw, lh),
                                    );
                                    unsafe { label.setFrame(frame) };
                                    unsafe { view.addSubview(&label) };
                                }
                                break;
                            }
                        }
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
                // Allow closing if user insists; otherwise parent will kill us when done.
                elwt.exit();
            }
        }

        fn about_to_wait(&mut self, elwt: &ActiveEventLoop) {
            elwt.set_control_flow(ControlFlow::Wait);
        }
    }

    let mut app = OverlayApp { window: None };
    let _ = event_loop.run_app(&mut app);
    Ok(())
}
