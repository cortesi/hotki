//! Grid placement for macOS windows runs through this module.
//!
//! `main_thread_ops` enqueues `MainOp::Place*` requests on the AppKit main
//! thread. Once drained, those requests resolve Accessibility windows and call
//! the functions re-exported from this module to perform the actual move. The
//! pipeline is shared by focused, id-based, and directional placements and
//! keeps the following flow:
//!
//! * Normalize window state and skip unsupported roles/subroles.
//! * Resolve the target grid cell within the screen's visible frame and
//!   translate it into global coordinates.
//! * Optionally "safe-park" windows near the global origin on secondary
//!   displays so subsequent moves happen in a stable coordinate space.
//! * Attempt placement using Accessibility setters via `apply`, choosing
//!   position-first or size-first order from cached hints and settling within
//!   the configured epsilon (defaults to `VERIFY_EPS`).
//! * Validate the resulting rect; nudge a single axis, retry with the opposite
//!   order, or fall back to shrink→move→grow when still clamped.
//!
//! Callers rely on several invariants: placement runs on the main thread with
//! Accessibility trust already granted, grid dimensions are non-zero, the
//! target rect remains inside the chosen visible frame, and success is only
//! reported when the final rect matches the requested cell within
//! `VERIFY_EPS`. The helper functions in this module remain side-effect free
//! so unit tests can lock down the geometry math while smoketests exercise the
//! full pipeline through `main_thread_ops` entry points.

use crate::geom;

const AX_WINDOW_RETRIES: usize = 40;
const AX_WINDOW_RETRY_DELAY_MS: u64 = 20;

mod adapter;
mod apply;
mod common;
#[cfg(test)]
mod deterministic_tests;
mod engine;
mod fallback;
mod metrics;
mod normalize;
mod ops_focused;
mod ops_id;
mod ops_move;
#[cfg(test)]
mod property_tests;

#[allow(unused_imports)]
pub use adapter::{
    AxAdapter, AxAdapterHandle, FakeApplyResponse, FakeAxAdapter, FakeOp, FakeWindowConfig,
};
#[allow(unused_imports)]
pub use common::{
    AttemptRecord, AttemptTimeline, FallbackInvocation, FallbackTrigger, PlaceAttemptOptions,
    PlacementContext, RetryLimits,
};
pub use engine::{PlacementEngine, PlacementEngineConfig, PlacementGrid, PlacementOutcome};
pub use metrics::{AttemptKind, AttemptOrder, PlacementCountersSnapshot};
pub(crate) use normalize::{normalize_before_move, skip_reason_for_role_subrole};
pub use ops_focused::{place_grid_focused, place_grid_focused_opts};
pub(crate) use ops_id::place_grid_opts;
pub use ops_move::place_move_grid;
pub(crate) use ops_move::place_move_grid_opts;

/// Resolve an AX window by id, retrying briefly when the window has not yet
/// surfaced in the AXWindows list. Newly spawned helpers sometimes delay their
/// accessibility registration; a short retry loop keeps placement callers from
/// racing that hand-off.
pub(super) fn ax_window_for_id_with_retry(
    id: crate::WindowId,
) -> crate::Result<(crate::AXElem, i32)> {
    let mut attempts = 0usize;
    loop {
        match crate::ax::ax_window_for_id(id) {
            Ok(found) => return Ok(found),
            Err(crate::Error::FocusedWindow) if attempts < AX_WINDOW_RETRIES => {
                attempts = attempts.saturating_add(1);
                common::sleep_ms(AX_WINDOW_RETRY_DELAY_MS);
            }
            Err(err) => return Err(err),
        }
    }
}

/// Capture a snapshot of placement attempt counters for diagnostics.
pub fn placement_counters_snapshot() -> PlacementCountersSnapshot {
    metrics::PLACEMENT_COUNTERS.snapshot()
}

/// Reset placement counters back to zero. Intended for deterministic tests.
pub fn placement_counters_reset() {
    metrics::PLACEMENT_COUNTERS.reset();
}

#[inline]
fn grid_guess_cell_by_pos(
    vf_x: f64,
    vf_y: f64,
    vf_w: f64,
    vf_h: f64,
    cols: u32,
    rows: u32,
    pos: geom::Point,
) -> (u32, u32) {
    let cols_f = cols.max(1) as f64;
    let rows_f = rows.max(1) as f64;
    let tile_w = (vf_w / cols_f).floor().max(1.0);
    let tile_h = (vf_h / rows_f).floor().max(1.0);
    let mut c = ((pos.x - vf_x) / tile_w).floor() as i64;
    let mut r = ((pos.y - vf_y) / tile_h).floor() as i64;
    if c < 0 {
        c = 0;
    }
    if r < 0 {
        r = 0;
    }
    if c as u32 >= cols {
        c = cols.saturating_sub(1) as i64;
    }
    if r as u32 >= rows {
        r = rows.saturating_sub(1) as i64;
    }
    (c as u32, r as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Point;

    #[test]
    fn guesses_cell_in_middle_of_grid() {
        let (col, row) =
            grid_guess_cell_by_pos(0.0, 0.0, 1200.0, 900.0, 4, 3, Point { x: 650.0, y: 620.0 });
        assert_eq!((col, row), (2, 2));
    }

    #[test]
    fn clamps_negative_coordinates_to_zero() {
        let (col, row) = grid_guess_cell_by_pos(
            200.0,
            100.0,
            800.0,
            600.0,
            3,
            2,
            Point { x: 120.0, y: 80.0 },
        );
        assert_eq!((col, row), (0, 0));
    }

    #[test]
    fn clamps_out_of_range_to_last_cell() {
        let (col, row) =
            grid_guess_cell_by_pos(50.0, 50.0, 500.0, 400.0, 5, 4, Point { x: 900.0, y: 600.0 });
        assert_eq!((col, row), (4, 3));
    }

    #[test]
    fn placement_counters_snapshot_and_reset() {
        super::metrics::PLACEMENT_COUNTERS.reset();
        super::metrics::PLACEMENT_COUNTERS.record_attempt(
            super::metrics::AttemptKind::Primary,
            12,
            true,
        );
        let snapshot = super::placement_counters_snapshot();
        assert_eq!(snapshot.primary.attempts, 1);
        assert_eq!(snapshot.primary.verified, 1);
        assert_eq!(snapshot.primary.settle_ms_total, 12);
        super::placement_counters_reset();
        let reset_snapshot = super::placement_counters_snapshot();
        assert_eq!(reset_snapshot.primary.attempts, 0);
        assert_eq!(reset_snapshot.primary.verified, 0);
        assert_eq!(reset_snapshot.primary.settle_ms_total, 0);
    }
}
