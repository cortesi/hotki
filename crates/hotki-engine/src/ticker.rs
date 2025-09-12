//! Ticker for scheduling repeated actions with cancellation support.
//!
//! Provides a task scheduling system that runs callbacks after an initial delay
//! and then on regular intervals. Supports immediate cancellation and bounded
//! wait times for task completion.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        mpsc::{Receiver, channel},
    },
    thread,
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use tokio::time::{self, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tracing::trace;

use crate::repeater::STOP_WAIT_TIMEOUT_MS;

/// Poll interval used when waiting for ticker tasks to finish.
pub const STOP_POLL_INTERVAL_MS: u64 = 2;

struct TickerEntry {
    token: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
    done_rx: Receiver<()>,
}

/// Minimal ticker core: schedules a closure after an initial delay and then on each interval tick.
/// Supports cancellation and a short stop_sync wait for completion.
#[derive(Clone)]
pub struct Ticker {
    entries: Arc<Mutex<HashMap<String, TickerEntry>>>,
}

impl Default for Ticker {
    fn default() -> Self {
        Self::new()
    }
}

impl Ticker {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check if a ticker is active for the given id.
    pub fn is_active(&self, id: &str) -> bool {
        self.entries.lock().contains_key(id)
    }

    /// Start or replace a ticker for `id` with given timings and on_tick closure.
    pub fn start<F>(&self, id: String, initial: Duration, interval: Duration, mut on_tick: F)
    where
        F: FnMut() + Send + 'static,
    {
        // Replace any existing ticker for this id
        self.stop(&id);

        let token = CancellationToken::new();
        let cancel = token.clone();
        let id_for_log = id.clone();
        let (done_tx, done_rx) = channel::<()>();

        let fut = async move {
            trace!(
                "ticker_start" = %id_for_log,
                init_ms = initial.as_millis(),
                int_ms = interval.as_millis()
            );

            // Initial delay with cancellation
            tokio::select! {
                _ = time::sleep(initial) => {}
                _ = cancel.cancelled() => {
                    trace!("ticker_cancelled_initial" = %id_for_log);
                    let _ = done_tx.send(());
                    return;
                }
            }

            let mut ticker = time::interval(interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        trace!("ticker_cancelled" = %id_for_log);
                        let _ = done_tx.send(());
                        return;
                    }
                    _ = ticker.tick() => {
                        on_tick();
                    }
                }
            }
        };

        let handle = tokio::spawn(fut);
        let entry = TickerEntry {
            token,
            handle,
            done_rx,
        };
        self.entries.lock().insert(id, entry);
    }

    /// Stop a ticker if present (non-blocking).
    pub fn stop(&self, id: &str) {
        if let Some(entry) = self.entries.lock().remove(id) {
            entry.token.cancel();
            // Don't abort the handle, let it cancel gracefully via the token
            trace!("ticker_stop" = %id);
        }
    }

    /// Stop a ticker if present and wait briefly for completion signal (blocking).
    pub fn stop_sync(&self, id: &str) {
        if let Some(entry) = self.entries.lock().remove(id) {
            entry.token.cancel();
            // Prefer a real blocking timeout via std::sync::mpsc
            let deadline = Duration::from_millis(STOP_WAIT_TIMEOUT_MS);
            let _ = entry.done_rx.recv_timeout(deadline);
            // As a backup, if the task has already finished but no signal received, quickly poll handle
            let handle = entry.handle;
            let start = Instant::now();
            while !handle.is_finished()
                && start.elapsed() < Duration::from_millis(STOP_WAIT_TIMEOUT_MS)
            {
                thread::sleep(Duration::from_millis(STOP_POLL_INTERVAL_MS));
            }
            trace!("ticker_stop_sync" = %id);
        }
    }

    /// Cancel and wait for all tickers to finish (blocking).
    pub fn clear_sync(&self) {
        let entries: Vec<TickerEntry> = {
            let mut map = self.entries.lock();
            map.drain().map(|(_, e)| e).collect()
        };

        // Cancel all tokens first
        for e in &entries {
            e.token.cancel();
        }

        // Wait for completion signals (blocking timeout), then backstop with quick handle polls.
        let mut handles = Vec::new();
        for e in entries {
            let _ = e
                .done_rx
                .recv_timeout(Duration::from_millis(STOP_WAIT_TIMEOUT_MS));
            handles.push(e.handle);
        }
        let deadline = Instant::now() + Duration::from_millis(STOP_WAIT_TIMEOUT_MS);
        for handle in handles {
            while !handle.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(STOP_POLL_INTERVAL_MS));
            }
        }
        trace!("ticker_clear_sync");
    }

    /// Cancel and wait for all tickers to finish (async).
    pub async fn clear_async(&self) {
        let entries: Vec<TickerEntry> = {
            let mut map = self.entries.lock();
            map.drain().map(|(_, e)| e).collect()
        };

        // Cancel all tokens first
        for e in &entries {
            e.token.cancel();
        }

        // Await each handle with a timeout
        for e in entries {
            let _ =
                tokio::time::timeout(Duration::from_millis(STOP_WAIT_TIMEOUT_MS), e.handle).await;
        }
        trace!("ticker_clear_async");
    }
}
