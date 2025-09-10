//! Screen helpers.
//!
//! - `active_frame()`: visible frame of the active screen at mouse location.
//! - `list_display_bounds()`: bounds for all active displays with IDs.

/// Get the active screen frame as `(x, y, w, h, global_top)`.
///
/// Delegates to the AppKit-backed implementation in `nswindow`.
pub fn active_frame() -> (f32, f32, f32, f32, f32) {
    crate::nswindow::active_screen_frame()
}

/// List active displays and their bounds as integer rectangles.
///
/// Returns a vector of `(id, x, y, w, h)` where `id` is the CGDirectDisplayID.
pub fn list_display_bounds() -> Vec<(u32, i32, i32, i32, i32)> {
    use core_graphics::display as cgdisp;
    let mut ids: [u32; 16] = [0; 16];
    let mut count: u32 = 0;
    let err = unsafe {
        cgdisp::CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count as *mut u32)
    };
    if err != 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(count as usize);
    for &id in ids.iter().take(count as usize) {
        let r = unsafe { cgdisp::CGDisplayBounds(id) };
        // CoreGraphics uses f64 CGRect; convert to i32 pixel bounds.
        let x = r.origin.x.round() as i32;
        let y = r.origin.y.round() as i32;
        let w = r.size.width.round() as i32;
        let h = r.size.height.round() as i32;
        out.push((id, x, y, w, h));
    }
    out
}
