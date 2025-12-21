//! Minimal NSWindow helpers kept within the UI crate.
//!
//! These functions are used to tweak egui/eframe windows (HUD, notifications,
//! details) after creation. They require the AppKit main thread and are
//! intentionally lightweight now that window operations have moved out of
//! process.

use std::{error::Error as StdError, fmt, result::Result as StdResult};

use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplication, NSColor, NSWindow, NSWindowCollectionBehavior};
use objc2_foundation::MainThreadMarker;

/// Errors that can occur while mutating NSWindow attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Operation requires the AppKit main thread.
    MainThread,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MainThread => write!(f, "operation requires AppKit main thread"),
        }
    }
}

impl StdError for Error {}

/// Convenient result alias for NSWindow helper operations.
pub type Result<T> = StdResult<T, Error>;

/// Return true if the window title matches the provided string.
fn window_title_matches(window: &NSWindow, title_match: &str) -> bool {
    let title = window.title();
    autoreleasepool(|pool| unsafe { title.to_str(pool) == title_match })
}

/// Apply full transparency and rounded corners to the window matching `title_match`.
pub fn apply_transparent_rounded(title_match: &str, radius: f64) -> Result<()> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(Error::MainThread);
    };
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    for w in windows.iter() {
        let window = &*w;
        if window_title_matches(window, title_match) {
            window.setOpaque(false);
            window.setHasShadow(false);
            let clear = NSColor::clearColor();
            window.setBackgroundColor(Some(&clear));
            if let Some(view) = window.contentView() {
                view.setWantsLayer(true);
                let layer_opt = view.layer();
                if let Some(layer) = layer_opt {
                    layer.setMasksToBounds(true);
                    layer.setCornerRadius(radius);
                }
            }
            let current_alpha = window.alphaValue();
            if (current_alpha - 1.0).abs() > 0.0001 {
                window.setAlphaValue(1.0);
            }
        }
    }
    Ok(())
}

/// Mark a window (by title) to appear on all Spaces/desktops.
pub fn set_on_all_spaces(title_match: &str) -> Result<()> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(Error::MainThread);
    };
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    for w in windows.iter() {
        let window = &*w;
        if window_title_matches(window, title_match) {
            window.setCollectionBehavior(NSWindowCollectionBehavior::CanJoinAllSpaces);
        }
    }
    Ok(())
}

/// Disable AppKit cursor rects for the window matching `title_match`.
///
/// The Details window was showing rapid cursor flicker in interactive regions
/// (tabs, scroll areas, selectable text) even when egui reported a stable
/// I-beam and AppKit `currentCursor` also reported I-beam. That combination
/// strongly suggests AppKit's cursor-rect system is periodically reasserting
/// the default cursor during the display cycle, fighting with egui/winit's
/// explicit cursor setting. Disabling cursor rects at the window level
/// removes that competing mechanism so egui becomes the sole cursor owner.
///
/// Returns `Ok(true)` if a window was found and updated.
pub fn disable_cursor_rects(title_match: &str) -> Result<bool> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(Error::MainThread);
    };
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    for w in windows.iter() {
        let window = &*w;
        if window_title_matches(window, title_match) {
            window.disableCursorRects();
            return Ok(true);
        }
    }
    Ok(false)
}

/// Re-enable AppKit cursor rects for the window matching `title_match`.
///
/// This is the counterpart to `disable_cursor_rects`, restoring default
/// AppKit cursor-rect behavior once the Details window is hidden.
///
/// Returns `Ok(true)` if a window was found and updated.
pub fn enable_cursor_rects(title_match: &str) -> Result<bool> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(Error::MainThread);
    };
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    for w in windows.iter() {
        let window = &*w;
        if window_title_matches(window, title_match) {
            window.enableCursorRects();
            return Ok(true);
        }
    }
    Ok(false)
}

/// Return the frame `(x, y, w, h)` for the window matching `title_match`.
/// Coordinates use the AppKit bottom-left origin. Returns `None` if not
/// found or when not running on the main thread.
#[must_use]
pub fn frame_by_title(title_match: &str) -> Option<(f32, f32, f32, f32)> {
    let mtm = MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    for w in windows.iter() {
        let window = &*w;
        if window_title_matches(window, title_match) {
            let fr = window.frame();
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
