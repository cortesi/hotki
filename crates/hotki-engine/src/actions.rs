use std::{
    env,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use config::runtime as dyn_engine;
use mac_keycode::Chord;

use crate::{
    DispatchResult, Engine, Result,
    repeater::{ProcessSpec, RepeatSpec},
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
                let outcome = match &action {
                    config::Action::Relay(spec) => self.apply_relay(identifier, spec, run).await?,
                    _ => EffectApplication {
                        result: self
                            .apply_action(identifier, &action, None, ctx.window.clone())
                            .await?,
                        terminal: false,
                    },
                };
                applied.combine_result(outcome.result);
                applied.terminal = terminal || outcome.terminal;
            }
            dyn_engine::Effect::Notify { kind, title, body } => {
                self.notifier.send_notification(kind, title, body)?;
            }
            dyn_engine::Effect::Nav(nav) => {
                applied.combine_result(self.apply_nav_request(nav, ctx.window.clone()).await);
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
            match cfg.execute_handler_with_permission(
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
        opening_window: Option<hotki_protocol::FocusSnapshot>,
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
            config::Action::Exec(spec) => {
                let process = self.direct_process_spec(spec).await;
                self.start_process_action(identifier, process, repeat);
                Ok(DispatchResult::AutoExit)
            }
            config::Action::Relay(spec) => Ok(self
                .apply_relay(identifier, spec, EffectRun::OneShot)
                .await?
                .result),
            config::Action::Pop => Ok(self
                .apply_nav_request(dyn_engine::NavRequest::Pop, opening_window)
                .await),
            config::Action::Exit => Ok(self
                .apply_nav_request(dyn_engine::NavRequest::Exit, opening_window)
                .await),
            config::Action::ShowRoot => Ok(self
                .apply_nav_request(dyn_engine::NavRequest::ShowRoot, opening_window)
                .await),
            config::Action::HideHud => Ok(self
                .apply_nav_request(dyn_engine::NavRequest::HideHud, opening_window)
                .await),
            config::Action::ReloadConfig => {
                if let Err(err) = self.reload_dynamic_config().await {
                    self.notifier.send_error("Config", err.to_string())?;
                }
                Ok(DispatchResult::AutoExit)
            }
            config::Action::ClearNotifications => {
                self.notifier
                    .try_send_ui(hotki_protocol::MsgToUI::ClearNotifications)?;
                Ok(DispatchResult::AutoExit)
            }
            config::Action::ShowDetails(arg) => {
                self.notifier
                    .try_send_ui(hotki_protocol::MsgToUI::ShowDetails(*arg))?;
                Ok(DispatchResult::AutoExit)
            }
            config::Action::Open(target) => {
                self.start_process_action(
                    identifier,
                    ProcessSpec::new(
                        "open",
                        vec!["--".to_string(), target.clone()],
                        None,
                        config::NotifyKind::Ignore,
                        config::NotifyKind::Warn,
                        "Open",
                    ),
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

    /// Apply one relay effect, resolving a process target only at gesture start.
    async fn apply_relay(
        &self,
        identifier: &str,
        spec: &config::RelaySpec,
        run: EffectRun,
    ) -> Result<EffectApplication> {
        let Some(chord) = Chord::parse(&spec.chord) else {
            self.notifier.send_error(
                "Relay",
                format!("Invalid relay chord string: {}", spec.chord),
            )?;
            return Ok(EffectApplication {
                result: DispatchResult::AutoExit,
                terminal: true,
            });
        };

        match run {
            EffectRun::OneShot | EffectRun::RepeatedFirst => {
                let Some(destination) = self.resolve_relay_destination(&spec.target).await? else {
                    return Ok(EffectApplication {
                        result: DispatchResult::AutoExit,
                        terminal: true,
                    });
                };
                self.relay
                    .start_relay(identifier.to_string(), chord, destination, false);
                if matches!(run, EffectRun::OneShot) {
                    let _ = self.relay.stop_relay(identifier);
                }
            }
            EffectRun::RepeatedTick => {
                if self.relay.repeat_relay(identifier) {
                    self.repeater.note_relay_repeat(identifier);
                }
            }
        }
        Ok(EffectApplication::EMPTY)
    }

    /// Resolve a configured relay target without changing application state.
    async fn resolve_relay_destination(
        &self,
        target: &config::RelayTarget,
    ) -> Result<Option<relaykey::RelayDestination>> {
        match target {
            config::RelayTarget::Focused => Ok(Some(relaykey::RelayDestination::Hid)),
            config::RelayTarget::ApplicationName(app_name) => {
                match self.world.resolve_application(app_name).await {
                    hotki_world::ApplicationResolution::Found(pid) => {
                        Ok(Some(relaykey::RelayDestination::Process(pid)))
                    }
                    hotki_world::ApplicationResolution::NotRunning => {
                        self.notifier.send_notification(
                            config::NotifyKind::Warn,
                            "Relay".to_string(),
                            format!("Application \"{app_name}\" is not running"),
                        )?;
                        Ok(None)
                    }
                    hotki_world::ApplicationResolution::Ambiguous(count) => {
                        self.notifier.send_notification(
                            config::NotifyKind::Warn,
                            "Relay".to_string(),
                            format!(
                                "Application \"{app_name}\" is ambiguous: {count} running matches"
                            ),
                        )?;
                        Ok(None)
                    }
                }
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
        self.start_process_action(
            identifier,
            ProcessSpec::shell(command, ok_notify, err_notify),
            repeat,
        );
    }

    fn start_process_action(
        &self,
        identifier: &str,
        process: ProcessSpec,
        repeat: Option<dyn_engine::RepeatSpec>,
    ) {
        self.repeater
            .start(identifier.to_string(), process, repeat_spec(repeat));
    }

    async fn direct_process_spec(&self, spec: &config::ExecSpec) -> ProcessSpec {
        ProcessSpec::new(
            spec.program.clone(),
            spec.args.clone().unwrap_or_default(),
            self.resolve_exec_cwd(spec.cwd.as_deref()).await,
            spec.ok_notify,
            spec.err_notify,
            "Process",
        )
    }

    async fn resolve_exec_cwd(&self, cwd: Option<&str>) -> Option<PathBuf> {
        let cwd = cwd?;
        let path = PathBuf::from(cwd);
        if path.is_absolute() {
            return Some(path);
        }

        let config_path = self.config_path.read().await.clone();
        let base = config_path
            .as_deref()
            .map(config_entry_directory)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        Some(base.join(path))
    }

    fn start_warn_apple_script(
        &self,
        identifier: &str,
        script: String,
        repeat: Option<dyn_engine::RepeatSpec>,
    ) {
        self.start_process_action(identifier, apple_script_process(script), repeat);
    }

    pub(crate) async fn apply_nav_request(
        &self,
        nav: dyn_engine::NavRequest,
        opening_window: Option<hotki_protocol::FocusSnapshot>,
    ) -> DispatchResult {
        let mut rt = self.runtime.lock().await;
        match nav {
            dyn_engine::NavRequest::Push { mode, title } => {
                let title = title
                    .or_else(|| mode.default_title().map(|t| t.to_string()))
                    .unwrap_or_else(|| "mode".to_string());
                rt.push_mode(title, mode, None, false, opening_window);
                DispatchResult::EnteredMode
            }
            dyn_engine::NavRequest::Pop => {
                rt.stack.pop();
                if rt.stack.depth() == 0 {
                    rt.hud_visible = false;
                    rt.end_session();
                }
                DispatchResult::Navigation
            }
            dyn_engine::NavRequest::Exit => {
                rt.stack.reset_to_root();
                rt.hud_visible = false;
                rt.end_session();
                DispatchResult::Navigation
            }
            dyn_engine::NavRequest::ShowRoot => {
                rt.stack.reset_to_root();
                rt.start_session(opening_window);
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
        let _ = self
            .apply_nav_request(dyn_engine::NavRequest::Exit, None)
            .await;
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

fn apple_script_process(script: String) -> ProcessSpec {
    let args = script
        .split('\n')
        .flat_map(|line| ["-e".to_string(), line.to_string()])
        .collect();
    ProcessSpec::new(
        "/usr/bin/osascript",
        args,
        None,
        config::NotifyKind::Ignore,
        config::NotifyKind::Warn,
        "Shell command",
    )
}

fn config_entry_directory(path: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
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

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        path::Path,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
    };

    use hotki_protocol::{MsgToUI, NotifyKind};
    use hotki_world::{TestApplication, TestWorld};
    use tokio::sync::mpsc;

    use super::*;
    use crate::{
        deps::MockHotkeyApi,
        relay::{RelayHandler, RelayPoster},
    };

    #[derive(Default)]
    struct RecordingPoster {
        events: Mutex<Vec<(bool, relaykey::RelayDestination)>>,
        chords: Mutex<Vec<Chord>>,
    }

    impl RecordingPoster {
        fn events(&self) -> Vec<(bool, relaykey::RelayDestination)> {
            self.events.lock().expect("recording poster lock").clone()
        }

        fn chords(&self) -> Vec<Chord> {
            self.chords.lock().expect("recording chord lock").clone()
        }
    }

    impl RelayPoster for RecordingPoster {
        fn key_down(
            &self,
            chord: &Chord,
            is_repeat: bool,
            destination: relaykey::RelayDestination,
        ) -> relaykey::Result<()> {
            self.chords
                .lock()
                .expect("recording chord lock")
                .push(chord.clone());
            self.events
                .lock()
                .expect("recording poster lock")
                .push((is_repeat, destination));
            Ok(())
        }

        fn key_up(
            &self,
            chord: &Chord,
            destination: relaykey::RelayDestination,
        ) -> relaykey::Result<()> {
            self.chords
                .lock()
                .expect("recording chord lock")
                .push(chord.clone());
            self.events
                .lock()
                .expect("recording poster lock")
                .push((false, destination));
            Ok(())
        }
    }

    fn application(name: &str, pid: i32) -> TestApplication {
        TestApplication {
            name: Some(name.to_string()),
            pid,
            terminated: false,
        }
    }

    fn relay_engine(
        world: Arc<TestWorld>,
    ) -> (Engine, Arc<RecordingPoster>, mpsc::Receiver<MsgToUI>) {
        let (tx, rx) = mpsc::channel(16);
        let mut engine =
            Engine::new_with_api_and_world(Arc::new(MockHotkeyApi::new()), tx, false, world);
        let poster = Arc::new(RecordingPoster::default());
        engine.relay = RelayHandler::new_with_poster(Some(poster.clone()));
        (engine, poster, rx)
    }

    #[tokio::test]
    async fn direct_exec_resolves_relative_cwd_without_changing_engine_cwd() {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tmp")
            .join(format!("engine-exec-cwd-{id}"));
        let child = root.join("child");
        fs::create_dir_all(&child).expect("create exec cwd");
        let config_path = root.join("config.luau");
        fs::write(&config_path, "return function(menu, ctx) end").expect("write config path");

        let (tx, mut rx) = mpsc::channel(16);
        let engine = Engine::new_with_api_and_world(
            Arc::new(MockHotkeyApi::new()),
            tx,
            false,
            Arc::new(TestWorld::new()),
        );
        *engine.config_path.write().await = Some(config_path);
        let before = env::current_dir().expect("read process cwd");
        let action = config::Action::Exec(config::ExecSpec {
            program: "/bin/pwd".to_string(),
            args: None,
            cwd: Some("child".to_string()),
            ok_notify: NotifyKind::Info,
            err_notify: NotifyKind::Warn,
        });
        engine
            .apply_action("pwd", &action, None, None)
            .await
            .expect("start direct exec");
        let message = rx.recv().await.expect("pwd notification");
        let expected = fs::canonicalize(&child).expect("canonical child");
        let expected = expected.to_string_lossy().into_owned();
        assert!(matches!(
            message,
            MsgToUI::Notify {
                kind: NotifyKind::Info,
                text,
                ..
            } if text == expected
        ));
        assert_eq!(env::current_dir().expect("read process cwd"), before);
        fs::remove_dir_all(root).expect("remove exec cwd");
    }

    #[tokio::test]
    async fn targeted_relay_pins_process_across_repeat_and_release() {
        let world = Arc::new(TestWorld::new());
        world.set_running_applications(vec![application("YouTube Music", 71)]);
        let (engine, poster, _rx) = relay_engine(world.clone());
        let spec = config::RelaySpec::application("YouTube Music", "space");

        let first = engine
            .apply_relay("music", &spec, EffectRun::RepeatedFirst)
            .await
            .expect("start targeted relay");
        assert!(!first.terminal);
        world.set_running_applications(vec![application("YouTube Music", 72)]);
        engine
            .apply_relay("music", &spec, EffectRun::RepeatedTick)
            .await
            .expect("repeat targeted relay");
        assert!(engine.relay.stop_relay("music"));

        assert_eq!(
            poster.events(),
            vec![
                (false, relaykey::RelayDestination::Process(71)),
                (true, relaykey::RelayDestination::Process(71)),
                (false, relaykey::RelayDestination::Process(71)),
            ]
        );
    }

    #[tokio::test]
    async fn missing_and_ambiguous_targets_warn_once_and_post_nothing() {
        for (applications, expected) in [
            (
                Vec::new(),
                "Application \"YouTube Music\" is not running".to_string(),
            ),
            (
                vec![
                    application("YouTube Music", 71),
                    application("YouTube Music", 72),
                ],
                "Application \"YouTube Music\" is ambiguous: 2 running matches".to_string(),
            ),
        ] {
            let world = Arc::new(TestWorld::new());
            world.set_running_applications(applications);
            let (engine, poster, mut rx) = relay_engine(world.clone());
            let spec = config::RelaySpec::application("YouTube Music", "space");

            let first = engine
                .apply_relay("music", &spec, EffectRun::RepeatedFirst)
                .await
                .expect("resolve targeted relay");
            assert!(first.terminal);
            world.set_running_applications(vec![application("YouTube Music", 73)]);
            engine
                .apply_relay("music", &spec, EffectRun::RepeatedTick)
                .await
                .expect("terminal tick is inert");

            assert!(poster.events().is_empty());
            let message = rx.try_recv().expect("warning notification");
            assert!(matches!(
                message,
                MsgToUI::Notify {
                    kind: NotifyKind::Warn,
                    title,
                    text,
                    ..
                } if title == "Relay" && text == expected
            ));
            assert!(rx.try_recv().is_err(), "warning emitted more than once");
        }
    }

    #[tokio::test]
    async fn focused_and_targeted_relays_share_invalid_chord_diagnostics() {
        for spec in [
            config::RelaySpec::focused("not-a-chord"),
            config::RelaySpec::application("YouTube Music", "not-a-chord"),
        ] {
            let world = Arc::new(TestWorld::new());
            world.set_running_applications(vec![application("YouTube Music", 71)]);
            let (engine, poster, mut rx) = relay_engine(world);

            let applied = engine
                .apply_relay("music", &spec, EffectRun::RepeatedFirst)
                .await
                .expect("invalid relay result");

            assert!(applied.terminal);
            assert!(poster.events().is_empty());
            assert!(matches!(
                rx.try_recv().expect("invalid chord notification"),
                MsgToUI::Notify { text, .. }
                    if text == "Invalid relay chord string: not-a-chord"
            ));
        }
    }

    #[tokio::test]
    async fn targeted_relays_forward_modified_main_keys_to_the_process() {
        let world = Arc::new(TestWorld::new());
        world.set_running_applications(vec![application("YouTube Music", 71)]);
        let (engine, poster, mut rx) = relay_engine(world);
        let spec = config::RelaySpec::application("YouTube Music", "shift+=");

        let applied = engine
            .apply_relay("music", &spec, EffectRun::OneShot)
            .await
            .expect("modified targeted relay result");

        assert!(!applied.terminal);
        assert_eq!(
            poster.events(),
            vec![
                (false, relaykey::RelayDestination::Process(71)),
                (false, relaykey::RelayDestination::Process(71)),
            ]
        );
        let chord = Chord::parse("shift+=").expect("modified chord");
        assert_eq!(poster.chords(), vec![chord.clone(), chord]);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn apple_script_process_preserves_each_script_line_as_an_argument() {
        let process = apple_script_process("first\nsecond\n".to_string());

        assert_eq!(process.program, "/usr/bin/osascript");
        assert_eq!(process.args, vec!["-e", "first", "-e", "second", "-e", "",]);
    }
}
