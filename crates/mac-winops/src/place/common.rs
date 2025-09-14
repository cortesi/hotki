use tracing::debug;

use crate::geom::{self, Rect};

/// Epsilon tolerance (in points) used to verify post‑placement position and size.
pub(super) const VERIFY_EPS: f64 = 2.0;

pub(super) const POLL_SLEEP_MS: u64 = 25;
pub(super) const POLL_TOTAL_MS: u64 = 400;

/// Logical axis used for corrective nudges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Axis {
    X,
    Y,
}

/// Options controlling placement attempts and fallback used primarily by tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlaceAttemptOptions {
    /// Force a second attempt with size->pos even if the first converged.
    pub force_second_attempt: bool,
    /// Disable size->pos retry; only attempt pos->size.
    pub pos_first_only: bool,
    /// Force shrink->move->grow fallback even if dual-order converged.
    pub force_shrink_move_grow: bool,
}

#[inline]
pub(super) fn sleep_ms(ms: u64) {
    use std::{thread::sleep, time::Duration};
    sleep(Duration::from_millis(ms));
}

#[inline]
pub(super) fn rect_from(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { x, y, w, h }
}

#[inline]
pub(super) fn diffs(a: &Rect, b: &Rect) -> (f64, f64, f64, f64) {
    (
        (a.x - b.x).abs(),
        (a.y - b.y).abs(),
        (a.w - b.w).abs(),
        (a.h - b.h).abs(),
    )
}

#[inline]
pub(super) fn within_eps(d: (f64, f64, f64, f64), eps: f64) -> bool {
    d.0 <= eps && d.1 <= eps && d.2 <= eps && d.3 <= eps
}

#[inline]
pub(super) fn one_axis_off(d: (f64, f64, f64, f64), eps: f64) -> Option<Axis> {
    let x_ok = d.0 <= eps && d.2 <= eps; // dx,dw within eps
    let y_ok = d.1 <= eps && d.3 <= eps; // dy,dh within eps
    if x_ok && !y_ok {
        Some(Axis::Y)
    } else if y_ok && !x_ok {
        Some(Axis::X)
    } else {
        None
    }
}

#[inline]
pub(super) fn clamp_flags(got: &Rect, vf: &Rect, eps: f64) -> crate::error::ClampFlags {
    crate::error::ClampFlags {
        left: geom::approx_eq(got.left(), vf.left(), eps),
        right: geom::approx_eq(got.right(), vf.right(), eps),
        bottom: geom::approx_eq(got.bottom(), vf.bottom(), eps),
        top: geom::approx_eq(got.top(), vf.top(), eps),
    }
}

/// Decide the initial setter ordering based on cached settable flags.
/// Returns `true` for `pos->size`, `false` for `size->pos`.
#[inline]
pub(super) fn choose_initial_order(can_pos: Option<bool>, can_size: Option<bool>) -> bool {
    match (can_pos, can_size) {
        (Some(false), Some(true)) => false,
        (Some(true), Some(false)) => true,
        (Some(true), Some(true)) => true,
        _ => true,
    }
}

#[inline]
pub(super) fn log_summary(order: &str, attempt: u32, eps: f64, d: (f64, f64, f64, f64)) {
    debug!(
        "summary: order={} attempt={} eps={:.1} dx={:.2} dy={:.2} dw={:.2} dh={:.2}",
        order, attempt, eps, d.0, d.1, d.2, d.3
    );
}

#[inline]
pub(super) fn now_ms(start: std::time::Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

#[inline]
pub(super) fn log_failure_context(win: &crate::AXElem, role: &str, subrole: &str) {
    let (can_pos, can_size) = crate::ax::ax_settable_pos_size(win.as_ptr());
    let s_pos = match can_pos {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    };
    let s_size = match can_size {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    };
    debug!(
        "failure: role='{}' subrole='{}' settable_pos={} settable_size={}",
        role, subrole, s_pos, s_size
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_both_settable_defaults_pos_first() {
        assert!(choose_initial_order(Some(true), Some(true)));
    }

    #[test]
    fn order_size_only_prefers_size_first() {
        assert!(!choose_initial_order(Some(false), Some(true)));
    }

    #[test]
    fn order_pos_only_prefers_pos_first() {
        assert!(choose_initial_order(Some(true), Some(false)));
    }

    #[test]
    fn order_unknown_defaults_pos_first() {
        assert!(choose_initial_order(None, None));
        assert!(choose_initial_order(Some(true), None));
        assert!(choose_initial_order(None, Some(true)));
    }

    #[test]
    fn clamp_flags_detects_each_edge_and_none() {
        let vf = Rect {
            x: 100.0,
            y: 200.0,
            w: 800.0,
            h: 600.0,
        };
        let eps = 2.0;

        // Left clamp only
        let got_left = Rect {
            x: 100.0,
            y: 250.0,
            w: 400.0,
            h: 300.0,
        };
        let f = clamp_flags(&got_left, &vf, eps);
        assert!(
            f.left && !f.right && !f.top && !f.bottom,
            "left only: {}",
            f
        );

        // Right clamp only (x + w == vf.right)
        let got_right = Rect {
            x: 600.0,
            y: 250.0,
            w: 300.0,
            h: 300.0,
        };
        let f = clamp_flags(&got_right, &vf, eps);
        assert!(
            !f.left && f.right && !f.top && !f.bottom,
            "right only: {}",
            f
        );

        // Bottom clamp only (y == vf.bottom)
        let got_bottom = Rect {
            x: 400.0,
            y: 200.0,
            w: 200.0,
            h: 300.0,
        };
        let f = clamp_flags(&got_bottom, &vf, eps);
        assert!(
            !f.left && !f.right && !f.top && f.bottom,
            "bottom only: {}",
            f
        );

        // Top clamp only (y + h == vf.top)
        let got_top = Rect {
            x: 400.0,
            y: 700.0,
            w: 200.0,
            h: 100.0,
        };
        let f = clamp_flags(&got_top, &vf, eps);
        assert!(!f.left && !f.right && f.top && !f.bottom, "top only: {}", f);

        // All edges clamped (exact match)
        let got_all = Rect {
            x: 100.0,
            y: 200.0,
            w: 800.0,
            h: 600.0,
        };
        let f = clamp_flags(&got_all, &vf, eps);
        assert!(f.left && f.right && f.top && f.bottom);
        assert_eq!(f.to_string(), "left,right,bottom,top");

        // None clamped
        let got_none = Rect {
            x: 150.0,
            y: 250.0,
            w: 500.0,
            h: 400.0,
        };
        let f = clamp_flags(&got_none, &vf, eps);
        assert!(!f.any());
        assert_eq!(f.to_string(), "none");
    }
}
