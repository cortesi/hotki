use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use config::script::engine as dyn_engine;
use mac_keycode::Chord;

use crate::{
    DispatchResult, Engine, Result,
    repeater::{ExecSpec, RepeatSpec},
    selector_controller::SelectorController,
};

/// How an effect queue is being applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectRun {
    /// Ordinary one-shot action activation.
    OneShot,
    /// First invocation of a held-key repeated action.
    RepeatedFirst,
    /// Timer-driven invocation of a held-key repeated action.
    RepeatedTick,
}

/// Result of applying an effect queue.
#[derive(Debug, Clone, Copy)]
struct EffectApplication {
    /// Post-dispatch behavior requested by the queue.
    result: DispatchResult,
    /// Whether no later effects or repeat ticks should run.
    terminal: bool,
}

impl EffectApplication {
    /// Empty effect-application result.
    const EMPTY: Self = Self {
        result: DispatchResult::AutoExit,
        terminal: false,
    };

    /// Merge in another dispatch result.
    fn combine_result(&mut self, result: DispatchResult) {
        self.result = self.result.combine(result);
    }
}

impl Engine {
    pub(crate) async fn apply_effects(
        &self,
        identifier: &str,
        effects: Vec<dyn_engine::Effect>,
        ctx: dyn_engine::ModeCtx,
    ) -> Result<DispatchResult> {
        Ok(self
            .apply_effects_one_shot(identifier, effects, ctx)
            .await?
            .result)
    }

    /// Apply ordinary action effects in source order.
    async fn apply_effects_one_shot(
        &self,
        identifier: &str,
        effects: Vec<dyn_engine::Effect>,
        ctx: dyn_engine::ModeCtx,
    ) -> Result<EffectApplication> {
        let mut applied = EffectApplication::EMPTY;
        for effect in effects {
            let effect_result = match effect {
                dyn_engine::Effect::UntilKeyUp { action, repeat } => {
                    self.start_until_keyup(identifier, action, repeat, ctx.clone())
                        .await?
                }
                effect => {
                    self.apply_effect(identifier, effect, ctx.clone(), EffectRun::OneShot)
                        .await?
                }
            };
            applied.combine_result(effect_result.result);
            if effect_result.terminal {
                applied.terminal = true;
                break;
            }
        }
        Ok(applied)
    }

    /// Apply repeated-action effects in source order.
    async fn apply_effects_repeated(
        &self,
        identifier: &str,
        effects: Vec<dyn_engine::Effect>,
        ctx: dyn_engine::ModeCtx,
        run: EffectRun,
    ) -> Result<EffectApplication> {
        let mut applied = EffectApplication::EMPTY;
        for effect in effects {
            if matches!(effect, dyn_engine::Effect::UntilKeyUp { .. }) {
                self.notifier.send_error(
                    "Handler",
                    "ctx:until_keyup cannot be nested inside a repeated action".to_string(),
                )?;
                applied.terminal = true;
                break;
            }
            let effect_result = self
                .apply_effect(identifier, effect, ctx.clone(), run)
                .await?;
            applied.combine_result(effect_result.result);
            if effect_result.terminal {
                applied.terminal = true;
                break;
            }
        }
        Ok(applied)
    }

    /// Apply one non-repeat effect.
    async fn apply_effect(
        &self,
        identifier: &str,
        effect: dyn_engine::Effect,
        ctx: dyn_engine::ModeCtx,
        run: EffectRun,
    ) -> Result<EffectApplication> {
        let mut applied = EffectApplication::EMPTY;
        match effect {
            dyn_engine::Effect::Exec(action) => {
                let terminal = matches!(action, config::Action::ReloadConfig);
                let outcome = match run {
                    EffectRun::RepeatedFirst | EffectRun::RepeatedTick
                        if matches!(action, config::Action::Relay(_)) =>
                    {
                        self.apply_repeated_relay(identifier, &action, run)?
                    }
                    _ => self.apply_action(identifier, &action, None).await?,
                };
                applied.combine_result(outcome);
                applied.terminal = terminal;
            }
            dyn_engine::Effect::Notify { kind, title, body } => {
                self.notifier.send_notification(kind, title, body)?;
            }
            dyn_engine::Effect::Nav(nav) => {
                applied.combine_result(self.apply_nav_request(nav).await);
            }
            dyn_engine::Effect::Select(config) => {
                if SelectorController::new(self).open(config, ctx).await? {
                    applied.combine_result(DispatchResult::SelectorOpened);
                }
            }
            dyn_engine::Effect::UntilKeyUp { .. } => unreachable!("handled by caller"),
        }
        Ok(applied)
    }

    /// Start a held-key repeat loop for a Luau action closure.
    async fn start_until_keyup(
        &self,
        identifier: &str,
        action: dyn_engine::HandlerRef,
        repeat: Option<dyn_engine::RepeatSpec>,
        ctx: dyn_engine::ModeCtx,
    ) -> Result<EffectApplication> {
        let first = self
            .run_repeated_action(
                identifier,
                action.clone(),
                ctx.clone(),
                EffectRun::RepeatedFirst,
            )
            .await?;
        if first.terminal || !self.key_tracker.is_down(identifier) {
            return Ok(first);
        }

        let (initial, interval) = self.repeater.effective_timings(repeat_spec(repeat));
        let id = identifier.to_string();
        let engine = self.clone_for_background();
        let running = Arc::new(AtomicBool::new(false));
        self.action_repeater.start(id.clone(), initial, interval, {
            let running = running.clone();
            move || {
                if running.swap(true, Ordering::SeqCst) {
                    tracing::trace!("action_repeat_tick_skip_running" = %id);
                    return;
                }
                let engine = engine.clone();
                let action = action.clone();
                let ctx = ctx.clone();
                let running = running.clone();
                let id = id.clone();
                tokio::spawn(async move {
                    let stop = match engine
                        .run_repeated_action(&id, action, ctx, EffectRun::RepeatedTick)
                        .await
                    {
                        Ok(applied) => applied.terminal,
                        Err(err) => {
                            tracing::warn!("repeated action failed for {}: {}", id, err);
                            true
                        }
                    };
                    running.store(false, Ordering::SeqCst);
                    if stop {
                        engine.action_repeater.stop(&id).await;
                    }
                });
            }
        });
        Ok(first)
    }

    /// Run one repeated action closure invocation and apply its effects.
    async fn run_repeated_action(
        &self,
        identifier: &str,
        action: dyn_engine::HandlerRef,
        ctx: dyn_engine::ModeCtx,
        run: EffectRun,
    ) -> Result<EffectApplication> {
        let result = {
            let mut cfg_guard = self.config.lock().await;
            let Some(cfg) = cfg_guard.as_mut() else {
                tracing::trace!("No dynamic config loaded; stopping repeated action");
                return Ok(EffectApplication {
                    result: DispatchResult::AutoExit,
                    terminal: true,
                });
            };
            match dyn_engine::execute_handler_with_permission(
                cfg,
                &action,
                &ctx,
                dyn_engine::ActionRepeatPermission::RepeatedAction,
            ) {
                Ok(result) => result,
                Err(err) => {
                    self.notifier.send_error("Handler", err.pretty())?;
                    return Ok(EffectApplication {
                        result: DispatchResult::AutoExit,
                        terminal: true,
                    });
                }
            }
        };

        let mut applied = self
            .apply_effects_repeated(identifier, result.effects, ctx, run)
            .await?;
        applied.result = applied.result.with_stay(result.stay);
        Ok(applied)
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

    /// Apply relay effects emitted by a repeated action closure.
    fn apply_repeated_relay(
        &self,
        identifier: &str,
        action: &config::Action,
        run: EffectRun,
    ) -> Result<DispatchResult> {
        let config::Action::Relay(spec) = action else {
            return Ok(DispatchResult::AutoExit);
        };
        let Some(target) = Chord::parse(spec) else {
            self.notifier
                .send_error("Relay", format!("Invalid relay chord string: {}", spec))?;
            return Ok(DispatchResult::AutoExit);
        };

        match run {
            EffectRun::RepeatedFirst => {
                let pid = self.current_focus_info().pid;
                self.relay
                    .start_relay(identifier.to_string(), target, pid, false);
            }
            EffectRun::RepeatedTick => {
                if self.relay.repeat_relay(identifier) {
                    self.repeater.note_relay_repeat(identifier);
                }
            }
            EffectRun::OneShot => {}
        }
        Ok(DispatchResult::AutoExit)
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
                let title = title
                    .or_else(|| mode.default_title().map(|t| t.to_string()))
                    .unwrap_or_else(|| "mode".to_string());
                rt.push_mode(title, mode, None, false);
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
        self.install_config(&path, crate::ConfigInstall::KeepFocus)
            .await
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
