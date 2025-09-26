//! World-centric smoketest cases implemented with the mimic harness.

use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use hotki_world::{
    PermissionState, World, WorldCfg, WorldHandle, WorldWindow, mimic::pump_active_mimics,
};
use mac_winops::{AxProps, WindowInfo, ops::MockWinOps};
use tokio::time::sleep;

use super::support::{
    ScenarioState, WindowSpawnSpec, block_on_with_pump, raise_window, shutdown_mimic,
    spawn_scenario,
};
use crate::{
    config,
    error::{Error, Result},
    suite::CaseCtx,
    world,
};

/// Verify that world status reports healthy capabilities and reasonable polling budgets.
pub fn world_status_permissions(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let mut scenario: Option<ScenarioState> = None;
    ctx.setup(|ctx| {
        let spec = WindowSpawnSpec::new("primary", "StatusProbe").configure(|config| {
            config.time_ms = 20_000;
            config.label_text = Some("WS".into());
        });
        scenario = Some(spawn_scenario(ctx, "world.status.permissions", vec![spec])?);
        Ok(())
    })?;

    ctx.action(|ctx| {
        if scenario.is_none() {
            return Err(Error::InvalidState("scenario missing during action".into()));
        }
        let world = ctx.world_clone();
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

        let focused = focused_repr.unwrap_or_else(|| "-".to_string());
        ctx.log_event(
            "world_status_permissions",
            &format!(
                "windows_count={} focused={} last_tick_ms={} current_poll_ms={} accessibility={:?} screen_recording={:?} debounce_cache={} debounce_pending={} reconcile_seq={} suspects_pending={}",
                status.windows_count,
                focused,
                status.last_tick_ms,
                status.current_poll_ms,
                status.capabilities.accessibility,
                status.capabilities.screen_recording,
                status.debounce_cache,
                status.debounce_pending,
                status.reconcile_seq,
                status.suspects_pending
            ),
        );

        Ok(())
    })?;

    ctx.settle(|ctx| {
        let state = scenario
            .take()
            .ok_or_else(|| Error::InvalidState("scenario missing during settle".into()))?;
        finalize_scenario(ctx, state)
    })?;

    Ok(())
}

/// Ensure AX properties surface on the focused window via world snapshots.
pub fn world_ax_focus_props(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let mut scenario: Option<ScenarioState> = None;
    ctx.setup(|ctx| {
        let spec = WindowSpawnSpec::new("primary", "AXProps").configure(|config| {
            config.time_ms = 20_000;
            config.label_text = Some("AX".into());
        });
        scenario = Some(spawn_scenario(ctx, "world.ax.focus_props", vec![spec])?);
        Ok(())
    })?;

    ctx.action(|ctx| {
        let state = scenario
            .as_mut()
            .ok_or_else(|| Error::InvalidState("scenario missing during action".into()))?;
        raise_window(ctx, state, "primary")?;
        let window = state.window("primary")?;
        let expected = window.world_id;
        let world = ctx.world_clone();

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

        ctx.log_event(
            "world_ax_focus_props",
            &format!(
                "role={:?} subrole={:?} can_set_pos={:?} can_set_size={:?}",
                props.role, props.subrole, props.can_set_pos, props.can_set_size
            ),
        );

        Ok(())
    })?;

    ctx.settle(|ctx| {
        let state = scenario
            .take()
            .ok_or_else(|| Error::InvalidState("scenario missing during settle".into()))?;
        finalize_scenario(ctx, state)
    })?;

    Ok(())
}

/// Record diagnostics and shutdown the mimic scenario.
fn finalize_scenario(_ctx: &CaseCtx<'_>, state: ScenarioState) -> Result<()> {
    shutdown_mimic(state.mimic)?;
    Ok(())
}

/// Verify that world adopts the active space within budget when mocked winops switch spaces.
pub fn world_spaces_adoption(ctx: &mut CaseCtx<'_>) -> Result<()> {
    const SLUG: &str = "world.spaces.adoption";
    let mut metrics: Option<WorldSpacesMetrics> = None;

    ctx.setup(|_| Ok(()))?;

    ctx.action(|_| {
        metrics = Some(run_world_spaces_simulation()?);
        Ok(())
    })?;

    ctx.settle(|ctx| {
        let outcome = metrics
            .take()
            .ok_or_else(|| Error::InvalidState("world-spaces metrics missing".into()))?;

        ctx.log_event(
            "world_spaces_outcome",
            &format!(
                "slug={} adoption_ms={} last_tick_ms={} current_poll_ms={}",
                SLUG, outcome.adoption_ms, outcome.last_tick_ms, outcome.current_poll_ms
            ),
        );

        Ok(())
    })?;

    Ok(())
}

/// Metrics captured during the world spaces simulation.
struct WorldSpacesMetrics {
    /// Measured milliseconds between hint refresh and adoption of the new space.
    adoption_ms: u64,
    /// Last tick duration reported by the world status snapshot.
    last_tick_ms: u64,
    /// Current poll interval reported by the world status snapshot.
    current_poll_ms: u64,
}

/// Simulate multi-space adoption against the mock winops backend.
fn run_world_spaces_simulation() -> Result<WorldSpacesMetrics> {
    world::block_on(async {
        const ADOPTION_BUDGET_MS: u64 = 250;

        let mock = Arc::new(MockWinOps::new());
        mock.set_active_spaces(vec![1]);
        mock.set_windows(vec![
            win(Some(1), 100, 1, true),
            win(Some(2), 200, 2, false),
        ]);

        let world = World::spawn(mock.clone(), world_cfg_fast());

        let initial_ok = wait_until(&world, Duration::from_millis(200), |snap| {
            let space1_active = snap.iter().any(|w| w.pid == 100 && w.on_active_space);
            let space2_present = snap.iter().any(|w| w.pid == 200 && !w.on_active_space);
            space1_active && space2_present
        })
        .await;

        if !initial_ok {
            return Err(Error::InvalidState(
                "initial world snapshot missing expected windows".into(),
            ));
        }

        mock.set_active_spaces(vec![2]);
        mock.set_windows(vec![
            win(Some(1), 100, 1, false),
            win(Some(2), 200, 2, true),
        ]);
        world.hint_refresh();

        let start = Instant::now();
        let switched = wait_until(
            &world,
            Duration::from_millis(config::DEFAULTS.timeout_ms),
            |snap| {
                snap.iter()
                    .any(|w| w.pid == 200 && w.on_active_space && w.space == Some(2))
            },
        )
        .await;

        if !switched {
            return Err(Error::InvalidState(
                "world did not adopt new active space within timeout".into(),
            ));
        }

        let elapsed_ms = start.elapsed().as_millis() as u64;
        if elapsed_ms > ADOPTION_BUDGET_MS {
            return Err(Error::InvalidState(format!(
                "space adoption exceeded budget: {}ms (budget {}ms)",
                elapsed_ms, ADOPTION_BUDGET_MS
            )));
        }

        let snap = world.snapshot().await;
        let old = snap
            .iter()
            .find(|w| w.pid == 100)
            .ok_or_else(|| Error::InvalidState("missing original space window".into()))?;
        if old.on_active_space {
            return Err(Error::InvalidState(
                "original space window remained marked active".into(),
            ));
        }
        let new = snap
            .iter()
            .find(|w| w.pid == 200)
            .ok_or_else(|| Error::InvalidState("missing adopted space window".into()))?;
        if !new.on_active_space {
            return Err(Error::InvalidState(
                "new active space window not marked on_active_space".into(),
            ));
        }

        let status = world.status().await;
        if status.last_tick_ms > ADOPTION_BUDGET_MS {
            return Err(Error::InvalidState(format!(
                "world tick exceeded budget: {}ms",
                status.last_tick_ms
            )));
        }
        if status.current_poll_ms > ADOPTION_BUDGET_MS {
            return Err(Error::InvalidState(format!(
                "world poll interval exceeded budget: {}ms",
                status.current_poll_ms
            )));
        }

        Ok(WorldSpacesMetrics {
            adoption_ms: elapsed_ms,
            last_tick_ms: status.last_tick_ms,
            current_poll_ms: status.current_poll_ms,
        })
    })?
}

/// Construct a fast-polling world configuration suitable for the simulation.
fn world_cfg_fast() -> WorldCfg {
    WorldCfg {
        poll_ms_min: 20,
        poll_ms_max: 120,
        include_offscreen: false,
        ax_watch_frontmost: false,
        events_buffer: 64,
    }
}

/// Convenience helper to construct a `WindowInfo` for the mock winops.
fn win(space: Option<i64>, pid: i32, id: u32, focused: bool) -> WindowInfo {
    WindowInfo {
        id,
        pid,
        app: format!("App{}", pid),
        title: format!("Win{}", id),
        pos: None,
        space,
        layer: 0,
        focused,
        is_on_screen: true,
        on_active_space: false,
    }
}

/// Await until the snapshot satisfies `pred` or the timeout elapses.
async fn wait_until<F>(world: &WorldHandle, timeout: Duration, mut pred: F) -> bool
where
    F: FnMut(&[WorldWindow]) -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        let snap = world.snapshot().await;
        if pred(&snap) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(5)).await;
    }
}
