use objc2::rc::Retained;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::config;

/// Run a borderless, always-on-top overlay window instructing the user to avoid typing.
/// The window stays up until the process is killed by the parent orchestrator.
pub fn run_warn_overlay(
    status_path_arg: Option<std::path::PathBuf>,
    info_path_arg: Option<std::path::PathBuf>,
) -> Result<(), String> {
    // Create winit event loop; do not explicitly activate the app to avoid stealing focus.
    let event_loop = winit::event_loop::EventLoop::new().map_err(|e| e.to_string())?;

    use winit::{
        application::ApplicationHandler,
        event::WindowEvent,
        event_loop::{ActiveEventLoop, ControlFlow},
    };

    struct OverlayApp {
        window: Option<winit::window::Window>,
        title_label: Option<Retained<objc2_app_kit::NSTextField>>,
        warn_label: Option<Retained<objc2_app_kit::NSTextField>>,
        info_label: Option<Retained<objc2_app_kit::NSTextField>>,
        countdown_label: Option<Retained<objc2_app_kit::NSTextField>>,
        status_path: Option<std::path::PathBuf>,
        info_path: Option<std::path::PathBuf>,
        last_title: String,
        last_info: String,
        next_deadline: Option<std::time::Instant>,
        start_time: std::time::Instant,
        countdown_active: bool,
        error: Option<String>,
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
                let win = match elwt.create_window(attrs) {
                    Ok(w) => w,
                    Err(e) => {
                        self.error = Some(format!("warn_overlay: failed to create window: {}", e));
                        elwt.exit();
                        return;
                    }
                };
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
                                    use objc2_app_kit::{
                                        NSColor, NSFont, NSTextAlignment, NSTextField,
                                    };
                                    let margin_x: f64 = 12.0;
                                    let lw =
                                        (config::WARN_OVERLAY_WIDTH - 2.0 * margin_x).max(10.0);

                                    // Title label (test name) - centered, MOST VISUAL WEIGHT
                                    let title_text = NSString::from_str("...");
                                    let title =
                                        unsafe { NSTextField::labelWithString(&title_text, mtm) };
                                    let title_font = unsafe { NSFont::boldSystemFontOfSize(24.0) };
                                    unsafe { title.setFont(Some(&title_font)) };
                                    unsafe { title.setAlignment(NSTextAlignment::Center) };
                                    let title_h: f64 = 32.0;
                                    let title_y = (config::WARN_OVERLAY_HEIGHT - title_h) / 2.0;
                                    let title_frame = NSRect::new(
                                        NSPoint::new(margin_x, title_y),
                                        NSSize::new(lw, title_h),
                                    );
                                    unsafe { title.setFrame(title_frame) };
                                    unsafe { view.addSubview(&title) };

                                    // Countdown/spinner label (above title)
                                    let countdown_text = NSString::from_str("2");
                                    let countdown = unsafe {
                                        NSTextField::labelWithString(&countdown_text, mtm)
                                    };
                                    let countdown_font =
                                        unsafe { NSFont::boldSystemFontOfSize(32.0) };
                                    unsafe { countdown.setFont(Some(&countdown_font)) };
                                    unsafe { countdown.setAlignment(NSTextAlignment::Center) };
                                    // Bright blue countdown color
                                    let countdown_color = unsafe {
                                        NSColor::colorWithCalibratedRed_green_blue_alpha(
                                            0.2, 0.6, 1.0, 1.0,
                                        )
                                    };
                                    unsafe { countdown.setTextColor(Some(&countdown_color)) };
                                    let countdown_h: f64 = 40.0;
                                    let countdown_y = title_y + title_h + 4.0;
                                    let countdown_frame = NSRect::new(
                                        NSPoint::new(margin_x, countdown_y),
                                        NSSize::new(lw, countdown_h),
                                    );
                                    unsafe { countdown.setFrame(countdown_frame) };
                                    unsafe { view.addSubview(&countdown) };

                                    // Info label (just below title)
                                    let info_text = NSString::from_str("");
                                    let info =
                                        unsafe { NSTextField::labelWithString(&info_text, mtm) };
                                    let info_font = unsafe { NSFont::systemFontOfSize(13.0) };
                                    unsafe { info.setFont(Some(&info_font)) };
                                    unsafe { info.setAlignment(NSTextAlignment::Center) };
                                    let info_color = unsafe { NSColor::secondaryLabelColor() };
                                    unsafe { info.setTextColor(Some(&info_color)) };
                                    let info_h: f64 = 18.0;
                                    let info_y = (title_y - info_h - 4.0).max(24.0);
                                    let info_frame = NSRect::new(
                                        NSPoint::new(margin_x, info_y),
                                        NSSize::new(lw, info_h),
                                    );
                                    unsafe { info.setFrame(info_frame) };
                                    unsafe { view.addSubview(&info) };

                                    // Warning text (at bottom, subtle)
                                    let warn_text =
                                        NSString::from_str("Hands off keyboard during tests");
                                    let warn =
                                        unsafe { NSTextField::labelWithString(&warn_text, mtm) };
                                    let warn_font = unsafe { NSFont::systemFontOfSize(12.0) };
                                    unsafe { warn.setFont(Some(&warn_font)) };
                                    unsafe { warn.setAlignment(NSTextAlignment::Center) };
                                    let warn_color = unsafe { NSColor::secondaryLabelColor() };
                                    unsafe { warn.setTextColor(Some(&warn_color)) };
                                    let warn_h: f64 = 16.0;
                                    let warn_y = 8.0;
                                    let warn_frame = NSRect::new(
                                        NSPoint::new(margin_x, warn_y),
                                        NSSize::new(lw, warn_h),
                                    );
                                    unsafe { warn.setFrame(warn_frame) };
                                    unsafe { view.addSubview(&warn) };

                                    self.warn_label = Some(warn);
                                    self.title_label = Some(title);
                                    self.info_label = Some(info);
                                    self.countdown_label = Some(countdown);
                                }
                                break;
                            }
                        }
                    }
                }

                self.window = Some(win);

                // Status path is pre-configured before run loop starts.
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

            // Check if we should update (either no deadline set yet, or deadline passed)
            let should_update = self.next_deadline.is_none_or(|deadline| now >= deadline);

            if should_update {
                // Update countdown timer or spinner
                if self.countdown_active {
                    let elapsed = now.duration_since(self.start_time);
                    let grace_ms = config::WARN_OVERLAY_INITIAL_DELAY_MS;
                    let remaining_ms = grace_ms.saturating_sub(elapsed.as_millis() as u64);

                    if let Some(countdown_label) = &self.countdown_label
                        && let Some(_mtm) = objc2_foundation::MainThreadMarker::new()
                    {
                        if remaining_ms > 0 {
                            // Show countdown number
                            let secs = remaining_ms.div_ceil(1000); // Round up
                            let text = format!("{}", secs);
                            let ns = NSString::from_str(&text);
                            unsafe { countdown_label.setStringValue(&ns) };
                        } else {
                            // Show spinner animation
                            let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                            let frame_idx =
                                ((elapsed.as_millis() / 100) as usize) % spinner_frames.len();
                            let ns = NSString::from_str(spinner_frames[frame_idx]);
                            unsafe { countdown_label.setStringValue(&ns) };
                        }
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                }

                // Update test name from status file
                if let (Some(path), Some(label)) = (&self.status_path, &self.title_label)
                    && let Ok(s) = std::fs::read_to_string(path)
                {
                    let name = s.trim();
                    if !name.is_empty()
                        && name != self.last_title
                        && let Some(_mtm) = objc2_foundation::MainThreadMarker::new()
                    {
                        // Check if transitioning from prep to a real test
                        if (self.last_title == "..." || self.last_title == "Preparing tests...")
                            && name != "Preparing tests..."
                        {
                            // Real test starting, but keep animating spinner
                            // self.countdown_active = false;
                        }

                        // Show the test name, or "..." if still preparing
                        let display_name = if name == "Preparing tests..." {
                            "..."
                        } else {
                            name
                        };

                        let ns = NSString::from_str(display_name);
                        unsafe { label.setStringValue(&ns) };
                        self.last_title = name.to_string();
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                }

                // Update info text from info file
                if let (Some(path), Some(label)) = (&self.info_path, &self.info_label)
                    && let Ok(s) = std::fs::read_to_string(path)
                {
                    let text = s.trim();
                    if let Some(_mtm) = objc2_foundation::MainThreadMarker::new() {
                        let display = if text.is_empty() { " " } else { text };
                        let ns = NSString::from_str(display);
                        unsafe { label.setStringValue(&ns) };
                        self.last_info = display.to_string();
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                }
                self.next_deadline = Some(now + Duration::from_millis(100));
            }
            let next = self
                .next_deadline
                .unwrap_or_else(|| now + std::time::Duration::from_millis(100));
            elwt.set_control_flow(ControlFlow::WaitUntil(next));
        }
    }

    let mut app = OverlayApp {
        window: None,
        title_label: None,
        warn_label: None,
        info_label: None,
        countdown_label: None,
        status_path: status_path_arg,
        info_path: info_path_arg,
        last_title: String::from("..."),
        last_info: String::new(),
        next_deadline: None,
        start_time: std::time::Instant::now(),
        countdown_active: true,
        error: None,
    };
    let _ = event_loop.run_app(&mut app);
    if let Some(e) = app.error.take() {
        Err(e)
    } else {
        Ok(())
    }
}
