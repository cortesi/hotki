use std::{collections::HashMap, sync::Arc};

use mac_keycode::Chord;
use parking_lot::Mutex;
use tracing::trace;

#[derive(Clone)]
struct ActiveRelay {
    chord: Chord,
    pid: i32,
}

/// Backend responsible for posting relayed key events.
pub(crate) trait RelayPoster: Send + Sync {
    /// Post a key-down or repeat event.
    fn key_down(&self, chord: &Chord, is_repeat: bool) -> relaykey::Result<()>;
    /// Post a key-up event.
    fn key_up(&self, chord: &Chord) -> relaykey::Result<()>;
}

impl RelayPoster for relaykey::RelayKey {
    fn key_down(&self, chord: &Chord, is_repeat: bool) -> relaykey::Result<()> {
        relaykey::RelayKey::key_down(self, chord, is_repeat)
    }

    fn key_up(&self, chord: &Chord) -> relaykey::Result<()> {
        relaykey::RelayKey::key_up(self, chord)
    }
}

/// Uniquely dropped relay state shared by lightweight handles.
struct RelayInner {
    active: Mutex<HashMap<String, ActiveRelay>>,
    poster: Option<Arc<dyn RelayPoster>>,
}

impl RelayInner {
    /// Release every active relay while retaining the inner allocation.
    fn stop_all(&self) {
        let mut active = self.active.lock();
        if let Some(poster) = &self.poster {
            for (id, relay) in active.drain() {
                if let Err(error) = poster.key_up(&relay.chord) {
                    tracing::warn!(?error, "relay_stop_all_up_failed");
                }
                trace!(pid = relay.pid, id = %id, "relay_stop_all_up");
            }
        } else {
            active.clear();
        }
    }
}

impl Drop for RelayInner {
    fn drop(&mut self) {
        self.stop_all();
    }
}

/// Relay handler that forwards key events to the focused process.
#[derive(Clone)]
pub struct RelayHandler {
    /// Last-owner state; cloning this handle never implies cleanup.
    inner: Arc<RelayInner>,
}

impl RelayHandler {
    /// Create a new relay handler with relay enabled/disabled.
    pub fn new_with_enabled(enabled: bool) -> Self {
        let poster = enabled.then(|| Arc::new(relaykey::RelayKey::new()) as Arc<dyn RelayPoster>);
        Self::new_with_poster(poster)
    }

    /// Create a relay handler using the provided event poster.
    pub(crate) fn new_with_poster(poster: Option<Arc<dyn RelayPoster>>) -> Self {
        Self {
            inner: Arc::new(RelayInner {
                active: Mutex::new(HashMap::new()),
                poster,
            }),
        }
    }

    /// Start relaying a chord to a pid (posts an initial KeyDown).
    pub fn start_relay(&self, id: String, chord: Chord, pid: i32, is_repeat: bool) {
        if let Some(poster) = &self.inner.poster
            && let Err(error) = poster.key_down(&chord, is_repeat)
        {
            tracing::warn!(?error, "relay_down_failed");
        }
        trace!(pid, id = %id, "relay_start");
        self.inner
            .active
            .lock()
            .insert(id, ActiveRelay { chord, pid });
    }

    /// Repeat relay for an active id (posts a repeat KeyDown).
    pub fn repeat_relay(&self, id: &str) -> bool {
        if let Some(active) = self.inner.active.lock().get(id).cloned() {
            if let Some(poster) = &self.inner.poster
                && let Err(error) = poster.key_down(&active.chord, true)
            {
                tracing::warn!(?error, "relay_repeat_failed");
            }
            true
        } else {
            false
        }
    }

    /// Stop relaying for id (posts KeyUp and clears state).
    pub fn stop_relay(&self, id: &str, pid: i32) -> bool {
        if let Some(active) = self.inner.active.lock().remove(id) {
            let target_pid = if active.pid != -1 { active.pid } else { pid };
            if let Some(poster) = &self.inner.poster
                && let Err(error) = poster.key_up(&active.chord)
            {
                tracing::warn!(?error, "relay_up_failed");
            }
            trace!(pid = target_pid, id = %id, "relay_stop");
            true
        } else {
            false
        }
    }

    /// Stop all relays (posts KeyUp for each active id, best-effort).
    pub fn stop_all(&self) {
        self.inner.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use mac_keycode::Key;

    use super::*;

    #[derive(Default)]
    struct CountingPoster {
        downs: AtomicUsize,
        repeat_downs: AtomicUsize,
        ups: AtomicUsize,
    }

    impl RelayPoster for CountingPoster {
        fn key_down(&self, _chord: &Chord, is_repeat: bool) -> relaykey::Result<()> {
            self.downs.fetch_add(1, Ordering::SeqCst);
            if is_repeat {
                self.repeat_downs.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }

        fn key_up(&self, _chord: &Chord) -> relaykey::Result<()> {
            self.ups.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn chord(key: Key) -> Chord {
        Chord {
            key,
            modifiers: HashSet::new(),
        }
    }

    fn handler(poster: Arc<CountingPoster>) -> RelayHandler {
        RelayHandler::new_with_poster(Some(poster))
    }

    #[test]
    fn start_repeat_stop_posts_balanced_events() {
        let poster = Arc::new(CountingPoster::default());
        let handler = handler(poster.clone());
        let id = "id1".to_string();

        handler.start_relay(id.clone(), chord(Key::A), 1234, false);
        assert!(handler.repeat_relay(&id));
        assert!(handler.stop_relay(&id, 1234));
        assert!(!handler.stop_relay(&id, 1234));

        assert_eq!(poster.downs.load(Ordering::SeqCst), 2);
        assert_eq!(poster.repeat_downs.load(Ordering::SeqCst), 1);
        assert_eq!(poster.ups.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropping_clone_does_not_release_active_relay() {
        let poster = Arc::new(CountingPoster::default());
        let handler = handler(poster.clone());
        handler.start_relay("id1".to_string(), chord(Key::A), 1234, false);

        drop(handler.clone());

        assert_eq!(poster.ups.load(Ordering::SeqCst), 0);
        assert!(handler.inner.active.lock().contains_key("id1"));
    }

    #[test]
    fn dropping_last_handle_releases_active_relays() {
        let poster = Arc::new(CountingPoster::default());
        let handler = handler(poster.clone());
        handler.start_relay("id1".to_string(), chord(Key::A), 1234, false);
        handler.start_relay("id2".to_string(), chord(Key::B), 1234, false);

        drop(handler);

        assert_eq!(poster.ups.load(Ordering::SeqCst), 2);
    }
}
