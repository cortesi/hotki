//! Focus-centric smoketest cases executed via the mimic harness.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use hotki_world::{RaiseIntent, WorldEvent, WorldHandle, mimic::pump_active_mimics};
use hotki_world_ids::WorldWindowId;
use regex::Regex;
use tracing::debug;

use super::support::{
    ScenarioState, WindowSpawnSpec, block_on_with_pump, record_mimic_diagnostics, shutdown_mimic,
    spawn_scenario,
};
use crate::{
    error::{Error, Result},
    suite::{CaseCtx, StageHandle},
};

/// Verify that `request_raise` selects windows by title and updates focus ordering.
pub fn raise(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let _fast_raise_guard = mac_winops::override_ensure_frontmost_config(3, 40, 160);
    let mut scenario: Option<ScenarioState> = None;
    let setup_start = Instant::now();
    ctx.setup(|stage| {
        let specs = vec![
            WindowSpawnSpec::new("primary", "raise-primary").configure(|config| {
                config.time_ms = 25_000;
                config.size = Some((540.0, 360.0));
                config.pos = Some((160.0, 140.0));
                config.label_text = Some("P".into());
            }),
            WindowSpawnSpec::new("sibling", "raise-sibling").configure(|config| {
                config.time_ms = 25_000;
                config.size = Some((540.0, 360.0));
                config.pos = Some((860.0, 140.0));
                config.label_text = Some("S".into());
            }),
        ];
        scenario = Some(spawn_scenario(stage, "raise", specs)?);
        Ok(())
    })?;
    debug!(
        case = "raise",
        setup_ms = setup_start.elapsed().as_millis(),
        "raise_setup_timing"
    );

    let action_start = Instant::now();
    ctx.action(|stage| {
        let state = scenario
            .as_mut()
            .ok_or_else(|| Error::InvalidState("raise scenario missing during action".into()))?;
        run_raise_sequence(stage, state)?;
        Ok(())
    })?;
    debug!(
        case = "raise",
        action_ms = action_start.elapsed().as_millis(),
        "raise_action_timing"
    );

    let settle_start = Instant::now();
    ctx.settle(|stage| {
        let state = scenario
            .take()
            .ok_or_else(|| Error::InvalidState("raise scenario missing during settle".into()))?;
        record_mimic_diagnostics(stage, state.slug, &state.mimic)?;
        shutdown_mimic(state.mimic)?;
        Ok(())
    })?;
    debug!(
        case = "raise",
        settle_ms = settle_start.elapsed().as_millis(),
        "raise_settle_timing"
    );

    Ok(())
}

/// Execute the ordered raise sequence for the focus scenario.
fn run_raise_sequence(stage: &StageHandle<'_>, state: &mut ScenarioState) -> Result<()> {
    raise_window(stage, state, "primary")?;
    raise_window(stage, state, "sibling")?;
    Ok(())
}

/// Raise a helper window identified by `label`, asserting focus updates.
fn raise_window(stage: &StageHandle<'_>, state: &mut ScenarioState, label: &str) -> Result<()> {
    let start_all = Instant::now();
    let window = state.window(label)?;
    let world = stage.world_clone();
    let window_title = window.title.clone();
    let expected_id = window.world_id;
    let pattern = format!("^{}$", regex::escape(&window_title));
    let regex = Regex::new(&pattern)
        .map_err(|err| Error::InvalidState(format!("invalid raise regex: {err}")))?;
    let intent = RaiseIntent {
        app_regex: None,
        title_regex: Some(Arc::new(regex)),
    };

    let request_world = world.clone();
    let request_start = Instant::now();
    let receipt = block_on_with_pump(async move { request_world.request_raise(intent).await })?
        .map_err(|err| Error::InvalidState(format!("raise request failed: {err}")))?;
    let request_ms = request_start.elapsed().as_millis();

    let target_id = receipt.target_id().ok_or_else(|| {
        Error::InvalidState(format!(
            "raise did not select a target window (label={label} title={window_title})"
        ))
    })?;

    if target_id != expected_id {
        return Err(Error::InvalidState(format!(
            "raise targeted unexpected window: expected pid={} id={} got pid={} id={}",
            expected_id.pid(),
            expected_id.window_id(),
            target_id.pid(),
            target_id.window_id()
        )));
    }

    let wait_start = Instant::now();
    wait_for_focus(stage, state, &world, expected_id)?;
    let wait_ms = wait_start.elapsed().as_millis();
    let total_ms = start_all.elapsed().as_millis();
    debug!(
        case = %stage.case_name(),
        label,
        request_ms,
        wait_ms,
        total_ms,
        "raise_window_timing"
    );
    Ok(())
}

/// Wait until the world reports focus on the expected helper window.
fn wait_for_focus(
    stage: &StageHandle<'_>,
    state: &mut ScenarioState,
    world: &WorldHandle,
    expected_id: WorldWindowId,
) -> Result<()> {
    let expected_pid = expected_id.pid();
    let expected_window = expected_id.window_id();
    let mut logged_none = false;
    let mut logged_mismatch = false;
    let baseline_lost = state.cursor.lost_count;

    let initial_world = world.clone();
    let initial_focus = block_on_with_pump(async move { initial_world.focused().await })?;
    if let Some(key) = initial_focus {
        if key.pid == expected_pid && key.id == expected_window {
            return Ok(());
        }
        debug!(
            case = %stage.case_name(),
            expected_pid,
            expected_window,
            observed_pid = key.pid,
            observed_window = key.id,
            "wait_for_focus_initial_mismatch"
        );
        logged_mismatch = true;
    } else {
        debug!(
            case = %stage.case_name(),
            expected_pid,
            expected_window,
            "wait_for_focus_initial_none"
        );
        logged_none = true;
    }

    let deadline = Instant::now() + Duration::from_millis(10_000);
    loop {
        pump_active_mimics();
        let focus_world = world.clone();
        if let Some(key) = block_on_with_pump(async move { focus_world.focused().await })? {
            if key.pid == expected_pid && key.id == expected_window {
                return Ok(());
            }
            if !logged_mismatch {
                debug!(
                    case = %stage.case_name(),
                    expected_pid,
                    expected_window,
                    observed_pid = key.pid,
                    observed_window = key.id,
                    "wait_for_focus_poll_mismatch"
                );
                logged_mismatch = true;
            }
        } else if !logged_none {
            debug!(
                case = %stage.case_name(),
                expected_pid,
                expected_window,
                "wait_for_focus_poll_none"
            );
            logged_none = true;
        }
        if Instant::now() >= deadline {
            return Err(Error::InvalidState(format!(
                "timeout waiting for {} (lost_count={} next_index={})",
                stage.case_name(),
                state.cursor.lost_count,
                state.cursor.next_index
            )));
        }
        let pump_until = Instant::now() + Duration::from_millis(10);
        world.pump_main_until(pump_until);
        pump_active_mimics();
        while let Some(event) = world.next_event_now(&mut state.cursor) {
            if state.cursor.lost_count > baseline_lost {
                return Err(Error::InvalidState(format!(
                    "events lost during wait (lost_count={}): see artifacts",
                    state.cursor.lost_count
                )));
            }
            if let WorldEvent::FocusChanged(change) = event {
                if let Some(ref key) = change.key {
                    debug!(
                        case = %stage.case_name(),
                        expected_pid,
                        expected_window,
                        observed_pid = key.pid,
                        observed_window = key.id,
                        "focus_event"
                    );
                    if key.pid == expected_pid && key.id == expected_window {
                        return Ok(());
                    }
                    if !logged_mismatch {
                        debug!(
                            case = %stage.case_name(),
                            expected_pid,
                            expected_window,
                            observed_pid = key.pid,
                            observed_window = key.id,
                            "wait_for_focus_mismatch"
                        );
                        logged_mismatch = true;
                    }
                } else {
                    debug!(
                        case = %stage.case_name(),
                        expected_pid,
                        expected_window,
                        "focus_event_none"
                    );
                    if !logged_none {
                        logged_none = true;
                    }
                }
            }
            pump_active_mimics();
        }
        if state.cursor.lost_count > baseline_lost {
            return Err(Error::InvalidState(format!(
                "events lost during wait (lost_count={}): see artifacts",
                state.cursor.lost_count
            )));
        }
    }
}
