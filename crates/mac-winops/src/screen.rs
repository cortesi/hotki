//! Screen helpers for UI placement.
//!
//! Provides the active screen frame at the current mouse location.

/// Get the active screen frame as `(x, y, w, h, global_top)`.
///
/// Delegates to the AppKit-backed implementation in `nswindow`.
pub fn active_frame() -> (f32, f32, f32, f32, f32) {
    crate::nswindow::active_screen_frame()
}
