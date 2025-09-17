use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use hotki_world::{World, WorldCfg, WorldHandle, WorldWindow};
use mac_winops::{WindowInfo, ops::MockWinOps};
use tokio::{runtime::Builder, time::sleep};

use crate::error::{Error, Result};

/// Maximum allowed time in milliseconds for world adoption during the test.
const ADOPTION_BUDGET_MS: u64 = 250;

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

/// Await until the snapshot from `world` satisfies `pred` or `timeout_ms` elapses.
async fn wait_until<F>(world: &WorldHandle, timeout_ms: u64, mut pred: F) -> bool
where
    F: FnMut(&[WorldWindow]) -> bool,
{
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
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

/// Execute the simulated multi-space adoption smoketest.
pub fn run_world_spaces_test(timeout_ms: u64, _logs: bool) -> Result<()> {
    let rt = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::InvalidState(format!("tokio runtime init failed: {}", e)))?;

    rt.block_on(async move {
        let mock = Arc::new(MockWinOps::new());
        mock.set_active_spaces(vec![1]);
        mock.set_windows(vec![
            win(Some(1), 100, 1, true),
            win(Some(2), 200, 2, false),
        ]);

        let world = World::spawn(mock.clone(), world_cfg_fast());

        let initial = wait_until(&world, 200, |snap| {
            let space1_active = snap.iter().any(|w| w.pid == 100 && w.on_active_space);
            let space2_present = snap.iter().any(|w| w.pid == 200 && !w.on_active_space);
            space1_active && space2_present
        })
        .await;
        if !initial {
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
        let switched = wait_until(&world, timeout_ms, |snap| {
            snap.iter()
                .any(|w| w.pid == 200 && w.on_active_space && w.space == Some(2))
        })
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
        Ok(())
    })
}
