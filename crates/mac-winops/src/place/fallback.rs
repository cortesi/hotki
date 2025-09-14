use tracing::debug;

use super::{apply::apply_and_wait, common::VERIFY_EPS};
use crate::{
    ax::{ax_get_point, ax_get_size},
    geom::Rect,
};

// Stage 4: shrink→move→grow fallback parameters
const FALLBACK_SAFE_MAX_W: f64 = 400.0;
const FALLBACK_SAFE_MAX_H: f64 = 300.0;

#[inline]
pub(super) fn needs_safe_park(target: &Rect, vf_x: f64, vf_y: f64) -> bool {
    // Trigger only when target is near global origin AND the chosen screen's
    // origin is not at the global origin (i.e., likely a non‑primary screen).
    let near_zero = crate::geom::approx_eq(target.x, 0.0, VERIFY_EPS)
        && crate::geom::approx_eq(target.y, 0.0, VERIFY_EPS);
    let non_primary = !crate::geom::approx_eq(vf_x, 0.0, VERIFY_EPS)
        || !crate::geom::approx_eq(vf_y, 0.0, VERIFY_EPS);
    near_zero && non_primary
}

/// Preflight "safe‑park" to avoid BadCoordinateSpace near global (0,0)
/// on non‑primary displays. Parks the window inside the visible frame with a
/// small safe size, then proceeds with the normal placement.
pub(super) fn preflight_safe_park(
    op_label: &str,
    win: &crate::AXElem,
    attr_pos: core_foundation::string::CFStringRef,
    attr_size: core_foundation::string::CFStringRef,
    vf_x: f64,
    vf_y: f64,
    target: &Rect,
) -> crate::Result<()> {
    // Only attempt when both setters are known to be supported (or unknown).
    let (can_pos, can_size) = crate::ax::ax_settable_pos_size(win.as_ptr());
    if matches!(can_pos, Some(false)) || matches!(can_size, Some(false)) {
        debug!("safe_park: skipped (setters not settable)");
        return Ok(());
    }

    // Pick a conservative in‑frame parking rect near the visible-frame origin.
    let park = Rect {
        x: vf_x + 32.0,
        y: vf_y + 32.0,
        w: target.w.min(FALLBACK_SAFE_MAX_W),
        h: target.h.min(FALLBACK_SAFE_MAX_H),
    };
    debug!(
        "safe_park: {} -> ({:.1},{:.1},{:.1},{:.1})",
        op_label, park.x, park.y, park.w, park.h
    );
    let _ = apply_and_wait(op_label, win, attr_pos, attr_size, &park, true, VERIFY_EPS)?;
    Ok(())
}

/// Fallback sequence to avoid edge clamps when growing while moving.
/// 1) Shrink to a safe size (<= 400x300) at current position.
/// 2) Move to the final position using pos->size ordering (position first).
/// 3) Grow to the final size using size->pos ordering (size first).
pub(super) fn fallback_shrink_move_grow(
    op_label: &str,
    win: &crate::AXElem,
    attr_pos: core_foundation::string::CFStringRef,
    attr_size: core_foundation::string::CFStringRef,
    target: &Rect,
) -> crate::Result<Rect> {
    // Determine safe size bounded by constants and no larger than target.
    let safe_w = target.w.min(FALLBACK_SAFE_MAX_W);
    let safe_h = target.h.min(FALLBACK_SAFE_MAX_H);

    // Read current position for initial shrink step.
    let cur_p = ax_get_point(win.as_ptr(), attr_pos)?;
    let _cur_s = ax_get_size(win.as_ptr(), attr_size)?;

    // Step 1: shrink at current position (size then pos ordering).
    let shrink_rect = Rect {
        x: cur_p.x,
        y: cur_p.y,
        w: safe_w,
        h: safe_h,
    };
    let (_shrink_got, _ms) = apply_and_wait(
        op_label,
        win,
        attr_pos,
        attr_size,
        &shrink_rect,
        false,
        VERIFY_EPS,
    )?;

    // Step 2: move to final position with pos->size using safe size.
    let move_rect = Rect {
        x: target.x,
        y: target.y,
        w: safe_w,
        h: safe_h,
    };
    let (_move_got, _ms2) = apply_and_wait(
        op_label, win, attr_pos, attr_size, &move_rect, true, VERIFY_EPS,
    )?;

    // Step 3: grow to the final size at the final position using size->pos.
    let grow_rect = Rect {
        x: target.x,
        y: target.y,
        w: target.w,
        h: target.h,
    };
    let (got, _ms3) = apply_and_wait(
        op_label, win, attr_pos, attr_size, &grow_rect, false, VERIFY_EPS,
    )?;

    Ok(got)
}
// Safe‑park and shrink→move→grow fallback strategies.
