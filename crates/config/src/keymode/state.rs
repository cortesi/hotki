use hotki_protocol::MsgToUI;
use mac_keycode::Chord;
use thiserror::Error;

use crate::{Action, KeysAttrs, NotifyKind};

/// Error type for keymode state handling
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum KeymodeError {
    /// Invalid relay keyspec string
    #[error("Invalid relay keyspec '{spec}'")]
    InvalidRelayKeyspec { spec: String },
}

/// Result of handling a key press
#[derive(Debug)]
#[allow(missing_docs)]
pub enum KeyResponse {
    /// No message; operation succeeded
    Ok,
    /// Informational message to display to the user
    Info { title: String, text: String },
    /// Warning message to display to the user
    Warn { title: String, text: String },
    /// Error message to display to the user
    Error { title: String, text: String },
    /// Success message to display to the user
    Success { title: String, text: String },
    /// UI message to be forwarded to clients
    Ui(MsgToUI),
    /// Relay a chord to the focused application with attributes
    Relay { chord: Chord, attrs: Box<KeysAttrs> },
    /// Shell command to execute asynchronously
    ShellAsync {
        command: String,
        ok_notify: NotifyKind,
        err_notify: NotifyKind,
        /// Optional software repeat configuration (only populated when attrs.noexit() && repeat)
        repeat: Option<ShellRepeatConfig>,
    },
}

/// Optional repeat configuration for shell actions
#[derive(Debug, Clone, Copy)]
pub struct ShellRepeatConfig {
    /// Optional initial delay before first repeat (milliseconds).
    pub initial_delay_ms: Option<u64>,
    /// Optional interval between repeats (milliseconds).
    pub interval_ms: Option<u64>,
}

/// Tracks only the logical cursor within the key hierarchy.
#[derive(Debug, Default)]
pub struct State {
    /// Current position within the configured key hierarchy.
    cursor: crate::Cursor,
}

impl State {
    /// Create a new state (root path, HUD hidden).
    pub fn new() -> Self {
        Self {
            cursor: crate::Cursor::default(),
        }
    }

    /// Execute an action with the given attributes
    fn execute_action(
        &mut self,
        action: &Action,
        attrs: &KeysAttrs,
        entered_index: Option<usize>,
    ) -> Result<KeyResponse, KeymodeError> {
        match action {
            Action::Keys(new_mode) => {
                let _ = new_mode; // contents live in Config; we just advance cursor
                if let Some(i) = entered_index {
                    self.cursor.push(i as u32);
                }
                Ok(KeyResponse::Ok)
            }
            Action::Relay(spec) => self.handle_relay(spec, attrs),
            Action::Pop => {
                if self.cursor.depth() > 0 {
                    let _ = self.cursor.pop();
                } else if self.cursor.viewing_root {
                    self.cursor.viewing_root = false;
                }
                Ok(KeyResponse::Ok)
            }
            Action::Exit => {
                self.reset();
                Ok(KeyResponse::Ok)
            }
            Action::Shell(spec) => self.handle_shell(spec, attrs),
            Action::ReloadConfig => self.simple_ui(MsgToUI::ReloadConfig, attrs),
            Action::ClearNotifications => self.simple_ui(MsgToUI::ClearNotifications, attrs),
            Action::ShowDetails(arg) => self.simple_ui(MsgToUI::ShowDetails(*arg), attrs),
            Action::ThemeNext => self.simple_ui(MsgToUI::ThemeNext, attrs),
            Action::ThemePrev => self.simple_ui(MsgToUI::ThemePrev, attrs),
            Action::ThemeSet(name) => self.simple_ui(MsgToUI::ThemeSet(name.clone()), attrs),
            Action::ShowHudRoot => {
                self.reset();
                self.cursor.viewing_root = true;
                Ok(KeyResponse::Ok)
            }
            Action::SetVolume(level) => self.handle_set_volume(*level, attrs),
            Action::ChangeVolume(delta) => self.handle_change_volume(*delta, attrs),
            Action::Mute(arg) => self.handle_mute(*arg, attrs),
            Action::UserStyle(arg) => {
                let response = KeyResponse::Ui(MsgToUI::UserStyle(*arg));
                if !attrs.noexit() {
                    self.reset();
                }
                Ok(response)
            }
        }
    }

    /// Parse and relay a chord string, carrying attributes through.
    fn handle_relay(&mut self, spec: &str, attrs: &KeysAttrs) -> Result<KeyResponse, KeymodeError> {
        match Chord::parse(spec) {
            Some(ch) => {
                let response = KeyResponse::Relay {
                    chord: ch,
                    attrs: Box::new(attrs.clone()),
                };
                if !attrs.noexit() {
                    self.reset();
                }
                Ok(response)
            }
            None => Err(KeymodeError::InvalidRelayKeyspec {
                spec: spec.to_string(),
            }),
        }
    }

    /// Build a `ShellAsync` response, attaching repeat configuration if effective.
    fn handle_shell(
        &mut self,
        spec: &crate::ShellSpec,
        attrs: &KeysAttrs,
    ) -> Result<KeyResponse, KeymodeError> {
        let mut repeat = None;
        if attrs.noexit() && attrs.repeat_effective() {
            repeat = Some(ShellRepeatConfig {
                initial_delay_ms: attrs.repeat_delay,
                interval_ms: attrs.repeat_interval,
            });
        }
        let response = KeyResponse::ShellAsync {
            command: spec.command().to_string(),
            ok_notify: spec.ok_notify(),
            err_notify: spec.err_notify(),
            repeat,
        };
        if !attrs.noexit() {
            self.reset();
        }
        Ok(response)
    }

    /// Convenience to wrap a UI message and reset when appropriate.
    fn simple_ui(&mut self, msg: MsgToUI, attrs: &KeysAttrs) -> Result<KeyResponse, KeymodeError> {
        let response = KeyResponse::Ui(msg);
        if !attrs.noexit() {
            self.reset();
        }
        Ok(response)
    }

    /// Build a shell command to set system output volume to an absolute level.
    fn handle_set_volume(
        &mut self,
        level: u8,
        attrs: &KeysAttrs,
    ) -> Result<KeyResponse, KeymodeError> {
        let script = format!("set volume output volume {}", (level).min(100));
        let response = KeyResponse::ShellAsync {
            command: format!("osascript -e '{}'", script),
            ok_notify: NotifyKind::Ignore,
            err_notify: NotifyKind::Warn,
            repeat: None,
        };
        if !attrs.noexit() {
            self.reset();
        }
        Ok(response)
    }

    /// Build a shell command to change system output volume by a delta.
    fn handle_change_volume(
        &mut self,
        delta: i8,
        attrs: &KeysAttrs,
    ) -> Result<KeyResponse, KeymodeError> {
        let script = format!(
            "set currentVolume to output volume of (get volume settings)\nset volume output volume (currentVolume + {})",
            delta
        );
        let mut repeat = None;
        if attrs.noexit() && attrs.repeat_effective() {
            repeat = Some(ShellRepeatConfig {
                initial_delay_ms: attrs.repeat_delay,
                interval_ms: attrs.repeat_interval,
            });
        }
        let response = KeyResponse::ShellAsync {
            command: format!("osascript -e '{}'", script.replace('\n', "' -e '")),
            ok_notify: NotifyKind::Ignore,
            err_notify: NotifyKind::Warn,
            repeat,
        };
        if !attrs.noexit() {
            self.reset();
        }
        Ok(response)
    }

    /// Build a shell command to toggle or set system mute state.
    fn handle_mute(
        &mut self,
        arg: crate::Toggle,
        attrs: &KeysAttrs,
    ) -> Result<KeyResponse, KeymodeError> {
        let script = match arg {
            crate::Toggle::On => "set volume output muted true".to_string(),
            crate::Toggle::Off => "set volume output muted false".to_string(),
            crate::Toggle::Toggle => {
                "set curMuted to output muted of (get volume settings)\nset volume output muted not curMuted".to_string()
            }
        };
        let response = KeyResponse::ShellAsync {
            command: format!("osascript -e '{}'", script.replace('\n', "' -e '")),
            ok_notify: NotifyKind::Ignore,
            err_notify: NotifyKind::Warn,
            repeat: None,
        };
        if !attrs.noexit() {
            self.reset();
        }
        Ok(response)
    }

    /// Reset to root (clear path and hide viewing_root).
    fn reset(&mut self) {
        self.cursor.clear();
        self.cursor.viewing_root = false;
    }

    /// Get the current mode depth (0 = root)
    pub fn depth(&self) -> usize {
        self.cursor.depth()
    }

    /// Ensure context by popping while guards on the entering entries do not match.
    pub fn ensure_context(&mut self, cfg: &crate::Config, app: &str, title: &str) -> bool {
        let (next, changed) = crate::CursorEnsureExt::ensure_in(&self.cursor, cfg, app, title);
        self.cursor = next;
        changed
    }

    /// Return the current cursor (version is set by the caller before sending to UI).
    pub fn current_cursor(&self) -> crate::Cursor {
        self.cursor.clone()
    }

    /// Process a key press with app/title context.
    pub fn handle_key_with_context(
        &mut self,
        cfg: &crate::Config,
        key: &Chord,
        app: &str,
        title: &str,
    ) -> Result<KeyResponse, KeymodeError> {
        if let Some((action, attrs, entered_index)) = cfg.action(&self.cursor, key, app, title) {
            return self.execute_action(&action, &attrs, entered_index);
        }
        Ok(KeyResponse::Ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Keys;

    fn chord(s: &str) -> Chord {
        mac_keycode::Chord::parse(s).unwrap()
    }

    fn press(
        state: &mut State,
        cfg: &crate::Config,
        chord: &Chord,
    ) -> Result<KeyResponse, KeymodeError> {
        state.handle_key_with_context(cfg, chord, "", "")
    }

    #[test]
    fn test_unknown_keys() {
        let keys: Keys = ron::from_str("[(\"a\", \"Action\", shell(\"test\"))]").unwrap();
        let cfg = crate::Config::from_parts(keys, crate::Style::default());
        let mut state = State::new();
        press(&mut state, &cfg, &chord("z")).unwrap();
        press(&mut state, &cfg, &chord("x")).unwrap();
        assert_eq!(state.depth(), 0);
    }

    #[test]
    fn test_noexit_behavior() {
        let ron_text = r#"[
            ("m", "Menu", keys([
                ("n", "Normal", shell("echo normal")),
                ("s", "Sticky", shell("echo sticky"), (noexit: true)),
                ("d", "Deep", keys([
                    ("x", "Execute", shell("echo deep")),
                    ("y", "Sticky Deep", shell("echo sticky deep"), (noexit: true)),
                ])),
            ])),
        ]"#;
        let keys: Keys = ron::from_str(ron_text).unwrap();
        let cfg = crate::Config::from_parts(keys, crate::Style::default());
        let mut state = State::new();

        press(&mut state, &cfg, &chord("m")).unwrap();
        assert_eq!(state.depth(), 1);
        press(&mut state, &cfg, &chord("n")).unwrap();
        assert_eq!(state.depth(), 0);
        press(&mut state, &cfg, &chord("m")).unwrap();
        assert_eq!(state.depth(), 1);
        press(&mut state, &cfg, &chord("s")).unwrap();
        assert_eq!(state.depth(), 1);
        press(&mut state, &cfg, &chord("d")).unwrap();
        assert_eq!(state.depth(), 2);
        press(&mut state, &cfg, &chord("x")).unwrap();
        assert_eq!(state.depth(), 0);
        press(&mut state, &cfg, &chord("m")).unwrap();
        press(&mut state, &cfg, &chord("d")).unwrap();
        assert_eq!(state.depth(), 2);
        press(&mut state, &cfg, &chord("y")).unwrap();
        assert_eq!(state.depth(), 2);
    }

    #[test]
    fn test_reload_and_clear_notifications() {
        // Reload non-sticky
        let keys: Keys = ron::from_str("[(\"r\", \"Reload\", reload_config)]").unwrap();
        let cfg = crate::Config::from_parts(keys, crate::Style::default());
        let mut state = State::new();
        match press(&mut state, &cfg, &chord("r")).unwrap() {
            KeyResponse::Ui(MsgToUI::ReloadConfig) => {}
            other => panic!("{:?}", other),
        }
        assert_eq!(state.depth(), 0);

        // Clear sticky inside submenu
        let keys2: Keys = ron::from_str(
            r#"[
                ("m", "Menu", keys([
                    ("c", "Clear", clear_notifications, (noexit: true)),
                    ("p", "Back", pop),
                ])),
            ]"#,
        )
        .unwrap();
        let cfg2 = crate::Config::from_parts(keys2, crate::Style::default());
        let mut state2 = State::new();
        press(&mut state2, &cfg2, &chord("m")).unwrap();
        assert_eq!(state2.depth(), 1);
        match press(&mut state2, &cfg2, &chord("c")).unwrap() {
            KeyResponse::Ui(MsgToUI::ClearNotifications) => {}
            other => panic!("{:?}", other),
        }
        assert_eq!(state2.depth(), 1);
    }

    #[test]
    fn test_demo_config_depth() {
        let ron_text = r#"[
            ("shift+cmd+0", "activate", keys([
                ("t", "Theme tester", keys([
                    ("h", "Theme Prev", theme_prev, (noexit: true)),
                    ("l", "Theme Next", theme_next, (noexit: true)),
                ])),
            ])),
            ("shift+cmd+0", "exit", exit, (global: true, hide: true)),
            ("esc", "Back", pop, (global: true, hide: true, hud_only: true)),
        ]"#;
        let keys: Keys = ron::from_str(ron_text).unwrap();
        let cfg = crate::Config::from_parts(keys, crate::Style::default());
        let mut state = State::new();
        press(&mut state, &cfg, &chord("shift+cmd+0")).unwrap();
        assert_eq!(state.depth(), 1);
    }
}
