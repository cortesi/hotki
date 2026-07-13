//! Bounded, message-aware delivery from background lanes to the UI thread.

use std::{
    collections::VecDeque,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use hotki_protocol::MsgToUI;
use parking_lot::{Condvar, Mutex, MutexGuard};

use crate::app::{UiCommand, UiEvent};

/// Maximum pending notifications, controls, and retained log messages.
const UI_ORDERED_CAPACITY: usize = 256;

/// Cumulative pressure counters for the app UI delivery lane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiDeliveryStats {
    /// Log messages dropped while the ordered lane was full.
    pub(crate) dropped_logs: u64,
    /// Snapshot messages replaced before the UI observed them.
    pub(crate) coalesced_snapshots: u64,
}

/// Result of accepting one UI event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiDeliveryOutcome {
    /// The event was appended to the bounded ordered lane.
    Queued,
    /// The event replaced an older pending snapshot.
    Coalesced,
    /// A log event was dropped because the ordered lane was full.
    DroppedLogFull,
}

/// The UI receiver has closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiDeliveryClosed;

impl fmt::Display for UiDeliveryClosed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("UI delivery lane closed")
    }
}

/// One accepted event with its causal mailbox position.
struct SequencedUiEvent {
    /// Monotonic position assigned by the mailbox.
    sequence: u64,
    /// Event delivered at this position.
    event: UiEvent,
}

/// Latest pending value for each coalescible state class.
#[derive(Default)]
struct PendingSnapshots {
    /// Latest HUD state.
    hud: Option<SequencedUiEvent>,
    /// Latest selector state.
    selector: Option<SequencedUiEvent>,
    /// Latest server heartbeat.
    heartbeat: Option<SequencedUiEvent>,
    /// Latest world snapshot.
    world: Option<SequencedUiEvent>,
    /// Latest complete runtime-health state.
    runtime_health: Option<SequencedUiEvent>,
    /// Latest server-binding health state.
    binding_health: Option<SequencedUiEvent>,
    /// Latest permission-health state.
    permission_health: Option<SequencedUiEvent>,
}

impl PendingSnapshots {
    /// Replace the matching snapshot class, or return an ordered event unchanged.
    fn replace(&mut self, entry: SequencedUiEvent) -> Result<bool, SequencedUiEvent> {
        let slot = match &entry.event {
            UiEvent::Message(MsgToUI::HudUpdate { .. }) => &mut self.hud,
            UiEvent::Message(MsgToUI::SelectorUpdate(_) | MsgToUI::SelectorHide) => {
                &mut self.selector
            }
            UiEvent::Message(MsgToUI::Heartbeat(_)) => &mut self.heartbeat,
            UiEvent::Message(MsgToUI::World(_)) => &mut self.world,
            UiEvent::Command(UiCommand::SetRuntimeHealth(_)) => &mut self.runtime_health,
            UiEvent::Command(UiCommand::SetServerBindings(_)) => &mut self.binding_health,
            UiEvent::Command(UiCommand::SetPermissionStatusOverride(_)) => {
                &mut self.permission_health
            }
            _ => return Err(entry),
        };
        Ok(slot.replace(entry).is_some())
    }

    /// Return the earliest surviving snapshot sequence.
    fn next_sequence(&self) -> Option<u64> {
        [
            self.hud.as_ref(),
            self.selector.as_ref(),
            self.runtime_health.as_ref(),
            self.binding_health.as_ref(),
            self.permission_health.as_ref(),
            self.heartbeat.as_ref(),
            self.world.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(|entry| entry.sequence)
        .min()
    }

    /// Return the number of retained snapshot classes.
    fn len(&self) -> usize {
        [
            self.hud.as_ref(),
            self.selector.as_ref(),
            self.runtime_health.as_ref(),
            self.binding_health.as_ref(),
            self.permission_health.as_ref(),
            self.heartbeat.as_ref(),
            self.world.as_ref(),
        ]
        .into_iter()
        .flatten()
        .count()
    }

    /// Remove the snapshot at `sequence`.
    fn take(&mut self, sequence: u64) -> Option<UiEvent> {
        for slot in [
            &mut self.hud,
            &mut self.selector,
            &mut self.runtime_health,
            &mut self.binding_health,
            &mut self.permission_health,
            &mut self.heartbeat,
            &mut self.world,
        ] {
            if slot
                .as_ref()
                .is_some_and(|entry| entry.sequence == sequence)
            {
                return slot.take().map(|entry| entry.event);
            }
        }
        None
    }
}

/// Mutable mailbox state protected by one short-lived lock.
#[derive(Default)]
struct UiDeliveryState {
    /// Bounded ordered controls, notifications, and logs.
    ordered: VecDeque<SequencedUiEvent>,
    /// Coalesced state snapshots.
    snapshots: PendingSnapshots,
    /// Cumulative pressure counters.
    stats: UiDeliveryStats,
    /// Sequence assigned to the next accepted event.
    next_sequence: u64,
}

/// Shared synchronization and storage for the mailbox pair.
struct UiDeliveryShared {
    /// Protected mailbox state.
    state: Mutex<UiDeliveryState>,
    /// Wakes blocked ordered producers after the UI drains one event.
    space_ready: Condvar,
    /// False after the sole receiver is dropped.
    receiver_open: AtomicBool,
}

/// Cloneable producer for the bounded UI delivery lane.
#[derive(Clone)]
pub struct UiDeliveryTx {
    /// Shared mailbox state.
    shared: Arc<UiDeliveryShared>,
}

impl UiDeliveryTx {
    /// Deliver one event according to its message class.
    pub(crate) fn send(&self, event: UiEvent) -> Result<UiDeliveryOutcome, UiDeliveryClosed> {
        if !self.shared.receiver_open.load(Ordering::Acquire) {
            return Err(UiDeliveryClosed);
        }

        let mut state = self.shared.state.lock();
        let entry = SequencedUiEvent {
            sequence: state.next_sequence,
            event,
        };
        state.next_sequence = state.next_sequence.wrapping_add(1);
        let entry = match state.snapshots.replace(entry) {
            Ok(coalesced) => {
                if coalesced {
                    state.stats.coalesced_snapshots += 1;
                    return Ok(UiDeliveryOutcome::Coalesced);
                }
                return Ok(UiDeliveryOutcome::Queued);
            }
            Err(entry) => entry,
        };

        let pending_count = state.snapshots.len();
        if matches!(&entry.event, UiEvent::Message(MsgToUI::Log { .. }))
            && state.ordered.len() + pending_count >= UI_ORDERED_CAPACITY
        {
            state.stats.dropped_logs += 1;
            return Ok(UiDeliveryOutcome::DroppedLogFull);
        }

        while let Some(sequence) = state.snapshots.next_sequence() {
            self.wait_for_ordered_space(&mut state)?;
            let snapshot = state
                .snapshots
                .take(sequence)
                .expect("next snapshot sequence remains present while locked");
            state.ordered.push_back(SequencedUiEvent {
                sequence,
                event: snapshot,
            });
        }

        self.wait_for_ordered_space(&mut state)?;
        state.ordered.push_back(entry);
        Ok(UiDeliveryOutcome::Queued)
    }

    /// Wait until one causally ordered event can be retained.
    fn wait_for_ordered_space(
        &self,
        state: &mut MutexGuard<'_, UiDeliveryState>,
    ) -> Result<(), UiDeliveryClosed> {
        while state.ordered.len() >= UI_ORDERED_CAPACITY {
            if !self.shared.receiver_open.load(Ordering::Acquire) {
                return Err(UiDeliveryClosed);
            }
            self.shared.space_ready.wait(state);
        }
        Ok(())
    }
}

/// UI-thread consumer for the bounded delivery lane.
pub struct UiDeliveryRx {
    /// Shared mailbox state.
    shared: Arc<UiDeliveryShared>,
}

impl UiDeliveryRx {
    /// Return the next available event without waiting.
    pub(crate) fn try_recv(&self) -> Option<UiEvent> {
        let mut state = self.shared.state.lock();
        let ordered_sequence = state.ordered.front().map(|entry| entry.sequence);
        let snapshot_sequence = state.snapshots.next_sequence();
        if let Some(sequence) = snapshot_sequence
            && ordered_sequence.is_none_or(|ordered| sequence < ordered)
        {
            return state.snapshots.take(sequence);
        }
        let event = state.ordered.pop_front()?.event;
        self.shared.space_ready.notify_one();
        Some(event)
    }

    /// Return current pressure counters.
    pub(crate) fn stats(&self) -> UiDeliveryStats {
        self.shared.state.lock().stats
    }
}

impl Drop for UiDeliveryRx {
    fn drop(&mut self) {
        self.shared.receiver_open.store(false, Ordering::Release);
        self.shared.space_ready.notify_all();
    }
}

/// Create the standard bounded UI delivery lane.
pub fn ui_delivery_channel() -> (UiDeliveryTx, UiDeliveryRx) {
    let shared = Arc::new(UiDeliveryShared {
        state: Mutex::new(UiDeliveryState::default()),
        space_ready: Condvar::new(),
        receiver_open: AtomicBool::new(true),
    });
    (
        UiDeliveryTx {
            shared: shared.clone(),
        },
        UiDeliveryRx { shared },
    )
}

#[cfg(test)]
mod tests {
    use hotki_protocol::Toggle;

    use super::*;

    fn log_event(index: usize) -> UiEvent {
        UiEvent::Message(MsgToUI::Log {
            level: "info".to_string(),
            target: "test".to_string(),
            message: format!("log {index}"),
        })
    }

    #[test]
    fn selector_state_coalesces_without_reordering_notifications() {
        let (tx, rx) = ui_delivery_channel();
        tx.send(UiEvent::Message(MsgToUI::SelectorHide))
            .expect("queue selector hide");
        tx.send(UiEvent::Message(MsgToUI::SelectorHide))
            .expect("coalesce selector hide");
        tx.send(UiEvent::Message(MsgToUI::ShowDetails(Toggle::On)))
            .expect("queue ordered control");

        assert_eq!(rx.stats().coalesced_snapshots, 1);
        assert!(matches!(
            rx.try_recv(),
            Some(UiEvent::Message(MsgToUI::SelectorHide))
        ));
        assert!(matches!(
            rx.try_recv(),
            Some(UiEvent::Message(MsgToUI::ShowDetails(Toggle::On)))
        ));
    }

    #[test]
    fn ordered_event_is_a_snapshot_coalescing_barrier() {
        let (tx, rx) = ui_delivery_channel();
        tx.send(UiEvent::Message(MsgToUI::SelectorHide))
            .expect("queue first selector state");
        tx.send(UiEvent::Message(MsgToUI::ShowDetails(Toggle::On)))
            .expect("queue ordered control");
        tx.send(UiEvent::Message(MsgToUI::SelectorHide))
            .expect("queue selector state after barrier");

        assert_eq!(rx.stats().coalesced_snapshots, 0);
        assert!(matches!(
            rx.try_recv(),
            Some(UiEvent::Message(MsgToUI::SelectorHide))
        ));
        assert!(matches!(
            rx.try_recv(),
            Some(UiEvent::Message(MsgToUI::ShowDetails(Toggle::On)))
        ));
        assert!(matches!(
            rx.try_recv(),
            Some(UiEvent::Message(MsgToUI::SelectorHide))
        ));
    }

    #[test]
    fn full_log_lane_and_closed_receiver_are_distinct() {
        let (tx, rx) = ui_delivery_channel();
        for index in 0..UI_ORDERED_CAPACITY {
            assert_eq!(
                tx.send(log_event(index)).expect("queue log"),
                UiDeliveryOutcome::Queued
            );
        }
        assert_eq!(
            tx.send(log_event(UI_ORDERED_CAPACITY))
                .expect("drop log under pressure"),
            UiDeliveryOutcome::DroppedLogFull
        );
        assert_eq!(rx.stats().dropped_logs, 1);

        drop(rx);
        assert_eq!(tx.send(log_event(0)), Err(UiDeliveryClosed));
    }
}
