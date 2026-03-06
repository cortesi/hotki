use std::{
    cell::RefCell,
    collections::{HashMap, hash_map::DefaultHasher},
    fmt,
    hash::{Hash, Hasher},
    mem,
    sync::{Arc, Mutex},
};

use mac_keycode::Chord;
use rhai::{Engine, EvalAltResult, FnPtr, Module, NativeCallContext};

use super::{
    Binding, BindingKind, ModeId, ModeRef, SelectorConfig, SelectorItem, SelectorItems,
    StyleOverlay, constants, util::lock_unpoisoned, validation::boxed_validation_error,
};
use crate::{Action, FontWeight, Mode, NotifyKind, NotifyPos, Pos, Toggle, raw, themes};

/// Rhai `action.*` namespace and fluent action helpers.
#[path = "dsl/action_api.rs"]
mod action_api;
/// Rhai mode/binding builder API and fluent binding modifiers.
#[path = "dsl/binding_api.rs"]
mod binding_api;
/// Rhai context object registration for mode and handler closures.
#[path = "dsl/context_api.rs"]
mod context_api;
/// Rhai selector schema parsing and selector builder helpers.
#[path = "dsl/selector_api.rs"]
mod selector_api;

#[derive(Debug)]
/// Script-global state captured while evaluating a dynamic config.
pub struct DynamicConfigScriptState {
    /// Theme registry populated with builtins and script registrations.
    pub(crate) themes: HashMap<String, raw::RawStyle>,
    /// Active theme name selected via `theme("...")`.
    pub(crate) active_theme: String,
    /// Root mode closure declared via `hotki.mode(...)`.
    pub(crate) root: Option<ModeRef>,
    /// Cached installed application list for selector helpers.
    pub(crate) applications_cache: Option<Arc<[SelectorItem]>>,
}

impl Default for DynamicConfigScriptState {
    fn default() -> Self {
        let themes = themes::builtin_raw_themes()
            .iter()
            .map(|(name, raw)| ((*name).to_string(), raw.clone()))
            .collect();

        Self {
            themes,
            active_theme: "default".to_string(),
            root: None,
            applications_cache: None,
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
        // Include action identity so changing an action produces a different mode ID
        match &b.kind {
            BindingKind::Action(action) => action.hash(&mut hasher),
            BindingKind::Handler(_) => "handler".hash(&mut hasher),
            BindingKind::Selector(cfg) => {
                "selector".hash(&mut hasher);
                cfg.title.hash(&mut hasher);
                cfg.placeholder.hash(&mut hasher);
                cfg.max_visible.hash(&mut hasher);
                match &cfg.items {
                    SelectorItems::Static(items) => {
                        for item in items {
                            item.label.hash(&mut hasher);
                            item.sublabel.hash(&mut hasher);
                        }
                    }
                    SelectorItems::Provider(func) => {
                        func.fn_name().hash(&mut hasher);
                    }
                }
                cfg.on_select.func.fn_name().hash(&mut hasher);
                cfg.on_cancel
                    .as_ref()
                    .map(|h| h.func.fn_name())
                    .hash(&mut hasher);
            }
            BindingKind::Mode(mode_ref) => mode_ref.id.hash(&mut hasher),
        }
    }
    ModeId::new(hasher.finish())
}

/// Register all dynamic config DSL types and functions into a Rhai engine.
pub fn register_dsl(engine: &mut Engine, state: Arc<Mutex<DynamicConfigScriptState>>) {
    engine.register_type::<ModeBuilder>();
    engine.register_type::<Action>();
    engine.register_type::<SelectorConfig>();
    engine.register_type::<Toggle>();
    engine.register_type::<NotifyKind>();
    engine.register_type::<Pos>();
    engine.register_type::<NotifyPos>();
    engine.register_type::<Mode>();
    engine.register_type::<FontWeight>();

    constants::register_dsl_constants(engine);
    register_hotki_namespace(engine, state.clone());
    action_api::register_handler_type(engine);
    action_api::register_action_namespace(engine);
    super::style_api::register_style_type(engine);
    super::style_api::register_theme_api(engine, state.clone());
    binding_api::register_mode_builder(engine);
    action_api::register_action_fluent(engine);
    context_api::register_context_types(engine);
    register_string_matches(engine);
    super::apps::register_apps_api(engine, state);
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

thread_local! {
    /// Thread-local cache for compiled regexes to avoid recompilation on every render.
    static REGEX_CACHE: RefCell<HashMap<String, regex::Regex>> = RefCell::new(HashMap::new());
}

/// Register `String.matches(regex)` used in render and handler contexts.
fn register_string_matches(engine: &mut Engine) {
    engine.register_fn(
        "matches",
        |ctx: NativeCallContext, s: &str, pat: &str| -> Result<bool, Box<EvalAltResult>> {
            REGEX_CACHE.with(|cache| {
                let mut cache = cache.borrow_mut();
                if let Some(re) = cache.get(pat) {
                    return Ok(re.is_match(s));
                }
                let re = regex::Regex::new(pat).map_err(|e| {
                    boxed_validation_error(
                        format!("invalid regex '{}': {}", pat, e),
                        ctx.call_position(),
                    )
                })?;
                let result = re.is_match(s);
                cache.insert(pat.to_string(), re);
                Ok(result)
            })
        },
    );
}
