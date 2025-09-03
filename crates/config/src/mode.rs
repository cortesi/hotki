use mac_keycode::Chord;
use serde::{Deserialize, Serialize};

use crate::{Toggle, raw};

/// Notification kinds for presenting command output in the host UI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationType {
    Info,
    Warn,
    Error,
    Success,
    /// Ignore any output; treat as Ok
    Ignore,
}

/// Attributes for key bindings
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct KeysAttrs {
    /// Don't exit the mode after executing this action
    #[serde(default)]
    pub noexit: Option<bool>,

    /// Key binding is global to this and all submodes
    #[serde(default)]
    pub global: Option<bool>,

    /// Hide this key binding from the HUD
    #[serde(default)]
    pub hide: Option<bool>,

    /// Only bind this key when the HUD is visible. Useful for setting global top-level bindings
    /// like "escape" to exit the HUD, while making sure they are only bound if the HUD is actually
    /// shown.
    #[serde(default)]
    pub hud_only: Option<bool>,

    /// Regex that must match the frontmost application name for a Mode action to be available
    #[serde(default)]
    pub match_app: Option<String>,

    /// Regex that must match the frontmost window title for a Mode action to be available
    #[serde(default)]
    pub match_title: Option<String>,

    /// Enable hold-to-repeat behavior for this binding (applies to shell and relay actions).
    /// If omitted, defaults to `noexit` (i.e., repeat=true when noexit=true).
    #[serde(default)]
    pub repeat: Option<bool>,

    /// Optional initial repeat delay override in milliseconds
    #[serde(default)]
    pub repeat_delay: Option<u64>,

    /// Optional repeat interval override in milliseconds
    #[serde(default)]
    pub repeat_interval: Option<u64>,

    /// Optional theme overlay (raw form) to apply when this binding's mode is active
    /// This is crate-internal to minimize the public API surface.
    #[serde(default)]
    pub(crate) style: Option<raw::RawStyle>,

    /// Capture all keys while this mode is active (when HUD is visible).
    ///
    /// When `true`, the hotkey system swallows all non-bound key presses
    /// so they are not delivered to the focused application. Only keys
    /// explicitly bound in the current mode (including inherited globals)
    /// are processed; everything else is ignored.
    #[serde(default)]
    pub capture: Option<bool>,
}

impl KeysAttrs {
    /// Effective repeat value; defaults to `noexit` when unset.
    pub fn repeat_effective(&self) -> bool {
        self.repeat.unwrap_or(self.noexit())
    }

    /// Return `noexit` (defaults to false when unset).
    pub fn noexit(&self) -> bool {
        self.noexit.unwrap_or(false)
    }
    /// Return `global` (defaults to false when unset).
    pub fn global(&self) -> bool {
        self.global.unwrap_or(false)
    }
    /// Return `hide` (defaults to false when unset).
    pub fn hide(&self) -> bool {
        self.hide.unwrap_or(false)
    }
    /// Return `hud_only` (defaults to false when unset).
    pub fn hud_only(&self) -> bool {
        self.hud_only.unwrap_or(false)
    }
    /// Return `capture` (defaults to false when unset).
    pub fn capture(&self) -> bool {
        self.capture.unwrap_or(false)
    }

    /// Merge another (child) attribute set on top of `self` (parent), obeying
    /// inheritance semantics for options: child's `Some` overrides; otherwise parent is kept.
    pub(crate) fn merged_with(&self, child: &KeysAttrs) -> KeysAttrs {
        KeysAttrs {
            noexit: child.noexit.or(self.noexit),
            global: child.global.or(self.global),
            hide: child.hide.or(self.hide),
            hud_only: child.hud_only.or(self.hud_only),
            match_app: child.match_app.clone().or(self.match_app.clone()),
            match_title: child.match_title.clone().or(self.match_title.clone()),
            repeat: child.repeat.or(self.repeat),
            repeat_delay: child.repeat_delay.or(self.repeat_delay),
            repeat_interval: child.repeat_interval.or(self.repeat_interval),
            style: child.style.clone(),
            capture: child.capture.or(self.capture),
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
    /// Control fullscreen: on/off/toggle with optional kind (native|nonnative)
    /// Syntax examples:
    /// - fullscreen(toggle)            // defaults to nonnative
    /// - fullscreen(on, native)
    /// - fullscreen(off, nonnative)
    Fullscreen(FullscreenSpec),
    /// Place the focused window into a grid cell on the current screen.
    ///
    /// Syntax:
    /// - place(grid(x, y), at(ix, iy))
    ///
    /// Constraints:
    /// - grid divisions x and y must be > 0
    /// - coordinates ix and iy are zero-based and must be within the grid
    Place(GridSpec, AtSpec),
    /// Move the focused window within a grid by one cell in the given direction.
    ///
    /// Syntax:
    /// - place_move(grid(x, y), left|right|up|down)
    ///
    /// Behavior:
    /// - If the window is not currently aligned to any cell in the grid,
    ///   the first invocation places it at (0, 0).
    /// - Movement clamps at the edges (no wrap-around).
    PlaceMove(GridSpec, MoveDir),
}

impl Action {
    /// Create a Shell action
    pub fn shell(cmd: impl Into<String>) -> Self {
        Action::Shell(ShellSpec::Cmd(cmd.into()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FullscreenKind {
    Native,
    Nonnative,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FullscreenSpec {
    One(Toggle),
    Two(Toggle, FullscreenKind),
}

// === Place action types ===

/// Grid divisions for the placement action. Zero is not allowed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Grid(pub u32, pub u32);

/// Zero-based coordinates within a grid for the placement action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct At(pub u32, pub u32);

/// Wrapper for `grid(x, y)`; named as an enum so RON supports `grid(…)` syntax.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GridSpec {
    Grid(Grid),
}

/// Wrapper for `at(ix, iy)`; named as an enum so RON supports `at(…)` syntax.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtSpec {
    At(At),
}

impl<'de> Deserialize<'de> for Grid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (x, y) = <(u32, u32)>::deserialize(deserializer)?;
        if x == 0 || y == 0 {
            return Err(serde::de::Error::custom(format!(
                "grid() divisions must be > 0; got (x={}, y={})",
                x, y
            )));
        }
        Ok(Grid(x, y))
    }
}

/// Direction for grid movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveDir {
    Left,
    Right,
    Up,
    Down,
}

/// Optional modifiers applied to Shell actions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellModifiers {
    /// Notification type for successful exit (status 0)
    /// Defaults to Ignore
    #[serde(default = "default_ok_notify")]
    pub ok_notify: NotificationType,

    /// Notification type for error exit (non-zero status)
    /// Defaults to Warn
    #[serde(default = "default_err_notify")]
    pub err_notify: NotificationType,
}

fn default_ok_notify() -> NotificationType {
    NotificationType::Ignore
}

fn default_err_notify() -> NotificationType {
    NotificationType::Warn
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
            ShellSpec::Cmd(c) => c,
            ShellSpec::WithMods(c, _) => c,
        }
    }

    /// Get notification type for successful exit
    pub fn ok_notify(&self) -> NotificationType {
        match self {
            ShellSpec::Cmd(_) => NotificationType::Ignore,
            ShellSpec::WithMods(_, m) => m.ok_notify,
        }
    }

    /// Get notification type for error exit
    pub fn err_notify(&self) -> NotificationType {
        match self {
            ShellSpec::Cmd(_) => NotificationType::Warn,
            ShellSpec::WithMods(_, m) => m.err_notify,
        }
    }
}

/// A collection of key bindings with their associated actions and descriptions.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Keys {
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
            Simple(String, String, Action),
            WithAttrs(String, String, Action, Box<KeysAttrs>),
        }

        let entries = Vec::<Entry>::deserialize(deserializer)?;
        let mut keys = Vec::with_capacity(entries.len());
        for e in entries {
            match e {
                Entry::Simple(k, n, a) => match Chord::parse(&k) {
                    Some(ch) => keys.push((ch, n, a, KeysAttrs::default())),
                    None => {
                        return Err(serde::de::Error::custom(format!(
                            "Failed to parse chord: {}",
                            k
                        )));
                    }
                },
                Entry::WithAttrs(k, n, a, attrs) => match Chord::parse(&k) {
                    Some(ch) => keys.push((ch, n, a, *attrs)),
                    None => {
                        return Err(serde::de::Error::custom(format!(
                            "Failed to parse chord: {}",
                            k
                        )));
                    }
                },
            }
        }
        Ok(Keys { keys })
    }
}

impl Keys {
    /// Create a `Keys` from a RON string.
    pub fn from_ron(ron_str: &str) -> Result<Self, crate::Error> {
        match ron::from_str::<Keys>(ron_str) {
            Ok(mode) => Ok(mode),
            Err(e) => Err(crate::Error::from_ron(ron_str, &e, None)),
        }
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

#[cfg(test)]
mod mode_err_tests {
    use super::*;

    #[test]
    fn mode_from_ron_error_is_config_error() {
        let ron = "[(\"BAD_KEY\", \"Desc\", exit)]"; // BAD_KEY is not a valid chord
        let err = Keys::from_ron(ron).unwrap_err();
        let pretty = err.pretty();
        assert!(pretty.contains("parse error"));
        assert!(pretty.contains("^"));
    }
}

#[cfg(test)]
mod fullscreen_parse_tests {
    use super::*;

    fn parse_keys(s: &str) -> Keys {
        Keys::from_ron(s).expect("parse")
    }

    #[test]
    fn parse_fullscreen_default_nonnative() {
        let k = parse_keys("[(\"f\", \"FS\", fullscreen(toggle))]");
        match &k.keys[0].2 {
            Action::Fullscreen(FullscreenSpec::One(Toggle::Toggle)) => {}
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn parse_fullscreen_with_kind() {
        let k = parse_keys("[(\"f\", \"FS\", fullscreen(on, native))]");
        match &k.keys[0].2 {
            Action::Fullscreen(FullscreenSpec::Two(Toggle::On, FullscreenKind::Native)) => {}
            other => panic!("unexpected: {:?}", other),
        }

        let k2 = parse_keys("[(\"f\", \"FS\", fullscreen(off, nonnative))]");
        match &k2.keys[0].2 {
            Action::Fullscreen(FullscreenSpec::Two(Toggle::Off, FullscreenKind::Nonnative)) => {}
            other => panic!("unexpected: {:?}", other),
        }
    }
}

#[cfg(test)]
mod place_parse_tests {
    use super::*;

    fn parse_keys(s: &str) -> Keys {
        Keys::from_ron(s).expect("parse")
    }

    #[test]
    fn parse_place_ok() {
        let k = parse_keys("[(\"g\", \"Left third\", place(grid(3, 1), at(0, 0)))]");
        match &k.keys[0].2 {
            Action::Place(GridSpec::Grid(Grid(gx, gy)), AtSpec::At(At(x, y))) => {
                assert_eq!((*gx, *gy, *x, *y), (3, 1, 0, 0));
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    // Parsing Actions standalone is not how user configs are fed; keep coverage via Keys.

    // No additional test here; Action parsing is covered via Keys parsing above.

    #[test]
    fn parse_fullscreen_action_only() {
        let a: Action = ron::from_str("fullscreen(toggle)").expect("action parse");
        match a {
            Action::Fullscreen(FullscreenSpec::One(Toggle::Toggle)) => {}
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn parse_grid_spec_zero_should_error() {
        let res: Result<GridSpec, _> = ron::from_str("grid(0, 1)");
        assert!(res.is_err());
    }

    // note: we intentionally parse grid()/at() only via PlaceSpec

    #[test]
    fn parse_place_grid_must_be_positive() {
        let err =
            Keys::from_ron("[(\"g\", \"Bad grid\", place(grid(0, 1), at(0, 0)))]").unwrap_err();
        // We at least fail to parse the entry
        let pretty = err.pretty();
        assert!(pretty.contains("Config parse error"), "{}", pretty);
    }

    // Note: coordinate range is validated at execution time where the focused
    // window's screen is known. Parsing ensures grid divisions are valid and
    // the DSL shape is correct.

    #[test]
    fn parse_place_move_ok() {
        let k = Keys::from_ron("[(\"g\", \"Move left\", place_move(grid(3, 2), left))]")
            .expect("parse");
        match &k.keys[0].2 {
            Action::PlaceMove(GridSpec::Grid(Grid(gx, gy)), MoveDir::Left) => {
                assert_eq!((*gx, *gy), (3, 2));
            }
            other => panic!("unexpected: {:?}", other),
        }
    }
}
