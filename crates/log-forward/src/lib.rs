//! Forward tracing events to the Hotki UI.
//!
//! This crate provides a small tracing [Layer] that forwards log events to the
//! UI over the Hotki protocol when a sink is set. It is used by the server to
//! relay its logs to connected clients for display in the Details window.
//!
//! Usage
//! - Call [`set_sink`] with a `tokio::sync::mpsc::UnboundedSender<hotki_protocol::MsgToUI>`
//!   when a client connects.
//! - Install the [`layer`] in your tracing subscriber. When a sink is present,
//!   events will be forwarded as `MsgToUI::Log { level, target, message }`.
//! - Call [`clear_sink`] when the client disconnects.
//!
//! The layer is lightweight and no-ops when no sink is set.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]
use std::sync::OnceLock;

use hotki_protocol::MsgToUI;
use parking_lot::Mutex;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

// A global sink that, when present, forwards server logs to the connected client.
static LOG_SINK: OnceLock<Mutex<Option<UnboundedSender<MsgToUI>>>> = OnceLock::new();

fn sink() -> &'static Mutex<Option<UnboundedSender<MsgToUI>>> {
    LOG_SINK.get_or_init(|| Mutex::new(None))
}

/// Set the forwarding sink (called when a client connects).
pub fn set_sink(tx: hotki_protocol::ipc::UiTx) {
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

        let r = logfmt::render_event(event);
        let _ = tx.send(MsgToUI::Log {
            level: r.level,
            target: r.target,
            message: r.message,
        });
    }
}

/// Create the forwarding layer instance to add to your subscriber.
pub fn layer() -> ForwardLayer {
    ForwardLayer
}
