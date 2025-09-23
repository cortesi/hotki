//! Placement smoketest cases implemented against the mimic harness.
use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use hotki_world::{
    Frames, MinimizedPolicy, MoveDirection, PlaceAttemptOptions, PlaceOptions, RaiseStrategy,
    RectDelta, RectPx, VisibilityPolicy, WaitConfig, WindowKey, WindowMode, WindowObserver,
    WorldHandle,
    mimic::{HelperConfig, MimicHandle, MimicScenario, MimicSpec, pump_active_mimics, spawn_mimic},
};
use hotki_world_ids::WorldWindowId;
use mac_winops::{
    self, AxAdapterHandle, FakeApplyResponse, FakeAxAdapter, FakeOp, FakeWindowConfig,
    FallbackTrigger, PlacementContext, PlacementCountersSnapshot, PlacementEngine,
    PlacementEngineConfig, PlacementGrid, PlacementOutcome, Rect, RetryLimits, screen,
};
use objc2_foundation::MainThreadMarker;
use serde_json::json;
use tracing::debug;

use super::support::{
    MainOpsDrainGuard, block_on_with_pump, ensure_window_ready, record_mimic_diagnostics,
    shutdown_mimic,
};
use crate::{
    config,
    error::{Error, Result},
    focus_guard::FocusGuard,
    helpers,
    suite::{CaseCtx, StageHandle},
    world,
};

/// Tracks helper state shared across placement test stages.
struct PlaceState {
    /// Handle to the spawned mimic windows used by the case.
    mimic: MimicHandle,
    /// Identifier passed to world placement commands.
    target_id: WorldWindowId,
    /// Key used when querying authoritative frame metadata.
    target_key: WindowKey,
    /// Expected authoritative rectangle after placement settles.
    expected: RectPx,
    /// Registry slug used when emitting diagnostics and artifacts.
    slug: &'static str,
    /// Title assigned to the helper window for focus operations.
    title: String,
    /// Raise strategy requested by the helper configuration.
    raise: RaiseStrategy,
}

/// Timing breakdown captured for the nonresizable move case.
#[derive(Default)]
struct MoveCaseBudget {
    /// Milliseconds spent warming up the mimic before spawning.
    warmup_ms: u64,
    /// Milliseconds required to spawn and resolve the helper window.
    spawn_ms: u64,
    /// Milliseconds spent waiting for initial frames to appear.
    initial_frames_ms: u64,
    /// Milliseconds consumed raising the helper during the action stage.
    action_raise_ms: u64,
    /// Milliseconds required to issue the move request and refresh cursors.
    move_request_ms: u64,
    /// Milliseconds spent waiting for the final frame to satisfy assertions.
    settle_wait_ms: u64,
}

/// Shared state for move-focused placement cases.
struct MoveCaseState {
    /// Placement harness state shared across stages.
    place: PlaceState,
    /// Expected authoritative rectangle after the move settles.
    expected: RectPx,
    /// Pixel tolerance applied when validating the final frame.
    eps: i32,
    /// Timing breakdown captured across stages.
    budget: MoveCaseBudget,
}

/// Verify fake adapter flows exercise apply, nudge, and fallback paths.
pub fn place_fake_adapter(ctx: &mut CaseCtx<'_>) -> Result<()> {
    ctx.setup(|stage| {
        let summaries = run_fake_adapter_scenarios()?;
        let slug = "place.fake.adapter";
        let ops_path = stage
            .artifacts_dir()
            .join(format!("{}_ops.json", slug.replace('.', "_")));
        let entries: Vec<_> = summaries
            .iter()
            .map(|(label, ops)| {
                json!({
                    "label": label,
                    "ops": ops.iter().map(|op| format!("{op:?}")).collect::<Vec<_>>()
                })
            })
            .collect();
        let payload = json!({ "scenarios": entries });
        let mut data = serde_json::to_string_pretty(&payload).map_err(|e| {
            Error::InvalidState(format!("failed to serialize fake adapter ops: {e}"))
        })?;
        data.push('\n');
        fs::write(&ops_path, data)?;
        stage.record_artifact(&ops_path);
        Ok(())
    })?;

    ctx.action(|_| Ok(()))?;
    ctx.settle(|_| Ok(()))?;
    Ok(())
}

/// Verify placement converges when the helper begins minimized and must be restored first.
pub fn place_minimized_defer(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let mut state: Option<PlaceState> = None;
    ctx.setup(|stage| {
        let expected = RectPx {
            x: 160,
            y: 160,
            w: 540,
            h: 320,
        };
        state = Some(spawn_place_state(
            stage,
            "place.minimized.defer",
            expected,
            |config, expected| {
                config.start_minimized = true;
                config.apply_target = Some(rect_to_f64(expected));
                config.place = PlaceOptions {
                    raise: RaiseStrategy::AppActivate,
                    minimized: MinimizedPolicy::AutoUnminimize,
                    animate: false,
                };
                config.time_ms = 25_000;
            },
        )?);
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state
            .as_ref()
            .ok_or_else(|| Error::InvalidState("place state missing during action".into()))?;
        let world = stage.world_clone();
        request_grid(&world, state_ref.target_id, (3, 2, 1, 0))?;
        Ok(())
    })?;

    ctx.settle(|stage| {
        let state_data = state
            .take()
            .ok_or_else(|| Error::InvalidState("place state missing during settle".into()))?;
        let world = stage.world_clone();
        let observer = world
            .window_observer_with_config(state_data.target_key, helpers::default_wait_config());
        let frames = wait_for_expected(stage, &state_data, observer, 2)?;
        let diag_path = record_mimic_diagnostics(stage, state_data.slug, &state_data.mimic)?;
        let artifacts = [diag_path];
        helpers::assert_frame_matches(
            stage.case_name(),
            state_data.expected,
            &frames,
            2,
            &artifacts,
        )?;
        shutdown_mimic(state_data.mimic)?;
        Ok(())
    })?;

    Ok(())
}

/// Verify placement normalizes geometry after starting from a zoomed window state.
pub fn place_zoomed_normalize(ctx: &mut CaseCtx<'_>) -> Result<()> {
    const SLUG: &str = "place.zoomed.normalize";
    let grid = (config::PLACE.grid_cols, config::PLACE.grid_rows, 0, 0);
    let mut state: Option<PlaceState> = None;
    ctx.setup(|stage| {
        let placeholder = RectPx {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        state = Some(spawn_place_state(stage, SLUG, placeholder, |config, _| {
            config.time_ms = 10_000;
            config.label_text = Some("ZOOM".into());
            config.start_zoomed = true;
            config.grid = Some(grid);
            config.place = PlaceOptions {
                raise: RaiseStrategy::SmartRaise {
                    deadline: Duration::from_millis(
                        config::INPUT_DELAYS
                            .ui_action_delay_ms
                            .saturating_mul(20)
                            .max(2_000),
                    ),
                },
                minimized: MinimizedPolicy::DeferUntilUnminimized,
                animate: false,
            };
        })?);
        if let Some(place) = state.as_mut() {
            let world = stage.world_clone();
            let frames = wait_for_initial_frames(stage.case_name(), &world, place.target_key)?;
            let expected = grid_rect_from_frames(&frames, grid.0, grid.1, grid.2, grid.3)?;
            rewrite_expected_artifact(stage, place.slug, expected)?;
            place.expected = expected;
        }
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("place state missing during action".into()))?;
        let world = stage.world_clone();
        let focus_guard = promote_helper_frontmost(stage.case_name(), state_ref)?;
        ensure_window_on_active_space(
            stage.case_name(),
            state_ref,
            &world,
            Duration::from_millis(1_600),
        )?;
        request_grid(&world, state_ref.target_id, grid)?;
        focus_guard.reassert()?;
        Ok(())
    })?;

    ctx.settle(|stage| {
        let state_data = state
            .take()
            .ok_or_else(|| Error::InvalidState("place state missing during settle".into()))?;
        let world = stage.world_clone();
        let observer = world
            .window_observer_with_config(state_data.target_key, helpers::default_wait_config());
        let frames = wait_for_expected(
            stage,
            &state_data,
            observer,
            config::PLACE.eps.round() as i32,
        )?;
        let diag_path = record_mimic_diagnostics(stage, state_data.slug, &state_data.mimic)?;
        let artifacts = [diag_path];
        helpers::assert_frame_matches(
            stage.case_name(),
            state_data.expected,
            &frames,
            config::PLACE.eps.round() as i32,
            &artifacts,
        )?;
        shutdown_mimic(state_data.mimic)?;
        Ok(())
    })?;

    Ok(())
}

/// Exercise tweened placement to ensure animated geometry converges to the target rectangle.
pub fn place_animated_tween(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let mut state: Option<PlaceState> = None;
    ctx.setup(|stage| {
        let expected = RectPx {
            x: 120,
            y: 120,
            w: 600,
            h: 360,
        };
        state = Some(spawn_place_state(
            stage,
            "place.animated.tween",
            expected,
            |config, expected| {
                config.tween_ms = 180;
                config.delay_apply_ms = 120;
                config.apply_target = Some(rect_to_f64(expected));
                config.place = PlaceOptions {
                    raise: RaiseStrategy::AppActivate,
                    minimized: MinimizedPolicy::AutoUnminimize,
                    animate: true,
                };
                config.time_ms = 25_000;
            },
        )?);
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("place state missing during action".into()))?;
        let world = stage.world_clone();
        request_grid(&world, state_ref.target_id, (3, 2, 2, 0))?;
        Ok(())
    })?;

    ctx.settle(|stage| {
        let state_data = state
            .take()
            .ok_or_else(|| Error::InvalidState("place state missing during settle".into()))?;
        let world = stage.world_clone();
        let observer = world
            .window_observer_with_config(state_data.target_key, helpers::default_wait_config());
        let frames = wait_for_expected(stage, &state_data, observer, 2)?;
        let diag_path = record_mimic_diagnostics(stage, state_data.slug, &state_data.mimic)?;
        let artifacts = [diag_path];
        helpers::assert_frame_matches(
            stage.case_name(),
            state_data.expected,
            &frames,
            2,
            &artifacts,
        )?;
        shutdown_mimic(state_data.mimic)?;
        Ok(())
    })?;

    Ok(())
}

/// Exercise delayed geometry application to mimic asynchronous placement behaviour.
pub fn place_async_delay(ctx: &mut CaseCtx<'_>) -> Result<()> {
    const GRID: (u32, u32, u32, u32) = (3, 2, 0, 1);
    let mut state: Option<PlaceState> = None;
    ctx.setup(|stage| {
        let placeholder = RectPx {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        state = Some(spawn_place_state(
            stage,
            "place.async.delay",
            placeholder,
            |config, _expected| {
                config.delay_apply_ms = 220;
                config.apply_grid = Some(GRID);
                config.place = PlaceOptions {
                    raise: RaiseStrategy::AppActivate,
                    minimized: MinimizedPolicy::DeferUntilUnminimized,
                    animate: false,
                };
                config.time_ms = 25_000;
            },
        )?);
        if let Some(place_state) = state.as_mut() {
            let focus_guard = promote_helper_frontmost(stage.case_name(), place_state)?;
            let world = stage.world_clone();
            let frames =
                wait_for_initial_frames(stage.case_name(), &world, place_state.target_key)?;
            let expected = grid_rect_from_frames(&frames, GRID.0, GRID.1, GRID.2, GRID.3)?;
            rewrite_expected_artifact(stage, place_state.slug, expected)?;
            place_state.expected = expected;
            focus_guard.reassert()?;
        }
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("place state missing during action".into()))?;
        let world = stage.world_clone();
        let focus_guard = promote_helper_frontmost(stage.case_name(), state_ref)?;
        request_grid(&world, state_ref.target_id, GRID)?;
        focus_guard.reassert()?;
        Ok(())
    })?;

    ctx.settle(|stage| {
        let state_data = state
            .take()
            .ok_or_else(|| Error::InvalidState("place state missing during settle".into()))?;
        let world = stage.world_clone();
        let observer = world
            .window_observer_with_config(state_data.target_key, helpers::default_wait_config());
        let frames = wait_for_expected(stage, &state_data, observer, 2)?;
        let diag_path = record_mimic_diagnostics(stage, state_data.slug, &state_data.mimic)?;
        let artifacts = [diag_path];
        helpers::assert_frame_matches(
            stage.case_name(),
            state_data.expected,
            &frames,
            2,
            &artifacts,
        )?;
        shutdown_mimic(state_data.mimic)?;
        Ok(())
    })?;

    Ok(())
}

/// Verify terminal-style placements remain anchored after rounding to window increments.
pub fn place_term_anchor(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let mut state: Option<PlaceState> = None;
    ctx.setup(|stage| {
        let placeholder = RectPx {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        state = Some(spawn_place_state(
            stage,
            "place.term.anchor",
            placeholder,
            |config, _expected| {
                config.time_ms = 25_000;
                config.label_text = Some("TM".into());
                config.step_size = Some((9.0, 18.0));
            },
        )?);
        if let Some(place) = state.as_mut() {
            let world = stage.world_clone();
            let frames = wait_for_initial_frames(stage.case_name(), &world, place.target_key)?;
            let expected = grid_rect_from_frames(&frames, 3, 1, 0, 0)?;
            rewrite_expected_artifact(stage, place.slug, expected)?;
            place.expected = expected;
        }
        Ok(())
    })?;

    ctx.action(|stage| {
        let place = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("place state missing during action".into()))?;
        let world = stage.world_clone();
        let observer =
            world.window_observer_with_config(place.target_key, helpers::default_wait_config());
        request_grid(&world, place.target_id, (3, 1, 0, 0))?;
        let eps = config::PLACE.eps.round() as i32;
        let frames = wait_for_expected(stage, place, observer, eps)?;
        debug!(
            case = %stage.case_name(),
            frame = ?frames.authoritative,
            "place_term_anchor_settled"
        );
        let world_ref = stage.world_clone();
        ensure_frame_stability(
            stage.case_name(),
            &world_ref,
            place.target_key,
            place.expected,
            eps,
            Duration::from_millis(320),
        )?;
        Ok(())
    })?;

    ctx.settle(|stage| {
        let place = state
            .take()
            .ok_or_else(|| Error::InvalidState("place state missing during settle".into()))?;
        let world = stage.world_clone();
        let target_key = WindowKey {
            pid: place.target_key.pid,
            id: place.target_key.id,
        };
        let world_clone = world;
        let frames = block_on_with_pump(async move { world_clone.frames(target_key).await })?
            .ok_or_else(|| Error::InvalidState("authoritative frame unavailable".into()))?;
        let diag_path = record_mimic_diagnostics(stage, place.slug, &place.mimic)?;
        let artifacts = [diag_path];
        helpers::assert_frame_matches(
            stage.case_name(),
            place.expected,
            &frames,
            config::PLACE.eps.round() as i32,
            &artifacts,
        )?;
        shutdown_mimic(place.mimic)?;
        Ok(())
    })?;

    Ok(())
}

/// Verify placement with resize increments across multiple grid scenarios.
pub fn place_increments_anchor(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let mut state: Option<PlaceState> = None;
    ctx.setup(|stage| {
        let placeholder = RectPx {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        state = Some(spawn_place_state(
            stage,
            "place.increments.anchor",
            placeholder,
            |config, _expected| {
                config.time_ms = 25_000;
                config.label_text = Some("INC".into());
                config.step_size = Some((9.0, 18.0));
            },
        )?);
        Ok(())
    })?;

    ctx.action(|stage| {
        let place = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("place state missing during action".into()))?;
        let world = stage.world_clone();
        let eps = config::PLACE.eps.round() as i32;

        // Scenario A: 2x2 grid bottom-right cell.
        {
            let key = WindowKey {
                pid: place.target_key.pid,
                id: place.target_key.id,
            };
            let world_clone = world.clone();
            let frames = block_on_with_pump(async move { world_clone.frames(key).await })?
                .ok_or_else(|| {
                    Error::InvalidState("frames unavailable before scenario A".into())
                })?;
            let expected = grid_rect_from_frames(&frames, 2, 2, 1, 1)?;
            place.expected = expected;
            rewrite_expected_artifact(stage, place.slug, expected)?;
            let observer =
                world.window_observer_with_config(place.target_key, helpers::default_wait_config());
            request_grid(&world, place.target_id, (2, 2, 1, 1))?;
            let settled = wait_for_expected(stage, place, observer, eps)?;
            debug!(
                case = %stage.case_name(),
                scenario = "2x2.br",
                frame = ?settled.authoritative,
                "place_increments_scenario_a"
            );
            let world_ref = stage.world_clone();
            ensure_frame_stability(
                stage.case_name(),
                &world_ref,
                place.target_key,
                expected,
                eps,
                Duration::from_millis(240),
            )?;
        }

        // Scenario B: 3x1 grid middle cell.
        {
            let key = WindowKey {
                pid: place.target_key.pid,
                id: place.target_key.id,
            };
            let world_clone = world.clone();
            let frames = block_on_with_pump(async move { world_clone.frames(key).await })?
                .ok_or_else(|| {
                    Error::InvalidState("frames unavailable before scenario B".into())
                })?;
            let expected = grid_rect_from_frames(&frames, 3, 1, 1, 0)?;
            place.expected = expected;
            rewrite_expected_artifact(stage, place.slug, expected)?;
            let observer =
                world.window_observer_with_config(place.target_key, helpers::default_wait_config());
            request_grid(&world, place.target_id, (3, 1, 1, 0))?;
            let settled = wait_for_expected(stage, place, observer, eps)?;
            debug!(
                case = %stage.case_name(),
                scenario = "3x1.mid",
                frame = ?settled.authoritative,
                "place_increments_scenario_b"
            );
            let world_ref = stage.world_clone();
            ensure_frame_stability(
                stage.case_name(),
                &world_ref,
                place.target_key,
                expected,
                eps,
                Duration::from_millis(240),
            )?;
        }

        Ok(())
    })?;

    ctx.settle(|stage| {
        let place = state
            .take()
            .ok_or_else(|| Error::InvalidState("place state missing during settle".into()))?;
        let world = stage.world_clone();
        let key = WindowKey {
            pid: place.target_key.pid,
            id: place.target_key.id,
        };
        let world_clone = world;
        let frames = block_on_with_pump(async move { world_clone.frames(key).await })?
            .ok_or_else(|| Error::InvalidState("authoritative frame unavailable".into()))?;
        let diag_path = record_mimic_diagnostics(stage, place.slug, &place.mimic)?;
        let artifacts = [diag_path];
        helpers::assert_frame_matches(
            stage.case_name(),
            place.expected,
            &frames,
            config::PLACE.eps.round() as i32,
            &artifacts,
        )?;
        shutdown_mimic(place.mimic)?;
        Ok(())
    })?;

    Ok(())
}

/// Cycle a helper window through every cell of the configured placement grid.
pub fn place_grid_cycle(ctx: &mut CaseCtx<'_>) -> Result<()> {
    const SLUG: &str = "place.grid.cycle";
    let mut state: Option<PlaceState> = None;
    ctx.setup(|stage| {
        let placeholder = RectPx {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        state = Some(spawn_place_state(stage, SLUG, placeholder, |config, _| {
            config.time_ms = 25_000;
            config.grid = Some((config::PLACE.grid_cols, config::PLACE.grid_rows, 0, 0));
            config.place = PlaceOptions {
                raise: RaiseStrategy::SmartRaise {
                    deadline: Duration::from_millis(
                        config::INPUT_DELAYS
                            .ui_action_delay_ms
                            .saturating_mul(18)
                            .max(1_800),
                    ),
                },
                minimized: MinimizedPolicy::AutoUnminimize,
                animate: false,
            };
        })?);
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("place state missing during action".into()))?;
        let world = stage.world_clone();
        let cols = config::PLACE.grid_cols;
        let rows = config::PLACE.grid_rows;
        let eps = config::PLACE.eps.round() as i32;
        let mut entries = Vec::new();

        for row in 0..rows {
            for col in 0..cols {
                let focus_guard = promote_helper_frontmost(stage.case_name(), state_ref)?;
                ensure_window_on_active_space(
                    stage.case_name(),
                    state_ref,
                    &world,
                    Duration::from_millis(1_400),
                )?;
                let world_for_frames = world.clone();
                let target_key = state_ref.target_key;
                let frames_before = block_on_with_pump(async move {
                    world_for_frames.frames(target_key).await
                })?
                .ok_or_else(|| Error::InvalidState("authoritative frame unavailable".into()))?;
                let expected = grid_rect_from_frames(&frames_before, cols, rows, col, row)?;
                state_ref.expected = expected;
                rewrite_expected_artifact(stage, state_ref.slug, expected)?;
                let observer = world.window_observer_with_config(
                    state_ref.target_key,
                    helpers::default_wait_config(),
                );
                request_grid(&world, state_ref.target_id, (cols, rows, col, row))?;
                let frames = wait_for_expected(stage, state_ref, observer, eps)?;
                let diag_path = record_mimic_diagnostics(stage, state_ref.slug, &state_ref.mimic)?;
                let artifacts = [diag_path.clone()];
                helpers::assert_frame_matches(
                    stage.case_name(),
                    expected,
                    &frames,
                    eps,
                    &artifacts,
                )?;
                focus_guard.reassert()?;
                let delta = expected.delta(&frames.authoritative);
                entries.push(json!({
                    "grid": { "cols": cols, "rows": rows, "col": col, "row": row },
                    "expected": {
                        "x": expected.x,
                        "y": expected.y,
                        "w": expected.w,
                        "h": expected.h,
                    },
                    "actual": {
                        "x": frames.authoritative.x,
                        "y": frames.authoritative.y,
                        "w": frames.authoritative.w,
                        "h": frames.authoritative.h,
                    },
                    "delta": {
                        "dx": delta.dx,
                        "dy": delta.dy,
                        "dw": delta.dw,
                        "dh": delta.dh,
                    },
                    "scale": frames.scale,
                }));
            }
        }

        let cells_path = stage
            .artifacts_dir()
            .join(format!("{}_cells.json", SLUG.replace('.', "_")));
        let payload = json!({ "cells": entries });
        let mut data = serde_json::to_string_pretty(&payload)
            .map_err(|e| Error::InvalidState(format!("failed to serialize cells log: {e}")))?;
        data.push('\n');
        fs::write(&cells_path, data)?;
        stage.record_artifact(&cells_path);
        Ok(())
    })?;

    ctx.settle(|stage| {
        let state_data = state
            .take()
            .ok_or_else(|| Error::InvalidState("place state missing during settle".into()))?;
        let world = stage.world_clone();
        let world_for_final = world;
        let frames =
            block_on_with_pump(async move { world_for_final.frames(state_data.target_key).await })?
                .ok_or_else(|| Error::InvalidState("authoritative frame unavailable".into()))?;
        let diag_path = record_mimic_diagnostics(stage, state_data.slug, &state_data.mimic)?;
        let artifacts = [diag_path];
        helpers::assert_frame_matches(
            stage.case_name(),
            state_data.expected,
            &frames,
            config::PLACE.eps.round() as i32,
            &artifacts,
        )?;
        shutdown_mimic(state_data.mimic)?;
        Ok(())
    })?;

    Ok(())
}

/// Exercise flexible placement with default attempt ordering.
pub fn place_flex_default(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let grid = (config::PLACE.grid_cols, config::PLACE.grid_rows, 0, 0);
    run_flex_case(
        ctx,
        "place.flex.default",
        grid,
        "FLEX",
        |_| {},
        |snapshot| {
            if snapshot.retry_opposite.attempts != 0 || snapshot.fallback_smg.attempts != 0 {
                return Err(Error::InvalidState(format!(
                    "unexpected fallback attempts (retry_opposite={}, fallback_smg={})",
                    snapshot.retry_opposite.attempts, snapshot.fallback_smg.attempts
                )));
            }
            Ok(())
        },
    )
}

/// Force size→pos retry ordering to verify opposite-order attempts are recorded.
pub fn place_flex_force_size_pos(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let grid = (config::PLACE.grid_cols, config::PLACE.grid_rows, 0, 0);
    run_flex_case(
        ctx,
        "place.flex.force_size_pos",
        grid,
        "FSP",
        |opts| {
            *opts = opts
                .clone()
                .with_force_second_attempt(true)
                .with_retry_limits(RetryLimits::new(0, 0, 0, 1));
        },
        |snapshot| {
            if snapshot.retry_opposite.attempts == 0 {
                return Err(Error::InvalidState(
                    "expected opposite-order retry when force_second_attempt=true".into(),
                ));
            }
            if snapshot.fallback_smg.attempts != 0 {
                return Err(Error::InvalidState(format!(
                    "unexpected shrink-move-grow fallback attempts: {}",
                    snapshot.fallback_smg.attempts
                )));
            }
            Ok(())
        },
    )
}

/// Force shrink→move→grow fallback sequencing.
pub fn place_flex_smg(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let grid = (config::PLACE.grid_cols, config::PLACE.grid_rows, 1, 1);
    run_flex_case(
        ctx,
        "place.flex.smg",
        grid,
        "SMG",
        |opts| {
            *opts = opts
                .clone()
                .with_force_second_attempt(true)
                .with_fallback_hook(|invocation| {
                    matches!(
                        invocation.trigger,
                        FallbackTrigger::Forced | FallbackTrigger::Final
                    )
                });
        },
        |snapshot| {
            if snapshot.fallback_smg.attempts == 0 {
                return Err(Error::InvalidState(
                    "expected shrink-move-grow fallback attempt to be recorded".into(),
                ));
            }
            Ok(())
        },
    )
}

/// Verify placement skips when the helper window is non-movable.
pub fn place_skip_nonmovable(ctx: &mut CaseCtx<'_>) -> Result<()> {
    const SLUG: &str = "place.skip.nonmovable";
    const GRID: (u32, u32, u32, u32) = (2, 2, 0, 0);
    let mut state: Option<PlaceState> = None;
    ctx.setup(|stage| {
        let placeholder = RectPx {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        state = Some(spawn_place_state(stage, SLUG, placeholder, |config, _| {
            config.time_ms = 20_000;
            config.panel_nonmovable = true;
            config.attach_sheet = true;
            config.place = PlaceOptions {
                raise: RaiseStrategy::AppActivate,
                minimized: MinimizedPolicy::AutoUnminimize,
                animate: false,
            };
        })?);
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("place state missing during action".into()))?;
        let world = stage.world_clone();
        let frames = wait_for_initial_frames(stage.case_name(), &world, state_ref.target_key)?;
        let initial = frames.authoritative;
        state_ref.expected = initial;
        rewrite_expected_artifact(stage, state_ref.slug, initial)?;
        let focus_guard = promote_helper_frontmost(stage.case_name(), state_ref)?;
        let observer =
            world.window_observer_with_config(state_ref.target_key, helpers::default_wait_config());
        let _drain_guard = MainOpsDrainGuard::disable();
        request_grid(&world, state_ref.target_id, GRID)?;
        mac_winops::drop_pending_main_ops();
        let frames_after =
            wait_for_expected(stage, state_ref, observer, config::PLACE.eps.round() as i32)?;
        mac_winops::drop_pending_main_ops();
        let diag_path = record_mimic_diagnostics(stage, state_ref.slug, &state_ref.mimic)?;
        let artifacts = [diag_path];
        helpers::assert_frame_matches(
            stage.case_name(),
            initial,
            &frames_after,
            config::PLACE.eps.round() as i32,
            &artifacts,
        )?;
        focus_guard.reassert()?;
        let delta = initial.delta(&frames_after.authoritative);
        let comparison_path = stage
            .artifacts_dir()
            .join(format!("{}_comparison.json", SLUG.replace('.', "_")));
        let payload = json!({
            "grid": { "cols": GRID.0, "rows": GRID.1, "col": GRID.2, "row": GRID.3 },
            "expected": {
                "x": initial.x,
                "y": initial.y,
                "w": initial.w,
                "h": initial.h,
            },
            "actual": {
                "x": frames_after.authoritative.x,
                "y": frames_after.authoritative.y,
                "w": frames_after.authoritative.w,
                "h": frames_after.authoritative.h,
            },
            "delta": {
                "dx": delta.dx,
                "dy": delta.dy,
                "dw": delta.dw,
                "dh": delta.dh,
            },
        });
        let mut data = serde_json::to_string_pretty(&payload)
            .map_err(|e| Error::InvalidState(format!("failed to serialize skip report: {e}")))?;
        data.push('\n');
        fs::write(&comparison_path, data)?;
        stage.record_artifact(&comparison_path);
        Ok(())
    })?;

    ctx.settle(|stage| {
        let state_data = state
            .take()
            .ok_or_else(|| Error::InvalidState("place state missing during settle".into()))?;
        let world = stage.world_clone();
        let world_for_final = world;
        let frames =
            block_on_with_pump(async move { world_for_final.frames(state_data.target_key).await })?
                .ok_or_else(|| Error::InvalidState("authoritative frame unavailable".into()))?;
        let diag_path = record_mimic_diagnostics(stage, state_data.slug, &state_data.mimic)?;
        let artifacts = [diag_path];
        helpers::assert_frame_matches(
            stage.case_name(),
            state_data.expected,
            &frames,
            config::PLACE.eps.round() as i32,
            &artifacts,
        )?;
        shutdown_mimic(state_data.mimic)?;
        Ok(())
    })?;

    Ok(())
}

/// Verify grid-relative moves when the helper enforces a taller minimum height.
pub fn place_move_min_anchor(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let mut state: Option<MoveCaseState> = None;
    ctx.setup(|stage| {
        debug!(case = %stage.case_name(), "place_move_min_setup_start");
        for _ in 0..120 {
            pump_active_mimics();
            thread::sleep(Duration::from_millis(5));
        }
        debug!(case = %stage.case_name(), "place_move_min_spawning_state");
        let placeholder = RectPx {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        let mut place =
            spawn_place_state(stage, "place.move.min", placeholder, |config, _expected| {
                config.time_ms = 25_000;
                config.label_text = Some("MIN".into());
                config.grid = Some((4, 4, 0, 0));
                config.min_size = Some((320.0, 380.0));
                config.place.raise = RaiseStrategy::KeepFrontWindow;
            })?;
        let focus_guard = promote_helper_frontmost(stage.case_name(), &place)?;
        let world = stage.world_clone();
        let frames = wait_for_initial_frames(stage.case_name(), &world, place.target_key)?;
        debug!(
            case = %stage.case_name(),
            initial = ?frames.authoritative,
            "place_move_min_after_wait"
        );
        let expected = grid_rect_from_frames(&frames, 4, 4, 1, 0)?;
        debug!(case = %stage.case_name(), expected = ?expected, "place_move_min_setup_ready");
        rewrite_expected_artifact(stage, place.slug, expected)?;
        place.expected = expected;
        focus_guard.reassert()?;
        let focus_guard = promote_helper_frontmost(stage.case_name(), &place)?;
        focus_guard.reassert()?;
        state = Some(MoveCaseState {
            place,
            expected,
            eps: config::PLACE.eps.round() as i32,
            budget: MoveCaseBudget::default(),
        });
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("move-min state missing during action".into()))?;
        let world = stage.world_clone();
        let focus_guard = promote_helper_frontmost(stage.case_name(), &state_ref.place)?;
        request_move(
            &world,
            state_ref.place.target_id,
            (4, 4),
            MoveDirection::Right,
        )?;
        debug!(case = %stage.case_name(), "place_move_min_move_requested");
        focus_guard.reassert()?;
        Ok(())
    })?;

    ctx.settle(|stage| {
        let state_data = state
            .take()
            .ok_or_else(|| Error::InvalidState("move-min state missing during settle".into()))?;
        let world = stage.world_clone();
        let target_key = state_data.place.target_key;
        let expected = state_data.expected;
        let eps = state_data.eps;
        let wait_result =
            wait_for_frame_condition(stage.case_name(), &world, target_key, move |frames| {
                let actual = frames.authoritative;
                let width_ok = (actual.w - expected.w).abs() <= eps;
                let height_ok = actual.h >= expected.h - eps;
                let left_ok = (actual.x - expected.x).abs() <= eps;
                let bottom_ok = (actual.y - expected.y).abs() <= eps;
                width_ok && height_ok && left_ok && bottom_ok
            });
        let diag_path =
            record_mimic_diagnostics(stage, state_data.place.slug, &state_data.place.mimic)?;
        let artifacts = [diag_path];
        match wait_result {
            Ok(frames) => {
                let actual = frames.authoritative;
                debug!(
                    case = %stage.case_name(),
                    actual = ?actual,
                    "place_move_min_frames_observed"
                );
                let eps = state_data.eps;
                let width_ok = (actual.w - state_data.expected.w).abs() <= eps;
                let height_ok = actual.h >= state_data.expected.h - eps;
                let left_ok = (actual.x - state_data.expected.x).abs() <= eps;
                let bottom_ok = (actual.y - state_data.expected.y).abs() <= eps;
                if !(width_ok && height_ok && left_ok && bottom_ok) {
                    let info = "checks=left,bottom,min_width,min_height".to_string();
                    let msg = format_frame_failure(
                        stage.case_name(),
                        state_data.expected,
                        Some(actual),
                        frames.scale,
                        eps,
                        &artifacts,
                        &info,
                    );
                    shutdown_mimic(state_data.place.mimic)?;
                    return Err(Error::InvalidState(msg));
                }
            }
            Err(wait_err) => {
                let fallback =
                    block_on_with_pump(async move { world.clone().frames(target_key).await })?;
                let (actual, scale) = frame_actual_and_scale(fallback);
                let info = format!("checks=left,bottom,min_width,min_height; wait_err={wait_err}");
                let msg = format_frame_failure(
                    stage.case_name(),
                    state_data.expected,
                    actual,
                    scale,
                    state_data.eps,
                    &artifacts,
                    &info,
                );
                shutdown_mimic(state_data.place.mimic)?;
                return Err(Error::InvalidState(msg));
            }
        }
        shutdown_mimic(state_data.place.mimic)?;
        Ok(())
    })?;

    Ok(())
}

/// Verify grid-relative moves when the helper disables resizing entirely.
pub fn place_move_nonresizable_anchor(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let mut state: Option<MoveCaseState> = None;
    ctx.setup(|stage| {
        debug!(case = %stage.case_name(), "place_move_nonres_setup_start");
        let mut budget = MoveCaseBudget::default();
        let warmup_start = Instant::now();
        let warmup_deadline = warmup_start + Duration::from_millis(300);
        while Instant::now() < warmup_deadline {
            pump_active_mimics();
            thread::sleep(Duration::from_millis(10));
        }
        budget.warmup_ms = warmup_start.elapsed().as_millis() as u64;
        debug!(case = %stage.case_name(), "place_move_nonres_spawning_state");
        let placeholder = RectPx {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        let spawn_start = Instant::now();
        let mut place = spawn_place_state(
            stage,
            "place.move.nonresizable",
            placeholder,
            |config, _expected| {
                config.time_ms = 25_000;
                config.label_text = Some("NR".into());
                config.grid = Some((4, 4, 0, 0));
                config.size = Some((1000.0, 700.0));
                config.panel_nonresizable = true;
                config.place.raise = RaiseStrategy::SmartRaise {
                    deadline: Duration::from_millis(600),
                };
            },
        )?;
        budget.spawn_ms = spawn_start.elapsed().as_millis() as u64;
        let focus_guard = promote_helper_frontmost(stage.case_name(), &place)?;
        let world = stage.world_clone();
        let frames_start = Instant::now();
        let frames = wait_for_initial_frames(stage.case_name(), &world, place.target_key)?;
        budget.initial_frames_ms = frames_start.elapsed().as_millis() as u64;
        debug!(
            case = %stage.case_name(),
            initial = ?frames.authoritative,
            "place_move_nonres_after_wait"
        );
        let expected = grid_rect_from_frames(&frames, 4, 4, 1, 0)?;
        debug!(
            case = %stage.case_name(),
            expected = ?expected,
            "place_move_nonres_setup_ready"
        );
        rewrite_expected_artifact(stage, place.slug, expected)?;
        place.expected = expected;
        focus_guard.reassert()?;
        let focus_guard = promote_helper_frontmost(stage.case_name(), &place)?;
        focus_guard.reassert()?;
        state = Some(MoveCaseState {
            place,
            expected,
            eps: config::PLACE.eps.round() as i32,
            budget,
        });
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state.as_mut().ok_or_else(|| {
            Error::InvalidState("move-nonresizable state missing during action".into())
        })?;
        let world = stage.world_clone();
        let raise_start = Instant::now();
        let focus_guard = promote_helper_frontmost(stage.case_name(), &state_ref.place)?;
        state_ref.budget.action_raise_ms = raise_start.elapsed().as_millis() as u64;
        let move_start = Instant::now();
        request_move(
            &world,
            state_ref.place.target_id,
            (4, 4),
            MoveDirection::Right,
        )?;
        state_ref.budget.move_request_ms = move_start.elapsed().as_millis() as u64;
        debug!(case = %stage.case_name(), "place_move_nonres_move_requested");
        focus_guard.reassert()?;
        Ok(())
    })?;

    ctx.settle(|stage| {
        let mut state_data = state.take().ok_or_else(|| {
            Error::InvalidState("move-nonresizable state missing during settle".into())
        })?;
        let world = stage.world_clone();
        let target_key = state_data.place.target_key;
        let settle_start = Instant::now();
        let expected = state_data.expected;
        let eps = state_data.eps;
        let wait_result =
            wait_for_frame_condition(stage.case_name(), &world, target_key, move |frames| {
                let actual = frames.authoritative;
                let left_ok = (actual.x - expected.x).abs() <= eps;
                let bottom_ok = (actual.y - expected.y).abs() <= eps;
                let width_ok = actual.w >= expected.w - eps;
                let height_ok = actual.h >= expected.h - eps;
                left_ok && bottom_ok && width_ok && height_ok
            });
        state_data.budget.settle_wait_ms = settle_start.elapsed().as_millis() as u64;
        let budget_path = write_move_budget(stage, state_data.place.slug, &state_data.budget)?;
        let diag_path =
            record_mimic_diagnostics(stage, state_data.place.slug, &state_data.place.mimic)?;
        let artifacts = [diag_path, budget_path];
        match wait_result {
            Ok(frames) => {
                let actual = frames.authoritative;
                debug!(
                    case = %stage.case_name(),
                    actual = ?actual,
                    "place_move_nonres_frames_observed"
                );
                let eps = state_data.eps;
                let left_ok = (actual.x - state_data.expected.x).abs() <= eps;
                let bottom_ok = (actual.y - state_data.expected.y).abs() <= eps;
                let width_ok = actual.w >= state_data.expected.w - eps;
                let height_ok = actual.h >= state_data.expected.h - eps;
                if !(left_ok && bottom_ok && width_ok && height_ok) {
                    let info = "checks=left,bottom,min_width,min_height".to_string();
                    let msg = format_frame_failure(
                        stage.case_name(),
                        state_data.expected,
                        Some(actual),
                        frames.scale,
                        eps,
                        &artifacts,
                        &info,
                    );
                    shutdown_mimic(state_data.place.mimic)?;
                    return Err(Error::InvalidState(msg));
                }
            }
            Err(wait_err) => {
                let fallback =
                    block_on_with_pump(async move { world.clone().frames(target_key).await })?;
                let (actual, scale) = frame_actual_and_scale(fallback);
                let info = format!("checks=left,bottom,min_width,min_height; wait_err={wait_err}");
                let msg = format_frame_failure(
                    stage.case_name(),
                    state_data.expected,
                    actual,
                    scale,
                    state_data.eps,
                    &artifacts,
                    &info,
                );
                shutdown_mimic(state_data.place.mimic)?;
                return Err(Error::InvalidState(msg));
            }
        }
        shutdown_mimic(state_data.place.mimic)?;
        Ok(())
    })?;

    Ok(())
}

/// Derive a grid-relative rectangle using the helper's current display.
fn grid_rect_from_frames(
    frames: &Frames,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
) -> Result<RectPx> {
    let center_x = frames.authoritative.x + frames.authoritative.w / 2;
    let center_y = frames.authoritative.y + frames.authoritative.h / 2;
    let vf = screen::visible_frame_containing_point(f64::from(center_x), f64::from(center_y))
        .ok_or_else(|| Error::InvalidState("visible frame not resolved".into()))?;
    let rect = mac_winops::cell_rect(vf, cols, rows, col, row);
    Ok(RectPx::from_ax(&rect))
}

/// Wait until the provided predicate returns true for the window's frames.
fn wait_for_frame_condition<F>(
    case: &str,
    world: &WorldHandle,
    key: WindowKey,
    mut predicate: F,
) -> Result<Frames>
where
    F: FnMut(&Frames) -> bool + Send + 'static,
{
    let config = helpers::default_wait_config();
    let world_clone = world.clone();
    let wait_result = block_on_with_pump(async move {
        let mut observer = world_clone.window_observer_with_config(key, config);
        observer
            .wait_for_frames("frame-condition", move |frames| predicate(frames))
            .await
    })?;
    wait_result.map_err(|err| helpers::wait_failure(case, &err))
}

/// Ensure the authoritative frame remains within `eps` for the supplied duration.
fn ensure_frame_stability(
    case: &str,
    world: &WorldHandle,
    key: WindowKey,
    expected: RectPx,
    eps: i32,
    duration: Duration,
) -> Result<()> {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        pump_active_mimics();
        thread::sleep(Duration::from_millis(40));
        let world_clone = world.clone();
        let frames = block_on_with_pump(async move { world_clone.frames(key).await })?
            .ok_or_else(|| Error::InvalidState("authoritative frame unavailable".into()))?;
        let delta = expected.delta(&frames.authoritative);
        if delta.dx.abs() > eps
            || delta.dy.abs() > eps
            || delta.dw.abs() > eps
            || delta.dh.abs() > eps
        {
            let info = format!("check=stability duration_ms={}", duration.as_millis());
            let message = format_frame_failure(
                case,
                expected,
                Some(frames.authoritative),
                frames.scale,
                eps,
                &[],
                &info,
            );
            return Err(Error::InvalidState(message));
        }
    }
    Ok(())
}

/// Extract the authoritative rectangle and scale factor from optional frames.
fn frame_actual_and_scale(frames: Option<Frames>) -> (Option<RectPx>, f32) {
    match frames {
        Some(frames) => (Some(frames.authoritative), frames.scale),
        None => (None, 1.0),
    }
}

/// Render a rectangle using the `<x,y,w,h>` canonical format.
fn format_rect_px(rect: RectPx) -> String {
    format!("<{},{},{},{}>", rect.x, rect.y, rect.w, rect.h)
}

/// Render a rectangle delta using the `<dx,dy,dw,dh>` canonical format.
fn format_delta_px(delta: RectDelta) -> String {
    format!("<{},{},{},{}>", delta.dx, delta.dy, delta.dw, delta.dh)
}

/// Render artifact paths as a comma-separated list.
fn format_artifacts(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        "-".to_string()
    } else {
        paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Construct the standard failure message used by placement cases.
fn format_frame_failure(
    case: &str,
    expected: RectPx,
    actual: Option<RectPx>,
    scale: f32,
    eps: i32,
    artifacts: &[PathBuf],
    info: &str,
) -> String {
    let expected_str = format_rect_px(expected);
    let (actual_str, delta_str) = match actual {
        Some(actual_rect) => (
            format_rect_px(actual_rect),
            format_delta_px(expected.delta(&actual_rect)),
        ),
        None => (
            "<n/a,n/a,n/a,n/a>".to_string(),
            "<n/a,n/a,n/a,n/a>".to_string(),
        ),
    };
    format!(
        "case=<{}> scale=<{:.2}> eps=<{}> expected={} got={} delta={} info={} artifacts={}",
        case,
        scale,
        eps,
        expected_str,
        actual_str,
        delta_str,
        info,
        format_artifacts(artifacts),
    )
}

/// Spawn a mimic helper and locate its world identifiers for subsequent stages.
fn spawn_place_state<F>(
    stage: &mut StageHandle<'_>,
    slug: &'static str,
    expected: RectPx,
    configure: F,
) -> Result<PlaceState>
where
    F: FnOnce(&mut HelperConfig, RectPx),
{
    let world = stage.world_clone();
    let slug_arc: Arc<str> = Arc::from(slug);
    let label_arc: Arc<str> = Arc::from("primary");
    let mut config = HelperConfig {
        scenario_slug: slug_arc.clone(),
        window_label: label_arc.clone(),
        ..HelperConfig::default()
    };
    configure(&mut config, expected);
    let raise_strategy = config.place.raise;
    let place_options = config.place;

    let spec = MimicSpec::new(slug_arc.clone(), label_arc, "Primary")
        .with_place(place_options)
        .with_config(config);
    let scenario = MimicScenario::new(slug_arc, vec![spec]);

    let mimic = spawn_mimic(scenario)
        .map_err(|e| Error::InvalidState(format!("spawn mimic failed for {}: {e}", slug)))?;
    pump_active_mimics();
    debug!(case = slug, "spawn_place_state_mimic_spawned");

    let marker = format!("[{slug}::primary]");
    let wait_result = block_on_with_pump({
        let world = world.clone();
        let marker = marker.clone();
        async move {
            world
                .await_window_where_with_config(
                    "spawn_place_state",
                    move |win| win.title.contains(&marker),
                    helpers::default_wait_config(),
                )
                .await
        }
    })?;
    let mut target = match wait_result {
        Ok(window) => window,
        Err(err) => return Err(helpers::wait_failure(slug, &err)),
    };
    target = ensure_window_ready(&world, &marker, target)?;
    let title = target.title.clone();

    let initial_raise_deadline = Duration::from_millis(
        config::INPUT_DELAYS
            .ui_action_delay_ms
            .saturating_mul(12)
            .max(1200),
    );
    if let Err(err) = world::smart_raise(target.world_id(), &title, initial_raise_deadline) {
        debug!(case = slug, error = %err, "spawn_place_state_initial_raise_failed");
    }

    let cg_pid = match world::list_windows() {
        Ok(windows) => windows
            .into_iter()
            .find(|win| win.id == target.id)
            .map(|win| win.pid),
        Err(err) => {
            debug!(case = slug, error = %err, "spawn_place_state_world_snapshot_failed");
            None
        }
    };
    let mut resolved_pid = target.pid;
    if let Some(cg_pid) = cg_pid {
        if cg_pid != target.pid {
            debug!(
                case = slug,
                world_pid = target.pid,
                cg_pid,
                id = target.id,
                "spawn_place_state_pid_adjust"
            );
            resolved_pid = cg_pid;
        }
    } else {
        debug!(
            case = slug,
            id = target.id,
            "spawn_place_state_pid_missing_in_cg"
        );
    }

    debug!(
        case = slug,
        pid = resolved_pid,
        world_pid = target.pid,
        id = target.id,
        "spawn_place_state_target_observed"
    );

    let target_id = WorldWindowId::new(resolved_pid, target.id);
    let target_key = WindowKey {
        pid: resolved_pid,
        id: target.id,
    };

    if let Err(err) = mac_winops::request_activate_pid(resolved_pid) {
        debug!(pid = resolved_pid, ?err, "activate pid request failed");
    }

    let expected_path = stage
        .artifacts_dir()
        .join(format!("{}_expected.txt", slug.replace('.', "_")));
    fs::write(&expected_path, format!("expected={:?}\n", expected))?;
    stage.record_artifact(&expected_path);

    let warmup_config = WaitConfig::new(
        Duration::from_millis(1_200),
        Duration::from_millis(config::INPUT_DELAYS.poll_interval_ms.max(5)),
        128,
    );
    let warmup_result = block_on_with_pump({
        let world_clone = world;
        async move {
            let mut observer = world_clone.window_observer_with_config(target_key, warmup_config);
            observer
                .wait_for_visibility(VisibilityPolicy::OnScreen)
                .await
        }
    })?;
    if let Err(err) = warmup_result {
        debug!(
            pid = resolved_pid,
            id = target.id,
            error = %err,
            "spawn_place_state_visibility_wait_failed"
        );
    }

    Ok(PlaceState {
        mimic,
        target_id,
        target_key,
        expected,
        slug,
        title,
        raise: raise_strategy,
    })
}

/// Best-effort ensure the helper window is frontmost before issuing commands.
fn promote_helper_frontmost(case: &str, state: &PlaceState) -> Result<FocusGuard> {
    debug!(case = %case, strategy = ?state.raise, "promote_helper_frontmost_start");
    match state.raise {
        RaiseStrategy::SmartRaise { deadline } => {
            if let Err(err) = world::smart_raise(state.target_id, &state.title, deadline) {
                debug!(case = %case, error = %err, "smart_raise_failed");
            }
        }
        RaiseStrategy::AppActivate => {
            let pid = state.target_id.pid();
            if let Err(err) = world::ensure_frontmost(
                pid,
                &state.title,
                6,
                config::INPUT_DELAYS.ui_action_delay_ms,
            ) {
                debug!(case = %case, error = %err, "ensure_frontmost_failed");
            }
            let deadline = Duration::from_millis(
                config::INPUT_DELAYS
                    .ui_action_delay_ms
                    .saturating_mul(10)
                    .max(400),
            );
            match world::smart_raise(state.target_id, &state.title, deadline) {
                Ok(()) => debug!(case = %case, "smart_raise_followup_ok"),
                Err(err) => debug!(case = %case, error = %err, "smart_raise_followup_failed"),
            }
        }
        _ => {}
    }

    FocusGuard::acquire(state.target_id.pid(), &state.title, Some(state.target_id))
}

/// Wait for the helper window to report on-screen and on the active space.
fn ensure_window_on_active_space(
    case: &str,
    state: &PlaceState,
    world: &WorldHandle,
    timeout: Duration,
) -> Result<()> {
    let idle = Duration::from_millis(config::INPUT_DELAYS.poll_interval_ms.max(5));
    let wait_config = WaitConfig::new(timeout, idle, 512);
    let target_key = state.target_key;
    let wait_result = block_on_with_pump({
        let world_clone = world.clone();
        async move {
            let mut observer = world_clone.window_observer_with_config(target_key, wait_config);
            observer
                .wait_for_visibility(VisibilityPolicy::OnScreenAndActive)
                .await
        }
    })?;
    match wait_result {
        Ok(window) => {
            debug!(
                case = %case,
                pid = window.pid,
                id = window.id,
                "ensure_window_on_active_space_ready"
            );
            Ok(())
        }
        Err(err) => Err(helpers::wait_failure(case, &err)),
    }
}

/// Wait until the world reports authoritative frames for the supplied key.
fn wait_for_initial_frames(case: &str, world: &WorldHandle, key: WindowKey) -> Result<Frames> {
    let config = helpers::default_wait_config();
    let world_clone = world.clone();
    let wait_result = block_on_with_pump(async move {
        let mut observer = world_clone.window_observer_with_config(key, config);
        observer.wait_for_frames("initial-frames", |_| true).await
    })?;
    wait_result.map_err(|err| helpers::wait_failure(case, &err))
}

/// Update the persisted expected rectangle artifact for a scenario.
fn rewrite_expected_artifact(
    stage: &mut StageHandle<'_>,
    slug: &str,
    expected: RectPx,
) -> Result<()> {
    let path = stage
        .artifacts_dir()
        .join(format!("{}_expected.txt", slug.replace('.', "_")));
    fs::write(&path, format!("expected={expected:?}\n"))?;
    stage.record_artifact(&path);
    Ok(())
}

/// Persist per-phase timings for the nonresizable move case.
fn write_move_budget(
    stage: &mut StageHandle<'_>,
    slug: &str,
    budget: &MoveCaseBudget,
) -> Result<PathBuf> {
    let path = stage
        .artifacts_dir()
        .join(format!("{}_phases.json", slug.replace('.', "_")));
    let payload = json!({
        "setup": {
            "warmup_ms": budget.warmup_ms,
            "spawn_ms": budget.spawn_ms,
            "initial_frames_ms": budget.initial_frames_ms,
        },
        "action": {
            "raise_ms": budget.action_raise_ms,
            "move_request_ms": budget.move_request_ms,
        },
        "settle": {
            "wait_ms": budget.settle_wait_ms,
        },
    });
    let mut data = serde_json::to_string_pretty(&payload)
        .map_err(|e| Error::InvalidState(format!("failed to serialize move budget: {}", e)))?;
    data.push('\n');
    fs::write(&path, data)?;
    stage.record_artifact(&path);
    Ok(path)
}

/// Persist placement counter snapshots for diagnostics.
fn record_counters_artifact(
    stage: &mut StageHandle<'_>,
    slug: &str,
    snapshot: &PlacementCountersSnapshot,
) -> Result<PathBuf> {
    let path = stage
        .artifacts_dir()
        .join(format!("{}_counters.json", slug.replace('.', "_")));
    let payload = json!({
        "primary": {
            "attempts": snapshot.primary.attempts,
            "verified": snapshot.primary.verified,
            "settle_ms_total": snapshot.primary.settle_ms_total,
        },
        "axis_nudge": {
            "attempts": snapshot.axis_nudge.attempts,
            "verified": snapshot.axis_nudge.verified,
            "settle_ms_total": snapshot.axis_nudge.settle_ms_total,
        },
        "retry_opposite": {
            "attempts": snapshot.retry_opposite.attempts,
            "verified": snapshot.retry_opposite.verified,
            "settle_ms_total": snapshot.retry_opposite.settle_ms_total,
        },
        "size_only": {
            "attempts": snapshot.size_only.attempts,
            "verified": snapshot.size_only.verified,
            "settle_ms_total": snapshot.size_only.settle_ms_total,
        },
        "anchor_size_only": {
            "attempts": snapshot.anchor_size_only.attempts,
            "verified": snapshot.anchor_size_only.verified,
            "settle_ms_total": snapshot.anchor_size_only.settle_ms_total,
        },
        "anchor_legal": {
            "attempts": snapshot.anchor_legal.attempts,
            "verified": snapshot.anchor_legal.verified,
            "settle_ms_total": snapshot.anchor_legal.settle_ms_total,
        },
        "fallback_smg": {
            "attempts": snapshot.fallback_smg.attempts,
            "verified": snapshot.fallback_smg.verified,
            "settle_ms_total": snapshot.fallback_smg.settle_ms_total,
        },
        "safe_park": snapshot.safe_park,
        "failures": snapshot.failures,
    });
    let mut data = serde_json::to_string_pretty(&payload)
        .map_err(|e| Error::InvalidState(format!("failed to serialize placement counters: {e}")))?;
    data.push('\n');
    fs::write(&path, data)?;
    stage.record_artifact(&path);
    Ok(path)
}

/// Shared implementation for flexible placement cases.
fn run_flex_case<F, V>(
    ctx: &mut CaseCtx<'_>,
    slug: &'static str,
    grid: (u32, u32, u32, u32),
    label: &str,
    configure_options: F,
    verify_snapshot: V,
) -> Result<()>
where
    F: FnOnce(&mut PlaceAttemptOptions),
    V: Fn(&PlacementCountersSnapshot) -> Result<()>,
{
    let mut state: Option<PlaceState> = None;
    ctx.setup(|stage| {
        mac_winops::placement_counters_reset();
        let placeholder = RectPx {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        state = Some(spawn_place_state(stage, slug, placeholder, |config, _| {
            config.time_ms = 25_000;
            config.label_text = Some(label.into());
        })?);
        if let Some(place) = state.as_mut() {
            let world = stage.world_clone();
            let frames = wait_for_initial_frames(stage.case_name(), &world, place.target_key)?;
            let expected = grid_rect_from_frames(&frames, grid.0, grid.1, grid.2, grid.3)?;
            rewrite_expected_artifact(stage, place.slug, expected)?;
            place.expected = expected;
        }
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("place state missing during action".into()))?;
        let world = stage.world_clone();
        let focus_guard = promote_helper_frontmost(stage.case_name(), state_ref)?;
        let mut options = PlaceAttemptOptions::default();
        configure_options(&mut options);
        request_grid_with_options(&world, state_ref.target_id, grid, Some(&options))?;
        focus_guard.reassert()?;
        Ok(())
    })?;

    ctx.settle(|stage| {
        let state_data = state
            .take()
            .ok_or_else(|| Error::InvalidState("place state missing during settle".into()))?;
        let world = stage.world_clone();
        let observer = world
            .window_observer_with_config(state_data.target_key, helpers::default_wait_config());
        let frames = wait_for_expected(
            stage,
            &state_data,
            observer,
            config::PLACE.eps.round() as i32,
        )?;
        let snapshot = mac_winops::placement_counters_snapshot();
        if snapshot.primary.attempts == 0 {
            return Err(Error::InvalidState(
                "expected at least one primary placement attempt".into(),
            ));
        }
        let counters_path = record_counters_artifact(stage, state_data.slug, &snapshot)?;
        verify_snapshot(&snapshot)?;
        let diag_path = record_mimic_diagnostics(stage, state_data.slug, &state_data.mimic)?;
        let artifacts = [diag_path, counters_path];
        helpers::assert_frame_matches(
            stage.case_name(),
            state_data.expected,
            &frames,
            config::PLACE.eps.round() as i32,
            &artifacts,
        )?;
        shutdown_mimic(state_data.mimic)?;
        Ok(())
    })?;

    Ok(())
}

/// Issue a world placement command with optional attempt configuration.
fn request_grid_with_options(
    world: &WorldHandle,
    target: WorldWindowId,
    grid: (u32, u32, u32, u32),
    options: Option<&PlaceAttemptOptions>,
) -> Result<()> {
    let (cols, rows, col, row) = grid;
    let mut attempts = 0;
    let target_key = WindowKey {
        pid: target.pid(),
        id: target.window_id(),
    };
    let wait_idle = Duration::from_millis(config::INPUT_DELAYS.poll_interval_ms.max(5));
    let wait_config = WaitConfig::new(
        Duration::from_millis(config::DEFAULTS.timeout_ms),
        wait_idle,
        512,
    );
    loop {
        let request_world = world.clone();
        let opts = options.cloned();
        let receipt_result = block_on_with_pump(async move {
            request_world
                .request_place_for_window(target, cols, rows, col, row, opts)
                .await
        })?;
        match receipt_result {
            Ok(receipt) => {
                receipt.target.ok_or_else(|| {
                    Error::InvalidState("placement did not select a target".into())
                })?;
                return Ok(());
            }
            Err(err) => {
                let message = err.to_string();
                if attempts < 3 && message.contains("mode=") {
                    attempts += 1;
                    let wait_result = block_on_with_pump({
                        let world_clone = world.clone();
                        async move {
                            let mut observer =
                                world_clone.window_observer_with_config(target_key, wait_config);
                            observer.wait_for_mode(WindowMode::Normal).await
                        }
                    })?;
                    match wait_result {
                        Ok(_) => {
                            debug!(
                                pid = target.pid(),
                                id = target.window_id(),
                                attempts,
                                "request_grid_retry_after_mode_wait"
                            );
                            continue;
                        }
                        Err(wait_err) => {
                            return Err(Error::InvalidState(format!(
                                "placement request failed: {message}; wait_error={wait_err}"
                            )));
                        }
                    }
                }
                return Err(Error::InvalidState(format!(
                    "placement request failed: {message}"
                )));
            }
        }
    }
}

/// Issue a world placement command for the supplied window and grid cell.
fn request_grid(
    world: &WorldHandle,
    target: WorldWindowId,
    grid: (u32, u32, u32, u32),
) -> Result<()> {
    request_grid_with_options(world, target, grid, None)
}

/// Issue a move command for the supplied window across the placement grid.
fn request_move(
    world: &WorldHandle,
    target: WorldWindowId,
    grid: (u32, u32),
    dir: MoveDirection,
) -> Result<()> {
    let (cols, rows) = grid;
    let mut attempts = 0;
    loop {
        let world_clone = world.clone();
        let receipt_result = block_on_with_pump(async move {
            world_clone
                .request_place_move_for_window(target, cols, rows, dir, None)
                .await
        })?;
        match receipt_result {
            Ok(receipt) => {
                receipt.target.ok_or_else(|| {
                    Error::InvalidState("placement move did not select a target".into())
                })?;
                return Ok(());
            }
            Err(err) => {
                let message = err.to_string();
                if attempts < 4 && message.contains("mode=") {
                    attempts += 1;
                    pump_active_mimics();
                    thread::sleep(Duration::from_millis(80));
                    continue;
                }
                return Err(Error::InvalidState(format!(
                    "move request failed: {message}"
                )));
            }
        }
    }
}

/// Target rectangle used when exercising the fake placement adapter.
const FAKE_TARGET: Rect = Rect {
    x: 100.0,
    y: 200.0,
    w: 640.0,
    h: 480.0,
};

/// Visible frame used when constructing fake placement scenarios.
const FAKE_VISIBLE: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 1_440.0,
    h: 900.0,
};

/// Execute the set of fake adapter scenarios and capture their observed operations.
fn run_fake_adapter_scenarios() -> Result<Vec<(&'static str, Vec<FakeOp>)>> {
    let mut results = Vec::new();

    let ops = run_fake_flow(
        "place_grid_focused",
        FakeWindowConfig::default(),
        PlaceAttemptOptions::default(),
        |ops| ensure_fake_op(ops, |op| matches!(op, FakeOp::Apply { .. }), "apply"),
    )?;
    results.push(("place_grid_focused", ops));

    let mut axis_cfg = FakeWindowConfig::default();
    axis_cfg
        .apply_script
        .push(FakeApplyResponse::new(Rect::new(100.0, 210.0, 640.0, 480.0)).with_persist(true));
    axis_cfg.nudge_script.push(FakeApplyResponse::new(Rect::new(
        100.0, 200.0, 640.0, 480.0,
    )));
    let ops = run_fake_flow(
        "place_grid",
        axis_cfg,
        PlaceAttemptOptions::default(),
        |ops| ensure_fake_op(ops, |op| matches!(op, FakeOp::Nudge { .. }), "nudge"),
    )?;
    results.push(("place_grid", ops));

    let mut fallback_cfg = FakeWindowConfig::default();
    fallback_cfg
        .apply_script
        .push(FakeApplyResponse::new(Rect::new(320.0, 420.0, 640.0, 480.0)).with_persist(true));
    fallback_cfg
        .fallback_script
        .push(FakeApplyResponse::new(FAKE_TARGET));
    let fallback_opts =
        PlaceAttemptOptions::default().with_retry_limits(RetryLimits::new(0, 0, 0, 1));
    let ops = run_fake_flow("place_move_grid", fallback_cfg, fallback_opts, |ops| {
        ensure_fake_op(ops, |op| matches!(op, FakeOp::Fallback { .. }), "fallback")
    })?;
    results.push(("place_move_grid", ops));

    Ok(results)
}

/// Drive a fake placement flow and return the recorded fake operations.
fn run_fake_flow<F>(
    label: &'static str,
    config: FakeWindowConfig,
    opts: PlaceAttemptOptions,
    verify_ops: F,
) -> Result<Vec<FakeOp>>
where
    F: Fn(&[FakeOp]) -> Result<()>,
{
    let fake = Arc::new(FakeAxAdapter::new());
    let win = fake.new_window(config);
    let adapter_handle: AxAdapterHandle = fake.clone() as AxAdapterHandle;
    let ctx = PlacementContext::with_adapter(
        win.clone(),
        FAKE_TARGET,
        FAKE_VISIBLE,
        opts,
        adapter_handle,
    );
    let engine = PlacementEngine::new(
        &ctx,
        PlacementEngineConfig {
            label,
            attr_pos: mac_winops::cfstr("AXPosition"),
            attr_size: mac_winops::cfstr("AXSize"),
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
        .map_err(|e| Error::InvalidState(format!("{label}: engine error {e}")))?;
    match outcome {
        PlacementOutcome::Verified(success) => {
            if success.final_rect != FAKE_TARGET {
                return Err(Error::InvalidState(format!(
                    "{label}: expected {:?} got {:?}",
                    FAKE_TARGET, success.final_rect
                )));
            }
        }
        other => {
            return Err(Error::InvalidState(format!(
                "{label}: unexpected outcome {other:?}"
            )));
        }
    }
    let ops = fake.operations(&win);
    verify_ops(&ops)?;
    Ok(ops)
}

/// Ensure an expected fake adapter operation was recorded.
fn ensure_fake_op<F>(ops: &[FakeOp], predicate: F, label: &str) -> Result<()>
where
    F: Fn(&FakeOp) -> bool,
{
    if ops.iter().any(predicate) {
        Ok(())
    } else {
        Err(Error::InvalidState(format!(
            "expected {label} operation to be recorded (got {ops:?})"
        )))
    }
}

/// Issue a world move command for the supplied window and grid configuration.
/// Wait until world reports the expected authoritative frame for the helper window.
fn wait_for_expected(
    stage: &StageHandle<'_>,
    state: &PlaceState,
    observer: WindowObserver,
    eps: i32,
) -> Result<hotki_world::Frames> {
    let expected = state.expected;
    let wait_result = block_on_with_pump(async move {
        let mut observer = observer;
        observer
            .wait_for_frames("placement-frame", move |frames| {
                rect_matches(frames.authoritative, expected, eps)
            })
            .await
    })?;
    match wait_result {
        Ok(frames) => Ok(frames),
        Err(err) => Err(helpers::wait_failure(stage.case_name(), &err)),
    }
}

/// Convert an integer rectangle into a float tuple consumed by helper configuration.
fn rect_to_f64(rect: RectPx) -> (f64, f64, f64, f64) {
    (
        f64::from(rect.x),
        f64::from(rect.y),
        f64::from(rect.w),
        f64::from(rect.h),
    )
}

/// Return `true` when two integer rectangles match within the supplied epsilon.
fn rect_matches(actual: RectPx, expected: RectPx, eps: i32) -> bool {
    let delta = expected.delta(&actual);
    delta.dx.abs() <= eps && delta.dy.abs() <= eps && delta.dw.abs() <= eps && delta.dh.abs() <= eps
}
