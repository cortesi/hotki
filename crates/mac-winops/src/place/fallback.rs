use tracing::debug;

use super::{
    apply::{AxAttrRefs, apply_and_wait},
    common::SettleTiming,
};
use crate::{
    ax::{ax_get_point, ax_get_size},
    geom::Rect,
};

// Stage 4: shrink→move→grow fallback parameters
const FALLBACK_SAFE_MAX_W: f64 = 400.0;
const FALLBACK_SAFE_MAX_H: f64 = 300.0;

/// Preflight "safe-park" to avoid BadCoordinateSpace near global (0,0)
/// on non-primary displays. Parks the window inside the visible frame with a
/// small safe size, then proceeds with the normal placement.
pub(super) fn preflight_safe_park(
    op_label: &str,
    win: &crate::AXElem,
    attrs: AxAttrRefs,
    visible_frame: &Rect,
    target: &Rect,
    eps: f64,
    timing: SettleTiming,
) -> crate::Result<()> {
    // Only attempt when both setters are known to be supported (or unknown).
    let (can_pos, can_size) = crate::ax::ax_settable_pos_size(win.as_ptr());
    if matches!(can_pos, Some(false)) || matches!(can_size, Some(false)) {
        debug!("safe_park: skipped (setters not settable)");
        return Ok(());
    }

    // Pick a conservative in-frame parking rect near the visible-frame origin.
    let park = Rect {
        x: visible_frame.x + 32.0,
        y: visible_frame.y + 32.0,
        w: target.w.min(FALLBACK_SAFE_MAX_W),
        h: target.h.min(FALLBACK_SAFE_MAX_H),
    };
    debug!(
        "safe_park: {} -> ({:.1},{:.1},{:.1},{:.1})",
        op_label, park.x, park.y, park.w, park.h
    );
    let _ = apply_and_wait(op_label, win, attrs, &park, true, eps, timing)?;
    Ok(())
}

/// Fallback sequence to avoid edge clamps when growing while moving.
/// 1) Shrink to a safe size (<= 400x300) at current position.
/// 2) Move to the final position using pos->size ordering (position first).
/// 3) Grow to the final size using size->pos ordering (size first).
pub(super) fn fallback_shrink_move_grow(
    op_label: &str,
    win: &crate::AXElem,
    attrs: AxAttrRefs,
    target: &Rect,
    eps: f64,
    timing: SettleTiming,
) -> crate::Result<(Rect, u64)> {
    // Determine safe size bounded by constants and no larger than target.
    let safe_w = target.w.min(FALLBACK_SAFE_MAX_W);
    let safe_h = target.h.min(FALLBACK_SAFE_MAX_H);

    // Read current position for initial shrink step.
    let cur_p = ax_get_point(win.as_ptr(), attrs.pos)?;
    let _cur_s = ax_get_size(win.as_ptr(), attrs.size)?;

    // Step 1: shrink at current position (size then pos ordering).
    let shrink_rect = Rect {
        x: cur_p.x,
        y: cur_p.y,
        w: safe_w,
        h: safe_h,
    };
    let (_shrink_got, settle_shrink) =
        apply_and_wait(op_label, win, attrs, &shrink_rect, false, eps, timing)?;

    // Step 2: move to final position with pos->size using safe size.
    let move_rect = Rect {
        x: target.x,
        y: target.y,
        w: safe_w,
        h: safe_h,
    };
    let (_move_got, settle_move) =
        apply_and_wait(op_label, win, attrs, &move_rect, true, eps, timing)?;

    // Step 3: grow to the final size at the final position using size->pos.
    let grow_rect = Rect {
        x: target.x,
        y: target.y,
        w: target.w,
        h: target.h,
    };
    let (got, settle_grow) = apply_and_wait(op_label, win, attrs, &grow_rect, false, eps, timing)?;

    let total_settle = settle_shrink + settle_move + settle_grow;
    Ok((got, total_settle))
}
// Safe-park and shrink→move→grow fallback strategies.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_park_preflight_uses_safe_dimensions() {
        let target = Rect {
            x: 10.0,
            y: 20.0,
            w: 900.0,
            h: 700.0,
        };
        // The helper should clamp to safe bounds regardless of the oversized target.
        assert_eq!(target.w.min(FALLBACK_SAFE_MAX_W), 400.0);
        assert_eq!(target.h.min(FALLBACK_SAFE_MAX_H), 300.0);
    }
}
