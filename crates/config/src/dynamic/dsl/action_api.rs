use rhai::{Engine, FnPtr, Map, Module, NativeCallContext};

use super::super::HandlerRef;
use crate::{Action, NotifyKind, Toggle};

#[derive(Debug, Clone, Copy)]
/// Namespace object exported to Rhai scripts as `action`.
struct ActionNamespace;

/// Register the global `action.*` factories and constants.
pub(super) fn register_action_namespace(engine: &mut Engine) {
    engine.register_type_with_name::<ActionNamespace>("ActionNamespace");

    engine.register_fn("shell", |_: ActionNamespace, cmd: &str| {
        Action::Shell(crate::ShellSpec::Cmd(cmd.to_string()))
    });
    engine.register_fn("relay", |_: ActionNamespace, spec: &str| {
        Action::Relay(spec.to_string())
    });
    engine.register_fn("show_details", |_: ActionNamespace, t: Toggle| {
        Action::ShowDetails(t)
    });
    engine.register_fn("theme_set", |_: ActionNamespace, name: &str| {
        Action::ThemeSet(name.to_string())
    });
    engine.register_fn(
        "set_volume",
        |ctx: NativeCallContext,
         _: ActionNamespace,
         level: i64|
         -> Result<Action, Box<rhai::EvalAltResult>> {
            if !(0..=100).contains(&level) {
                return Err(super::boxed_validation_error(
                    format!("set_volume: level must be 0..=100, got {}", level),
                    ctx.call_position(),
                ));
            }
            let level_u8: u8 = level.try_into().map_err(|_| {
                super::boxed_validation_error(
                    "set_volume: level out of range".to_string(),
                    ctx.call_position(),
                )
            })?;
            Ok(Action::SetVolume(level_u8))
        },
    );
    engine.register_fn(
        "change_volume",
        |ctx: NativeCallContext,
         _: ActionNamespace,
         delta: i64|
         -> Result<Action, Box<rhai::EvalAltResult>> {
            if !(-100..=100).contains(&delta) {
                return Err(super::boxed_validation_error(
                    format!("change_volume: delta must be -100..=100, got {}", delta),
                    ctx.call_position(),
                ));
            }
            let delta_i8: i8 = delta.try_into().map_err(|_| {
                super::boxed_validation_error(
                    "change_volume: delta out of range".to_string(),
                    ctx.call_position(),
                )
            })?;
            Ok(Action::ChangeVolume(delta_i8))
        },
    );
    engine.register_fn("mute", |_: ActionNamespace, t: Toggle| Action::Mute(t));

    engine.register_get("pop", |_: &mut ActionNamespace| Action::Pop);
    engine.register_get("exit", |_: &mut ActionNamespace| Action::Exit);
    engine.register_get("show_root", |_: &mut ActionNamespace| Action::ShowRoot);
    engine.register_get("hide_hud", |_: &mut ActionNamespace| Action::HideHud);
    engine.register_get("reload_config", |_: &mut ActionNamespace| {
        Action::ReloadConfig
    });
    engine.register_get("clear_notifications", |_: &mut ActionNamespace| {
        Action::ClearNotifications
    });
    engine.register_get("theme_next", |_: &mut ActionNamespace| Action::ThemeNext);
    engine.register_get("theme_prev", |_: &mut ActionNamespace| Action::ThemePrev);

    engine.register_fn("run", |_: ActionNamespace, func: FnPtr| HandlerRef { func });

    engine.register_fn(
        "selector",
        |ctx: NativeCallContext,
         _: ActionNamespace,
         config: Map|
         -> Result<super::super::SelectorConfig, Box<rhai::EvalAltResult>> {
            super::selector_api::selector_config_from_map(&ctx, config)
        },
    );

    engine.register_fn("selector_item", |label: &str, data: rhai::Dynamic| -> Map {
        let mut map = Map::new();
        map.insert("label".into(), rhai::Dynamic::from(label.to_string()));
        map.insert("sublabel".into(), rhai::Dynamic::UNIT);
        map.insert("data".into(), data);
        map
    });

    let mut module = Module::new();
    module.set_var("action", ActionNamespace);
    engine.register_global_module(module.into());
}

/// Register fluent modifiers on action values (e.g., `shell(...).notify(...)`).
pub(super) fn register_action_fluent(engine: &mut Engine) {
    engine.register_fn("clone", |a: Action| a);
    engine.register_fn(
        "notify",
        |ctx: NativeCallContext, a: Action, ok: NotifyKind, err: NotifyKind| match a {
            Action::Shell(crate::ShellSpec::Cmd(cmd))
            | Action::Shell(crate::ShellSpec::WithMods(cmd, _)) => {
                Ok(Action::Shell(crate::ShellSpec::WithMods(
                    cmd,
                    crate::ShellModifiers {
                        ok_notify: ok,
                        err_notify: err,
                    },
                )))
            }
            _ => Err(super::boxed_validation_error(
                "notify is only valid on shell(...) actions".to_string(),
                ctx.call_position(),
            )),
        },
    );
    engine.register_fn("silent", |ctx: NativeCallContext, a: Action| match a {
        Action::Shell(crate::ShellSpec::Cmd(cmd))
        | Action::Shell(crate::ShellSpec::WithMods(cmd, _)) => {
            Ok(Action::Shell(crate::ShellSpec::WithMods(
                cmd,
                crate::ShellModifiers {
                    ok_notify: NotifyKind::Ignore,
                    err_notify: NotifyKind::Ignore,
                },
            )))
        }
        _ => Err(super::boxed_validation_error(
            "silent is only valid on shell(...) actions".to_string(),
            ctx.call_position(),
        )),
    });
}

/// Register the `HandlerRef` type (used internally by `action.run(...)`).
pub(super) fn register_handler_type(engine: &mut Engine) {
    engine.register_type::<HandlerRef>();
}
