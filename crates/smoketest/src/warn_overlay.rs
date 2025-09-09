use crate::config;
use objc2::rc::Retained;
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
        title_label: Option<Retained<objc2_app_kit::NSTextField>>,
        warn_label: Option<Retained<objc2_app_kit::NSTextField>>,
        status_path: Option<std::path::PathBuf>,
        last_title: String,
        next_deadline: Option<std::time::Instant>,
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
                                    use objc2_app_kit::{NSFont, NSTextAlignment, NSTextField};
                                    let margin_x: f64 = 12.0;
                                    let lw =
                                        (config::WARN_OVERLAY_WIDTH - 2.0 * margin_x).max(10.0);

                                    // Warning text (centered)
                                    let warn_text = NSString::from_str(
                                        "Hands off the keyboard while smoketests run.",
                                    );
                                    let warn =
                                        unsafe { NSTextField::labelWithString(&warn_text, mtm) };
                                    let warn_font = unsafe { NSFont::boldSystemFontOfSize(16.0) };
                                    unsafe { warn.setFont(Some(&warn_font)) };
                                    unsafe { warn.setAlignment(NSTextAlignment::Center) };
                                    let warn_h: f64 = 26.0;
                                    let warn_y = (config::WARN_OVERLAY_HEIGHT - warn_h) / 2.0;
                                    let warn_frame = NSRect::new(
                                        NSPoint::new(margin_x, warn_y),
                                        NSSize::new(lw, warn_h),
                                    );
                                    unsafe { warn.setFrame(warn_frame) };
                                    unsafe { view.addSubview(&warn) };

                                    // Title label (current test), centered above warning
                                    let title_text = NSString::from_str("Preparing tests...");
                                    let title =
                                        unsafe { NSTextField::labelWithString(&title_text, mtm) };
                                    let title_font = unsafe { NSFont::boldSystemFontOfSize(18.0) };
                                    unsafe { title.setFont(Some(&title_font)) };
                                    unsafe { title.setAlignment(NSTextAlignment::Center) };
                                    let title_h: f64 = 22.0;
                                    let title_y = warn_y + warn_h + 8.0;
                                    let title_frame = NSRect::new(
                                        NSPoint::new(margin_x, title_y),
                                        NSSize::new(lw, title_h),
                                    );
                                    unsafe { title.setFrame(title_frame) };
                                    unsafe { view.addSubview(&title) };

                                    self.warn_label = Some(warn);
                                    self.title_label = Some(title);
                                }
                                break;
                            }
                        }
                    }
                }

                self.window = Some(win);

                // Capture status path from env for title updates
                self.status_path = std::env::var("HOTKI_SMOKETEST_STATUS_PATH")
                    .ok()
                    .map(Into::into);
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
            // Periodically poll the status file and update title if it changed
            use std::time::{Duration, Instant};
            let now = Instant::now();
            let deadline = self
                .next_deadline
                .unwrap_or_else(|| now + Duration::from_millis(250));

            if now >= deadline {
                if let (Some(path), Some(label)) = (&self.status_path, &self.title_label)
                    && let Ok(s) = std::fs::read_to_string(path)
                {
                    let name = s.trim();
                    if !name.is_empty()
                        && name != self.last_title
                        && let Some(_mtm) = objc2_foundation::MainThreadMarker::new()
                    {
                        let ns = NSString::from_str(name);
                        unsafe { label.setStringValue(&ns) };
                        self.last_title = name.to_string();
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                }
                self.next_deadline = Some(now + Duration::from_millis(250));
            }
            let next = self
                .next_deadline
                .unwrap_or_else(|| now + std::time::Duration::from_millis(250));
            elwt.set_control_flow(ControlFlow::WaitUntil(next));
        }
    }

    let mut app = OverlayApp {
        window: None,
        title_label: None,
        warn_label: None,
        status_path: None,
        last_title: String::new(),
        next_deadline: None,
    };
    let _ = event_loop.run_app(&mut app);
    Ok(())
}
