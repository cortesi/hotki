use std::cmp::min;

use objc2_foundation::MainThreadMarker;
use tracing::debug;

use super::{
    apply::{
        anchor_legal_size_and_wait, apply_and_wait, apply_size_only_and_wait,
        nudge_axis_pos_and_wait,
    },
    common::{PlaceAttemptOptions, VERIFY_EPS, clamp_flags, log_failure_context, log_summary},
    fallback::{fallback_shrink_move_grow, needs_safe_park, preflight_safe_park},
    normalize::{normalize_before_move, skip_reason_for_role_subrole},
};
use crate::{
    Error, Result,
    ax::{ax_check, ax_get_point, ax_get_size, cfstr},
    geom::{self, Axis, Rect},
    screen_util::visible_frame_containing_point,
};

/// Place the currently focused window of `pid` into the specified grid cell on its current screen.
/// This resolves the window via Accessibility focus and performs the move immediately.
pub fn place_grid_focused(pid: i32, cols: u32, rows: u32, col: u32, row: u32) -> Result<()> {
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
        if needs_safe_park(&target, vf.x, vf.y) {
            preflight_safe_park(
                "place_grid_focused",
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
            "WinOps: place_grid_focused: pid={} role='{}' subrole='{}' title='{}' cols={} rows={} col={} row={} | cur={} vf={} target={}",
            pid, role, subrole, title, cols, rows, col, row, cur, vf, target
        );
        // Stage 2: choose initial order from cached settable bits; if that
        // does not converge within eps, retry with the opposite order (Stage 3).
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
        let (got1, _settle_ms1) = apply_and_wait(
            "place_grid_focused",
            &win,
            attr_pos,
            attr_size,
            &target,
            initial_pos_first,
            VERIFY_EPS,
        )?;
        let d1 = got1.diffs(&target);
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
        log_summary(
            if initial_pos_first {
                "pos->size"
            } else {
                "size->pos"
            },
            1,
            VERIFY_EPS,
        );
        if got1.approx_eq(&target, VERIFY_EPS) {
            debug!("verified=true");
            debug!(
                "WinOps: place_grid_focused verified | pid={} target={} got={}",
                pid, target, got1
            );
            Ok(())
        } else {
            // Stage 7.1: If only one axis is off, try a single-axis nudge first.
            let mut attempt_idx = 2u32;
            if let Some(axis) = super::common::one_axis_off(d1, VERIFY_EPS) {
                let (got_ax, _settle_ms_ax) = nudge_axis_pos_and_wait(
                    "place_grid_focused",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                    axis,
                    VERIFY_EPS,
                )?;
                // no diff logging; rely on target/got only
                let vf3 = visible_frame_containing_point(
                    mtm,
                    geom::Point {
                        x: got_ax.cx(),
                        y: got_ax.cy(),
                    },
                );
                debug!(
                    "vf_used:center=({:.1},{:.1}) -> vf={}",
                    got_ax.cx(),
                    got_ax.cy(),
                    vf3
                );
                debug!("clamp={}", clamp_flags(&got_ax, &vf3, VERIFY_EPS));
                let label = match axis {
                    Axis::Horizontal => "axis-pos:x",
                    Axis::Vertical => "axis-pos:y",
                };
                log_summary(label, attempt_idx, VERIFY_EPS);
                if got_ax.approx_eq(&target, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=axis-pos, attempts=2");
                    debug!(
                        "WinOps: place_grid_focused verified | pid={} target={} got={}",
                        pid, target, got_ax
                    );
                    return Ok(());
                }
                attempt_idx = 3;
            }
            // Stage 3: retry with the opposite order
            let (got2, _settle_ms2) = apply_and_wait(
                "place_grid_focused",
                &win,
                attr_pos,
                attr_size,
                &target,
                !initial_pos_first,
                VERIFY_EPS,
            )?;
            let d2 = got2.diffs(&target);
            let vf4 = visible_frame_containing_point(
                mtm,
                geom::Point {
                    x: got2.cx(),
                    y: got2.cy(),
                },
            );
            debug!(
                "vf_used:center=({:.1},{:.1}) -> vf={}",
                got2.cx(),
                got2.cy(),
                vf4
            );
            debug!("clamp={}", clamp_flags(&got2, &vf4, VERIFY_EPS));
            log_summary(
                if initial_pos_first {
                    "size->pos"
                } else {
                    "pos->size"
                },
                attempt_idx,
                VERIFY_EPS,
            );
            let force_smg = false;
            if force_smg {
                debug!("fallback_used=true");
                let got3 = fallback_shrink_move_grow(
                    "place_grid_focused",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                )?;
                if got3.approx_eq(&target, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=shrink->move->grow, attempts=3");
                    debug!(
                        "WinOps: place_grid_focused verified | pid={} target={} got={}",
                        pid, target, got3
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
                    debug!(
                        "vf_used:center=({:.1},{:.1}) -> vf={}",
                        got3.cx(),
                        got3.cy(),
                        vf
                    );
                    let clamped = clamp_flags(&got3, &vf, VERIFY_EPS);
                    Err(Error::PlacementVerificationFailed {
                        op: "place_grid_focused",
                        expected: target,
                        got: got3,
                        epsilon: VERIFY_EPS,
                        clamped,
                    })
                }
            } else if got2.approx_eq(&target, VERIFY_EPS) {
                debug!("verified=true");
                debug!("order_used=size->pos, attempts=2");
                debug!(
                    "WinOps: place_grid_focused verified | pid={} target={} got={}",
                    pid, target, got2
                );
                Ok(())
            } else {
                // Latch if position reached the correct origin; then grow/shrink only.
                let pos_latched = d2.x <= VERIFY_EPS && d2.y <= VERIFY_EPS;
                if pos_latched {
                    debug!("pos_latched=true (x,y within eps); switching to size-only adjustments");
                    let (got_sz, _ms) = apply_size_only_and_wait(
                        "place_grid_focused:size-only",
                        &win,
                        attr_size,
                        (target.w, target.h),
                        VERIFY_EPS,
                    )?;
                    // Accept anchored legal size: compare against an anchored target using observed size
                    let (got_anchor, anchored, _ms2) = anchor_legal_size_and_wait(
                        "place_grid_focused",
                        &win,
                        attr_pos,
                        attr_size,
                        &target,
                        &got_sz,
                        cols,
                        rows,
                        col,
                        row,
                        VERIFY_EPS,
                    )?;
                    log_summary(
                        "anchor-legal:size-only",
                        attempt_idx.saturating_add(1),
                        VERIFY_EPS,
                    );
                    if got_anchor.approx_eq(&anchored, VERIFY_EPS) {
                        debug!("verified=true");
                        debug!(
                            "WinOps: place_grid_focused verified (anchored legal) | pid={} anchored={} got={}",
                            pid, anchored, got_anchor
                        );
                        return Ok(());
                    }
                }
                // Stage: anchor the legal size (pos->size) as a fallback if not latched
                let (got_anchor, anchored, _settle_ms_anchor) = anchor_legal_size_and_wait(
                    "place_grid_focused",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                    &got2,
                    cols,
                    rows,
                    col,
                    row,
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
                log_summary("anchor-legal", attempt_idx.saturating_add(1), VERIFY_EPS);
                if got_anchor.approx_eq(&anchored, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=anchor-legal, attempts={}", attempt_idx + 1);
                    debug!(
                        "WinOps: place_grid_focused verified | pid={} anchored={} got={}",
                        pid, anchored, got_anchor
                    );
                    return Ok(());
                }
                // Stage 4: shrink→move→grow fallback
                debug!("fallback_used=true");
                let got3 = fallback_shrink_move_grow(
                    "place_grid_focused",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                )?;
                if got3.approx_eq(&target, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=shrink->move->grow, attempts=3");
                    debug!(
                        "WinOps: place_grid_focused verified | pid={} target={} got={}",
                        pid, target, got3
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
                    debug!(
                        "vf_used:center=({:.1},{:.1}) -> vf={}",
                        got3.cx(),
                        got3.cy(),
                        vf
                    );
                    let clamped = clamp_flags(&got3, &vf, VERIFY_EPS);
                    Err(Error::PlacementVerificationFailed {
                        op: "place_grid_focused",
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

/// As `place_grid_focused` but with explicit attempt options (smoketests).
pub fn place_grid_focused_opts(
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
        let col = core::cmp::min(col, cols.saturating_sub(1));
        let row = core::cmp::min(row, rows.saturating_sub(1));
        let Rect { x, y, w, h } = vf.grid_cell(cols, rows, col, row);
        let target = Rect::new(x, y, w, h);
        if needs_safe_park(&target, vf.x, vf.y) {
            preflight_safe_park(
                "place_grid_focused",
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
            "WinOps: place_grid_focused_opts: pid={} role='{}' subrole='{}' title='{}' cols={} rows={} col={} row={} | cur={} vf={} target={}",
            pid, role, subrole, title, cols, rows, col, row, cur, vf, target
        );
        let force_second = opts.force_second_attempt;
        let pos_first_only = opts.pos_first_only;
        if force_second {
            debug!("opts: force_second_attempt=true");
        }
        if pos_first_only {
            debug!("opts: pos_first_only=true");
        }
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
        let (got1, _settle_ms1) = apply_and_wait(
            "place_grid_focused",
            &win,
            attr_pos,
            attr_size,
            &target,
            initial_pos_first,
            VERIFY_EPS,
        )?;
        let d1 = got1.diffs(&target);
        // Stage 7.2: validate against the final screen selected by window center
        let vf2 = visible_frame_containing_point(
            mtm,
            geom::Point {
                x: got1.cx(),
                y: got1.cy(),
            },
        );
        debug!(
            "vf_used:center=({:.1},{:.1}) -> vf={}",
            got1.cx(),
            got1.cy(),
            vf2
        );
        debug!("clamp={}", clamp_flags(&got1, &vf2, VERIFY_EPS));
        log_summary(
            if initial_pos_first {
                "pos->size"
            } else {
                "size->pos"
            },
            1,
            VERIFY_EPS,
        );
        if got1.approx_eq(&target, VERIFY_EPS) && !force_second {
            debug!("verified=true");
            debug!(
                "WinOps: place_grid_focused verified | pid={} target={} got={}",
                pid, target, got1
            );
            Ok(())
        } else {
            if pos_first_only {
                debug!("verified=false");
                log_failure_context(&win, &role, &subrole);
                let clamped = clamp_flags(&got1, &vf2, VERIFY_EPS);
                return Err(Error::PlacementVerificationFailed {
                    op: "place_grid_focused",
                    expected: target,
                    got: got1,
                    epsilon: VERIFY_EPS,
                    clamped,
                });
            }
            // Stage 7.1: If only one axis is off, try a single-axis nudge first.
            let mut attempt_idx = 2u32;
            if let Some(axis) = super::common::one_axis_off(d1, VERIFY_EPS) {
                let (got_ax, _settle_ms_ax) = nudge_axis_pos_and_wait(
                    "place_grid_focused",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                    axis,
                    VERIFY_EPS,
                )?;
                // no diff logging
                let vf3 = visible_frame_containing_point(
                    mtm,
                    geom::Point {
                        x: got_ax.cx(),
                        y: got_ax.cy(),
                    },
                );
                debug!("vf_used:center={} -> vf={}", got_ax.center(), vf3);
                debug!("clamp={}", clamp_flags(&got_ax, &vf3, VERIFY_EPS));
                let label = match axis {
                    Axis::Horizontal => "axis-pos:x",
                    Axis::Vertical => "axis-pos:y",
                };
                log_summary(label, attempt_idx, VERIFY_EPS);
                if got_ax.approx_eq(&target, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=axis-pos, attempts=2");
                    debug!(
                        "WinOps: place_grid_focused verified | pid={} target={} got={}",
                        pid, target, got_ax
                    );
                    return Ok(());
                }
                attempt_idx = 3;
            }
            // Stage 3: retry with the opposite order
            let (got2, _settle_ms2) = apply_and_wait(
                "place_grid_focused",
                &win,
                attr_pos,
                attr_size,
                &target,
                !initial_pos_first,
                VERIFY_EPS,
            )?;
            let d2 = got2.diffs(&target);
            let vf4 = visible_frame_containing_point(
                mtm,
                geom::Point {
                    x: got2.cx(),
                    y: got2.cy(),
                },
            );
            debug!("vf_used:center={} -> vf={}", got2.center(), vf4);
            debug!("clamp={}", clamp_flags(&got2, &vf4, VERIFY_EPS));
            log_summary(
                if initial_pos_first {
                    "size->pos"
                } else {
                    "pos->size"
                },
                attempt_idx,
                VERIFY_EPS,
            );
            let force_smg = opts.force_shrink_move_grow;
            if force_smg {
                debug!("fallback_used=true");
                let got3 = fallback_shrink_move_grow(
                    "place_grid_focused",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                )?;
                if got3.approx_eq(&target, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=shrink->move->grow, attempts=3");
                    debug!(
                        "WinOps: place_grid_focused verified | pid={} target={} got={}",
                        pid, target, got3
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
                        op: "place_grid_focused",
                        expected: target,
                        got: got3,
                        epsilon: VERIFY_EPS,
                        clamped,
                    })
                }
            } else if got2.approx_eq(&target, VERIFY_EPS) {
                debug!("verified=true");
                debug!("order_used=size->pos, attempts=2");
                debug!(
                    "WinOps: place_grid_focused verified | pid={} target={} got={}",
                    pid, target, got2
                );
                Ok(())
            } else {
                // Latch if position reached the correct origin; then grow/shrink only.
                let pos_latched = d2.x <= VERIFY_EPS && d2.y <= VERIFY_EPS;
                if pos_latched {
                    debug!("pos_latched=true (x,y within eps); switching to size-only adjustments");
                    let (got_sz, _ms) = apply_size_only_and_wait(
                        "place_grid_focused:size-only",
                        &win,
                        attr_size,
                        (target.w, target.h),
                        VERIFY_EPS,
                    )?;
                    // Accept anchored legal size: compare against an anchored target using observed size
                    let (got_anchor, anchored, _ms2) = anchor_legal_size_and_wait(
                        "place_grid_focused",
                        &win,
                        attr_pos,
                        attr_size,
                        &target,
                        &got_sz,
                        cols,
                        rows,
                        col,
                        row,
                        VERIFY_EPS,
                    )?;
                    log_summary(
                        "anchor-legal:size-only",
                        attempt_idx.saturating_add(1),
                        VERIFY_EPS,
                    );
                    if got_anchor.approx_eq(&anchored, VERIFY_EPS) {
                        debug!("verified=true");
                        debug!(
                            "WinOps: place_grid_focused verified (anchored legal) | pid={} anchored={} got={}",
                            pid, anchored, got_anchor
                        );
                        return Ok(());
                    }
                }
                // Stage: anchor the legal size (pos->size) as a fallback if not latched
                let (got_anchor, anchored, _settle_ms_anchor) = anchor_legal_size_and_wait(
                    "place_grid_focused",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                    &got2,
                    cols,
                    rows,
                    col,
                    row,
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
                log_summary("anchor-legal", attempt_idx.saturating_add(1), VERIFY_EPS);
                if got_anchor.approx_eq(&anchored, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=anchor-legal, attempts={}", attempt_idx + 1);
                    debug!(
                        "WinOps: place_grid_focused verified | pid={} anchored={} got={}",
                        pid, anchored, got_anchor
                    );
                    return Ok(());
                }
                // Stage 4: shrink→move→grow fallback
                debug!("fallback_used=true");
                let got3 = fallback_shrink_move_grow(
                    "place_grid_focused",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                )?;
                if got3.approx_eq(&target, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=shrink->move->grow, attempts=3");
                    debug!(
                        "WinOps: place_grid_focused verified | pid={} target={} got={}",
                        pid, target, got3
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
                    debug!(
                        "vf_used:center=({:.1},{:.1}) -> vf={}",
                        got3.cx(),
                        got3.cy(),
                        vf
                    );
                    let clamped = clamp_flags(&got3, &vf, VERIFY_EPS);
                    Err(Error::PlacementVerificationFailed {
                        op: "place_grid_focused",
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
