use config::script::engine as dyn_engine;
use hotki_protocol::HudState;
use mac_keycode::Chord;

use crate::{
    Engine, Result,
    runtime::{FocusInfo, RuntimeState},
};

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
    cfg: Option<&dyn_engine::DynamicConfig>,
    focus: FocusInfo,
) -> RefreshPlan {
    rt.focus = focus;

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
    cfg: &dyn_engine::DynamicConfig,
) -> RefreshPlan {
    rt.ensure_root(cfg.root());
    if !cfg.theme_exists(rt.theme_name.as_str()) {
        rt.theme_name = cfg.active_theme().to_string();
    }
    let base_style = cfg.base_style(Some(rt.theme_name.as_str()));

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

    let (warnings, errors) = render_stack_with_recovery(rt, cfg, &base_style);
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
    cfg: &dyn_engine::DynamicConfig,
    base_style: &config::Style,
) -> (Vec<dyn_engine::Effect>, Vec<String>) {
    let mut ctx = rt.focus.mode_ctx(rt.hud_visible, rt.depth());
    let mut errors = Vec::new();

    match dyn_engine::render_stack(cfg, &mut rt.stack, &ctx, base_style) {
        Ok(output) => {
            rt.rendered = output.rendered;
            (output.warnings, errors)
        }
        Err(err) => {
            errors.push(err.pretty());
            rt.stack.truncate(1);
            ctx.depth = 0;

            match dyn_engine::render_stack(cfg, &mut rt.stack, &ctx, base_style) {
                Ok(output) => {
                    rt.rendered = output.rendered;
                    (output.warnings, errors)
                }
                Err(err) => {
                    errors.push(err.pretty());
                    rt.rendered = RuntimeState::empty_rendered(base_style.clone());
                    (Vec::new(), errors)
                }
            }
        }
    }
}

pub(crate) fn hud_state_for_ui_from_state(rt: &RuntimeState) -> hotki_protocol::HudState {
    hotki_protocol::HudState {
        visible: rt.hud_visible,
        rows: rt.rendered.hud_rows.clone(),
        depth: rt.depth(),
        breadcrumbs: rt.stack.iter().skip(1).map(|f| f.title.clone()).collect(),
        style: rt.rendered.style.clone(),
        capture: rt.hud_visible && rt.rendered.capture,
    }
}

impl Engine {
    pub(crate) async fn rebind_and_refresh(&self, focus: FocusInfo) -> Result<()> {
        tracing::debug!("start app={} title={}", focus.app, focus.title);

        let plan = {
            let cfg_guard = self.config.read().await;
            let mut rt = self.runtime.lock().await;
            build_refresh_plan(&mut rt, cfg_guard.as_ref(), focus)
        };

        for message in plan.errors {
            self.notifier.send_error("Config", message)?;
        }

        for effect in plan.warnings {
            if let dyn_engine::Effect::Notify { kind, title, body } = effect {
                self.notifier.send_notification(kind, title, body)?;
            }
        }

        let start = std::time::Instant::now();
        let key_count = plan.key_pairs.len();
        let bindings_changed = {
            let mut manager = self.binding_manager.lock().await;
            manager.set_capture_all(plan.capture_all);
            manager.update_bindings(plan.key_pairs)?
        };
        if bindings_changed {
            tracing::debug!("bindings updated, clearing repeater + relay");
            self.repeater.clear_async().await;
            self.relay.stop_all();
        }

        let displays_snapshot = self.world.displays().await;
        self.publish_hud_with_displays(plan.hud, displays_snapshot)
            .await?;

        let elapsed = start.elapsed();
        if elapsed > std::time::Duration::from_millis(crate::BIND_UPDATE_WARN_MS) {
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
}

pub(crate) fn theme_step_name(theme_names: &[String], current: &str, step: isize) -> String {
    if theme_names.is_empty() {
        return "default".to_string();
    }

    let Some(idx) = theme_names.iter().position(|n| n == current) else {
        return theme_names
            .first()
            .expect("checked for non-empty list")
            .clone();
    };

    let len = theme_names.len();
    let next = match step.cmp(&0) {
        std::cmp::Ordering::Greater => (idx + 1) % len,
        std::cmp::Ordering::Less => idx.checked_sub(1).unwrap_or(len - 1),
        std::cmp::Ordering::Equal => idx,
    };

    theme_names[next].clone()
}
