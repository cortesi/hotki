//! Helper window (winit) used by smoketests to verify placement behaviors.
use std::time::{Duration, Instant};

use tracing::{debug, info};

use crate::{config, world};
use hotki_world::PlaceAttemptOptions;
use hotki_world_ids::WorldWindowId;

/// Target rect as ((x,y), (w,h), name) used for tween targets.
type TargetRect = ((f64, f64), (f64, f64), &'static str);

/// Run the helper window configured by the provided parameters.
#[allow(clippy::too_many_arguments)]
pub fn run_focus_winhelper(
    title: &str,
    time_ms: u64,
    delay_setframe_ms: u64,
    delay_apply_ms: u64,
    tween_ms: u64,
    apply_target: Option<(f64, f64, f64, f64)>,
    apply_grid: Option<(u32, u32, u32, u32)>,
    slot: Option<u8>,
    grid: Option<(u32, u32, u32, u32)>,
    size: Option<(f64, f64)>,
    pos: Option<(f64, f64)>,
    label_text: Option<String>,
    min_size: Option<(f64, f64)>,
    step_size: Option<(f64, f64)>,
    start_minimized: bool,
    start_zoomed: bool,
    panel_nonmovable: bool,
    panel_nonresizable: bool,
    attach_sheet: bool,
) -> Result<(), String> {
    // Create event loop after items below to satisfy clippy's items-after-statements lint.

    use std::{cmp::min, process::id, thread};

    use objc2::rc::autoreleasepool;
    use winit::{
        application::ApplicationHandler,
        dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize},
        event::WindowEvent,
        event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
        window::{Window, WindowId},
    };

    struct HelperApp {
        /// Handle to the helper window, if created.
        window: Option<Window>,
        /// Window title used to locate the NSWindow for tweaks.
        title: String,
        /// Time at which the helper should terminate.
        deadline: Instant,
        /// Delay before applying a set_frame operation (ms).
        delay_setframe_ms: u64,
        /// Delay before applying the main placement (ms).
        delay_apply_ms: u64,
        /// Tween duration for animated moves (ms).
        tween_ms: u64,
        /// Explicit target rect to apply, if present.
        apply_target: Option<(f64, f64, f64, f64)>,
        /// Grid parameters to compute target rect, if present.
        apply_grid: Option<(u32, u32, u32, u32)>,
        // Async-frame state
        /// Last observed window position.
        last_pos: Option<(f64, f64)>,
        /// Last observed window size.
        last_size: Option<(f64, f64)>,
        /// Desired position requested by the test.
        desired_pos: Option<(f64, f64)>,
        /// Desired size requested by the test.
        desired_size: Option<(f64, f64)>,
        /// Time at which to apply pending placement.
        apply_after: Option<Instant>,
        // Tween state
        /// Whether a tween animation is currently active.
        tween_active: bool,
        /// Tween start time.
        tween_start: Option<Instant>,
        /// Tween end time.
        tween_end: Option<Instant>,
        /// Starting position for tween.
        tween_from_pos: Option<(f64, f64)>,
        /// Starting size for tween.
        tween_from_size: Option<(f64, f64)>,
        /// Target position for tween.
        tween_to_pos: Option<(f64, f64)>,
        /// Target size for tween.
        tween_to_size: Option<(f64, f64)>,
        /// Suppress processing of window events while applying changes.
        suppress_events: bool,
        /// Optional 2x2 slot for placement.
        slot: Option<u8>,
        /// Optional grid spec for placement.
        grid: Option<(u32, u32, u32, u32)>,
        /// Optional explicit initial size.
        size: Option<(f64, f64)>,
        /// Optional explicit initial position.
        pos: Option<(f64, f64)>,
        /// Optional label text to display.
        label_text: Option<String>,
        /// Optional minimum content size.
        min_size: Option<(f64, f64)>,
        /// Fatal error encountered during setup.
        error: Option<String>,
        /// Start minimized if requested.
        start_minimized: bool,
        /// Start zoomed (macOS “zoomed”) if requested.
        start_zoomed: bool,
        /// Make the panel non-movable if requested.
        panel_nonmovable: bool,
        /// Make the panel non-resizable if requested.
        panel_nonresizable: bool,
        /// Attach a modal sheet to the helper window if requested.
        attach_sheet: bool,
        // Optional: round requested sizes to nearest multiples
        /// Width rounding step for requested sizes.
        step_w: f64,
        /// Height rounding step for requested sizes.
        step_h: f64,
    }

    impl HelperApp {
        /// Create the helper window with initial attributes.
        fn try_create_window(&self, elwt: &ActiveEventLoop) -> Result<Window, String> {
            use winit::dpi::LogicalSize;
            let attrs = Window::default_attributes()
                .with_title(self.title.clone())
                .with_visible(true)
                .with_decorations(false)
                .with_inner_size(LogicalSize::new(
                    self.size
                        .map(|s| s.0)
                        .unwrap_or(config::HELPER_WINDOW.width_px),
                    self.size
                        .map(|s| s.1)
                        .unwrap_or(config::HELPER_WINDOW.height_px),
                ));
            elwt.create_window(attrs).map_err(|e| e.to_string())
        }

        /// Bring the application to the foreground on resume.
        fn activate_app(&self) {
            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                unsafe { app.activate() };
            }
        }

        /// Enforce the configured minimum content size, if any.
        fn apply_min_size_if_requested(&self) {
            if let Some((min_w, min_h)) = self.min_size
                && let Some(mtm) = objc2_foundation::MainThreadMarker::new()
            {
                use objc2_foundation::NSSize;
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                for w in app.windows().iter() {
                    let t = w.title();
                    let is_match = autoreleasepool(|pool| unsafe { t.to_str(pool) == self.title });
                    if is_match {
                        unsafe {
                            w.setMinSize(NSSize::new(min_w, min_h));
                            w.setContentMinSize(NSSize::new(min_w, min_h));
                        }
                        break;
                    }
                }
            }
        }

        /// Make the panel non-movable if configured.
        fn apply_nonmovable_if_requested(&self) {
            if self.panel_nonmovable
                && let Some(mtm) = objc2_foundation::MainThreadMarker::new()
            {
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                let windows = app.windows();
                for w in windows.iter() {
                    let t = w.title();
                    let is_match = autoreleasepool(|pool| unsafe { t.to_str(pool) == self.title });
                    if is_match {
                        w.setMovable(false);
                        break;
                    }
                }
            }
        }

        /// Make the panel non-resizable if configured.
        fn apply_nonresizable_if_requested(&self) {
            if self.panel_nonresizable
                && let Some(mtm) = objc2_foundation::MainThreadMarker::new()
            {
                use objc2_app_kit::NSWindowStyleMask;
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                let windows = app.windows();
                for w in windows.iter() {
                    let t = w.title();
                    let is_match = autoreleasepool(|pool| unsafe { t.to_str(pool) == self.title });
                    if is_match {
                        let mut mask = w.styleMask();
                        mask.remove(NSWindowStyleMask::Resizable);
                        w.setStyleMask(mask);
                        break;
                    }
                }
            }
        }

        /// Poll the world snapshot to resolve the helper window's identifier.
        fn resolve_world_window(&self) -> Option<WorldWindowId> {
            let pid = id() as i32;
            match world::list_windows() {
                Ok(windows) => windows
                    .into_iter()
                    .find(|w| w.pid == pid && w.title == self.title)
                    .map(|w| WorldWindowId::new(w.pid, w.id)),
                Err(err) => {
                    tracing::debug!("winhelper: world snapshot failed: {}", err);
                    None
                }
            }
        }

        /// Perform the initial placement of the window.
        fn initial_placement(&self, win: &Window) {
            use winit::dpi::LogicalPosition;
            let pid = id() as i32;
            let _ =
                world::ensure_frontmost(pid, &self.title, 3, config::INPUT_DELAYS.retry_delay_ms);
            if let Some((cols, rows, col, row)) = self.grid {
                self.try_world_place(cols, rows, col, row, None);
            } else if let Some(slot) = self.slot {
                let (col, row) = match slot {
                    1 => (0, 0),
                    2 => (1, 0),
                    3 => (0, 1),
                    _ => (1, 1),
                };
                self.try_world_place(2, 2, col, row, None);
            } else if let Some((x, y)) = self.pos {
                win.set_outer_position(LogicalPosition::new(x, y));
            } else if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                // Fallback: bottom-right corner at a fixed small size on main screen.
                use objc2_app_kit::NSScreen;
                let margin: f64 = config::HELPER_WINDOW.margin_px;
                if let Some(scr) = NSScreen::mainScreen(mtm) {
                    let vf = scr.visibleFrame();
                    let w = config::HELPER_WINDOW.width_px;
                    let x = (vf.origin.x + vf.size.width - w - margin).max(0.0);
                    let y = (vf.origin.y + margin).max(0.0);
                    win.set_outer_position(LogicalPosition::new(x, y));
                }
            }
            // Apply style tweaks after initial placement to avoid interfering with it.
            self.apply_nonresizable_if_requested();
            self.apply_nonmovable_if_requested();
        }

        /// Retry world placement until the helper window is visible to the world snapshot.
        fn try_world_place(
            &self,
            cols: u32,
            rows: u32,
            col: u32,
            row: u32,
            options: Option<PlaceAttemptOptions>,
        ) {
            for attempt in 0..120 {
                if let Some(target) = self.resolve_world_window() {
                    match world::place_window(target, cols, rows, col, row, options.clone()) {
                        Ok(_) => return,
                        Err(err) => {
                            debug!(
                                "winhelper: world placement attempt {} failed: {}",
                                attempt, err
                            );
                        }
                    }
                }
                thread::sleep(Duration::from_millis(20));
            }
            debug!("winhelper: world placement giving up after retries");
        }

        /// Capture the starting geometry used by delayed/tweened placement logic.
        fn capture_initial_geometry(&mut self, win: &Window) {
            let scale = win.scale_factor();
            if let Ok(p) = win.outer_position() {
                let lp = p.to_logical::<f64>(scale);
                self.last_pos = Some((lp.x, lp.y));
            }
            let isz = win.inner_size();
            let lsz = isz.to_logical::<f64>(scale);
            self.last_size = Some((lsz.width, lsz.height));
        }

        /// Arm delayed application if explicitly configured.
        fn arm_delayed_apply_if_configured(&mut self) {
            if self.delay_apply_ms > 0 {
                self.apply_after = Some(Instant::now() + config::ms(self.delay_apply_ms));
                debug!(
                    "winhelper: armed delayed-apply +{}ms target={:?} grid={:?}",
                    self.delay_apply_ms, self.apply_target, self.apply_grid
                );
            }
        }

        /// Apply initial zoom/minimize state if requested.
        fn apply_initial_state_options(&self) {
            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                let windows = app.windows();
                for w in windows.iter() {
                    let t = w.title();
                    let is_match = autoreleasepool(|pool| unsafe { t.to_str(pool) == self.title });
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
        }

        /// Add a large centered label to the content view.
        fn add_centered_label(&self) {
            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                let windows = app.windows();
                for w in windows.iter() {
                    let title = w.title();
                    let is_match =
                        autoreleasepool(|pool| unsafe { title.to_str(pool) == self.title });
                    if is_match {
                        if let Some(view) = w.contentView() {
                            use objc2::rc::Retained;
                            use objc2_app_kit::{NSColor, NSFont, NSTextAlignment, NSTextField};
                            use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
                            let label_str = self.compute_label_text();
                            let ns = NSString::from_str(&label_str);
                            let label: Retained<NSTextField> =
                                unsafe { NSTextField::labelWithString(&ns, mtm) };
                            let vframe = view.frame();
                            let vw = vframe.size.width;
                            let vh = vframe.size.height;
                            let base = vw.min(vh) * 0.35;
                            let font = unsafe { NSFont::boldSystemFontOfSize(base) };
                            unsafe { label.setFont(Some(&font)) };
                            unsafe { label.setAlignment(NSTextAlignment::Center) };
                            let color = unsafe { NSColor::whiteColor() };
                            unsafe { label.setTextColor(Some(&color)) };
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
        }

        /// Handle a `WindowEvent::Moved`.
        fn on_moved(&mut self, new_pos: PhysicalPosition<i32>) {
            use winit::dpi::LogicalPosition;
            debug!("winhelper: moved event: x={} y={}", new_pos.x, new_pos.y);
            if (self.delay_setframe_ms > 0 || self.tween_ms > 0) && !self.suppress_events {
                if let Some(win) = self.window.as_ref() {
                    let scale = win.scale_factor();
                    let lp = new_pos.to_logical::<f64>(scale);
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
                        win.set_outer_position(LogicalPosition::new(x, y));
                        self.suppress_events = false;
                    }
                    if self.tween_ms > 0 {
                        if self.delay_apply_ms > 0
                            && (self.apply_target.is_some() || self.apply_grid.is_some())
                        {
                            self.apply_after =
                                Some(Instant::now() + config::ms(self.delay_apply_ms));
                        } else {
                            let now = Instant::now();
                            self.ensure_tween_started_pos(now);
                            self.tween_to_pos = self.desired_pos;
                            self.apply_after = Some(now);
                        }
                    } else {
                        self.apply_after =
                            Some(Instant::now() + config::ms(self.delay_setframe_ms));
                        debug!(
                            "winhelper: scheduled apply_after at +{}ms",
                            self.delay_setframe_ms
                        );
                    }
                }
            } else if !self.suppress_events
                && let Some(win) = self.window.as_ref()
            {
                let scale = win.scale_factor();
                let lp = new_pos.to_logical::<f64>(scale);
                self.last_pos = Some((lp.x, lp.y));
                debug!("winhelper: track move -> last=({:.1},{:.1})", lp.x, lp.y);
            }
        }

        /// Handle a `WindowEvent::Resized`.
        fn on_resized(&mut self, new_size: PhysicalSize<u32>) {
            use winit::dpi::LogicalSize;
            debug!(
                "winhelper: resized event: w={} h={}",
                new_size.width, new_size.height
            );
            if (self.delay_setframe_ms > 0 || self.tween_ms > 0) && !self.suppress_events {
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
                        let _maybe_size = win.request_inner_size(LogicalSize::new(w, h));
                        self.suppress_events = false;
                    }
                    if self.tween_ms > 0 {
                        if self.delay_apply_ms > 0
                            && (self.apply_target.is_some() || self.apply_grid.is_some())
                        {
                            self.apply_after =
                                Some(Instant::now() + config::ms(self.delay_apply_ms));
                        } else {
                            let now = Instant::now();
                            self.ensure_tween_started_size(now);
                            self.tween_to_size = self.desired_size;
                            self.apply_after = Some(now);
                        }
                    } else {
                        self.apply_after =
                            Some(Instant::now() + config::ms(self.delay_setframe_ms));
                        debug!(
                            "winhelper: scheduled apply_after at +{}ms",
                            self.delay_setframe_ms
                        );
                    }
                }
            } else if !self.suppress_events
                && let Some(win) = self.window.as_ref()
            {
                let scale = win.scale_factor();
                let lsz = new_size.to_logical::<f64>(scale);
                self.last_size = Some((lsz.width, lsz.height));
                debug!(
                    "winhelper: track resize -> last=({:.1},{:.1})",
                    lsz.width, lsz.height
                );
            }
        }

        /// Handle a `WindowEvent::Focused`.
        fn on_focused(&self, focused: bool) {
            info!(title = %self.title, focused, "winhelper: focus event");
            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                let windows = app.windows();
                for w in windows.iter() {
                    let title = w.title();
                    let is_match =
                        autoreleasepool(|pool| unsafe { title.to_str(pool) == self.title });
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
        /// Return the visible frame of the screen containing the given window.
        fn active_visible_frame_for_window(&self, win: &Window) -> (f64, f64, f64, f64) {
            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                use objc2_app_kit::NSScreen;
                let scale = win.scale_factor();
                let p = win
                    .outer_position()
                    .ok()
                    .map(|p| p.to_logical::<f64>(scale))
                    .unwrap_or(LogicalPosition::new(0.0, 0.0));
                let mut chosen: Option<(f64, f64, f64, f64)> = None;
                for s in NSScreen::screens(mtm).iter() {
                    let fr = s.visibleFrame();
                    let sx = fr.origin.x;
                    let sy = fr.origin.y;
                    let sw = fr.size.width;
                    let sh = fr.size.height;
                    if p.x >= sx && p.x <= sx + sw && p.y >= sy && p.y <= sy + sh {
                        chosen = Some((sx, sy, sw, sh));
                        break;
                    }
                }
                if let Some(v) = chosen {
                    return v;
                }
                if let Some(scr) = NSScreen::mainScreen(mtm) {
                    let r = scr.visibleFrame();
                    return (r.origin.x, r.origin.y, r.size.width, r.size.height);
                }
            }
            (0.0, 0.0, 1440.0, 900.0)
        }

        /// Compute the rectangle for a tile within a grid on the active screen.
        fn grid_rect(
            &self,
            win: &Window,
            cols: u32,
            rows: u32,
            col: u32,
            row: u32,
        ) -> (f64, f64, f64, f64) {
            let (vf_x, vf_y, vf_w, vf_h) = self.active_visible_frame_for_window(win);
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
            (tx, ty, tw, th)
        }

        /// Compute the label text to display in the helper window.
        /// Helper selectors are inlined at call sites to avoid borrow conflicts.
        fn compute_label_text(&self) -> String {
            if let Some(ref t) = self.label_text {
                return t.clone();
            }
            if let Some((cols, rows, col, row)) = self.grid {
                if cols == 2 && rows == 2 {
                    return match (col, row) {
                        (0, 0) => "TL".into(),
                        (1, 0) => "TR".into(),
                        (0, 1) => "BL".into(),
                        _ => "BR".into(),
                    };
                }
                return self.title.clone();
            }
            if let Some(slot) = self.slot {
                return match slot {
                    1 => "TL".into(),
                    2 => "TR".into(),
                    3 => "BL".into(),
                    _ => "BR".into(),
                };
            }
            self.title.clone()
        }

        /// Ensure tween state is initialized for position changes.
        fn ensure_tween_started_pos(&mut self, now: Instant) {
            if !self.tween_active {
                self.tween_active = true;
                self.tween_start = Some(now);
                self.tween_end = Some(now + config::ms(self.tween_ms));
                self.tween_from_pos = self.last_pos;
            }
        }

        /// Ensure tween state is initialized for size changes.
        fn ensure_tween_started_size(&mut self, now: Instant) {
            if !self.tween_active {
                self.tween_active = true;
                self.tween_start = Some(now);
                self.tween_end = Some(now + config::ms(self.tween_ms));
                self.tween_from_size = self.last_size;
            }
        }

        /// Optionally round a size to the configured step.
        fn rounded_size(&self, w: f64, h: f64) -> (f64, f64) {
            if self.step_w > 0.0 && self.step_h > 0.0 {
                (
                    (w / self.step_w).round() * self.step_w,
                    (h / self.step_h).round() * self.step_h,
                )
            } else {
                (w, h)
            }
        }

        /// Select the target rectangle to apply (explicit target, grid, or desired geometry).
        fn select_apply_target(&self, win: &Window) -> Option<TargetRect> {
            if let Some((x, y, w, h)) = self.apply_target {
                return Some(((x, y), (w, h), "target"));
            }
            if let Some((c, r, ic, ir)) = self.apply_grid {
                let (tx, ty, tw, th) = self.grid_rect(win, c, r, ic, ir);
                return Some(((tx, ty), (tw, th), "grid"));
            }
            let pos = self.desired_pos.or(self.last_pos);
            let size = self.desired_size.or(self.last_size);
            match (pos, size) {
                (Some(p), Some(s)) => Some((p, s, "desired")),
                _ => None,
            }
        }

        /// Initialize tween destination from a target rectangle.
        fn set_tween_target_from(&mut self, target: TargetRect) {
            let ((x, y), (w, h), kind) = target;
            self.tween_to_pos = Some((x, y));
            self.tween_to_size = Some((w, h));
            debug!(
                "winhelper: tween-start ({}) -> ({:.1},{:.1},{:.1},{:.1})",
                kind, x, y, w, h
            );
        }

        /// Compute tween progress in the range [0.0, 1.0].
        fn tween_progress(&self, now: Instant) -> f64 {
            let start = match self.tween_start {
                Some(s) => s,
                None => return 1.0,
            };
            let end = match self.tween_end {
                Some(e) => e,
                None => return 1.0,
            };
            let total = end.saturating_duration_since(start);
            if total.as_millis() == 0 {
                1.0
            } else {
                let elapsed = now.saturating_duration_since(start).as_secs_f64();
                (elapsed / total.as_secs_f64()).clamp(0.0, 1.0)
            }
        }

        /// Interpolate position and size based on tween progress `t`.
        fn tween_interpolate(&self, t: f64) -> (f64, f64, f64, f64) {
            let (mut nx, mut ny) = self.last_pos.unwrap_or((0.0, 0.0));
            let (mut nw, mut nh) = self.last_size.unwrap_or((
                config::HELPER_WINDOW.width_px,
                config::HELPER_WINDOW.height_px,
            ));
            if let (Some((fx, fy)), Some((tx, ty))) = (self.tween_from_pos, self.tween_to_pos) {
                nx = fx + (tx - fx) * t;
                ny = fy + (ty - fy) * t;
            }
            if let (Some((fw, fh)), Some((tw, th))) = (self.tween_from_size, self.tween_to_size) {
                nw = fw + (tw - fw) * t;
                nh = fh + (th - fh) * t;
            }
            (nx, ny, nw, nh)
        }

        /// Revert minor drift that may occur while testing async placement.
        fn revert_drift_if_needed(&mut self) {
            if let Some(win) = self.window.as_ref()
                && let (Some((lx, ly)), Some((lw, lh))) = (self.last_pos, self.last_size)
            {
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
                        let _maybe_size = win.request_inner_size(LogicalSize::new(lw, lh));
                        win.set_outer_position(LogicalPosition::new(lx, ly));
                        self.suppress_events = false;
                    }
                }
            }
        }

        /// Apply a single tween step, updating window position/size.
        fn apply_tween_step(&mut self) {
            let now = Instant::now();
            if let Some(win) = self.window.as_ref() {
                let target = self.select_apply_target(win);
                if !self.tween_active {
                    self.tween_active = true;
                    self.tween_start = Some(now);
                    self.tween_end = Some(now + config::ms(self.tween_ms));
                    self.tween_from_pos = self.last_pos;
                    self.tween_from_size = self.last_size;
                    if let Some(target) = target {
                        self.set_tween_target_from(target);
                    }
                }
            }
            if let Some(win2) = self.window.as_ref() {
                let t = self.tween_progress(now);
                let (nx, ny, nw, nh) = self.tween_interpolate(t);
                let (rw, rh) = self.rounded_size(nw, nh);
                let _maybe_size = win2.request_inner_size(LogicalSize::new(rw, rh));
                win2.set_outer_position(LogicalPosition::new(nx, ny));
            }
            if self.tween_start.is_some() && self.tween_end.is_some() {
                let t_done = self.tween_progress(Instant::now());
                if (t_done - 1.0).abs() < f64::EPSILON {
                    if let Some((w, h)) = self.tween_to_size {
                        let (_rw, _rh) = self.rounded_size(w, h);
                        self.last_size = Some((w, h));
                    }
                    self.last_pos = self.tween_to_pos.or(self.last_pos);
                    self.tween_active = false;
                    self.tween_start = None;
                    self.tween_end = None;
                    self.tween_from_pos = None;
                    self.tween_from_size = None;
                    self.tween_to_pos = None;
                    self.tween_to_size = None;
                    self.apply_after = None;
                } else {
                    self.apply_after = Some(now + config::ms(16));
                }
            }
        }

        /// Apply target geometry immediately without tweening.
        fn apply_immediate(&mut self) {
            if let Some(win) = self.window.as_ref() {
                if let Some((x, y, w, h)) = self.apply_target {
                    let (rw, rh) = self.rounded_size(w, h);
                    // Ignore the returned size; it is advisory for winit.
                    let _maybe_size = win.request_inner_size(LogicalSize::new(rw, rh));
                    win.set_outer_position(LogicalPosition::new(x, y));
                    self.last_pos = Some((x, y));
                    self.last_size = Some((rw, rh));
                    debug!(
                        "winhelper: explicit apply (explicit) -> ({:.1},{:.1},{:.1},{:.1})",
                        x, y, rw, rh
                    );
                } else if let Some((cols, rows, col, row)) = self.apply_grid {
                    let (tx, ty, tw, th) = self.grid_rect(win, cols, rows, col, row);
                    let (rw, rh) = self.rounded_size(tw, th);
                    let _maybe_size = win.request_inner_size(LogicalSize::new(rw, rh));
                    win.set_outer_position(LogicalPosition::new(tx, ty));
                    self.last_pos = Some((tx, ty));
                    self.last_size = Some((rw, rh));
                    debug!(
                        "winhelper: explicit apply (grid) -> ({:.1},{:.1},{:.1},{:.1})",
                        tx, ty, rw, rh
                    );
                } else {
                    let desired_size_val = self.desired_size;
                    let desired_pos_val = self.desired_pos;
                    if let Some(win2) = self.window.as_ref() {
                        if let Some((w, h)) = desired_size_val {
                            let (rw, rh) = self.rounded_size(w, h);
                            let _maybe_size = win2.request_inner_size(LogicalSize::new(rw, rh));
                            self.last_size = Some((rw, rh));
                            self.desired_size = None;
                        }
                        if let Some((x, y)) = desired_pos_val {
                            win2.set_outer_position(LogicalPosition::new(x, y));
                            self.last_pos = Some((x, y));
                            self.desired_pos = None;
                        }
                        debug!("winhelper: applied desired pos/size");
                    }
                }
            }
            // Ensure we clear any pending apply-after state after applying.
            self.apply_after = None;
        }

        /// Apply placement when the apply deadline has been reached.
        fn process_apply_ready(&mut self) {
            if self.window.is_none() {
                self.apply_after = None;
                return;
            }
            self.suppress_events = true;
            if self.tween_ms > 0 {
                self.apply_tween_step();
            } else {
                self.apply_immediate();
            }
            self.suppress_events = false;
        }
    }

    impl ApplicationHandler for HelperApp {
        fn resumed(&mut self, elwt: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let win = match self.try_create_window(elwt) {
                Ok(w) => w,
                Err(e) => {
                    self.error = Some(format!("winhelper: failed to create window: {}", e));
                    elwt.exit();
                    return;
                }
            };
            self.activate_app();
            self.apply_min_size_if_requested();
            self.apply_nonmovable_if_requested();
            self.initial_placement(&win);
            // Allow registration to settle before adding label.
            thread::sleep(config::ms(
                config::INPUT_DELAYS.window_registration_delay_ms,
            ));
            self.capture_initial_geometry(&win);
            self.arm_delayed_apply_if_configured();
            let _ = self.attach_sheet; // placeholder hook
            self.apply_initial_state_options();
            self.add_centered_label();
            self.window = Some(win);
        }
        fn window_event(&mut self, elwt: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
            match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::Moved(pos) => self.on_moved(pos),
                WindowEvent::Resized(sz) => self.on_resized(sz),
                WindowEvent::Focused(f) => self.on_focused(f),
                _ => {}
            }
        }
        fn about_to_wait(&mut self, elwt: &ActiveEventLoop) {
            let now = Instant::now();
            if now >= self.deadline {
                elwt.exit();
                return;
            }
            match self.apply_after {
                Some(when) if now < when => self.revert_drift_if_needed(),
                _ => self.process_apply_ready(),
            }
            // Wake up at the next interesting time (apply_after or final deadline)
            let next = match self.apply_after {
                Some(t) => min(t, self.deadline),
                None => self.deadline,
            };
            elwt.set_control_flow(ControlFlow::WaitUntil(next));
        }
    }

    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut app = HelperApp {
        window: None,
        title: title.to_string(),
        deadline: Instant::now() + config::ms(time_ms.max(1000)),
        delay_setframe_ms,
        delay_apply_ms,
        tween_ms,
        apply_target,
        apply_grid,
        last_pos: None,
        last_size: None,
        desired_pos: None,
        desired_size: None,
        apply_after: None,
        tween_active: false,
        tween_start: None,
        tween_end: None,
        tween_from_pos: None,
        tween_from_size: None,
        tween_to_pos: None,
        tween_to_size: None,
        suppress_events: false,
        slot,
        grid,
        size,
        pos,
        label_text,
        min_size,
        error: None,
        start_minimized,
        start_zoomed,
        panel_nonmovable,
        panel_nonresizable,
        attach_sheet,
        step_w: step_size.map(|s| s.0).unwrap_or(0.0),
        step_h: step_size.map(|s| s.1).unwrap_or(0.0),
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    if let Some(e) = app.error.take() {
        Err(e)
    } else {
        Ok(())
    }
}
