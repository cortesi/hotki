//! Utilities to render `tracing` events into concise logfmt strings.
//!
//! This crate provides helpers to extract level, target, and message from
//! `tracing::Event` records and to render remaining fields in `key=value`
//! form. It is used for structured logging throughout the application.

use std::fmt::{Debug, Write};

use tracing::{
    Event, Metadata,
    field::{Field, Visit},
};

/// Rendered fields extracted from a tracing Event.
#[derive(Debug, Clone)]
pub struct RenderedLog {
    /// Severity level (e.g., INFO, WARN) for the event.
    pub level: String,
    /// Event target (typically the module path).
    pub target: String,
    /// Human‑readable message or rendered `key=value` pairs.
    pub message: String,
}

/// Extract a concise triple (level, target, message) from a tracing Event.
///
/// Behavior:
/// - If the event contains a `message` field, use it.
/// - Otherwise, concatenate `key=value` pairs from remaining fields.
pub fn render_event(event: &Event<'_>) -> RenderedLog {
    struct MsgVisitor {
        /// Captured `message` field, if present.
        msg: Option<String>,
        /// Accumulated non‑message fields rendered as `key=value`.
        fields: String,
    }
    impl Visit for MsgVisitor {
        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "message" {
                self.msg = Some(value.to_string());
            } else {
                let _ignored = write!(&mut self.fields, "{}=\"{}\" ", field.name(), value);
            }
        }
        fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
            if field.name() == "message" {
                self.msg = Some(format!("{:?}", value));
            } else {
                let _ignored = write!(&mut self.fields, "{}={:?} ", field.name(), value);
            }
        }
    }
    let meta: &Metadata<'_> = event.metadata();
    let mut vis = MsgVisitor {
        msg: None,
        fields: String::new(),
    };
    event.record(&mut vis);
    let rendered = vis.msg.unwrap_or_else(|| vis.fields.trim_end().to_string());
    RenderedLog {
        level: meta.level().to_string(),
        target: meta.target().to_string(),
        message: rendered,
    }
}
