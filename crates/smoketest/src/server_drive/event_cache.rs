//! Local cache of asynchronous server events observed by the driver.

use std::{
    collections::{BTreeSet, VecDeque},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use hotki_protocol::MsgToUI;

use super::{
    DriverEventRecord, HudSnapshot,
    types::{DriverEventId, canonicalize_ident},
};

/// Local event cache and latest-HUD index.
pub(super) struct EventCache {
    /// Maximum number of recent events retained for diagnostics.
    capacity: usize,
    /// Circular buffer of recent server events.
    buffer: VecDeque<DriverEventRecord>,
    /// Latest HUD snapshot emitted by the server.
    latest_hud: Option<HudSnapshot>,
    /// Next local event id assigned to an observed server event.
    next_event_id: DriverEventId,
}

impl EventCache {
    /// Construct an empty event cache with a fixed retained-event capacity.
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            buffer: VecDeque::new(),
            latest_hud: None,
            next_event_id: 0,
        }
    }

    /// Clear server-derived cached state while preserving monotonic local event ids.
    pub(super) fn clear(&mut self) {
        self.buffer.clear();
        self.latest_hud = None;
    }

    /// Record one asynchronous server event into the local caches.
    pub(super) fn record(&mut self, payload: MsgToUI) -> DriverEventRecord {
        let id = self.next_event_id;
        self.next_event_id = self.next_event_id.wrapping_add(1);
        let timestamp_ms = now_millis();

        if let MsgToUI::HudUpdate { hud, displays } = &payload {
            let idents: BTreeSet<String> = hud
                .rows
                .iter()
                .map(|row| canonicalize_ident(&row.chord.to_string()))
                .collect();
            self.latest_hud = Some(HudSnapshot {
                event_id: id,
                received_ms: timestamp_ms,
                hud: (**hud).clone(),
                displays: displays.clone(),
                idents,
            });
        }

        if self.buffer.len() >= self.capacity {
            self.buffer.pop_front();
        }
        let record = DriverEventRecord {
            id,
            timestamp_ms,
            payload,
        };
        self.buffer.push_back(record.clone());
        record
    }

    /// Return the latest HUD snapshot observed on the server event stream.
    pub(super) fn latest_hud(&self) -> Option<HudSnapshot> {
        self.latest_hud.clone()
    }

    /// Return the local event id of the latest HUD update.
    pub(super) fn latest_hud_event_id(&self) -> Option<DriverEventId> {
        self.latest_hud.as_ref().map(|snapshot| snapshot.event_id)
    }

    /// Return the event id that will be assigned to the next observed event.
    pub(super) fn cursor(&self) -> DriverEventId {
        self.next_event_id
    }

    /// Find a retained event at or after `cursor` matching `predicate`.
    pub(super) fn find_since<F>(
        &self,
        cursor: DriverEventId,
        mut predicate: F,
    ) -> Option<DriverEventRecord>
    where
        F: FnMut(&MsgToUI) -> bool,
    {
        self.buffer
            .iter()
            .find(|record| record.id >= cursor && predicate(&record.payload))
            .cloned()
    }

    /// Check whether the cached HUD snapshot contains every requested identifier.
    pub(super) fn hud_contains_all(&self, want: &BTreeSet<String>) -> bool {
        if want.is_empty() {
            return true;
        }
        self.latest_hud
            .as_ref()
            .map(|snapshot| want.is_subset(&snapshot.idents))
            .unwrap_or(false)
    }

    /// Return canonicalized identifiers from the latest HUD snapshot.
    pub(super) fn latest_hud_idents(&self) -> BTreeSet<String> {
        self.latest_hud
            .as_ref()
            .map(|snapshot| snapshot.idents.clone())
            .unwrap_or_default()
    }

    /// Number of retained events.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Return the last retained payload.
    #[cfg(test)]
    pub(super) fn back_payload(&self) -> Option<&MsgToUI> {
        self.buffer.back().map(|record| &record.payload)
    }
}

/// Return the current wall-clock timestamp in milliseconds since the Unix epoch.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_returns_new_record_when_buffer_is_full() {
        let mut cache = EventCache::new(128);
        for tick in 0..128 {
            cache.record(MsgToUI::Heartbeat(tick));
        }

        let record = cache.record(MsgToUI::Heartbeat(999));

        assert_eq!(record.id, 128);
        assert_eq!(record.payload, MsgToUI::Heartbeat(999));
        assert_eq!(cache.len(), 128);
        assert_eq!(cache.back_payload(), Some(&MsgToUI::Heartbeat(999)));
    }

    #[test]
    fn clear_drops_cached_events_but_preserves_event_ids() {
        let mut cache = EventCache::new(128);
        cache.record(MsgToUI::Heartbeat(1));

        cache.clear();
        let record = cache.record(MsgToUI::Heartbeat(2));

        assert_eq!(record.id, 1);
        assert_eq!(cache.len(), 1);
    }
}
