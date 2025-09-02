use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
};

use egui::Color32;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Client,
    Server,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub side: Side,
    pub level: String,
    pub target: String,
    pub message: String,
}

impl LogEntry {
    pub fn color(&self) -> Color32 {
        match self.level.as_str() {
            "ERROR" => Color32::from_rgb(220, 50, 47),
            "WARN" => Color32::from_rgb(203, 75, 22),
            "INFO" => Color32::from_rgb(133, 153, 0),
            "DEBUG" => Color32::from_rgb(38, 139, 210),
            "TRACE" => Color32::from_rgb(108, 113, 196),
            _ => Color32::from_rgb(200, 200, 200),
        }
    }
}

static LOGS: OnceLock<Mutex<VecDeque<LogEntry>>> = OnceLock::new();

fn buffer() -> &'static Mutex<VecDeque<LogEntry>> {
    LOGS.get_or_init(|| Mutex::new(VecDeque::with_capacity(2048)))
}

pub fn push(entry: LogEntry) {
    let mut buf = buffer().lock().expect("logs mutex poisoned");
    if buf.len() > 5000 {
        buf.pop_front();
    }
    buf.push_back(entry);
}

pub fn push_server(level: String, target: String, message: String) {
    push(LogEntry {
        side: Side::Server,
        level,
        target,
        message,
    });
}

pub fn snapshot() -> Vec<LogEntry> {
    buffer()
        .lock()
        .expect("logs mutex poisoned")
        .iter()
        .cloned()
        .collect()
}

pub fn clear() {
    buffer().lock().expect("logs mutex poisoned").clear();
}

/// Tracing layer that records client-side logs into the buffer.
pub struct ClientLayer;

impl<S> Layer<S> for ClientLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let level = meta.level().to_string();
        let target = meta.target().to_string();

        // Extract a message from event fields
        use tracing::field::{Field, Visit};
        struct MsgVisitor {
            msg: Option<String>,
            fields: String,
        }
        impl Visit for MsgVisitor {
            fn record_str(&mut self, field: &Field, value: &str) {
                if field.name() == "message" {
                    self.msg = Some(value.to_string());
                } else {
                    let _ = std::fmt::Write::write_fmt(
                        &mut self.fields,
                        format_args!("{}=\"{}\" ", field.name(), value),
                    );
                }
            }
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.msg = Some(format!("{:?}", value));
                } else {
                    let _ = std::fmt::Write::write_fmt(
                        &mut self.fields,
                        format_args!("{}={:?} ", field.name(), value),
                    );
                }
            }
        }
        let mut vis = MsgVisitor {
            msg: None,
            fields: String::new(),
        };
        event.record(&mut vis);
        let rendered = vis.msg.unwrap_or_else(|| vis.fields.trim_end().to_string());

        push(LogEntry {
            side: Side::Client,
            level,
            target,
            message: rendered,
        });
    }
}

pub fn client_layer() -> ClientLayer {
    ClientLayer
}
