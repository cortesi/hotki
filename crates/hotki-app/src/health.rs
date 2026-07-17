//! Runtime health shared by the connection driver and every UI surface.

use std::path::PathBuf;

use hotki_protocol::{InputHealth, SecureInputOwner, SecureInputState, TapLifecycle, TapMode};
use permissions::PermissionsStatus;

/// Transition-stable input state used by normal UI presentation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputProjection {
    /// Whether the server owns a production physical event tap.
    pub(crate) tap_mode: TapMode,
    /// Current lifecycle of the event tap.
    pub(crate) tap_lifecycle: TapLifecycle,
    /// Last sampled Secure Input state.
    pub(crate) secure_input: SecureInputState,
    /// Best-effort owner identity at observation time.
    pub(crate) secure_input_owner: Option<SecureInputOwner>,
    /// Whether physical registered hotkeys are currently blocked.
    pub(crate) blocked: bool,
}

impl From<&InputHealth> for InputProjection {
    fn from(input: &InputHealth) -> Self {
        Self {
            tap_mode: input.tap_mode,
            tap_lifecycle: input.tap_lifecycle,
            secure_input: input.secure_input,
            secure_input_owner: input.secure_input_owner.clone(),
            blocked: input.blocked,
        }
    }
}

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

/// Semantic emphasis for a user-facing runtime notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeTone {
    /// A connection or recovery transition is in progress.
    Progress,
    /// User action is required before Hotki can continue.
    Attention,
    /// Hotki could not complete the requested runtime operation.
    Error,
}

impl NoticeTone {
    /// Stable value exposed to UI automation.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Progress => "progress",
            Self::Attention => "attention",
            Self::Error => "error",
        }
    }
}

/// Compact notice shown only while Hotki is transitioning or needs attention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeNotice {
    /// Strong primary copy shared by the main window and tray.
    pub(crate) title: &'static str,
    /// Optional explanation or next step.
    pub(crate) detail: Option<String>,
    /// Semantic emphasis for the notice mark.
    pub(crate) tone: NoticeTone,
    /// Whether the mark should animate as progress.
    pub(crate) progress: bool,
}

/// Intent of the leading command in the main-window footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimaryAction {
    /// Reload the currently selected configuration.
    ReloadConfig,
    /// Open Hotki's required-permissions helper.
    OpenPermissions,
    /// Retry the failed startup or configuration operation.
    TryAgain,
}

impl PrimaryAction {
    /// Short verb phrase used by normal UI surfaces.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ReloadConfig => "Reload Config",
            Self::OpenPermissions => "Open Permissions",
            Self::TryAgain => "Try Again",
        }
    }
}

/// Presentation-ready state shared by the main window and tray.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePresentation {
    /// Optional transition or problem notice.
    pub(crate) notice: Option<RuntimeNotice>,
    /// Optional leading command for the main-window footer.
    pub(crate) primary_action: Option<PrimaryAction>,
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
    /// Transition-stable physical-input health.
    pub(crate) input: InputProjection,
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
            input: InputProjection::default(),
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

    /// Derive the complete user-facing presentation from one runtime snapshot.
    pub(crate) fn presentation(&self) -> RuntimePresentation {
        let (notice, primary_action) = match self.phase {
            RuntimePhase::Ready if self.input.blocked => (
                Some(RuntimeNotice {
                    title: "Hotkeys paused by Secure Input",
                    detail: Some(secure_input_detail(&self.input)),
                    tone: NoticeTone::Attention,
                    progress: false,
                }),
                Some(PrimaryAction::ReloadConfig),
            ),
            RuntimePhase::Ready => (None, Some(PrimaryAction::ReloadConfig)),
            RuntimePhase::Connecting => (
                Some(RuntimeNotice {
                    title: "Starting Hotki…",
                    detail: Some("Connecting and loading the configuration.".to_string()),
                    tone: NoticeTone::Progress,
                    progress: true,
                }),
                None,
            ),
            RuntimePhase::Retrying => (
                Some(RuntimeNotice {
                    title: "Trying again…",
                    detail: Some("Reconnecting and reloading the configuration.".to_string()),
                    tone: NoticeTone::Progress,
                    progress: true,
                }),
                None,
            ),
            RuntimePhase::WaitingPermissions => (
                Some(RuntimeNotice {
                    title: "Hotki needs permission",
                    detail: missing_required_permissions(self.permissions),
                    tone: NoticeTone::Attention,
                    progress: false,
                }),
                Some(PrimaryAction::OpenPermissions),
            ),
            RuntimePhase::InvalidConfig => (
                Some(RuntimeNotice {
                    title: "Hotki couldn't load the configuration",
                    detail: Some(if self.active_config.is_some() {
                        "The previous configuration is still active.".to_string()
                    } else {
                        "No configuration is currently active.".to_string()
                    }),
                    tone: NoticeTone::Error,
                    progress: false,
                }),
                self.retry_action(),
            ),
            RuntimePhase::Disconnected => (
                Some(RuntimeNotice {
                    title: "Hotki isn't running",
                    detail: None,
                    tone: NoticeTone::Error,
                    progress: false,
                }),
                self.retry_action(),
            ),
            RuntimePhase::ShuttingDown => (None, None),
        };
        RuntimePresentation {
            notice,
            primary_action,
        }
    }

    /// Return retry intent only while the failed operation is user-retryable.
    fn retry_action(&self) -> Option<PrimaryAction> {
        matches!(self.retry, RetryState::Available).then_some(PrimaryAction::TryAgain)
    }
}

/// Qualify owner attribution without implying that it controls blocking.
fn secure_input_detail(input: &InputProjection) -> String {
    input.secure_input_owner.as_ref().map_or_else(
        || {
            "Another application enabled Secure Input. Hotkeys resume automatically when it ends."
                .to_string()
        },
        |owner| {
            format!(
                "{} may own Secure Input (best effort). Hotkeys resume automatically when it ends.",
                owner.app_name
            )
        },
    )
}

/// Explain which required permissions still need user action.
fn missing_required_permissions(status: PermissionsStatus) -> Option<String> {
    let mut missing = Vec::with_capacity(2);
    if !status.accessibility.is_granted() {
        missing.push("Accessibility");
    }
    if !status.input_monitoring.is_granted() {
        missing.push("Input Monitoring");
    }
    match missing.as_slice() {
        [] => None,
        [permission] => Some(format!("Grant {permission} in System Settings.")),
        [first, second] => Some(format!("Grant {first} and {second} in System Settings.")),
        _ => unreachable!("Hotki has exactly two required permissions"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use permissions::{PermissionState, PermissionsStatus};

    use super::{
        ConnectionStatus, InputProjection, NoticeTone, PrimaryAction, RetryState, RuntimeHealth,
        RuntimePhase,
    };

    fn health_for_phase(phase: RuntimePhase) -> RuntimeHealth {
        RuntimeHealth {
            phase,
            permissions: PermissionsStatus {
                accessibility: PermissionState::Granted,
                input_monitoring: PermissionState::Granted,
                screen_recording: PermissionState::Denied,
            },
            retry: RetryState::Available,
            ..RuntimeHealth::default()
        }
    }

    #[test]
    fn connecting_health_keeps_candidate_pending() {
        let health = RuntimeHealth::connecting(PathBuf::from("/tmp/hotki.luau"));

        assert_eq!(health.phase, RuntimePhase::Connecting);
        assert_eq!(health.connection, ConnectionStatus::Connecting);
        assert_eq!(health.active_config, None);
        assert_eq!(
            health.pending_config,
            Some(PathBuf::from("/tmp/hotki.luau"))
        );
        assert_eq!(health.retry, RetryState::InProgress);
        assert!(!health.server_connected());
    }

    #[test]
    fn every_runtime_phase_has_one_complete_presentation() {
        let cases = [
            (
                RuntimePhase::Disconnected,
                Some("Hotki isn't running"),
                None,
                Some(PrimaryAction::TryAgain),
                Some(NoticeTone::Error),
            ),
            (
                RuntimePhase::Connecting,
                Some("Starting Hotki…"),
                Some("Connecting and loading the configuration."),
                None,
                Some(NoticeTone::Progress),
            ),
            (
                RuntimePhase::InvalidConfig,
                Some("Hotki couldn't load the configuration"),
                Some("No configuration is currently active."),
                Some(PrimaryAction::TryAgain),
                Some(NoticeTone::Error),
            ),
            (
                RuntimePhase::WaitingPermissions,
                Some("Hotki needs permission"),
                None,
                Some(PrimaryAction::OpenPermissions),
                Some(NoticeTone::Attention),
            ),
            (
                RuntimePhase::Ready,
                None,
                None,
                Some(PrimaryAction::ReloadConfig),
                None,
            ),
            (
                RuntimePhase::Retrying,
                Some("Trying again…"),
                Some("Reconnecting and reloading the configuration."),
                None,
                Some(NoticeTone::Progress),
            ),
            (RuntimePhase::ShuttingDown, None, None, None, None),
        ];

        for (phase, title, detail, action, tone) in cases {
            let presentation = health_for_phase(phase).presentation();
            assert_eq!(
                presentation.notice.as_ref().map(|notice| notice.title),
                title
            );
            assert_eq!(
                presentation
                    .notice
                    .as_ref()
                    .and_then(|notice| notice.detail.as_deref()),
                detail
            );
            assert_eq!(presentation.primary_action, action);
            assert_eq!(presentation.notice.map(|notice| notice.tone), tone);
        }
    }

    #[test]
    fn required_permission_detail_names_only_missing_capabilities() {
        let cases = [
            (PermissionState::Granted, PermissionState::Granted, None),
            (
                PermissionState::Denied,
                PermissionState::Granted,
                Some("Grant Accessibility in System Settings."),
            ),
            (
                PermissionState::Granted,
                PermissionState::Denied,
                Some("Grant Input Monitoring in System Settings."),
            ),
            (
                PermissionState::Denied,
                PermissionState::Denied,
                Some("Grant Accessibility and Input Monitoring in System Settings."),
            ),
            (
                PermissionState::Unknown,
                PermissionState::Unknown,
                Some("Grant Accessibility and Input Monitoring in System Settings."),
            ),
        ];

        for (accessibility, input_monitoring, detail) in cases {
            let mut health = health_for_phase(RuntimePhase::WaitingPermissions);
            health.permissions.accessibility = accessibility;
            health.permissions.input_monitoring = input_monitoring;
            assert_eq!(
                health
                    .presentation()
                    .notice
                    .and_then(|notice| notice.detail)
                    .as_deref(),
                detail
            );
        }
    }

    #[test]
    fn secure_input_notice_is_ready_only_and_owner_is_qualified() {
        let mut ready = health_for_phase(RuntimePhase::Ready);
        ready.input = InputProjection {
            tap_mode: hotki_protocol::TapMode::Physical,
            tap_lifecycle: hotki_protocol::TapLifecycle::Running,
            secure_input: hotki_protocol::SecureInputState::Active,
            secure_input_owner: Some(hotki_protocol::SecureInputOwner {
                pid: 42,
                app_name: "Terminal".to_string(),
            }),
            blocked: true,
        };
        let presentation = ready.presentation();
        let notice = presentation.notice.expect("secure input notice");
        assert_eq!(notice.title, "Hotkeys paused by Secure Input");
        assert!(notice.detail.unwrap().contains("best effort"));
        assert_eq!(notice.tone, NoticeTone::Attention);

        ready.phase = RuntimePhase::InvalidConfig;
        assert_eq!(
            ready.presentation().notice.unwrap().title,
            "Hotki couldn't load the configuration"
        );
    }

    #[test]
    fn screen_recording_does_not_block_ready_presentation() {
        for screen_recording in [
            PermissionState::Granted,
            PermissionState::Denied,
            PermissionState::Unknown,
        ] {
            let mut health = health_for_phase(RuntimePhase::Ready);
            health.permissions.screen_recording = screen_recording;
            let presentation = health.presentation();
            assert!(presentation.notice.is_none());
            assert_eq!(
                presentation.primary_action,
                Some(PrimaryAction::ReloadConfig)
            );
        }
    }

    #[test]
    fn invalid_config_explains_active_predecessor() {
        let mut health = health_for_phase(RuntimePhase::InvalidConfig);
        assert_eq!(
            health
                .presentation()
                .notice
                .and_then(|notice| notice.detail),
            Some("No configuration is currently active.".to_string())
        );

        health.active_config = Some(PathBuf::from("/tmp/previous.luau"));
        assert_eq!(
            health
                .presentation()
                .notice
                .and_then(|notice| notice.detail),
            Some("The previous configuration is still active.".to_string())
        );
    }

    #[test]
    fn retry_action_tracks_availability_without_changing_notice() {
        for phase in [RuntimePhase::Disconnected, RuntimePhase::InvalidConfig] {
            for (retry, action) in [
                (RetryState::Idle, None),
                (RetryState::Available, Some(PrimaryAction::TryAgain)),
                (RetryState::InProgress, None),
            ] {
                let mut health = health_for_phase(phase);
                health.retry = retry;
                let presentation = health.presentation();
                assert!(presentation.notice.is_some());
                assert_eq!(presentation.primary_action, action);
            }
        }
    }

    #[test]
    fn progress_flag_is_independent_from_notice_tone() {
        assert!(
            health_for_phase(RuntimePhase::Connecting)
                .presentation()
                .notice
                .expect("connecting notice")
                .progress
        );
        assert!(
            !health_for_phase(RuntimePhase::WaitingPermissions)
                .presentation()
                .notice
                .expect("permission notice")
                .progress
        );
    }
}
