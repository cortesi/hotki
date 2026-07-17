use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use config::runtime as dyn_engine;
use hotki_protocol::{DisplaysSnapshot, HudState, MsgToUI};
use mac_keycode::Chord;

use crate::{
    ConfigInstall, Engine, Error, Result,
    runtime::{RuntimeState, mode_ctx},
};

pub(crate) struct PreparedConfig {
    path: PathBuf,
    config: dyn_engine::ConfigRuntime,
    runtime: RuntimeState,
    plan: RefreshPlan,
    displays: DisplaysSnapshot,
}

#[derive(Debug)]
pub(crate) struct RefreshPlan {
    pub(crate) warnings: Vec<dyn_engine::Effect>,
    pub(crate) errors: Vec<String>,
    pub(crate) key_pairs: Vec<(String, Chord)>,
    pub(crate) capture_all: bool,
    pub(crate) hud: HudState,
}

pub(crate) fn build_refresh_plan(
    rt: &mut RuntimeState,
    cfg: Option<&mut dyn_engine::ConfigRuntime>,
    focus: &Option<hotki_protocol::FocusSnapshot>,
) -> RefreshPlan {
    rt.focus = rt.context_window(focus);

    match cfg {
        Some(cfg) => build_loaded_refresh_plan(rt, cfg),
        None => {
            rt.clear_config_state(config::Style::default());
            RefreshPlan {
                warnings: Vec::new(),
                errors: Vec::new(),
                key_pairs: Vec::new(),
                capture_all: false,
                hud: hud_state_for_ui_from_state(rt),
            }
        }
    }
}

fn build_loaded_refresh_plan(
    rt: &mut RuntimeState,
    cfg: &mut dyn_engine::ConfigRuntime,
) -> RefreshPlan {
    cfg.ensure_stack(&mut rt.stack);

    if rt.selector.is_some() {
        let key_pairs = crate::selector::selector_capture_chords()
            .into_iter()
            .map(|chord| (chord.to_string(), chord))
            .collect();
        return RefreshPlan {
            warnings: Vec::new(),
            errors: Vec::new(),
            key_pairs,
            capture_all: true,
            hud: hud_state_for_ui_from_state(rt),
        };
    }

    let (warnings, errors) = render_stack_with_recovery(rt, cfg);
    let mut key_pairs = rt
        .rendered
        .bindings
        .iter()
        .map(|(chord, _binding)| (chord.to_string(), chord.clone()))
        .collect::<Vec<_>>();
    key_pairs.sort_by(|a, b| a.0.cmp(&b.0));

    RefreshPlan {
        warnings,
        errors,
        key_pairs,
        capture_all: rt.hud_visible && rt.rendered.capture,
        hud: hud_state_for_ui_from_state(rt),
    }
}

fn render_stack_with_recovery(
    rt: &mut RuntimeState,
    cfg: &mut dyn_engine::ConfigRuntime,
) -> (Vec<dyn_engine::Effect>, Vec<String>) {
    RenderRecovery::default().render(rt, cfg)
}

/// Render stack recovery policy used after render failures.
#[derive(Debug, Default)]
struct RenderRecovery {
    /// Human-readable render errors collected while recovering.
    errors: Vec<String>,
}

impl RenderRecovery {
    /// Render the current stack, falling back to root and then to empty state.
    fn render(
        mut self,
        rt: &mut RuntimeState,
        cfg: &mut dyn_engine::ConfigRuntime,
    ) -> (Vec<dyn_engine::Effect>, Vec<String>) {
        let mut ctx = mode_ctx(&rt.focus, rt.hud_visible, rt.depth());
        if let Some(warnings) = self.try_render(rt, cfg, &ctx) {
            return (warnings, self.errors);
        }

        rt.stack.reset_to_root();
        ctx.depth = 0;
        if let Some(warnings) = self.try_render(rt, cfg, &ctx) {
            return (warnings, self.errors);
        }

        rt.rendered = RuntimeState::empty_rendered(cfg.style());
        (Vec::new(), self.errors)
    }

    /// Attempt one render pass, updating runtime state on success.
    fn try_render(
        &mut self,
        rt: &mut RuntimeState,
        cfg: &mut dyn_engine::ConfigRuntime,
        ctx: &dyn_engine::ModeCtx,
    ) -> Option<Vec<dyn_engine::Effect>> {
        match cfg.render(&mut rt.stack, ctx) {
            Ok(output) => {
                rt.rendered = output.state;
                Some(output.warnings)
            }
            Err(err) => {
                self.errors.push(err.pretty());
                None
            }
        }
    }
}

pub(crate) fn hud_state_for_ui_from_state(rt: &RuntimeState) -> hotki_protocol::HudState {
    hotki_protocol::HudState {
        visible: rt.hud_visible,
        rows: rt.rendered.hud_rows.clone(),
        depth: rt.depth(),
        breadcrumbs: rt.stack.breadcrumbs(),
        style: rt.rendered.style.clone(),
        capture: rt.hud_visible && rt.rendered.capture,
    }
}

impl Engine {
    pub(crate) async fn prepare_config(
        &self,
        path: &Path,
        mode: ConfigInstall,
    ) -> Result<PreparedConfig> {
        let mut config =
            dyn_engine::ConfigRuntime::load(path).map_err(|error| Error::Msg(error.pretty()))?;
        let (hud_visible, focus) = match mode {
            ConfigInstall::ResetFocus => (false, self.current_focus_snapshot()),
            ConfigInstall::KeepFocus => {
                let runtime = self.runtime.lock().await;
                (runtime.hud_visible, runtime.focus.clone())
            }
        };
        let mut runtime = RuntimeState::empty();
        runtime.hud_visible = hud_visible;
        runtime.focus = focus.clone();
        if hud_visible {
            runtime.start_session(focus.clone());
        }
        runtime.install_config(&config);
        let plan = build_refresh_plan(&mut runtime, Some(&mut config), &focus);
        if !plan.errors.is_empty() {
            return Err(Error::Msg(plan.errors.join("\n")));
        }
        let displays = self.world.displays();
        Ok(PreparedConfig {
            path: path.to_path_buf(),
            config,
            runtime,
            plan,
            displays,
        })
    }

    pub(crate) async fn commit_config(&self, prepared: PreparedConfig) -> Result<()> {
        let PreparedConfig {
            path,
            config,
            runtime,
            plan,
            displays,
        } = prepared;
        let RefreshPlan {
            warnings,
            errors,
            key_pairs,
            capture_all,
            hud,
        } = plan;

        let mut config_guard = self.config.lock().await;
        let mut runtime_guard = self.runtime.lock().await;
        let mut manager = self.binding_manager.lock().await;
        let mut path_guard = self.config_path.write().await;
        let mut display_guard = self.display_snapshot.lock().await;
        let permit = self.notifier.reserve_ui()?;
        let bindings_changed = manager.update_bindings(key_pairs)?;
        manager.set_capture_all(capture_all);

        *config_guard = Some(config);
        *runtime_guard = runtime;
        *path_guard = Some(path);
        *display_guard = displays.clone();
        permit.send(MsgToUI::HudUpdate {
            hud: Box::new(hud),
            displays,
        });

        drop(display_guard);
        drop(path_guard);
        drop(manager);
        drop(runtime_guard);
        drop(config_guard);

        if bindings_changed {
            tracing::debug!("bindings updated, clearing repeater + relay");
            self.repeater.stop_repeats_async().await;
            self.action_repeater.clear_async().await;
            self.relay.release_all();
        }
        self.deliver_refresh_diagnostics(errors, warnings);
        Ok(())
    }

    pub(crate) async fn rebind_and_refresh(
        &self,
        focus: &Option<hotki_protocol::FocusSnapshot>,
    ) -> Result<()> {
        let _transaction = self.config_transaction.lock().await;
        tracing::debug!(focus = ?focus, "start context update");
        let start = Instant::now();
        let displays = self.world.displays();
        let mut config_guard = self.config.lock().await;
        let mut runtime_guard = self.runtime.lock().await;
        let mut manager = self.binding_manager.lock().await;
        let mut display_guard = self.display_snapshot.lock().await;
        let permit = self.notifier.reserve_ui()?;
        let checkpoint = runtime_guard.checkpoint();
        let rollback = checkpoint.clone();
        let rollback_focus = runtime_guard.focus.clone();
        let selector_active = runtime_guard.selector.is_some();
        let plan = build_refresh_plan(&mut runtime_guard, config_guard.as_mut(), focus);
        let RefreshPlan {
            warnings,
            errors,
            key_pairs,
            capture_all,
            hud,
        } = plan;
        let key_count = key_pairs.len();
        let bindings_changed = match manager.update_bindings(key_pairs) {
            Ok(changed) => changed,
            Err(error) => {
                runtime_guard.restore(checkpoint);
                if !selector_active && let Some(config) = config_guard.as_mut() {
                    let restored =
                        build_refresh_plan(&mut runtime_guard, Some(config), &rollback_focus);
                    if !restored.errors.is_empty() {
                        tracing::error!(
                            errors = ?restored.errors,
                            "refresh_rollback_render_failed"
                        );
                        runtime_guard.restore(rollback);
                    }
                }
                return Err(error);
            }
        };
        manager.set_capture_all(capture_all);
        *display_guard = displays.clone();
        permit.send(MsgToUI::HudUpdate {
            hud: Box::new(hud),
            displays,
        });

        drop(display_guard);
        drop(manager);
        drop(runtime_guard);
        drop(config_guard);

        if bindings_changed {
            tracing::debug!("bindings updated, clearing repeater + relay");
            self.repeater.stop_repeats_async().await;
            self.action_repeater.clear_async().await;
            self.relay.release_all();
        }
        self.deliver_refresh_diagnostics(errors, warnings);

        let elapsed = start.elapsed();
        if elapsed > Duration::from_millis(crate::BIND_UPDATE_WARN_MS) {
            tracing::warn!(
                "Context update bind step took {:?} for {} keys",
                elapsed,
                key_count
            );
        } else {
            tracing::trace!(
                "Context update bind step completed in {:?} for {} keys",
                elapsed,
                key_count
            );
        }

        Ok(())
    }

    fn deliver_refresh_diagnostics(&self, errors: Vec<String>, warnings: Vec<dyn_engine::Effect>) {
        for message in errors {
            if let Err(error) = self.notifier.send_error("Config", message) {
                tracing::warn!(?error, "refresh_error_delivery_failed");
            }
        }
        for effect in warnings {
            if let dyn_engine::Effect::Notify { kind, title, body } = effect
                && let Err(error) = self.notifier.send_notification(kind, title, body)
            {
                tracing::warn!(?error, "refresh_warning_delivery_failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process,
        sync::{
            Arc,
            atomic::{AtomicU32, AtomicUsize, Ordering},
        },
    };

    use hotki_protocol::{FocusSnapshot, MsgToUI};
    use hotki_world::TestWorld;
    use tokio::sync::mpsc;

    use super::*;
    use crate::deps::{CaptureGuard, HotkeyApi, MockHotkeyApi};

    struct FailingUnregisterApi {
        next_id: AtomicU32,
        unregisters: AtomicUsize,
        fail_on: usize,
    }

    impl FailingUnregisterApi {
        fn new(fail_on: usize) -> Self {
            Self {
                next_id: AtomicU32::new(2000),
                unregisters: AtomicUsize::new(0),
                fail_on,
            }
        }
    }

    impl HotkeyApi for FailingUnregisterApi {
        fn intercept(&self, _chord: mac_keycode::Chord) -> u32 {
            self.next_id.fetch_add(1, Ordering::SeqCst) + 1
        }

        fn unregister(&self, _id: u32) -> Result<()> {
            let call = self.unregisters.fetch_add(1, Ordering::SeqCst) + 1;
            if call == self.fail_on {
                Err(Error::Msg("injected unregister failure".to_string()))
            } else {
                Ok(())
            }
        }

        fn capture_all(&self) -> CaptureGuard {
            CaptureGuard::Fake
        }
    }

    struct ActiveSnapshot {
        path: Option<PathBuf>,
        style: config::Style,
        bindings: Vec<(String, mac_keycode::Chord)>,
        focus: Option<FocusSnapshot>,
        hud: HudState,
        displays: DisplaysSnapshot,
    }

    async fn active_snapshot(engine: &Engine) -> ActiveSnapshot {
        let (style, focus, hud) = {
            let runtime = engine.runtime.lock().await;
            (
                runtime.rendered.style.clone(),
                runtime.focus.clone(),
                hud_state_for_ui_from_state(&runtime),
            )
        };
        ActiveSnapshot {
            path: engine.config_path.read().await.clone(),
            style,
            bindings: engine.bindings_snapshot().await,
            focus,
            hud,
            displays: engine.display_snapshot.lock().await.clone(),
        }
    }

    async fn assert_snapshot(engine: &Engine, expected: &ActiveSnapshot) {
        let actual = active_snapshot(engine).await;
        assert_eq!(actual.path, expected.path);
        assert_eq!(actual.style, expected.style);
        assert_eq!(actual.bindings, expected.bindings);
        assert_eq!(actual.focus, expected.focus);
        assert_eq!(actual.hud, expected.hud);
        assert_eq!(actual.displays, expected.displays);
    }

    fn config_source(label: &str) -> String {
        format!(
            r#"
            local a = hotki.actions
            return function(menu)
              menu:bind("a", "active", a.notify("info", "Active", "{label}"))
              menu:bind("r", "reload", a.reload_config)
            end
            "#
        )
    }

    fn replacement_source() -> &'static str {
        r#"
        local a = hotki.actions
        return function(menu)
          menu:bind("b", "replacement", a.notify("info", "Active", "B"))
        end
        "#
    }

    fn contextual_source() -> &'static str {
        r#"
        local a = hotki.actions
        return function(menu, ctx)
          local window = ctx.window
          if window ~= nil and window:app_matches("Candidate") then
            menu:bind("b", "candidate", a.notify("info", "Active", "B"))
          else
            menu:bind("a", "active", a.notify("info", "Active", "A"))
            menu:bind("r", "reload", a.reload_config)
          end
        end
        "#
    }

    fn write_config(name: &str, source: &str, color: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tmp")
            .join(format!("config-transaction-{name}-{}-{id}", process::id()));
        fs::create_dir_all(&directory).expect("create config directory");
        fs::write(
            directory.join("style.luau"),
            format!(r##"return {{ hud = {{ bg = "{color}" }} }}"##),
        )
        .expect("write style");
        let path = directory.join("config.luau");
        fs::write(&path, source).expect("write config");
        path
    }

    fn remove_config(path: &Path) {
        if let Some(directory) = path.parent() {
            let _ = fs::remove_dir_all(directory);
        }
    }

    async fn engine_with_api(
        api: Arc<dyn HotkeyApi>,
        capacity: usize,
    ) -> (Engine, mpsc::Sender<MsgToUI>, mpsc::Receiver<MsgToUI>) {
        let (tx, rx) = mpsc::channel(capacity);
        let engine =
            Engine::new_with_api_and_world(api, tx.clone(), false, Arc::new(TestWorld::new()));
        (engine, tx, rx)
    }

    fn drain(rx: &mut mpsc::Receiver<MsgToUI>) {
        while rx.try_recv().is_ok() {}
    }

    async fn dispatch_notification(engine: &Engine, rx: &mut mpsc::Receiver<MsgToUI>) -> String {
        let id = engine
            .resolve_id_for_ident("a")
            .await
            .expect("active callback binding");
        engine
            .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
            .await
            .expect("dispatch active callback");
        engine
            .dispatch(id, mac_hotkey::EventKind::KeyUp, false)
            .await
            .expect("release active callback");
        let mut text = None;
        while let Ok(message) = rx.try_recv() {
            if let MsgToUI::Notify {
                title, text: body, ..
            } = message
                && title == "Active"
            {
                text = Some(body);
            }
        }
        text.expect("active callback notification")
    }

    #[tokio::test]
    async fn invalid_new_path_preserves_active_config_and_reload_target() {
        let path_a = write_config("invalid-a", &config_source("A"), "#112233");
        let path_b = write_config("invalid-b", "return 42", "#445566");
        let (engine, _tx, mut rx) = engine_with_api(Arc::new(MockHotkeyApi::new()), 32).await;
        engine
            .set_config_path(path_a.clone())
            .await
            .expect("install A");
        drain(&mut rx);
        let active = active_snapshot(&engine).await;

        assert!(engine.set_config_path(path_b.clone()).await.is_err());

        assert_snapshot(&engine, &active).await;
        assert!(rx.try_recv().is_err());
        assert_eq!(dispatch_notification(&engine, &mut rx).await, "A");

        fs::write(&path_a, config_source("A reloaded")).expect("rewrite A");
        let reload = engine
            .resolve_id_for_ident("r")
            .await
            .expect("reload binding");
        engine
            .dispatch(reload, mac_hotkey::EventKind::KeyDown, false)
            .await
            .expect("reload A");
        engine
            .dispatch(reload, mac_hotkey::EventKind::KeyUp, false)
            .await
            .expect("release reload");
        drain(&mut rx);
        assert_eq!(dispatch_notification(&engine, &mut rx).await, "A reloaded");

        remove_config(&path_a);
        remove_config(&path_b);
    }

    #[tokio::test]
    async fn binding_failure_rolls_back_candidate_config() {
        let path_a = write_config("binding-a", &config_source("A"), "#112233");
        let path_b = write_config("binding-b", replacement_source(), "#445566");
        let (engine, _tx, mut rx) =
            engine_with_api(Arc::new(FailingUnregisterApi::new(2)), 32).await;
        engine
            .set_config_path(path_a.clone())
            .await
            .expect("install A");
        drain(&mut rx);
        let active = active_snapshot(&engine).await;
        let replacement_style = dyn_engine::ConfigRuntime::load(&path_b)
            .expect("load replacement style")
            .style();
        assert_ne!(active.style, replacement_style);

        assert!(engine.set_config_path(path_b.clone()).await.is_err());

        assert_snapshot(&engine, &active).await;
        assert!(rx.try_recv().is_err());
        assert_eq!(dispatch_notification(&engine, &mut rx).await, "A");

        remove_config(&path_a);
        remove_config(&path_b);
    }

    #[tokio::test]
    async fn ui_reservation_failure_preserves_active_config() {
        let path_a = write_config("ui-a", &config_source("A"), "#112233");
        let path_b = write_config("ui-b", replacement_source(), "#445566");
        let (engine, tx, mut rx) = engine_with_api(Arc::new(MockHotkeyApi::new()), 2).await;
        engine
            .set_config_path(path_a.clone())
            .await
            .expect("install A");
        drain(&mut rx);
        let active = active_snapshot(&engine).await;
        let replacement_style = dyn_engine::ConfigRuntime::load(&path_b)
            .expect("load replacement style")
            .style();
        assert_ne!(active.style, replacement_style);
        tx.try_send(MsgToUI::ClearNotifications)
            .expect("fill UI channel");
        tx.try_send(MsgToUI::ClearNotifications)
            .expect("fill final UI channel slot");

        assert!(engine.set_config_path(path_b.clone()).await.is_err());

        assert_snapshot(&engine, &active).await;
        assert!(matches!(rx.try_recv(), Ok(MsgToUI::ClearNotifications)));
        assert!(matches!(rx.try_recv(), Ok(MsgToUI::ClearNotifications)));
        assert!(rx.try_recv().is_err());
        assert_eq!(dispatch_notification(&engine, &mut rx).await, "A");

        remove_config(&path_a);
        remove_config(&path_b);
    }

    #[tokio::test]
    async fn rebind_full_ui_lane_preserves_active_generation() {
        let path = write_config("rebind-ui", contextual_source(), "#112233");
        let (engine, tx, mut rx) = engine_with_api(Arc::new(MockHotkeyApi::new()), 2).await;
        engine
            .set_config_path(path.clone())
            .await
            .expect("install A");
        drain(&mut rx);
        let active = active_snapshot(&engine).await;
        tx.try_send(MsgToUI::ClearNotifications)
            .expect("fill UI channel");
        tx.try_send(MsgToUI::ClearNotifications)
            .expect("fill final UI channel slot");
        let candidate = Some(FocusSnapshot {
            id: 2,
            app: "Candidate".to_string(),
            title: "B".to_string(),
            pid: 2,
            display_id: None,
        });

        assert!(engine.rebind_and_refresh(&candidate).await.is_err());

        assert_snapshot(&engine, &active).await;
        assert!(matches!(rx.try_recv(), Ok(MsgToUI::ClearNotifications)));
        assert!(matches!(rx.try_recv(), Ok(MsgToUI::ClearNotifications)));
        assert!(rx.try_recv().is_err());
        assert_eq!(dispatch_notification(&engine, &mut rx).await, "A");

        remove_config(&path);
    }

    #[tokio::test]
    async fn rebind_unregister_failure_restores_runtime_and_callbacks() {
        let path = write_config("rebind-binding", contextual_source(), "#112233");
        let (engine, _tx, mut rx) =
            engine_with_api(Arc::new(FailingUnregisterApi::new(2)), 32).await;
        engine
            .set_config_path(path.clone())
            .await
            .expect("install A");
        drain(&mut rx);
        let active = active_snapshot(&engine).await;
        let candidate = Some(FocusSnapshot {
            id: 2,
            app: "Candidate".to_string(),
            title: "B".to_string(),
            pid: 2,
            display_id: None,
        });

        assert!(engine.rebind_and_refresh(&candidate).await.is_err());

        assert_snapshot(&engine, &active).await;
        assert!(rx.try_recv().is_err());
        assert_eq!(dispatch_notification(&engine, &mut rx).await, "A");

        remove_config(&path);
    }
}
