use std::sync::{Arc, Mutex};

use mac_keycode::Chord;
use rhai::{
    Dynamic, Engine, EvalAltResult, FnPtr, Map, NativeCallContext, Position, serde::from_dynamic,
};

use super::{
    super::{
        Binding, BindingFlags, BindingKind, HandlerRef, ModeId, ModeRef, RepeatSpec,
        SelectorConfig, StyleOverlay, binding_style::parse_binding_style_map, style_api::RhaiStyle,
    },
    BindingRef, BindingsRef, ModeBuildState, ModeBuilder, boxed_validation_error, lock_unpoisoned,
    mode_id_for, mode_id_for_static, parse_chord,
};
use crate::{Action, raw};

/// Apply a binding style overlay from a Rhai map.
fn binding_style_overlay(
    ctx: &NativeCallContext,
    binding: &mut Binding,
    map: Map,
) -> Result<(), Box<EvalAltResult>> {
    let style = parse_binding_style_map(map, ctx.call_position())?;
    if style.hidden {
        binding.flags.hidden = true;
    }
    binding.style = style.overlay.map(|raw| StyleOverlay {
        func: None,
        raw: Some(raw),
    });
    Ok(())
}

/// Unified binding payload used by the DSL bind/mode constructors.
enum BindingSpec {
    /// A direct action binding.
    Action(Action),
    /// A Rhai handler binding.
    Handler(HandlerRef),
    /// A selector binding.
    Selector(SelectorConfig),
    /// A closure-backed nested mode.
    ClosureMode(FnPtr),
    /// A static nested mode defined by inline bindings.
    StaticMode(Vec<Binding>),
}

/// Build a closure-backed mode reference with a stable id and default title.
fn closure_mode(func: FnPtr, title: String) -> ModeRef {
    ModeRef {
        id: mode_id_for(&func),
        func: Some(func),
        static_bindings: None,
        default_title: Some(title),
    }
}

/// Build a static mode reference from inline bindings.
fn static_mode(bindings: Vec<Binding>, title: String) -> ModeRef {
    ModeRef {
        id: mode_id_for_static(&bindings),
        func: None,
        static_bindings: Some(bindings),
        default_title: Some(title),
    }
}

/// Convert a parsed binding spec into the runtime binding kind plus optional mode id.
fn binding_from_spec(desc: &str, spec: BindingSpec) -> (BindingKind, Option<ModeId>) {
    match spec {
        BindingSpec::Action(action) => (BindingKind::Action(action), None),
        BindingSpec::Handler(handler) => (BindingKind::Handler(handler), None),
        BindingSpec::Selector(selector) => (BindingKind::Selector(selector), None),
        BindingSpec::ClosureMode(func) => {
            let mode = closure_mode(func, desc.to_string());
            (BindingKind::Mode(mode.clone()), Some(mode.id))
        }
        BindingSpec::StaticMode(bindings) => {
            let mode = static_mode(bindings, desc.to_string());
            (BindingKind::Mode(mode.clone()), Some(mode.id))
        }
    }
}

/// Append a new binding to the current mode builder and return its fluent handle.
fn push_binding_spec(
    state: &Arc<Mutex<ModeBuildState>>,
    chord: Chord,
    desc: String,
    spec: BindingSpec,
    pos: Position,
) -> BindingRef {
    let (kind, mode_id) = binding_from_spec(&desc, spec);
    let mut guard = lock_unpoisoned(state);
    let index = guard.bindings.len();
    guard.bindings.push(Binding {
        chord,
        desc,
        kind,
        mode_id,
        flags: BindingFlags::default(),
        style: None,
        mode_capture: false,
        pos,
    });
    BindingRef {
        state: state.clone(),
        index,
    }
}

/// Parse the third element of a DSL binding tuple into a normalized binding spec.
fn parse_binding_spec(
    value: Dynamic,
    pos: Position,
    label: &str,
    allow_selector: bool,
) -> Result<BindingSpec, Box<EvalAltResult>> {
    if let Some(action) = value.clone().try_cast::<Action>() {
        return Ok(BindingSpec::Action(action));
    }
    if let Some(handler) = value.clone().try_cast::<HandlerRef>() {
        return Ok(BindingSpec::Handler(handler));
    }
    if allow_selector && let Some(selector) = value.clone().try_cast::<SelectorConfig>() {
        return Ok(BindingSpec::Selector(selector));
    }
    if let Some(func) = value.try_cast::<FnPtr>() {
        return Ok(BindingSpec::ClosureMode(func));
    }

    let expected = if allow_selector {
        "an Action, action.run, action.selector, or mode closure"
    } else {
        "an Action, action.run, or mode closure"
    };
    Err(boxed_validation_error(
        format!("{label} must have {expected} as third item"),
        pos,
    ))
}

/// Parse a `[chord, desc, action]` tuple used by array-form `bind()` and `mode()`.
fn parse_array_binding(
    item: Dynamic,
    index: usize,
    pos: Position,
    label: &str,
    allow_selector: bool,
) -> Result<(Chord, String, BindingSpec), Box<EvalAltResult>> {
    let arr = item.into_array().map_err(|_| {
        boxed_validation_error(
            format!("{label}: element {index} must be an array [chord, desc, action]"),
            pos,
        )
    })?;
    if arr.len() != 3 {
        return Err(boxed_validation_error(
            format!(
                "{label}: element {index} must have exactly 3 items [chord, desc, action], got {}",
                arr.len()
            ),
            pos,
        ));
    }

    let chord_str = arr[0].clone().into_immutable_string().map_err(|_| {
        boxed_validation_error(
            format!("{label}: element {index} chord must be a string"),
            pos,
        )
    })?;
    let desc = arr[1].clone().into_immutable_string().map_err(|_| {
        boxed_validation_error(
            format!("{label}: element {index} description must be a string"),
            pos,
        )
    })?;
    let chord = Chord::parse(&chord_str).ok_or_else(|| {
        boxed_validation_error(
            format!("{label}: element {index} has invalid chord: {}", chord_str),
            pos,
        )
    })?;
    let desc = desc.to_string();
    let spec = parse_binding_spec(
        arr[2].clone(),
        pos,
        &format!("{label}: element {index}"),
        allow_selector,
    )?;
    Ok((chord, desc, spec))
}

/// Apply the same mutation closure to one or more binding handles.
fn mutate_bindings(
    state: &Arc<Mutex<ModeBuildState>>,
    indices: &[usize],
    pos: Position,
    mut apply: impl FnMut(&mut Binding) -> Result<(), Box<EvalAltResult>>,
) -> Result<(), Box<EvalAltResult>> {
    let mut guard = lock_unpoisoned(state);
    for &index in indices {
        let binding = guard
            .bindings
            .get_mut(index)
            .ok_or_else(|| boxed_validation_error("invalid binding handle".to_string(), pos))?;
        apply(binding)?;
    }
    Ok(())
}

/// Validate and apply repeat settings for repeat-capable binding kinds.
fn set_repeat(
    binding: &mut Binding,
    repeat: RepeatSpec,
    pos: Position,
    method: &str,
) -> Result<(), Box<EvalAltResult>> {
    match &binding.kind {
        BindingKind::Action(Action::Shell(_))
        | BindingKind::Action(Action::Relay(_))
        | BindingKind::Action(Action::SetVolume(_))
        | BindingKind::Action(Action::ChangeVolume(_)) => {}
        BindingKind::Action(_) => {
            return Err(boxed_validation_error(
                format!(
                    "{method} is only valid on shell(...), relay(...), set_volume(...), and change_volume(...) actions"
                ),
                pos,
            ));
        }
        BindingKind::Handler(_) | BindingKind::Selector(_) | BindingKind::Mode(_) => {
            return Err(boxed_validation_error(
                format!("{method} is not valid on handlers, selectors, or mode entries"),
                pos,
            ));
        }
    }
    binding.flags.repeat = Some(repeat);
    binding.flags.stay = true;
    Ok(())
}

/// Register a fluent binding modifier for both `BindingRef` and `BindingsRef`.
macro_rules! register_binding_modifier {
    ($engine:expr, $name:literal, |$ctx:ident, $entry:ident| $body:block) => {
        $engine.register_fn(
            $name,
            |$ctx: NativeCallContext,
             binding: BindingRef|
             -> Result<BindingRef, Box<EvalAltResult>> {
                mutate_bindings(
                    &binding.state,
                    &[binding.index],
                    $ctx.call_position(),
                    |$entry| $body,
                )?;
                Ok(binding)
            },
        );

        $engine.register_fn(
            $name,
            |$ctx: NativeCallContext,
             bindings: BindingsRef|
             -> Result<BindingsRef, Box<EvalAltResult>> {
                mutate_bindings(
                    &bindings.state,
                    &bindings.indices,
                    $ctx.call_position(),
                    |$entry| $body,
                )?;
                Ok(bindings)
            },
        );
    };
}

/// Register the `ModeBuilder` and binding modifier APIs.
pub(super) fn register_mode_builder(engine: &mut Engine) {
    engine.register_type::<BindingRef>();
    engine.register_type::<BindingsRef>();

    engine.register_fn(
        "bind",
        |ctx: NativeCallContext,
         m: &mut ModeBuilder,
         chord: &str,
         desc: &str,
         action: Action|
         -> Result<BindingRef, Box<EvalAltResult>> {
            let chord = parse_chord(&ctx, chord)?;
            Ok(push_binding_spec(
                &m.state,
                chord,
                desc.to_string(),
                BindingSpec::Action(action),
                ctx.call_position(),
            ))
        },
    );

    engine.register_fn(
        "bind",
        |ctx: NativeCallContext,
         m: &mut ModeBuilder,
         chord: &str,
         desc: &str,
         handler: HandlerRef|
         -> Result<BindingRef, Box<EvalAltResult>> {
            let chord = parse_chord(&ctx, chord)?;
            Ok(push_binding_spec(
                &m.state,
                chord,
                desc.to_string(),
                BindingSpec::Handler(handler),
                ctx.call_position(),
            ))
        },
    );

    engine.register_fn(
        "bind",
        |ctx: NativeCallContext,
         m: &mut ModeBuilder,
         chord: &str,
         desc: &str,
         selector: SelectorConfig|
         -> Result<BindingRef, Box<EvalAltResult>> {
            let chord = parse_chord(&ctx, chord)?;
            Ok(push_binding_spec(
                &m.state,
                chord,
                desc.to_string(),
                BindingSpec::Selector(selector),
                ctx.call_position(),
            ))
        },
    );

    engine.register_fn(
        "bind",
        |ctx: NativeCallContext,
         m: &mut ModeBuilder,
         chord: &str,
         desc: &str,
         func: FnPtr|
         -> Result<BindingRef, Box<EvalAltResult>> {
            let chord = parse_chord(&ctx, chord)?;
            Ok(push_binding_spec(
                &m.state,
                chord,
                desc.to_string(),
                BindingSpec::ClosureMode(func),
                ctx.call_position(),
            ))
        },
    );

    engine.register_fn(
        "mode",
        |ctx: NativeCallContext,
         m: &mut ModeBuilder,
         chord: &str,
         title: &str,
         func: FnPtr|
         -> Result<BindingRef, Box<EvalAltResult>> {
            let chord = parse_chord(&ctx, chord)?;
            Ok(push_binding_spec(
                &m.state,
                chord,
                title.to_string(),
                BindingSpec::ClosureMode(func),
                ctx.call_position(),
            ))
        },
    );

    engine.register_fn(
        "mode",
        |ctx: NativeCallContext,
         m: &mut ModeBuilder,
         chord: &str,
         title: &str,
         bindings: rhai::Array|
         -> Result<BindingRef, Box<EvalAltResult>> {
            let chord = parse_chord(&ctx, chord)?;
            let pos = ctx.call_position();
            let mut static_bindings = Vec::with_capacity(bindings.len());
            for (index, item) in bindings.into_iter().enumerate() {
                let (binding_chord, desc, spec) =
                    parse_array_binding(item, index, pos, "mode bindings", false)?;
                let (kind, binding_mode_id) = binding_from_spec(&desc, spec);
                static_bindings.push(Binding {
                    chord: binding_chord,
                    desc,
                    kind,
                    mode_id: binding_mode_id,
                    flags: BindingFlags::default(),
                    style: None,
                    mode_capture: false,
                    pos,
                });
            }

            Ok(push_binding_spec(
                &m.state,
                chord,
                title.to_string(),
                BindingSpec::StaticMode(static_bindings),
                pos,
            ))
        },
    );

    engine.register_fn("capture", |m: &mut ModeBuilder| {
        lock_unpoisoned(&m.state).capture = true;
    });

    engine.register_fn(
        "style",
        |ctx: NativeCallContext, m: &mut ModeBuilder, map: Map| -> Result<(), Box<EvalAltResult>> {
            let dyn_map = Dynamic::from_map(map);
            let style: raw::RawStyle = from_dynamic(&dyn_map).map_err(|error| {
                boxed_validation_error(format!("invalid style map: {}", error), ctx.call_position())
            })?;
            lock_unpoisoned(&m.state).styles.push(style);
            Ok(())
        },
    );

    engine.register_fn("style", |m: &mut ModeBuilder, style: RhaiStyle| {
        lock_unpoisoned(&m.state).styles.push(style.raw);
    });

    register_binding_modifier!(engine, "hidden", |ctx, entry| {
        let _ = ctx;
        entry.flags.hidden = true;
        Ok(())
    });

    register_binding_modifier!(engine, "stay", |ctx, entry| {
        let _ = ctx;
        entry.flags.stay = true;
        Ok(())
    });

    register_binding_modifier!(engine, "global", |ctx, entry| {
        if matches!(entry.kind, BindingKind::Mode(_)) {
            return Err(boxed_validation_error(
                "global() is not allowed on mode entries".to_string(),
                ctx.call_position(),
            ));
        }
        entry.flags.global = true;
        Ok(())
    });

    engine.register_fn(
        "repeat",
        |ctx: NativeCallContext, binding: BindingRef| -> Result<BindingRef, Box<EvalAltResult>> {
            mutate_bindings(
                &binding.state,
                &[binding.index],
                ctx.call_position(),
                |entry| {
                    set_repeat(
                        entry,
                        RepeatSpec {
                            delay_ms: None,
                            interval_ms: None,
                        },
                        ctx.call_position(),
                        "repeat()",
                    )
                },
            )?;
            Ok(binding)
        },
    );

    engine.register_fn(
        "repeat_ms",
        |ctx: NativeCallContext,
         binding: BindingRef,
         delay: i64,
         interval: i64|
         -> Result<BindingRef, Box<EvalAltResult>> {
            let delay_ms: u64 = delay.try_into().map_err(|_| {
                boxed_validation_error(
                    "repeat_ms: delay must be >= 0".to_string(),
                    ctx.call_position(),
                )
            })?;
            let interval_ms: u64 = interval.try_into().map_err(|_| {
                boxed_validation_error(
                    "repeat_ms: interval must be >= 0".to_string(),
                    ctx.call_position(),
                )
            })?;
            mutate_bindings(
                &binding.state,
                &[binding.index],
                ctx.call_position(),
                |entry| {
                    set_repeat(
                        entry,
                        RepeatSpec {
                            delay_ms: Some(delay_ms),
                            interval_ms: Some(interval_ms),
                        },
                        ctx.call_position(),
                        "repeat_ms()",
                    )
                },
            )?;
            Ok(binding)
        },
    );

    engine.register_fn(
        "capture",
        |ctx: NativeCallContext, binding: BindingRef| -> Result<BindingRef, Box<EvalAltResult>> {
            mutate_bindings(
                &binding.state,
                &[binding.index],
                ctx.call_position(),
                |entry| match entry.kind {
                    BindingKind::Mode(_) => {
                        entry.mode_capture = true;
                        Ok(())
                    }
                    _ => Err(boxed_validation_error(
                        "capture() is only valid on mode entries".to_string(),
                        ctx.call_position(),
                    )),
                },
            )?;
            Ok(binding)
        },
    );

    engine.register_fn(
        "style",
        |ctx: NativeCallContext,
         binding: BindingRef,
         map: Map|
         -> Result<BindingRef, Box<EvalAltResult>> {
            mutate_bindings(
                &binding.state,
                &[binding.index],
                ctx.call_position(),
                |entry| binding_style_overlay(&ctx, entry, map.clone()),
            )?;
            Ok(binding)
        },
    );

    engine.register_fn(
        "style",
        |ctx: NativeCallContext,
         binding: BindingRef,
         func: FnPtr|
         -> Result<BindingRef, Box<EvalAltResult>> {
            mutate_bindings(
                &binding.state,
                &[binding.index],
                ctx.call_position(),
                |entry| {
                    if matches!(entry.kind, BindingKind::Mode(_)) {
                        return Err(boxed_validation_error(
                            "style(closure) is not supported on mode entries".to_string(),
                            ctx.call_position(),
                        ));
                    }
                    entry.style = Some(StyleOverlay {
                        func: Some(func.clone()),
                        raw: None,
                    });
                    Ok(())
                },
            )?;
            Ok(binding)
        },
    );

    engine.register_fn(
        "bind",
        |ctx: NativeCallContext,
         m: &mut ModeBuilder,
         bindings: rhai::Array|
         -> Result<BindingsRef, Box<EvalAltResult>> {
            let mut indices = Vec::with_capacity(bindings.len());
            for (index, item) in bindings.into_iter().enumerate() {
                let (chord, desc, spec) =
                    parse_array_binding(item, index, ctx.call_position(), "bind", true)?;
                indices.push(
                    push_binding_spec(&m.state, chord, desc, spec, ctx.call_position()).index,
                );
            }
            Ok(BindingsRef {
                state: m.state.clone(),
                indices,
            })
        },
    );
}
