use std::{collections::HashMap, sync::Arc};

use mac_keycode::Chord;
use parking_lot::Mutex;
use relaykey::RelayDestination;
use tracing::trace;

#[derive(Clone)]
struct ActiveRelay {
    chord: Chord,
    destination: RelayDestination,
}

/// Backend responsible for posting relayed key events.
pub(crate) trait RelayPoster: Send + Sync {
    /// Post a key-down or repeat event.
    fn key_down(
        &self,
        chord: &Chord,
        is_repeat: bool,
        destination: RelayDestination,
    ) -> relaykey::Result<()>;
    /// Post a key-up event.
    fn key_up(&self, chord: &Chord, destination: RelayDestination) -> relaykey::Result<()>;
}

impl RelayPoster for relaykey::RelayKey {
    fn key_down(
        &self,
        chord: &Chord,
        is_repeat: bool,
        destination: RelayDestination,
    ) -> relaykey::Result<()> {
        relaykey::RelayKey::key_down(self, chord, is_repeat, destination)
    }

    fn key_up(&self, chord: &Chord, destination: RelayDestination) -> relaykey::Result<()> {
        relaykey::RelayKey::key_up(self, chord, destination)
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
                if let Err(error) = poster.key_up(&relay.chord, relay.destination) {
                    tracing::warn!(?error, "relay_stop_all_up_failed");
                }
                trace!(?relay.destination, id = %id, "relay_stop_all_up");
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

/// Relay handler that pins each key gesture to one delivery destination.
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

    /// Start relaying a chord to a destination (posts an initial KeyDown).
    pub fn start_relay(
        &self,
        id: String,
        chord: Chord,
        destination: RelayDestination,
        is_repeat: bool,
    ) {
        if let Some(poster) = &self.inner.poster
            && let Err(error) = poster.key_down(&chord, is_repeat, destination)
        {
            tracing::warn!(?error, "relay_down_failed");
        }
        trace!(?destination, id = %id, "relay_start");
        self.inner
            .active
            .lock()
            .insert(id, ActiveRelay { chord, destination });
    }

    /// Repeat relay for an active id (posts a repeat KeyDown).
    pub fn repeat_relay(&self, id: &str) -> bool {
        if let Some(active) = self.inner.active.lock().get(id).cloned() {
            if let Some(poster) = &self.inner.poster
                && let Err(error) = poster.key_down(&active.chord, true, active.destination)
            {
                tracing::warn!(?error, "relay_repeat_failed");
            }
            true
        } else {
            false
        }
    }

    /// Stop relaying for id (posts KeyUp and clears state).
    pub fn stop_relay(&self, id: &str) -> bool {
        if let Some(active) = self.inner.active.lock().remove(id) {
            if let Some(poster) = &self.inner.poster
                && let Err(error) = poster.key_up(&active.chord, active.destination)
            {
                tracing::warn!(?error, "relay_up_failed");
            }
            trace!(?active.destination, id = %id, "relay_stop");
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
        sync::{
            Mutex as StdMutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use mac_keycode::Key;

    use super::*;

    #[derive(Default)]
    struct CountingPoster {
        downs: AtomicUsize,
        repeat_downs: AtomicUsize,
        ups: AtomicUsize,
        destinations: StdMutex<Vec<RelayDestination>>,
    }

    impl RelayPoster for CountingPoster {
        fn key_down(
            &self,
            _chord: &Chord,
            is_repeat: bool,
            destination: RelayDestination,
        ) -> relaykey::Result<()> {
            self.downs.fetch_add(1, Ordering::SeqCst);
            if is_repeat {
                self.repeat_downs.fetch_add(1, Ordering::SeqCst);
            }
            self.destinations
                .lock()
                .expect("destinations lock")
                .push(destination);
            Ok(())
        }

        fn key_up(&self, _chord: &Chord, destination: RelayDestination) -> relaykey::Result<()> {
            self.ups.fetch_add(1, Ordering::SeqCst);
            self.destinations
                .lock()
                .expect("destinations lock")
                .push(destination);
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
        let destination = RelayDestination::Process(1234);

        handler.start_relay(id.clone(), chord(Key::A), destination, false);
        assert!(handler.repeat_relay(&id));
        assert!(handler.stop_relay(&id));
        assert!(!handler.stop_relay(&id));

        assert_eq!(poster.downs.load(Ordering::SeqCst), 2);
        assert_eq!(poster.repeat_downs.load(Ordering::SeqCst), 1);
        assert_eq!(poster.ups.load(Ordering::SeqCst), 1);
        assert_eq!(
            *poster.destinations.lock().expect("destinations lock"),
            vec![destination, destination, destination]
        );
    }

    struct FailingPoster;

    impl RelayPoster for FailingPoster {
        fn key_down(
            &self,
            _chord: &Chord,
            _is_repeat: bool,
            _destination: RelayDestination,
        ) -> relaykey::Result<()> {
            Err(relaykey::Error::EventCreate)
        }

        fn key_up(&self, _chord: &Chord, _destination: RelayDestination) -> relaykey::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn failed_start_remains_releasable() {
        let handler = RelayHandler::new_with_poster(Some(Arc::new(FailingPoster)));
        let id = "failed-relay".to_string();

        handler.start_relay(
            id.clone(),
            chord(Key::A),
            RelayDestination::Process(1234),
            false,
        );

        assert!(handler.repeat_relay(&id));
        assert!(handler.stop_relay(&id));
        assert!(!handler.repeat_relay(&id));
    }

    #[test]
    fn dropping_clone_does_not_release_active_relay() {
        let poster = Arc::new(CountingPoster::default());
        let handler = handler(poster.clone());
        handler.start_relay(
            "id1".to_string(),
            chord(Key::A),
            RelayDestination::Process(1234),
            false,
        );

        drop(handler.clone());

        assert_eq!(poster.ups.load(Ordering::SeqCst), 0);
        assert!(handler.inner.active.lock().contains_key("id1"));
    }

    #[test]
    fn dropping_last_handle_releases_active_relays() {
        let poster = Arc::new(CountingPoster::default());
        let handler = handler(poster.clone());
        handler.start_relay(
            "id1".to_string(),
            chord(Key::A),
            RelayDestination::Process(1234),
            false,
        );
        handler.start_relay(
            "id2".to_string(),
            chord(Key::B),
            RelayDestination::Process(1234),
            false,
        );

        drop(handler);

        assert_eq!(poster.ups.load(Ordering::SeqCst), 2);
    }
}
