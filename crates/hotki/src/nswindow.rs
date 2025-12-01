//! Minimal NSWindow helpers kept within the UI crate.
//!
//! These functions are used to tweak egui/eframe windows (HUD, notifications,
//! details) after creation. They require the AppKit main thread and are
//! intentionally lightweight now that window operations have moved out of
//! process.

use std::{error::Error as StdError, fmt, result::Result as StdResult};

use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplication, NSColor, NSWindowCollectionBehavior};
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

/// Apply full transparency and rounded corners to the window matching `title_match`.
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
            // SAFETY: AppKit main thread is enforced above; `clearColor` returns a
            // shared autoreleased NSColor.
            let clear = unsafe { NSColor::clearColor() };
            w.setBackgroundColor(Some(&clear));
            if let Some(view) = w.contentView() {
                view.setWantsLayer(true);
                // SAFETY: After `setWantsLayer(true)`, AppKit ensures a backing
                // layer exists; `layer()` returns an optional retained reference.
                let layer_opt = unsafe { view.layer() };
                if let Some(layer) = layer_opt {
                    layer.setMasksToBounds(true);
                    layer.setCornerRadius(radius);
                }
            }
            // SAFETY: Accessing window properties on AppKit main thread.
            let current_alpha = unsafe { w.alphaValue() };
            if (current_alpha - 1.0).abs() > 0.0001 {
                // SAFETY: AppKit main thread and valid `w`.
                unsafe { w.setAlphaValue(1.0) };
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
        let title = w.title();
        let is_match = autoreleasepool(|pool| unsafe { title.to_str(pool) == title_match });
        if is_match {
            // SAFETY: AppKit main thread and valid window instance.
            unsafe { w.setCollectionBehavior(NSWindowCollectionBehavior::CanJoinAllSpaces) };
        }
    }
    Ok(())
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
