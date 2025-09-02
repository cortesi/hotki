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

use std::{
    fmt::Write,
    sync::{Mutex, OnceLock},
};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::layer::{Context, Layer};

use hotki_protocol::MsgToUI;

// A global sink that, when present, forwards server logs to the connected client.
static LOG_SINK: OnceLock<Mutex<Option<UnboundedSender<MsgToUI>>>> = OnceLock::new();

fn sink() -> &'static Mutex<Option<UnboundedSender<MsgToUI>>> {
    LOG_SINK.get_or_init(|| Mutex::new(None))
}

/// Set the forwarding sink (called when a client connects).
pub fn set_sink(tx: hotki_protocol::ipc::UiTx) {
    let mut guard = sink().lock().expect("log sink mutex poisoned");
    *guard = Some(tx);
}

/// Clear the forwarding sink (called when a client disconnects).
pub fn clear_sink() {
    let mut guard = sink().lock().expect("log sink mutex poisoned");
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
        let tx_opt = {
            let guard = sink().lock().expect("log sink mutex poisoned");
            guard.clone()
        };
        let Some(tx) = tx_opt else { return };

        // Pull fields from the event
        struct MsgVisitor {
            msg: Option<String>,
            fields: String,
        }
        impl Visit for MsgVisitor {
            fn record_str(&mut self, field: &Field, value: &str) {
                if field.name() == "message" {
                    self.msg = Some(value.to_string());
                } else {
                    let _ = write!(&mut self.fields, "{}=\"{}\" ", field.name(), value);
                }
            }
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.msg = Some(format!("{:?}", value));
                } else {
                    let _ = write!(&mut self.fields, "{}={:?} ", field.name(), value);
                }
            }
        }
        let mut vis = MsgVisitor {
            msg: None,
            fields: String::new(),
        };
        event.record(&mut vis);
        let rendered = vis.msg.unwrap_or_else(|| vis.fields.trim_end().to_string());

        let meta = event.metadata();
        let level = meta.level().to_string();
        let target = meta.target().to_string();
        let _ = tx.send(MsgToUI::Log {
            level,
            target,
            message: rendered,
        });
    }
}

/// Create the forwarding layer instance to add to your subscriber.
pub fn layer() -> ForwardLayer {
    ForwardLayer
}
