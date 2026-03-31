use std::{
    collections::HashMap,
    ffi::OsStr,
    fmt, fs, mem,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use mac_keycode::Chord;
use mlua::{
    AnyUserData, Function, Lua, LuaSerdeExt, Result as LuaResult, StdLib, Table, UserData,
    UserDataFields, UserDataMethods, Value, VmState,
};
use regex::Regex;
use serde::Deserialize;

use super::{
    ActionCtx, Binding, BindingFlags, BindingKind, DynamicConfig, HandlerRef, ModeCtx, ModeRef,
    NavRequest, RepeatSpec, SelectorConfig, SelectorItems, SourcePos, StyleOverlay, apps,
    binding_style::BindingStyleSpec, selector, util::lock_unpoisoned,
};
use crate::{Action, Error, NotifyKind, Toggle, error::excerpt_at, raw, themes};

/// Build a locationless `Error::Validation` with only a path and message.
fn validation_error(path: Option<PathBuf>, err: impl fmt::Display) -> Error {
    Error::Validation {
        path,
        line: None,
        col: None,
        message: err.to_string(),
        excerpt: None,
    }
}

/// Maximum instruction interrupts allowed while evaluating a Luau chunk.
const EXECUTION_LIMIT: u64 = 200_000;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
/// Software-repeat options parsed from Luau binding tables.
struct RepeatOptionsSpec {
    /// Optional initial repeat delay in milliseconds.
    delay_ms: Option<u64>,
    /// Optional repeat interval in milliseconds.
    interval_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
/// Shell action modifiers parsed from Luau tables.
struct ShellOptionsSpec {
    /// Notification kind used for successful shell exits.
    ok_notify: Option<NotifyKind>,
    /// Notification kind used for failing shell exits.
    err_notify: Option<NotifyKind>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
/// Common binding options parsed from Luau tables.
struct BindingOptionsSpec {
    /// Whether the binding should be hidden from the HUD.
    hidden: Option<bool>,
    /// Whether the binding should be inherited by child modes.
    global: Option<bool>,
    /// Whether the binding suppresses auto-exit after execution.
    stay: Option<bool>,
    /// Optional software-repeat configuration.
    repeat: Option<RepeatOptionsSpec>,
    /// Optional binding-level style overrides.
    style: Option<BindingStyleSpec>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
/// Submenu-specific binding options parsed from Luau tables.
struct SubmenuOptionsSpec {
    /// Embedded binding options shared with primitive bindings.
    #[serde(flatten)]
    binding: BindingOptionsSpec,
    /// Whether entering the submenu enables capture-all behavior.
    capture: Option<bool>,
}

#[derive(Debug, Clone, Default)]
/// Mutable loader state shared across the Luau runtime.
struct RuntimeState {
    /// Root mode declared by `hotki.root(...)`.
    root: Option<ModeRef>,
    /// Theme registry after built-in, user, and script registration.
    themes: HashMap<String, raw::RawStyle>,
    /// Active theme selected during loading.
    active_theme: String,
    /// Cached application selector items.
    applications_cache: Option<Arc<[super::SelectorItem]>>,
    /// Directory containing the root config file.
    config_dir: Option<PathBuf>,
    /// Source text cache used for excerpts and diagnostics.
    sources: super::config::SourceMap,
    /// Imported role modules keyed by `(role, canonical_path)`.
    imports: HashMap<(ImportRole, PathBuf), ImportedValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Role-specific import kinds accepted by the Luau host API.
enum ImportRole {
    /// Imported mode renderer.
    Mode,
    /// Imported selector items provider or static list.
    Items,
    /// Imported action handler.
    Handler,
    /// Imported style overlay.
    Style,
}

#[derive(Clone, Debug)]
/// Cached imported values stored in loader state.
enum ImportedValue {
    /// Imported mode renderer.
    Mode(ModeRef),
    /// Imported selector item provider.
    Items(Function),
    /// Imported action handler.
    Handler(HandlerRef),
    /// Imported style overlay.
    Style(Box<StyleOverlay>),
}

#[derive(Clone, Debug)]
/// Luau userdata used to build one rendered mode.
pub struct ModeBuilder {
    /// Shared mutable builder state populated by Luau methods.
    state: Arc<Mutex<ModeBuildState>>,
}

#[derive(Debug, Default)]
/// Mutable contents collected by a `ModeBuilder`.
struct ModeBuildState {
    /// Bindings declared by the current mode render.
    bindings: Vec<Binding>,
    /// Mode-level style overlays applied during render.
    styles: Vec<raw::RawStyle>,
    /// Whether the mode requested capture-all behavior.
    capture: bool,
}

#[derive(Clone, Debug)]
/// Opaque Luau userdata wrapping an action payload.
struct ActionValue {
    /// Underlying action-like payload.
    payload: ActionPayload,
}

#[derive(Clone, Debug)]
/// Supported payload variants for Luau action userdata.
enum ActionPayload {
    /// Primitive engine action.
    Action(Action),
    /// Handler closure.
    Handler(HandlerRef),
    /// Selector popup configuration.
    Selector(SelectorConfig),
}

#[derive(Clone, Debug)]
/// Luau userdata wrapper for mode render contexts.
struct ModeContextUserData(ModeCtx);

#[derive(Clone, Debug)]
/// Luau userdata wrapper for action handler contexts.
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
        let style = merge_style_overlays(&guard.styles).map(|raw| StyleOverlay { raw });
        (bindings, style, capture)
    }
}

impl UserData for ModeBuilder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut(
            "bind",
            |lua, this, (chord, desc, action_value, opts): (String, String, AnyUserData, Value)| {
                let action_value = action_value.borrow::<ActionValue>()?.clone();
                let options = parse_optional::<BindingOptionsSpec>(lua, opts)?;
                let binding = binding_from_action(lua, &chord, desc, action_value, options)?;
                lock_unpoisoned(&this.state).bindings.push(binding);
                Ok(())
            },
        );

        methods.add_method_mut("bind_many", |lua, this, entries: Table| {
            for entry in entries.sequence_values::<Table>() {
                let entry = entry?;
                if let Ok(action_ud) = entry.get::<AnyUserData>("action") {
                    let chord: String = entry.get("chord")?;
                    let desc: String = entry.get("desc")?;
                    let opts = entry.get::<Value>("opts").unwrap_or(Value::Nil);
                    let action_value = action_ud.borrow::<ActionValue>()?.clone();
                    let options = parse_optional::<BindingOptionsSpec>(lua, opts)?;
                    let binding = binding_from_action(lua, &chord, desc, action_value, options)?;
                    lock_unpoisoned(&this.state).bindings.push(binding);
                    continue;
                }

                let chord: String = entry.get("chord")?;
                let title: String = entry.get("title")?;
                let render: Function = entry.get("render")?;
                let opts = entry.get::<Value>("opts").unwrap_or(Value::Nil);
                let options = parse_optional::<SubmenuOptionsSpec>(lua, opts)?;
                let binding = binding_from_mode(lua, &chord, title, render, options)?;
                lock_unpoisoned(&this.state).bindings.push(binding);
            }
            Ok(())
        });

        methods.add_method_mut(
            "submenu",
            |lua, this, (chord, title, render, opts): (String, String, Function, Value)| {
                let options = parse_optional::<SubmenuOptionsSpec>(lua, opts)?;
                let binding = binding_from_mode(lua, &chord, title, render, options)?;
                lock_unpoisoned(&this.state).bindings.push(binding);
                Ok(())
            },
        );

        methods.add_method_mut("style", |lua, this, overlay: Value| {
            let raw = parse_raw_style(lua, overlay)?;
            lock_unpoisoned(&this.state).styles.push(raw);
            Ok(())
        });

        methods.add_method_mut("capture", |_lua, this, ()| {
            lock_unpoisoned(&this.state).capture = true;
            Ok(())
        });
    }
}

impl UserData for ActionValue {}

impl UserData for ModeContextUserData {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("app", |_lua, this| Ok(this.0.app.clone()));
        fields.add_field_method_get("title", |_lua, this| Ok(this.0.title.clone()));
        fields.add_field_method_get("pid", |_lua, this| Ok(this.0.pid));
        fields.add_field_method_get("hud", |_lua, this| Ok(this.0.hud));
        fields.add_field_method_get("depth", |_lua, this| Ok(this.0.depth));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("app_matches", |_lua, this, pattern: String| {
            regex_matches(&this.0.app, &pattern)
        });
        methods.add_method("title_matches", |_lua, this, pattern: String| {
            regex_matches(&this.0.title, &pattern)
        });
    }
}

impl UserData for ActionContextUserData {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("app", |_lua, this| Ok(this.0.app().to_string()));
        fields.add_field_method_get("title", |_lua, this| Ok(this.0.title().to_string()));
        fields.add_field_method_get("pid", |_lua, this| Ok(this.0.pid()));
        fields.add_field_method_get("hud", |_lua, this| Ok(this.0.hud()));
        fields.add_field_method_get("depth", |_lua, this| Ok(this.0.depth()));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("app_matches", |_lua, this, pattern: String| {
            regex_matches(this.0.app(), &pattern)
        });
        methods.add_method("title_matches", |_lua, this, pattern: String| {
            regex_matches(this.0.title(), &pattern)
        });
        methods.add_method(
            "notify",
            |lua, this, (kind, title, body): (Value, String, String)| {
                let kind: NotifyKind = lua.from_value(kind)?;
                this.0
                    .push_effect(super::Effect::Notify { kind, title, body });
                Ok(())
            },
        );
        methods.add_method("stay", |_lua, this, ()| {
            this.0.set_stay();
            Ok(())
        });
        methods.add_method("exec", |_lua, this, action_ud: AnyUserData| {
            let action = action_ud.borrow::<ActionValue>()?.clone();
            match action.payload {
                ActionPayload::Action(action) => {
                    this.0.push_effect(super::Effect::Exec(action));
                    Ok(())
                }
                _ => Err(mlua::Error::runtime("ctx:exec expects a primitive action")),
            }
        });
        methods.add_method(
            "push",
            |_lua, this, (render, title): (Function, Option<String>)| {
                this.0.set_nav(NavRequest::Push {
                    mode: ModeRef::from_function(render, title.clone()),
                    title,
                });
                Ok(())
            },
        );
        methods.add_method("pop", |_lua, this, ()| {
            this.0.set_nav(NavRequest::Pop);
            Ok(())
        });
        methods.add_method("exit", |_lua, this, ()| {
            this.0.set_nav(NavRequest::Exit);
            Ok(())
        });
        methods.add_method("show_root", |_lua, this, ()| {
            this.0.set_nav(NavRequest::ShowRoot);
            Ok(())
        });
    }
}

/// Load a Luau config from a file at `path`.
pub fn load_dynamic_config(path: &Path) -> Result<DynamicConfig, Error> {
    if path.extension() != Some(OsStr::new("luau")) {
        return Err(Error::Read {
            path: Some(path.to_path_buf()),
            message: "Unsupported config format (expected a .luau file)".to_string(),
        });
    }

    let source = fs::read_to_string(path).map_err(|err| Error::Read {
        path: Some(path.to_path_buf()),
        message: err.to_string(),
    })?;

    load_dynamic_config_from_string(&source, Some(path.to_path_buf()))
}

/// Load a Luau config from source text and an optional origin path.
pub fn load_dynamic_config_from_string(
    source: &str,
    path: Option<PathBuf>,
) -> Result<DynamicConfig, Error> {
    let sources = Arc::new(Mutex::new(HashMap::new()));
    if let Some(path) = &path {
        lock_unpoisoned(&sources)
            .insert(path.clone(), Arc::from(source.to_string().into_boxed_str()));
    }

    let mut state = RuntimeState {
        active_theme: "default".to_string(),
        config_dir: path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf),
        sources: sources.clone(),
        ..RuntimeState::default()
    };

    if let Some(dir) = path.as_deref().and_then(Path::parent) {
        state
            .themes
            .extend(themes::load_user_themes(&dir.join("themes"))?);
    }
    state.themes.extend(
        themes::builtin_raw_themes()
            .iter()
            .map(|(name, raw)| ((*name).to_string(), raw.clone())),
    );

    let state = Arc::new(Mutex::new(state));
    let lua = Lua::new_with(StdLib::ALL_SAFE, mlua::LuaOptions::default())
        .map_err(|err| validation_error(path.clone(), err))?;
    lua.sandbox(true)
        .map_err(|err| validation_error(path.clone(), err))?;
    let interrupt_steps = install_interrupt_limit(&lua);
    install_api(&lua, state.clone()).map_err(|err| validation_error(path.clone(), err))?;

    reset_execution_budget(&interrupt_steps);
    lua.load(source)
        .set_name(path_display(path.as_deref()))
        .exec()
        .map_err(|err| load_error_from_mlua(source, path.as_deref(), &err))?;

    let root = lock_unpoisoned(&state).root.clone().ok_or_else(|| {
        validation_error(path.clone(), "hotki.root() must be called exactly once")
    })?;
    reset_execution_budget(&interrupt_steps);
    validate_root(&lua, &root, path.as_deref(), source)?;

    let state = lock_unpoisoned(&state);
    Ok(DynamicConfig {
        root,
        themes: state.themes.clone(),
        active_theme: state.active_theme.clone(),
        lua,
        path,
        sources,
        interrupt_steps,
    })
}

/// Wrap a render context snapshot as Luau userdata.
pub fn mode_context_userdata(lua: &Lua, ctx: ModeCtx) -> LuaResult<AnyUserData> {
    lua.create_userdata(ModeContextUserData(ctx))
}

/// Wrap an action context snapshot as Luau userdata.
pub fn action_context_userdata(lua: &Lua, ctx: ActionCtx) -> LuaResult<AnyUserData> {
    lua.create_userdata(ActionContextUserData(ctx))
}

/// Invoke the configured root mode once to validate its output shape.
fn validate_root(
    lua: &Lua,
    root: &ModeRef,
    path: Option<&Path>,
    source: &str,
) -> Result<(), Error> {
    let builder = lua
        .create_userdata(ModeBuilder::new_for_render(None, false))
        .map_err(|err| validation_error(path.map(Path::to_path_buf), err))?;
    let ctx = mode_context_userdata(
        lua,
        ModeCtx {
            app: String::new(),
            title: String::new(),
            pid: 0,
            hud: false,
            depth: 0,
        },
    )
    .map_err(|err| validation_error(path.map(Path::to_path_buf), err))?;

    root.func
        .call::<()>((builder, ctx))
        .map_err(|err| load_error_from_mlua(source, path, &err))
}

/// Install the top-level Luau host API globals.
fn install_api(lua: &Lua, state: Arc<Mutex<RuntimeState>>) -> LuaResult<()> {
    let globals = lua.globals();
    globals.set("hotki", hotki_table(lua, state.clone())?)?;
    globals.set("action", action_table(lua)?)?;
    globals.set("themes", themes_table(lua, state)?)?;
    Ok(())
}

/// Construct the `hotki` global table.
fn hotki_table(lua: &Lua, state: Arc<Mutex<RuntimeState>>) -> LuaResult<Table> {
    let hotki = lua.create_table()?;

    let state_for_root = state.clone();
    hotki.set(
        "root",
        lua.create_function(move |_lua, render: Function| {
            let mut guard = lock_unpoisoned(&state_for_root);
            if guard.root.is_some() {
                return Err(mlua::Error::runtime(
                    "hotki.root() must be called exactly once",
                ));
            }
            guard.root = Some(ModeRef::from_function(render, None));
            Ok(())
        })?,
    )?;

    let state_for_apps = state.clone();
    hotki.set(
        "applications",
        lua.create_function(move |lua, ()| {
            let cached = { lock_unpoisoned(&state_for_apps).applications_cache.clone() };
            let items = if let Some(cached) = cached {
                cached
            } else {
                let apps = apps::application_items(lua)?;
                let shared: Arc<[super::SelectorItem]> = apps.into();
                lock_unpoisoned(&state_for_apps).applications_cache = Some(shared.clone());
                shared
            };
            selector_items_table(lua, items.as_ref())
        })?,
    )?;

    hotki.set(
        "import_mode",
        import_function(lua, state.clone(), ImportRole::Mode)?,
    )?;
    hotki.set(
        "import_items",
        import_function(lua, state.clone(), ImportRole::Items)?,
    )?;
    hotki.set(
        "import_handler",
        import_function(lua, state.clone(), ImportRole::Handler)?,
    )?;
    hotki.set(
        "import_style",
        import_function(lua, state, ImportRole::Style)?,
    )?;

    Ok(hotki)
}

/// Construct the `action` global table.
fn action_table(lua: &Lua) -> LuaResult<Table> {
    let action = lua.create_table()?;

    set_action_const(lua, &action, "pop", Action::Pop)?;
    set_action_const(lua, &action, "exit", Action::Exit)?;
    set_action_const(lua, &action, "show_root", Action::ShowRoot)?;
    set_action_const(lua, &action, "hide_hud", Action::HideHud)?;
    set_action_const(lua, &action, "reload_config", Action::ReloadConfig)?;
    set_action_const(
        lua,
        &action,
        "clear_notifications",
        Action::ClearNotifications,
    )?;
    set_action_const(lua, &action, "theme_next", Action::ThemeNext)?;
    set_action_const(lua, &action, "theme_prev", Action::ThemePrev)?;

    action.set(
        "shell",
        lua.create_function(|lua, (cmd, opts): (String, Value)| {
            let opts = parse_optional::<ShellOptionsSpec>(lua, opts)?;
            let defaults = crate::ShellModifiers::default();
            let spec = match opts {
                Some(opts) => crate::ShellSpec::WithMods(
                    cmd,
                    crate::ShellModifiers {
                        ok_notify: opts.ok_notify.unwrap_or(defaults.ok_notify),
                        err_notify: opts.err_notify.unwrap_or(defaults.err_notify),
                    },
                ),
                None => crate::ShellSpec::Cmd(cmd),
            };
            action_userdata(lua, ActionPayload::Action(Action::Shell(spec)))
        })?,
    )?;

    action.set(
        "open",
        lua.create_function(|lua, target: String| {
            action_userdata(lua, ActionPayload::Action(Action::Open(target)))
        })?,
    )?;
    action.set(
        "relay",
        lua.create_function(|lua, spec: String| {
            action_userdata(lua, ActionPayload::Action(Action::Relay(spec)))
        })?,
    )?;
    action.set(
        "show_details",
        lua.create_function(|lua, toggle: Value| {
            let toggle: Toggle = lua.from_value(toggle)?;
            action_userdata(lua, ActionPayload::Action(Action::ShowDetails(toggle)))
        })?,
    )?;
    action.set(
        "theme_set",
        lua.create_function(|lua, name: String| {
            action_userdata(lua, ActionPayload::Action(Action::ThemeSet(name)))
        })?,
    )?;
    action.set(
        "set_volume",
        lua.create_function(|lua, level: u8| {
            action_userdata(lua, ActionPayload::Action(Action::SetVolume(level)))
        })?,
    )?;
    action.set(
        "change_volume",
        lua.create_function(|lua, delta: i8| {
            action_userdata(lua, ActionPayload::Action(Action::ChangeVolume(delta)))
        })?,
    )?;
    action.set(
        "mute",
        lua.create_function(|lua, toggle: Value| {
            let toggle: Toggle = lua.from_value(toggle)?;
            action_userdata(lua, ActionPayload::Action(Action::Mute(toggle)))
        })?,
    )?;
    action.set(
        "run",
        lua.create_function(|lua, func: Function| {
            action_userdata(lua, ActionPayload::Handler(HandlerRef { func }))
        })?,
    )?;
    action.set(
        "selector",
        lua.create_function(|lua, spec: Value| {
            let selector = parse_selector_config(lua, spec)?;
            action_userdata(lua, ActionPayload::Selector(selector))
        })?,
    )?;

    Ok(action)
}

/// Construct the `themes` global table.
fn themes_table(lua: &Lua, state: Arc<Mutex<RuntimeState>>) -> LuaResult<Table> {
    let themes_table = lua.create_table()?;

    let state_for_use = state.clone();
    themes_table.set(
        "use",
        lua.create_function(move |_lua, (_this, name): (Table, String)| {
            let mut guard = lock_unpoisoned(&state_for_use);
            if !guard.themes.contains_key(name.as_str()) {
                return Err(mlua::Error::runtime(format!("unknown theme: {name}")));
            }
            guard.active_theme = name;
            Ok(())
        })?,
    )?;

    let state_for_current = state.clone();
    themes_table.set(
        "current",
        lua.create_function(move |_lua, _this: Table| {
            Ok(lock_unpoisoned(&state_for_current).active_theme.clone())
        })?,
    )?;

    let state_for_list = state.clone();
    themes_table.set(
        "list",
        lua.create_function(move |_lua, _this: Table| {
            let mut names = lock_unpoisoned(&state_for_list)
                .themes
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            names.sort();
            Ok(names)
        })?,
    )?;

    let state_for_get = state.clone();
    themes_table.set(
        "get",
        lua.create_function(move |lua, (_this, name): (Table, String)| {
            let raw = lock_unpoisoned(&state_for_get)
                .themes
                .get(name.as_str())
                .cloned()
                .ok_or_else(|| mlua::Error::runtime(format!("unknown theme: {name}")))?;
            lua.to_value(&raw)
        })?,
    )?;

    let state_for_register = state.clone();
    themes_table.set(
        "register",
        lua.create_function(move |lua, (_this, name, style): (Table, String, Value)| {
            let raw = parse_raw_style(lua, style)?;
            lock_unpoisoned(&state_for_register)
                .themes
                .insert(name, raw);
            Ok(())
        })?,
    )?;

    themes_table.set(
        "remove",
        lua.create_function(move |_lua, (_this, name): (Table, String)| {
            let mut guard = lock_unpoisoned(&state);
            if name == "default" {
                return Err(mlua::Error::runtime(
                    "themes.remove: cannot remove 'default'",
                ));
            }
            if guard.themes.remove(name.as_str()).is_none() {
                return Err(mlua::Error::runtime(format!(
                    "themes.remove: unknown theme: {name}"
                )));
            }
            if guard.active_theme == name {
                guard.active_theme = "default".to_string();
            }
            Ok(())
        })?,
    )?;

    Ok(themes_table)
}

/// Build one role-specific import function for the `hotki` table.
fn import_function(
    lua: &Lua,
    state: Arc<Mutex<RuntimeState>>,
    role: ImportRole,
) -> LuaResult<Function> {
    lua.create_function(move |lua, path: String| import_value(lua, &state, role, &path))
}

/// Load, cache, and convert one imported Luau module value.
fn import_value(
    lua: &Lua,
    state: &Arc<Mutex<RuntimeState>>,
    role: ImportRole,
    path: &str,
) -> LuaResult<Value> {
    let resolved = resolve_import_path(&lock_unpoisoned(state), path)?;
    let cache_key = (role, resolved.clone());
    if let Some(value) = lock_unpoisoned(state).imports.get(&cache_key).cloned() {
        return imported_value_to_lua(lua, value);
    }

    let source = fs::read_to_string(&resolved).map_err(mlua::Error::external)?;
    lock_unpoisoned(&lock_unpoisoned(state).sources)
        .insert(resolved.clone(), Arc::from(source.clone().into_boxed_str()));

    let value = lua
        .load(source.as_str())
        .set_name(resolved.to_string_lossy().as_ref())
        .eval::<Value>()?;
    let imported = parse_imported_value(lua, role, value)?;
    lock_unpoisoned(state)
        .imports
        .insert(cache_key, imported.clone());
    imported_value_to_lua(lua, imported)
}

/// Validate the return value of one imported module against its declared role.
fn parse_imported_value(lua: &Lua, role: ImportRole, value: Value) -> LuaResult<ImportedValue> {
    match role {
        ImportRole::Mode => {
            let Value::Function(func) = value else {
                return Err(mlua::Error::runtime("import_mode must return a function"));
            };
            Ok(ImportedValue::Mode(ModeRef::from_function(func, None)))
        }
        ImportRole::Items => match value {
            Value::Function(func) => Ok(ImportedValue::Items(func)),
            Value::Table(table) => {
                let func = lua.create_function(move |_lua, ()| Ok(table.clone()))?;
                Ok(ImportedValue::Items(func))
            }
            other => Err(mlua::Error::runtime(format!(
                "import_items must return a function or array table, got {}",
                other.type_name()
            ))),
        },
        ImportRole::Handler => {
            let Value::Function(func) = value else {
                return Err(mlua::Error::runtime(
                    "import_handler must return a function",
                ));
            };
            Ok(ImportedValue::Handler(HandlerRef { func }))
        }
        ImportRole::Style => Ok(ImportedValue::Style(Box::new(StyleOverlay {
            raw: parse_raw_style(lua, value)?,
        }))),
    }
}

/// Convert a cached imported value back into a Luau value.
fn imported_value_to_lua(lua: &Lua, imported: ImportedValue) -> LuaResult<Value> {
    match imported {
        ImportedValue::Mode(mode) => Ok(Value::Function(mode.func)),
        ImportedValue::Items(func) => Ok(Value::Function(func)),
        ImportedValue::Handler(handler) => Ok(Value::Function(handler.func)),
        ImportedValue::Style(style) => lua.to_value(&style.raw),
    }
}

/// Parse a selector configuration record from Luau.
fn parse_selector_config(_lua: &Lua, value: Value) -> LuaResult<SelectorConfig> {
    let Value::Table(table) = value else {
        return Err(mlua::Error::runtime("action.selector expects a table"));
    };

    let items_value: Value = table
        .get("items")
        .map_err(|_| mlua::Error::runtime("selector: missing required field 'items'"))?;
    let items = match items_value {
        Value::Function(func) => SelectorItems::Provider(func),
        other => SelectorItems::Static(
            selector::parse_selector_items(other).map_err(mlua::Error::runtime)?,
        ),
    };

    let on_select: Function = table
        .get("on_select")
        .map_err(|_| mlua::Error::runtime("selector: missing required field 'on_select'"))?;
    let on_cancel = table
        .get::<Option<Function>>("on_cancel")?
        .map(|func| HandlerRef { func });

    Ok(SelectorConfig {
        title: table
            .get::<Option<String>>("title")?
            .unwrap_or_else(|| "Select".to_string()),
        placeholder: table
            .get::<Option<String>>("placeholder")?
            .unwrap_or_default(),
        items,
        on_select: HandlerRef { func: on_select },
        on_cancel,
        max_visible: table.get::<Option<usize>>("max_visible")?.unwrap_or(10),
    })
}

/// Build one primitive-action, handler, or selector binding from Luau inputs.
fn binding_from_action(
    lua: &Lua,
    chord: &str,
    desc: String,
    action_value: ActionValue,
    options: Option<BindingOptionsSpec>,
) -> LuaResult<Binding> {
    let chord = parse_chord(chord)?;
    let pos = current_source_pos(lua);
    let mut binding = Binding {
        chord,
        desc,
        kind: match action_value.payload {
            ActionPayload::Action(action) => BindingKind::Action(action),
            ActionPayload::Handler(handler) => BindingKind::Handler(handler),
            ActionPayload::Selector(selector) => BindingKind::Selector(selector),
        },
        mode_id: None,
        flags: BindingFlags::default(),
        style: None,
        mode_capture: false,
        pos,
    };
    apply_binding_options(&mut binding, options);
    Ok(binding)
}

/// Build one submenu binding from Luau inputs.
fn binding_from_mode(
    lua: &Lua,
    chord: &str,
    title: String,
    render: Function,
    options: Option<SubmenuOptionsSpec>,
) -> LuaResult<Binding> {
    let chord = parse_chord(chord)?;
    let mode = ModeRef::from_function(render, Some(title.clone()));
    let pos = current_source_pos(lua);
    let mut binding = Binding {
        chord,
        desc: title,
        mode_id: Some(mode.id()),
        kind: BindingKind::Mode(mode),
        flags: BindingFlags::default(),
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

/// Apply parsed Luau binding options to a binding.
fn apply_binding_options(binding: &mut Binding, options: Option<BindingOptionsSpec>) {
    let Some(options) = options else {
        return;
    };

    binding.flags.hidden = options.hidden.unwrap_or(false);
    binding.flags.global = options.global.unwrap_or(false);
    binding.flags.stay = options.stay.unwrap_or(false);
    binding.flags.repeat = options.repeat.map(|repeat| RepeatSpec {
        delay_ms: repeat.delay_ms,
        interval_ms: repeat.interval_ms,
    });
    binding.style = options.style.map(BindingStyleSpec::into_binding_style);
}

/// Merge a series of mode-level style overlays into one overlay.
fn merge_style_overlays(overlays: &[raw::RawStyle]) -> Option<raw::RawStyle> {
    let mut iter = overlays.iter();
    let first = iter.next()?.clone();
    Some(iter.fold(first, |acc, overlay| acc.merge(overlay)))
}

/// Deserialize an optional Luau record, treating `nil` as `None`.
fn parse_optional<T>(lua: &Lua, value: Value) -> LuaResult<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    if matches!(value, Value::Nil) {
        return Ok(None);
    }
    lua.from_value(value).map(Some)
}

/// Deserialize a Luau style overlay table into raw config types.
fn parse_raw_style(lua: &Lua, value: Value) -> LuaResult<raw::RawStyle> {
    lua.from_value(value)
}

/// Parse a hotkey chord string into a normalized `Chord`.
fn parse_chord(spec: &str) -> LuaResult<Chord> {
    Chord::parse(spec).ok_or_else(|| mlua::Error::runtime(format!("invalid chord string: {spec}")))
}

/// Wrap an action payload as Luau userdata.
fn action_userdata(lua: &Lua, payload: ActionPayload) -> LuaResult<AnyUserData> {
    lua.create_userdata(ActionValue { payload })
}

/// Install a primitive action constant into the `action` table.
fn set_action_const(lua: &Lua, table: &Table, key: &str, action: Action) -> LuaResult<()> {
    table.set(key, action_userdata(lua, ActionPayload::Action(action))?)
}

/// Render a display name for an optional source path.
fn path_display(path: Option<&Path>) -> String {
    path.map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "<memory>".to_string())
}

/// Capture the current Luau stack position for binding diagnostics.
fn current_source_pos(lua: &Lua) -> Option<SourcePos> {
    lua.inspect_stack(1, |debug| {
        let source = debug.source();
        let path = source
            .source
            .as_ref()
            .map(|value| value.to_string())
            .filter(|value| value != "<memory>")
            .map(PathBuf::from);
        SourcePos {
            path,
            line: debug.current_line(),
            col: Some(1),
        }
    })
}

/// Resolve a role import path within the current config directory.
fn resolve_import_path(state: &RuntimeState, raw_path: &str) -> LuaResult<PathBuf> {
    let root = state
        .config_dir
        .as_ref()
        .ok_or_else(|| mlua::Error::runtime("imports require a filesystem-backed config"))?;

    let path = Path::new(raw_path);
    if path.is_absolute() {
        return Err(mlua::Error::runtime(
            "absolute import paths are not allowed",
        ));
    }

    for component in path.components() {
        if matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        ) {
            return Err(mlua::Error::runtime(
                "parent traversal is not allowed in imports",
            ));
        }
    }

    let candidate = if path.extension().is_some() {
        root.join(path)
    } else {
        root.join(path).with_extension("luau")
    };
    let root_canon = fs::canonicalize(root).unwrap_or_else(|_| root.clone());
    let canon = fs::canonicalize(&candidate).map_err(mlua::Error::external)?;
    if !canon.starts_with(&root_canon) {
        return Err(mlua::Error::runtime("import escapes the config directory"));
    }
    Ok(canon)
}

/// Convert selector items into a Luau array table.
fn selector_items_table(lua: &Lua, items: &[super::SelectorItem]) -> LuaResult<Table> {
    let out = lua.create_table()?;
    for (index, item) in items.iter().enumerate() {
        let table = lua.create_table()?;
        table.set("label", item.label.clone())?;
        table.set("sublabel", item.sublabel.clone())?;
        table.set("data", item.data.clone())?;
        out.set(index + 1, table)?;
    }
    Ok(out)
}

/// Evaluate one regex match helper for mode and action contexts.
fn regex_matches(text: &str, pattern: &str) -> LuaResult<bool> {
    Regex::new(pattern)
        .map(|regex| regex.is_match(text))
        .map_err(|err| mlua::Error::runtime(err.to_string()))
}

/// Install a hard interrupt limit for Luau evaluation.
fn install_interrupt_limit(lua: &Lua) -> Arc<AtomicU64> {
    let steps = Arc::new(AtomicU64::new(0));
    let interrupt_steps = steps.clone();
    lua.set_interrupt(move |_lua| {
        let next = steps.fetch_add(1, Ordering::Relaxed) + 1;
        if next > EXECUTION_LIMIT {
            return Err(mlua::Error::runtime(
                "script exceeded the execution limit during evaluation",
            ));
        }
        Ok(VmState::Continue)
    });
    interrupt_steps
}

/// Reset the shared interrupt counter before a fresh Luau entrypoint call.
fn reset_execution_budget(steps: &AtomicU64) {
    steps.store(0, Ordering::Relaxed);
}

/// Convert an `mlua` error into a source-located config error.
fn load_error_from_mlua(source: &str, path: Option<&Path>, err: &mlua::Error) -> Error {
    let mut path_buf = path.map(Path::to_path_buf);
    let mut line = None;
    let mut col = None;

    if let Some((parsed_path, parsed_line, parsed_col)) = super::render::parse_error_location(err) {
        if parsed_path
            .as_deref()
            .is_some_and(|value| value != Path::new("<memory>"))
        {
            path_buf = parsed_path;
        }
        line = parsed_line;
        col = parsed_col;
    }

    let excerpt = line.map(|line| excerpt_at(source, line, col.unwrap_or(1)));

    match err {
        mlua::Error::SyntaxError { message, .. } => Error::Parse {
            path: path_buf,
            line: line.unwrap_or(1),
            col: col.unwrap_or(1),
            message: message.clone(),
            excerpt: excerpt.unwrap_or_else(|| excerpt_at(source, 1, 1)),
        },
        _ => Error::Validation {
            path: path_buf,
            line,
            col,
            message: err.to_string(),
            excerpt,
        },
    }
}
