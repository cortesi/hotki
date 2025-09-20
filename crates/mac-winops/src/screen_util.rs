use objc2_app_kit::NSScreen;
use objc2_foundation::MainThreadMarker;
use tracing::debug;

use crate::geom::{self, Point, Rect};

/// Compute the visible frame (excluding menu bar and Dock) of the screen
/// containing `p`. Falls back to main screen when not found.
pub(crate) fn visible_frame_containing_point(mtm: MainThreadMarker, p: Point) -> Rect {
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
        if r.contains(p.x, p.y) {
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
        return Rect {
            x: r.origin.x,
            y: r.origin.y,
            w: r.size.width,
            h: r.size.height,
        };
    }
    if let Some(s) = NSScreen::screens(mtm).iter().next() {
        debug!("visible_frame_containing_point: main screen unavailable; using first screen");
        let r = s.visibleFrame();
        return Rect {
            x: r.origin.x,
            y: r.origin.y,
            w: r.size.width,
            h: r.size.height,
        };
    }
    // As a last resort, return a zero rect to avoid panics.
    debug!("visible_frame_containing_point: no screens available; returning zero rect");
    Rect {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    }
}

/// Resolve the backing scale factor for the screen containing `p`.
pub(crate) fn scale_factor_containing_point(mtm: MainThreadMarker, p: Point) -> f64 {
    let mut chosen = None;
    for s in NSScreen::screens(mtm).iter() {
        let fr = s.visibleFrame();
        let r = geom::Rect {
            x: fr.origin.x,
            y: fr.origin.y,
            w: fr.size.width,
            h: fr.size.height,
        };
        if r.contains(p.x, p.y) {
            chosen = Some(s);
            break;
        }
    }

    if let Some(scr) = chosen
        .or_else(|| NSScreen::mainScreen(mtm))
        .or_else(|| NSScreen::screens(mtm).iter().next())
    {
        return scr.backingScaleFactor();
    }

    1.0
}

/// Convert a rectangle expressed in screen‑local coordinates to global
/// coordinates by adding the screen origin.
///
/// Parameters:
/// - `local`: Rectangle with `x`/`y` relative to the screen/frame origin
///   (bottom‑left origin, AppKit/AX coordinate space).
/// - `screen_origin_x`, `screen_origin_y`: The global origin of the screen
///   or the visible frame the `local` rect is relative to.
///
/// Returns a new `Rect` in global coordinates.
pub fn globalize_rect(local: Rect, screen_origin_x: f64, screen_origin_y: f64) -> Rect {
    Rect {
        x: screen_origin_x + local.x,
        y: screen_origin_y + local.y,
        w: local.w,
        h: local.h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globalize_rect_adds_origin() {
        let local = Rect {
            x: 10.0,
            y: 20.0,
            w: 300.0,
            h: 400.0,
        };
        let g = globalize_rect(local, 100.0, 200.0);
        assert_eq!(g.x, 110.0);
        assert_eq!(g.y, 220.0);
        assert_eq!(g.w, 300.0);
        assert_eq!(g.h, 400.0);
    }
}
