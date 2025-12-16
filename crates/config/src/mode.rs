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
    /// Relay a keystroke (with optional modifiers) to the currently
    /// focused application. Example: relay("cmd+shift+n").
    Relay(String),
    /// Return to the previous mode
    Pop,
    /// Pop all modes until the root mode is reached
    Exit,
    /// Ask host application to reload its configuration
    ReloadConfig,
    /// Ask host to clear all on-screen notifications
    ClearNotifications,
    /// Control the details window: on/off/toggle
    ShowDetails(Toggle),
    /// Switch to the next theme in the list
    ThemeNext,
    /// Switch to the previous theme in the list
    ThemePrev,
    /// Set a specific theme by name
    ThemeSet(String),
    /// Clear to root and show HUD.
    ShowRoot,
    /// Hide the HUD without changing the mode stack.
    HideHud,
    /// Set the system volume to an absolute value (0-100)
    SetVolume(u8),
    /// Change the system volume by a relative amount (-100 to +100)
    ChangeVolume(i8),
    /// Control mute state: on/off/toggle
    Mute(Toggle),
}

impl Action {
    /// Create a Shell action
    pub fn shell(cmd: impl Into<String>) -> Self {
        Self::Shell(ShellSpec::Cmd(cmd.into()))
    }
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
            Self::Cmd(_) => NotifyKind::Ignore,
            Self::WithMods(_, m) => m.ok_notify,
        }
    }

    /// Get notification type for error exit
    pub fn err_notify(&self) -> NotifyKind {
        match self {
            Self::Cmd(_) => NotifyKind::Warn,
            Self::WithMods(_, m) => m.err_notify,
        }
    }
}
