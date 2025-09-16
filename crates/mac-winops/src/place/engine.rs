use objc2_foundation::MainThreadMarker;
use tracing::debug;

use super::{
    apply::{
        anchor_legal_size_and_wait, apply_and_wait, apply_size_only_and_wait,
        nudge_axis_pos_and_wait,
    },
    common::{
        AttemptKind, AttemptOrder, PlacementContext, VERIFY_EPS, choose_initial_order, clamp_flags,
        log_failure_context, log_summary, one_axis_off,
    },
    fallback::fallback_shrink_move_grow,
};
use crate::{
    Result,
    geom::{self, Rect},
    screen_util::visible_frame_containing_point,
};

use core_foundation::string::CFStringRef;

/// Successful placement outcome details.
#[derive(Debug, Clone, Copy)]
pub struct PlacementSuccess {
    /// Final observed rectangle returned by the Accessibility API.
    pub final_rect: Rect,
    /// Anchored rectangle when verification succeeded using anchoring.
    pub anchored_target: Option<Rect>,
}

/// Context captured when verification fails.
#[derive(Debug, Clone, Copy)]
pub struct PlacementFailureContext {
    /// Last observed rectangle reported by Accessibility.
    pub got: Rect,
    /// Clamp flags comparing the observed rect against the visible frame.
    pub clamped: crate::error::ClampFlags,
}

/// Result of executing the placement engine.
#[derive(Debug, Clone, Copy)]
pub enum PlacementOutcome {
    /// Placement verified successfully.
    Verified(PlacementSuccess),
    /// Placement aborted because only the initial attempt was permitted.
    PosFirstOnlyFailure(PlacementFailureContext),
    /// Placement exhausted all fallbacks without verification.
    VerificationFailure(PlacementFailureContext),
}

/// Grid placement parameters consumed by the engine.
#[derive(Debug, Clone, Copy)]
pub struct PlacementGrid {
    pub cols: u32,
    pub rows: u32,
    pub col: u32,
    pub row: u32,
}

/// Static configuration describing the current placement run.
#[derive(Debug, Clone, Copy)]
pub struct PlacementEngineConfig<'a> {
    pub label: &'a str,
    pub attr_pos: CFStringRef,
    pub attr_size: CFStringRef,
    pub grid: PlacementGrid,
    pub role: &'a str,
    pub subrole: &'a str,
}

/// Shared placement pipeline used by focused, id-based, and directional ops.
pub struct PlacementEngine<'a> {
    ctx: &'a PlacementContext,
    config: PlacementEngineConfig<'a>,
}

impl<'a> PlacementEngine<'a> {
    /// Construct a new placement engine for the supplied context.
    pub fn new(ctx: &'a PlacementContext, config: PlacementEngineConfig<'a>) -> Self {
        Self { ctx, config }
    }

    /// Execute the multi-attempt placement pipeline.
    pub fn execute(&self, mtm: MainThreadMarker) -> Result<PlacementOutcome> {
        let win = self.ctx.win();
        let target = *self.ctx.target();
        let opts = self.ctx.attempt_options();
        let label = self.config.label;
        let attr_pos = self.config.attr_pos;
        let attr_size = self.config.attr_size;
        let grid = self.config.grid;

        let (can_pos, can_size) = crate::ax::ax_settable_pos_size(win.as_ptr());
        let initial_pos_first = choose_initial_order(can_pos, can_size);
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
            label,
            win,
            attr_pos,
            attr_size,
            &target,
            initial_pos_first,
            VERIFY_EPS,
        )?;
        let d1 = got1.diffs(&target);
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
            label,
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
        if first_verified && !opts.force_second_attempt {
            debug!("verified=true");
            return Ok(PlacementOutcome::Verified(PlacementSuccess {
                final_rect: got1,
                anchored_target: None,
            }));
        }

        if opts.pos_first_only {
            debug!("verified=false");
            log_failure_context(win, self.config.role, self.config.subrole);
            let clamped = clamp_flags(&got1, &vf2, VERIFY_EPS);
            return Ok(PlacementOutcome::PosFirstOnlyFailure(
                PlacementFailureContext { got: got1, clamped },
            ));
        }

        let mut attempt_idx = 2u32;
        if let Some(axis) = one_axis_off(d1, VERIFY_EPS) {
            let (got_ax, settle_ms_ax) = nudge_axis_pos_and_wait(
                label, win, attr_pos, attr_size, &target, axis, VERIFY_EPS,
            )?;
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
            let axis_order = match axis {
                crate::geom::Axis::Horizontal => AttemptOrder::AxisHorizontal,
                crate::geom::Axis::Vertical => AttemptOrder::AxisVertical,
            };
            let axis_verified = got_ax.approx_eq(&target, VERIFY_EPS);
            log_summary(
                label,
                AttemptKind::AxisNudge,
                axis_order,
                attempt_idx,
                settle_ms_ax,
                axis_verified,
            );
            if axis_verified {
                debug!("verified=true");
                debug!("order_used=axis-pos, attempts={}", attempt_idx);
                return Ok(PlacementOutcome::Verified(PlacementSuccess {
                    final_rect: got_ax,
                    anchored_target: None,
                }));
            }
            attempt_idx = attempt_idx.saturating_add(1);
        }

        let (got2, settle_ms2) = apply_and_wait(
            label,
            win,
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
        let retry_verified = got2.approx_eq(&target, VERIFY_EPS);
        log_summary(
            label,
            AttemptKind::RetryOpposite,
            if initial_pos_first {
                AttemptOrder::SizeThenPos
            } else {
                AttemptOrder::PosThenSize
            },
            attempt_idx,
            settle_ms2,
            retry_verified,
        );

        if opts.force_shrink_move_grow {
            debug!("fallback_used=true");
            let (got_fallback, settle_ms_fb) =
                fallback_shrink_move_grow(label, win, attr_pos, attr_size, &target)?;
            let smg_verified = got_fallback.approx_eq(&target, VERIFY_EPS);
            log_summary(
                label,
                AttemptKind::FallbackShrinkMoveGrow,
                AttemptOrder::Fallback,
                attempt_idx.saturating_add(1),
                settle_ms_fb,
                smg_verified,
            );
            if smg_verified {
                debug!("verified=true");
                debug!(
                    "order_used=shrink->move->grow, attempts={}",
                    attempt_idx.saturating_add(1)
                );
                return Ok(PlacementOutcome::Verified(PlacementSuccess {
                    final_rect: got_fallback,
                    anchored_target: None,
                }));
            }
            debug!("verified=false");
            log_failure_context(win, self.config.role, self.config.subrole);
            let vf = visible_frame_containing_point(
                mtm,
                geom::Point {
                    x: got_fallback.cx(),
                    y: got_fallback.cy(),
                },
            );
            debug!(
                "vf_used:center=({:.1},{:.1}) -> vf={}",
                got_fallback.cx(),
                got_fallback.cy(),
                vf
            );
            let clamped = clamp_flags(&got_fallback, &vf, VERIFY_EPS);
            return Ok(PlacementOutcome::VerificationFailure(
                PlacementFailureContext {
                    got: got_fallback,
                    clamped,
                },
            ));
        }

        if retry_verified {
            debug!("verified=true");
            debug!("order_used=size->pos, attempts={}", attempt_idx);
            return Ok(PlacementOutcome::Verified(PlacementSuccess {
                final_rect: got2,
                anchored_target: None,
            }));
        }

        let pos_latched = d2.x <= VERIFY_EPS && d2.y <= VERIFY_EPS;
        if pos_latched {
            debug!("pos_latched=true (x,y within eps)");
            if can_size == Some(false) {
                debug!("size_not_settable=true; skipping size-only and anchoring legal size");
            } else {
                debug!("switching to size-only adjustments");
                let size_only_label = format!("{}:size-only", label);
                match apply_size_only_and_wait(
                    &size_only_label,
                    win,
                    attr_size,
                    (target.w, target.h),
                    VERIFY_EPS,
                ) {
                    Ok((got_sz, settle_ms_sz)) => {
                        let size_only_verified = got_sz.approx_eq(&target, VERIFY_EPS);
                        log_summary(
                            label,
                            AttemptKind::SizeOnly,
                            AttemptOrder::SizeOnly,
                            attempt_idx,
                            settle_ms_sz,
                            size_only_verified,
                        );
                        let (got_anchor, anchored, settle_ms_anchor_sz) =
                            anchor_legal_size_and_wait(
                                label, win, attr_pos, attr_size, &target, &got_sz, grid.cols,
                                grid.rows, grid.col, grid.row, VERIFY_EPS,
                            )?;
                        let anchor_verified = got_anchor.approx_eq(&anchored, VERIFY_EPS);
                        log_summary(
                            label,
                            AttemptKind::AnchorSizeOnly,
                            AttemptOrder::Anchor,
                            attempt_idx.saturating_add(1),
                            settle_ms_anchor_sz,
                            anchor_verified,
                        );
                        if anchor_verified {
                            debug!("verified=true");
                            return Ok(PlacementOutcome::Verified(PlacementSuccess {
                                final_rect: got_anchor,
                                anchored_target: Some(anchored),
                            }));
                        }
                    }
                    Err(err) => {
                        debug!(
                            "size-only failed ({}); anchoring legal size using observed retry rect",
                            err
                        );
                    }
                }
            }
        }

        let (got_anchor, anchored, settle_ms_anchor) = anchor_legal_size_and_wait(
            label, win, attr_pos, attr_size, &target, &got2, grid.cols, grid.rows, grid.col,
            grid.row, VERIFY_EPS,
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
            label,
            AttemptKind::AnchorLegal,
            AttemptOrder::Anchor,
            attempt_idx.saturating_add(1),
            settle_ms_anchor,
            anchor_verified,
        );
        if anchor_verified {
            debug!("verified=true");
            debug!(
                "order_used=anchor-legal, attempts={}",
                attempt_idx.saturating_add(1)
            );
            return Ok(PlacementOutcome::Verified(PlacementSuccess {
                final_rect: got_anchor,
                anchored_target: Some(anchored),
            }));
        }

        debug!("fallback_used=true");
        let (got_fallback, settle_ms_fb) =
            fallback_shrink_move_grow(label, win, attr_pos, attr_size, &target)?;
        let smg_verified = got_fallback.approx_eq(&target, VERIFY_EPS);
        log_summary(
            label,
            AttemptKind::FallbackShrinkMoveGrow,
            AttemptOrder::Fallback,
            attempt_idx.saturating_add(1),
            settle_ms_fb,
            smg_verified,
        );
        if smg_verified {
            debug!("verified=true");
            debug!(
                "order_used=shrink->move->grow, attempts={}",
                attempt_idx.saturating_add(1)
            );
            return Ok(PlacementOutcome::Verified(PlacementSuccess {
                final_rect: got_fallback,
                anchored_target: None,
            }));
        }

        debug!("verified=false");
        log_failure_context(win, self.config.role, self.config.subrole);
        let vf = visible_frame_containing_point(
            mtm,
            geom::Point {
                x: got_fallback.cx(),
                y: got_fallback.cy(),
            },
        );
        debug!(
            "vf_used:center=({:.1},{:.1}) -> vf={}",
            got_fallback.cx(),
            got_fallback.cy(),
            vf
        );
        let clamped = clamp_flags(&got_fallback, &vf, VERIFY_EPS);
        Ok(PlacementOutcome::VerificationFailure(
            PlacementFailureContext {
                got: got_fallback,
                clamped,
            },
        ))
    }
}
