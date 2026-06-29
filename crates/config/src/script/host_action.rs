//! Native Luau `action` library implementation.

use ruau::{
    decl::DeclSource,
    vm::{
        HostType, HostTypeBuilder, ModuleBuilderExt, MultiValue, RuntimeError, Scope,
        ScopedHostFunction, ScopedValue, Userdata,
    },
    vm_api::{ModuleBinding, ModuleBuilder, ModuleValue, NativeModule},
};

use super::{
    HandlerRef, SelectorConfig,
    host_args::HostArgs,
    host_parse::{ShellOptionsSpec, parse_optional},
    selector,
};
use crate::{Action, ShellModifiers, ShellSpec, Toggle};

/// Tag used to distinguish primitive action constants from other light userdata.
const ACTION_TOKEN_TAG: u8 = 0x48;

/// Primitive `action.*` constants exposed as light userdata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum PrimitiveAction {
    /// Pop the current mode.
    Pop = 1,
    /// Exit the current stack.
    Exit = 2,
    /// Show the root HUD.
    ShowRoot = 3,
    /// Hide the HUD.
    HideHud = 4,
    /// Reload the active config.
    ReloadConfig = 5,
    /// Clear in-app notifications.
    ClearNotifications = 6,
    /// Select the next theme.
    ThemeNext = 7,
    /// Select the previous theme.
    ThemePrev = 8,
}

impl PrimitiveAction {
    /// All primitive action constants installed in the `action` module.
    const ALL: [Self; 8] = [
        Self::Pop,
        Self::Exit,
        Self::ShowRoot,
        Self::HideHud,
        Self::ReloadConfig,
        Self::ClearNotifications,
        Self::ThemeNext,
        Self::ThemePrev,
    ];

    /// Name installed in the Luau `action` module.
    fn name(self) -> &'static str {
        match self {
            Self::Pop => "pop",
            Self::Exit => "exit",
            Self::ShowRoot => "show_root",
            Self::HideHud => "hide_hud",
            Self::ReloadConfig => "reload_config",
            Self::ClearNotifications => "clear_notifications",
            Self::ThemeNext => "theme_next",
            Self::ThemePrev => "theme_prev",
        }
    }

    /// Decode a light-userdata handle into a primitive action.
    fn from_handle(handle: u32) -> Option<Self> {
        Some(match handle {
            1 => Self::Pop,
            2 => Self::Exit,
            3 => Self::ShowRoot,
            4 => Self::HideHud,
            5 => Self::ReloadConfig,
            6 => Self::ClearNotifications,
            7 => Self::ThemeNext,
            8 => Self::ThemePrev,
            _ => return None,
        })
    }

    /// Convert to the engine action represented by this token.
    fn to_action(self) -> Action {
        match self {
            Self::Pop => Action::Pop,
            Self::Exit => Action::Exit,
            Self::ShowRoot => Action::ShowRoot,
            Self::HideHud => Action::HideHud,
            Self::ReloadConfig => Action::ReloadConfig,
            Self::ClearNotifications => Action::ClearNotifications,
            Self::ThemeNext => Action::ThemeNext,
            Self::ThemePrev => Action::ThemePrev,
        }
    }

    /// Build a light-userdata module constant for this primitive action.
    fn token(self) -> ModuleValue {
        ModuleValue::LightUserdata {
            handle: self as u32,
            tag: ACTION_TOKEN_TAG,
        }
    }
}

/// Opaque Luau userdata wrapping an action payload.
#[derive(Clone, Debug)]
struct ActionValue {
    /// Underlying action-like payload.
    payload: ActionPayload,
}

/// Supported payload variants for Luau action userdata.
#[derive(Clone, Debug)]
pub(super) enum ActionPayload {
    /// Primitive engine action.
    Action(Action),
    /// Handler closure.
    Handler(HandlerRef),
    /// Selector popup configuration.
    Selector(SelectorConfig),
}

/// Native module backing the `action` global library.
pub(super) struct ActionModule;

impl NativeModule for ActionModule {
    fn name(&self) -> &str {
        "action"
    }

    fn declaration(&self) -> DeclSource<'_> {
        DeclSource::Text(crate::luau_api())
    }

    fn build(&self, builder: &mut dyn ModuleBuilder) {
        let binding = ModuleBinding::library("action");
        for action in PrimitiveAction::ALL {
            builder.constant(action.name(), binding.clone(), action.token());
        }

        for kind in [
            ActionFunction::Shell,
            ActionFunction::Open,
            ActionFunction::Relay,
            ActionFunction::ShowDetails,
            ActionFunction::ThemeSet,
            ActionFunction::SetVolume,
            ActionFunction::ChangeVolume,
            ActionFunction::Mute,
            ActionFunction::Run,
            ActionFunction::Selector,
        ] {
            builder.scoped_function(kind.name(), binding.clone(), Box::new(kind));
        }
    }
}

/// Host functions exposed on the `action` global library.
#[derive(Clone, Copy)]
enum ActionFunction {
    /// Build a shell command action.
    Shell,
    /// Build an open-target action.
    Open,
    /// Build a relaykey action.
    Relay,
    /// Build a show-details toggle action.
    ShowDetails,
    /// Build a set-theme action.
    ThemeSet,
    /// Build an absolute volume action.
    SetVolume,
    /// Build a relative volume action.
    ChangeVolume,
    /// Build a mute toggle action.
    Mute,
    /// Build an action that calls a Luau handler.
    Run,
    /// Build an action that opens a selector.
    Selector,
}

impl ActionFunction {
    /// Return the function name installed in the `action` library.
    fn name(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Open => "open",
            Self::Relay => "relay",
            Self::ShowDetails => "show_details",
            Self::ThemeSet => "theme_set",
            Self::SetVolume => "set_volume",
            Self::ChangeVolume => "change_volume",
            Self::Mute => "mute",
            Self::Run => "run",
            Self::Selector => "selector",
        }
    }
}

impl ScopedHostFunction for ActionFunction {
    fn call<'s>(
        &self,
        scope: &Scope<'s>,
        args: MultiValue<'s>,
    ) -> Result<MultiValue<'s>, RuntimeError> {
        let mut args = HostArgs::new(args);
        let payload = match self {
            Self::Shell => {
                let cmd = args.string(scope, "action.shell command")?;
                let opts = parse_optional::<ShellOptionsSpec>(scope, args.optional())?;
                args.finish("action.shell")?;
                let defaults = ShellModifiers::default();
                let spec = match opts {
                    Some(opts) => ShellSpec::WithMods(
                        cmd,
                        ShellModifiers {
                            ok_notify: opts.ok_notify.unwrap_or(defaults.ok_notify),
                            err_notify: opts.err_notify.unwrap_or(defaults.err_notify),
                        },
                    ),
                    None => ShellSpec::Cmd(cmd),
                };
                ActionPayload::Action(Action::Shell(spec))
            }
            Self::Open => {
                let target = args.string(scope, "action.open target")?;
                args.finish("action.open")?;
                ActionPayload::Action(Action::Open(target))
            }
            Self::Relay => {
                let spec = args.string(scope, "action.relay spec")?;
                args.finish("action.relay")?;
                ActionPayload::Action(Action::Relay(spec))
            }
            Self::ShowDetails => {
                let toggle = args.serde::<Toggle>(scope, "action.show_details toggle")?;
                args.finish("action.show_details")?;
                ActionPayload::Action(Action::ShowDetails(toggle))
            }
            Self::ThemeSet => {
                let name = args.string(scope, "action.theme_set name")?;
                args.finish("action.theme_set")?;
                ActionPayload::Action(Action::ThemeSet(name))
            }
            Self::SetVolume => {
                let level = args.lua::<u8>(scope, "action.set_volume level")?;
                args.finish("action.set_volume")?;
                ActionPayload::Action(Action::SetVolume(level))
            }
            Self::ChangeVolume => {
                let delta = args.lua::<i8>(scope, "action.change_volume delta")?;
                args.finish("action.change_volume")?;
                ActionPayload::Action(Action::ChangeVolume(delta))
            }
            Self::Mute => {
                let toggle = args.serde::<Toggle>(scope, "action.mute toggle")?;
                args.finish("action.mute")?;
                ActionPayload::Action(Action::Mute(toggle))
            }
            Self::Run => {
                let func = args.function("action.run handler")?;
                args.finish("action.run")?;
                ActionPayload::Handler(HandlerRef::from_function(scope, func)?)
            }
            Self::Selector => {
                let spec = args.required_with_message("action.selector expects a table")?;
                args.finish("action.selector")?;
                ActionPayload::Selector(selector::parse_selector_config(scope, spec)?)
            }
        };
        action_userdata(scope, payload)
    }
}

/// Build the host userdata type definition for action values.
pub(super) fn action_value_type() -> HostType {
    HostTypeBuilder::<ActionValue>::new("ActionValue")
        .declaration("declare class ActionValue\nend\n")
        .build()
}

/// Decode any Luau action value into its Rust payload.
pub(super) fn action_payload_from_value<'s>(
    scope: &Scope<'s>,
    value: ScopedValue<'s>,
) -> Result<ActionPayload, RuntimeError> {
    match value {
        ScopedValue::Userdata(userdata) => {
            Ok(userdata.borrow::<ActionValue>(scope)?.payload.clone())
        }
        ScopedValue::LightUserdata { handle, tag } if tag == ACTION_TOKEN_TAG => {
            PrimitiveAction::from_handle(handle)
                .map(|action| ActionPayload::Action(action.to_action()))
                .ok_or_else(|| RuntimeError::runtime("unknown action token"))
        }
        other => Err(RuntimeError::runtime(format!(
            "expected action userdata, got {}",
            other.type_name()
        ))),
    }
}

/// Decode a primitive action token from light userdata.
pub(super) fn primitive_action_from_value<'s>(
    scope: &Scope<'s>,
    value: ScopedValue<'s>,
) -> Result<Action, RuntimeError> {
    match action_payload_from_value(scope, value)? {
        ActionPayload::Action(action) => Ok(action),
        _ => Err(RuntimeError::runtime("ctx:exec expects a primitive action")),
    }
}

/// Wrap an action payload as Luau userdata.
fn action_userdata<'s>(
    scope: &Scope<'s>,
    payload: ActionPayload,
) -> Result<MultiValue<'s>, RuntimeError> {
    let userdata: Userdata<'_> = scope.create_userdata(ActionValue { payload })?;
    Ok(MultiValue::from_values(vec![ScopedValue::Userdata(
        userdata,
    )]))
}
