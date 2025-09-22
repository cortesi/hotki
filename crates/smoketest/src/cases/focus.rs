//! Focus-centric smoketest cases executed via the mimic harness.
use std::time::Instant;

use tracing::debug;

use super::support::{
    ScenarioState, WindowSpawnSpec, raise_window, record_mimic_diagnostics, shutdown_mimic,
    spawn_scenario,
};
use crate::{
    error::{Error, Result},
    suite::{CaseCtx, StageHandle},
};

/// Wrap the legacy focus tracking smoketest within the suite runner.
pub fn focus_tracking(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let _fast_raise_guard = mac_winops::override_ensure_frontmost_config(3, 40, 160);
    let mut scenario: Option<ScenarioState> = None;
    ctx.setup(|stage| {
        let specs = vec![
            WindowSpawnSpec::new("support", "focus-support").configure(|config| {
                config.time_ms = 25_000;
                config.size = Some((520.0, 320.0));
                config.pos = Some((180.0, 200.0));
                config.label_text = Some("S".into());
            }),
            WindowSpawnSpec::new("primary", "focus-primary").configure(|config| {
                config.time_ms = 25_000;
                config.size = Some((520.0, 320.0));
                config.pos = Some((860.0, 200.0));
                config.label_text = Some("P".into());
            }),
        ];
        scenario = Some(spawn_scenario(stage, "focus.tracking", specs)?);
        Ok(())
    })?;
    ctx.action(|stage| {
        let state = scenario
            .as_mut()
            .ok_or_else(|| Error::InvalidState("focus scenario missing during action".into()))?;
        raise_window(stage, state, "support")?;
        raise_window(stage, state, "primary")?;
        Ok(())
    })?;
    ctx.settle(|stage| {
        let state = scenario
            .take()
            .ok_or_else(|| Error::InvalidState("focus scenario missing during settle".into()))?;
        record_mimic_diagnostics(stage, state.slug, &state.mimic)?;
        shutdown_mimic(state.mimic)?;
        Ok(())
    })?;
    Ok(())
}

/// Wrap the legacy focus navigation smoketest within the suite runner.
pub fn focus_nav(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let _fast_raise_guard = mac_winops::override_ensure_frontmost_config(3, 40, 160);
    let mut scenario: Option<ScenarioState> = None;
    ctx.setup(|stage| {
        let slots = [
            ("tl", "focus-nav-tl", (180.0, 520.0), "TL"),
            ("tr", "focus-nav-tr", (860.0, 520.0), "TR"),
            ("br", "focus-nav-br", (860.0, 200.0), "BR"),
            ("bl", "focus-nav-bl", (180.0, 200.0), "BL"),
        ];
        let specs = slots
            .into_iter()
            .map(|(label, title, (x, y), tag)| {
                WindowSpawnSpec::new(label, title).configure(move |config| {
                    config.time_ms = 25_000;
                    config.size = Some((520.0, 320.0));
                    config.pos = Some((x, y));
                    config.label_text = Some(tag.into());
                })
            })
            .collect::<Vec<_>>();
        scenario = Some(spawn_scenario(stage, "focus.nav", specs)?);
        Ok(())
    })?;
    ctx.action(|stage| {
        let state = scenario
            .as_mut()
            .ok_or_else(|| Error::InvalidState("focus scenario missing during action".into()))?;
        for label in ["tl", "tr", "br", "bl"] {
            raise_window(stage, state, label)?;
        }
        Ok(())
    })?;
    ctx.settle(|stage| {
        let state = scenario
            .take()
            .ok_or_else(|| Error::InvalidState("focus scenario missing during settle".into()))?;
        record_mimic_diagnostics(stage, state.slug, &state.mimic)?;
        shutdown_mimic(state.mimic)?;
        Ok(())
    })?;
    Ok(())
}

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
