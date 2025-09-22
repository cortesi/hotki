//! Placement smoketest cases implemented against the mimic harness.
use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use hotki_world::{
    EventCursor, Frames, MinimizedPolicy, MoveDirection, PlaceOptions, RaiseStrategy, RectDelta,
    RectPx, WindowKey, WorldHandle,
    mimic::{HelperConfig, MimicHandle, MimicScenario, MimicSpec, pump_active_mimics, spawn_mimic},
};
use hotki_world_ids::WorldWindowId;
use mac_winops::{self, screen};
use serde_json::json;
use tracing::debug;

use super::support::{block_on_with_pump, record_mimic_diagnostics, shutdown_mimic};
use crate::{
    config,
    error::{Error, Result},
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
    /// Event cursor that enforces ordering and lost-count checks.
    cursor: EventCursor,
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
    /// Maximum duration permitted for the wait loop.
    timeout: Duration,
    /// Timing breakdown captured across stages.
    budget: MoveCaseBudget,
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
        let mut state_data = state
            .take()
            .ok_or_else(|| Error::InvalidState("place state missing during settle".into()))?;
        let frames = wait_for_expected(stage, &mut state_data, Duration::from_millis(2_000), 2)?;
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
        if let Some(place) = state.as_mut() {
            let world = stage.world_clone();
            refresh_cursor(place, &world)?;
        }
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("place state missing during action".into()))?;
        let world = stage.world_clone();
        refresh_cursor(state_ref, &world)?;
        request_grid(&world, state_ref.target_id, (3, 2, 2, 0))?;
        Ok(())
    })?;

    ctx.settle(|stage| {
        let mut state_data = state
            .take()
            .ok_or_else(|| Error::InvalidState("place state missing during settle".into()))?;
        let frames = wait_for_expected(stage, &mut state_data, Duration::from_millis(3_000), 2)?;
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
    let mut state: Option<PlaceState> = None;
    ctx.setup(|stage| {
        let expected = RectPx {
            x: 200,
            y: 180,
            w: 500,
            h: 300,
        };
        state = Some(spawn_place_state(
            stage,
            "place.async.delay",
            expected,
            |config, expected| {
                config.delay_apply_ms = 220;
                config.apply_target = Some(rect_to_f64(expected));
                config.place = PlaceOptions {
                    raise: RaiseStrategy::AppActivate,
                    minimized: MinimizedPolicy::DeferUntilUnminimized,
                    animate: false,
                };
                config.time_ms = 25_000;
            },
        )?);
        if let Some(place_state) = state.as_mut() {
            let world = stage.world_clone();
            refresh_cursor(place_state, &world)?;
        }
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("place state missing during action".into()))?;
        let world = stage.world_clone();
        refresh_cursor(state_ref, &world)?;
        request_grid(&world, state_ref.target_id, (3, 2, 0, 1))?;
        Ok(())
    })?;

    ctx.settle(|stage| {
        let mut state_data = state
            .take()
            .ok_or_else(|| Error::InvalidState("place state missing during settle".into()))?;
        let frames = wait_for_expected(stage, &mut state_data, Duration::from_millis(3_500), 2)?;
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
        promote_helper_frontmost(stage.case_name(), &place);
        let world = stage.world_clone();
        refresh_cursor(&mut place, &world)?;
        let frames = wait_for_initial_frames(
            stage.case_name(),
            &world,
            &mut place.cursor,
            place.target_key,
            Duration::from_millis(config::PLACE.step_timeout_ms),
        )?;
        debug!(
            case = %stage.case_name(),
            initial = ?frames.authoritative,
            "place_move_min_after_wait"
        );
        let expected = grid_rect_from_frames(&frames, 4, 4, 1, 0)?;
        debug!(case = %stage.case_name(), expected = ?expected, "place_move_min_setup_ready");
        rewrite_expected_artifact(stage, place.slug, expected)?;
        place.expected = expected;
        promote_helper_frontmost(stage.case_name(), &place);
        refresh_cursor(&mut place, &world)?;
        state = Some(MoveCaseState {
            place,
            expected,
            eps: config::PLACE.eps.round() as i32,
            timeout: Duration::from_millis(config::PLACE.step_timeout_ms),
            budget: MoveCaseBudget::default(),
        });
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("move-min state missing during action".into()))?;
        let world = stage.world_clone();
        promote_helper_frontmost(stage.case_name(), &state_ref.place);
        request_move(
            &world,
            state_ref.place.target_id,
            (4, 4),
            MoveDirection::Right,
        )?;
        refresh_cursor(&mut state_ref.place, &world)?;
        debug!(case = %stage.case_name(), "place_move_min_move_requested");
        Ok(())
    })?;

    ctx.settle(|stage| {
        let mut state_data = state
            .take()
            .ok_or_else(|| Error::InvalidState("move-min state missing during settle".into()))?;
        let world = stage.world_clone();
        let target_key = state_data.place.target_key;
        let wait_result = wait_for_frame_condition(
            stage.case_name(),
            &world,
            &mut state_data.place.cursor,
            target_key,
            state_data.timeout,
            |frames| {
                let actual = frames.authoritative;
                let expected = state_data.expected;
                let eps = state_data.eps;
                let width_ok = (actual.w - expected.w).abs() <= eps;
                let height_ok = actual.h >= expected.h - eps;
                let left_ok = (actual.x - expected.x).abs() <= eps;
                let bottom_ok = (actual.y - expected.y).abs() <= eps;
                Ok(width_ok && height_ok && left_ok && bottom_ok)
            },
        );
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
        promote_helper_frontmost(stage.case_name(), &place);
        let world = stage.world_clone();
        refresh_cursor(&mut place, &world)?;
        let frames_start = Instant::now();
        let frames = wait_for_initial_frames(
            stage.case_name(),
            &world,
            &mut place.cursor,
            place.target_key,
            Duration::from_millis(1_500),
        )?;
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
        promote_helper_frontmost(stage.case_name(), &place);
        refresh_cursor(&mut place, &world)?;
        state = Some(MoveCaseState {
            place,
            expected,
            eps: config::PLACE.eps.round() as i32,
            timeout: Duration::from_millis(2_400),
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
        promote_helper_frontmost(stage.case_name(), &state_ref.place);
        state_ref.budget.action_raise_ms = raise_start.elapsed().as_millis() as u64;
        let move_start = Instant::now();
        request_move(
            &world,
            state_ref.place.target_id,
            (4, 4),
            MoveDirection::Right,
        )?;
        refresh_cursor(&mut state_ref.place, &world)?;
        state_ref.budget.move_request_ms = move_start.elapsed().as_millis() as u64;
        debug!(case = %stage.case_name(), "place_move_nonres_move_requested");
        Ok(())
    })?;

    ctx.settle(|stage| {
        let mut state_data = state.take().ok_or_else(|| {
            Error::InvalidState("move-nonresizable state missing during settle".into())
        })?;
        let world = stage.world_clone();
        let target_key = state_data.place.target_key;
        let settle_start = Instant::now();
        let wait_result = wait_for_frame_condition(
            stage.case_name(),
            &world,
            &mut state_data.place.cursor,
            target_key,
            state_data.timeout,
            |frames| {
                let actual = frames.authoritative;
                let expected = state_data.expected;
                let eps = state_data.eps;
                let left_ok = (actual.x - expected.x).abs() <= eps;
                let bottom_ok = (actual.y - expected.y).abs() <= eps;
                let width_ok = actual.w >= expected.w - eps;
                let height_ok = actual.h >= expected.h - eps;
                Ok(left_ok && bottom_ok && width_ok && height_ok)
            },
        );
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
    cursor: &mut EventCursor,
    key: WindowKey,
    timeout: Duration,
    mut predicate: F,
) -> Result<Frames>
where
    F: FnMut(&Frames) -> Result<bool>,
{
    helpers::wait_for_events_or(case, world, cursor, timeout, || {
        let world_clone = world.clone();
        let frames = block_on_with_pump(async move { world_clone.frames(key).await })?;
        match frames {
            Some(ref frames) => predicate(frames),
            None => Ok(false),
        }
    })?;
    let world_clone = world.clone();
    block_on_with_pump(async move { world_clone.frames(key).await })?
        .ok_or_else(|| Error::InvalidState("authoritative frame unavailable".into()))
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

    let world_for_subscribe = world.clone();
    let (cursor, snapshot, _) =
        block_on_with_pump(async move { world_for_subscribe.subscribe_with_snapshot().await })?;
    debug!(case = slug, "spawn_place_state_subscribed");
    let marker = format!("[{slug}::primary]");
    let mut target = snapshot.into_iter().find(|win| win.title.contains(&marker));
    if target.is_none() {
        for _ in 0..20 {
            pump_active_mimics();
            thread::sleep(Duration::from_millis(5));
            let world_for_snapshot = world.clone();
            let refreshed = block_on_with_pump(async move { world_for_snapshot.snapshot().await })?;
            target = refreshed
                .into_iter()
                .find(|win| win.title.contains(&marker));
            if target.is_some() {
                debug!(case = slug, "spawn_place_state_target_resolved_retry");
                break;
            }
        }
    }
    let mut target =
        target.ok_or_else(|| Error::InvalidState(format!("mimic window {} not observed", slug)))?;
    let title = target.title.clone();

    if !target.is_on_screen {
        let wait_until = Instant::now() + Duration::from_millis(750);
        while !target.is_on_screen && Instant::now() < wait_until {
            pump_active_mimics();
            thread::sleep(Duration::from_millis(10));
            let world_for_snapshot = world.clone();
            let refreshed = block_on_with_pump(async move { world_for_snapshot.snapshot().await })?;
            if let Some(updated) = refreshed
                .into_iter()
                .find(|win| win.title.contains(&marker))
            {
                target = updated;
                debug!(
                    case = slug,
                    on_screen = target.is_on_screen,
                    "spawn_place_state_target_visibility_update"
                );
            }
        }
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

    let warmup_deadline = Instant::now() + Duration::from_millis(1_200);
    let world_for_frames = world;
    while Instant::now() < warmup_deadline {
        pump_active_mimics();
        let frames_ready = block_on_with_pump({
            let world_clone = world_for_frames.clone();
            async move { world_clone.frames(target_key).await }
        })?;
        if frames_ready.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    Ok(PlaceState {
        mimic,
        target_id,
        target_key,
        expected,
        cursor,
        slug,
        title,
        raise: raise_strategy,
    })
}

/// Best-effort ensure the helper window is frontmost before issuing commands.
fn promote_helper_frontmost(case: &str, state: &PlaceState) {
    match state.raise {
        RaiseStrategy::SmartRaise { deadline } => {
            if let Err(err) = world::smart_raise(state.target_id, &state.title, deadline) {
                debug!(case = %case, error = %err, "smart_raise_failed");
            }
        }
        RaiseStrategy::AppActivate => {
            if let Err(err) = world::ensure_frontmost(
                state.target_id.pid(),
                &state.title,
                6,
                config::INPUT_DELAYS.ui_action_delay_ms,
            ) {
                debug!(case = %case, error = %err, "ensure_frontmost_failed");
            }
        }
        _ => {}
    }
}

/// Wait until the world reports authoritative frames for the supplied key.
fn wait_for_initial_frames(
    case: &str,
    world: &WorldHandle,
    cursor: &mut EventCursor,
    key: WindowKey,
    timeout: Duration,
) -> Result<Frames> {
    helpers::wait_for_events_or(case, world, cursor, timeout, || {
        let world_clone = world.clone();
        let frames = block_on_with_pump(async move { world_clone.frames(key).await })?;
        Ok(frames.is_some())
    })?;
    let world_clone = world.clone();
    block_on_with_pump(async move { world_clone.frames(key).await })?
        .ok_or_else(|| Error::InvalidState("authoritative frame unavailable".into()))
}

/// Re-subscribe to world events so subsequent waits start from a fresh cursor.
fn refresh_cursor(place: &mut PlaceState, world: &WorldHandle) -> Result<()> {
    let world_clone = world.clone();
    let (cursor, _snapshot, _events) =
        block_on_with_pump(async move { world_clone.subscribe_with_snapshot().await })?;
    place.cursor = cursor;
    Ok(())
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

/// Issue a world placement command for the supplied window and grid cell.
fn request_grid(
    world: &WorldHandle,
    target: WorldWindowId,
    grid: (u32, u32, u32, u32),
) -> Result<()> {
    let (cols, rows, col, row) = grid;
    let mut attempts = 0;
    loop {
        let request_world = world.clone();
        let receipt_result = block_on_with_pump(async move {
            request_world
                .request_place_for_window(target, cols, rows, col, row, None)
                .await
        })?;
        match receipt_result {
            Ok(receipt) => {
                let _target = receipt.target.ok_or_else(|| {
                    Error::InvalidState("placement did not select a target".into())
                })?;
                return Ok(());
            }
            Err(err) => {
                let message = err.to_string();
                if attempts < 3 && message.contains("mode=") {
                    attempts += 1;
                    pump_active_mimics();
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }
                return Err(Error::InvalidState(format!(
                    "placement request failed: {message}"
                )));
            }
        }
    }
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

/// Issue a world move command for the supplied window and grid configuration.
/// Wait until world reports the expected authoritative frame for the helper window.
fn wait_for_expected(
    stage: &StageHandle<'_>,
    state: &mut PlaceState,
    timeout: Duration,
    eps: i32,
) -> Result<hotki_world::Frames> {
    let world = stage.world_clone();
    let target_key = state.target_key;
    let expected_rect = state.expected;
    helpers::wait_for_events_or(
        stage.case_name(),
        &world,
        &mut state.cursor,
        timeout,
        || {
            let world_for_frames = world.clone();
            let maybe_frames =
                block_on_with_pump(async move { world_for_frames.frames(target_key).await })?;
            let frames = match maybe_frames {
                Some(f) => f,
                None => return Ok(false),
            };
            let matches = rect_matches(frames.authoritative, expected_rect, eps);
            if !matches {
                debug!(
                    case = %stage.case_name(),
                    actual = ?frames.authoritative,
                    expected = ?expected_rect,
                    "wait_for_expected_mismatch"
                );
            }
            Ok(matches)
        },
    )?;
    let world_for_final = world.clone();
    block_on_with_pump(async move { world_for_final.frames(target_key).await })?
        .ok_or_else(|| Error::InvalidState("authoritative frame unavailable".into()))
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
