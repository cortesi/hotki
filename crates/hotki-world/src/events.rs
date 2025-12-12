use tokio::{
    sync::broadcast,
    time::{Instant as TokioInstant, timeout_at},
};

use crate::WorldEvent;

/// Default broadcast buffer capacity for world events.
pub(crate) const DEFAULT_EVENT_CAPACITY: usize = 16_384;

/// Cursor tracking progress through a subscription stream.
pub struct EventCursor {
    /// Total number of events dropped for this cursor due to lag.
    pub lost_count: u64,
    receiver: broadcast::Receiver<WorldEvent>,
    closed: bool,
}

impl EventCursor {
    pub(crate) fn new(receiver: broadcast::Receiver<WorldEvent>) -> Self {
        Self {
            lost_count: 0,
            receiver,
            closed: false,
        }
    }

    /// True when the underlying stream has been closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

/// Lightweight event fan-out using a Tokio broadcast channel.
pub(crate) struct EventHub {
    sender: broadcast::Sender<WorldEvent>,
}

impl EventHub {
    /// Create a new hub with the given channel capacity.
    pub(crate) fn new(capacity: usize) -> Self {
        let capacity = capacity.max(8);
        let (sender, _rx) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Subscribe to events.
    pub(crate) fn subscribe(&self) -> EventCursor {
        EventCursor::new(self.sender.subscribe())
    }

    /// Publish an event to all subscribers.
    pub(crate) fn publish(&self, event: WorldEvent) {
        self.sender.send(event).ok();
    }

    /// Await the next event until the given deadline, returning `None` on timeout or close.
    pub(crate) async fn next_event_until(
        &self,
        cursor: &mut EventCursor,
        deadline: TokioInstant,
    ) -> Option<WorldEvent> {
        loop {
            match timeout_at(deadline, cursor.receiver.recv()).await {
                Ok(Ok(event)) => return Some(event),
                Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                    cursor.lost_count = cursor.lost_count.saturating_add(n);
                    continue;
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    cursor.closed = true;
                    return None;
                }
                Err(_) => return None,
            }
        }
    }
}
