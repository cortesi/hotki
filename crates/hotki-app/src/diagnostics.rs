//! Shared privacy-bounded diagnostics for automation and support reports.

use std::{fmt::Write as _, process, sync::Arc};

use hotki_protocol::{InputHealth, SecureInputState, TapLifecycle, TapMode};
use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::{
    health::RuntimeHealth,
    logs::{self, LogEntry, Side},
    notification::NotificationStackAlias,
    ui_delivery::UiDeliveryStats,
};

/// Maximum number of classified lifecycle events included in a report.
const LOG_TAIL_LIMIT: usize = 20;

/// Thread-safe latest diagnostic snapshot shared by every renderer.
#[derive(Clone, Default)]
pub struct DiagnosticStore {
    /// Latest complete snapshot behind one short-lived lock.
    snapshot: Arc<Mutex<DiagnosticSnapshot>>,
}

impl DiagnosticStore {
    /// Replace the snapshot from current UI-thread state.
    pub(crate) fn update(
        &self,
        runtime: &RuntimeHealth,
        bindings: &[String],
        input: &InputHealth,
        notifications: &[NotificationStackAlias],
        delivery: UiDeliveryStats,
    ) {
        *self.snapshot.lock() = DiagnosticSnapshot::capture(
            runtime,
            bindings,
            input,
            notifications,
            delivery,
            &logs::entries(),
        );
    }

    /// Render the latest snapshot as deterministic JSON.
    pub(crate) fn json(&self) -> Value {
        self.snapshot.lock().to_json()
    }

    /// Render the latest snapshot as deterministic support-report text.
    pub(crate) fn plain_text(&self) -> String {
        self.snapshot.lock().to_plain_text()
    }
}

/// One privacy-bounded diagnostic record.
#[derive(Clone, Debug, Default)]
struct DiagnosticSnapshot {
    /// Runtime state shared by normal UI surfaces.
    runtime: RuntimeHealth,
    /// Full server input-health status.
    input: InputHealth,
    /// Count only; binding identifiers are deliberately excluded.
    binding_count: usize,
    /// Metadata-only live notification aliases.
    notifications: Vec<NotificationDiagnostic>,
    /// UI mailbox pressure counters.
    delivery: UiDeliveryStats,
    /// Bounded allowlisted lifecycle events.
    logs: Vec<DiagnosticLog>,
}

impl DiagnosticSnapshot {
    /// Capture current state while applying report privacy boundaries.
    fn capture(
        runtime: &RuntimeHealth,
        bindings: &[String],
        input: &InputHealth,
        notifications: &[NotificationStackAlias],
        delivery: UiDeliveryStats,
        logs: &[LogEntry],
    ) -> Self {
        let notifications = notifications
            .iter()
            .map(|alias| NotificationDiagnostic {
                index: alias.index,
                live_id: alias.live_id.clone(),
                kind: alias.kind,
            })
            .collect();
        let logs = logs
            .iter()
            .filter_map(DiagnosticLog::from_entry)
            .rev()
            .take(LOG_TAIL_LIMIT)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Self {
            runtime: runtime.clone(),
            input: input.clone(),
            binding_count: bindings.len(),
            notifications,
            delivery,
            logs,
        }
    }

    /// Render the automation-facing JSON schema.
    fn to_json(&self) -> Value {
        let owner = self.input.secure_input_owner.as_ref().map(|owner| {
            json!({
                "pid": owner.pid,
                "app_name": owner.app_name,
                "attribution": "best_effort",
            })
        });
        let notifications = self
            .notifications
            .iter()
            .map(NotificationDiagnostic::to_json)
            .collect::<Vec<_>>();
        let logs = self
            .logs
            .iter()
            .map(DiagnosticLog::to_json)
            .collect::<Vec<_>>();
        json!({
            "app": {
                "name": "Hotki",
                "version": env!("CARGO_PKG_VERSION"),
                "pid": process::id(),
            },
            "server": {
                "connected": self.runtime.server_connected(),
                "pid": (self.input.server_pid != 0).then_some(self.input.server_pid),
            },
            "runtime": {
                "phase": self.runtime.phase().label(),
                "connection": self.runtime.connection().label(),
                "config": {
                    "active": self.runtime.active_config().is_some(),
                    "pending": self.runtime.pending_config().is_some(),
                },
                "permissions": {
                    "accessibility": permission_label(self.runtime.permissions.accessibility),
                    "input_monitoring": permission_label(self.runtime.permissions.input_monitoring),
                    "screen_recording": permission_label(self.runtime.permissions.screen_recording),
                },
                "retry": self.runtime.retry().label(),
            },
            "input": {
                "tap_mode": tap_mode_label(self.input.tap_mode),
                "tap_lifecycle": tap_lifecycle_label(self.input.tap_lifecycle),
                "secure_input": secure_input_label(self.input.secure_input),
                "secure_input_owner": owner,
                "blocked": self.input.blocked,
                "registered_hotkeys": self.input.registered_hotkeys,
                "physical_event_count": self.input.physical_event_count,
                "physical_event_age_ms": self.input.physical_event_age_ms,
                "os_disable_count": self.input.os_disable_count,
                "os_reenable_count": self.input.os_reenable_count,
                "observed_at_ms": self.input.observed_at_ms,
            },
            "bindings": { "count": self.binding_count },
            "delivery": {
                "dropped_logs": self.delivery.dropped_logs,
                "coalesced_snapshots": self.delivery.coalesced_snapshots,
            },
            "notifications": {
                "live_count": self.notifications.len(),
                "items": notifications,
            },
            "logs": logs,
        })
    }

    /// Render the clipboard-facing plain-text support report.
    fn to_plain_text(&self) -> String {
        let owner = self.input.secure_input_owner.as_ref().map_or_else(
            || "unknown (best effort)".to_string(),
            |owner| {
                format!(
                    "{} (pid {}, best effort)",
                    single_line(&owner.app_name),
                    owner.pid
                )
            },
        );
        let mut report = String::new();
        writeln!(report, "Hotki Diagnostics").expect("writing to String cannot fail");
        writeln!(
            report,
            "app: Hotki {} (pid {})",
            env!("CARGO_PKG_VERSION"),
            process::id()
        )
        .expect("writing to String cannot fail");
        writeln!(
            report,
            "server: connected={} pid={}",
            self.runtime.server_connected(),
            optional_u32(self.input.server_pid)
        )
        .expect("writing to String cannot fail");
        writeln!(
            report,
            "runtime: phase={} connection={} active_config={} pending_config={} retry={}",
            self.runtime.phase().label(),
            self.runtime.connection().label(),
            self.runtime.active_config().is_some(),
            self.runtime.pending_config().is_some(),
            self.runtime.retry().label(),
        )
        .expect("writing to String cannot fail");
        writeln!(
            report,
            "permissions: accessibility={} input_monitoring={} screen_recording={}",
            permission_label(self.runtime.permissions.accessibility),
            permission_label(self.runtime.permissions.input_monitoring),
            permission_label(self.runtime.permissions.screen_recording),
        )
        .expect("writing to String cannot fail");
        writeln!(
            report,
            "input: mode={} lifecycle={} secure_input={} blocked={} owner={owner}",
            tap_mode_label(self.input.tap_mode),
            tap_lifecycle_label(self.input.tap_lifecycle),
            secure_input_label(self.input.secure_input),
            self.input.blocked,
        )
        .expect("writing to String cannot fail");
        writeln!(
            report,
            "input_counts: registered={} physical={} physical_age_ms={} disabled={} reenabled={} observed_at_ms={}",
            self.input.registered_hotkeys,
            self.input.physical_event_count,
            optional_u64(self.input.physical_event_age_ms),
            self.input.os_disable_count,
            self.input.os_reenable_count,
            optional_u64(self.input.observed_at_ms),
        )
        .expect("writing to String cannot fail");
        writeln!(
            report,
            "bindings: count={} (identifiers omitted)",
            self.binding_count
        )
        .expect("writing to String cannot fail");
        writeln!(
            report,
            "delivery: dropped_logs={} coalesced_snapshots={}",
            self.delivery.dropped_logs, self.delivery.coalesced_snapshots
        )
        .expect("writing to String cannot fail");
        writeln!(
            report,
            "notifications: live_count={} (text omitted)",
            self.notifications.len()
        )
        .expect("writing to String cannot fail");
        writeln!(report, "health_logs: {}", self.logs.len())
            .expect("writing to String cannot fail");
        for log in &self.logs {
            writeln!(report, "- {} {} {}", log.side, log.level, log.event)
                .expect("writing to String cannot fail");
        }
        report
    }
}

#[derive(Clone, Debug)]
/// Metadata for one live notification without user-authored text.
struct NotificationDiagnostic {
    /// Stack index, newest first.
    index: usize,
    /// Stable runtime viewport identifier.
    live_id: String,
    /// Stable notification severity label.
    kind: &'static str,
}

impl NotificationDiagnostic {
    /// Render metadata as deterministic JSON.
    fn to_json(&self) -> Value {
        json!({ "index": self.index, "live_id": self.live_id, "kind": self.kind })
    }
}

#[derive(Clone, Debug)]
/// One fixed-name lifecycle or health event allowed into reports.
struct DiagnosticLog {
    /// Whether the event came from client or server.
    side: &'static str,
    /// Captured tracing level.
    level: String,
    /// Fixed allowlisted event label, never the raw log message.
    event: &'static str,
}

impl DiagnosticLog {
    /// Classify one raw buffered entry into the report-safe form.
    fn from_entry(entry: &LogEntry) -> Option<Self> {
        let event = classify_health_event(entry)?;
        Some(Self {
            side: match entry.side {
                Side::Client => "client",
                Side::Server => "server",
            },
            level: entry.level.clone(),
            event,
        })
    }

    /// Render the classified event as deterministic JSON.
    fn to_json(&self) -> Value {
        json!({ "side": self.side, "level": self.level, "event": self.event })
    }
}

/// Return the fixed event label for an allowlisted target and message.
fn classify_health_event(entry: &LogEntry) -> Option<&'static str> {
    const TARGETS: [&str; 4] = [
        "mac_hotkey",
        "mac_hotkey::sys",
        "hotki_app::connection_driver",
        "hotki_server::ipc::service::events::sources",
    ];
    const EVENTS: [&str; 9] = [
        "input_health_transition",
        "mac_hotkey_manager_started",
        "creating_event_tap",
        "event_tap_started_run_loop",
        "event_tap_exited",
        "tap_disabled_by_os_reenabling",
        "server acknowledged shutdown",
        "Server connection lost; reconnecting",
        "Config path sent to server engine",
    ];
    if !TARGETS.contains(&entry.target.as_str()) {
        return None;
    }
    EVENTS
        .into_iter()
        .find(|event| entry.message.contains(event))
}

/// Stable report label for one permission observation.
fn permission_label(state: permissions::PermissionState) -> &'static str {
    match state {
        permissions::PermissionState::Granted => "granted",
        permissions::PermissionState::Denied => "denied",
        permissions::PermissionState::Unknown => "unknown",
    }
}

/// Stable report label for the tap mode.
fn tap_mode_label(mode: TapMode) -> &'static str {
    match mode {
        TapMode::Physical => "physical",
        TapMode::InjectionOnly => "injection_only",
    }
}

/// Stable report label for the tap lifecycle.
fn tap_lifecycle_label(lifecycle: TapLifecycle) -> &'static str {
    match lifecycle {
        TapLifecycle::Starting => "starting",
        TapLifecycle::Running => "running",
        TapLifecycle::Stopped => "stopped",
    }
}

/// Stable report label for a Secure Input observation.
fn secure_input_label(state: SecureInputState) -> &'static str {
    match state {
        SecureInputState::Unknown => "unknown",
        SecureInputState::Inactive => "inactive",
        SecureInputState::Active => "active",
    }
}

/// Render an optional counter or timestamp with an explicit unknown value.
fn optional_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "unknown".to_string(), |value| value.to_string())
}

/// Render a sentinel-zero PID with an explicit unknown value.
fn optional_u32(value: u32) -> String {
    if value != 0 {
        value.to_string()
    } else {
        "unknown".to_string()
    }
}

/// Escape control characters that could forge lines in the plain-text report.
fn single_line(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use hotki_protocol::{SecureInputOwner, SecureInputState};

    use super::{DiagnosticSnapshot, LOG_TAIL_LIMIT};
    use crate::{
        health::RuntimeHealth,
        logs::{LogEntry, Side},
        ui_delivery::UiDeliveryStats,
    };

    #[test]
    fn reports_unknown_and_best_effort_owner_explicitly() {
        let unknown = DiagnosticSnapshot::default().to_plain_text();
        assert!(unknown.contains("secure_input=unknown"));
        assert!(unknown.contains("owner=unknown (best effort)"));

        let mut active = DiagnosticSnapshot::default();
        active.input.secure_input = SecureInputState::Active;
        active.input.secure_input_owner = Some(SecureInputOwner {
            pid: 7,
            app_name: "Terminal & Shell".to_string(),
        });
        let json = active.to_json().to_string();
        assert!(json.contains("Terminal & Shell"));
        assert!(json.contains("best_effort"));
    }

    #[test]
    fn reports_inactive_empty_logs_and_active_missing_owner() {
        let mut snapshot = DiagnosticSnapshot::default();
        snapshot.input.secure_input = SecureInputState::Inactive;
        let inactive = snapshot.to_plain_text();
        assert!(inactive.contains("secure_input=inactive"));
        assert!(inactive.contains("health_logs: 0"));

        snapshot.input.secure_input = SecureInputState::Active;
        let active = snapshot.to_plain_text();
        assert!(active.contains("secure_input=active"));
        assert!(active.contains("owner=unknown (best effort)"));
    }

    #[test]
    fn plain_text_owner_cannot_inject_report_lines() {
        let mut snapshot = DiagnosticSnapshot::default();
        snapshot.input.secure_input_owner = Some(SecureInputOwner {
            pid: 7,
            app_name: "Terminal\nforged: value\\path\tend".to_string(),
        });

        let report = snapshot.to_plain_text();
        assert!(report.contains("Terminal\\nforged: value\\\\path\\tend"));
        assert!(
            !report
                .lines()
                .any(|line| line == "forged: value\\path\tend")
        );
    }

    #[test]
    fn reports_exclude_sensitive_source_text_and_bound_logs() {
        let bindings = vec!["ctrl+secret".to_string()];
        let notifications = Vec::new();
        let mut entries = vec![LogEntry {
            side: Side::Client,
            level: "INFO".to_string(),
            target: "hotki_app::notification".to_string(),
            message:
                "event_tap_started_run_loop typed text config-source notification-body ctrl+secret"
                    .to_string(),
        }];
        entries.extend((0..(LOG_TAIL_LIMIT + 5)).map(|_| LogEntry {
            side: Side::Server,
            level: "INFO".to_string(),
            target: "mac_hotkey::sys".to_string(),
            message: "event_tap_started_run_loop malicious-special-<>&".to_string(),
        }));
        let snapshot = DiagnosticSnapshot::capture(
            &RuntimeHealth::default(),
            &bindings,
            &hotki_protocol::InputHealth::default(),
            &notifications,
            UiDeliveryStats::default(),
            &entries,
        );
        let report = snapshot.to_plain_text();
        assert_eq!(snapshot.logs.len(), LOG_TAIL_LIMIT);
        for secret in [
            "ctrl+secret",
            "typed text",
            "config-source",
            "notification-body",
            "malicious-special-<>&",
        ] {
            assert!(!report.contains(secret));
        }
    }
}
