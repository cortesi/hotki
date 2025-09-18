use core_foundation::string::CFStringRef;
use objc2_foundation::MainThreadMarker;
use tracing::debug;

use super::{
    apply::{AxAttrRefs, anchor_legal_size_and_wait},
    common::{
        AttemptKind, AttemptOrder, AttemptRecord, AttemptTimeline, FallbackInvocation,
        FallbackTrigger, PlacementContext, choose_initial_order, clamp_flags, log_failure_context,
        log_summary, one_axis_off,
    },
};
use crate::{
    Result,
    geom::{self, Rect},
};

/// Successful placement outcome details.
#[derive(Debug, Clone, Copy)]
pub struct PlacementSuccess {
    /// Final observed rectangle returned by the Accessibility API.
    pub final_rect: Rect,
    /// Anchored rectangle when verification succeeded using anchoring.
    pub anchored_target: Option<Rect>,
}

/// Context captured when verification fails.
#[derive(Debug, Clone)]
pub struct PlacementFailureContext {
    /// Last observed rectangle reported by Accessibility.
    pub got: Rect,
    /// Clamp flags comparing the observed rect against the visible frame.
    pub clamped: crate::error::ClampFlags,
    /// Visible frame used while validating the final attempt.
    pub visible_frame: Rect,
    /// Timeline of attempts executed before reporting failure.
    pub timeline: AttemptTimeline,
}

/// Result of executing the placement engine.
#[derive(Debug, Clone)]
pub enum PlacementOutcome {
    /// Placement verified successfully.
    Verified(PlacementSuccess),
    /// Placement aborted because only the initial attempt was permitted.
    PosFirstOnlyFailure(PlacementFailureContext),
    /// Placement exhausted all fallbacks without verification.
    VerificationFailure(PlacementFailureContext),
}

fn log_and_record_attempt(
    label: &str,
    timeline: &mut AttemptTimeline,
    attempt_counter: &mut u32,
    kind: AttemptKind,
    order: AttemptOrder,
    settle_ms: u64,
    verified: bool,
) -> u32 {
    let idx = *attempt_counter;
    log_summary(label, kind, order, idx, settle_ms, verified);
    timeline.push(AttemptRecord {
        kind,
        order,
        attempt_idx: idx,
        settle_ms,
        verified,
    });
    *attempt_counter = attempt_counter.saturating_add(1);
    idx
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
        let options = self.ctx.options();
        let tuning = options.tuning();
        let limits = options.retry_limits();
        let hooks = options.hooks().clone();
        let eps = tuning.epsilon();
        let timing = tuning.settle_timing();
        let label = self.config.label;
        let attr_pos = self.config.attr_pos;
        let attr_size = self.config.attr_size;
        let grid = self.config.grid;
        let attrs = AxAttrRefs {
            pos: attr_pos,
            size: attr_size,
        };
        let adapter = self.ctx.adapter();
        let adapter_ref = adapter.as_ref();

        let (can_pos, can_size) = adapter_ref.settable_pos_size(win);
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

        let mut timeline = AttemptTimeline::default();
        let mut attempt_counter = 1u32;
        let allow_axis_nudge = limits.max_axis_nudges > 0;
        let allow_opposite_order = options.force_second_attempt() || limits.max_opposite_order > 0;
        let mut anchor_attempts_remaining = limits.max_anchor_attempts;

        let (got1, settle_ms1) = adapter_ref.apply_and_wait(
            label,
            win,
            attrs,
            &target,
            initial_pos_first,
            eps,
            timing,
        )?;
        let vf2 = self.ctx.resolve_visible_frame(
            mtm,
            geom::Point {
                x: got1.cx(),
                y: got1.cy(),
            },
        );
        debug!("vf_used:center={} -> vf={}", got1.center(), vf2);
        debug!("clamp={}", clamp_flags(&got1, &vf2, eps));
        let d1 = got1.diffs(&target);
        let mut latest_rect = got1;
        let mut latest_vf = vf2;
        let mut latest_diffs = d1;
        let first_verified = got1.approx_eq(&target, eps);
        log_and_record_attempt(
            label,
            &mut timeline,
            &mut attempt_counter,
            AttemptKind::Primary,
            if initial_pos_first {
                AttemptOrder::PosThenSize
            } else {
                AttemptOrder::SizeThenPos
            },
            settle_ms1,
            first_verified,
        );
        if first_verified && !options.force_second_attempt() {
            debug!("verified=true");
            return Ok(PlacementOutcome::Verified(PlacementSuccess {
                final_rect: got1,
                anchored_target: None,
            }));
        }

        if options.pos_first_only() {
            debug!("verified=false");
            log_failure_context(adapter_ref, win, self.config.role, self.config.subrole);
            let clamped = clamp_flags(&got1, &vf2, eps);
            return Ok(PlacementOutcome::PosFirstOnlyFailure(
                PlacementFailureContext {
                    got: got1,
                    clamped,
                    visible_frame: vf2,
                    timeline,
                },
            ));
        }

        if let Some(axis) = one_axis_off(d1, eps) {
            if allow_axis_nudge {
                let (got_ax, settle_ms_ax) = adapter_ref
                    .nudge_axis_pos_and_wait(label, win, attrs, &target, axis, eps, timing)?;
                let vf3 = self.ctx.resolve_visible_frame(
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
                debug!("clamp={}", clamp_flags(&got_ax, &vf3, eps));
                let axis_order = match axis {
                    crate::geom::Axis::Horizontal => AttemptOrder::AxisHorizontal,
                    crate::geom::Axis::Vertical => AttemptOrder::AxisVertical,
                };
                let axis_verified = got_ax.approx_eq(&target, eps);
                let axis_idx = log_and_record_attempt(
                    label,
                    &mut timeline,
                    &mut attempt_counter,
                    AttemptKind::AxisNudge,
                    axis_order,
                    settle_ms_ax,
                    axis_verified,
                );
                if axis_verified {
                    debug!("verified=true");
                    debug!("order_used=axis-pos, attempts={}", axis_idx);
                    return Ok(PlacementOutcome::Verified(PlacementSuccess {
                        final_rect: got_ax,
                        anchored_target: None,
                    }));
                }
                let diffs_ax = got_ax.diffs(&target);
                if diffs_ax.x <= eps && diffs_ax.y <= eps {
                    latest_rect = got_ax;
                    latest_vf = vf3;
                    latest_diffs = diffs_ax;
                }
            } else {
                debug!("axis_nudge_skipped=true; limit_exceeded");
            }
        }

        let mut got2 = latest_rect;
        let mut vf4 = latest_vf;
        let mut retry_verified = false;
        let mut diffs_after_pos_stage = latest_diffs;
        let mut should_run_opposite = allow_opposite_order;
        let mut opposite_skip_reason: Option<&str> = None;
        if !allow_opposite_order {
            should_run_opposite = false;
            opposite_skip_reason = Some("limit_exceeded");
        } else if !options.force_second_attempt() {
            let pos_aligned = diffs_after_pos_stage.x <= eps && diffs_after_pos_stage.y <= eps;
            let size_off = diffs_after_pos_stage.w > eps || diffs_after_pos_stage.h > eps;
            if pos_aligned && size_off {
                should_run_opposite = false;
                opposite_skip_reason = Some("latched_pos_size_mismatch");
            }
        }

        if should_run_opposite {
            let (got_retry, settle_retry) = adapter_ref.apply_and_wait(
                label,
                win,
                attrs,
                &target,
                !initial_pos_first,
                eps,
                timing,
            )?;
            got2 = got_retry;
            vf4 = self.ctx.resolve_visible_frame(
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
            debug!("clamp={}", clamp_flags(&got2, &vf4, eps));
            retry_verified = got2.approx_eq(&target, eps);
            diffs_after_pos_stage = got2.diffs(&target);
            log_and_record_attempt(
                label,
                &mut timeline,
                &mut attempt_counter,
                AttemptKind::RetryOpposite,
                if initial_pos_first {
                    AttemptOrder::SizeThenPos
                } else {
                    AttemptOrder::PosThenSize
                },
                settle_retry,
                retry_verified,
            );
        } else if let Some(reason) = opposite_skip_reason {
            debug!("opposite_order_retry_skipped=true; reason={}", reason);
        }

        let forced_allowed = limits.max_fallback_runs > 0
            && hooks.should_run_fallback(&FallbackInvocation {
                trigger: FallbackTrigger::Forced,
                context: self.ctx,
                timeline: &timeline,
            });
        if forced_allowed {
            debug!("fallback_used=true (forced)");
            let (got_fallback, settle_ms_fb) =
                adapter_ref.fallback_shrink_move_grow(label, win, attrs, &target, eps, timing)?;
            let smg_verified = got_fallback.approx_eq(&target, eps);
            let fb_idx = log_and_record_attempt(
                label,
                &mut timeline,
                &mut attempt_counter,
                AttemptKind::FallbackShrinkMoveGrow,
                AttemptOrder::Fallback,
                settle_ms_fb,
                smg_verified,
            );
            if smg_verified {
                debug!("verified=true");
                debug!(
                    "order_used=shrink->move->grow (forced), attempts={}",
                    fb_idx
                );
                return Ok(PlacementOutcome::Verified(PlacementSuccess {
                    final_rect: got_fallback,
                    anchored_target: None,
                }));
            }
            debug!("verified=false");
            log_failure_context(adapter_ref, win, self.config.role, self.config.subrole);
            let vf = self.ctx.resolve_visible_frame(
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
            let clamped = clamp_flags(&got_fallback, &vf, eps);
            return Ok(PlacementOutcome::VerificationFailure(
                PlacementFailureContext {
                    got: got_fallback,
                    clamped,
                    visible_frame: vf,
                    timeline,
                },
            ));
        }

        if retry_verified {
            debug!("verified=true");
            debug!(
                "order_used=size->pos, attempts={}",
                attempt_counter.saturating_sub(1)
            );
            return Ok(PlacementOutcome::Verified(PlacementSuccess {
                final_rect: got2,
                anchored_target: None,
            }));
        }

        let pos_latched = diffs_after_pos_stage.x <= eps && diffs_after_pos_stage.y <= eps;
        if pos_latched {
            debug!("pos_latched=true (x,y within eps)");
            if can_size == Some(false) {
                debug!("size_not_settable=true; skipping size-only and anchoring legal size");
            } else {
                debug!("switching to size-only adjustments");
                let size_only_label = format!("{}:size-only", label);
                match adapter_ref.apply_size_only_and_wait(
                    &size_only_label,
                    win,
                    attrs,
                    (target.w, target.h),
                    eps,
                    timing,
                ) {
                    Ok((got_sz, settle_ms_sz)) => {
                        let size_only_verified = got_sz.approx_eq(&target, eps);
                        log_and_record_attempt(
                            label,
                            &mut timeline,
                            &mut attempt_counter,
                            AttemptKind::SizeOnly,
                            AttemptOrder::SizeOnly,
                            settle_ms_sz,
                            size_only_verified,
                        );
                        if anchor_attempts_remaining > 0 {
                            anchor_attempts_remaining = anchor_attempts_remaining.saturating_sub(1);
                            let (got_anchor, anchored, settle_ms_anchor_sz) =
                                anchor_legal_size_and_wait(
                                    adapter_ref,
                                    label,
                                    win,
                                    attrs,
                                    &target,
                                    &got_sz,
                                    grid.cols,
                                    grid.rows,
                                    grid.col,
                                    grid.row,
                                    eps,
                                    timing,
                                )?;
                            let anchor_verified = got_anchor.approx_eq(&anchored, eps);
                            log_and_record_attempt(
                                label,
                                &mut timeline,
                                &mut attempt_counter,
                                AttemptKind::AnchorSizeOnly,
                                AttemptOrder::Anchor,
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
                        } else {
                            debug!("anchor_size_only_skipped=true; limit_exceeded");
                        }
                        got2 = got_sz;
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

        if anchor_attempts_remaining > 0 {
            let (got_anchor, anchored, settle_ms_anchor) = anchor_legal_size_and_wait(
                adapter_ref,
                label,
                win,
                attrs,
                &target,
                &got2,
                grid.cols,
                grid.rows,
                grid.col,
                grid.row,
                eps,
                timing,
            )?;
            let vf5 = self.ctx.resolve_visible_frame(
                mtm,
                geom::Point {
                    x: got_anchor.cx(),
                    y: got_anchor.cy(),
                },
            );
            debug!("clamp={}", clamp_flags(&got_anchor, &vf5, eps));
            let anchor_verified = got_anchor.approx_eq(&anchored, eps);
            log_and_record_attempt(
                label,
                &mut timeline,
                &mut attempt_counter,
                AttemptKind::AnchorLegal,
                AttemptOrder::Anchor,
                settle_ms_anchor,
                anchor_verified,
            );
            if anchor_verified {
                debug!("verified=true");
                debug!(
                    "order_used=anchor-legal, attempts={}",
                    attempt_counter.saturating_sub(1)
                );
                return Ok(PlacementOutcome::Verified(PlacementSuccess {
                    final_rect: got_anchor,
                    anchored_target: Some(anchored),
                }));
            }
            got2 = got_anchor;
            vf4 = vf5;
        } else {
            debug!("anchor_legal_skipped=true; limit_exceeded");
        }

        let final_allowed = limits.max_fallback_runs > 0
            && hooks.should_run_fallback(&FallbackInvocation {
                trigger: FallbackTrigger::Final,
                context: self.ctx,
                timeline: &timeline,
            });
        if final_allowed {
            debug!("fallback_used=true");
            let (got_fallback, settle_ms_fb) =
                adapter_ref.fallback_shrink_move_grow(label, win, attrs, &target, eps, timing)?;
            let smg_verified = got_fallback.approx_eq(&target, eps);
            log_and_record_attempt(
                label,
                &mut timeline,
                &mut attempt_counter,
                AttemptKind::FallbackShrinkMoveGrow,
                AttemptOrder::Fallback,
                settle_ms_fb,
                smg_verified,
            );
            if smg_verified {
                debug!("verified=true");
                debug!(
                    "order_used=shrink->move->grow, attempts={}",
                    attempt_counter.saturating_sub(1)
                );
                return Ok(PlacementOutcome::Verified(PlacementSuccess {
                    final_rect: got_fallback,
                    anchored_target: None,
                }));
            }
            debug!("verified=false");
            log_failure_context(adapter_ref, win, self.config.role, self.config.subrole);
            let vf = self.ctx.resolve_visible_frame(
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
            let clamped = clamp_flags(&got_fallback, &vf, eps);
            return Ok(PlacementOutcome::VerificationFailure(
                PlacementFailureContext {
                    got: got_fallback,
                    clamped,
                    visible_frame: vf,
                    timeline,
                },
            ));
        }

        debug!("verified=false");
        log_failure_context(adapter_ref, win, self.config.role, self.config.subrole);
        let clamped = clamp_flags(&got2, &vf4, eps);
        Ok(PlacementOutcome::VerificationFailure(
            PlacementFailureContext {
                got: got2,
                clamped,
                visible_frame: vf4,
                timeline,
            },
        ))
    }
}
