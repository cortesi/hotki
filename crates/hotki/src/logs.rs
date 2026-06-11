//! UI log buffer and tracing integration.
use std::{collections::VecDeque, sync::OnceLock};

use egui::Color32;
use hotki_protocol::NotifyKind;
use logging::fmt;
use parking_lot::Mutex;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The origin of a log entry.
pub enum Side {
    /// Client-side (UI) log.
    Client,
    /// Server-side log forwarded to the UI.
    Server,
}

#[derive(Debug, Clone)]
/// A single structured log entry captured for display.
pub struct LogEntry {
    /// Which side produced the entry.
    pub side: Side,
    /// Level rendered as a short string (ERROR/WARN/...).
    pub level: String,
    /// Target/logger name.
    pub target: String,
    /// Rendered message text.
    pub message: String,
}

impl LogEntry {
    /// Choose a representative color for the entry level.
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

#[derive(Debug, Clone)]
/// Snapshot of log entries paired with the buffer generation that produced it.
pub struct LogSnapshot {
    /// Monotonic generation for detecting unchanged buffers.
    pub generation: u64,
    /// Current log entries.
    pub entries: Vec<LogEntry>,
}

/// Mutable log buffer state protected by one lock.
struct LogBuffer {
    /// Monotonic generation for detecting unchanged buffers.
    generation: u64,
    /// Recent log entries.
    entries: VecDeque<LogEntry>,
}

/// Global buffer for recent log entries.
static LOGS: OnceLock<Mutex<LogBuffer>> = OnceLock::new();

/// Access the global buffer, initializing it on first use.
fn buffer() -> &'static Mutex<LogBuffer> {
    LOGS.get_or_init(|| {
        Mutex::new(LogBuffer {
            generation: 0,
            entries: VecDeque::with_capacity(2048),
        })
    })
}

/// Push a new entry into the buffer, evicting from the front if oversized.
pub fn push(entry: LogEntry) {
    let mut buf = buffer().lock();
    if buf.entries.len() > 5000 {
        buf.entries.pop_front();
    }
    buf.entries.push_back(entry);
    buf.generation = buf.generation.saturating_add(1);
}

/// Helper to push a server-side entry.
pub fn push_server(level: String, target: String, message: String) {
    push(LogEntry {
        side: Side::Server,
        level,
        target,
        message,
    });
}

/// Push a client-side notification as a structured log entry.
pub fn push_client_notification(kind: NotifyKind, title: &str, text: &str) {
    push(LogEntry {
        side: Side::Client,
        level: notification_level(kind).to_string(),
        target: "hotki::notification".to_string(),
        message: notification_message(kind, title, text),
    });
}

/// Snapshot the current buffer contents and generation.
#[cfg(test)]
fn snapshot_with_generation() -> LogSnapshot {
    let buf = buffer().lock();
    LogSnapshot {
        generation: buf.generation,
        entries: buf.entries.iter().cloned().collect(),
    }
}

/// Snapshot the buffer only when it changed after `generation`.
pub fn snapshot_after(generation: u64) -> Option<LogSnapshot> {
    let buf = buffer().lock();
    (buf.generation != generation).then(|| LogSnapshot {
        generation: buf.generation,
        entries: buf.entries.iter().cloned().collect(),
    })
}

/// Clear the buffer.
pub fn clear() {
    let mut buf = buffer().lock();
    buf.entries.clear();
    buf.generation = buf.generation.saturating_add(1);
}

/// Tracing layer that records client-side logs into the buffer.
pub struct ClientLayer;

impl<S> Layer<S> for ClientLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let r = fmt::render_event(event);
        push(LogEntry {
            side: Side::Client,
            level: r.level,
            target: r.target,
            message: r.message,
        });
    }
}

/// Build a tracing layer that captures client logs into the in-memory buffer.
pub fn client_layer() -> ClientLayer {
    ClientLayer
}

/// Map a notification kind to the UI log level label.
fn notification_level(kind: NotifyKind) -> &'static str {
    match kind {
        NotifyKind::Error => "ERROR",
        NotifyKind::Warn => "WARN",
        NotifyKind::Info | NotifyKind::Success | NotifyKind::Ignore => "INFO",
    }
}

/// Render a popup notification as a compact log-style payload.
fn notification_message(kind: NotifyKind, title: &str, text: &str) -> String {
    format!("notification=display kind={kind:?} title={title:?} text={text:?}")
}

#[cfg(test)]
mod test {
    use hotki_protocol::NotifyKind;

    use super::{clear, push_client_notification, snapshot_after, snapshot_with_generation};

    #[test]
    fn client_notifications_map_kind_to_log_level() {
        clear();

        push_client_notification(NotifyKind::Warn, "Config", "Duplicate chord");
        push_client_notification(NotifyKind::Error, "Config", "Reload failed");

        let logs = snapshot_after(u64::MAX).expect("changed snapshot").entries;
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].level, "WARN");
        assert_eq!(logs[1].level, "ERROR");
        assert!(logs[0].message.contains("Duplicate chord"));
        assert!(logs[1].message.contains("Reload failed"));
    }

    #[test]
    fn snapshot_after_only_clones_when_generation_changes() {
        clear();
        let empty = snapshot_with_generation();
        assert!(snapshot_after(empty.generation).is_none());

        push_client_notification(NotifyKind::Info, "Config", "Reloaded");
        let changed = snapshot_after(empty.generation).expect("changed snapshot");

        assert_eq!(changed.entries.len(), 1);
        assert!(changed.generation > empty.generation);
        assert!(snapshot_after(changed.generation).is_none());
    }
}
