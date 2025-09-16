use std::sync::atomic::{AtomicU64, Ordering};

use once_cell::sync::Lazy;

/// Categorization of placement attempts for structured tracing and metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptKind {
    /// First attempt using the preferred ordering from AX hints.
    Primary,
    /// Single-axis nudge when only one axis remains out of bounds.
    AxisNudge,
    /// Re-run with the opposite setter ordering.
    RetryOpposite,
    /// Adjust size while keeping the latched position.
    SizeOnly,
    /// Anchor observed legal size back onto the grid after a size-only adjust.
    AnchorSizeOnly,
    /// Anchor observed legal size directly after a retry.
    AnchorLegal,
    /// Shrink→move→grow fallback sequence.
    FallbackShrinkMoveGrow,
}

/// Ordering applied for a placement attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptOrder {
    PosThenSize,
    SizeThenPos,
    AxisHorizontal,
    AxisVertical,
    SizeOnly,
    Anchor,
    Fallback,
}

#[derive(Default)]
struct AttemptCounters {
    attempts: AtomicU64,
    verified: AtomicU64,
    settle_ms_total: AtomicU64,
}

impl AttemptCounters {
    fn record(&self, settle_ms: u64, verified: bool) {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        self.settle_ms_total.fetch_add(settle_ms, Ordering::Relaxed);
        if verified {
            self.verified.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn snapshot(&self) -> AttemptSnapshot {
        AttemptSnapshot {
            attempts: self.attempts.load(Ordering::Relaxed),
            verified: self.verified.load(Ordering::Relaxed),
            settle_ms_total: self.settle_ms_total.load(Ordering::Relaxed),
        }
    }

    fn reset(&self) {
        self.attempts.store(0, Ordering::Relaxed);
        self.verified.store(0, Ordering::Relaxed);
        self.settle_ms_total.store(0, Ordering::Relaxed);
    }
}

/// Aggregate counters for placement attempts and fallbacks.
#[derive(Default)]
pub(crate) struct PlacementCounters {
    primary: AttemptCounters,
    axis_nudge: AttemptCounters,
    retry_opposite: AttemptCounters,
    size_only: AttemptCounters,
    anchor_size_only: AttemptCounters,
    anchor_legal: AttemptCounters,
    fallback_smg: AttemptCounters,
    safe_park: AtomicU64,
    failures: AtomicU64,
}

impl PlacementCounters {
    fn bucket(&self, kind: AttemptKind) -> &AttemptCounters {
        match kind {
            AttemptKind::Primary => &self.primary,
            AttemptKind::AxisNudge => &self.axis_nudge,
            AttemptKind::RetryOpposite => &self.retry_opposite,
            AttemptKind::SizeOnly => &self.size_only,
            AttemptKind::AnchorSizeOnly => &self.anchor_size_only,
            AttemptKind::AnchorLegal => &self.anchor_legal,
            AttemptKind::FallbackShrinkMoveGrow => &self.fallback_smg,
        }
    }

    pub(crate) fn record_attempt(&self, kind: AttemptKind, settle_ms: u64, verified: bool) {
        self.bucket(kind).record(settle_ms, verified);
    }

    pub(crate) fn record_safe_park(&self) {
        self.safe_park.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> PlacementCountersSnapshot {
        PlacementCountersSnapshot {
            primary: self.primary.snapshot(),
            axis_nudge: self.axis_nudge.snapshot(),
            retry_opposite: self.retry_opposite.snapshot(),
            size_only: self.size_only.snapshot(),
            anchor_size_only: self.anchor_size_only.snapshot(),
            anchor_legal: self.anchor_legal.snapshot(),
            fallback_smg: self.fallback_smg.snapshot(),
            safe_park: self.safe_park.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn reset(&self) {
        self.primary.reset();
        self.axis_nudge.reset();
        self.retry_opposite.reset();
        self.size_only.reset();
        self.anchor_size_only.reset();
        self.anchor_legal.reset();
        self.fallback_smg.reset();
        self.safe_park.store(0, Ordering::Relaxed);
        self.failures.store(0, Ordering::Relaxed);
    }
}

/// Snapshot of attempt statistics for inspection in tests or diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptSnapshot {
    pub attempts: u64,
    pub verified: u64,
    pub settle_ms_total: u64,
}

/// Snapshot of placement counters, including fallback and failure tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacementCountersSnapshot {
    pub primary: AttemptSnapshot,
    pub axis_nudge: AttemptSnapshot,
    pub retry_opposite: AttemptSnapshot,
    pub size_only: AttemptSnapshot,
    pub anchor_size_only: AttemptSnapshot,
    pub anchor_legal: AttemptSnapshot,
    pub fallback_smg: AttemptSnapshot,
    pub safe_park: u64,
    pub failures: u64,
}

pub(crate) static PLACEMENT_COUNTERS: Lazy<PlacementCounters> =
    Lazy::new(PlacementCounters::default);
