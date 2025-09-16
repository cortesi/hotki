use objc2_foundation::MainThreadMarker;
use tracing::{debug, warn};

use super::{
    apply::apply_and_wait,
    common::{
        AttemptKind, AttemptOrder, VERIFY_EPS, clamp_flags, log_failure_context, log_summary,
        trace_safe_park,
    },
    fallback::{fallback_shrink_move_grow, needs_safe_park, preflight_safe_park},
    normalize::{normalize_before_move, skip_reason_for_role_subrole},
};
use crate::{
    Error, Result, WindowId,
    ax::{ax_check, ax_get_point, ax_get_size, ax_window_for_id, cfstr},
    geom::{self, Rect},
    screen_util::visible_frame_containing_point,
};

/// Move a window (by `id`) within a grid in the given direction.
pub(crate) fn place_move_grid(
    id: WindowId,
    cols: u32,
    rows: u32,
    dir: crate::MoveDir,
) -> Result<()> {
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

        let eps = VERIFY_EPS;
        let cur_cell = vf.grid_find_cell(cols, rows, cur_p, cur_s, eps);

        let (next_col, next_row) = match cur_cell {
            None => {
                // Fallback: infer current cell by position only, ignoring size mismatches
                let (mut nc, mut nr) =
                    super::grid_guess_cell_by_pos(vf.x, vf.y, vf.w, vf.h, cols, rows, cur_p);
                match dir {
                    crate::MoveDir::Left => nc = nc.saturating_sub(1),
                    crate::MoveDir::Right => {
                        if nc + 1 < cols {
                            nc += 1;
                        }
                    }
                    crate::MoveDir::Up => nr = nr.saturating_sub(1),
                    crate::MoveDir::Down => {
                        if nr + 1 < rows {
                            nr += 1;
                        }
                    }
                }
                (nc, nr)
            }
            Some((c, r)) => {
                let (mut nc, mut nr) = (c, r);
                match dir {
                    crate::MoveDir::Left => nc = nc.saturating_sub(1),
                    crate::MoveDir::Right => {
                        if nc + 1 < cols {
                            nc += 1;
                        }
                    }
                    crate::MoveDir::Up => nr = nr.saturating_sub(1),
                    crate::MoveDir::Down => {
                        if nr + 1 < rows {
                            nr += 1;
                        }
                    }
                }
                (nc, nr)
            }
        };

        let Rect { x, y, w, h } = vf.grid_cell(cols, rows, next_col, next_row);
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
        if needs_safe_park(&target, vf.x, vf.y) {
            trace_safe_park("place_move_grid");
            preflight_safe_park(
                "place_move_grid",
                &win,
                attr_pos,
                attr_size,
                vf.x,
                vf.y,
                &target,
            )?;
        }
        let cur = Rect::from((cur_p, cur_s));
        debug!(
            "WinOps: place_move_grid: id={} pid={} role='{}' subrole='{}' title='{}' cols={} rows={} dir={:?} | cur={} vf={} cur_cell={:?} next_cell=({}, {}) target={}",
            id,
            pid_for_id,
            role,
            subrole,
            title,
            cols,
            rows,
            dir,
            cur,
            vf,
            cur_cell,
            next_col,
            next_row,
            target
        );

        // Stage 2: choose initial order from cached settable bits; if that
        // does not converge within eps, retry with the opposite order (Stage 3).
        let force_second = false;
        let pos_first_only = false;
        let (can_pos, can_size) = crate::ax::ax_settable_pos_size(win.as_ptr());
        let initial_pos_first = super::common::choose_initial_order(can_pos, can_size);
        debug!(
            "order_hint: settable_pos={:?} settable_size={:?} -> initial={}",
            can_pos,
            can_size,
            if initial_pos_first {
                "pos->size"
            } else {
                "size->pos"
            }
        );
        let (got1, settle_ms1) = apply_and_wait(
            "place_move_grid",
            &win,
            attr_pos,
            attr_size,
            &target,
            initial_pos_first,
            VERIFY_EPS,
        )?;
        let _d1 = got1.diffs(&target);
        // Stage 7.2: validate against the final screen selected by window center
        let vf2 = visible_frame_containing_point(
            mtm,
            geom::Point {
                x: got1.cx(),
                y: got1.cy(),
            },
        );
        debug!("vf_used:center={} -> vf={}", got1.center(), vf2);
        debug!("clamp={}", clamp_flags(&got1, &vf2, VERIFY_EPS));
        let first_verified = got1.approx_eq(&target, VERIFY_EPS);
        log_summary(
            "place_move_grid",
            AttemptKind::Primary,
            if initial_pos_first {
                AttemptOrder::PosThenSize
            } else {
                AttemptOrder::SizeThenPos
            },
            1,
            settle_ms1,
            first_verified,
        );
        if first_verified && !force_second {
            debug!("verified=true");
            debug!(
                "WinOps: place_move_grid verified | id={} target={} got={}",
                id, target, got1
            );
            Ok(())
        } else {
            if pos_first_only {
                debug!("verified=false");
                log_failure_context(&win, &role, &subrole);
                let clamped = clamp_flags(&got1, &vf2, VERIFY_EPS);
                warn!(
                    "PlaceMoveGrid failed (pos-first-only): id={} expected={} got={} clamp={}",
                    id, target, got1, clamped
                );
                let _ = crate::raise::raise_window(pid_for_id, id);
                return Err(Error::PlacementVerificationFailed {
                    op: "place_move_grid",
                    expected: target,
                    got: got1,
                    epsilon: VERIFY_EPS,
                    clamped,
                });
            }
            // Stage 3: retry with the opposite order
            let (got2, settle_ms2) = apply_and_wait(
                "place_move_grid",
                &win,
                attr_pos,
                attr_size,
                &target,
                !initial_pos_first,
                VERIFY_EPS,
            )?;
            let d2 = got2.diffs(&target);
            // keep local pos_latched logic later only uses got2; no diff needed here
            let vf4 = visible_frame_containing_point(
                mtm,
                geom::Point {
                    x: got2.cx(),
                    y: got2.cy(),
                },
            );
            debug!("vf_used:center={} -> vf={}", got2.center(), vf4);
            debug!("clamp={}", clamp_flags(&got2, &vf4, VERIFY_EPS));
            let retry_order = if initial_pos_first {
                AttemptOrder::SizeThenPos
            } else {
                AttemptOrder::PosThenSize
            };
            let retry_verified = got2.approx_eq(&target, VERIFY_EPS);
            log_summary(
                "place_move_grid",
                AttemptKind::RetryOpposite,
                retry_order,
                2,
                settle_ms2,
                retry_verified,
            );
            let force_smg = false;
            if force_smg {
                debug!("fallback_used=true");
                let (got3, settle_smg) = fallback_shrink_move_grow(
                    "place_move_grid",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                )?;
                let smg_verified = got3.approx_eq(&target, VERIFY_EPS);
                log_summary(
                    "place_move_grid",
                    AttemptKind::FallbackShrinkMoveGrow,
                    AttemptOrder::Fallback,
                    3,
                    settle_smg,
                    smg_verified,
                );
                if smg_verified {
                    debug!("verified=true");
                    debug!("order_used=shrink->move->grow, attempts=3");
                    debug!(
                        "WinOps: place_move_grid verified | id={} target={} got={}",
                        id, target, got3
                    );
                    Ok(())
                } else {
                    debug!("verified=false");
                    log_failure_context(&win, &role, &subrole);
                    let vf = visible_frame_containing_point(
                        mtm,
                        geom::Point {
                            x: got3.cx(),
                            y: got3.cy(),
                        },
                    );
                    debug!("vf_used:center={} -> vf={}", got3.center(), vf);
                    let clamped = clamp_flags(&got3, &vf, VERIFY_EPS);
                    Err(Error::PlacementVerificationFailed {
                        op: "place_move_grid",
                        expected: target,
                        got: got3,
                        epsilon: VERIFY_EPS,
                        clamped,
                    })
                }
            } else if retry_verified {
                debug!("verified=true");
                debug!("order_used=size->pos, attempts=2");
                debug!(
                    "WinOps: place_move_grid verified | id={} target={} got={}",
                    id, target, got2
                );
                Ok(())
            } else {
                // Stage: if only position is latched, try size-only then anchor legal size
                let pos_latched = d2.x <= VERIFY_EPS && d2.y <= VERIFY_EPS;
                if pos_latched {
                    debug!("pos_latched=true (x,y within eps)");
                    // If size is known not settable, skip size-only and anchor immediately.
                    if can_size == Some(false) {
                        debug!(
                            "size_not_settable=true; skipping size-only and anchoring legal size"
                        );
                        let (got_anchor, anchored, settle_ms_anchor_skip) =
                            super::apply::anchor_legal_size_and_wait(
                                "place_move_grid",
                                &win,
                                attr_pos,
                                attr_size,
                                &target,
                                &got2,
                                cols,
                                rows,
                                next_col,
                                next_row,
                                VERIFY_EPS,
                            )?;
                        let anchor_verified = got_anchor.approx_eq(&anchored, VERIFY_EPS);
                        log_summary(
                            "place_move_grid",
                            AttemptKind::AnchorLegal,
                            AttemptOrder::Anchor,
                            2,
                            settle_ms_anchor_skip,
                            anchor_verified,
                        );
                        if anchor_verified {
                            debug!("verified=true");
                            debug!(
                                "WinOps: place_move_grid verified (anchored legal) | id={} anchored={} got={}",
                                id, anchored, got_anchor
                            );
                            return Ok(());
                        }
                    } else {
                        debug!("switching to size-only adjustments");
                        match super::apply::apply_size_only_and_wait(
                            "place_move_grid:size-only",
                            &win,
                            attr_size,
                            (target.w, target.h),
                            VERIFY_EPS,
                        ) {
                            Ok((got_sz, settle_ms_sz)) => {
                                let size_only_verified = got_sz.approx_eq(&target, VERIFY_EPS);
                                log_summary(
                                    "place_move_grid",
                                    AttemptKind::SizeOnly,
                                    AttemptOrder::SizeOnly,
                                    2,
                                    settle_ms_sz,
                                    size_only_verified,
                                );
                                let (got_anchor, anchored, settle_ms_anchor_sz) =
                                    super::apply::anchor_legal_size_and_wait(
                                        "place_move_grid",
                                        &win,
                                        attr_pos,
                                        attr_size,
                                        &target,
                                        &got_sz,
                                        cols,
                                        rows,
                                        next_col,
                                        next_row,
                                        VERIFY_EPS,
                                    )?;
                                let anchor_verified = got_anchor.approx_eq(&anchored, VERIFY_EPS);
                                log_summary(
                                    "place_move_grid",
                                    AttemptKind::AnchorSizeOnly,
                                    AttemptOrder::Anchor,
                                    2,
                                    settle_ms_anchor_sz,
                                    anchor_verified,
                                );
                                if anchor_verified {
                                    debug!("verified=true");
                                    debug!(
                                        "WinOps: place_move_grid verified (anchored legal) | id={} anchored={} got={}",
                                        id, anchored, got_anchor
                                    );
                                    return Ok(());
                                }
                            }
                            Err(e) => {
                                debug!(
                                    "size-only failed ({}); anchoring legal size using observed got2",
                                    e
                                );
                                let (got_anchor, anchored, settle_ms_anchor_fallback) =
                                    super::apply::anchor_legal_size_and_wait(
                                        "place_move_grid",
                                        &win,
                                        attr_pos,
                                        attr_size,
                                        &target,
                                        &got2,
                                        cols,
                                        rows,
                                        next_col,
                                        next_row,
                                        VERIFY_EPS,
                                    )?;
                                let anchor_verified = got_anchor.approx_eq(&anchored, VERIFY_EPS);
                                log_summary(
                                    "place_move_grid",
                                    AttemptKind::AnchorLegal,
                                    AttemptOrder::Anchor,
                                    2,
                                    settle_ms_anchor_fallback,
                                    anchor_verified,
                                );
                                if anchor_verified {
                                    debug!("verified=true");
                                    debug!(
                                        "WinOps: place_move_grid verified (anchored legal) | id={} anchored={} got={}",
                                        id, anchored, got_anchor
                                    );
                                    return Ok(());
                                }
                            }
                        }
                    }
                }

                // Stage: anchor legal size using observed rect if not latched
                let (got_anchor, anchored, settle_ms_anchor) =
                    super::apply::anchor_legal_size_and_wait(
                        "place_move_grid",
                        &win,
                        attr_pos,
                        attr_size,
                        &target,
                        &got2,
                        cols,
                        rows,
                        next_col,
                        next_row,
                        VERIFY_EPS,
                    )?;
                let vf5 = visible_frame_containing_point(
                    mtm,
                    geom::Point {
                        x: got_anchor.cx(),
                        y: got_anchor.cy(),
                    },
                );
                debug!("clamp={}", clamp_flags(&got_anchor, &vf5, VERIFY_EPS));
                let anchor_verified = got_anchor.approx_eq(&anchored, VERIFY_EPS);
                log_summary(
                    "place_move_grid",
                    AttemptKind::AnchorLegal,
                    AttemptOrder::Anchor,
                    2,
                    settle_ms_anchor,
                    anchor_verified,
                );
                if anchor_verified {
                    debug!("verified=true");
                    debug!("order_used=anchor-legal, attempts=2");
                    debug!(
                        "WinOps: place_move_grid verified | id={} anchored={} got={}",
                        id, anchored, got_anchor
                    );
                    return Ok(());
                }

                // Stage 4: shrink→move→grow fallback
                debug!("fallback_used=true");
                let (got3, settle_smg) = fallback_shrink_move_grow(
                    "place_move_grid",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                )?;
                let smg_verified = got3.approx_eq(&target, VERIFY_EPS);
                log_summary(
                    "place_move_grid",
                    AttemptKind::FallbackShrinkMoveGrow,
                    AttemptOrder::Fallback,
                    3,
                    settle_smg,
                    smg_verified,
                );
                if smg_verified {
                    debug!("verified=true");
                    debug!("order_used=shrink->move->grow, attempts=3");
                    debug!(
                        "WinOps: place_move_grid verified | id={} target={} got={}",
                        id, target, got3
                    );
                    Ok(())
                } else {
                    debug!("verified=false");
                    log_failure_context(&win, &role, &subrole);
                    let vf = visible_frame_containing_point(
                        mtm,
                        geom::Point {
                            x: got3.cx(),
                            y: got3.cy(),
                        },
                    );
                    debug!("vf_used:center={} -> vf={}", got3.center(), vf);
                    let clamped = clamp_flags(&got3, &vf, VERIFY_EPS);
                    warn!(
                        "PlaceMoveGrid failed: id={} expected={} got={} clamp={}",
                        id, target, got3, clamped
                    );
                    let _ = crate::raise::raise_window(pid_for_id, id);
                    Err(Error::PlacementVerificationFailed {
                        op: "place_move_grid",
                        expected: target,
                        got: got3,
                        epsilon: VERIFY_EPS,
                        clamped,
                    })
                }
            }
        }
    })()
}
