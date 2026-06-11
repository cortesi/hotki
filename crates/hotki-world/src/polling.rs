use std::{
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
    time::{Duration, Instant},
};

use tokio::time;

use crate::{
    FocusSnapshot, WindowKey, WorldCfg, WorldWindow,
    platform::{PlatformSnapshot, capture_platform_snapshot},
    state::{CoreWorldView, WorldCore, WorldPollUpdate},
};

/// Lightweight world implementation backed by periodic polling of focus + displays.
pub(crate) struct PollingWorld {
    core: Arc<WorldCore>,
    poll_tuner: Arc<PollTuner>,
}

impl PollingWorld {
    pub(crate) fn spawn(cfg: WorldCfg) -> Arc<Self> {
        let poll_tuner = Arc::new(PollTuner::new(cfg.poll_ms_min, cfg.poll_ms_max));
        let core = WorldCore::new();
        let world = Arc::new(Self {
            core: core.clone(),
            poll_tuner: poll_tuner.clone(),
        });

        tokio::spawn(Self::run_poll_loop(Arc::downgrade(&core), cfg, poll_tuner));
        world
    }

    async fn run_poll_loop(core: Weak<WorldCore>, cfg: WorldCfg, poll_tuner: Arc<PollTuner>) {
        let mut interval_ms = cfg.poll_ms_min.max(50);
        loop {
            let Some(core) = core.upgrade() else {
                break;
            };
            Self::poll_once_core(&core, interval_ms).await;
            interval_ms = poll_tuner.next_interval(interval_ms);
            drop(core);
            time::sleep(Duration::from_millis(interval_ms)).await;
        }
    }

    async fn poll_once_core(core: &WorldCore, interval_ms: u64) {
        let start = Instant::now();
        let platform = capture_platform_snapshot();
        let elapsed = start.elapsed().as_millis() as u64;
        let changes =
            core.state
                .apply_poll_update(world_poll_update(platform), elapsed, interval_ms);
        changes.publish(&core.hub);
    }
}

impl CoreWorldView for PollingWorld {
    fn core(&self) -> &Arc<WorldCore> {
        &self.core
    }

    fn hint_refresh_impl(&self) {
        self.poll_tuner.reset();
    }
}

/// Simple backoff controller for polling cadence.
struct PollTuner {
    min_ms: u64,
    max_ms: u64,
    next_ms: AtomicU64,
}

impl PollTuner {
    fn new(min_ms: u64, max_ms: u64) -> Self {
        let clamped_min = min_ms.max(50);
        Self {
            min_ms: clamped_min,
            max_ms,
            next_ms: AtomicU64::new(clamped_min),
        }
    }

    /// Compute the next interval, applying a gentle backoff up to max_ms.
    fn next_interval(&self, last_ms: u64) -> u64 {
        let proposed = last_ms.saturating_add(10).min(self.max_ms);
        self.next_ms.store(proposed, AtomicOrdering::SeqCst);
        proposed
    }

    /// Reset the cadence to the minimum to react quickly to external changes.
    fn reset(&self) {
        self.next_ms.store(self.min_ms, AtomicOrdering::SeqCst);
    }
}

fn world_poll_update(platform: PlatformSnapshot) -> WorldPollUpdate {
    let focused = platform.focused.as_ref().map(|window| WindowKey {
        pid: window.pid,
        id: window.id,
    });
    let focus = platform.focused.as_ref().map(|window| FocusSnapshot {
        app: window.app.clone(),
        title: window.title.clone(),
        pid: window.pid,
        display_id: window.display_id,
    });
    let snapshot: Vec<WorldWindow> = platform
        .windows
        .iter()
        .map(|window| WorldWindow {
            app: window.app.clone(),
            title: window.title.clone(),
            pid: window.pid,
            id: window.id,
            display_id: window.display_id,
            focused: focused
                .map(|key| key.pid == window.pid && key.id == window.id)
                .unwrap_or(false),
        })
        .collect();

    WorldPollUpdate {
        snapshot,
        focused,
        focus,
        displays: platform.displays,
        capabilities: platform.capabilities,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn poll_tuner_clamps_minimum_and_backs_off_to_maximum() {
        let tuner = PollTuner::new(10, 70);

        assert_eq!(tuner.next_interval(50), 60);
        assert_eq!(tuner.next_interval(60), 70);
        assert_eq!(tuner.next_interval(70), 70);

        tuner.reset();
        assert_eq!(tuner.next_ms.load(AtomicOrdering::SeqCst), 50);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn polling_world_task_stops_after_world_drop() {
        let cfg = WorldCfg {
            poll_ms_min: 10,
            poll_ms_max: 20,
        };
        let world = PollingWorld::spawn(cfg);
        let weak_core = Arc::downgrade(&world.core);

        assert!(weak_core.upgrade().is_some());
        tokio::task::yield_now().await;

        drop(world);

        for _ in 0..3 {
            if weak_core.upgrade().is_none() {
                return;
            }
            tokio::time::advance(Duration::from_millis(50)).await;
            tokio::task::yield_now().await;
        }
        panic!("WorldCore was not deallocated; loop might be leaking the strong reference!");
    }
}
