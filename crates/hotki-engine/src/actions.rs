use config::script::engine as dyn_engine;
use mac_keycode::Chord;

use crate::{
    DispatchResult, Engine, Result,
    repeater::{ExecSpec, RepeatSpec},
};

impl Engine {
    pub(crate) async fn apply_effects_and_nav(
        &self,
        identifier: &str,
        effects: Vec<dyn_engine::Effect>,
        nav: Option<dyn_engine::NavRequest>,
    ) -> Result<DispatchResult> {
        let mut result = DispatchResult::AutoExit;

        for effect in effects {
            match effect {
                dyn_engine::Effect::Exec(action) => {
                    let outcome = self.apply_action(identifier, &action, None).await?;
                    result = result.combine(outcome);
                }
                dyn_engine::Effect::Notify { kind, title, body } => {
                    self.notifier.send_notification(kind, title, body)?;
                }
            }
        }

        if let Some(nav) = nav {
            let outcome = self.apply_nav_request(nav).await;
            result = result.combine(outcome);
        }

        Ok(result)
    }

    pub(crate) async fn apply_action(
        &self,
        identifier: &str,
        action: &config::Action,
        repeat: Option<dyn_engine::RepeatSpec>,
    ) -> Result<DispatchResult> {
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
                Ok(DispatchResult::AutoExit)
            }
            config::Action::Relay(spec) => {
                let Some(target) = Chord::parse(spec) else {
                    self.notifier
                        .send_error("Relay", format!("Invalid relay chord string: {}", spec))?;
                    return Ok(DispatchResult::AutoExit);
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
                    return Ok(DispatchResult::AutoExit);
                }

                let pid = self.current_focus_info().pid;
                self.relay
                    .start_relay(identifier.to_string(), target, pid, false);
                let _ = self.relay.stop_relay(identifier, pid);
                Ok(DispatchResult::AutoExit)
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
                Ok(DispatchResult::AutoExit)
            }
            config::Action::ClearNotifications => {
                self.notifier
                    .send_ui(hotki_protocol::MsgToUI::ClearNotifications)?;
                Ok(DispatchResult::AutoExit)
            }
            config::Action::ShowDetails(arg) => {
                self.notifier
                    .send_ui(hotki_protocol::MsgToUI::ShowDetails(*arg))?;
                Ok(DispatchResult::AutoExit)
            }
            config::Action::Open(target) => {
                self.start_shell_action(
                    identifier,
                    format!("open -- {}", shell_quote(target)),
                    config::NotifyKind::Ignore,
                    config::NotifyKind::Warn,
                    repeat,
                );
                Ok(DispatchResult::AutoExit)
            }
            config::Action::SetVolume(level) => {
                self.start_warn_apple_script(identifier, set_volume_script(*level), repeat);
                Ok(DispatchResult::AutoExit)
            }
            config::Action::ChangeVolume(delta) => {
                self.start_warn_apple_script(
                    identifier,
                    change_volume_script((*delta).into()),
                    repeat,
                );
                Ok(DispatchResult::AutoExit)
            }
            config::Action::Mute(arg) => {
                self.start_warn_apple_script(identifier, mute_script(*arg), None);
                Ok(DispatchResult::AutoExit)
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

    pub(crate) async fn apply_nav_request(&self, nav: dyn_engine::NavRequest) -> DispatchResult {
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
                    capture: false,
                });
                DispatchResult::EnteredMode
            }
            dyn_engine::NavRequest::Pop => {
                if rt.stack.len() > 1 {
                    rt.stack.pop();
                }
                if rt.stack.len() <= 1 {
                    rt.hud_visible = false;
                }
                DispatchResult::Navigation
            }
            dyn_engine::NavRequest::Exit => {
                if rt.stack.len() > 1 {
                    rt.stack.truncate(1);
                }
                rt.hud_visible = false;
                DispatchResult::Navigation
            }
            dyn_engine::NavRequest::ShowRoot => {
                if rt.stack.len() > 1 {
                    rt.stack.truncate(1);
                }
                rt.hud_visible = true;
                DispatchResult::Navigation
            }
            dyn_engine::NavRequest::HideHud => {
                rt.hud_visible = false;
                DispatchResult::Navigation
            }
        }
    }

    pub(crate) async fn auto_exit(&self) {
        let _ = self.apply_nav_request(dyn_engine::NavRequest::Exit).await;
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

        {
            let mut g = self.config.lock().await;
            *g = Some(dyn_cfg);
        }
        {
            let mut rt = self.runtime.lock().await;
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
