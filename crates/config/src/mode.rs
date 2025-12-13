//! Declarative configuration types for actions, modes, and key bindings.

pub use hotki_protocol::NotifyKind;
use mac_keycode::Chord;
use serde::{Deserialize, Serialize, de::Error as DeError};

use crate::{Toggle, raw};

/// Attributes for key bindings
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct KeysAttrs {
    /// Don't exit the mode after executing this action
    #[serde(default)]
    pub noexit: raw::Maybe<bool>,

    /// Key binding is global to this and all submodes
    #[serde(default)]
    pub global: raw::Maybe<bool>,

    /// Hide this key binding from the HUD
    #[serde(default)]
    pub hide: raw::Maybe<bool>,

    /// Only bind this key when the HUD is visible. Useful for setting global top-level bindings
    /// like "escape" to exit the HUD, while making sure they are only bound if the HUD is actually
    /// shown.
    #[serde(default)]
    pub hud_only: raw::Maybe<bool>,

    /// Regex that must match the frontmost application name for a Mode action to be available
    #[serde(default)]
    pub match_app: raw::Maybe<String>,

    /// Regex that must match the frontmost window title for a Mode action to be available
    #[serde(default)]
    pub match_title: raw::Maybe<String>,

    /// Enable hold-to-repeat behavior for this binding (applies to shell and relay actions).
    /// If omitted, defaults to `noexit` (i.e., repeat=true when noexit=true).
    #[serde(default)]
    pub repeat: raw::Maybe<bool>,

    /// Optional initial repeat delay override in milliseconds
    #[serde(default)]
    pub repeat_delay: raw::Maybe<u64>,

    /// Optional repeat interval override in milliseconds
    #[serde(default)]
    pub repeat_interval: raw::Maybe<u64>,

    /// Optional theme overlay (raw form) to apply when this binding's mode is active
    /// This is crate-internal to minimize the public API surface.
    #[serde(default)]
    pub(crate) style: raw::Maybe<raw::RawStyle>,

    /// Capture all keys while this mode is active (when HUD is visible).
    ///
    /// When `true`, the hotkey system swallows all non-bound key presses
    /// so they are not delivered to the focused application. Only keys
    /// explicitly bound in the current mode (including inherited globals)
    /// are processed; everything else is ignored.
    #[serde(default)]
    pub capture: raw::Maybe<bool>,
}

/// Generate boolean accessor methods that return `false` when the field is unset.
macro_rules! bool_accessors {
    ($($field:ident),+ $(,)?) => {
        $(
            #[doc = concat!("Return `", stringify!($field), "` (defaults to false when unset).")]
            pub fn $field(&self) -> bool {
                self.$field.as_option().copied().unwrap_or(false)
            }
        )+
    };
}

impl KeysAttrs {
    bool_accessors!(noexit, global, hide, hud_only, capture);

    /// Effective repeat value; defaults to `noexit` when unset.
    pub fn repeat_effective(&self) -> bool {
        self.repeat.as_option().copied().unwrap_or(self.noexit())
    }

    /// Merge another (child) attribute set on top of `self` (parent), obeying
    /// inheritance semantics for options: child's `Some` overrides; otherwise parent is kept.
    pub(crate) fn merged_with(&self, child: &Self) -> Self {
        fn merge_maybe<T: Clone>(parent: &raw::Maybe<T>, child: &raw::Maybe<T>) -> raw::Maybe<T> {
            match child.as_option() {
                Some(v) => raw::Maybe::Value(v.clone()),
                None => match parent.as_option() {
                    Some(v) => raw::Maybe::Value(v.clone()),
                    None => raw::Maybe::Unit(()),
                },
            }
        }

        Self {
            noexit: merge_maybe(&self.noexit, &child.noexit),
            global: merge_maybe(&self.global, &child.global),
            hide: merge_maybe(&self.hide, &child.hide),
            hud_only: merge_maybe(&self.hud_only, &child.hud_only),
            match_app: merge_maybe(&self.match_app, &child.match_app),
            match_title: merge_maybe(&self.match_title, &child.match_title),
            repeat: merge_maybe(&self.repeat, &child.repeat),
            repeat_delay: merge_maybe(&self.repeat_delay, &child.repeat_delay),
            repeat_interval: merge_maybe(&self.repeat_interval, &child.repeat_interval),
            style: child.style.clone(),
            capture: merge_maybe(&self.capture, &child.capture),
        }
    }
}

/// Actions that can be triggered by hotkeys
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Execute a shell command (optionally with modifiers)
    Shell(ShellSpec),
    /// Relay a keystroke (with optional modifiers) to the currently
    /// focused application. Example: relay("cmd+shift+n").
    Relay(String),
    /// Execute a script action registered by a Rhai config at load time.
    Rhai { id: u64 },
    /// Enter a nested keys section
    Keys(Keys),
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
    /// Show the HUD with root-level key bindings
    ShowHudRoot,
    /// Set the system volume to an absolute value (0-100)
    SetVolume(u8),
    /// Change the system volume by a relative amount (-100 to +100)
    ChangeVolume(i8),
    /// Control mute state: on/off/toggle
    Mute(Toggle),
    /// Control user style configuration: on/off/toggle
    UserStyle(Toggle),
}

impl Action {
    /// Create a Shell action
    pub fn shell(cmd: impl Into<String>) -> Self {
        Self::Shell(ShellSpec::Cmd(cmd.into()))
    }
}

/// Optional modifiers applied to Shell actions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ShellSpec {
    Cmd(String),
    WithMods(String, ShellModifiers),
}

impl ShellSpec {
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

/// A collection of key bindings with their associated actions and descriptions.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Keys {
    /// The list of key bindings: `(chord, description, action, attributes)`.
    pub(crate) keys: Vec<(Chord, String, Action, KeysAttrs)>,
}

// Manual Serialize implementation that respects transparent
impl Serialize for Keys {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.keys.len()))?;
        for (key, desc, action, attrs) in &self.keys {
            // Serialize as a tuple with key converted to string
            if attrs == &KeysAttrs::default() {
                seq.serialize_element(&(key.to_string(), desc, action))?;
            } else {
                seq.serialize_element(&(key.to_string(), desc, action, attrs))?;
            }
        }
        seq.end()
    }
}

// Custom deserializer that accepts both 3-tuples and 4-tuples
impl<'de> Deserialize<'de> for Keys {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Entry {
            /// Tuple form: `(key, description, action)`.
            Simple(String, String, Action),
            /// Tuple form with attributes: `(key, description, action, attrs)`.
            WithAttrs(String, String, Action, Box<KeysAttrs>),
        }

        let entries = Vec::<Entry>::deserialize(deserializer)?;
        let mut keys = Vec::with_capacity(entries.len());
        for e in entries {
            match e {
                Entry::Simple(k, n, a) => match Chord::parse(&k) {
                    Some(ch) => keys.push((ch, n, a, KeysAttrs::default())),
                    None => {
                        return Err(DeError::custom(format!("Failed to parse chord: {}", k)));
                    }
                },
                Entry::WithAttrs(k, n, a, attrs) => match Chord::parse(&k) {
                    Some(ch) => keys.push((ch, n, a, *attrs)),
                    None => {
                        return Err(DeError::custom(format!("Failed to parse chord: {}", k)));
                    }
                },
            }
        }
        Ok(Self { keys })
    }
}

impl Keys {
    /// Get the action and attributes associated with a key.
    pub(crate) fn get_with_attrs(&self, key: &Chord) -> Option<(&Action, &KeysAttrs)> {
        self.keys
            .iter()
            .find(|(k, _, _, _)| k == key)
            .map(|(_, _, action, attrs)| (action, attrs))
    }

    /// Get all keys in this mode.
    ///
    /// Returns an iterator over tuples of (key_string, description)
    pub fn keys(&self) -> impl Iterator<Item = (String, &str)> + '_ {
        self.keys
            .iter()
            .map(|(k, desc, _, _)| (k.to_string(), desc.as_str()))
    }

    /// Get all `Chord` objects in this mode.
    pub fn key_objects(&self) -> impl Iterator<Item = &Chord> + '_ {
        self.keys.iter().map(|(k, _, _, _)| k)
    }

    /// Get all keys with their names and attributes.
    pub(crate) fn keys_with_attrs(&self) -> impl Iterator<Item = (Chord, String, KeysAttrs)> + '_ {
        self.keys
            .iter()
            .map(|(k, desc, _, attrs)| (k.clone(), desc.clone(), attrs.clone()))
    }

    // Note: additional convenience getters were removed as unused to keep API minimal.
}

// Window-management parsing tests removed alongside the action variants.
