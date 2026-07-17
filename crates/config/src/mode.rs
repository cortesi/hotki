//! Primitive actions and shell specs used by Hotki.

pub use hotki_protocol::NotifyKind;
use serde::{Deserialize, Serialize};

use crate::Toggle;

/// Actions that can be triggered by hotkeys
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Execute a shell command (optionally with modifiers)
    Shell(ShellSpec),
    /// Execute a process directly without invoking a shell.
    Exec(ExecSpec),
    /// Relay a keystroke (with optional modifiers) to the currently
    /// focused application. Example: relay("cmd+shift+n").
    Relay(RelaySpec),
    /// Ask host application to reload its configuration
    ReloadConfig,
    /// Ask host to clear all on-screen notifications
    ClearNotifications,
    /// Control the main window: on/off/toggle
    ShowMainWindow(Toggle),
    /// Open a path or URL via the system opener.
    Open(String),
    /// Set the system volume to an absolute value (0-100)
    SetVolume(u8),
    /// Change the system volume by a relative amount (-100 to +100)
    ChangeVolume(i8),
    /// Control mute state: on/off/toggle
    Mute(Toggle),
}

/// Configured target for a relayed key gesture.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelayTarget {
    /// Deliver through the HID event stream to the focused application.
    Focused,
    /// Resolve one exact AppKit localized application name at actuation time.
    ApplicationName(String),
}

/// Chord and configured target for one relayed key gesture.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelaySpec {
    /// Chord string parsed by the engine at actuation time.
    pub chord: String,
    /// Configured destination identity.
    pub target: RelayTarget,
}

impl RelaySpec {
    /// Construct a relay to the focused application.
    pub fn focused(chord: impl Into<String>) -> Self {
        Self {
            chord: chord.into(),
            target: RelayTarget::Focused,
        }
    }

    /// Construct a relay to one exact AppKit localized application name.
    pub fn application(app_name: impl Into<String>, chord: impl Into<String>) -> Self {
        Self {
            chord: chord.into(),
            target: RelayTarget::ApplicationName(app_name.into()),
        }
    }
}

/// Specification for a direct process execution action.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecSpec {
    /// Program path or bare program name resolved by the operating system.
    pub program: String,
    /// Literal arguments passed to the program, preserving element boundaries.
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// Optional working directory, relative to the config entry when relative.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Notification type for successful exit.
    #[serde(default = "default_ok_notify")]
    pub ok_notify: NotifyKind,
    /// Notification type for error exit.
    #[serde(default = "default_err_notify")]
    pub err_notify: NotifyKind,
}

/// Optional modifiers applied to Shell actions
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellModifiers {
    /// Notification type for successful exit (status 0)
    /// Defaults to Ignore
    #[serde(default = "default_ok_notify")]
    pub ok_notify: NotifyKind,

    /// Notification type for error exit (non-zero status)
    /// Defaults to Warn
    #[serde(default = "default_err_notify")]
    pub err_notify: NotifyKind,
}

/// Serde default: successful shell command produces no notification.
fn default_ok_notify() -> NotifyKind {
    NotifyKind::Ignore
}

/// Serde default: shell command errors produce a warning notification.
fn default_err_notify() -> NotifyKind {
    NotifyKind::Warn
}

#[cfg(test)]
mod tests {
    use super::Action;

    #[test]
    fn action_serde_surface_excludes_navigation_requests() {
        assert_eq!(
            serde_json::from_str::<Action>(r#""reload_config""#).unwrap(),
            Action::ReloadConfig
        );

        for removed in ["pop", "exit", "show_root", "hide_hud"] {
            assert!(
                serde_json::from_str::<Action>(&format!(r#""{removed}""#)).is_err(),
                "removed navigation action {removed} still decoded"
            );
        }
    }
}

impl Default for ShellModifiers {
    fn default() -> Self {
        Self {
            ok_notify: default_ok_notify(),
            err_notify: default_err_notify(),
        }
    }
}

/// Specification for a Shell action: either just a command string, or a command
/// with modifiers, written as shell("cmd") or shell("cmd", (notify: info)).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ShellSpec {
    /// A bare shell command.
    Cmd(String),
    /// A shell command paired with modifiers such as notification preferences.
    WithMods(String, ShellModifiers),
}

impl ShellSpec {
    /// Return the underlying shell command string.
    pub fn command(&self) -> &str {
        match self {
            Self::Cmd(c) => c,
            Self::WithMods(c, _) => c,
        }
    }

    /// Get notification type for successful exit
    pub fn ok_notify(&self) -> NotifyKind {
        match self {
            Self::Cmd(_) => default_ok_notify(),
            Self::WithMods(_, m) => m.ok_notify,
        }
    }

    /// Get notification type for error exit
    pub fn err_notify(&self) -> NotifyKind {
        match self {
            Self::Cmd(_) => default_err_notify(),
            Self::WithMods(_, m) => m.err_notify,
        }
    }
}
