//! Event-driven waiting primitives used by smoketests to observe world state changes.

use std::{
    fmt::Write as _,
    sync::Arc,
    time::{Duration, Instant},
};

use thiserror::Error;
use tokio::time::Instant as TokioInstant;

use crate::{
    Frames, WindowKey, WindowMode, WorldEvent, WorldHandle, WorldWindow,
    events::{EventCursor, EventFilter},
};

/// Configuration for bounded world waits.
#[derive(Clone, Copy, Debug)]
pub struct WaitConfig {
    /// Total time allowed for the wait.
    pub overall: Duration,
    /// Maximum idle period when no new events are observed before re-checking state.
    pub idle: Duration,
    /// Maximum number of relevant events to observe before considering the wait saturated.
    pub max_events: usize,
}

impl WaitConfig {
    /// Create a wait configuration using the supplied bounds.
    #[must_use]
    pub const fn new(overall: Duration, idle: Duration, max_events: usize) -> Self {
        Self {
            overall,
            idle,
            max_events,
        }
    }
}

impl Default for WaitConfig {
    fn default() -> Self {
        Self {
            overall: Duration::from_secs(8),
            idle: Duration::from_millis(80),
            max_events: 512,
        }
    }
}

const MAIN_PUMP_SLICE: Duration = Duration::from_millis(5);

/// Visibility policies evaluated against the tracked window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisibilityPolicy {
    /// Window must report `is_on_screen = true`.
    OnScreen,
    /// Window must be both on-screen and on the active space.
    OnScreenAndActive,
    /// Window must report `on_active_space = true` regardless of on-screen status.
    OnActiveSpace,
    /// Window mode must indicate the window is visible (`Frames::mode.is_visible`).
    VisibleMode,
}

/// Errors surfaced when waiting for world state transitions.
#[derive(Debug, Error)]
pub enum WaitError {
    /// The tracked window disappeared while waiting for the target condition.
    #[error(
        "window {key:?} removed while waiting for {condition} after {elapsed:?} (events={events}, lost={lost})"
    )]
    Removed {
        /// Identifier of the tracked window.
        key: WindowKey,
        /// Human-readable condition description.
        condition: &'static str,
        /// Duration spent waiting.
        elapsed: Duration,
        /// Number of relevant events observed.
        events: usize,
        /// Events dropped from the subscription buffer while waiting.
        lost: u64,
    },
    /// The event stream closed unexpectedly.
    #[error(
        "event stream closed while waiting for {condition} on {key:?} after {elapsed:?} (events={events}, lost={lost})"
    )]
    StreamClosed {
        /// Identifier of the tracked window.
        key: WindowKey,
        /// Human-readable condition description.
        condition: &'static str,
        /// Duration spent waiting.
        elapsed: Duration,
        /// Number of relevant events observed.
        events: usize,
        /// Events dropped from the subscription buffer while waiting.
        lost: u64,
    },
    /// The condition was not satisfied before the overall deadline elapsed.
    #[error(
        "timeout waiting for {condition} on {key:?} after {elapsed:?} (events={events}, lost={lost}){details}"
    )]
    Timeout {
        /// Identifier of the tracked window.
        key: WindowKey,
        /// Human-readable condition description.
        condition: &'static str,
        /// Duration spent waiting.
        elapsed: Duration,
        /// Number of relevant events observed.
        events: usize,
        /// Events dropped from the subscription buffer while waiting.
        lost: u64,
        /// Formatted summary of the last observed state.
        details: String,
    },
    /// The wait observed more events than allowed by the configuration.
    #[error(
        "exhausted {events} events while waiting for {condition} on {key:?} after {elapsed:?} (lost={lost}){details}"
    )]
    Saturated {
        /// Identifier of the tracked window.
        key: WindowKey,
        /// Human-readable condition description.
        condition: &'static str,
        /// Duration spent waiting.
        elapsed: Duration,
        /// Number of relevant events observed.
        events: usize,
        /// Events dropped from the subscription buffer while waiting.
        lost: u64,
        /// Formatted summary of the last observed state.
        details: String,
    },
    /// No matching window appeared before the overall deadline elapsed.
    #[error(
        "timeout awaiting windows matching {condition} after {elapsed:?} (events={events}, lost={lost}){details}"
    )]
    NotFound {
        /// Human-readable condition description.
        condition: &'static str,
        /// Duration spent waiting.
        elapsed: Duration,
        /// Number of relevant events observed.
        events: usize,
        /// Events dropped from the subscription buffer while waiting.
        lost: u64,
        /// Formatted summary of the last observed state.
        details: String,
    },
}

/// Observer that waits on world events for a specific window key.
pub struct WindowObserver {
    world: WorldHandle,
    cursor: EventCursor,
    key: WindowKey,
    config: WaitConfig,
    baseline_lost: u64,
}

impl WindowObserver {
    pub(crate) fn new(
        world: WorldHandle,
        key: WindowKey,
        cursor: EventCursor,
        config: WaitConfig,
    ) -> Self {
        let baseline_lost = cursor.lost_count;
        Self {
            world,
            cursor,
            key,
            config,
            baseline_lost,
        }
    }

    /// Configuration applied to this observer.
    #[must_use]
    pub const fn config(&self) -> WaitConfig {
        self.config
    }

    /// Identifier of the tracked window.
    #[must_use]
    pub const fn key(&self) -> WindowKey {
        self.key
    }

    /// Wait until the provided predicate over `Frames` evaluates to true.
    pub async fn wait_for_frames<F>(
        &mut self,
        condition: &'static str,
        mut predicate: F,
    ) -> Result<Frames, WaitError>
    where
        F: FnMut(&Frames) -> bool + Send,
    {
        self.wait_for_frames_inner(condition, move |frames| predicate(frames))
            .await
    }

    /// Wait until the authoritative frame matches `expected` within `eps` pixels.
    pub async fn wait_for_rect(
        &mut self,
        expected: crate::RectPx,
        eps: i32,
    ) -> Result<Frames, WaitError> {
        self.wait_for_frames_inner("frame-match", move |frames| {
            rect_within_eps(&frames.authoritative, &expected, eps)
        })
        .await
    }

    /// Wait until the tracked window reports the expected [`WindowMode`].
    pub async fn wait_for_mode(&mut self, expected: WindowMode) -> Result<Frames, WaitError> {
        self.wait_for_frames_inner("mode", move |frames| frames.mode == expected)
            .await
    }

    /// Wait until visibility criteria are satisfied.
    pub async fn wait_for_visibility(
        &mut self,
        policy: VisibilityPolicy,
    ) -> Result<WorldWindow, WaitError> {
        self.wait_for_window_inner("visibility", move |window, frames| match policy {
            VisibilityPolicy::OnScreen => window.is_on_screen,
            VisibilityPolicy::OnScreenAndActive => window.is_on_screen && window.on_active_space,
            VisibilityPolicy::OnActiveSpace => window.on_active_space,
            VisibilityPolicy::VisibleMode => frames.is_some_and(|f| f.mode.is_visible()),
        })
        .await
    }

    async fn wait_for_frames_inner<F>(
        &mut self,
        condition: &'static str,
        mut predicate: F,
    ) -> Result<Frames, WaitError>
    where
        F: FnMut(&Frames) -> bool + Send,
    {
        let start = Instant::now();
        let mut events = 0usize;
        let overall_deadline = start + self.config.overall;
        let mut last_frames: Option<Frames> = None;

        loop {
            if let Some(frames) = self.world.frames(self.key).await {
                if predicate(&frames) {
                    return Ok(frames);
                }
                last_frames = Some(frames.clone());
            }

            let wait_result = self
                .wait_for_event(condition, start, overall_deadline, events)
                .await?;
            match wait_result {
                WaitProgress::Advanced => {
                    events += 1;
                    if events >= self.config.max_events {
                        return Err(WaitError::Saturated {
                            key: self.key,
                            condition,
                            elapsed: start.elapsed(),
                            events,
                            lost: self.lost_since_baseline(),
                            details: format_detail(last_frames.as_ref(), None),
                        });
                    }
                }
                WaitProgress::Idle => {
                    continue;
                }
                WaitProgress::Expired => {
                    return Err(WaitError::Timeout {
                        key: self.key,
                        condition,
                        elapsed: start.elapsed(),
                        events,
                        lost: self.lost_since_baseline(),
                        details: format_detail(last_frames.as_ref(), None),
                    });
                }
            }
        }
    }

    async fn wait_for_window_inner<F>(
        &mut self,
        condition: &'static str,
        mut predicate: F,
    ) -> Result<WorldWindow, WaitError>
    where
        F: FnMut(&WorldWindow, Option<&Frames>) -> bool + Send,
    {
        let start = Instant::now();
        let mut events = 0usize;
        let overall_deadline = start + self.config.overall;
        let mut last_window: Option<WorldWindow> = None;
        let mut last_frames: Option<Frames> = None;

        loop {
            let window_opt = self.world.get(self.key).await;
            let frames_opt = self.world.frames(self.key).await;
            match window_opt {
                Some(window) => {
                    if predicate(&window, frames_opt.as_ref()) {
                        return Ok(window);
                    }
                    last_window = Some(window);
                    last_frames = frames_opt;
                }
                None => {
                    // Fall through to event wait so we can detect removals explicitly.
                }
            }

            let wait_result = self
                .wait_for_event(condition, start, overall_deadline, events)
                .await?;
            match wait_result {
                WaitProgress::Advanced => {
                    events += 1;
                    if events >= self.config.max_events {
                        return Err(WaitError::Saturated {
                            key: self.key,
                            condition,
                            elapsed: start.elapsed(),
                            events,
                            lost: self.lost_since_baseline(),
                            details: format_detail(last_frames.as_ref(), last_window.as_ref()),
                        });
                    }
                }
                WaitProgress::Idle => {
                    continue;
                }
                WaitProgress::Expired => {
                    return Err(WaitError::Timeout {
                        key: self.key,
                        condition,
                        elapsed: start.elapsed(),
                        events,
                        lost: self.lost_since_baseline(),
                        details: format_detail(last_frames.as_ref(), last_window.as_ref()),
                    });
                }
            }
        }
    }

    async fn wait_for_event(
        &mut self,
        condition: &'static str,
        start: Instant,
        overall_deadline: Instant,
        events: usize,
    ) -> Result<WaitProgress, WaitError> {
        let mut now = Instant::now();
        if now >= overall_deadline {
            return Ok(WaitProgress::Expired);
        }
        let remaining = overall_deadline.saturating_duration_since(now);
        let pump_slice = MAIN_PUMP_SLICE.min(remaining);
        let pump_deadline = now + pump_slice;
        self.world.pump_main_until(pump_deadline);
        self.world.hint_refresh();

        now = Instant::now();
        if now >= overall_deadline {
            return Ok(WaitProgress::Expired);
        }
        let wait_for = self
            .config
            .idle
            .min(overall_deadline.saturating_duration_since(now));
        if wait_for.is_zero() {
            return Ok(WaitProgress::Idle);
        }
        let deadline = TokioInstant::now() + wait_for;
        match self
            .world
            .next_event_until(&mut self.cursor, deadline)
            .await
        {
            Some(event) => {
                if let WorldEvent::Removed(key) = event
                    && key == self.key
                {
                    return Err(WaitError::Removed {
                        key: self.key,
                        condition,
                        elapsed: start.elapsed(),
                        events,
                        lost: self.lost_since_baseline(),
                    });
                }
                Ok(WaitProgress::Advanced)
            }
            None => {
                if self.cursor.is_closed() {
                    return Err(WaitError::StreamClosed {
                        key: self.key,
                        condition,
                        elapsed: start.elapsed(),
                        events,
                        lost: self.lost_since_baseline(),
                    });
                }
                Ok(WaitProgress::Idle)
            }
        }
    }

    fn lost_since_baseline(&self) -> u64 {
        self.cursor.lost_count.saturating_sub(self.baseline_lost)
    }
}

enum WaitProgress {
    Advanced,
    Idle,
    Expired,
}

fn format_detail(frames: Option<&Frames>, window: Option<&WorldWindow>) -> String {
    let mut detail = String::new();
    if let Some(frames) = frames {
        let rect = frames.authoritative;
        let _ = write!(
            detail,
            " frame=<{}, {}, {}, {}> mode={:?} scale={:.2}",
            rect.x, rect.y, rect.w, rect.h, frames.mode, frames.scale
        );
    }
    if let Some(window) = window {
        let _ = write!(
            detail,
            " window=on_screen:{} active:{} focused:{}",
            window.is_on_screen, window.on_active_space, window.focused
        );
    }
    if detail.is_empty() {
        String::new()
    } else {
        format!(" last={detail}")
    }
}

fn rect_within_eps(actual: &crate::RectPx, expected: &crate::RectPx, eps: i32) -> bool {
    let delta = expected.delta(actual);
    delta.dx.abs() <= eps && delta.dy.abs() <= eps && delta.dw.abs() <= eps && delta.dh.abs() <= eps
}

fn event_filter_for_key(key: WindowKey) -> EventFilter {
    Arc::new(move |event: &WorldEvent| match event {
        WorldEvent::Added(window) => window.pid == key.pid && window.id == key.id,
        WorldEvent::Updated(event_key, _) => *event_key == key,
        WorldEvent::Removed(event_key) => *event_key == key,
        _ => false,
    })
}

pub(crate) fn make_window_observer(
    world: &WorldHandle,
    key: WindowKey,
    config: WaitConfig,
) -> WindowObserver {
    let filter = event_filter_for_key(key);
    let cursor = world.subscribe_with_filter(filter);
    WindowObserver::new(world.clone(), key, cursor, config)
}

pub(crate) fn make_window_watcher_filter<F>(predicate: Arc<F>) -> EventFilter
where
    F: Fn(&WorldWindow) -> bool + Send + Sync + 'static,
{
    Arc::new(move |event| match event {
        WorldEvent::Added(window) => predicate(window),
        _ => false,
    })
}

/// Wait for a window satisfying `predicate` to appear.
pub async fn await_window_matching<F>(
    world: &WorldHandle,
    predicate: Arc<F>,
    condition: &'static str,
    config: WaitConfig,
) -> Result<WorldWindow, WaitError>
where
    F: Fn(&WorldWindow) -> bool + Send + Sync + 'static,
{
    let snapshot = world.snapshot().await;
    if let Some(window) = snapshot.into_iter().find(|window| predicate(window)) {
        return Ok(window);
    }

    let mut cursor = world.subscribe_with_filter(make_window_watcher_filter(predicate));
    let start = Instant::now();
    let mut events = 0usize;
    let baseline_lost = cursor.lost_count;
    let overall_deadline = start + config.overall;

    loop {
        let now = Instant::now();
        if now >= overall_deadline {
            return Err(WaitError::NotFound {
                condition,
                elapsed: start.elapsed(),
                events,
                lost: cursor.lost_count.saturating_sub(baseline_lost),
                details: String::new(),
            });
        }
        let wait_for = config
            .idle
            .min(overall_deadline.saturating_duration_since(now));
        let deadline = TokioInstant::now() + wait_for;
        match world.next_event_until(&mut cursor, deadline).await {
            Some(WorldEvent::Added(window)) => {
                let window = *window;
                return Ok(window);
            }
            Some(_) => {
                events += 1;
                if events >= config.max_events {
                    return Err(WaitError::NotFound {
                        condition,
                        elapsed: start.elapsed(),
                        events,
                        lost: cursor.lost_count.saturating_sub(baseline_lost),
                        details: String::from(" exhausted events without match"),
                    });
                }
            }
            None => {
                if cursor.is_closed() {
                    return Err(WaitError::NotFound {
                        condition,
                        elapsed: start.elapsed(),
                        events,
                        lost: cursor.lost_count.saturating_sub(baseline_lost),
                        details: String::from(" stream closed"),
                    });
                }
            }
        }
    }
}
