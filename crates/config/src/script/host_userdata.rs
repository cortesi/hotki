//! Native Luau userdata for menu builders and execution contexts.

use std::{
    mem,
    sync::{Arc, Mutex},
};

use regex::Regex;
use ruau::vm::{
    FromLua, Function, HostType, HostTypeBuilder, IntoLua, MultiValue, RuntimeError, Scope,
    ScopedValue, Userdata,
};

use super::{
    ActionCtx, Binding, BindingFlags, BindingKind, Effect, HandlerRef, ModeCtx, ModeRef,
    NavRequest, RepeatSpec, SourcePos,
    host_args::HostArgs,
    host_parse::{
        BindingOptionsSpec, RepeatOptionsSpec, ShellOptionsSpec, SubmenuOptionsSpec,
        apply_binding_options, parse_chord, parse_optional,
    },
    selector,
    util::lock_unpoisoned,
};
use crate::{Action, ExecSpec, NotifyKind, RelaySpec, ShellModifiers, ShellSpec, Toggle};

/// Luau userdata used to build one rendered mode.
#[derive(Clone, Debug)]
pub struct ModeBuilder {
    /// Shared mutable builder state populated by Luau methods.
    state: Arc<Mutex<ModeBuildState>>,
    /// Binding defaults inherited by this view and its derived views.
    defaults: BindingOptionsSpec,
}

/// Mutable contents collected by a `ModeBuilder`.
#[derive(Debug, Default)]
struct ModeBuildState {
    /// Bindings declared by the current mode render.
    bindings: Vec<Binding>,
    /// Whether the mode requested capture-all behavior.
    capture: bool,
}

/// Luau userdata wrapper for mode render contexts.
#[derive(Clone, Debug)]
struct ModeContextUserData(ModeCtx);

/// Luau userdata wrapper for action handler contexts.
#[derive(Clone, Debug)]
struct ActionContextUserData(ActionCtx);

/// Luau userdata wrapper for an immutable focused-window snapshot.
#[derive(Clone, Debug)]
struct WindowContextUserData(hotki_protocol::FocusSnapshot);

impl<'s> IntoLua<'s> for WindowContextUserData {
    fn into_lua(self, scope: &Scope<'s>) -> Result<ScopedValue<'s>, RuntimeError> {
        Ok(ScopedValue::Userdata(scope.create_userdata(self)?))
    }
}

impl ModeBuilder {
    /// Create a mode builder seeded with inherited capture state.
    pub(crate) fn new_for_render(capture: bool) -> Self {
        let state = ModeBuildState {
            capture,
            ..ModeBuildState::default()
        };
        Self {
            state: Arc::new(Mutex::new(state)),
            defaults: BindingOptionsSpec::default(),
        }
    }

    /// Finish the builder and return bindings plus capture flag.
    pub(crate) fn finish(self) -> (Vec<Binding>, bool) {
        let mut guard = lock_unpoisoned(&self.state);
        let bindings = mem::take(&mut guard.bindings);
        let capture = guard.capture;
        (bindings, capture)
    }
}

/// Wrap a mode builder as Luau userdata.
pub fn mode_builder_userdata<'s>(
    scope: &Scope<'s>,
    builder: ModeBuilder,
) -> Result<Userdata<'s>, RuntimeError> {
    scope.create_userdata(builder)
}

/// Wrap a render context snapshot as Luau userdata.
pub fn mode_context_userdata<'s>(
    scope: &Scope<'s>,
    ctx: ModeCtx,
) -> Result<Userdata<'s>, RuntimeError> {
    scope.create_userdata(ModeContextUserData(ctx))
}

/// Wrap an action context snapshot as Luau userdata.
pub fn action_context_userdata<'s>(
    scope: &Scope<'s>,
    ctx: ActionCtx,
) -> Result<Userdata<'s>, RuntimeError> {
    scope.create_userdata(ActionContextUserData(ctx))
}

/// Build the host userdata type definition for mode builders.
pub(super) fn mode_builder_type() -> HostType {
    HostTypeBuilder::<ModeBuilder>::new("ModeBuilder")
        .method_raw("bind", mode_builder_bind)
        .method_raw("submenu", mode_builder_submenu)
        .method_raw("with", mode_builder_with)
        .method_raw("capture", mode_builder_capture)
        .declaration(
            "declare class ModeBuilder\n\
    with: (self: ModeBuilder, defaults: BindingOptions) -> ModeBuilder\n\
end\n",
        )
        .build()
}

/// Build the host userdata type definition for mode render contexts.
pub(super) fn mode_context_type() -> HostType {
    HostTypeBuilder::<ModeContextUserData>::new("ModeContext")
        .getter("window", |_, this| {
            Ok(this.0.window.clone().map(WindowContextUserData))
        })
        .getter("hud", |_, this| Ok(this.0.hud))
        .getter("depth", |_, this| Ok(this.0.depth))
        .declaration("declare class ModeContext\nend\n")
        .build()
}

/// Build the host userdata type definition for immutable focused-window snapshots.
pub(super) fn window_context_type() -> HostType {
    HostTypeBuilder::<WindowContextUserData>::new("WindowContext")
        .getter("id", |_, this| Ok(f64::from(this.0.id)))
        .getter("pid", |_, this| Ok(f64::from(this.0.pid)))
        .getter("app", |_, this| Ok(this.0.app.clone()))
        .getter("title", |_, this| Ok(this.0.title.clone()))
        .getter("display_id", |_, this| Ok(this.0.display_id.map(f64::from)))
        .method("app_matches", |_, this, pattern: String| {
            regex_matches(&this.0.app, &pattern)
        })
        .method("title_matches", |_, this, pattern: String| {
            regex_matches(&this.0.title, &pattern)
        })
        .declaration("declare class WindowContext\nend\n")
        .build()
}

/// Build the host userdata type definition for action handler contexts.
pub(super) fn action_context_type() -> HostType {
    HostTypeBuilder::<ActionContextUserData>::new("ActionContext")
        .getter("window", |_, this| {
            Ok(this.0.snapshot.window.clone().map(WindowContextUserData))
        })
        .getter("hud", |_, this| Ok(this.0.hud()))
        .getter("depth", |_, this| Ok(this.0.depth()))
        .method_raw("notify", action_context_notify)
        .method("stay", |_, this, (): ()| this.0.set_stay())
        .method_raw("push", action_context_push)
        .method("pop", |_, this, (): ()| this.0.push_nav(NavRequest::Pop))
        .method("exit", |_, this, (): ()| this.0.push_nav(NavRequest::Exit))
        .method("show_root", |_, this, (): ()| {
            this.0.push_nav(NavRequest::ShowRoot)
        })
        .method("hide_hud", |_, this, (): ()| {
            this.0.push_nav(NavRequest::HideHud)
        })
        .method("reload_config", |_, this, (): ()| {
            this.0.push_effect(Effect::Exec(Action::ReloadConfig))
        })
        .method("clear_notifications", |_, this, (): ()| {
            this.0.push_effect(Effect::Exec(Action::ClearNotifications))
        })
        .method_raw("shell", action_context_shell)
        .method_raw("exec", action_context_exec)
        .method_raw("open", action_context_open)
        .method_raw("relay", action_context_relay)
        .method_raw("relay_to_app", action_context_relay_to_app)
        .method_raw("show_main_window", action_context_show_main_window)
        .method_raw("set_volume", action_context_set_volume)
        .method_raw("change_volume", action_context_change_volume)
        .method_raw("mute", action_context_mute)
        .method_raw("until_keyup", action_context_until_keyup)
        .method_raw("select", action_context_select)
        .declaration("declare class ActionContext\nend\n")
        .build()
}

/// Build one handler binding from Luau inputs.
fn binding_from_handler<'s>(
    scope: &Scope<'s>,
    chord: &str,
    desc: String,
    action: Function<'s>,
    defaults: &BindingOptionsSpec,
    options: Option<&BindingOptionsSpec>,
) -> Result<Binding, RuntimeError> {
    let chord = parse_chord(chord)?;
    let pos = current_source_pos(scope);
    let mut binding = Binding {
        chord,
        desc,
        kind: BindingKind::Handler(HandlerRef::from_function(scope, action)?),
        flags: BindingFlags::default(),
        mode_id: None,
        mode_capture: false,
        pos,
    };
    apply_binding_options(&mut binding, Some(defaults.merged_with(options)));
    Ok(binding)
}

/// Build one submenu binding from Luau inputs.
fn binding_from_mode<'s>(
    scope: &Scope<'s>,
    chord: &str,
    title: String,
    render: Function<'s>,
    defaults: &BindingOptionsSpec,
    options: Option<&SubmenuOptionsSpec>,
) -> Result<Binding, RuntimeError> {
    let chord = parse_chord(chord)?;
    let mode = ModeRef::from_function(scope, render, Some(title.clone()))?;
    let pos = current_source_pos(scope);
    let mut binding = Binding {
        chord,
        desc: title,
        kind: BindingKind::Mode(mode.clone()),
        flags: BindingFlags::default(),
        mode_id: Some(mode.id),
        mode_capture: false,
        pos,
    };
    let binding_opts = options.map(|opts| &opts.binding);
    apply_binding_options(&mut binding, Some(defaults.merged_with(binding_opts)));
    binding.mode_capture = options.and_then(|opts| opts.capture).unwrap_or(false);
    Ok(binding)
}

/// Capture the current Luau stack position for binding diagnostics.
fn current_source_pos(scope: &Scope<'_>) -> Option<SourcePos> {
    scope.caller_location(0).map(SourcePos::from_location)
}

/// Evaluate one regex match helper for mode and action contexts.
fn regex_matches(text: &str, pattern: &str) -> Result<bool, RuntimeError> {
    Regex::new(pattern)
        .map(|regex| regex.is_match(text))
        .map_err(|err| RuntimeError::runtime(err.to_string()))
}

/// Implement `menu:bind`.
fn mode_builder_bind<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let chord = args.string(scope, "menu:bind chord")?;
    let desc = args.string(scope, "menu:bind desc")?;
    let action = args.function("menu:bind expected action function")?;
    let opts = args.optional();
    args.finish("menu:bind")?;

    let options = parse_optional::<BindingOptionsSpec>(scope, opts)?;
    let defaults = receiver.borrow::<ModeBuilder>(scope)?.defaults.clone();
    let binding = binding_from_handler(scope, &chord, desc, action, &defaults, options.as_ref())?;
    receiver
        .borrow_mut::<ModeBuilder>(scope)?
        .state
        .lock()
        .map_err(|err| RuntimeError::runtime(err.to_string()))?
        .bindings
        .push(binding);
    Ok(MultiValue::new())
}

/// Implement `menu:submenu`.
fn mode_builder_submenu<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let chord = args.string(scope, "menu:submenu chord")?;
    let title = args.string(scope, "menu:submenu title")?;
    let render = args.function("menu:submenu render")?;
    let opts = args.optional();
    args.finish("menu:submenu")?;
    let options = parse_optional::<SubmenuOptionsSpec>(scope, opts)?;
    let defaults = receiver.borrow::<ModeBuilder>(scope)?.defaults.clone();
    let binding = binding_from_mode(scope, &chord, title, render, &defaults, options.as_ref())?;
    receiver
        .borrow_mut::<ModeBuilder>(scope)?
        .state
        .lock()
        .map_err(|err| RuntimeError::runtime(err.to_string()))?
        .bindings
        .push(binding);
    Ok(MultiValue::new())
}

/// Implement `menu:with` by deriving an immutable view over shared state.
fn mode_builder_with<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let defaults = args.serde::<BindingOptionsSpec>(scope, "menu:with defaults")?;
    args.finish("menu:with")?;

    let builder = receiver.borrow::<ModeBuilder>(scope)?;
    let derived = ModeBuilder {
        state: Arc::clone(&builder.state),
        defaults: builder.defaults.merged_with(Some(&defaults)),
    };
    drop(builder);

    let value = scope.create_userdata(derived)?;
    Ok(MultiValue::from_values(vec![ScopedValue::Userdata(value)]))
}

/// Implement `menu:capture`.
fn mode_builder_capture<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    HostArgs::new(args).finish("menu:capture")?;
    receiver
        .borrow_mut::<ModeBuilder>(scope)?
        .state
        .lock()
        .map_err(|err| RuntimeError::runtime(err.to_string()))?
        .capture = true;
    Ok(MultiValue::new())
}

/// Implement `ctx:notify`.
fn action_context_notify<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let kind = args.serde::<NotifyKind>(scope, "ctx:notify kind")?;
    let title = args.string(scope, "ctx:notify title")?;
    let body = args.string(scope, "ctx:notify body")?;
    args.finish("ctx:notify")?;
    receiver
        .borrow::<ActionContextUserData>(scope)?
        .0
        .push_effect(Effect::Notify { kind, title, body })?;
    Ok(MultiValue::new())
}

/// Implement `ctx:shell`.
fn action_context_shell<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let cmd = args.string(scope, "ctx:shell command")?;
    let opts = parse_optional::<ShellOptionsSpec>(scope, args.optional())?;
    args.finish("ctx:shell")?;
    let action = Action::Shell(shell_spec(cmd, opts));
    receiver
        .borrow::<ActionContextUserData>(scope)?
        .0
        .push_effect(Effect::Exec(action))?;
    Ok(MultiValue::new())
}

/// Implement `ctx:exec` with a strict direct-process specification.
fn action_context_exec<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let spec = args.serde::<ExecSpec>(scope, "ctx:exec spec")?;
    args.finish("ctx:exec")?;
    push_exec(scope, receiver, Action::Exec(spec))
}

/// Implement `ctx:push`.
fn action_context_push<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let render = args.function("ctx:push render")?;
    let title = match args.optional() {
        ScopedValue::Nil => None,
        value => Some(String::from_lua(value, scope)?),
    };
    args.finish("ctx:push")?;
    let mode = ModeRef::from_function(scope, render, title.clone())?;
    receiver
        .borrow::<ActionContextUserData>(scope)?
        .0
        .push_nav(NavRequest::Push { mode, title })?;
    Ok(MultiValue::new())
}

/// Implement `ctx:open`.
fn action_context_open<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let target = args.string(scope, "ctx:open target")?;
    args.finish("ctx:open")?;
    push_exec(scope, receiver, Action::Open(target))
}

/// Implement `ctx:relay`.
fn action_context_relay<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let spec = args.string(scope, "ctx:relay spec")?;
    args.finish("ctx:relay")?;
    push_exec(scope, receiver, Action::Relay(RelaySpec::focused(spec)))
}

/// Implement `ctx:relay_to_app`.
fn action_context_relay_to_app<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let app_name = args.string(scope, "ctx:relay_to_app app_name")?;
    let spec = args.string(scope, "ctx:relay_to_app spec")?;
    args.finish("ctx:relay_to_app")?;
    if app_name.is_empty() {
        return Err(RuntimeError::runtime(
            "ctx:relay_to_app app_name must not be empty",
        ));
    }
    push_exec(
        scope,
        receiver,
        Action::Relay(RelaySpec::application(app_name, spec)),
    )
}

/// Implement `ctx:show_main_window`.
fn action_context_show_main_window<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let toggle = args.serde::<Toggle>(scope, "ctx:show_main_window toggle")?;
    args.finish("ctx:show_main_window")?;
    push_exec(scope, receiver, Action::ShowMainWindow(toggle))
}

/// Implement `ctx:set_volume`.
fn action_context_set_volume<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let level = args.lua::<u8>(scope, "ctx:set_volume level")?;
    args.finish("ctx:set_volume")?;
    push_exec(scope, receiver, Action::SetVolume(level))
}

/// Implement `ctx:change_volume`.
fn action_context_change_volume<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let delta = args.lua::<i8>(scope, "ctx:change_volume delta")?;
    args.finish("ctx:change_volume")?;
    push_exec(scope, receiver, Action::ChangeVolume(delta))
}

/// Implement `ctx:mute`.
fn action_context_mute<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let toggle = args.serde::<Toggle>(scope, "ctx:mute toggle")?;
    args.finish("ctx:mute")?;
    push_exec(scope, receiver, Action::Mute(toggle))
}

/// Implement `ctx:until_keyup`.
fn action_context_until_keyup<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let action = args.function("ctx:until_keyup action")?;
    let repeat = parse_optional::<RepeatOptionsSpec>(scope, args.optional())?;
    args.finish("ctx:until_keyup")?;
    let repeat = repeat.map(|repeat| RepeatSpec {
        delay_ms: repeat.delay_ms,
        interval_ms: repeat.interval_ms,
    });
    receiver
        .borrow::<ActionContextUserData>(scope)?
        .0
        .push_until_keyup(HandlerRef::from_function(scope, action)?, repeat)?;
    Ok(MultiValue::new())
}

/// Implement `ctx:select`.
fn action_context_select<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let spec = args.required_with_message("ctx:select expects a table")?;
    args.finish("ctx:select")?;
    let selector = selector::parse_selector_config(scope, spec)?;
    receiver
        .borrow::<ActionContextUserData>(scope)?
        .0
        .push_effect(Effect::Select(selector))?;
    Ok(MultiValue::new())
}

/// Push an action effect into an action context.
fn push_exec<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    action: Action,
) -> Result<MultiValue<'s>, RuntimeError> {
    receiver
        .borrow::<ActionContextUserData>(scope)?
        .0
        .push_effect(Effect::Exec(action))?;
    Ok(MultiValue::new())
}

/// Build a shell spec from parsed options.
fn shell_spec(cmd: String, opts: Option<ShellOptionsSpec>) -> ShellSpec {
    match opts {
        Some(opts) => {
            let defaults = ShellModifiers::default();
            ShellSpec::WithMods(
                cmd,
                ShellModifiers {
                    ok_notify: opts.ok_notify.unwrap_or(defaults.ok_notify),
                    err_notify: opts.err_notify.unwrap_or(defaults.err_notify),
                },
            )
        }
        None => ShellSpec::Cmd(cmd),
    }
}
