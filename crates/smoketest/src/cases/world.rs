//! World-centric smoketest cases implemented with the mimic harness.

use std::{
    fs, thread,
    time::{Duration, Instant},
};

use hotki_world::{PermissionState, mimic::pump_active_mimics};
use mac_winops::AxProps;
use serde_json::json;

use super::support::{
    ScenarioState, WindowSpawnSpec, block_on_with_pump, raise_window, record_mimic_diagnostics,
    shutdown_mimic, spawn_scenario,
};
use crate::{
    error::{Error, Result},
    suite::{CaseCtx, StageHandle},
};

/// Verify that world status reports healthy capabilities and reasonable polling budgets.
pub fn world_status_permissions(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let mut scenario: Option<ScenarioState> = None;
    ctx.setup(|stage| {
        let spec = WindowSpawnSpec::new("primary", "StatusProbe").configure(|config| {
            config.time_ms = 20_000;
            config.label_text = Some("WS".into());
        });
        scenario = Some(spawn_scenario(
            stage,
            "world.status.permissions",
            vec![spec],
        )?);
        Ok(())
    })?;

    ctx.action(|stage| {
        if scenario.is_none() {
            return Err(Error::InvalidState("scenario missing during action".into()));
        }
        let world = stage.world_clone();
        let status = block_on_with_pump(async move { world.status().await })?;

        if status.windows_count == 0 {
            return Err(Error::InvalidState(
                "world-status: expected at least one window in snapshot".into(),
            ));
        }

        if status.capabilities.accessibility != PermissionState::Granted
            || status.capabilities.screen_recording != PermissionState::Granted
        {
            return Err(Error::InvalidState(format!(
                "world-status: capabilities not granted (accessibility={:?} screen_recording={:?})",
                status.capabilities.accessibility, status.capabilities.screen_recording
            )));
        }

        if !(10..=5_000).contains(&status.current_poll_ms) {
            return Err(Error::InvalidState(format!(
                "world-status: current poll outside bounds ({} ms)",
                status.current_poll_ms
            )));
        }

        let focused_repr = status
            .focused
            .map(|key| format!("pid={} id={}", key.pid, key.id));

        let status_path = stage.artifacts_dir().join("world_status_permissions.json");
        let payload = json!({
            "windows_count": status.windows_count,
            "focused": focused_repr,
            "last_tick_ms": status.last_tick_ms,
            "current_poll_ms": status.current_poll_ms,
            "capabilities": {
                "accessibility": format!("{:?}", status.capabilities.accessibility),
                "screen_recording": format!("{:?}", status.capabilities.screen_recording),
            },
            "debounce_cache": status.debounce_cache,
            "debounce_pending": status.debounce_pending,
            "reconcile_seq": status.reconcile_seq,
            "suspects_pending": status.suspects_pending,
        });
        let mut data = serde_json::to_string_pretty(&payload)
            .map_err(|e| Error::InvalidState(format!("failed to serialize world status: {e}")))?;
        data.push('\n');
        fs::write(&status_path, data)?;
        stage.record_artifact(&status_path);

        Ok(())
    })?;

    ctx.settle(|stage| {
        let state = scenario
            .take()
            .ok_or_else(|| Error::InvalidState("scenario missing during settle".into()))?;
        finalize_scenario(stage, state)
    })?;

    Ok(())
}

/// Ensure AX properties surface on the focused window via world snapshots.
pub fn world_ax_focus_props(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let mut scenario: Option<ScenarioState> = None;
    ctx.setup(|stage| {
        let spec = WindowSpawnSpec::new("primary", "AXProps").configure(|config| {
            config.time_ms = 20_000;
            config.label_text = Some("AX".into());
        });
        scenario = Some(spawn_scenario(stage, "world.ax.focus_props", vec![spec])?);
        Ok(())
    })?;

    ctx.action(|stage| {
        let state = scenario
            .as_mut()
            .ok_or_else(|| Error::InvalidState("scenario missing during action".into()))?;
        raise_window(stage, state, "primary")?;
        let window = state.window("primary")?;
        let expected = window.world_id;
        let world = stage.world_clone();

        let deadline = Instant::now() + Duration::from_millis(2_000);
        let props: AxProps = loop {
            let focused = block_on_with_pump({
                let world_clone = world.clone();
                async move { world_clone.focused_window().await }
            })?;
            if let Some(fw) = focused
                && fw.pid == expected.pid()
                && fw.id == expected.window_id()
                && let Some(ax) = fw.ax.clone()
            {
                break ax;
            }
            if Instant::now() >= deadline {
                return Err(Error::InvalidState(
                    "world-ax: focused window props not observed".into(),
                ));
            }
            pump_active_mimics();
            thread::sleep(Duration::from_millis(25));
        };

        let role_ok = props
            .role
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        let subrole_ok = props
            .subrole
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        let pos_flag_ok = props.can_set_pos.is_some();
        let size_flag_ok = props.can_set_size.is_some();

        if !(role_ok && subrole_ok && pos_flag_ok && size_flag_ok) {
            return Err(Error::InvalidState(format!(
                "world-ax: invalid props role={:?} subrole={:?} can_set_pos={:?} can_set_size={:?}",
                props.role, props.subrole, props.can_set_pos, props.can_set_size
            )));
        }

        let props_path = stage.artifacts_dir().join("world_ax_focus_props.json");
        let payload = json!({
            "role": props.role,
            "subrole": props.subrole,
            "can_set_pos": props.can_set_pos,
            "can_set_size": props.can_set_size,
        });
        let mut data = serde_json::to_string_pretty(&payload)
            .map_err(|e| Error::InvalidState(format!("failed to serialize ax props: {e}")))?;
        data.push('\n');
        fs::write(&props_path, data)?;
        stage.record_artifact(&props_path);

        Ok(())
    })?;

    ctx.settle(|stage| {
        let state = scenario
            .take()
            .ok_or_else(|| Error::InvalidState("scenario missing during settle".into()))?;
        finalize_scenario(stage, state)
    })?;

    Ok(())
}

/// Record diagnostics and shutdown the mimic scenario.
fn finalize_scenario(stage: &mut StageHandle<'_>, state: ScenarioState) -> Result<()> {
    record_mimic_diagnostics(stage, state.slug, &state.mimic)?;
    shutdown_mimic(state.mimic)?;
    Ok(())
}
