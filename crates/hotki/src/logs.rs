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
        let r = logfmt::render_event(event);
        push(LogEntry {
            side: Side::Client,
            level: r.level,
            target: r.target,
            message: r.message,
        });
    }
}

pub fn client_layer() -> ClientLayer {
    ClientLayer
}
