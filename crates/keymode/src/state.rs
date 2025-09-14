use hotki_protocol::MsgToUI;
use mac_keycode::Chord;

use crate::{Action, KeymodeError, KeysAttrs, NotificationType};

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
        ok_notify: NotificationType,
        err_notify: NotificationType,
        /// Optional software repeat configuration (only populated when attrs.noexit() && repeat)
        repeat: Option<ShellRepeatConfig>,
    },
    /// Place window into a grid cell on the current screen
    Place {
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
    },
    /// Move within a grid by one cell in the given direction.
    PlaceMove {
        cols: u32,
        rows: u32,
        dir: config::Dir,
    },
    /// Focus the next window in the given direction.
    Focus { dir: config::Dir },
    /// Fullscreen operation request handled in the engine/backend
    Fullscreen {
        desired: config::Toggle,
        kind: config::FullscreenKind,
    },
    /// Raise a window matching the given spec.
    Raise {
        app: Option<String>,
        title: Option<String>,
    },
    /// Hide or reveal the focused window (tri-state)
    Hide { desired: config::Toggle },
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
    cursor: config::Cursor,
}

impl State {
    /// Create a new state (root path, HUD hidden).
    pub fn new() -> Self {
        Self {
            cursor: config::Cursor::default(),
        }
    }

    /// Process a key press (no context). Equivalent to `handle_key_with_context` with empty app/title.
    pub fn handle_key(
        &mut self,
        cfg: &config::Config,
        key: &Chord,
    ) -> Result<KeyResponse, KeymodeError> {
        self.handle_key_with_context(cfg, key, "", "")
    }

    /// Execute an action with the given attributes
    fn execute_action(
        &mut self,
        action: &Action,
        attrs: &KeysAttrs,
        entered_index: Option<usize>,
    ) -> Result<KeyResponse, KeymodeError> {
        match action {
            Action::Place(grid, at) => self.handle_place(grid, at, attrs),
            Action::PlaceMove(grid, dir) => self.handle_place_move(grid, *dir, attrs),
            Action::Focus(dir) => self.handle_focus(*dir, attrs),
            Action::Raise(spec) => {
                if spec.app.is_none() && spec.title.is_none() {
                    return Err(KeymodeError::RaiseMissingAppOrTitle);
                }
                let resp = KeyResponse::Raise {
                    app: spec.app.clone(),
                    title: spec.title.clone(),
                };
                if !attrs.noexit() {
                    self.reset();
                }
                Ok(resp)
            }
            Action::Fullscreen(spec) => self.handle_fullscreen(spec, attrs),
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
            Action::Hide(desired) => {
                let response = KeyResponse::Hide { desired: *desired };
                if !attrs.noexit() {
                    self.reset();
                }
                Ok(response)
            }
        }
    }

    /// Build a `KeyResponse::Place` after validating indices are in range.
    fn handle_place(
        &mut self,
        grid: &config::GridSpec,
        at: &config::AtSpec,
        attrs: &KeysAttrs,
    ) -> Result<KeyResponse, KeymodeError> {
        let (gx, gy) = match grid {
            config::GridSpec::Grid(config::Grid(x, y)) => (*x, *y),
        };
        let (ix, iy) = match at {
            config::AtSpec::At(config::At(x, y)) => (*x, *y),
        };
        if ix >= gx || iy >= gy {
            return Err(KeymodeError::PlaceAtOutOfRange {
                ix,
                iy,
                gx,
                gy,
                max_x: gx.saturating_sub(1),
                max_y: gy.saturating_sub(1),
            });
        }
        let resp = KeyResponse::Place {
            cols: gx,
            rows: gy,
            col: ix,
            row: iy,
        };
        if !attrs.noexit() {
            self.reset();
        }
        Ok(resp)
    }

    /// Build a `KeyResponse::PlaceMove` for the given grid and direction.
    fn handle_place_move(
        &mut self,
        grid: &config::GridSpec,
        dir: config::Dir,
        attrs: &KeysAttrs,
    ) -> Result<KeyResponse, KeymodeError> {
        let (gx, gy) = match grid {
            config::GridSpec::Grid(config::Grid(x, y)) => (*x, *y),
        };
        let resp = KeyResponse::PlaceMove {
            cols: gx,
            rows: gy,
            dir,
        };
        if !attrs.noexit() {
            self.reset();
        }
        Ok(resp)
    }

    /// Build a `KeyResponse::Focus` for the given direction.
    fn handle_focus(
        &mut self,
        dir: config::Dir,
        attrs: &KeysAttrs,
    ) -> Result<KeyResponse, KeymodeError> {
        let resp = KeyResponse::Focus { dir };
        if !attrs.noexit() {
            self.reset();
        }
        Ok(resp)
    }

    

    /// Build a `KeyResponse::Fullscreen` from a fullscreen specification.
    fn handle_fullscreen(
        &mut self,
        spec: &config::FullscreenSpec,
        attrs: &KeysAttrs,
    ) -> Result<KeyResponse, KeymodeError> {
        let (toggle, kind) = match spec {
            config::FullscreenSpec::One(t) => (t, config::FullscreenKind::Nonnative),
            config::FullscreenSpec::Two(t, k) => (t, *k),
        };
        if !attrs.noexit() {
            self.reset();
        }
        Ok(KeyResponse::Fullscreen {
            desired: *toggle,
            kind,
        })
    }

    /// Parse and relay a chord string, carrying attributes through.
    fn handle_relay(
        &mut self,
        spec: &str,
        attrs: &KeysAttrs,
    ) -> Result<KeyResponse, KeymodeError> {
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
            None => Err(KeymodeError::InvalidRelayKeyspec { spec: spec.to_string() }),
        }
    }

    /// Build a `ShellAsync` response, attaching repeat configuration if effective.
    fn handle_shell(
        &mut self,
        spec: &config::ShellSpec,
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
    fn simple_ui(
        &mut self,
        msg: MsgToUI,
        attrs: &KeysAttrs,
    ) -> Result<KeyResponse, KeymodeError> {
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
            ok_notify: NotificationType::Ignore,
            err_notify: NotificationType::Warn,
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
            ok_notify: NotificationType::Ignore,
            err_notify: NotificationType::Warn,
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
        arg: config::Toggle,
        attrs: &KeysAttrs,
    ) -> Result<KeyResponse, KeymodeError> {
        let script = match arg {
            config::Toggle::On => "set volume output muted true".to_string(),
            config::Toggle::Off => "set volume output muted false".to_string(),
            config::Toggle::Toggle => {
                "set curMuted to output muted of (get volume settings)\nset volume output muted not curMuted".to_string()
            }
        };
        let response = KeyResponse::ShellAsync {
            command: format!("osascript -e '{}'", script.replace('\n', "' -e '")),
            ok_notify: NotificationType::Ignore,
            err_notify: NotificationType::Warn,
            repeat: None,
        };
        if !attrs.noexit() {
            self.reset();
        }
        Ok(response)
    }

    /// Reset to root (clear path and hide viewing_root).
    pub fn reset(&mut self) {
        self.cursor.clear();
        self.cursor.viewing_root = false;
    }

    /// Get the current mode depth (0 = root)
    pub fn depth(&self) -> usize {
        self.cursor.depth()
    }

    /// Ensure context by popping while guards on the entering entries do not match.
    pub fn ensure_context(&mut self, cfg: &config::Config, app: &str, title: &str) -> bool {
        let (next, changed) = config::CursorEnsureExt::ensure_in(&self.cursor, cfg, app, title);
        self.cursor = next;
        changed
    }

    /// Return the current cursor (version is set by the caller before sending to UI).
    pub fn current_cursor(&self) -> config::Cursor {
        self.cursor.clone()
    }

    /// Process a key press with app/title context.
    pub fn handle_key_with_context(
        &mut self,
        cfg: &config::Config,
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

    #[test]
    fn test_unknown_keys() {
        let keys: Keys = ron::from_str("[(\"a\", \"Action\", shell(\"test\"))]").unwrap();
        let cfg = config::Config::from_parts(keys, config::Style::default());
        let mut state = State::new();
        state.handle_key(&cfg, &chord("z")).unwrap();
        state.handle_key(&cfg, &chord("x")).unwrap();
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
        let cfg = config::Config::from_parts(keys, config::Style::default());
        let mut state = State::new();

        state.handle_key(&cfg, &chord("m")).unwrap();
        assert_eq!(state.depth(), 1);
        state.handle_key(&cfg, &chord("n")).unwrap();
        assert_eq!(state.depth(), 0);
        state.handle_key(&cfg, &chord("m")).unwrap();
        assert_eq!(state.depth(), 1);
        state.handle_key(&cfg, &chord("s")).unwrap();
        assert_eq!(state.depth(), 1);
        state.handle_key(&cfg, &chord("d")).unwrap();
        assert_eq!(state.depth(), 2);
        state.handle_key(&cfg, &chord("x")).unwrap();
        assert_eq!(state.depth(), 0);
        state.handle_key(&cfg, &chord("m")).unwrap();
        state.handle_key(&cfg, &chord("d")).unwrap();
        assert_eq!(state.depth(), 2);
        state.handle_key(&cfg, &chord("y")).unwrap();
        assert_eq!(state.depth(), 2);
    }

    #[test]
    fn test_reload_and_clear_notifications() {
        // Reload non-sticky
        let keys: Keys = ron::from_str("[(\"r\", \"Reload\", reload_config)]").unwrap();
        let cfg = config::Config::from_parts(keys, config::Style::default());
        let mut state = State::new();
        match state.handle_key(&cfg, &chord("r")).unwrap() {
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
        let cfg2 = config::Config::from_parts(keys2, config::Style::default());
        let mut state2 = State::new();
        state2.handle_key(&cfg2, &chord("m")).unwrap();
        assert_eq!(state2.depth(), 1);
        match state2.handle_key(&cfg2, &chord("c")).unwrap() {
            KeyResponse::Ui(MsgToUI::ClearNotifications) => {}
            other => panic!("{:?}", other),
        }
        assert_eq!(state2.depth(), 1);
    }
}
