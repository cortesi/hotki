//! Hide/show smoketest cases executed with the mimic harness.
use std::time::Duration;

use hotki_world::{CommandToggle, HideIntent, RectPx, WindowKey, WindowMode, WorldHandle};
use hotki_world_ids::WorldWindowId;
use mac_winops::drain_main_ops;
use tracing::debug;

use super::support::{
    ScenarioState, WindowSpawnSpec, block_on_with_pump, raise_window, record_mimic_diagnostics,
    shutdown_mimic, spawn_scenario,
};
use crate::{
    config,
    error::{Error, Result},
    helpers, runtime,
    suite::{CaseCtx, StageHandle},
};

/// Shared state for the hide toggle smoketest.
struct HideCaseState {
    /// Active mimic scenario with helper window metadata.
    scenario: ScenarioState,
    /// Label assigned to the primary helper window.
    target_label: &'static str,
    /// World identifier for the helper window under test.
    target_id: WorldWindowId,
    /// Window key used for frame lookups.
    target_key: WindowKey,
    /// Expected authoritative rectangle once the window is restored.
    expected: RectPx,
    /// Integer pixel tolerance used when validating the restored frame.
    eps: i32,
    /// Whether the case observed the hidden window mode during action.
    hidden_observed: bool,
}

/// Verify that world-driven hide commands move the helper off-screen and restore it on demand.
pub fn hide_toggle_roundtrip(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let mut state: Option<HideCaseState> = None;
    ctx.setup(|stage| {
        let specs = vec![
            WindowSpawnSpec::new("primary", "hide-primary").configure(|config| {
                config.time_ms = 25_000;
                config.size = Some((560.0, 360.0));
                config.pos = Some((480.0, 260.0));
                config.label_text = Some("H".into());
            }),
        ];
        let scenario = spawn_scenario(stage, "hide.toggle", specs)?;
        let window = scenario.window("primary")?;
        let world = stage.world_clone();
        let target_id = window.world_id;
        let target_key = window.key;
        let frames = block_on_with_pump(async move { world.frames(target_key).await })?
            .ok_or_else(|| Error::InvalidState("hide: frames unavailable after spawn".into()))?;
        let eps = config::PLACE.eps.round() as i32;
        debug!(
            case = %stage.case_name(),
            initial = ?frames.authoritative,
            scale = frames.scale,
            "hide_setup_initial_frame"
        );
        state = Some(HideCaseState {
            scenario,
            target_label: "primary",
            target_id,
            target_key,
            expected: frames.authoritative,
            eps,
            hidden_observed: false,
        });
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("hide state missing during action".into()))?;
        let scenario = &mut state_ref.scenario;
        let world = stage.world_clone();

        raise_window(stage, scenario, state_ref.target_label)?;

        let normal_frames = wait_for_mode(
            stage,
            scenario,
            &world,
            state_ref.target_key,
            WindowMode::Normal,
            Duration::from_millis(config::HIDE.restore_max_ms),
        )?;
        debug!(
            case = %stage.case_name(),
            mode = ?normal_frames.mode,
            frame = ?normal_frames.authoritative,
            "hide_action_ready"
        );

        let hide_world = world.clone();
        let receipt = runtime::block_on(async move {
            hide_world
                .request_hide(HideIntent {
                    desired: CommandToggle::On,
                })
                .await
        })?
        .map_err(|err| Error::InvalidState(format!("hide(on) request failed: {err}")))?;
        if let Some(target) = receipt.target_id()
            && target != state_ref.target_id
        {
            return Err(Error::InvalidState(format!(
                "hide(on) targeted unexpected window: expected pid={} id={} got pid={} id={}",
                state_ref.target_id.pid(),
                state_ref.target_id.window_id(),
                target.pid(),
                target.window_id()
            )));
        }
        drain_main_ops();
        let hidden_frames = wait_for_mode(
            stage,
            scenario,
            &world,
            state_ref.target_key,
            WindowMode::Hidden,
            Duration::from_millis(config::HIDE.restore_max_ms.saturating_add(800)),
        )?;
        state_ref.hidden_observed = true;
        debug!(
            case = %stage.case_name(),
            frame = ?hidden_frames.authoritative,
            mode = ?hidden_frames.mode,
            "hide_action_hidden"
        );

        let show_world = world.clone();
        let receipt = runtime::block_on(async move {
            show_world
                .request_hide(HideIntent {
                    desired: CommandToggle::Off,
                })
                .await
        })?
        .map_err(|err| Error::InvalidState(format!("hide(off) request failed: {err}")))?;
        if let Some(target) = receipt.target_id()
            && target != state_ref.target_id
        {
            return Err(Error::InvalidState(format!(
                "hide(off) targeted unexpected window: expected pid={} id={} got pid={} id={}",
                state_ref.target_id.pid(),
                state_ref.target_id.window_id(),
                target.pid(),
                target.window_id()
            )));
        }
        drain_main_ops();
        let restored_frames = wait_for_rect(
            stage,
            scenario,
            &world,
            state_ref.target_key,
            state_ref.expected,
            state_ref.eps,
            Duration::from_millis(config::HIDE.restore_max_ms.saturating_add(1_200)),
        )?;
        debug!(
            case = %stage.case_name(),
            frame = ?restored_frames.authoritative,
            mode = ?restored_frames.mode,
            "hide_action_restored"
        );
        Ok(())
    })?;

    ctx.settle(|stage| {
        let state_data = state
            .take()
            .ok_or_else(|| Error::InvalidState("hide state missing during settle".into()))?;
        if !state_data.hidden_observed {
            return Err(Error::InvalidState(
                "hide case did not observe hidden window state".into(),
            ));
        }
        let world = stage.world_clone();
        let final_frames =
            block_on_with_pump(async move { world.frames(state_data.target_key).await })?
                .ok_or_else(|| Error::InvalidState("hide: final frames unavailable".into()))?;
        if final_frames.mode != WindowMode::Normal {
            return Err(Error::InvalidState(format!(
                "hide: expected Normal mode after restore, saw {:?}",
                final_frames.mode
            )));
        }
        let diag_path =
            record_mimic_diagnostics(stage, state_data.scenario.slug, &state_data.scenario.mimic)?;
        let artifacts = [diag_path];
        helpers::assert_frame_matches(
            stage.case_name(),
            state_data.expected,
            &final_frames,
            state_data.eps,
            &artifacts,
        )?;
        shutdown_mimic(state_data.scenario.mimic)?;
        Ok(())
    })?;

    Ok(())
}

/// Wait until the authoritative frame matches the expected rectangle within the provided epsilon.
/// Wait until the helper window reports the expected mode.
fn wait_for_mode(
    stage: &StageHandle<'_>,
    state: &mut ScenarioState,
    world: &WorldHandle,
    target: WindowKey,
    expected_mode: WindowMode,
    timeout: Duration,
) -> Result<hotki_world::Frames> {
    helpers::wait_for_events_or(stage.case_name(), world, &mut state.cursor, timeout, || {
        let world_clone = world.clone();
        let maybe_frames = block_on_with_pump(async move { world_clone.frames(target).await })?;
        let frames = match maybe_frames {
            Some(frames) => frames,
            None => {
                debug!(
                    case = %stage.case_name(),
                    "hide_wait_for_mode_missing_frames"
                );
                return Ok(false);
            }
        };
        let matches = frames.mode == expected_mode;
        if matches {
            debug!(
                case = %stage.case_name(),
                current_mode = ?frames.mode,
                expected = ?expected_mode,
                "hide_wait_for_mode_match"
            );
        } else {
            debug!(
                case = %stage.case_name(),
                current_mode = ?frames.mode,
                expected = ?expected_mode,
                "hide_wait_for_mode_poll"
            );
        }
        Ok(matches)
    })?;
    let world_clone = world.clone();
    block_on_with_pump(async move { world_clone.frames(target).await })?
        .ok_or_else(|| Error::InvalidState("hide: frames unavailable after wait".into()))
}

/// Wait until the authoritative frame matches the expected rectangle within the provided epsilon.
fn wait_for_rect(
    stage: &StageHandle<'_>,
    state: &mut ScenarioState,
    world: &WorldHandle,
    target: WindowKey,
    expected: RectPx,
    eps: i32,
    timeout: Duration,
) -> Result<hotki_world::Frames> {
    helpers::wait_for_events_or(stage.case_name(), world, &mut state.cursor, timeout, || {
        let world_clone = world.clone();
        let maybe_frames = block_on_with_pump(async move { world_clone.frames(target).await })?;
        let frames = match maybe_frames {
            Some(frames) => frames,
            None => return Ok(false),
        };
        if frames.mode != WindowMode::Normal {
            debug!(
                case = %stage.case_name(),
                current_mode = ?frames.mode,
                "hide_wait_for_rect_mode_pending"
            );
            return Ok(false);
        }
        let delta = expected.delta(&frames.authoritative);
        Ok(delta.dx.abs() <= eps
            && delta.dy.abs() <= eps
            && delta.dw.abs() <= eps
            && delta.dh.abs() <= eps)
    })?;
    let world_clone = world.clone();
    block_on_with_pump(async move { world_clone.frames(target).await })?
        .ok_or_else(|| Error::InvalidState("hide: frames unavailable after wait".into()))
}
