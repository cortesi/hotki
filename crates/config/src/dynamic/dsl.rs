use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fmt,
    hash::{Hash, Hasher},
    mem,
    sync::{Arc, Mutex},
};

use mac_keycode::Chord;
use rhai::{
    Dynamic, Engine, EvalAltResult, FnPtr, Map, Module, NativeCallContext, serde::from_dynamic,
};

use super::{
    ActionCtx, Binding, BindingFlags, BindingKind, HandlerRef, ModeCtx, ModeId, ModeRef,
    NavRequest, RepeatSpec, StyleOverlay, style_api::RhaiStyle, util::lock_unpoisoned,
    validation::boxed_validation_error,
};
use crate::{Action, FontWeight, Mode, NotifyKind, NotifyPos, Pos, Toggle, raw, themes};

#[derive(Debug)]
/// Script-global state captured while evaluating a dynamic config.
pub struct DynamicConfigScriptState {
    /// Theme registry populated with builtins and script registrations.
    pub(crate) themes: HashMap<String, raw::RawStyle>,
    /// Active theme name selected via `theme("...")`.
    pub(crate) active_theme: String,
    /// Root mode closure declared via `hotki.mode(...)`.
    pub(crate) root: Option<ModeRef>,
}

impl Default for DynamicConfigScriptState {
    fn default() -> Self {
        let themes = themes::list_themes()
            .into_iter()
            .map(|name| {
                let theme = themes::load_theme(Some(name));
                (name.to_string(), theme.to_raw())
            })
            .collect();

        Self {
            themes,
            active_theme: "default".to_string(),
            root: None,
        }
    }
}

#[derive(Debug, Default)]
/// Mutable build state for a single mode render.
struct ModeBuildState {
    /// Rendered bindings declared by the mode closure.
    bindings: Vec<Binding>,
    /// Mode-level style overlays, applied left-to-right.
    styles: Vec<raw::RawStyle>,
    /// Whether this mode requests capture-all while HUD-visible.
    capture: bool,
}

#[derive(Clone)]
/// Builder passed into mode closures for declaring bindings and modifiers.
pub struct ModeBuilder {
    /// Shared state so Rhai can mutate it by reference.
    state: Arc<Mutex<ModeBuildState>>,
}

impl ModeBuilder {
    /// Create a fresh builder for a new mode render.
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ModeBuildState::default())),
        }
    }

    /// Create a builder seeded with inherited mode style/capture state.
    pub(crate) fn new_for_render(style: Option<StyleOverlay>, capture: bool) -> Self {
        let mut inherited = Vec::new();
        if let Some(style) = style
            && let Some(raw) = style.raw
        {
            inherited.push(raw);
        }

        let state = ModeBuildState {
            styles: inherited,
            capture,
            ..ModeBuildState::default()
        };
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// Consume the builder and return its collected bindings and modifiers.
    pub(crate) fn finish(self) -> (Vec<Binding>, Option<StyleOverlay>, bool) {
        let mut guard = lock_unpoisoned(&self.state);
        let bindings = mem::take(&mut guard.bindings);
        let styles = mem::take(&mut guard.styles);
        let style = merge_style_overlays(&styles);
        let capture = guard.capture;
        (bindings, style, capture)
    }
}

/// Merge a sequence of raw style overlays into a single overlay.
fn merge_style_overlays(styles: &[raw::RawStyle]) -> Option<StyleOverlay> {
    if styles.is_empty() {
        return None;
    }

    let mut merged = raw::RawStyle::default();
    for overlay in styles {
        merged = merged.merge(overlay);
    }

    Some(StyleOverlay {
        func: None,
        raw: Some(merged),
    })
}

#[derive(Clone)]
/// Opaque handle returned by `bind()`/`mode()` to apply fluent binding modifiers.
pub struct BindingRef {
    /// Shared builder state used to mutate the referenced binding.
    state: Arc<Mutex<ModeBuildState>>,
    /// Index into `ModeBuildState.bindings`.
    index: usize,
}

impl fmt::Debug for BindingRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BindingRef")
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
/// Opaque handle returned by `bind()` (array form) to apply fluent modifiers to multiple bindings.
pub struct BindingsRef {
    /// Shared builder state used to mutate the referenced bindings.
    state: Arc<Mutex<ModeBuildState>>,
    /// Indices into `ModeBuildState.bindings`.
    indices: Vec<usize>,
}

impl fmt::Debug for BindingsRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BindingsRef")
            .field("indices", &self.indices)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
/// Namespace object exported to Rhai scripts as `hotki`.
struct HotkiNamespace {
    /// Shared config script state.
    state: Arc<Mutex<DynamicConfigScriptState>>,
}

/// Parse a chord string or return a validation error.
fn parse_chord(ctx: &NativeCallContext, spec: &str) -> Result<Chord, Box<EvalAltResult>> {
    Chord::parse(spec).ok_or_else(|| {
        boxed_validation_error(
            format!("invalid chord string: {}", spec),
            ctx.call_position(),
        )
    })
}

/// Derive a stable-ish identity for a mode closure for orphan detection.
fn mode_id_for(func: &FnPtr) -> ModeId {
    let mut hasher = DefaultHasher::new();
    func.fn_name().hash(&mut hasher);
    ModeId::new(hasher.finish())
}

/// Derive a stable identity for a static mode from its bindings.
fn mode_id_for_static(bindings: &[Binding]) -> ModeId {
    let mut hasher = DefaultHasher::new();
    for b in bindings {
        b.chord.to_string().hash(&mut hasher);
        b.desc.hash(&mut hasher);
    }
    ModeId::new(hasher.finish())
}

/// Register all dynamic config DSL types and functions into a Rhai engine.
pub fn register_dsl(engine: &mut Engine, state: Arc<Mutex<DynamicConfigScriptState>>) {
    engine.register_type::<ModeBuilder>();
    engine.register_type::<BindingRef>();
    engine.register_type::<BindingsRef>();
    engine.register_type::<Action>();
    engine.register_type::<Toggle>();
    engine.register_type::<NotifyKind>();
    engine.register_type::<Pos>();
    engine.register_type::<NotifyPos>();
    engine.register_type::<Mode>();
    engine.register_type::<FontWeight>();

    register_global_constants(engine);
    register_hotki_namespace(engine, state.clone());
    register_action_namespace(engine);
    super::style_api::register_style_type(engine);
    super::style_api::register_theme_api(engine, state);
    register_mode_builder(engine);
    register_handler(engine);
    register_action_fluent(engine);
    register_context_types(engine);
    register_string_matches(engine);
}

/// Register global constants used by the DSL (toggles, positions, weights, etc.).
fn register_global_constants(engine: &mut Engine) {
    let mut module = Module::new();

    module.set_var("on", Toggle::On);
    module.set_var("off", Toggle::Off);
    module.set_var("toggle", Toggle::Toggle);

    module.set_var("ignore", NotifyKind::Ignore);
    module.set_var("info", NotifyKind::Info);
    module.set_var("warn", NotifyKind::Warn);
    module.set_var("error", NotifyKind::Error);
    module.set_var("success", NotifyKind::Success);

    module.set_var("center", "center");
    module.set_var("n", "n");
    module.set_var("ne", "ne");
    module.set_var("e", "e");
    module.set_var("se", "se");
    module.set_var("s", "s");
    module.set_var("sw", "sw");
    module.set_var("w", "w");
    module.set_var("nw", "nw");

    module.set_var("left", "left");
    module.set_var("right", "right");

    module.set_var("hud", "hud");
    module.set_var("mini", "mini");
    module.set_var("hide", "hide");

    module.set_var("thin", "thin");
    module.set_var("light", "light");
    module.set_var("regular", "regular");
    module.set_var("medium", "medium");
    module.set_var("semibold", "semibold");
    module.set_var("bold", "bold");
    module.set_var("extrabold", "extrabold");
    module.set_var("black", "black");

    engine.register_global_module(module.into());
}

/// Register the global `hotki` namespace used to define the root mode.
fn register_hotki_namespace(engine: &mut Engine, state: Arc<Mutex<DynamicConfigScriptState>>) {
    engine.register_type_with_name::<HotkiNamespace>("HotkiNamespace");

    engine.register_fn(
        "mode",
        move |ctx: NativeCallContext,
              ns: HotkiNamespace,
              func: FnPtr|
              -> Result<(), Box<EvalAltResult>> {
            let mut guard = lock_unpoisoned(&ns.state);
            if guard.root.is_some() {
                return Err(boxed_validation_error(
                    "hotki.mode() must be called exactly once".to_string(),
                    ctx.call_position(),
                ));
            }

            guard.root = Some(ModeRef {
                id: mode_id_for(&func),
                func: Some(func),
                static_bindings: None,
                default_title: None,
            });
            Ok(())
        },
    );

    let mut module = Module::new();
    module.set_var("hotki", HotkiNamespace { state });
    engine.register_global_module(module.into());
}

#[derive(Debug, Clone, Copy)]
/// Namespace object exported to Rhai scripts as `action`.
struct ActionNamespace;

/// Register the global `action.*` factories and constants.
fn register_action_namespace(engine: &mut Engine) {
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
         -> Result<Action, Box<EvalAltResult>> {
            if !(0..=100).contains(&level) {
                return Err(boxed_validation_error(
                    format!("set_volume: level must be 0..=100, got {}", level),
                    ctx.call_position(),
                ));
            }
            let level_u8: u8 = level.try_into().map_err(|_| {
                boxed_validation_error(
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
         -> Result<Action, Box<EvalAltResult>> {
            if !(-100..=100).contains(&delta) {
                return Err(boxed_validation_error(
                    format!("change_volume: delta must be -100..=100, got {}", delta),
                    ctx.call_position(),
                ));
            }
            let delta_i8: i8 = delta.try_into().map_err(|_| {
                boxed_validation_error(
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

    let mut module = Module::new();
    module.set_var("action", ActionNamespace);
    engine.register_global_module(module.into());
}

/// Register fluent modifiers on action values (e.g., `shell(...).notify(...)`).
fn register_action_fluent(engine: &mut Engine) {
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
            _ => Err(boxed_validation_error(
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
        _ => Err(boxed_validation_error(
            "silent is only valid on shell(...) actions".to_string(),
            ctx.call_position(),
        )),
    });
}

/// Register the `handler(...)` factory.
fn register_handler(engine: &mut Engine) {
    engine.register_type::<HandlerRef>();
    engine.register_fn("handler", |func: FnPtr| HandlerRef { func });
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
/// Binding-level style overrides accepted by the DSL.
struct RawBindingStyle {
    #[serde(default)]
    /// When true, hide the binding from the HUD.
    hidden: Option<bool>,
    #[serde(default)]
    /// Override key foreground color.
    key_fg: Option<String>,
    #[serde(default)]
    /// Override key background color.
    key_bg: Option<String>,
    #[serde(default)]
    /// Override modifier foreground color.
    mod_fg: Option<String>,
    #[serde(default)]
    /// Override modifier background color.
    mod_bg: Option<String>,
    #[serde(default)]
    /// Override submenu tag color.
    tag_fg: Option<String>,
}

/// Apply a binding style overlay from a Rhai map.
fn binding_style_overlay(
    ctx: &NativeCallContext,
    binding: &mut Binding,
    map: Map,
) -> Result<(), Box<EvalAltResult>> {
    let dyn_map = Dynamic::from_map(map);
    let style: RawBindingStyle = from_dynamic(&dyn_map).map_err(|e| {
        boxed_validation_error(
            format!("invalid binding style map: {}", e),
            ctx.call_position(),
        )
    })?;

    if style.hidden.unwrap_or(false) {
        binding.flags.hidden = true;
    }

    if style.key_fg.is_none()
        && style.key_bg.is_none()
        && style.mod_fg.is_none()
        && style.mod_bg.is_none()
        && style.tag_fg.is_none()
    {
        binding.style = None;
        return Ok(());
    }

    let mut hud = raw::RawHud::default();
    if let Some(v) = style.key_fg {
        hud.key_fg = raw::Maybe::Value(v);
    }
    if let Some(v) = style.key_bg {
        hud.key_bg = raw::Maybe::Value(v);
    }
    if let Some(v) = style.mod_fg {
        hud.mod_fg = raw::Maybe::Value(v);
    }
    if let Some(v) = style.mod_bg {
        hud.mod_bg = raw::Maybe::Value(v);
    }
    if let Some(v) = style.tag_fg {
        hud.tag_fg = raw::Maybe::Value(v);
    }
    binding.style = Some(StyleOverlay {
        func: None,
        raw: Some(raw::RawStyle {
            hud: raw::Maybe::Value(hud),
            notify: raw::Maybe::Unit(()),
        }),
    });
    Ok(())
}

/// Register the `ModeBuilder` and `BindingRef` APIs (`bind`, `mode`, and modifiers).
fn register_mode_builder(engine: &mut Engine) {
    engine.register_fn(
        "bind",
        |ctx: NativeCallContext,
         m: &mut ModeBuilder,
         chord: &str,
         desc: &str,
         action: Action|
         -> Result<BindingRef, Box<EvalAltResult>> {
            let chord = parse_chord(&ctx, chord)?;
            let mut guard = lock_unpoisoned(&m.state);
            let idx = guard.bindings.len();
            guard.bindings.push(Binding {
                chord,
                desc: desc.to_string(),
                kind: BindingKind::Action(action),
                mode_id: None,
                flags: BindingFlags::default(),
                style: None,
                mode_capture: false,
                pos: ctx.call_position(),
            });
            Ok(BindingRef {
                state: m.state.clone(),
                index: idx,
            })
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
            let mut guard = lock_unpoisoned(&m.state);
            let idx = guard.bindings.len();
            guard.bindings.push(Binding {
                chord,
                desc: desc.to_string(),
                kind: BindingKind::Handler(handler),
                mode_id: None,
                flags: BindingFlags::default(),
                style: None,
                mode_capture: false,
                pos: ctx.call_position(),
            });
            Ok(BindingRef {
                state: m.state.clone(),
                index: idx,
            })
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
            let mut guard = lock_unpoisoned(&m.state);
            let idx = guard.bindings.len();
            let mode = ModeRef {
                id: mode_id_for(&func),
                func: Some(func),
                static_bindings: None,
                default_title: Some(desc.to_string()),
            };
            guard.bindings.push(Binding {
                chord,
                desc: desc.to_string(),
                kind: BindingKind::Mode(mode.clone()),
                mode_id: Some(mode.id),
                flags: BindingFlags::default(),
                style: None,
                mode_capture: false,
                pos: ctx.call_position(),
            });
            Ok(BindingRef {
                state: m.state.clone(),
                index: idx,
            })
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
            let mut guard = lock_unpoisoned(&m.state);
            let idx = guard.bindings.len();
            let mode = ModeRef {
                id: mode_id_for(&func),
                func: Some(func),
                static_bindings: None,
                default_title: Some(title.to_string()),
            };
            guard.bindings.push(Binding {
                chord,
                desc: title.to_string(),
                kind: BindingKind::Mode(mode.clone()),
                mode_id: Some(mode.id),
                flags: BindingFlags::default(),
                style: None,
                mode_capture: false,
                pos: ctx.call_position(),
            });
            Ok(BindingRef {
                state: m.state.clone(),
                index: idx,
            })
        },
    );

    // mode() with inline bindings array
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

            // Parse the bindings array into Binding objects
            let mut static_bindings = Vec::with_capacity(bindings.len());
            for (i, item) in bindings.into_iter().enumerate() {
                let arr = item.into_array().map_err(|_| {
                    boxed_validation_error(
                        format!(
                            "mode bindings: element {} must be an array [chord, desc, action]",
                            i
                        ),
                        pos,
                    )
                })?;

                if arr.len() != 3 {
                    return Err(boxed_validation_error(
                        format!(
                            "mode bindings: element {} must have exactly 3 items [chord, desc, action], got {}",
                            i,
                            arr.len()
                        ),
                        pos,
                    ));
                }

                let binding_chord_str = arr[0].clone().into_immutable_string().map_err(|_| {
                    boxed_validation_error(
                        format!("mode bindings: element {} chord must be a string", i),
                        pos,
                    )
                })?;

                let desc = arr[1].clone().into_immutable_string().map_err(|_| {
                    boxed_validation_error(
                        format!("mode bindings: element {} description must be a string", i),
                        pos,
                    )
                })?;

                let binding_chord = Chord::parse(&binding_chord_str).ok_or_else(|| {
                    boxed_validation_error(
                        format!(
                            "mode bindings: element {} has invalid chord: {}",
                            i, binding_chord_str
                        ),
                        pos,
                    )
                })?;

                // Try to extract an Action or FnPtr (mode closure) from the third element
                let third = arr[2].clone();
                let (kind, binding_mode_id) =
                    if let Some(action) = third.clone().try_cast::<Action>() {
                        (BindingKind::Action(action), None)
                    } else if let Some(func) = third.try_cast::<FnPtr>() {
                        let nested_mode = ModeRef {
                            id: mode_id_for(&func),
                            func: Some(func),
                            static_bindings: None,
                            default_title: Some(desc.to_string()),
                        };
                        let id = nested_mode.id;
                        (BindingKind::Mode(nested_mode), Some(id))
                    } else {
                        return Err(boxed_validation_error(
                            format!(
                                "mode bindings: element {} must have an Action or mode closure as third item",
                                i
                            ),
                            pos,
                        ));
                    };

                static_bindings.push(Binding {
                    chord: binding_chord,
                    desc: desc.to_string(),
                    kind,
                    mode_id: binding_mode_id,
                    flags: BindingFlags::default(),
                    style: None,
                    mode_capture: false,
                    pos,
                });
            }

            // Generate a stable mode id from the bindings
            let id = mode_id_for_static(&static_bindings);

            let mode = ModeRef {
                id,
                func: None,
                static_bindings: Some(static_bindings),
                default_title: Some(title.to_string()),
            };

            let mut guard = lock_unpoisoned(&m.state);
            let idx = guard.bindings.len();
            guard.bindings.push(Binding {
                chord,
                desc: title.to_string(),
                kind: BindingKind::Mode(mode.clone()),
                mode_id: Some(mode.id),
                flags: BindingFlags::default(),
                style: None,
                mode_capture: false,
                pos,
            });
            Ok(BindingRef {
                state: m.state.clone(),
                index: idx,
            })
        },
    );

    engine.register_fn("capture", |m: &mut ModeBuilder| {
        lock_unpoisoned(&m.state).capture = true;
    });

    engine.register_fn(
        "style",
        |ctx: NativeCallContext, m: &mut ModeBuilder, map: Map| -> Result<(), Box<EvalAltResult>> {
            let dyn_map = Dynamic::from_map(map);
            let style: raw::RawStyle = from_dynamic(&dyn_map).map_err(|e| {
                boxed_validation_error(format!("invalid style map: {}", e), ctx.call_position())
            })?;
            lock_unpoisoned(&m.state).styles.push(style);
            Ok(())
        },
    );

    engine.register_fn("style", |m: &mut ModeBuilder, style: RhaiStyle| {
        lock_unpoisoned(&m.state).styles.push(style.raw);
    });

    engine.register_fn(
        "hidden",
        |ctx: NativeCallContext, b: BindingRef| -> Result<BindingRef, Box<EvalAltResult>> {
            {
                let mut guard = lock_unpoisoned(&b.state);
                let entry = guard.bindings.get_mut(b.index).ok_or_else(|| {
                    boxed_validation_error(
                        "invalid binding handle".to_string(),
                        ctx.call_position(),
                    )
                })?;
                entry.flags.hidden = true;
            }
            Ok(b)
        },
    );

    engine.register_fn(
        "stay",
        |ctx: NativeCallContext, b: BindingRef| -> Result<BindingRef, Box<EvalAltResult>> {
            {
                let mut guard = lock_unpoisoned(&b.state);
                let entry = guard.bindings.get_mut(b.index).ok_or_else(|| {
                    boxed_validation_error(
                        "invalid binding handle".to_string(),
                        ctx.call_position(),
                    )
                })?;
                entry.flags.stay = true;
            }
            Ok(b)
        },
    );

    engine.register_fn(
        "global",
        |ctx: NativeCallContext, b: BindingRef| -> Result<BindingRef, Box<EvalAltResult>> {
            {
                let mut guard = lock_unpoisoned(&b.state);
                let entry = guard.bindings.get_mut(b.index).ok_or_else(|| {
                    boxed_validation_error(
                        "invalid binding handle".to_string(),
                        ctx.call_position(),
                    )
                })?;
                if matches!(entry.kind, BindingKind::Mode(_)) {
                    return Err(boxed_validation_error(
                        "global() is not allowed on mode entries".to_string(),
                        ctx.call_position(),
                    ));
                }
                entry.flags.global = true;
            }
            Ok(b)
        },
    );

    engine.register_fn(
        "repeat",
        |ctx: NativeCallContext, b: BindingRef| -> Result<BindingRef, Box<EvalAltResult>> {
            {
                let mut guard = lock_unpoisoned(&b.state);
                let entry = guard.bindings.get_mut(b.index).ok_or_else(|| {
                    boxed_validation_error(
                        "invalid binding handle".to_string(),
                        ctx.call_position(),
                    )
                })?;
                match &entry.kind {
                    BindingKind::Action(Action::Shell(_))
                    | BindingKind::Action(Action::Relay(_))
                    | BindingKind::Action(Action::SetVolume(_))
                    | BindingKind::Action(Action::ChangeVolume(_)) => {}
                    BindingKind::Action(_) => {
                        return Err(boxed_validation_error(
                            "repeat() is only valid on shell(...), relay(...), set_volume(...), and change_volume(...) actions".to_string(),
                            ctx.call_position(),
                        ));
                    }
                    BindingKind::Handler(_) | BindingKind::Mode(_) => {
                        return Err(boxed_validation_error(
                            "repeat() is not valid on handlers or mode entries".to_string(),
                            ctx.call_position(),
                        ));
                    }
                }
                entry.flags.repeat = Some(RepeatSpec {
                    delay_ms: None,
                    interval_ms: None,
                });
                entry.flags.stay = true;
            }
            Ok(b)
        },
    );

    engine.register_fn(
        "repeat_ms",
        |ctx: NativeCallContext,
         b: BindingRef,
         delay: i64,
         interval: i64|
         -> Result<BindingRef, Box<EvalAltResult>> {
            let delay_ms: u64 = delay.try_into().map_err(|_| {
                boxed_validation_error("repeat_ms: delay must be >= 0".to_string(), ctx.call_position())
            })?;
            let interval_ms: u64 = interval.try_into().map_err(|_| {
                boxed_validation_error(
                    "repeat_ms: interval must be >= 0".to_string(),
                    ctx.call_position(),
                )
            })?;
            {
                let mut guard = lock_unpoisoned(&b.state);
                let entry = guard.bindings.get_mut(b.index).ok_or_else(|| {
                    boxed_validation_error(
                        "invalid binding handle".to_string(),
                        ctx.call_position(),
                    )
                })?;
                match &entry.kind {
                    BindingKind::Action(Action::Shell(_))
                    | BindingKind::Action(Action::Relay(_))
                    | BindingKind::Action(Action::SetVolume(_))
                    | BindingKind::Action(Action::ChangeVolume(_)) => {}
                    BindingKind::Action(_) => {
                        return Err(boxed_validation_error(
                            "repeat_ms() is only valid on shell(...), relay(...), set_volume(...), and change_volume(...) actions".to_string(),
                            ctx.call_position(),
                        ));
                    }
                    BindingKind::Handler(_) | BindingKind::Mode(_) => {
                        return Err(boxed_validation_error(
                            "repeat_ms() is not valid on handlers or mode entries".to_string(),
                            ctx.call_position(),
                        ));
                    }
                }
                entry.flags.repeat = Some(RepeatSpec {
                    delay_ms: Some(delay_ms),
                    interval_ms: Some(interval_ms),
                });
                entry.flags.stay = true;
            }
            Ok(b)
        },
    );

    engine.register_fn(
        "capture",
        |ctx: NativeCallContext, b: BindingRef| -> Result<BindingRef, Box<EvalAltResult>> {
            {
                let mut guard = lock_unpoisoned(&b.state);
                let entry = guard.bindings.get_mut(b.index).ok_or_else(|| {
                    boxed_validation_error(
                        "invalid binding handle".to_string(),
                        ctx.call_position(),
                    )
                })?;
                match entry.kind {
                    BindingKind::Mode(_) => {
                        entry.mode_capture = true;
                    }
                    _ => {
                        return Err(boxed_validation_error(
                            "capture() is only valid on mode entries".to_string(),
                            ctx.call_position(),
                        ));
                    }
                }
            }
            Ok(b)
        },
    );

    engine.register_fn(
        "style",
        |ctx: NativeCallContext,
         b: BindingRef,
         map: Map|
         -> Result<BindingRef, Box<EvalAltResult>> {
            {
                let mut guard = lock_unpoisoned(&b.state);
                let entry = guard.bindings.get_mut(b.index).ok_or_else(|| {
                    boxed_validation_error(
                        "invalid binding handle".to_string(),
                        ctx.call_position(),
                    )
                })?;
                binding_style_overlay(&ctx, entry, map)?;
            }
            Ok(b)
        },
    );

    engine.register_fn(
        "style",
        |ctx: NativeCallContext,
         b: BindingRef,
         func: FnPtr|
         -> Result<BindingRef, Box<EvalAltResult>> {
            {
                let mut guard = lock_unpoisoned(&b.state);
                let entry = guard.bindings.get_mut(b.index).ok_or_else(|| {
                    boxed_validation_error(
                        "invalid binding handle".to_string(),
                        ctx.call_position(),
                    )
                })?;
                if matches!(entry.kind, BindingKind::Mode(_)) {
                    return Err(boxed_validation_error(
                        "style(closure) is not supported on mode entries".to_string(),
                        ctx.call_position(),
                    ));
                }
                entry.style = Some(StyleOverlay {
                    func: Some(func),
                    raw: None,
                });
            }
            Ok(b)
        },
    );

    // Register bind() overload for batch binding creation (array form)
    engine.register_fn(
        "bind",
        |ctx: NativeCallContext,
         m: &mut ModeBuilder,
         bindings: rhai::Array|
         -> Result<BindingsRef, Box<EvalAltResult>> {
            let mut indices = Vec::with_capacity(bindings.len());
            let mut guard = lock_unpoisoned(&m.state);

            for (i, item) in bindings.into_iter().enumerate() {
                let arr = item.into_array().map_err(|_| {
                    boxed_validation_error(
                        format!("bind: element {} must be an array [chord, desc, action]", i),
                        ctx.call_position(),
                    )
                })?;

                if arr.len() != 3 {
                    return Err(boxed_validation_error(
                        format!(
                            "bind: element {} must have exactly 3 items [chord, desc, action], got {}",
                            i,
                            arr.len()
                        ),
                        ctx.call_position(),
                    ));
                }

                let chord_str = arr[0].clone().into_immutable_string().map_err(|_| {
                    boxed_validation_error(
                        format!("bind: element {} chord must be a string", i),
                        ctx.call_position(),
                    )
                })?;

                let desc = arr[1].clone().into_immutable_string().map_err(|_| {
                    boxed_validation_error(
                        format!("bind: element {} description must be a string", i),
                        ctx.call_position(),
                    )
                })?;

                let chord = Chord::parse(&chord_str).ok_or_else(|| {
                    boxed_validation_error(
                        format!("bind: element {} has invalid chord: {}", i, chord_str),
                        ctx.call_position(),
                    )
                })?;

                // Try to extract an Action or FnPtr (mode closure) from the third element
                let third = arr[2].clone();
                let (kind, mode_id) =
                    if let Some(action) = third.clone().try_cast::<Action>() {
                        (BindingKind::Action(action), None)
                    } else if let Some(func) = third.try_cast::<FnPtr>() {
                        let mode = ModeRef {
                            id: mode_id_for(&func),
                            func: Some(func),
                            static_bindings: None,
                            default_title: Some(desc.to_string()),
                        };
                        let id = mode.id;
                        (BindingKind::Mode(mode), Some(id))
                    } else {
                        return Err(boxed_validation_error(
                            format!(
                                "bind: element {} must have an Action or mode closure as third item",
                                i
                            ),
                            ctx.call_position(),
                        ));
                    };

                let idx = guard.bindings.len();
                guard.bindings.push(Binding {
                    chord,
                    desc: desc.to_string(),
                    kind,
                    mode_id,
                    flags: BindingFlags::default(),
                    style: None,
                    mode_capture: false,
                    pos: ctx.call_position(),
                });
                indices.push(idx);
            }

            drop(guard);
            Ok(BindingsRef {
                state: m.state.clone(),
                indices,
            })
        },
    );

    // Register modifier methods on BindingsRef
    engine.register_fn(
        "hidden",
        |ctx: NativeCallContext, b: BindingsRef| -> Result<BindingsRef, Box<EvalAltResult>> {
            let mut guard = lock_unpoisoned(&b.state);
            for &idx in &b.indices {
                let entry = guard.bindings.get_mut(idx).ok_or_else(|| {
                    boxed_validation_error(
                        "invalid binding handle".to_string(),
                        ctx.call_position(),
                    )
                })?;
                entry.flags.hidden = true;
            }
            drop(guard);
            Ok(b)
        },
    );

    engine.register_fn(
        "stay",
        |ctx: NativeCallContext, b: BindingsRef| -> Result<BindingsRef, Box<EvalAltResult>> {
            let mut guard = lock_unpoisoned(&b.state);
            for &idx in &b.indices {
                let entry = guard.bindings.get_mut(idx).ok_or_else(|| {
                    boxed_validation_error(
                        "invalid binding handle".to_string(),
                        ctx.call_position(),
                    )
                })?;
                entry.flags.stay = true;
            }
            drop(guard);
            Ok(b)
        },
    );

    engine.register_fn(
        "global",
        |ctx: NativeCallContext, b: BindingsRef| -> Result<BindingsRef, Box<EvalAltResult>> {
            let mut guard = lock_unpoisoned(&b.state);
            for &idx in &b.indices {
                let entry = guard.bindings.get_mut(idx).ok_or_else(|| {
                    boxed_validation_error(
                        "invalid binding handle".to_string(),
                        ctx.call_position(),
                    )
                })?;
                if matches!(entry.kind, BindingKind::Mode(_)) {
                    return Err(boxed_validation_error(
                        "global() is not allowed on mode entries".to_string(),
                        ctx.call_position(),
                    ));
                }
                entry.flags.global = true;
            }
            drop(guard);
            Ok(b)
        },
    );

    engine.register_fn(
        "repeat",
        |ctx: NativeCallContext, b: BindingsRef| -> Result<BindingsRef, Box<EvalAltResult>> {
            let mut guard = lock_unpoisoned(&b.state);
            for &idx in &b.indices {
                let entry = guard.bindings.get_mut(idx).ok_or_else(|| {
                    boxed_validation_error(
                        "invalid binding handle".to_string(),
                        ctx.call_position(),
                    )
                })?;
                match &entry.kind {
                    BindingKind::Action(Action::Shell(_))
                    | BindingKind::Action(Action::Relay(_))
                    | BindingKind::Action(Action::SetVolume(_))
                    | BindingKind::Action(Action::ChangeVolume(_)) => {}
                    BindingKind::Action(_) => {
                        return Err(boxed_validation_error(
                            "repeat() is only valid on shell(...), relay(...), set_volume(...), and change_volume(...) actions".to_string(),
                            ctx.call_position(),
                        ));
                    }
                    BindingKind::Handler(_) | BindingKind::Mode(_) => {
                        return Err(boxed_validation_error(
                            "repeat() is not valid on handlers or mode entries".to_string(),
                            ctx.call_position(),
                        ));
                    }
                }
                entry.flags.repeat = Some(RepeatSpec {
                    delay_ms: None,
                    interval_ms: None,
                });
                entry.flags.stay = true;
            }
            drop(guard);
            Ok(b)
        },
    );

    engine.register_fn(
        "repeat_ms",
        |ctx: NativeCallContext,
         b: BindingsRef,
         delay: i64,
         interval: i64|
         -> Result<BindingsRef, Box<EvalAltResult>> {
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
            let mut guard = lock_unpoisoned(&b.state);
            for &idx in &b.indices {
                let entry = guard.bindings.get_mut(idx).ok_or_else(|| {
                    boxed_validation_error(
                        "invalid binding handle".to_string(),
                        ctx.call_position(),
                    )
                })?;
                match &entry.kind {
                    BindingKind::Action(Action::Shell(_))
                    | BindingKind::Action(Action::Relay(_))
                    | BindingKind::Action(Action::SetVolume(_))
                    | BindingKind::Action(Action::ChangeVolume(_)) => {}
                    BindingKind::Action(_) => {
                        return Err(boxed_validation_error(
                            "repeat_ms() is only valid on shell(...), relay(...), set_volume(...), and change_volume(...) actions".to_string(),
                            ctx.call_position(),
                        ));
                    }
                    BindingKind::Handler(_) | BindingKind::Mode(_) => {
                        return Err(boxed_validation_error(
                            "repeat_ms() is not valid on handlers or mode entries".to_string(),
                            ctx.call_position(),
                        ));
                    }
                }
                entry.flags.repeat = Some(RepeatSpec {
                    delay_ms: Some(delay_ms),
                    interval_ms: Some(interval_ms),
                });
                entry.flags.stay = true;
            }
            drop(guard);
            Ok(b)
        },
    );

    engine.register_fn(
        "style",
        |ctx: NativeCallContext,
         b: BindingsRef,
         map: Map|
         -> Result<BindingsRef, Box<EvalAltResult>> {
            let mut guard = lock_unpoisoned(&b.state);
            for &idx in &b.indices {
                let entry = guard.bindings.get_mut(idx).ok_or_else(|| {
                    boxed_validation_error(
                        "invalid binding handle".to_string(),
                        ctx.call_position(),
                    )
                })?;
                binding_style_overlay(&ctx, entry, map.clone())?;
            }
            drop(guard);
            Ok(b)
        },
    );
}

/// Register `String.matches(regex)` used in render and handler contexts.
fn register_string_matches(engine: &mut Engine) {
    engine.register_fn(
        "matches",
        |ctx: NativeCallContext, s: &str, pat: &str| -> Result<bool, Box<EvalAltResult>> {
            let re = regex::Regex::new(pat).map_err(|e| {
                boxed_validation_error(
                    format!("invalid regex '{}': {}", pat, e),
                    ctx.call_position(),
                )
            })?;
            Ok(re.is_match(s))
        },
    );
}

/// Register `ModeCtx` and `ActionCtx` types and methods.
fn register_context_types(engine: &mut Engine) {
    engine.register_type::<ModeCtx>();
    engine.register_get("app", |ctx: &mut ModeCtx| ctx.app.clone());
    engine.register_get("title", |ctx: &mut ModeCtx| ctx.title.clone());
    engine.register_get("pid", |ctx: &mut ModeCtx| ctx.pid);
    engine.register_get("hud", |ctx: &mut ModeCtx| ctx.hud);
    engine.register_get("depth", |ctx: &mut ModeCtx| ctx.depth);

    engine.register_type::<ActionCtx>();
    engine.register_get("app", |ctx: &mut ActionCtx| ctx.app.clone());
    engine.register_get("title", |ctx: &mut ActionCtx| ctx.title.clone());
    engine.register_get("pid", |ctx: &mut ActionCtx| ctx.pid);
    engine.register_get("hud", |ctx: &mut ActionCtx| ctx.hud);
    engine.register_get("depth", |ctx: &mut ActionCtx| ctx.depth);

    engine.register_fn("exec", |ctx: &mut ActionCtx, a: Action| {
        ctx.push_effect(super::Effect::Exec(a));
    });
    engine.register_fn(
        "notify",
        |ctx: &mut ActionCtx, kind: NotifyKind, title: &str, body: &str| {
            ctx.push_effect(super::Effect::Notify {
                kind,
                title: title.to_string(),
                body: body.to_string(),
            });
        },
    );
    engine.register_fn("stay", |ctx: &mut ActionCtx| {
        ctx.set_stay();
    });
    engine.register_fn("push", |ctx: &mut ActionCtx, func: FnPtr| {
        ctx.set_nav(NavRequest::Push {
            mode: ModeRef {
                id: mode_id_for(&func),
                func: Some(func),
                static_bindings: None,
                default_title: None,
            },
            title: None,
        });
    });
    engine.register_fn("push", |ctx: &mut ActionCtx, func: FnPtr, title: &str| {
        let title = title.to_string();
        ctx.set_nav(NavRequest::Push {
            mode: ModeRef {
                id: mode_id_for(&func),
                func: Some(func),
                static_bindings: None,
                default_title: Some(title.clone()),
            },
            title: Some(title),
        });
    });
    engine.register_fn("pop", |ctx: &mut ActionCtx| {
        ctx.set_nav(NavRequest::Pop);
    });
    engine.register_fn("exit", |ctx: &mut ActionCtx| {
        ctx.set_nav(NavRequest::Exit);
    });
    engine.register_fn("show_root", |ctx: &mut ActionCtx| {
        ctx.set_nav(NavRequest::ShowRoot);
    });
}
