use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplication, NSColor, NSEvent, NSScreen, NSWindowCollectionBehavior};
use objc2_foundation::MainThreadMarker;

use crate::error::{Error, Result};

/// Apply full transparency and rounded corners to an NSWindow with the given title.
///
/// Requires AppKit main thread.
pub fn apply_transparent_rounded(title_match: &str, radius: f64) -> Result<()> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(Error::MainThread);
    };
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    for w in windows.iter() {
        let title = w.title();
        let is_match = autoreleasepool(|pool| unsafe { title.to_str(pool) == title_match });
        if is_match {
            w.setOpaque(false);
            w.setHasShadow(false);
            let clear = unsafe { NSColor::clearColor() };
            w.setBackgroundColor(Some(&clear));
            if let Some(view) = w.contentView() {
                view.setWantsLayer(true);
                let layer_opt = unsafe { view.layer() };
                if let Some(layer) = layer_opt {
                    layer.setMasksToBounds(true);
                    let _ = radius;
                }
            }
            let current_alpha = unsafe { w.alphaValue() };
            if (current_alpha - 1.0).abs() > 0.0001 {
                unsafe { w.setAlphaValue(1.0) };
            }
        }
    }
    Ok(())
}

/// Set a window (by title) to appear on all Spaces/desktops.
///
/// Requires AppKit main thread.
pub fn set_on_all_spaces(title_match: &str) -> Result<()> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(Error::MainThread);
    };
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    for w in windows.iter() {
        let title = w.title();
        let is_match = autoreleasepool(|pool| unsafe { title.to_str(pool) == title_match });
        if is_match {
            unsafe { w.setCollectionBehavior(NSWindowCollectionBehavior::CanJoinAllSpaces) };
        }
    }
    Ok(())
}

/// Return the frame (x, y, w, h) for an NSWindow matching `title_match` using
/// AppKit coordinates (origin at bottom-left). Returns `None` if not found.
///
/// Requires AppKit main thread; returns `None` if not on main thread.
pub fn frame_by_title(title_match: &str) -> Option<(f32, f32, f32, f32)> {
    let mtm = MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    for w in windows.iter() {
        let title = w.title();
        let is_match = autoreleasepool(|pool| unsafe { title.to_str(pool) == title_match });
        if is_match {
            let fr = w.frame();
            return Some((
                fr.origin.x as f32,
                fr.origin.y as f32,
                fr.size.width as f32,
                fr.size.height as f32,
            ));
        }
    }
    None
}

/// Compute the active screen frame at the mouse location as `(x, y, w, h, global_top)`.
///
/// `global_top` is the maximum top Y across all screens, used to convert to
/// top-left coordinate space.
pub fn active_screen_frame() -> (f32, f32, f32, f32, f32) {
    unsafe {
        // Fall back to a reasonable default frame if not on the main thread or no screens are found.
        const DEFAULT_W: f32 = 1440.0;
        const DEFAULT_H: f32 = 900.0;
        let Some(mtm) = MainThreadMarker::new() else {
            return (0.0, 0.0, DEFAULT_W, DEFAULT_H, DEFAULT_H);
        };
        let screens = NSScreen::screens(mtm);
        let mut global_top: f32 = f32::MIN;
        for scr in screens.iter() {
            let fr = scr.frame();
            let top = fr.origin.y as f32 + fr.size.height as f32;
            if top > global_top {
                global_top = top;
            }
        }

        let mouse = NSEvent::mouseLocation();
        let mut chosen_frame = None;
        for scr in screens.iter() {
            let fr = scr.frame();
            let x = fr.origin.x as f32;
            let y = fr.origin.y as f32;
            let w = fr.size.width as f32;
            let h = fr.size.height as f32;
            if mouse.x as f32 >= x
                && mouse.x as f32 <= x + w
                && mouse.y as f32 >= y
                && mouse.y as f32 <= y + h
            {
                chosen_frame = Some(fr);
                break;
            }
        }
        if let Some(fr) = chosen_frame {
            return (
                fr.origin.x as f32,
                fr.origin.y as f32,
                fr.size.width as f32,
                fr.size.height as f32,
                global_top,
            );
        }
        if let Some(main) = NSScreen::mainScreen(mtm) {
            let fr = main.frame();
            return (
                fr.origin.x as f32,
                fr.origin.y as f32,
                fr.size.width as f32,
                fr.size.height as f32,
                global_top,
            );
        }
        if let Some(first) = screens.iter().next() {
            let fr = first.frame();
            return (
                fr.origin.x as f32,
                fr.origin.y as f32,
                fr.size.width as f32,
                fr.size.height as f32,
                global_top,
            );
        }
        // No screens found: return a default frame at origin with a sensible size.
        (0.0, 0.0, DEFAULT_W, DEFAULT_H, DEFAULT_H)
    }
}
