use std::time::Instant;
use tracing::debug;

use crate::config;

pub(crate) fn run_focus_winhelper(
    title: &str,
    time_ms: u64,
    delay_setframe_ms: u64,
    delay_apply_ms: u64,
    apply_target: Option<(f64, f64, f64, f64)>,
    apply_grid: Option<(u32, u32, u32, u32)>,
    slot: Option<u8>,
    grid: Option<(u32, u32, u32, u32)>,
    size: Option<(f64, f64)>,
    pos: Option<(f64, f64)>,
    label_text: Option<String>,
    start_minimized: bool,
    start_zoomed: bool,
    panel_nonmovable: bool,
    attach_sheet: bool,
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
        delay_setframe_ms: u64,
        delay_apply_ms: u64,
        apply_target: Option<(f64, f64, f64, f64)>,
        apply_grid: Option<(u32, u32, u32, u32)>,
        // Async-frame state
        last_pos: Option<(f64, f64)>,
        last_size: Option<(f64, f64)>,
        desired_pos: Option<(f64, f64)>,
        desired_size: Option<(f64, f64)>,
        apply_after: Option<Instant>,
        suppress_events: bool,
        slot: Option<u8>,
        grid: Option<(u32, u32, u32, u32)>,
        size: Option<(f64, f64)>,
        pos: Option<(f64, f64)>,
        label_text: Option<String>,
        error: Option<String>,
        start_minimized: bool,
        start_zoomed: bool,
        panel_nonmovable: bool,
        attach_sheet: bool,
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
                // If requested, make the helper window non-movable (pre-gate target on some systems).
                if self.panel_nonmovable
                    && let Some(mtm) = objc2_foundation::MainThreadMarker::new()
                {
                    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                    let windows = app.windows();
                    for w in windows.iter() {
                        let t = w.title();
                        let is_match = objc2::rc::autoreleasepool(|pool| unsafe {
                            t.to_str(pool) == self.title
                        });
                        if is_match {
                            w.setMovable(false);
                            break;
                        }
                    }
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

                // Capture initial pos/size as our last-known geometry for async delay logic
                let scale = win.scale_factor();
                if let Ok(p) = win.outer_position() {
                    let lp = p.to_logical::<f64>(scale);
                    self.last_pos = Some((lp.x, lp.y));
                }
                let isz = win.inner_size();
                let lsz = isz.to_logical::<f64>(scale);
                self.last_size = Some((lsz.width, lsz.height));

                // Arm explicit delayed-apply if configured
                if self.delay_apply_ms > 0 {
                    self.apply_after =
                        Some(Instant::now() + crate::config::ms(self.delay_apply_ms));
                    debug!(
                        "winhelper: armed delayed-apply +{}ms target={:?} grid={:?}",
                        self.delay_apply_ms, self.apply_target, self.apply_grid
                    );
                }

                // Optionally attach a simple sheet — placeholder (no-op for now).
                let _ = self.attach_sheet;

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
                WindowEvent::Moved(new_pos) => {
                    debug!("winhelper: moved event: x={} y={}", new_pos.x, new_pos.y);
                    if self.delay_setframe_ms > 0 && !self.suppress_events {
                        if let Some(win) = self.window.as_ref() {
                            let scale = win.scale_factor();
                            let lp = new_pos.to_logical::<f64>(scale);
                            // Initialize last_pos if missing
                            if self.last_pos.is_none()
                                && let Ok(p0) = win.outer_position()
                            {
                                let p0l = p0.to_logical::<f64>(scale);
                                self.last_pos = Some((p0l.x, p0l.y));
                            }
                            self.desired_pos = Some((lp.x, lp.y));
                            debug!(
                                "winhelper: intercept move -> desired=({:.1},{:.1}) last={:?}",
                                lp.x, lp.y, self.last_pos
                            );
                            if let Some((x, y)) = self.last_pos {
                                self.suppress_events = true;
                                win.set_outer_position(winit::dpi::LogicalPosition::new(x, y));
                                self.suppress_events = false;
                            }
                            self.apply_after =
                                Some(Instant::now() + crate::config::ms(self.delay_setframe_ms));
                            debug!(
                                "winhelper: scheduled apply_after at +{}ms",
                                self.delay_setframe_ms
                            );
                        }
                    } else if !self.suppress_events {
                        // Track last position when not delaying
                        if let Some(win) = self.window.as_ref() {
                            let scale = win.scale_factor();
                            let lp = new_pos.to_logical::<f64>(scale);
                            self.last_pos = Some((lp.x, lp.y));
                            debug!("winhelper: track move -> last=({:.1},{:.1})", lp.x, lp.y);
                        }
                    }
                }
                WindowEvent::Resized(new_size) => {
                    debug!(
                        "winhelper: resized event: w={} h={}",
                        new_size.width, new_size.height
                    );
                    if self.delay_setframe_ms > 0 && !self.suppress_events {
                        if let Some(win) = self.window.as_ref() {
                            let scale = win.scale_factor();
                            let lsz = new_size.to_logical::<f64>(scale);
                            if self.last_size.is_none() {
                                let s0 = win.inner_size().to_logical::<f64>(scale);
                                self.last_size = Some((s0.width, s0.height));
                            }
                            self.desired_size = Some((lsz.width, lsz.height));
                            debug!(
                                "winhelper: intercept resize -> desired=({:.1},{:.1}) last={:?}",
                                lsz.width, lsz.height, self.last_size
                            );
                            if let Some((w, h)) = self.last_size {
                                self.suppress_events = true;
                                let _ = win.request_inner_size(winit::dpi::LogicalSize::new(w, h));
                                self.suppress_events = false;
                            }
                            self.apply_after =
                                Some(Instant::now() + crate::config::ms(self.delay_setframe_ms));
                            debug!(
                                "winhelper: scheduled apply_after at +{}ms",
                                self.delay_setframe_ms
                            );
                        }
                    } else if !self.suppress_events {
                        // Track last size when not delaying
                        if let Some(win) = self.window.as_ref() {
                            let scale = win.scale_factor();
                            let lsz = new_size.to_logical::<f64>(scale);
                            self.last_size = Some((lsz.width, lsz.height));
                            debug!(
                                "winhelper: track resize -> last=({:.1},{:.1})",
                                lsz.width, lsz.height
                            );
                        }
                    }
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
            let now = Instant::now();
            if now >= self.deadline {
                elwt.exit();
                return;
            }
            if let Some(when) = self.apply_after {
                if now < when {
                    // Before apply: resist external changes by reverting to last
                    if let Some(win) = self.window.as_ref() {
                        if let (Some((lx, ly)), Some((lw, lh))) = (self.last_pos, self.last_size) {
                            let scale = win.scale_factor();
                            let p = win
                                .outer_position()
                                .ok()
                                .map(|p| p.to_logical::<f64>(scale));
                            let s = win.inner_size().to_logical::<f64>(scale);
                            if let Some(p) = p {
                                let dx = (p.x - lx).abs();
                                let dy = (p.y - ly).abs();
                                let dw = (s.width - lw).abs();
                                let dh = (s.height - lh).abs();
                                if dx > 0.5 || dy > 0.5 || dw > 0.5 || dh > 0.5 {
                                    debug!(
                                        "winhelper: revert drift dx={:.1} dy={:.1} dw={:.1} dh={:.1}",
                                        dx, dy, dw, dh
                                    );
                                    self.suppress_events = true;
                                    let _ = win
                                        .request_inner_size(winit::dpi::LogicalSize::new(lw, lh));
                                    win.set_outer_position(winit::dpi::LogicalPosition::new(
                                        lx, ly,
                                    ));
                                    self.suppress_events = false;
                                }
                            }
                        }
                    }
                } else {
                    // Apply time reached: prefer explicit target; else desired_*
                    if let Some(win) = self.window.as_ref() {
                        self.suppress_events = true;
                        if let Some((x, y, w, h)) = self.apply_target {
                            let _ = win.request_inner_size(winit::dpi::LogicalSize::new(w, h));
                            win.set_outer_position(winit::dpi::LogicalPosition::new(x, y));
                            self.last_pos = Some((x, y));
                            self.last_size = Some((w, h));
                            debug!(
                                "winhelper: explicit apply -> ({:.1},{:.1},{:.1},{:.1})",
                                x, y, w, h
                            );
                        } else if let Some((cols, rows, col, row)) = self.apply_grid {
                            // Compute target rect on current screen visible frame
                            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                                use objc2_app_kit::NSScreen;
                                // Use window center to pick screen
                                let scale = win.scale_factor();
                                let p = win
                                    .outer_position()
                                    .ok()
                                    .map(|p| p.to_logical::<f64>(scale))
                                    .unwrap_or(winit::dpi::LogicalPosition::new(0.0, 0.0));
                                let (vf_x, vf_y, vf_w, vf_h) = {
                                    let mut chosen = None;
                                    for s in NSScreen::screens(mtm).iter() {
                                        let fr = s.visibleFrame();
                                        let sx = fr.origin.x;
                                        let sy = fr.origin.y;
                                        let sw = fr.size.width;
                                        let sh = fr.size.height;
                                        if p.x >= sx
                                            && p.x <= sx + sw
                                            && p.y >= sy
                                            && p.y <= sy + sh
                                        {
                                            chosen = Some((sx, sy, sw, sh));
                                            break;
                                        }
                                    }
                                    chosen.or_else(|| {
                                        NSScreen::mainScreen(mtm).map(|scr| {
                                            let r = scr.visibleFrame();
                                            (r.origin.x, r.origin.y, r.size.width, r.size.height)
                                        })
                                    })
                                }
                                .unwrap_or((0.0, 0.0, 1440.0, 900.0));
                                let c = cols.max(1) as f64;
                                let r = rows.max(1) as f64;
                                let tile_w = (vf_w / c).floor().max(1.0);
                                let tile_h = (vf_h / r).floor().max(1.0);
                                let rem_w = vf_w - tile_w * (cols as f64);
                                let rem_h = vf_h - tile_h * (rows as f64);
                                let tx = vf_x + tile_w * (col as f64);
                                let tw = if col == cols.saturating_sub(1) {
                                    tile_w + rem_w
                                } else {
                                    tile_w
                                };
                                let ty = vf_y + tile_h * (row as f64);
                                let th = if row == rows.saturating_sub(1) {
                                    tile_h + rem_h
                                } else {
                                    tile_h
                                };
                                let _ =
                                    win.request_inner_size(winit::dpi::LogicalSize::new(tw, th));
                                win.set_outer_position(winit::dpi::LogicalPosition::new(tx, ty));
                                self.last_pos = Some((tx, ty));
                                self.last_size = Some((tw, th));
                                debug!(
                                    "winhelper: explicit apply (grid) -> ({:.1},{:.1},{:.1},{:.1})",
                                    tx, ty, tw, th
                                );
                            }
                        } else {
                            if let Some((w, h)) = self.desired_size.take() {
                                let _ = win.request_inner_size(winit::dpi::LogicalSize::new(w, h));
                                self.last_size = Some((w, h));
                            }
                            if let Some((x, y)) = self.desired_pos.take() {
                                win.set_outer_position(winit::dpi::LogicalPosition::new(x, y));
                                self.last_pos = Some((x, y));
                            }
                            debug!("winhelper: applied desired pos/size");
                        }
                        self.suppress_events = false;
                    }
                    self.apply_after = None;
                }
            }
            // Wake up at the next interesting time (apply_after or final deadline)
            let next = match self.apply_after {
                Some(t) => std::cmp::min(t, self.deadline),
                None => self.deadline,
            };
            elwt.set_control_flow(ControlFlow::WaitUntil(next));
        }
    }

    let mut app = HelperApp {
        window: None,
        title: title.to_string(),
        deadline: Instant::now() + config::ms(time_ms.max(1000)),
        delay_setframe_ms,
        delay_apply_ms,
        apply_target,
        apply_grid,
        last_pos: None,
        last_size: None,
        desired_pos: None,
        desired_size: None,
        apply_after: None,
        suppress_events: false,
        slot,
        grid,
        size,
        pos,
        label_text,
        error: None,
        start_minimized,
        start_zoomed,
        panel_nonmovable,
        attach_sheet,
    };
    let _ = event_loop.run_app(&mut app);
    if let Some(e) = app.error.take() {
        Err(e)
    } else {
        Ok(())
    }
}
