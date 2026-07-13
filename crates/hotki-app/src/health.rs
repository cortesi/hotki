//! Runtime health shared by the connection driver and every UI surface.

use std::path::{Path, PathBuf};

use permissions::{PermissionState, PermissionsStatus};

/// Coarse lifecycle phase for the app and its server runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimePhase {
    /// No server connection is active.
    Disconnected,
    /// A server connection is being established.
    Connecting,
    /// The current config candidate was rejected.
    InvalidConfig,
    /// Required macOS permissions have not been granted.
    WaitingPermissions,
    /// The server is connected and the active config is running.
    Ready,
    /// A failed operation is being attempted again.
    Retrying,
    /// Owned runtime work is finishing before the app exits.
    ShuttingDown,
}

impl RuntimePhase {
    /// Stable diagnostic label for this phase.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::InvalidConfig => "invalid_config",
            Self::WaitingPermissions => "waiting_permissions",
            Self::Ready => "ready",
            Self::Retrying => "retrying",
            Self::ShuttingDown => "shutting_down",
        }
    }

    /// Human-readable label for normal UI surfaces.
    pub(crate) fn display_label(self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting",
            Self::InvalidConfig => "Invalid config",
            Self::WaitingPermissions => "Waiting for permissions",
            Self::Ready => "Ready",
            Self::Retrying => "Retrying",
            Self::ShuttingDown => "Shutting down",
        }
    }
}

/// Connection component of the atomic runtime-health snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// No server connection is active.
    #[default]
    Disconnected,
    /// A connection attempt is in progress.
    Connecting,
    /// The app has an active server connection.
    Connected,
    /// The active connection is closing during shutdown.
    Closing,
}

impl ConnectionStatus {
    /// Stable diagnostic label for this state.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Closing => "closing",
        }
    }

    /// Human-readable label for normal UI surfaces.
    pub(crate) fn display_label(self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Closing => "Closing",
        }
    }
}

/// Availability of a user-initiated retry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RetryState {
    /// No retry is needed.
    #[default]
    Idle,
    /// The failed operation can be attempted again.
    Available,
    /// A retry is currently in progress.
    InProgress,
}

impl RetryState {
    /// Stable diagnostic label for this state.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Available => "available",
            Self::InProgress => "in_progress",
        }
    }

    /// Human-readable label for normal UI surfaces.
    pub(crate) fn display_label(self) -> &'static str {
        match self {
            Self::Idle => "Not needed",
            Self::Available => "Available",
            Self::InProgress => "In progress",
        }
    }
}

/// Complete UI-visible state of the app's runtime lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeHealth {
    /// Current lifecycle phase.
    pub(crate) phase: RuntimePhase,
    /// Current server connection state.
    pub(crate) connection: ConnectionStatus,
    /// Config currently installed in the engine.
    pub(crate) active_config: Option<PathBuf>,
    /// Candidate config awaiting activation or rejected by validation.
    pub(crate) pending_config: Option<PathBuf>,
    /// Latest runtime-owned macOS permission observation.
    pub(crate) permissions: PermissionsStatus,
    /// Whether the current failure can be retried.
    pub(crate) retry: RetryState,
    /// Optional user-facing context for the current phase.
    pub(crate) message: Option<String>,
}

impl Default for RuntimeHealth {
    fn default() -> Self {
        Self {
            phase: RuntimePhase::Disconnected,
            connection: ConnectionStatus::Disconnected,
            active_config: None,
            pending_config: None,
            permissions: PermissionsStatus::default(),
            retry: RetryState::Idle,
            message: None,
        }
    }
}

impl RuntimeHealth {
    /// Initial snapshot while the first connection and config activation run.
    pub(crate) fn connecting(config_path: PathBuf) -> Self {
        Self {
            phase: RuntimePhase::Connecting,
            connection: ConnectionStatus::Connecting,
            pending_config: Some(config_path),
            retry: RetryState::InProgress,
            ..Self::default()
        }
    }

    /// Whether the current snapshot has a live server connection.
    pub(crate) fn server_connected(&self) -> bool {
        matches!(self.connection, ConnectionStatus::Connected)
    }

    /// Compact label for Accessibility, Input Monitoring, and Screen Recording.
    pub(crate) fn permissions_label(&self) -> String {
        format!(
            "Accessibility {}, Input Monitoring {}, Screen Recording {}",
            permission_label(self.permissions.accessibility),
            permission_label(self.permissions.input_monitoring),
            permission_label(self.permissions.screen_recording),
        )
    }

    /// Config path to show in the editor, preferring an uncommitted candidate.
    pub(crate) fn displayed_config(&self) -> Option<&Path> {
        self.pending_config
            .as_deref()
            .or(self.active_config.as_deref())
    }

    /// Compact active-config label for constrained UI surfaces.
    pub(crate) fn active_config_label(&self) -> String {
        config_label(self.active_config.as_deref())
    }

    /// Compact pending-config label for constrained UI surfaces.
    pub(crate) fn pending_config_label(&self) -> String {
        config_label(self.pending_config.as_deref())
    }
}

/// Convert one permission value into a compact UI word.
fn permission_label(state: PermissionState) -> &'static str {
    match state {
        PermissionState::Granted => "granted",
        PermissionState::Denied => "denied",
        PermissionState::Unknown => "unknown",
    }
}

/// Format a config path for tray-sized UI, retaining the full path as a fallback.
fn config_label(path: Option<&Path>) -> String {
    let Some(path) = path else {
        return "None".to_string();
    };
    path.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| path.display().to_string(), str::to_string)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use permissions::{PermissionState, PermissionsStatus};

    use super::{ConnectionStatus, RetryState, RuntimeHealth, RuntimePhase};

    #[test]
    fn connecting_health_keeps_candidate_pending() {
        let health = RuntimeHealth::connecting(PathBuf::from("/tmp/hotki.luau"));

        assert_eq!(health.phase, RuntimePhase::Connecting);
        assert_eq!(health.connection, ConnectionStatus::Connecting);
        assert_eq!(health.active_config, None);
        assert_eq!(
            health.displayed_config(),
            Some(Path::new("/tmp/hotki.luau"))
        );
        assert_eq!(health.retry, RetryState::InProgress);
        assert!(!health.server_connected());
    }

    #[test]
    fn permission_label_preserves_each_capability() {
        let health = RuntimeHealth {
            permissions: PermissionsStatus {
                accessibility: PermissionState::Granted,
                input_monitoring: PermissionState::Denied,
                screen_recording: PermissionState::Unknown,
            },
            ..RuntimeHealth::default()
        };

        assert_eq!(
            health.permissions_label(),
            "Accessibility granted, Input Monitoring denied, Screen Recording unknown"
        );
    }
}
