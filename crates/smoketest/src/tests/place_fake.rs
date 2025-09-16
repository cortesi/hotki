use std::sync::Arc;

use mac_winops::{
    AxAdapterHandle, FakeApplyResponse, FakeAxAdapter, FakeOp, FakeWindowConfig,
    PlaceAttemptOptions, PlacementContext, PlacementEngine, PlacementEngineConfig, PlacementGrid,
    PlacementOutcome, Rect, RetryLimits, cfstr,
};
use objc2_foundation::MainThreadMarker;

use crate::error::{Error, Result};

/// Target rectangle used for fake placement runs.
const TARGET: Rect = Rect {
    x: 100.0,
    y: 200.0,
    w: 640.0,
    h: 480.0,
};
/// Visible frame used for fake placement runs.
const VISIBLE: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 1440.0,
    h: 900.0,
};

/// Run focused, id-based, and move placement flows against the fake adapter.
pub fn run_fake_place_test(_timeout_ms: u64, _logs: bool) -> Result<()> {
    run_flow(
        "place_grid_focused",
        FakeWindowConfig::default(),
        PlaceAttemptOptions::default(),
        |ops| ensure_op(ops, |op| matches!(op, FakeOp::Apply { .. }), "apply"),
    )?;

    let mut axis_cfg = FakeWindowConfig::default();
    axis_cfg
        .apply_script
        .push(FakeApplyResponse::new(Rect::new(100.0, 210.0, 640.0, 480.0)).with_persist(true));
    axis_cfg.nudge_script.push(FakeApplyResponse::new(Rect::new(
        100.0, 200.0, 640.0, 480.0,
    )));
    run_flow(
        "place_grid",
        axis_cfg,
        PlaceAttemptOptions::default(),
        |ops| ensure_op(ops, |op| matches!(op, FakeOp::Nudge { .. }), "nudge"),
    )?;

    let mut fallback_cfg = FakeWindowConfig::default();
    fallback_cfg
        .apply_script
        .push(FakeApplyResponse::new(Rect::new(320.0, 420.0, 640.0, 480.0)).with_persist(true));
    fallback_cfg
        .fallback_script
        .push(FakeApplyResponse::new(TARGET));
    let fallback_opts =
        PlaceAttemptOptions::default().with_retry_limits(RetryLimits::new(0, 0, 0, 1));
    run_flow("place_move_grid", fallback_cfg, fallback_opts, |ops| {
        ensure_op(ops, |op| matches!(op, FakeOp::Fallback { .. }), "fallback")
    })?;

    Ok(())
}

/// Ensure an expected operation was recorded by the fake adapter.
fn ensure_op<F>(ops: &[FakeOp], predicate: F, label: &str) -> Result<()>
where
    F: Fn(&FakeOp) -> bool,
{
    if ops.iter().any(predicate) {
        Ok(())
    } else {
        Err(Error::InvalidState(format!(
            "expected {} operation to be recorded (got {:?})",
            label, ops
        )))
    }
}

/// Drive a placement flow using the supplied fake window configuration and verifier.
fn run_flow<F>(
    label: &'static str,
    config: FakeWindowConfig,
    opts: PlaceAttemptOptions,
    verify_ops: F,
) -> Result<()>
where
    F: Fn(&[FakeOp]) -> Result<()>,
{
    let fake = Arc::new(FakeAxAdapter::new());
    let win = fake.new_window(config);
    let adapter_handle: AxAdapterHandle = fake.clone() as AxAdapterHandle;
    let ctx = PlacementContext::with_adapter(win.clone(), TARGET, VISIBLE, opts, adapter_handle);
    let engine = PlacementEngine::new(
        &ctx,
        PlacementEngineConfig {
            label,
            attr_pos: cfstr("AXPosition"),
            attr_size: cfstr("AXSize"),
            grid: PlacementGrid {
                cols: 3,
                rows: 2,
                col: 1,
                row: 1,
            },
            role: "AXWindow",
            subrole: "AXStandardWindow",
        },
    );
    let mtm =
        MainThreadMarker::new().unwrap_or_else(|| unsafe { MainThreadMarker::new_unchecked() });
    let outcome = engine
        .execute(mtm)
        .map_err(|e| Error::InvalidState(format!("{}: engine error {}", label, e)))?;
    match outcome {
        PlacementOutcome::Verified(success) => {
            if success.final_rect != TARGET {
                return Err(Error::InvalidState(format!(
                    "{}: expected {:?} got {:?}",
                    label, TARGET, success.final_rect
                )));
            }
        }
        other => {
            return Err(Error::InvalidState(format!(
                "{}: unexpected outcome {:?}",
                label, other
            )));
        }
    }
    let ops = fake.operations(&win);
    verify_ops(&ops)?;
    Ok(())
}
