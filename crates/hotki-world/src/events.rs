use std::{
    collections::VecDeque,
    fmt,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use parking_lot::Mutex;
use tokio::{
    sync::Notify,
    time::{Instant as TokioInstant, timeout_at},
};

use crate::WorldEvent;

/// Default per-subscriber event ring capacity.
pub(crate) const DEFAULT_EVENT_CAPACITY: usize = 16_384;

struct EventEntry {
    seq: u64,
    event: WorldEvent,
}

struct EventBuffer {
    events: VecDeque<EventEntry>,
    lost_count: u64,
    head_seq: u64,
    next_seq: u64,
    capacity: usize,
}

impl EventBuffer {
    fn new(start_seq: u64, capacity: usize) -> Self {
        Self {
            events: VecDeque::new(),
            lost_count: 0,
            head_seq: start_seq,
            next_seq: start_seq,
            capacity,
        }
    }

    fn push(&mut self, seq: u64, event: WorldEvent) {
        if self.events.len() == self.capacity {
            self.events.pop_front();
            self.lost_count = self.lost_count.saturating_add(1);
        }
        self.events.push_back(EventEntry { seq, event });
        self.next_seq = seq.saturating_add(1);
        self.head_seq = self
            .events
            .front()
            .map(|entry| entry.seq)
            .unwrap_or(self.next_seq);
    }

    fn pop(&mut self) -> Option<EventEntry> {
        let entry = self.events.pop_front();
        self.head_seq = self
            .events
            .front()
            .map(|entry| entry.seq)
            .unwrap_or(self.next_seq);
        entry
    }
}

struct StreamInner {
    buffer: Mutex<EventBuffer>,
    notify: Notify,
    closed: AtomicBool,
}

impl StreamInner {
    fn new(start_seq: u64, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            buffer: Mutex::new(EventBuffer::new(start_seq, capacity)),
            notify: Notify::new(),
            closed: AtomicBool::new(false),
        })
    }

    fn push(&self, seq: u64, event: &WorldEvent) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let mut buffer = self.buffer.lock();
        buffer.push(seq, event.clone());
        drop(buffer);
        self.notify.notify_waiters();
    }

    fn try_next(&self, cursor: &mut EventCursor) -> Option<WorldEvent> {
        let mut buffer = self.buffer.lock();
        cursor.lost_count = buffer.lost_count;
        if cursor.next_index < buffer.head_seq {
            cursor.next_index = buffer.head_seq;
        }
        let entry = buffer.pop();
        match entry {
            Some(entry) => {
                if cursor.next_index < entry.seq {
                    cursor.next_index = entry.seq;
                }
                cursor.next_index = entry.seq.saturating_add(1);
                cursor.lost_count = buffer.lost_count;
                Some(entry.event)
            }
            None => None,
        }
    }

    fn sync_counters(&self, cursor: &mut EventCursor) {
        let buffer = self.buffer.lock();
        cursor.lost_count = buffer.lost_count;
        if cursor.next_index < buffer.head_seq {
            cursor.next_index = buffer.head_seq;
        }
    }

    fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut buffer = self.buffer.lock();
        buffer.events.clear();
        buffer.head_seq = buffer.next_seq;
        drop(buffer);
        self.notify.notify_waiters();
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

/// Cursor tracking progress through a subscription stream.
pub struct EventCursor {
    /// Global sequence number of the next event to consume.
    pub next_index: u64,
    /// Total number of events dropped for this cursor due to overflow.
    pub lost_count: u64,
    stream: Arc<StreamInner>,
}

impl EventCursor {
    fn new(stream: Arc<StreamInner>, start_index: u64) -> Self {
        Self {
            stream,
            next_index: start_index,
            lost_count: 0,
        }
    }

    /// True when the underlying stream has been closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.stream.is_closed()
    }
}

impl fmt::Debug for EventCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventCursor")
            .field("next_index", &self.next_index)
            .field("lost_count", &self.lost_count)
            .finish_non_exhaustive()
    }
}

/// Lightweight event fan-out with per-subscriber ring buffers.
pub(crate) struct EventHub {
    seq: AtomicU64,
    capacity: usize,
    subscribers: Mutex<Vec<Weak<StreamInner>>>,
}

impl EventHub {
    /// Create a new hub with the given per-subscriber capacity.
    pub(crate) fn new(capacity: usize) -> Self {
        let capacity = capacity.max(8);
        Self {
            seq: AtomicU64::new(0),
            capacity,
            subscribers: Mutex::new(Vec::new()),
        }
    }

    /// Subscribe to events.
    pub(crate) fn subscribe(&self) -> EventCursor {
        let start = self.seq.load(Ordering::SeqCst);
        let stream = StreamInner::new(start, self.capacity);
        self.subscribers.lock().push(Arc::downgrade(&stream));
        EventCursor::new(stream, start)
    }

    /// Publish an event to all subscribers.
    pub(crate) fn publish(&self, event: WorldEvent) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let mut stale = false;
        {
            let subscribers = self.subscribers.lock();
            for weak in subscribers.iter() {
                if let Some(stream) = weak.upgrade() {
                    stream.push(seq, &event);
                } else {
                    stale = true;
                }
            }
        }
        if stale {
            self.prune();
        }
    }

    /// Await the next event until the given deadline, returning `None` on timeout.
    pub(crate) async fn next_event_until(
        &self,
        cursor: &mut EventCursor,
        deadline: TokioInstant,
    ) -> Option<WorldEvent> {
        let stream = cursor.stream.clone();
        loop {
            if let Some(event) = stream.try_next(cursor) {
                return Some(event);
            }
            if stream.is_closed() {
                stream.sync_counters(cursor);
                return None;
            }
            let now = TokioInstant::now();
            if now >= deadline {
                stream.sync_counters(cursor);
                return None;
            }
            let notified = stream.notify.notified();
            if timeout_at(deadline, notified).await.is_err() {
                stream.sync_counters(cursor);
                return None;
            }
        }
    }

    fn prune(&self) {
        let mut subscribers = self.subscribers.lock();
        subscribers.retain(|weak| {
            weak.strong_count() > 0 && weak.upgrade().is_some_and(|stream| !stream.is_closed())
        })
    }
}

impl Drop for EventHub {
    fn drop(&mut self) {
        let subscribers = self.subscribers.lock();
        for weak in subscribers.iter() {
            if let Some(stream) = weak.upgrade() {
                stream.close();
            }
        }
    }
}
