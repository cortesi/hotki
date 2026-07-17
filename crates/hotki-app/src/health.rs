//! Runtime health shared by the connection driver and every UI surface.

use std::path::{Path, PathBuf};

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

/// Valid lifecycle states for the app's runtime lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeState {
    /// No server connection is active.
    Disconnected {
        /// Failure context when a retry is available.
        message: Option<String>,
    },
    /// The first server connection and config activation are in progress.
    InitialConnection {
        /// Config candidate being activated.
        candidate: PathBuf,
    },
    /// A reconnect has been requested but no connection attempt is active yet.
    QueuedReconnect {
        /// Config candidate to activate after reconnecting.
        candidate: PathBuf,
    },
    /// A post-failure server connection attempt is active.
    Reconnecting {
        /// Config candidate being activated after reconnecting.
        candidate: PathBuf,
    },
    /// A connected server is activating a replacement config.
    LiveConfigReload {
        /// Config that remains active until the candidate is accepted.
        active: Option<PathBuf>,
        /// Config candidate being activated.
        candidate: PathBuf,
    },
    /// Required permissions are missing.
    PermissionBlocked {
        /// Config active in a live server, if the server remains available.
        active: Option<PathBuf>,
        /// Whether a live server remains available.
        live_runtime: bool,
    },
    /// A connected server is running the active config.
    Running {
        /// Config installed in the engine.
        active: PathBuf,
    },
    /// A connected server rejected a config candidate.
    ConfigRejected {
        /// Previous config that remains active, if any.
        active: Option<PathBuf>,
        /// Rejected config candidate.
        candidate: PathBuf,
        /// Validation or activation failure.
        message: String,
    },
    /// Owned runtime work is finishing before the app exits.
    ShuttingDown {
        /// Whether a live server connection is closing.
        live_runtime: bool,
    },
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self::Disconnected { message: None }
    }
}

/// Complete UI-visible state of the app's runtime lane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeHealth {
    /// Valid-by-construction runtime lifecycle.
    pub(crate) state: RuntimeState,
    /// Latest runtime-owned macOS permission observation.
    pub(crate) permissions: PermissionsStatus,
    /// Transition-stable physical-input health.
    pub(crate) input: InputProjection,
}

impl RuntimeHealth {
    /// Initial snapshot while the first connection and config activation run.
    pub(crate) fn connecting(config_path: PathBuf) -> Self {
        Self {
            state: RuntimeState::InitialConnection {
                candidate: config_path,
            },
            ..Self::default()
        }
    }

    /// Record an offline failure that can be retried.
    pub(crate) fn disconnect(&mut self, message: impl Into<String>) {
        self.state = RuntimeState::Disconnected {
            message: Some(message.into()),
        };
    }

    /// Begin the current initial connection or a post-failure reconnect.
    pub(crate) fn start_connecting(&mut self, candidate: PathBuf) {
        self.state = if matches!(self.state, RuntimeState::InitialConnection { .. }) {
            RuntimeState::InitialConnection { candidate }
        } else {
            RuntimeState::Reconnecting { candidate }
        };
    }

    /// Queue a reconnect before a connection attempt can start.
    pub(crate) fn queue_reconnect(&mut self, candidate: PathBuf) {
        self.state = RuntimeState::QueuedReconnect { candidate };
    }

    /// Record missing permissions while preserving whether the server remains live.
    pub(crate) fn block_on_permissions(&mut self) {
        let live_runtime = self.server_connected();
        let active = self.active_config().map(Path::to_path_buf);
        self.state = RuntimeState::PermissionBlocked {
            active,
            live_runtime,
        };
    }

    /// Record an acknowledged active config and ready server.
    pub(crate) fn run_config(&mut self, active: PathBuf) {
        self.state = RuntimeState::Running { active };
    }

    /// Begin activating a replacement config on a live server.
    pub(crate) fn start_config_reload(&mut self, candidate: PathBuf) {
        let active = self.active_config().map(Path::to_path_buf);
        self.state = RuntimeState::LiveConfigReload { active, candidate };
    }

    /// Record a rejected candidate while retaining the previous live config.
    pub(crate) fn reject_config(&mut self, candidate: PathBuf, message: impl Into<String>) {
        let active = self.active_config().map(Path::to_path_buf);
        self.state = RuntimeState::ConfigRejected {
            active,
            candidate,
            message: message.into(),
        };
    }

    /// Record shutdown before waiting for owned runtime work to finish.
    pub(crate) fn begin_shutdown(&mut self) {
        self.state = RuntimeState::ShuttingDown {
            live_runtime: self.server_connected(),
        };
    }

    /// Stable diagnostic phase projection.
    pub(crate) fn phase(&self) -> RuntimePhase {
        match self.state {
            RuntimeState::Disconnected { .. } => RuntimePhase::Disconnected,
            RuntimeState::InitialConnection { .. } => RuntimePhase::Connecting,
            RuntimeState::QueuedReconnect { .. }
            | RuntimeState::Reconnecting { .. }
            | RuntimeState::LiveConfigReload { .. } => RuntimePhase::Retrying,
            RuntimeState::PermissionBlocked { .. } => RuntimePhase::WaitingPermissions,
            RuntimeState::Running { .. } => RuntimePhase::Ready,
            RuntimeState::ConfigRejected { .. } => RuntimePhase::InvalidConfig,
            RuntimeState::ShuttingDown { .. } => RuntimePhase::ShuttingDown,
        }
    }

    /// Stable diagnostic connection projection.
    pub(crate) fn connection(&self) -> ConnectionStatus {
        match self.state {
            RuntimeState::Disconnected { .. }
            | RuntimeState::QueuedReconnect { .. }
            | RuntimeState::PermissionBlocked {
                live_runtime: false,
                ..
            }
            | RuntimeState::ShuttingDown {
                live_runtime: false,
            } => ConnectionStatus::Disconnected,
            RuntimeState::InitialConnection { .. } | RuntimeState::Reconnecting { .. } => {
                ConnectionStatus::Connecting
            }
            RuntimeState::LiveConfigReload { .. }
            | RuntimeState::PermissionBlocked {
                live_runtime: true, ..
            }
            | RuntimeState::Running { .. }
            | RuntimeState::ConfigRejected { .. } => ConnectionStatus::Connected,
            RuntimeState::ShuttingDown { live_runtime: true } => ConnectionStatus::Closing,
        }
    }

    /// Stable diagnostic retry projection.
    pub(crate) fn retry(&self) -> RetryState {
        match self.state {
            RuntimeState::Disconnected { message: None }
            | RuntimeState::Running { .. }
            | RuntimeState::ShuttingDown { .. } => RetryState::Idle,
            RuntimeState::Disconnected { message: Some(_) }
            | RuntimeState::PermissionBlocked { .. }
            | RuntimeState::ConfigRejected { .. } => RetryState::Available,
            RuntimeState::InitialConnection { .. }
            | RuntimeState::QueuedReconnect { .. }
            | RuntimeState::Reconnecting { .. }
            | RuntimeState::LiveConfigReload { .. } => RetryState::InProgress,
        }
    }

    /// Config currently installed in a live engine.
    pub(crate) fn active_config(&self) -> Option<&Path> {
        match &self.state {
            RuntimeState::LiveConfigReload { active, .. }
            | RuntimeState::PermissionBlocked { active, .. }
            | RuntimeState::ConfigRejected { active, .. } => active.as_deref(),
            RuntimeState::Running { active } => Some(active.as_path()),
            RuntimeState::Disconnected { .. }
            | RuntimeState::InitialConnection { .. }
            | RuntimeState::QueuedReconnect { .. }
            | RuntimeState::Reconnecting { .. }
            | RuntimeState::ShuttingDown { .. } => None,
        }
    }

    /// Config candidate awaiting activation or rejected by validation.
    pub(crate) fn pending_config(&self) -> Option<&Path> {
        match &self.state {
            RuntimeState::InitialConnection { candidate }
            | RuntimeState::QueuedReconnect { candidate }
            | RuntimeState::Reconnecting { candidate }
            | RuntimeState::LiveConfigReload { candidate, .. }
            | RuntimeState::ConfigRejected { candidate, .. } => Some(candidate.as_path()),
            RuntimeState::Disconnected { .. }
            | RuntimeState::PermissionBlocked { .. }
            | RuntimeState::Running { .. }
            | RuntimeState::ShuttingDown { .. } => None,
        }
    }

    /// Whether the current snapshot has a live server connection.
    pub(crate) fn server_connected(&self) -> bool {
        matches!(self.connection(), ConnectionStatus::Connected)
    }

    /// Whether owned runtime work is finishing.
    pub(crate) fn is_shutting_down(&self) -> bool {
        matches!(self.state, RuntimeState::ShuttingDown { .. })
    }

    /// Derive the complete user-facing presentation from one runtime snapshot.
    pub(crate) fn presentation(&self) -> RuntimePresentation {
        let (notice, primary_action) = match self.phase() {
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
                    detail: Some(if self.active_config().is_some() {
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
        matches!(self.retry(), RetryState::Available).then_some(PrimaryAction::TryAgain)
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
    use std::path::{Path, PathBuf};

    use permissions::{PermissionState, PermissionsStatus};

    use super::{
        ConnectionStatus, InputProjection, NoticeTone, PrimaryAction, RetryState, RuntimeHealth,
        RuntimePhase, RuntimeState,
    };

    fn health_for_phase(phase: RuntimePhase) -> RuntimeHealth {
        RuntimeHealth {
            state: match phase {
                RuntimePhase::Disconnected => RuntimeState::Disconnected {
                    message: Some("connection failed".to_string()),
                },
                RuntimePhase::Connecting => RuntimeState::InitialConnection {
                    candidate: PathBuf::from("/tmp/hotki.luau"),
                },
                RuntimePhase::InvalidConfig => RuntimeState::ConfigRejected {
                    active: None,
                    candidate: PathBuf::from("/tmp/hotki.luau"),
                    message: "candidate rejected".to_string(),
                },
                RuntimePhase::WaitingPermissions => RuntimeState::PermissionBlocked {
                    active: None,
                    live_runtime: false,
                },
                RuntimePhase::Ready => RuntimeState::Running {
                    active: PathBuf::from("/tmp/hotki.luau"),
                },
                RuntimePhase::Retrying => RuntimeState::QueuedReconnect {
                    candidate: PathBuf::from("/tmp/hotki.luau"),
                },
                RuntimePhase::ShuttingDown => RuntimeState::ShuttingDown {
                    live_runtime: false,
                },
            },
            permissions: PermissionsStatus {
                accessibility: PermissionState::Granted,
                input_monitoring: PermissionState::Granted,
                screen_recording: PermissionState::Denied,
            },
            ..RuntimeHealth::default()
        }
    }

    #[test]
    fn connecting_health_keeps_candidate_pending() {
        let health = RuntimeHealth::connecting(PathBuf::from("/tmp/hotki.luau"));

        assert_eq!(health.phase(), RuntimePhase::Connecting);
        assert_eq!(health.connection(), ConnectionStatus::Connecting);
        assert_eq!(health.active_config(), None);
        assert_eq!(health.pending_config(), Some(Path::new("/tmp/hotki.luau")));
        assert_eq!(health.retry(), RetryState::InProgress);
        assert!(!health.server_connected());
    }

    #[test]
    fn every_lifecycle_variant_has_stable_projections() {
        let candidate = PathBuf::from("/tmp/candidate.luau");
        let active = PathBuf::from("/tmp/active.luau");
        let cases = [
            (
                RuntimeState::Disconnected { message: None },
                RuntimePhase::Disconnected,
                ConnectionStatus::Disconnected,
                RetryState::Idle,
            ),
            (
                RuntimeState::Disconnected {
                    message: Some("failed".to_string()),
                },
                RuntimePhase::Disconnected,
                ConnectionStatus::Disconnected,
                RetryState::Available,
            ),
            (
                RuntimeState::InitialConnection {
                    candidate: candidate.clone(),
                },
                RuntimePhase::Connecting,
                ConnectionStatus::Connecting,
                RetryState::InProgress,
            ),
            (
                RuntimeState::QueuedReconnect {
                    candidate: candidate.clone(),
                },
                RuntimePhase::Retrying,
                ConnectionStatus::Disconnected,
                RetryState::InProgress,
            ),
            (
                RuntimeState::Reconnecting {
                    candidate: candidate.clone(),
                },
                RuntimePhase::Retrying,
                ConnectionStatus::Connecting,
                RetryState::InProgress,
            ),
            (
                RuntimeState::LiveConfigReload {
                    active: Some(active.clone()),
                    candidate: candidate.clone(),
                },
                RuntimePhase::Retrying,
                ConnectionStatus::Connected,
                RetryState::InProgress,
            ),
            (
                RuntimeState::PermissionBlocked {
                    active: None,
                    live_runtime: false,
                },
                RuntimePhase::WaitingPermissions,
                ConnectionStatus::Disconnected,
                RetryState::Available,
            ),
            (
                RuntimeState::PermissionBlocked {
                    active: Some(active.clone()),
                    live_runtime: true,
                },
                RuntimePhase::WaitingPermissions,
                ConnectionStatus::Connected,
                RetryState::Available,
            ),
            (
                RuntimeState::Running {
                    active: active.clone(),
                },
                RuntimePhase::Ready,
                ConnectionStatus::Connected,
                RetryState::Idle,
            ),
            (
                RuntimeState::ConfigRejected {
                    active: Some(active),
                    candidate,
                    message: "rejected".to_string(),
                },
                RuntimePhase::InvalidConfig,
                ConnectionStatus::Connected,
                RetryState::Available,
            ),
            (
                RuntimeState::ShuttingDown {
                    live_runtime: false,
                },
                RuntimePhase::ShuttingDown,
                ConnectionStatus::Disconnected,
                RetryState::Idle,
            ),
            (
                RuntimeState::ShuttingDown { live_runtime: true },
                RuntimePhase::ShuttingDown,
                ConnectionStatus::Closing,
                RetryState::Idle,
            ),
        ];

        for (state, phase, connection, retry) in cases {
            let health = RuntimeHealth {
                state,
                ..RuntimeHealth::default()
            };
            assert_eq!(health.phase(), phase);
            assert_eq!(health.connection(), connection);
            assert_eq!(health.retry(), retry);
        }
    }

    #[test]
    fn transitions_retain_only_state_valid_data() {
        let mut health = RuntimeHealth::connecting(PathBuf::from("/tmp/first.luau"));
        health.disconnect("offline");
        assert!(matches!(
            &health.state,
            RuntimeState::Disconnected {
                message: Some(message)
            } if message == "offline"
        ));

        health.queue_reconnect(PathBuf::from("/tmp/retry.luau"));
        health.start_connecting(PathBuf::from("/tmp/retry.luau"));
        assert!(matches!(health.state, RuntimeState::Reconnecting { .. }));

        health.run_config(PathBuf::from("/tmp/active.luau"));
        health.start_config_reload(PathBuf::from("/tmp/candidate.luau"));
        health.reject_config(PathBuf::from("/tmp/candidate.luau"), "invalid");
        assert_eq!(health.active_config(), Some(Path::new("/tmp/active.luau")));
        assert!(matches!(
            &health.state,
            RuntimeState::ConfigRejected { message, .. } if message == "invalid"
        ));

        health.block_on_permissions();
        assert!(health.server_connected());
        health.begin_shutdown();
        assert_eq!(health.connection(), ConnectionStatus::Closing);
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

        ready.reject_config(PathBuf::from("/tmp/candidate.luau"), "candidate rejected");
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

        health.run_config(PathBuf::from("/tmp/previous.luau"));
        health.reject_config(PathBuf::from("/tmp/candidate.luau"), "candidate rejected");
        assert_eq!(
            health
                .presentation()
                .notice
                .and_then(|notice| notice.detail),
            Some("The previous configuration is still active.".to_string())
        );
    }

    #[test]
    fn retry_action_is_derived_from_retryable_states() {
        let idle = RuntimeHealth::default().presentation();
        assert!(idle.notice.is_some());
        assert_eq!(idle.primary_action, None);

        let disconnected = health_for_phase(RuntimePhase::Disconnected).presentation();
        assert!(disconnected.notice.is_some());
        assert_eq!(disconnected.primary_action, Some(PrimaryAction::TryAgain));

        let rejected = health_for_phase(RuntimePhase::InvalidConfig).presentation();
        assert!(rejected.notice.is_some());
        assert_eq!(rejected.primary_action, Some(PrimaryAction::TryAgain));
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
