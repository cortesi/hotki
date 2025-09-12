use std::time::Instant;

use crate::config;

pub(crate) fn run_focus_winhelper(
    title: &str,
    time_ms: u64,
    slot: Option<u8>,
    grid: Option<(u32, u32, u32, u32)>,
    size: Option<(f64, f64)>,
    pos: Option<(f64, f64)>,
    label_text: Option<String>,
    start_minimized: bool,
    start_zoomed: bool,
) -> Result<(), String> {
    let event_loop = winit::event_loop::EventLoop::new().map_err(|e| e.to_string())?;

    use winit::{
        application::ApplicationHandler,
        event::WindowEvent,
        event_loop::{ActiveEventLoop, ControlFlow},
    };

    struct HelperApp {
        window: Option<winit::window::Window>,
        title: String,
        deadline: Instant,
        slot: Option<u8>,
        grid: Option<(u32, u32, u32, u32)>,
        size: Option<(f64, f64)>,
        pos: Option<(f64, f64)>,
        label_text: Option<String>,
        error: Option<String>,
        start_minimized: bool,
        start_zoomed: bool,
    }

    impl ApplicationHandler for HelperApp {
        fn resumed(&mut self, elwt: &ActiveEventLoop) {
            if self.window.is_none() {
                use winit::dpi::{LogicalPosition, LogicalSize};
                let attrs = winit::window::Window::default_attributes()
                    .with_title(self.title.clone())
                    .with_visible(true)
                    .with_decorations(false)
                    // Small helper window; reduce visual intrusion.
                    .with_inner_size(LogicalSize::new(
                        self.size
                            .map(|s| s.0)
                            .unwrap_or(crate::config::HELPER_WIN_WIDTH),
                        self.size
                            .map(|s| s.1)
                            .unwrap_or(crate::config::HELPER_WIN_HEIGHT),
                    ));
                let win = match elwt.create_window(attrs) {
                    Ok(w) => w,
                    Err(e) => {
                        self.error = Some(format!("winhelper: failed to create window: {}", e));
                        elwt.exit();
                        return;
                    }
                };
                if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                    unsafe { app.activate() };
                }
                // Placement: explicit grid -> 2x2 slot -> explicit pos -> fallback
                if let Some((cols, rows, col, row)) = self.grid {
                    let _ = mac_winops::place_grid_focused(
                        std::process::id() as i32,
                        cols,
                        rows,
                        col,
                        row,
                    );
                } else if let Some(slot) = self.slot {
                    let (col, row) = match slot {
                        1 => (0, 0),
                        2 => (1, 0),
                        3 => (0, 1),
                        _ => (1, 1),
                    };
                    let _ =
                        mac_winops::place_grid_focused(std::process::id() as i32, 2, 2, col, row);
                } else if let Some((x, y)) = self.pos {
                    win.set_outer_position(LogicalPosition::new(x, y));
                } else {
                    // Fallback: bottom-right corner at a fixed small size
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
                }
                // Give the system a brief moment to settle placement before labeling
                std::thread::sleep(crate::config::ms(
                    crate::config::WINDOW_REGISTRATION_DELAY_MS,
                ));

                // Apply initial window state (minimized/zoomed) if requested.
                if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                    let windows = app.windows();
                    for w in windows.iter() {
                        let t = w.title();
                        let is_match = objc2::rc::autoreleasepool(|pool| unsafe {
                            t.to_str(pool) == self.title
                        });
                        if is_match {
                            unsafe {
                                if self.start_zoomed && !w.isZoomed() {
                                    w.performZoom(None);
                                }
                                if self.start_minimized && !w.isMiniaturized() {
                                    w.miniaturize(None);
                                }
                            }
                            break;
                        }
                    }
                }

                // Always add a big, centered label with either explicit label text, derived TL/TR/etc., or the title
                if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                    let windows = app.windows();
                    for w in windows.iter() {
                        let title = w.title();
                        let is_match = objc2::rc::autoreleasepool(|pool| unsafe {
                            title.to_str(pool) == self.title
                        });
                        if is_match {
                            if let Some(view) = w.contentView() {
                                use objc2::rc::Retained;
                                use objc2_app_kit::{
                                    NSColor, NSFont, NSTextAlignment, NSTextField,
                                };
                                use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
                                // Pick label: explicit, else 2x2 grid mapping, else title
                                let label_str = if let Some(ref t) = self.label_text {
                                    t.clone()
                                } else if let Some((cols, rows, col, row)) = self.grid {
                                    if cols == 2 && rows == 2 {
                                        match (col, row) {
                                            (0, 0) => "TL".into(),
                                            (1, 0) => "TR".into(),
                                            (0, 1) => "BL".into(),
                                            _ => "BR".into(),
                                        }
                                    } else {
                                        self.title.clone()
                                    }
                                } else if let Some(slot) = self.slot {
                                    match slot {
                                        1 => "TL".into(),
                                        2 => "TR".into(),
                                        3 => "BL".into(),
                                        _ => "BR".into(),
                                    }
                                } else {
                                    self.title.clone()
                                };
                                let ns = NSString::from_str(&label_str);
                                let label: Retained<NSTextField> =
                                    unsafe { NSTextField::labelWithString(&ns, mtm) };
                                // Size font relative to content view size so letters are large and visible
                                let vframe = view.frame();
                                let vw = vframe.size.width;
                                let vh = vframe.size.height;
                                let base = vw.min(vh) * 0.35; // 35% of min dimension
                                let font = unsafe { NSFont::boldSystemFontOfSize(base) };
                                unsafe { label.setFont(Some(&font)) };
                                unsafe { label.setAlignment(NSTextAlignment::Center) };
                                let color = unsafe { NSColor::whiteColor() };
                                unsafe { label.setTextColor(Some(&color)) };
                                // Center the label within the content view with small margins
                                let margin_x = 8.0;
                                let margin_y = 8.0;
                                let lw = (vw - 2.0 * margin_x).max(10.0);
                                let lh = (vh - 2.0 * margin_y).max(20.0);
                                let lx = vframe.origin.x + margin_x;
                                let ly = vframe.origin.y + margin_y;
                                let frame = NSRect::new(NSPoint::new(lx, ly), NSSize::new(lw, lh));
                                unsafe { label.setFrame(frame) };
                                unsafe { view.addSubview(&label) };
                            }
                            break;
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
            match event {
                WindowEvent::CloseRequested => {
                    elwt.exit();
                }
                WindowEvent::Focused(focused) => {
                    // Update window background color on focus changes
                    if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                        let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                        let windows = app.windows();
                        for w in windows.iter() {
                            let title = w.title();
                            let is_match = objc2::rc::autoreleasepool(|pool| unsafe {
                                title.to_str(pool) == self.title
                            });
                            if is_match {
                                let color = unsafe {
                                    if focused {
                                        objc2_app_kit::NSColor::systemBlueColor()
                                    } else {
                                        objc2_app_kit::NSColor::controlBackgroundColor()
                                    }
                                };
                                w.setBackgroundColor(Some(&color));
                                break;
                            }
                        }
                    }
                }
                _ => {}
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
        slot,
        grid,
        size,
        pos,
        label_text,
        error: None,
        start_minimized,
        start_zoomed,
    };
    let _ = event_loop.run_app(&mut app);
    if let Some(e) = app.error.take() {
        Err(e)
    } else {
        Ok(())
    }
}
