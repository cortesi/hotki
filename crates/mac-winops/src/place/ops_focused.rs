use std::cmp::min;

use objc2_foundation::MainThreadMarker;
use tracing::debug;

use super::{
    apply::AxAttrRefs,
    common::{PlaceAttemptOptions, PlacementContext, trace_safe_park},
    engine::{PlacementEngine, PlacementEngineConfig, PlacementGrid, PlacementOutcome},
    fallback::preflight_safe_park,
    normalize::{normalize_before_move, skip_reason_for_role_subrole},
};
use crate::{
    Error, Result,
    ax::{ax_check, ax_get_point, ax_get_size, cfstr},
    error::PlacementErrorDetails,
    geom::Rect,
    screen_util::visible_frame_containing_point,
};

/// Place the currently focused window of `pid` into the specified grid cell on its current screen.
/// This resolves the window via Accessibility focus and performs the move immediately.
fn place_grid_focused_inner(
    pid: i32,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
    opts: PlaceAttemptOptions,
) -> Result<()> {
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let win = crate::focused_window_for_pid(pid)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    (|| -> Result<()> {
        // Stage 1: normalize state for focused window (may bail for fullscreen).
        normalize_before_move(&win, pid, None)?;
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
        let ctx = PlacementContext::new(win.clone(), target, vf, opts);
        let vf = *ctx.visible_frame();
        let target = *ctx.target();
        let opts = ctx.attempt_options();
        let tuning = opts.tuning();
        if opts.force_second_attempt() {
            debug!("opts: force_second_attempt=true");
        }
        if opts.pos_first_only() {
            debug!("opts: pos_first_only=true");
        }
        let engine = PlacementEngine::new(
            &ctx,
            PlacementEngineConfig {
                label: "place_grid_focused",
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
        if opts.hooks().should_safe_park(&ctx) {
            trace_safe_park("place_grid_focused");
            preflight_safe_park(
                "place_grid_focused",
                ctx.win(),
                AxAttrRefs {
                    pos: attr_pos,
                    size: attr_size,
                },
                &vf,
                &target,
                tuning.epsilon(),
                tuning.settle_timing(),
            )?;
        }
        let cur = Rect::from((cur_p, cur_s));
        debug!(
            "WinOps: place_grid_focused: pid={} role='{}' subrole='{}' title='{}' cols={} rows={} col={} row={} | cur={} vf={} target={}",
            pid, role, subrole, title, cols, rows, col, row, cur, vf, target
        );
        match engine.execute(mtm)? {
            PlacementOutcome::Verified(success) => {
                if let Some(anchored) = success.anchored_target {
                    debug!(
                        "WinOps: place_grid_focused verified (anchored legal) | pid={} anchored={} got={}",
                        pid, anchored, success.final_rect
                    );
                } else {
                    debug!(
                        "WinOps: place_grid_focused verified | pid={} target={} got={}",
                        pid, target, success.final_rect
                    );
                }
                Ok(())
            }
            PlacementOutcome::PosFirstOnlyFailure(failure)
            | PlacementOutcome::VerificationFailure(failure) => {
                Err(Error::PlacementVerificationFailed {
                    op: "place_grid_focused",
                    details: Box::new(PlacementErrorDetails {
                        expected: target,
                        got: failure.got,
                        epsilon: tuning.epsilon(),
                        clamped: failure.clamped,
                        visible_frame: failure.visible_frame,
                        timeline: failure.timeline,
                    }),
                })
            }
        }
    })()
}

pub fn place_grid_focused(pid: i32, cols: u32, rows: u32, col: u32, row: u32) -> Result<()> {
    place_grid_focused_inner(pid, cols, rows, col, row, PlaceAttemptOptions::default())
}

/// As `place_grid_focused` but with explicit attempt options (smoketests).
pub fn place_grid_focused_opts(
    pid: i32,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
    opts: PlaceAttemptOptions,
) -> Result<()> {
    place_grid_focused_inner(pid, cols, rows, col, row, opts)
}
