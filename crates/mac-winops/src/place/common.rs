use std::{fmt, sync::Arc};

use tracing::debug;

// Shared placement utilities: constants, small helpers, and attempt options.
use super::adapter::{self, AxAdapter, AxAdapterHandle};
use super::metrics::PLACEMENT_COUNTERS;
pub(super) use super::metrics::{AttemptKind, AttemptOrder};
use crate::geom::{self, Rect};

/// Default epsilon tolerance (in points) used to verify post-placement geometry.
pub(super) const VERIFY_EPS: f64 = 2.0;

pub(super) const POLL_SLEEP_MS: u64 = 25;
pub(super) const POLL_TOTAL_MS: u64 = 400;

// Use the unified geometry axis everywhere in placement modules.
pub(super) use crate::geom::Axis;

/// Timing controls for setter application and settle polling.
#[derive(Debug, Clone, Copy)]
pub struct SettleTiming {
    pub apply_stutter_ms: u64,
    pub settle_sleep_ms: u64,
    pub settle_total_ms: u64,
}

impl SettleTiming {
    #[inline]
    pub const fn new(apply_stutter_ms: u64, settle_sleep_ms: u64, settle_total_ms: u64) -> Self {
        Self {
            apply_stutter_ms,
            settle_sleep_ms,
            settle_total_ms,
        }
    }
}

impl Default for SettleTiming {
    fn default() -> Self {
        Self::new(2, 20, 600)
    }
}

/// Limits governing retry behaviour within the placement pipeline.
#[derive(Debug, Clone, Copy)]
pub struct RetryLimits {
    pub max_axis_nudges: u32,
    pub max_opposite_order: u32,
    pub max_anchor_attempts: u32,
    pub max_fallback_runs: u32,
}

impl RetryLimits {
    #[inline]
    pub const fn new(
        max_axis_nudges: u32,
        max_opposite_order: u32,
        max_anchor_attempts: u32,
        max_fallback_runs: u32,
    ) -> Self {
        Self {
            max_axis_nudges,
            max_opposite_order,
            max_anchor_attempts,
            max_fallback_runs,
        }
    }
}

impl Default for RetryLimits {
    fn default() -> Self {
        // Defaults match the previous hard-coded pipeline: one axis nudge,
        // one opposite-order retry, both anchor passes, and a single fallback run.
        Self::new(1, 1, 2, 1)
    }
}

/// Tunable parameters applied to every attempt within a placement run.
#[derive(Debug, Clone, Copy)]
pub struct PlacementTuning {
    epsilon: f64,
    settle_timing: SettleTiming,
}

impl PlacementTuning {
    #[inline]
    pub const fn new(epsilon: f64, settle_timing: SettleTiming) -> Self {
        Self {
            epsilon,
            settle_timing,
        }
    }

    #[inline]
    pub fn with_epsilon(mut self, epsilon: f64) -> Self {
        self.epsilon = epsilon;
        self
    }

    #[inline]
    pub fn with_settle_timing(mut self, timing: SettleTiming) -> Self {
        self.settle_timing = timing;
        self
    }

    #[inline]
    pub fn epsilon(&self) -> f64 {
        self.epsilon
    }

    #[inline]
    pub fn settle_timing(&self) -> SettleTiming {
        self.settle_timing
    }
}

impl Default for PlacementTuning {
    fn default() -> Self {
        Self::new(VERIFY_EPS, SettleTiming::default())
    }
}

/// Recorded attempt summary used for diagnostics and hooks.
#[derive(Debug, Clone, Copy)]
pub struct AttemptRecord {
    pub kind: AttemptKind,
    pub order: AttemptOrder,
    pub attempt_idx: u32,
    pub settle_ms: u64,
    pub verified: bool,
}

/// Timeline of placement attempts.
#[derive(Debug, Default, Clone)]
pub struct AttemptTimeline {
    entries: Vec<AttemptRecord>,
}

impl AttemptTimeline {
    #[inline]
    pub fn push(&mut self, record: AttemptRecord) {
        self.entries.push(record);
    }

    #[inline]
    pub fn entries(&self) -> &[AttemptRecord] {
        &self.entries
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Stage at which shrink→move→grow fallback is being considered.
impl fmt::Display for AttemptRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{{idx={} kind={:?} order={:?} settle_ms={} verified={}}}",
            self.attempt_idx, self.kind, self.order, self.settle_ms, self.verified
        )
    }
}

impl fmt::Display for AttemptTimeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.entries.is_empty() {
            return write!(f, "[]");
        }
        write!(f, "[")?;
        for (idx, record) in self.entries.iter().enumerate() {
            if idx > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", record)?;
        }
        write!(f, "]")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackTrigger {
    /// Legacy forced fallback requested by tests or hooks.
    Forced,
    /// Final fallback after all other attempts failed to verify.
    Final,
}

/// Invocation context handed to fallback decision hooks.
#[derive(Debug)]
pub struct FallbackInvocation<'a> {
    pub trigger: FallbackTrigger,
    pub context: &'a PlacementContext,
    pub timeline: &'a AttemptTimeline,
}

type SafeParkHook = dyn Fn(&PlacementContext) -> bool + Send + Sync;
type FallbackHook = dyn Fn(&FallbackInvocation<'_>) -> bool + Send + Sync;

/// Customisable decision hooks for safe-park and fallback execution.
#[derive(Clone)]
pub struct PlacementDecisionHooks {
    safe_park: Arc<SafeParkHook>,
    fallback: Arc<FallbackHook>,
}

impl fmt::Debug for PlacementDecisionHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlacementDecisionHooks")
            .field("safe_park", &"hook")
            .field("fallback", &"hook")
            .finish()
    }
}

impl PlacementDecisionHooks {
    #[inline]
    pub(crate) fn new(safe_park: Arc<SafeParkHook>, fallback: Arc<FallbackHook>) -> Self {
        Self {
            safe_park,
            fallback,
        }
    }

    #[inline]
    pub fn with_safe_park<F>(mut self, hook: F) -> Self
    where
        F: Fn(&PlacementContext) -> bool + Send + Sync + 'static,
    {
        self.safe_park = Arc::new(hook);
        self
    }

    #[inline]
    pub fn with_fallback<F>(mut self, hook: F) -> Self
    where
        F: Fn(&FallbackInvocation<'_>) -> bool + Send + Sync + 'static,
    {
        self.fallback = Arc::new(hook);
        self
    }

    #[inline]
    pub fn should_safe_park(&self, ctx: &PlacementContext) -> bool {
        (self.safe_park)(ctx)
    }

    #[inline]
    pub fn should_run_fallback(&self, invocation: &FallbackInvocation<'_>) -> bool {
        (self.fallback)(invocation)
    }
}

impl Default for PlacementDecisionHooks {
    fn default() -> Self {
        Self::new(
            Arc::new(default_safe_park_hook),
            Arc::new(default_fallback_hook),
        )
    }
}

fn safe_park_required(target: &Rect, vf: &Rect, eps: f64) -> bool {
    let near_zero =
        crate::geom::approx_eq(target.x, 0.0, eps) && crate::geom::approx_eq(target.y, 0.0, eps);
    let non_primary =
        !crate::geom::approx_eq(vf.x, 0.0, eps) || !crate::geom::approx_eq(vf.y, 0.0, eps);
    near_zero && non_primary
}

fn default_safe_park_hook(ctx: &PlacementContext) -> bool {
    safe_park_required(
        ctx.target(),
        ctx.visible_frame(),
        ctx.options().tuning().epsilon(),
    )
}

fn default_fallback_hook(invocation: &FallbackInvocation<'_>) -> bool {
    matches!(invocation.trigger, FallbackTrigger::Final)
}

/// Options controlling placement attempts, tuning, and decision hooks.
#[derive(Debug, Clone, Default)]
pub struct PlaceAttemptOptions {
    force_second_attempt: bool,
    pos_first_only: bool,
    retry_limits: RetryLimits,
    tuning: PlacementTuning,
    hooks: PlacementDecisionHooks,
}

impl PlaceAttemptOptions {
    #[inline]
    pub fn force_second_attempt(&self) -> bool {
        self.force_second_attempt
    }

    #[inline]
    pub fn pos_first_only(&self) -> bool {
        self.pos_first_only
    }

    #[inline]
    pub fn retry_limits(&self) -> RetryLimits {
        self.retry_limits
    }

    #[inline]
    pub fn tuning(&self) -> PlacementTuning {
        self.tuning
    }

    #[inline]
    pub fn hooks(&self) -> &PlacementDecisionHooks {
        &self.hooks
    }

    #[inline]
    pub fn with_force_second_attempt(mut self, enabled: bool) -> Self {
        self.force_second_attempt = enabled;
        self
    }

    #[inline]
    pub fn with_pos_first_only(mut self, enabled: bool) -> Self {
        self.pos_first_only = enabled;
        self
    }

    #[inline]
    pub fn with_retry_limits(mut self, limits: RetryLimits) -> Self {
        self.retry_limits = limits;
        self
    }

    #[inline]
    pub fn with_tuning(mut self, tuning: PlacementTuning) -> Self {
        self.tuning = tuning;
        self
    }

    #[inline]
    pub fn with_safe_park_hook<F>(mut self, hook: F) -> Self
    where
        F: Fn(&PlacementContext) -> bool + Send + Sync + 'static,
    {
        self.hooks = self.hooks.clone().with_safe_park(hook);
        self
    }

    #[inline]
    pub fn with_fallback_hook<F>(mut self, hook: F) -> Self
    where
        F: Fn(&FallbackInvocation<'_>) -> bool + Send + Sync + 'static,
    {
        self.hooks = self.hooks.clone().with_fallback(hook);
        self
    }
}

/// Shared placement inputs derived during normalization.
#[derive(Clone)]
pub struct PlacementContext {
    win: crate::AXElem,
    target: Rect,
    visible_frame: Rect,
    attempt_options: PlaceAttemptOptions,
    adapter: AxAdapterHandle,
}

impl fmt::Debug for PlacementContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlacementContext")
            .field("win_ptr", &(self.win.as_ptr() as *const ()))
            .field("target", &self.target)
            .field("visible_frame", &self.visible_frame)
            .field("attempt_options", &self.attempt_options)
            .field("adapter", &"adapter")
            .finish()
    }
}

impl PlacementContext {
    /// Build a placement context from the normalized window state.
    #[inline]
    pub(crate) fn new(
        win: crate::AXElem,
        target: Rect,
        visible_frame: Rect,
        attempt_options: PlaceAttemptOptions,
    ) -> Self {
        Self::with_adapter(
            win,
            target,
            visible_frame,
            attempt_options,
            adapter::system_adapter_handle(),
        )
    }

    pub fn with_adapter(
        win: crate::AXElem,
        target: Rect,
        visible_frame: Rect,
        attempt_options: PlaceAttemptOptions,
        adapter: AxAdapterHandle,
    ) -> Self {
        Self {
            win,
            target,
            visible_frame,
            attempt_options,
            adapter,
        }
    }

    /// Access the retained Accessibility element for subsequent operations.
    #[inline]
    pub(crate) fn win(&self) -> &crate::AXElem {
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
    pub(crate) fn attempt_options(&self) -> PlaceAttemptOptions {
        self.attempt_options.clone()
    }

    /// Borrow the placement options without cloning.
    #[inline]
    pub(crate) fn options(&self) -> &PlaceAttemptOptions {
        &self.attempt_options
    }

    /// Access the tuning values for this context.
    #[inline]
    pub fn tuning(&self) -> PlacementTuning {
        self.attempt_options.tuning()
    }

    /// Access the adapter used for Accessibility operations.
    #[inline]
    pub(crate) fn adapter(&self) -> AxAdapterHandle {
        self.adapter.clone()
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
pub(super) fn log_failure_context(
    adapter: &dyn AxAdapter,
    win: &crate::AXElem,
    role: &str,
    subrole: &str,
) {
    let (can_pos, can_size) = adapter.settable_pos_size(win);
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

    #[test]
    fn default_safe_park_matches_previous_heuristic() {
        let target = Rect {
            x: 0.0,
            y: 0.0,
            w: 640.0,
            h: 480.0,
        };
        let vf_secondary = Rect {
            x: 2560.0,
            y: 0.0,
            w: 1440.0,
            h: 900.0,
        };
        let vf_primary = Rect {
            x: 0.0,
            y: 0.0,
            w: 1440.0,
            h: 900.0,
        };
        assert!(safe_park_required(&target, &vf_secondary, VERIFY_EPS));
        assert!(!safe_park_required(&target, &vf_primary, VERIFY_EPS));
        let far_target = Rect {
            x: 400.0,
            y: 300.0,
            w: 640.0,
            h: 480.0,
        };
        assert!(!safe_park_required(&far_target, &vf_secondary, VERIFY_EPS));
    }
}
