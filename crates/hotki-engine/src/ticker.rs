//! Ticker for scheduling repeated actions with cancellation support.
//!
//! Provides owned tasks that run callbacks after an initial delay and then on
//! regular intervals. Async stop paths await graceful cancellation; synchronous
//! replacement and last-owner cleanup abort tasks after cancelling them.

use std::{collections::HashMap, sync::Arc, time::Duration};

use parking_lot::Mutex;
use tokio::{
    task::JoinHandle,
    time::{self, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;
use tracing::trace;

struct TickerEntry {
    token: CancellationToken,
    handle: JoinHandle<()>,
}

#[derive(Default)]
struct TickerInner {
    entries: Mutex<HashMap<String, TickerEntry>>,
}

impl TickerInner {
    fn drain(&self) -> Vec<TickerEntry> {
        self.entries
            .lock()
            .drain()
            .map(|(_, entry)| entry)
            .collect()
    }
}

impl Drop for TickerInner {
    fn drop(&mut self) {
        for entry in self.entries.get_mut().drain().map(|(_, entry)| entry) {
            entry.token.cancel();
            entry.handle.abort();
        }
    }
}

/// Schedules repeat callbacks and owns their worker tasks.
#[derive(Clone, Default)]
pub struct Ticker {
    inner: Arc<TickerInner>,
}

impl Ticker {
    /// Check if a ticker is active for the given id.
    pub fn is_active(&self, id: &str) -> bool {
        self.inner.entries.lock().contains_key(id)
    }

    /// Start or replace a ticker for `id` with given timings and on-tick closure.
    pub fn start<F>(&self, id: String, initial: Duration, interval: Duration, mut on_tick: F)
    where
        F: FnMut() + Send + 'static,
    {
        self.abort(&id);

        let token = CancellationToken::new();
        let cancel = token.clone();
        let id_for_log = id.clone();
        let handle = tokio::spawn(async move {
            trace!(
                "ticker_start" = %id_for_log,
                init_ms = initial.as_millis(),
                int_ms = interval.as_millis()
            );

            tokio::select! {
                _ = time::sleep(initial) => {}
                _ = cancel.cancelled() => {
                    trace!("ticker_cancelled_initial" = %id_for_log);
                    return;
                }
            }

            let mut ticker = time::interval(interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        trace!("ticker_cancelled" = %id_for_log);
                        return;
                    }
                    _ = ticker.tick() => on_tick(),
                }
            }
        });

        self.inner
            .entries
            .lock()
            .insert(id, TickerEntry { token, handle });
    }

    /// Cancel a ticker and wait for its task to finish.
    pub async fn stop(&self, id: &str) {
        let entry = self.inner.entries.lock().remove(id);
        if let Some(entry) = entry {
            entry.token.cancel();
            if let Err(error) = entry.handle.await {
                tracing::warn!(?error, %id, "ticker_task_failed");
            }
            trace!("ticker_stop" = %id);
        }
    }

    /// Cancel and abort a ticker when the caller cannot await task completion.
    pub(crate) fn abort(&self, id: &str) {
        if let Some(entry) = self.inner.entries.lock().remove(id) {
            entry.token.cancel();
            entry.handle.abort();
            trace!("ticker_abort" = %id);
        }
    }

    /// Cancel and abort every ticker when the caller cannot await completion.
    pub(crate) fn abort_all(&self) {
        for entry in self.inner.drain() {
            entry.token.cancel();
            entry.handle.abort();
        }
        trace!("ticker_abort_all");
    }

    /// Cancel all tickers and wait for their tasks to finish.
    pub async fn clear_async(&self) {
        let entries = self.inner.drain();
        for entry in &entries {
            entry.token.cancel();
        }
        for entry in entries {
            if let Err(error) = entry.handle.await {
                tracing::warn!(?error, "ticker_task_failed_during_clear");
            }
        }
        trace!("ticker_clear_async");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::time::advance;

    use super::*;

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn stop_awaits_task_and_prevents_ticks() {
        let ticks = Arc::new(AtomicUsize::new(0));
        let ticker = Ticker::default();
        let observed = ticks.clone();
        ticker.start(
            "held".to_string(),
            Duration::from_millis(100),
            Duration::from_millis(100),
            move || {
                observed.fetch_add(1, Ordering::SeqCst);
            },
        );
        tokio::task::yield_now().await;

        ticker.stop("held").await;
        advance(Duration::from_secs(1)).await;

        assert_eq!(ticks.load(Ordering::SeqCst), 0);
        assert!(!ticker.is_active("held"));
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn dropping_clone_does_not_cancel_task() {
        let ticks = Arc::new(AtomicUsize::new(0));
        let ticker = Ticker::default();
        let remaining = ticker.clone();
        let observed = ticks.clone();
        ticker.start(
            "held".to_string(),
            Duration::from_millis(100),
            Duration::from_millis(100),
            move || {
                observed.fetch_add(1, Ordering::SeqCst);
            },
        );
        tokio::task::yield_now().await;

        drop(ticker);
        advance(Duration::from_millis(100)).await;
        tokio::task::yield_now().await;

        assert_eq!(ticks.load(Ordering::SeqCst), 1);
        remaining.clear_async().await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn dropping_last_handle_releases_inner_state() {
        let ticker = Ticker::default();
        let inner = Arc::downgrade(&ticker.inner);
        ticker.start(
            "held".to_string(),
            Duration::from_secs(1),
            Duration::from_secs(1),
            || {},
        );

        drop(ticker);

        assert!(inner.upgrade().is_none());
    }
}
