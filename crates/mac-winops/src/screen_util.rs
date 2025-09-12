use objc2_app_kit::NSScreen;
use objc2_foundation::MainThreadMarker;
use tracing::debug;

use crate::geom::{self, CGPoint};

/// Compute the visible frame (excluding menu bar and Dock) of the screen
/// containing `p`. Falls back to main screen when not found.
pub(crate) fn visible_frame_containing_point(
    mtm: MainThreadMarker,
    p: CGPoint,
) -> (f64, f64, f64, f64) {
    // Try to find a screen containing the point.
    let mut chosen = None;
    for s in NSScreen::screens(mtm).iter() {
        let fr = s.visibleFrame();
        let r = geom::Rect {
            x: fr.origin.x,
            y: fr.origin.y,
            w: fr.size.width,
            h: fr.size.height,
        };
        if geom::point_in_rect(p.x, p.y, &r) {
            chosen = Some(s);
            break;
        }
    }
    // Prefer the chosen screen; otherwise try main, then first.
    if let Some(scr) = chosen.or_else(|| {
        debug!(
            "visible_frame_containing_point: no screen contains point ({:.1},{:.1}); using main",
            p.x, p.y
        );
        NSScreen::mainScreen(mtm)
    }) {
        let r = scr.visibleFrame();
        return (r.origin.x, r.origin.y, r.size.width, r.size.height);
    }
    if let Some(s) = NSScreen::screens(mtm).iter().next() {
        debug!("visible_frame_containing_point: main screen unavailable; using first screen");
        let r = s.visibleFrame();
        return (r.origin.x, r.origin.y, r.size.width, r.size.height);
    }
    // As a last resort, return a zero rect to avoid panics.
    debug!("visible_frame_containing_point: no screens available; returning zero rect");
    (0.0, 0.0, 0.0, 0.0)
}
