use hotki_protocol::MsgToUI;
use mac_keycode::Chord;

use crate::{Action, KeysAttrs, NotificationType};

/// Result of handling a key press
#[derive(Debug)]
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
        dir: config::MoveDir,
    },
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
    pub initial_delay_ms: Option<u64>,
    pub interval_ms: Option<u64>,
}

/// Tracks only the logical cursor within the key hierarchy.
#[derive(Debug, Default)]
pub struct State {
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
    pub fn handle_key(&mut self, cfg: &config::Config, key: &Chord) -> Result<KeyResponse, String> {
        self.handle_key_with_context(cfg, key, "", "")
    }

    /// Execute an action with the given attributes
    fn execute_action(
        &mut self,
        action: &Action,
        attrs: &KeysAttrs,
        entered_index: Option<usize>,
    ) -> Result<KeyResponse, String> {
        match action {
            Action::Place(grid, at) => {
                let (gx, gy) = match grid {
                    config::GridSpec::Grid(config::Grid(x, y)) => (*x, *y),
                };
                let (ix, iy) = match at {
                    config::AtSpec::At(config::At(x, y)) => (*x, *y),
                };
                if ix >= gx || iy >= gy {
                    return Err(format!(
                        "place(): at() out of range: got ({}, {}) for grid ({} x {})\n  Valid x: 0..{}  |  Valid y: 0..{}",
                        ix,
                        iy,
                        gx,
                        gy,
                        gx.saturating_sub(1),
                        gy.saturating_sub(1)
                    ));
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
            Action::PlaceMove(grid, dir) => {
                let (gx, gy) = match grid {
                    config::GridSpec::Grid(config::Grid(x, y)) => (*x, *y),
                };
                let resp = KeyResponse::PlaceMove {
                    cols: gx,
                    rows: gy,
                    dir: *dir,
                };
                if !attrs.noexit() {
                    self.reset();
                }
                Ok(resp)
            }
            Action::Raise(spec) => {
                if spec.app.is_none() && spec.title.is_none() {
                    return Err("raise(): at least one of app or title must be provided".into());
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
            Action::Fullscreen(spec) => {
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
            Action::Keys(new_mode) => {
                let _ = new_mode; // contents live in Config; we just advance cursor
                if let Some(i) = entered_index {
                    self.cursor.push(i as u32);
                }
                Ok(KeyResponse::Ok)
            }
            Action::Relay(spec) => {
                // Parse the provided chord specification; report error via KeyResponse
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
                    None => Err(format!("Invalid relay keyspec '{}'", spec)),
                }
            }
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
            Action::Shell(spec) => {
                // Store shell command info for async execution
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
            Action::ReloadConfig => {
                let response = KeyResponse::Ui(MsgToUI::ReloadConfig);
                if !attrs.noexit() {
                    self.reset();
                }
                Ok(response)
            }
            Action::ClearNotifications => {
                let response = KeyResponse::Ui(MsgToUI::ClearNotifications);
                if !attrs.noexit() {
                    self.reset();
                }
                Ok(response)
            }
            Action::ShowDetails(arg) => {
                let response = KeyResponse::Ui(MsgToUI::ShowDetails(*arg));
                if !attrs.noexit() {
                    self.reset();
                }
                Ok(response)
            }
            Action::ThemeNext => {
                let response = KeyResponse::Ui(MsgToUI::ThemeNext);
                if !attrs.noexit() {
                    self.reset();
                }
                Ok(response)
            }
            Action::ThemePrev => {
                let response = KeyResponse::Ui(MsgToUI::ThemePrev);
                if !attrs.noexit() {
                    self.reset();
                }
                Ok(response)
            }
            Action::ThemeSet(name) => {
                let response = KeyResponse::Ui(MsgToUI::ThemeSet(name.clone()));
                if !attrs.noexit() {
                    self.reset();
                }
                Ok(response)
            }
            Action::ShowHudRoot => {
                self.reset();
                self.cursor.viewing_root = true;
                Ok(KeyResponse::Ok)
            }
            Action::SetVolume(level) => {
                let script = format!("set volume output volume {}", (*level).min(100));
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
            Action::ChangeVolume(delta) => {
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
            Action::Mute(arg) => {
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
    ) -> Result<KeyResponse, String> {
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
