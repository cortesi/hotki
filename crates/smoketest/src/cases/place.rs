//! Placement smoketest cases implemented against the mimic harness.
use std::{
    fs,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use hotki_world::{
    EventCursor, MinimizedPolicy, PlaceOptions, RaiseStrategy, RectPx, WindowKey, WorldHandle,
    mimic::{HelperConfig, MimicHandle, MimicScenario, MimicSpec, pump_active_mimics, spawn_mimic},
};
use hotki_world_ids::WorldWindowId;
use mac_winops;
use tracing::debug;

use super::support::{block_on_with_pump, record_mimic_diagnostics, shutdown_mimic};
use crate::{
    error::{Error, Result},
    helpers,
    suite::{CaseCtx, StageHandle},
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
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state
            .as_ref()
            .ok_or_else(|| Error::InvalidState("place state missing during action".into()))?;
        let world = stage.world_clone();
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
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state
            .as_ref()
            .ok_or_else(|| Error::InvalidState("place state missing during action".into()))?;
        let world = stage.world_clone();
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

    let spec = MimicSpec::new(slug_arc.clone(), label_arc, "Primary").with_config(config);
    let scenario = MimicScenario::new(slug_arc, vec![spec]);

    let mimic = spawn_mimic(scenario)
        .map_err(|e| Error::InvalidState(format!("spawn mimic failed for {}: {e}", slug)))?;
    pump_active_mimics();

    let world_for_subscribe = world.clone();
    let (cursor, snapshot, _) =
        block_on_with_pump(async move { world_for_subscribe.subscribe_with_snapshot().await })?;
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
                break;
            }
        }
    }
    let mut target =
        target.ok_or_else(|| Error::InvalidState(format!("mimic window {} not observed", slug)))?;

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
            }
        }
    }

    let target_id = WorldWindowId::new(target.pid, target.id);
    let target_key = WindowKey {
        pid: target.pid,
        id: target.id,
    };

    if let Err(err) = mac_winops::request_activate_pid(target.pid) {
        debug!(pid = target.pid, ?err, "activate pid request failed");
    }

    let expected_path = stage
        .artifacts_dir()
        .join(format!("{}_expected.txt", slug.replace('.', "_")));
    fs::write(&expected_path, format!("expected={:?}\n", expected))?;
    stage.record_artifact(&expected_path);

    // Allow the mimic event loop to process initial timers before issuing actions.
    for _ in 0..200 {
        pump_active_mimics();
        thread::sleep(Duration::from_millis(5));
    }

    Ok(PlaceState {
        mimic,
        target_id,
        target_key,
        expected,
        cursor,
        slug,
    })
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
