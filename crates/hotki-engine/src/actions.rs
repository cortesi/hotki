use config::script::engine as dyn_engine;
use mac_keycode::Chord;

use crate::{
    DispatchOutcome, Engine, Result,
    refresh::theme_step_name,
    repeater::{ExecSpec, RepeatSpec},
};

impl Engine {
    pub(crate) async fn apply_effects_and_nav(
        &self,
        identifier: &str,
        effects: Vec<dyn_engine::Effect>,
        nav: Option<dyn_engine::NavRequest>,
    ) -> Result<DispatchOutcome> {
        let mut out = DispatchOutcome::default();

        for effect in effects {
            match effect {
                dyn_engine::Effect::Exec(action) => {
                    let outcome = self.apply_action(identifier, &action, None).await?;
                    out.is_nav |= outcome.is_nav;
                    out.entered_mode |= outcome.entered_mode;
                }
                dyn_engine::Effect::Notify { kind, title, body } => {
                    self.notifier.send_notification(kind, title, body)?;
                }
            }
        }

        if let Some(nav) = nav {
            let outcome = self.apply_nav_request(nav).await;
            out.is_nav |= outcome.is_nav;
            out.entered_mode |= outcome.entered_mode;
        }

        Ok(out)
    }

    pub(crate) async fn apply_action(
        &self,
        identifier: &str,
        action: &config::Action,
        repeat: Option<dyn_engine::RepeatSpec>,
    ) -> Result<DispatchOutcome> {
        self.key_tracker.set_repeat_allowed(identifier, false);

        match action {
            config::Action::Shell(spec) => {
                self.start_shell_action(
                    identifier,
                    spec.command().to_string(),
                    spec.ok_notify(),
                    spec.err_notify(),
                    repeat,
                );
                Ok(DispatchOutcome::default())
            }
            config::Action::Relay(spec) => {
                let Some(target) = Chord::parse(spec) else {
                    self.notifier
                        .send_error("Relay", format!("Invalid relay chord string: {}", spec))?;
                    return Ok(DispatchOutcome::default());
                };

                let repeat = repeat_spec(repeat);

                if let Some(repeat) = repeat {
                    let allow_os_repeat =
                        repeat.initial_delay_ms.is_none() && repeat.interval_ms.is_none();
                    self.key_tracker
                        .set_repeat_allowed(identifier, allow_os_repeat);
                    self.repeater.start(
                        identifier.to_string(),
                        ExecSpec::Relay { chord: target },
                        Some(repeat),
                    );
                    return Ok(DispatchOutcome::default());
                }

                let pid = self.current_dispatch_context().pid;
                self.relay
                    .start_relay(identifier.to_string(), target.clone(), pid, false);
                let _ = self.relay.stop_relay(identifier, pid);
                Ok(DispatchOutcome::default())
            }
            config::Action::Pop => Ok(self.apply_nav_request(dyn_engine::NavRequest::Pop).await),
            config::Action::Exit => Ok(self.apply_nav_request(dyn_engine::NavRequest::Exit).await),
            config::Action::ShowRoot => Ok(self
                .apply_nav_request(dyn_engine::NavRequest::ShowRoot)
                .await),
            config::Action::HideHud => Ok(self
                .apply_nav_request(dyn_engine::NavRequest::HideHud)
                .await),
            config::Action::ReloadConfig => {
                if let Err(err) = self.reload_dynamic_config().await {
                    self.notifier.send_error("Config", err.to_string())?;
                }
                Ok(DispatchOutcome::default())
            }
            config::Action::ClearNotifications => {
                self.notifier
                    .send_ui(hotki_protocol::MsgToUI::ClearNotifications)?;
                Ok(DispatchOutcome::default())
            }
            config::Action::ShowDetails(arg) => {
                self.notifier
                    .send_ui(hotki_protocol::MsgToUI::ShowDetails(*arg))?;
                Ok(DispatchOutcome::default())
            }
            config::Action::ThemeNext => self.cycle_theme(identifier, true).await,
            config::Action::ThemePrev => self.cycle_theme(identifier, false).await,
            config::Action::ThemeSet(name) => self.set_theme_by_name(identifier, name).await,
            config::Action::Open(target) => {
                self.start_shell_action(
                    identifier,
                    format!("open -- {}", shell_quote(target)),
                    config::NotifyKind::Ignore,
                    config::NotifyKind::Warn,
                    repeat,
                );
                Ok(DispatchOutcome::default())
            }
            config::Action::SetVolume(level) => {
                self.start_warn_apple_script(identifier, set_volume_script(*level), repeat);
                Ok(DispatchOutcome::default())
            }
            config::Action::ChangeVolume(delta) => {
                self.start_warn_apple_script(
                    identifier,
                    change_volume_script((*delta).into()),
                    repeat,
                );
                Ok(DispatchOutcome::default())
            }
            config::Action::Mute(arg) => {
                self.start_warn_apple_script(identifier, mute_script(*arg), None);
                Ok(DispatchOutcome::default())
            }
        }
    }

    fn start_shell_action(
        &self,
        identifier: &str,
        command: String,
        ok_notify: config::NotifyKind,
        err_notify: config::NotifyKind,
        repeat: Option<dyn_engine::RepeatSpec>,
    ) {
        self.repeater.start(
            identifier.to_string(),
            ExecSpec::Shell {
                command,
                ok_notify,
                err_notify,
            },
            repeat_spec(repeat),
        );
    }

    fn start_warn_apple_script(
        &self,
        identifier: &str,
        script: String,
        repeat: Option<dyn_engine::RepeatSpec>,
    ) {
        self.start_shell_action(
            identifier,
            apple_script_command(script),
            config::NotifyKind::Ignore,
            config::NotifyKind::Warn,
            repeat,
        );
    }

    async fn cycle_theme(&self, identifier: &str, next: bool) -> Result<DispatchOutcome> {
        self.key_tracker.set_repeat_allowed(identifier, false);
        let cfg_guard = self.config.read().await;
        let Some(cfg) = cfg_guard.as_ref() else {
            return Ok(DispatchOutcome::default());
        };
        let theme_names = cfg.theme_names();
        drop(cfg_guard);

        let mut rt = self.runtime.lock().await;
        let step = if next { 1 } else { -1 };
        rt.theme_name = theme_step_name(&theme_names, rt.theme_name.as_str(), step);
        Ok(DispatchOutcome::default())
    }

    async fn set_theme_by_name(&self, identifier: &str, name: &str) -> Result<DispatchOutcome> {
        self.key_tracker.set_repeat_allowed(identifier, false);
        let cfg_guard = self.config.read().await;
        let exists = cfg_guard.as_ref().is_some_and(|cfg| cfg.theme_exists(name));
        drop(cfg_guard);

        if exists {
            let mut rt = self.runtime.lock().await;
            rt.theme_name = name.to_string();
        } else {
            self.notifier.send_notification(
                config::NotifyKind::Warn,
                "Theme".to_string(),
                format!("Unknown theme: {}", name),
            )?;
        }
        Ok(DispatchOutcome::default())
    }

    pub(crate) async fn apply_nav_request(&self, nav: dyn_engine::NavRequest) -> DispatchOutcome {
        let mut rt = self.runtime.lock().await;
        match nav {
            dyn_engine::NavRequest::Push { mode, title } => {
                rt.hud_visible = true;
                let title = title
                    .or_else(|| mode.default_title().map(|t| t.to_string()))
                    .unwrap_or_else(|| "mode".to_string());
                rt.stack.push(dyn_engine::ModeFrame {
                    title,
                    closure: mode,
                    entered_via: None,
                    rendered: Vec::new(),
                    style: None,
                    capture: false,
                });
                DispatchOutcome {
                    is_nav: true,
                    entered_mode: true,
                }
            }
            dyn_engine::NavRequest::Pop => {
                if rt.stack.len() > 1 {
                    rt.stack.pop();
                }
                if rt.stack.len() <= 1 {
                    rt.hud_visible = false;
                }
                DispatchOutcome {
                    is_nav: true,
                    entered_mode: false,
                }
            }
            dyn_engine::NavRequest::Exit => {
                if rt.stack.len() > 1 {
                    rt.stack.truncate(1);
                }
                rt.hud_visible = false;
                DispatchOutcome {
                    is_nav: true,
                    entered_mode: false,
                }
            }
            dyn_engine::NavRequest::ShowRoot => {
                if rt.stack.len() > 1 {
                    rt.stack.truncate(1);
                }
                rt.hud_visible = true;
                DispatchOutcome {
                    is_nav: true,
                    entered_mode: false,
                }
            }
            dyn_engine::NavRequest::HideHud => {
                rt.hud_visible = false;
                DispatchOutcome {
                    is_nav: true,
                    entered_mode: false,
                }
            }
        }
    }

    pub(crate) async fn auto_exit(&self) {
        let mut rt = self.runtime.lock().await;
        if rt.stack.len() > 1 {
            rt.stack.truncate(1);
        }
        rt.hud_visible = false;
    }

    async fn reload_dynamic_config(&self) -> Result<()> {
        let path = { self.config_path.read().await.clone() };
        let Some(path) = path else {
            return Err(crate::Error::Msg(
                "No config path set; cannot reload config".to_string(),
            ));
        };

        let dyn_cfg =
            dyn_engine::load_dynamic_config(&path).map_err(|e| crate::Error::Msg(e.pretty()))?;
        let root = dyn_cfg.root();
        let current_theme = { self.runtime.lock().await.theme_name.clone() };
        let theme_name = if dyn_cfg.theme_exists(current_theme.as_str()) {
            current_theme
        } else {
            dyn_cfg.active_theme().to_string()
        };

        {
            let mut g = self.config.write().await;
            *g = Some(dyn_cfg);
        }
        {
            let mut rt = self.runtime.lock().await;
            rt.theme_name = theme_name;
            rt.reset_to_root(root);
        }

        Ok(())
    }
}

fn repeat_spec(repeat: Option<dyn_engine::RepeatSpec>) -> Option<RepeatSpec> {
    repeat.map(|repeat| RepeatSpec {
        initial_delay_ms: repeat.delay_ms,
        interval_ms: repeat.interval_ms,
    })
}

fn apple_script_command(script: String) -> String {
    format!("osascript -e '{}'", script.replace('\n', "' -e '"))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn set_volume_script(level: u8) -> String {
    format!("set volume output volume {}", level.min(100))
}

fn change_volume_script(delta: i32) -> String {
    format!(
        "set currentVolume to output volume of (get volume settings)\nset volume output volume (currentVolume + {})",
        delta
    )
}

fn mute_script(toggle: config::Toggle) -> String {
    match toggle {
        config::Toggle::On => "set volume output muted true".to_string(),
        config::Toggle::Off => "set volume output muted false".to_string(),
        config::Toggle::Toggle => {
            "set curMuted to output muted of (get volume settings)\nset volume output muted not curMuted".to_string()
        }
    }
}
