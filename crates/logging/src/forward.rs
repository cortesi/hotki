//! Forward tracing events to the Hotki UI.
//!
//! This module provides a small tracing [`Layer`] that forwards log events to the
//! UI over the Hotki protocol when a sink is set. It is used by the server to
//! relay its logs to connected clients for display in the Details window.
//!
//! Usage:
//! - Call [`set_sink`] with a `tokio::sync::mpsc::Sender<hotki_protocol::MsgToUI>`
//!   when a client connects.
//! - Install the [`layer`] in your tracing subscriber. When a sink is present,
//!   events will be forwarded as `MsgToUI::Log { level, target, message }`.
//! - Call [`clear_sink`] when the client disconnects.
//!
//! The layer is lightweight and no-ops when no sink is set.

use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};

use hotki_protocol::{MsgToUI, ipc::UiTx};
use parking_lot::Mutex;
use tokio::sync::mpsc::{Sender, error::TrySendError};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::fmt;

/// A global sink that, when present, forwards server logs to the connected client.
static LOG_SINK: OnceLock<Mutex<Option<Sender<MsgToUI>>>> = OnceLock::new();

/// Count of log events dropped due to a full UI pipeline.
static LOG_DROPS: OnceLock<AtomicU64> = OnceLock::new();

/// Access the global sink.
fn sink() -> &'static Mutex<Option<Sender<MsgToUI>>> {
    LOG_SINK.get_or_init(|| Mutex::new(None))
}

/// Set the forwarding sink (called when a client connects).
pub fn set_sink(tx: UiTx) {
    let mut guard = sink().lock();
    *guard = Some(tx);
}

/// Clear the forwarding sink (called when a client disconnects).
pub fn clear_sink() {
    let mut guard = sink().lock();
    *guard = None;
}

/// Tracing layer that forwards events to the UI when a sink is set.
pub struct ForwardLayer;

impl<S> Layer<S> for ForwardLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        // Early-exit if there is no sink set
        let tx_opt = { sink().lock().clone() };
        let Some(tx) = tx_opt else { return };

        let r = fmt::render_event(event);
        match tx.try_send(MsgToUI::Log {
            level: r.level,
            target: r.target,
            message: r.message,
        }) {
            Ok(()) => {}
            Err(TrySendError::Closed(_)) => {
                // Sink disappeared; clear to avoid repeated work.
                clear_sink();
            }
            Err(TrySendError::Full(_)) => {
                let ctr = LOG_DROPS.get_or_init(|| AtomicU64::new(0));
                let n = ctr.fetch_add(1, Ordering::SeqCst) + 1;
                if n == 1 || n.is_multiple_of(1000) {
                    // Throttled debug to avoid log storms; still visible in Details logs.
                    tracing::debug!(count = n, "ui_log_drop");
                }
            }
        }
    }
}

/// Create the forwarding layer instance to add to your subscriber.
pub fn layer() -> ForwardLayer {
    ForwardLayer
}
