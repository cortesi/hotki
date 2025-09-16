use std::cmp::min;

use objc2_foundation::MainThreadMarker;
use tracing::debug;

use super::{
    common::{PlaceAttemptOptions, PlacementContext, VERIFY_EPS, trace_safe_park},
    engine::{PlacementEngine, PlacementEngineConfig, PlacementGrid, PlacementOutcome},
    fallback::{needs_safe_park, preflight_safe_park},
    normalize::{normalize_before_move, skip_reason_for_role_subrole},
};
use crate::{
    Error, Result, WindowId,
    ax::{ax_check, ax_get_point, ax_get_size, ax_window_for_id, cfstr},
    geom::Rect,
    screen_util::visible_frame_containing_point,
};

/// Compute the visible frame for the screen containing the given window and
/// place the window into the specified grid cell (top-left is (0,0)).
pub(crate) fn place_grid(id: WindowId, cols: u32, rows: u32, col: u32, row: u32) -> Result<()> {
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let (win, pid_for_id) = ax_window_for_id(id)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    (|| -> Result<()> {
        // Stage 1: normalize state (may bail for fullscreen).
        normalize_before_move(&win, pid_for_id, Some(id))?;
        let role = crate::ax::ax_get_string(win.as_ptr(), cfstr("AXRole")).unwrap_or_default();
        let subrole =
            crate::ax::ax_get_string(win.as_ptr(), cfstr("AXSubrole")).unwrap_or_default();
        let title = crate::ax::ax_get_string(win.as_ptr(), cfstr("AXTitle")).unwrap_or_default();
        if let Some(reason) = skip_reason_for_role_subrole(&role, &subrole) {
            debug!(
                "skipped: role/subrole reason={} role='{}' subrole='{}' title='{}'",
                reason, role, subrole, title
            );
            return Ok(());
        }
        let cur_p = ax_get_point(win.as_ptr(), attr_pos)?;
        let cur_s = ax_get_size(win.as_ptr(), attr_size)?;
        let vf = visible_frame_containing_point(mtm, cur_p);
        let col = min(col, cols.saturating_sub(1));
        let row = min(row, rows.saturating_sub(1));
        let Rect { x, y, w, h } = vf.grid_cell(cols, rows, col, row);
        // Stage 5: compute a local rect relative to the chosen visible frame and
        // convert to global coordinates explicitly.
        let target_local = Rect {
            x: x - vf.x,
            y: y - vf.y,
            w,
            h,
        };
        let g = crate::screen_util::globalize_rect(target_local, vf.x, vf.y);
        debug!(
            "coordspace: local={} + origin=({:.1},{:.1}) -> global={}",
            target_local, vf.x, vf.y, g
        );
        let target = Rect::new(g.x, g.y, g.w, g.h);
        let ctx = PlacementContext::new(win.clone(), target, vf, PlaceAttemptOptions::default());
        let vf = *ctx.visible_frame();
        let target = *ctx.target();
        let engine = PlacementEngine::new(
            &ctx,
            PlacementEngineConfig {
                label: "place_grid",
                attr_pos,
                attr_size,
                grid: PlacementGrid {
                    cols,
                    rows,
                    col,
                    row,
                },
                role: &role,
                subrole: &subrole,
            },
        );
        // Stage 3.1: if we would trip coordinate-space issues near global (0,0)
        if needs_safe_park(&target, vf.x, vf.y) {
            trace_safe_park("place_grid");
            preflight_safe_park(
                "place_grid",
                ctx.win(),
                attr_pos,
                attr_size,
                vf.x,
                vf.y,
                &target,
            )?;
        }
        let cur = Rect::from((cur_p, cur_s));
        debug!(
            "WinOps: place_grid: id={} pid={} role='{}' subrole='{}' title='{}' cols={} rows={} col={} row={} | cur={} vf={} target={}",
            id, pid_for_id, role, subrole, title, cols, rows, col, row, cur, vf, target
        );
        match engine.execute(mtm)? {
            PlacementOutcome::Verified(success) => {
                if let Some(anchored) = success.anchored_target {
                    debug!(
                        "WinOps: place_grid verified (anchored legal) | id={} anchored={} got={}",
                        id, anchored, success.final_rect
                    );
                } else {
                    debug!(
                        "WinOps: place_grid verified | id={} target={} got={}",
                        id, target, success.final_rect
                    );
                }
                Ok(())
            }
            PlacementOutcome::PosFirstOnlyFailure(failure)
            | PlacementOutcome::VerificationFailure(failure) => {
                Err(Error::PlacementVerificationFailed {
                    op: "place_grid",
                    expected: target,
                    got: failure.got,
                    epsilon: VERIFY_EPS,
                    clamped: failure.clamped,
                })
            }
        }
    })()
}
