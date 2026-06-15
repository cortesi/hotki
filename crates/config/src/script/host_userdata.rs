//! Native Luau userdata for menu builders and execution contexts.

use std::{
    mem,
    sync::{Arc, Mutex},
};

use regex::Regex;
use ruau::embed::{
    FromLua, Function, HostType, HostTypeBuilder, MultiValue, RuntimeError, Scope, ScopedValue,
    Table, Userdata,
};

use super::{
    ActionCtx, Binding, BindingFlags, BindingKind, ModeCtx, ModeRef, NavRequest, SourcePos,
    StyleOverlay,
    host_action::{ActionPayload, action_payload_from_value, primitive_action_from_value},
    host_args::HostArgs,
    host_parse::{
        BindingOptionsSpec, SubmenuOptionsSpec, apply_binding_options, merge_style_overlays,
        parse_chord, parse_optional, parse_raw_style,
    },
    util::lock_unpoisoned,
};
use crate::{NotifyKind, raw};

/// Luau userdata used to build one rendered mode.
#[derive(Clone, Debug)]
pub struct ModeBuilder {
    /// Shared mutable builder state populated by Luau methods.
    state: Arc<Mutex<ModeBuildState>>,
}

/// Mutable contents collected by a `ModeBuilder`.
#[derive(Debug, Default)]
struct ModeBuildState {
    /// Bindings declared by the current mode render.
    bindings: Vec<Binding>,
    /// Mode-level style overlays applied during render.
    styles: Vec<raw::RawStyle>,
    /// Whether the mode requested capture-all behavior.
    capture: bool,
}

/// Luau userdata wrapper for mode render contexts.
#[derive(Clone, Debug)]
struct ModeContextUserData(ModeCtx);

/// Luau userdata wrapper for action handler contexts.
#[derive(Clone, Debug)]
struct ActionContextUserData(ActionCtx);

impl ModeBuilder {
    /// Create a mode builder seeded with inherited style and capture state.
    pub(crate) fn new_for_render(style: Option<StyleOverlay>, capture: bool) -> Self {
        let mut state = ModeBuildState {
            capture,
            ..ModeBuildState::default()
        };
        if let Some(style) = style {
            state.styles.push(style.raw);
        }
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// Finish the builder and return bindings, merged style, and capture flag.
    pub(crate) fn finish(self) -> (Vec<Binding>, Option<StyleOverlay>, bool) {
        let mut guard = lock_unpoisoned(&self.state);
        let bindings = mem::take(&mut guard.bindings);
        let capture = guard.capture;
        let style = merge_style_overlays(&guard.styles);
        (bindings, style, capture)
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
        .method_raw("bind_many", mode_builder_bind_many)
        .method_raw("submenu", mode_builder_submenu)
        .method_raw("style", mode_builder_style)
        .method_raw("capture", mode_builder_capture)
        .declaration("declare class ModeBuilder\nend\n")
        .build()
}

/// Build the host userdata type definition for mode render contexts.
pub(super) fn mode_context_type() -> HostType {
    HostTypeBuilder::<ModeContextUserData>::new("ModeContext")
        .getter("app", |_, this| Ok(this.0.app.clone()))
        .getter("title", |_, this| Ok(this.0.title.clone()))
        .getter("pid", |_, this| Ok(this.0.pid))
        .getter("hud", |_, this| Ok(this.0.hud))
        .getter("depth", |_, this| Ok(this.0.depth))
        .method("app_matches", |_, this, pattern: String| {
            regex_matches(&this.0.app, &pattern)
        })
        .method("title_matches", |_, this, pattern: String| {
            regex_matches(&this.0.title, &pattern)
        })
        .declaration("declare class ModeContext\nend\n")
        .build()
}

/// Build the host userdata type definition for action handler contexts.
pub(super) fn action_context_type() -> HostType {
    HostTypeBuilder::<ActionContextUserData>::new("ActionContext")
        .getter("app", |_, this| Ok(this.0.app().to_string()))
        .getter("title", |_, this| Ok(this.0.title().to_string()))
        .getter("pid", |_, this| Ok(this.0.pid()))
        .getter("hud", |_, this| Ok(this.0.hud()))
        .getter("depth", |_, this| Ok(this.0.depth()))
        .method("app_matches", |_, this, pattern: String| {
            regex_matches(this.0.app(), &pattern)
        })
        .method("title_matches", |_, this, pattern: String| {
            regex_matches(this.0.title(), &pattern)
        })
        .method_raw("notify", action_context_notify)
        .method("stay", |_, this, (): ()| {
            this.0.set_stay();
            Ok(())
        })
        .method_raw("exec", action_context_exec)
        .method_raw("push", action_context_push)
        .method("pop", |_, this, (): ()| {
            this.0.set_nav(NavRequest::Pop);
            Ok(())
        })
        .method("exit", |_, this, (): ()| {
            this.0.set_nav(NavRequest::Exit);
            Ok(())
        })
        .method("show_root", |_, this, (): ()| {
            this.0.set_nav(NavRequest::ShowRoot);
            Ok(())
        })
        .declaration("declare class ActionContext\nend\n")
        .build()
}

/// Build one primitive-action, handler, or selector binding from Luau inputs.
fn binding_from_action<'s>(
    scope: &Scope<'s>,
    chord: &str,
    desc: String,
    action_value: ActionPayload,
    options: Option<BindingOptionsSpec>,
) -> Result<Binding, RuntimeError> {
    let chord = parse_chord(chord)?;
    let pos = current_source_pos(scope);
    let mut binding = match action_value {
        ActionPayload::Action(action) => Binding {
            chord,
            desc,
            kind: BindingKind::Action(action),
            flags: BindingFlags::default(),
            mode_id: None,
            style: None,
            mode_capture: false,
            pos,
        },
        ActionPayload::Handler(handler) => Binding {
            chord,
            desc,
            kind: BindingKind::Handler(handler),
            flags: BindingFlags::default(),
            mode_id: None,
            style: None,
            mode_capture: false,
            pos,
        },
        ActionPayload::Selector(config) => Binding {
            chord,
            desc,
            kind: BindingKind::Selector(config),
            flags: BindingFlags::default(),
            mode_id: None,
            style: None,
            mode_capture: false,
            pos,
        },
    };
    apply_binding_options(&mut binding, options);
    Ok(binding)
}

/// Build one submenu binding from Luau inputs.
fn binding_from_mode<'s>(
    scope: &Scope<'s>,
    chord: &str,
    title: String,
    render: Function<'s>,
    options: Option<SubmenuOptionsSpec>,
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
        style: None,
        mode_capture: false,
        pos,
    };
    let binding_opts = options.as_ref().map(|opts| opts.binding.clone());
    apply_binding_options(&mut binding, binding_opts);
    if let Some(options) = options {
        binding.mode_capture = options.capture.unwrap_or(false);
    }
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
    let action = args.required_with_message("menu:bind expects an action")?;
    let opts = args.optional();
    args.finish("menu:bind")?;

    let action = action_payload_from_value(scope, action)?;
    let options = parse_optional::<BindingOptionsSpec>(scope, opts)?;
    let binding = binding_from_action(scope, &chord, desc, action, options)?;
    receiver
        .borrow_mut::<ModeBuilder>(scope)?
        .state
        .lock()
        .map_err(|err| RuntimeError::runtime(err.to_string()))?
        .bindings
        .push(binding);
    Ok(MultiValue::new())
}

/// Implement `menu:bind_many`.
fn mode_builder_bind_many<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let table = args.table("menu:bind_many entries")?;
    args.finish("menu:bind_many")?;

    let len = usize::try_from(table.len(scope)?)
        .map_err(|_| RuntimeError::runtime("menu:bind_many entries length does not fit usize"))?;
    let mut bindings = Vec::with_capacity(len);
    for index in 1..=len {
        let entry: Table<'_> = table.get(scope, index as f64)?;
        let action: ScopedValue<'_> = entry.get(scope, "action")?;
        if !matches!(action, ScopedValue::Nil) {
            let chord: String = entry.get(scope, "chord")?;
            let desc: String = entry.get(scope, "desc")?;
            let opts: ScopedValue<'_> = entry.get(scope, "opts")?;
            let action = action_payload_from_value(scope, action)?;
            let options = parse_optional::<BindingOptionsSpec>(scope, opts)?;
            bindings.push(binding_from_action(scope, &chord, desc, action, options)?);
            continue;
        }

        let chord: String = entry.get(scope, "chord")?;
        let title: String = entry.get(scope, "title")?;
        let render: Function<'_> = entry.get(scope, "render")?;
        let opts: ScopedValue<'_> = entry.get(scope, "opts")?;
        let options = parse_optional::<SubmenuOptionsSpec>(scope, opts)?;
        bindings.push(binding_from_mode(scope, &chord, title, render, options)?);
    }

    receiver
        .borrow_mut::<ModeBuilder>(scope)?
        .state
        .lock()
        .map_err(|err| RuntimeError::runtime(err.to_string()))?
        .bindings
        .extend(bindings);
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
    let binding = binding_from_mode(scope, &chord, title, render, options)?;
    receiver
        .borrow_mut::<ModeBuilder>(scope)?
        .state
        .lock()
        .map_err(|err| RuntimeError::runtime(err.to_string()))?
        .bindings
        .push(binding);
    Ok(MultiValue::new())
}

/// Implement `menu:style`.
fn mode_builder_style<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let overlay = args.required_with_message("menu:style expects a style overlay")?;
    args.finish("menu:style")?;
    let raw = parse_raw_style(scope, overlay)?;
    receiver
        .borrow_mut::<ModeBuilder>(scope)?
        .state
        .lock()
        .map_err(|err| RuntimeError::runtime(err.to_string()))?
        .styles
        .push(raw);
    Ok(MultiValue::new())
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
        .push_effect(super::Effect::Notify { kind, title, body });
    Ok(MultiValue::new())
}

/// Implement `ctx:exec`.
fn action_context_exec<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let value = args.required_with_message("ctx:exec expects an action")?;
    args.finish("ctx:exec")?;
    let action = primitive_action_from_value(scope, value)?;
    receiver
        .borrow::<ActionContextUserData>(scope)?
        .0
        .push_effect(super::Effect::Exec(action));
    Ok(MultiValue::new())
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
        .set_nav(NavRequest::Push { mode, title });
    Ok(MultiValue::new())
}
