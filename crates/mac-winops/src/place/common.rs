use std::fmt;

use tracing::debug;

// Shared placement utilities: constants, small helpers, and attempt options.
use super::metrics::PLACEMENT_COUNTERS;
pub(super) use super::metrics::{AttemptKind, AttemptOrder};
use crate::geom::{self, Rect};

/// Epsilon tolerance (in points) used to verify post‑placement position and size.
pub(super) const VERIFY_EPS: f64 = 2.0;

pub(super) const POLL_SLEEP_MS: u64 = 25;
pub(super) const POLL_TOTAL_MS: u64 = 400;

// Use the unified geometry axis everywhere in placement modules.
pub(super) use crate::geom::Axis;

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

/// Shared placement inputs derived during normalization.
#[derive(Clone)]
pub struct PlacementContext {
    win: crate::AXElem,
    target: Rect,
    visible_frame: Rect,
    attempt_options: PlaceAttemptOptions,
}

impl fmt::Debug for PlacementContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlacementContext")
            .field("win_ptr", &(self.win.as_ptr() as *const ()))
            .field("target", &self.target)
            .field("visible_frame", &self.visible_frame)
            .field("attempt_options", &self.attempt_options)
            .finish()
    }
}

impl PlacementContext {
    /// Build a placement context from the normalized window state.
    #[inline]
    pub fn new(
        win: crate::AXElem,
        target: Rect,
        visible_frame: Rect,
        attempt_options: PlaceAttemptOptions,
    ) -> Self {
        Self {
            win,
            target,
            visible_frame,
            attempt_options,
        }
    }

    /// Access the retained Accessibility element for subsequent operations.
    #[inline]
    pub fn win(&self) -> &crate::AXElem {
        &self.win
    }

    /// Retrieve the resolved visible frame used during placement.
    #[inline]
    pub fn visible_frame(&self) -> &Rect {
        &self.visible_frame
    }

    /// Retrieve the global target rect produced by grid resolution.
    #[inline]
    pub fn target(&self) -> &Rect {
        &self.target
    }

    /// Return the placement attempt options configured by the caller.
    #[inline]
    pub fn attempt_options(&self) -> PlaceAttemptOptions {
        self.attempt_options
    }
}

#[inline]
pub(super) fn sleep_ms(ms: u64) {
    use std::{thread::sleep, time::Duration};
    sleep(Duration::from_millis(ms));
}

// within_eps moved to Rect::within_eps

#[inline]
pub(super) fn one_axis_off(d: Rect, eps: f64) -> Option<Axis> {
    let x_ok = d.x <= eps && d.w <= eps; // dx,dw within eps
    let y_ok = d.y <= eps && d.h <= eps; // dy,dh within eps
    if x_ok && !y_ok {
        Some(Axis::Vertical)
    } else if y_ok && !x_ok {
        Some(Axis::Horizontal)
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
pub(super) fn log_summary(
    op: &str,
    kind: AttemptKind,
    order: AttemptOrder,
    attempt: u32,
    settle_ms: u64,
    verified: bool,
) {
    debug!(
        target: "mac_winops::place",
        op = %op,
        attempt_idx = attempt,
        kind = ?kind,
        order = ?order,
        settle_ms,
        verified,
        "placement_attempt"
    );
    PLACEMENT_COUNTERS.record_attempt(kind, settle_ms, verified);
}

#[inline]
pub(super) fn trace_safe_park(op: &str) {
    debug!(target: "mac_winops::place", op = %op, "safe_park_preflight");
    PLACEMENT_COUNTERS.record_safe_park();
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
        target: "mac_winops::place",
        role = %role,
        subrole = %subrole,
        settable_pos = %s_pos,
        settable_size = %s_size,
        "placement_failure_context"
    );
    PLACEMENT_COUNTERS.record_failure();
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
