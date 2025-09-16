use std::sync::Arc;

use objc2_foundation::MainThreadMarker;

use super::{
    adapter::{AxAdapterHandle, FakeApplyResponse, FakeAxAdapter, FakeOp, FakeWindowConfig},
    common::{PlaceAttemptOptions, PlacementContext, RetryLimits},
    engine::{PlacementEngine, PlacementEngineConfig, PlacementGrid, PlacementOutcome},
};
use crate::{ax::cfstr, geom::Rect};

fn main_thread_marker() -> MainThreadMarker {
    MainThreadMarker::new().unwrap_or_else(|| unsafe { MainThreadMarker::new_unchecked() })
}

fn engine_config<'a>() -> PlacementEngineConfig<'a> {
    PlacementEngineConfig {
        label: "test",
        attr_pos: cfstr("AXPosition"),
        attr_size: cfstr("AXSize"),
        grid: PlacementGrid {
            cols: 4,
            rows: 3,
            col: 1,
            row: 1,
        },
        role: "AXWindow",
        subrole: "AXStandardWindow",
    }
}

#[test]
fn placement_succeeds_on_primary_attempt() {
    let fake = Arc::new(FakeAxAdapter::new());
    let win = fake.new_window(FakeWindowConfig::default());
    let target = Rect::new(100.0, 200.0, 640.0, 480.0);
    let visible = Rect::new(0.0, 0.0, 1440.0, 900.0);
    let adapter_handle: AxAdapterHandle = fake.clone() as AxAdapterHandle;
    let ctx = PlacementContext::with_adapter(
        win.clone(),
        target,
        visible,
        PlaceAttemptOptions::default(),
        adapter_handle,
    );
    let engine = PlacementEngine::new(&ctx, engine_config());

    let outcome = engine
        .execute(main_thread_marker())
        .expect("engine should succeed");
    match outcome {
        PlacementOutcome::Verified(success) => {
            assert_eq!(success.final_rect, target);
            assert!(success.anchored_target.is_none());
        }
        other => panic!("unexpected outcome: {:?}", other),
    }

    let ops = fake.operations(&win);
    assert!(
        ops.iter().any(|op| matches!(op, FakeOp::Apply { .. })),
        "apply op missing: {:?}",
        ops
    );
}

#[test]
fn placement_recovers_with_axis_nudge() {
    let fake = Arc::new(FakeAxAdapter::new());
    let mut config = FakeWindowConfig::default();
    config
        .apply_script
        .push(FakeApplyResponse::new(Rect::new(100.0, 210.0, 640.0, 480.0)).with_persist(true));
    config.nudge_script.push(FakeApplyResponse::new(Rect::new(
        100.0, 200.0, 640.0, 480.0,
    )));
    let win = fake.new_window(config);
    let target = Rect::new(100.0, 200.0, 640.0, 480.0);
    let visible = Rect::new(0.0, 0.0, 1440.0, 900.0);
    let adapter_handle: AxAdapterHandle = fake.clone() as AxAdapterHandle;
    let ctx = PlacementContext::with_adapter(
        win.clone(),
        target,
        visible,
        PlaceAttemptOptions::default(),
        adapter_handle,
    );
    let engine = PlacementEngine::new(&ctx, engine_config());

    let outcome = engine
        .execute(main_thread_marker())
        .expect("engine should succeed");
    match outcome {
        PlacementOutcome::Verified(success) => assert_eq!(success.final_rect, target),
        other => panic!("unexpected outcome: {:?}", other),
    }

    let ops = fake.operations(&win);
    assert!(
        ops.iter().any(|op| matches!(op, FakeOp::Nudge { .. })),
        "nudge op missing: {:?}",
        ops
    );
}

#[test]
fn placement_uses_fallback_on_retry_exhaustion() {
    let fake = Arc::new(FakeAxAdapter::new());
    let mut config = FakeWindowConfig::default();
    config
        .apply_script
        .push(FakeApplyResponse::new(Rect::new(220.0, 340.0, 640.0, 480.0)).with_persist(true));
    config
        .fallback_script
        .push(FakeApplyResponse::new(Rect::new(
            100.0, 200.0, 640.0, 480.0,
        )));
    let win = fake.new_window(config);
    let target = Rect::new(100.0, 200.0, 640.0, 480.0);
    let visible = Rect::new(0.0, 0.0, 1440.0, 900.0);
    let limits = RetryLimits::new(0, 0, 0, 1);
    let opts = PlaceAttemptOptions::default().with_retry_limits(limits);
    let adapter_handle: AxAdapterHandle = fake.clone() as AxAdapterHandle;
    let ctx = PlacementContext::with_adapter(win.clone(), target, visible, opts, adapter_handle);
    let engine = PlacementEngine::new(&ctx, engine_config());

    let outcome = engine
        .execute(main_thread_marker())
        .expect("engine should succeed");
    match outcome {
        PlacementOutcome::Verified(success) => assert_eq!(success.final_rect, target),
        other => panic!("unexpected outcome: {:?}", other),
    }

    let ops = fake.operations(&win);
    assert!(
        ops.iter().any(|op| matches!(op, FakeOp::Fallback { .. })),
        "fallback op missing: {:?}",
        ops
    );
}

#[test]
fn placement_reports_failure_when_all_attempts_exhausted() {
    let fake = Arc::new(FakeAxAdapter::new());
    let mut config = FakeWindowConfig::default();
    config
        .apply_script
        .push(FakeApplyResponse::new(Rect::new(320.0, 420.0, 640.0, 480.0)).with_persist(true));
    config
        .fallback_script
        .push(FakeApplyResponse::new(Rect::new(320.0, 420.0, 640.0, 480.0)).with_persist(true));
    let win = fake.new_window(config);
    let target = Rect::new(100.0, 200.0, 640.0, 480.0);
    let visible = Rect::new(0.0, 0.0, 1440.0, 900.0);
    let limits = RetryLimits::new(0, 0, 0, 1);
    let opts = PlaceAttemptOptions::default().with_retry_limits(limits);
    let adapter_handle: AxAdapterHandle = fake.clone() as AxAdapterHandle;
    let ctx = PlacementContext::with_adapter(win.clone(), target, visible, opts, adapter_handle);
    let engine = PlacementEngine::new(&ctx, engine_config());

    let outcome = engine
        .execute(main_thread_marker())
        .expect("engine should run");
    match outcome {
        PlacementOutcome::VerificationFailure(failure) => {
            assert_eq!(failure.got, Rect::new(320.0, 420.0, 640.0, 480.0));
            assert!(failure.timeline.entries().len() >= 2);
        }
        other => panic!("unexpected outcome: {:?}", other),
    }

    let ops = fake.operations(&win);
    assert!(
        ops.iter().any(|op| matches!(op, FakeOp::Fallback { .. })),
        "fallback op missing: {:?}",
        ops
    );
}
