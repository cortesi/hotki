use std::fmt::Write;

use tracing::{
    Event, Metadata,
    field::{Field, Visit},
};

/// Rendered fields extracted from a tracing Event.
#[derive(Debug, Clone)]
pub struct RenderedLog {
    pub level: String,
    pub target: String,
    pub message: String,
}

/// Extract a concise triple (level, target, message) from a tracing Event.
///
/// Behavior:
/// - If the event contains a `message` field, use it.
/// - Otherwise, concatenate `key=value` pairs from remaining fields.
pub fn render_event(event: &Event<'_>) -> RenderedLog {
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
