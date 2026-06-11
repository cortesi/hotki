use std::{
    collections::HashMap,
    ffi::OsStr,
    fmt, fs, mem,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use mac_keycode::Chord;
use oxau::{
    compile::{self, CompileError, CompileOptions},
    embed::{
        FromLua, Function, HostType, HostTypeBuilder, IntoLuaMulti, ModuleBinding, ModuleBuilder,
        ModuleBuilderExt, ModuleValue, MultiValue, NativeModule, RuntimeError, Scope,
        ScopedHostFunction, ScopedValue, ScriptError, StashedClosure, Table, Userdata,
        serde::{from_scoped_value, to_scoped_value},
    },
    profile::Profile,
    session::{Ambient, ProtectedScriptError, TracebackFrame, Vm},
};
use regex::Regex;
use serde::Deserialize;

use super::{
    ActionCtx, Binding, BindingFlags, BindingKind, DynamicConfig, HandlerRef, ModeCtx, ModeRef,
    NavRequest, RepeatSpec, SelectorConfig, SourcePos, StyleOverlay, apps,
    binding_style::BindingStyleSpec,
    imports::{self, ImportRole},
    selector,
    util::lock_unpoisoned,
};
use crate::{Action, Error, NotifyKind, Toggle, error::excerpt_at, raw, themes};

/// Shared mutable state captured by native host functions installed into one VM.
type SharedRuntimeState = Arc<Mutex<RuntimeState>>;

/// Checked-in Luau declaration source for the script host API.
const HOTKI_DECLARATION: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/luau/hotki.d.luau"));

/// Tag used to distinguish primitive action constants from other light userdata.
const ACTION_TOKEN_TAG: u8 = 0x48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
/// Primitive `action.*` constants exposed as light userdata.
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

#[derive(Clone, Debug)]
/// Cached imported values stored in loader state.
enum ImportedValue {
    /// Imported mode renderer.
    Mode(ModeRef),
    /// Imported selector item provider or static list.
    Items(ImportedItems),
    /// Imported action handler.
    Handler(HandlerRef),
    /// Imported style overlay.
    Style(Box<raw::RawStyle>),
}

#[derive(Clone, Debug)]
/// Imported selector item values.
enum ImportedItems {
    /// Imported item provider closure.
    Provider(StashedClosure),
    /// Imported static selector item list.
    Static(Vec<super::SelectorItem>),
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
    let source_key = path.clone().unwrap_or_else(|| PathBuf::from("<memory>"));
    lock_unpoisoned(&sources).insert(source_key, Arc::from(source.to_string().into_boxed_str()));

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
    let profile = Profile::full().with_runtime_compilation();
    let chunk = compile::compile_for(
        &profile,
        source.as_bytes(),
        &CompileOptions::for_vm_execution(),
    )
    .map_err(|err| compile_error_to_config(source, &err, path.as_deref()))?;
    let mut vm = build_vm(profile, state.clone(), path.as_deref())?;
    let chunk_name = chunk_name(path.as_deref());
    let module = vm
        .load_named(&chunk, chunk_name.as_bytes())
        .map_err(|err| validation_error(path.clone(), err))?;

    match vm
        .call_protected_with_limits(&module, DynamicConfig::entry_limits())
        .map_err(|err| validation_error(path.clone(), format!("{err:?}")))?
    {
        Ok(_) => {}
        Err(err) => {
            return Err(protected_error_to_config(
                source,
                path.as_deref(),
                &sources,
                &err,
            ));
        }
    }

    let root = lock_unpoisoned(&state).root.clone().ok_or_else(|| {
        validation_error(path.clone(), "hotki.root() must be called exactly once")
    })?;
    validate_root(&mut vm, &root, path.as_deref(), &sources)?;

    let state_guard = lock_unpoisoned(&state);
    Ok(DynamicConfig {
        root,
        themes: state_guard.themes.clone(),
        active_theme: state_guard.active_theme.clone(),
        vm,
        _root_module: module,
        path,
        sources,
    })
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

/// Build the sandboxed retained VM used by a dynamic config.
fn build_vm(profile: Profile, state: SharedRuntimeState, path: Option<&Path>) -> Result<Vm, Error> {
    Vm::builder()
        .ambient(Ambient::deterministic(0))
        .limits(DynamicConfig::entry_limits())
        .profile(profile)
        .module(Arc::new(HotkiModule {
            state: state.clone(),
        }))
        .module(Arc::new(ActionModule))
        .module(Arc::new(ThemesModule { state }))
        .host_type(mode_builder_type())
        .host_type(action_value_type())
        .host_type(mode_context_type())
        .host_type(action_context_type())
        .build_sandboxed()
        .map_err(|err| validation_error(path.map(Path::to_path_buf), err))
}

/// Invoke the configured root mode once to validate its output shape.
fn validate_root(
    vm: &mut Vm,
    root: &ModeRef,
    path: Option<&Path>,
    sources: &super::config::SourceMap,
) -> Result<(), Error> {
    let builder = ModeBuilder::new_for_render(None, false);
    let ctx = ModeCtx {
        app: String::new(),
        title: String::new(),
        pid: 0,
        hud: false,
        depth: 0,
    };
    let mut script_error = None;
    vm.step_with_limits(DynamicConfig::entry_limits(), |scope| {
        let builder = mode_builder_userdata(scope, builder.clone())?;
        let ctx = mode_context_userdata(scope, ctx.clone())?;
        let root = scope.fetch_function(&root.func)?;
        let result: Result<(), ScriptError<'_>> = scope.call_protected(root, (builder, ctx))?;
        if let Err(err) = result {
            script_error = Some(super::render::script_error_to_config(
                path, sources, scope, &err,
            ));
        }
        Ok(())
    })
    .map_err(|err| validation_error(path.map(Path::to_path_buf), err.message()))?;

    if let Some(err) = script_error {
        return Err(err);
    }
    Ok(())
}

/// Native module backing the `hotki` global library.
struct HotkiModule {
    /// Shared loader state mutated by root, application, and import functions.
    state: SharedRuntimeState,
}

impl NativeModule for HotkiModule {
    fn name(&self) -> &str {
        "hotki"
    }

    fn declaration(&self) -> &str {
        HOTKI_DECLARATION
    }

    fn build(&self, builder: &mut dyn ModuleBuilder) {
        let binding = ModuleBinding::library("hotki");
        builder.scoped_function(
            "root",
            binding.clone(),
            Box::new(HotkiRoot {
                state: self.state.clone(),
            }),
        );
        builder.scoped_function(
            "applications",
            binding.clone(),
            Box::new(HotkiApplications {
                state: self.state.clone(),
            }),
        );
        for role in ImportRole::ALL {
            builder.scoped_function(
                role.function_name(),
                binding.clone(),
                Box::new(ImportFunction {
                    state: self.state.clone(),
                    role,
                }),
            );
        }
    }
}

/// Native module backing the `action` global library.
struct ActionModule;

impl NativeModule for ActionModule {
    fn name(&self) -> &str {
        "action"
    }

    fn declaration(&self) -> &str {
        HOTKI_DECLARATION
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

/// Native module backing the `themes` global library.
struct ThemesModule {
    /// Shared loader state containing the theme registry and active selection.
    state: SharedRuntimeState,
}

impl NativeModule for ThemesModule {
    fn name(&self) -> &str {
        "themes"
    }

    fn declaration(&self) -> &str {
        HOTKI_DECLARATION
    }

    fn build(&self, builder: &mut dyn ModuleBuilder) {
        let binding = ModuleBinding::library("themes");
        for kind in [
            ThemesFunctionKind::Use,
            ThemesFunctionKind::Current,
            ThemesFunctionKind::List,
            ThemesFunctionKind::Get,
            ThemesFunctionKind::Register,
            ThemesFunctionKind::Remove,
        ] {
            builder.scoped_function(
                kind.name(),
                binding.clone(),
                Box::new(ThemesFunction {
                    state: self.state.clone(),
                    kind,
                }),
            );
        }
    }
}

/// Host implementation of `hotki.root`.
struct HotkiRoot {
    /// Shared loader state where the root renderer is recorded.
    state: SharedRuntimeState,
}

impl ScopedHostFunction for HotkiRoot {
    fn call<'s>(
        &self,
        scope: &Scope<'s>,
        args: MultiValue<'s>,
    ) -> Result<MultiValue<'s>, RuntimeError> {
        let mut args = args.into_vec().into_iter();
        let render = expect_function(args.next(), "hotki.root render")?;
        expect_no_extra(args.next(), "hotki.root")?;
        let mode = ModeRef::from_function(scope, render, None)?;
        let mut guard = lock_unpoisoned(&self.state);
        if guard.root.is_some() {
            return Err(RuntimeError::runtime(
                "hotki.root() must be called exactly once",
            ));
        }
        guard.root = Some(mode);
        Ok(MultiValue::new())
    }
}

/// Host implementation of `hotki.applications`.
struct HotkiApplications {
    /// Shared loader state containing the applications selector cache.
    state: SharedRuntimeState,
}

impl ScopedHostFunction for HotkiApplications {
    fn call<'s>(
        &self,
        scope: &Scope<'s>,
        _args: MultiValue<'s>,
    ) -> Result<MultiValue<'s>, RuntimeError> {
        let cached = { lock_unpoisoned(&self.state).applications_cache.clone() };
        let items = if let Some(cached) = cached {
            cached
        } else {
            let apps = apps::application_items(scope)?;
            let shared: Arc<[super::SelectorItem]> = apps.into();
            lock_unpoisoned(&self.state).applications_cache = Some(shared.clone());
            shared
        };
        let table = selector_items_table(scope, items.as_ref())?;
        Ok(MultiValue::from_values(vec![ScopedValue::Table(table)]))
    }
}

/// Host implementation shared by the role-specific `hotki.import_*` functions.
struct ImportFunction {
    /// Shared loader state containing the import cache and source map.
    state: SharedRuntimeState,
    /// Role expected from the imported module's return value.
    role: ImportRole,
}

impl ScopedHostFunction for ImportFunction {
    fn call<'s>(
        &self,
        scope: &Scope<'s>,
        args: MultiValue<'s>,
    ) -> Result<MultiValue<'s>, RuntimeError> {
        let mut args = args.into_vec().into_iter();
        let path = expect_string(scope, args.next(), "import path")?;
        expect_no_extra(args.next(), self.role.function_name())?;
        import_value(scope, &self.state, self.role, &path)
    }
}

/// Load, cache, and convert one imported Luau module value.
fn import_value<'s>(
    scope: &Scope<'s>,
    state: &SharedRuntimeState,
    role: ImportRole,
    path: &str,
) -> Result<MultiValue<'s>, RuntimeError> {
    let resolved = resolve_import_path(&lock_unpoisoned(state), path)?;
    let cache_key = (role, resolved.clone());
    if let Some(value) = lock_unpoisoned(state).imports.get(&cache_key).cloned() {
        return imported_value_to_lua(scope, value);
    }

    let source = fs::read_to_string(&resolved).map_err(RuntimeError::external)?;
    let sources = { lock_unpoisoned(state).sources.clone() };
    lock_unpoisoned(&sources).insert(resolved.clone(), Arc::from(source.clone().into_boxed_str()));

    let results = scope.eval_chunk(source.as_bytes(), chunk_name(Some(&resolved)).as_bytes())?;
    let value = single_return(results, role.function_name())?;
    let imported = parse_imported_value(scope, role, value)?;
    lock_unpoisoned(state)
        .imports
        .insert(cache_key, imported.clone());
    imported_value_to_lua(scope, imported)
}

/// Validate the return value of one imported module against its declared role.
fn parse_imported_value<'s>(
    scope: &Scope<'s>,
    role: ImportRole,
    value: ScopedValue<'s>,
) -> Result<ImportedValue, RuntimeError> {
    match role {
        ImportRole::Mode => {
            let func = expect_function(Some(value), "import_mode return value")?;
            Ok(ImportedValue::Mode(ModeRef::from_function(
                scope, func, None,
            )?))
        }
        ImportRole::Items => match value {
            ScopedValue::Function(func) => Ok(ImportedValue::Items(ImportedItems::Provider(
                scope.stash_function(func)?,
            ))),
            ScopedValue::Table(_) => Ok(ImportedValue::Items(ImportedItems::Static(
                selector::parse_selector_items(scope, value)?,
            ))),
            other => Err(RuntimeError::runtime(format!(
                "import_items must return a function or array table, got {}",
                other.type_name()
            ))),
        },
        ImportRole::Handler => {
            let func = expect_function(Some(value), "import_handler return value")?;
            Ok(ImportedValue::Handler(HandlerRef::from_function(
                scope, func,
            )?))
        }
        ImportRole::Style => Ok(ImportedValue::Style(Box::new(parse_raw_style(
            scope, value,
        )?))),
    }
}

/// Convert a cached imported value back into a Luau value.
fn imported_value_to_lua<'s>(
    scope: &Scope<'s>,
    imported: ImportedValue,
) -> Result<MultiValue<'s>, RuntimeError> {
    let value = match imported {
        ImportedValue::Mode(mode) => ScopedValue::Function(scope.fetch_function(&mode.func)?),
        ImportedValue::Items(ImportedItems::Provider(provider)) => {
            ScopedValue::Function(scope.fetch_function(&provider)?)
        }
        ImportedValue::Items(ImportedItems::Static(items)) => {
            ScopedValue::Table(selector_items_table(scope, &items)?)
        }
        ImportedValue::Handler(handler) => {
            ScopedValue::Function(scope.fetch_function(&handler.func)?)
        }
        ImportedValue::Style(style) => to_scoped_value(scope, &*style)?,
    };
    Ok(MultiValue::from_values(vec![value]))
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
        let mut args = args.into_vec().into_iter();
        let payload = match self {
            Self::Shell => {
                let cmd = expect_string(scope, args.next(), "action.shell command")?;
                let opts = parse_optional::<ShellOptionsSpec>(
                    scope,
                    args.next().unwrap_or(ScopedValue::Nil),
                )?;
                expect_no_extra(args.next(), "action.shell")?;
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
                ActionPayload::Action(Action::Shell(spec))
            }
            Self::Open => {
                let target = expect_string(scope, args.next(), "action.open target")?;
                expect_no_extra(args.next(), "action.open")?;
                ActionPayload::Action(Action::Open(target))
            }
            Self::Relay => {
                let spec = expect_string(scope, args.next(), "action.relay spec")?;
                expect_no_extra(args.next(), "action.relay")?;
                ActionPayload::Action(Action::Relay(spec))
            }
            Self::ShowDetails => {
                let toggle =
                    expect_serde::<Toggle>(scope, args.next(), "action.show_details toggle")?;
                expect_no_extra(args.next(), "action.show_details")?;
                ActionPayload::Action(Action::ShowDetails(toggle))
            }
            Self::ThemeSet => {
                let name = expect_string(scope, args.next(), "action.theme_set name")?;
                expect_no_extra(args.next(), "action.theme_set")?;
                ActionPayload::Action(Action::ThemeSet(name))
            }
            Self::SetVolume => {
                let level = expect_lua::<u8>(scope, args.next(), "action.set_volume level")?;
                expect_no_extra(args.next(), "action.set_volume")?;
                ActionPayload::Action(Action::SetVolume(level))
            }
            Self::ChangeVolume => {
                let delta = expect_lua::<i8>(scope, args.next(), "action.change_volume delta")?;
                expect_no_extra(args.next(), "action.change_volume")?;
                ActionPayload::Action(Action::ChangeVolume(delta))
            }
            Self::Mute => {
                let toggle = expect_serde::<Toggle>(scope, args.next(), "action.mute toggle")?;
                expect_no_extra(args.next(), "action.mute")?;
                ActionPayload::Action(Action::Mute(toggle))
            }
            Self::Run => {
                let func = expect_function(args.next(), "action.run handler")?;
                expect_no_extra(args.next(), "action.run")?;
                ActionPayload::Handler(HandlerRef::from_function(scope, func)?)
            }
            Self::Selector => {
                let spec = args
                    .next()
                    .ok_or_else(|| RuntimeError::runtime("action.selector expects a table"))?;
                expect_no_extra(args.next(), "action.selector")?;
                ActionPayload::Selector(selector::parse_selector_config(scope, spec)?)
            }
        };
        action_userdata(scope, payload)
    }
}

/// Host methods exposed on the `themes` global library.
#[derive(Clone, Copy)]
enum ThemesFunctionKind {
    /// Select the active theme.
    Use,
    /// Return the active theme name.
    Current,
    /// List known theme names.
    List,
    /// Return one theme style overlay.
    Get,
    /// Register or replace a script-defined theme.
    Register,
    /// Remove a script-defined theme.
    Remove,
}

impl ThemesFunctionKind {
    /// Return the method name installed in the `themes` library.
    fn name(self) -> &'static str {
        match self {
            Self::Use => "use",
            Self::Current => "current",
            Self::List => "list",
            Self::Get => "get",
            Self::Register => "register",
            Self::Remove => "remove",
        }
    }
}

/// Host implementation shared by the `themes` library methods.
struct ThemesFunction {
    /// Shared loader state containing theme data.
    state: SharedRuntimeState,
    /// Concrete theme operation to execute.
    kind: ThemesFunctionKind,
}

impl ScopedHostFunction for ThemesFunction {
    fn call<'s>(
        &self,
        scope: &Scope<'s>,
        args: MultiValue<'s>,
    ) -> Result<MultiValue<'s>, RuntimeError> {
        let mut args = skip_method_receiver(args);
        match self.kind {
            ThemesFunctionKind::Use => {
                let name = expect_string(scope, args.next(), "themes:use name")?;
                expect_no_extra(args.next(), "themes:use")?;
                let mut guard = lock_unpoisoned(&self.state);
                if !guard.themes.contains_key(name.as_str()) {
                    return Err(RuntimeError::runtime(format!("unknown theme: {name}")));
                }
                guard.active_theme = name;
                Ok(MultiValue::new())
            }
            ThemesFunctionKind::Current => {
                expect_no_extra(args.next(), "themes:current")?;
                lock_unpoisoned(&self.state)
                    .active_theme
                    .clone()
                    .into_lua_multi(scope)
            }
            ThemesFunctionKind::List => {
                expect_no_extra(args.next(), "themes:list")?;
                let mut names = lock_unpoisoned(&self.state)
                    .themes
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>();
                names.sort();
                names.into_lua_multi(scope)
            }
            ThemesFunctionKind::Get => {
                let name = expect_string(scope, args.next(), "themes:get name")?;
                expect_no_extra(args.next(), "themes:get")?;
                let raw = lock_unpoisoned(&self.state)
                    .themes
                    .get(name.as_str())
                    .cloned()
                    .ok_or_else(|| RuntimeError::runtime(format!("unknown theme: {name}")))?;
                Ok(MultiValue::from_values(vec![to_scoped_value(scope, &raw)?]))
            }
            ThemesFunctionKind::Register => {
                let name = expect_string(scope, args.next(), "themes:register name")?;
                let style = args
                    .next()
                    .ok_or_else(|| RuntimeError::runtime("themes:register expects a style"))?;
                expect_no_extra(args.next(), "themes:register")?;
                let raw = parse_raw_style(scope, style)?;
                lock_unpoisoned(&self.state).themes.insert(name, raw);
                Ok(MultiValue::new())
            }
            ThemesFunctionKind::Remove => {
                let name = expect_string(scope, args.next(), "themes:remove name")?;
                expect_no_extra(args.next(), "themes:remove")?;
                let mut guard = lock_unpoisoned(&self.state);
                if name == "default" {
                    return Err(RuntimeError::runtime(
                        "themes.remove: cannot remove 'default'",
                    ));
                }
                if guard.themes.remove(name.as_str()).is_none() {
                    return Err(RuntimeError::runtime(format!(
                        "themes.remove: unknown theme: {name}"
                    )));
                }
                if guard.active_theme == name {
                    guard.active_theme = "default".to_string();
                }
                Ok(MultiValue::new())
            }
        }
    }
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
    let mut binding = Binding {
        chord,
        desc,
        kind: match action_value {
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
fn parse_optional<'s, T>(
    scope: &Scope<'s>,
    value: ScopedValue<'s>,
) -> Result<Option<T>, RuntimeError>
where
    T: for<'de> Deserialize<'de>,
{
    if matches!(value, ScopedValue::Nil) {
        return Ok(None);
    }
    from_scoped_value(scope, value)
        .map(Some)
        .map_err(|err| RuntimeError::runtime(err.message()))
}

/// Deserialize a Luau style overlay table into raw config types.
fn parse_raw_style<'s>(
    scope: &Scope<'s>,
    value: ScopedValue<'s>,
) -> Result<raw::RawStyle, RuntimeError> {
    from_scoped_value(scope, value).map_err(|err| RuntimeError::runtime(err.message()))
}

/// Parse a hotkey chord string into a normalized `Chord`.
fn parse_chord(spec: &str) -> Result<Chord, RuntimeError> {
    Chord::parse(spec).ok_or_else(|| RuntimeError::runtime(format!("invalid chord string: {spec}")))
}

/// Capture the current Luau stack position for binding diagnostics.
fn current_source_pos(scope: &Scope<'_>) -> Option<SourcePos> {
    scope.caller_location(0).map(SourcePos::from_location)
}

/// Resolve a role import path within the current config directory.
fn resolve_import_path(state: &RuntimeState, raw_path: &str) -> Result<PathBuf, RuntimeError> {
    let root = state
        .config_dir
        .as_ref()
        .ok_or_else(|| RuntimeError::runtime("imports require a filesystem-backed config"))?;

    imports::resolve_path(root, raw_path).map_err(|err| err.into_runtime_error())
}

/// Convert selector items into a Luau array table.
fn selector_items_table<'s>(
    scope: &Scope<'s>,
    items: &[super::SelectorItem],
) -> Result<Table<'s>, RuntimeError> {
    let out = scope.create_table()?;
    for (index, item) in items.iter().enumerate() {
        let table = scope.create_table()?;
        table.set(scope, "label", item.label.clone())?;
        table.set(scope, "sublabel", item.sublabel.clone())?;
        table.set(scope, "data", item.data.fetch(scope)?)?;
        out.set(scope, (index + 1) as f64, table)?;
    }
    Ok(out)
}

/// Evaluate one regex match helper for mode and action contexts.
fn regex_matches(text: &str, pattern: &str) -> Result<bool, RuntimeError> {
    Regex::new(pattern)
        .map(|regex| regex.is_match(text))
        .map_err(|err| RuntimeError::runtime(err.to_string()))
}

/// Render a display name for an optional source path.
fn chunk_name(path: Option<&Path>) -> String {
    path.map(|path| format!("@{}", path.display()))
        .unwrap_or_else(|| "=<memory>".to_string())
}

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

/// Convert a structured oxau compile error into a config error.
fn compile_error_to_config(source: &str, err: &CompileError, path: Option<&Path>) -> Error {
    let Some(location) = err.location() else {
        return validation_error(path.map(Path::to_path_buf), err.message());
    };

    let line = location.begin.line as usize + 1;
    let col = location.begin.column as usize + 1;
    Error::Parse {
        path: path.map(Path::to_path_buf),
        line,
        col,
        message: err.message().to_string(),
        excerpt: excerpt_at(source, line, col),
    }
}

/// Convert a VM-level protected script failure into a located config error.
fn protected_error_to_config(
    source: &str,
    default_path: Option<&Path>,
    sources: &super::config::SourceMap,
    err: &ProtectedScriptError,
) -> Error {
    let message = err
        .traceback()
        .and_then(|traceback| traceback.lines().next())
        .unwrap_or("script raised an error")
        .to_string();
    let (path, line, col) = err
        .frames()
        .iter()
        .find_map(frame_location)
        .map(|(path, line)| {
            (
                normalize_error_path(path, default_path.map(Path::to_path_buf)),
                Some(line),
                Some(1),
            )
        })
        .unwrap_or((default_path.map(Path::to_path_buf), None, None));
    let excerpt =
        line.and_then(|line| error_excerpt(source, sources, path.as_ref(), line, col.unwrap_or(1)));

    Error::Validation {
        path,
        line,
        col,
        message,
        excerpt,
    }
}

/// Extract a chunk name and line from one protected-error traceback frame.
fn frame_location(frame: &TracebackFrame) -> Option<(String, usize)> {
    frame
        .line
        .map(|line| (frame.chunk_name.clone(), line as usize))
}

/// Convert VM traceback chunk names into user-facing filesystem paths.
fn normalize_error_path(path: String, default_path: Option<PathBuf>) -> Option<PathBuf> {
    match path.as_str() {
        "<memory>" => None,
        value if value.starts_with("[string ") => default_path,
        _ => Some(PathBuf::from(path)),
    }
}

/// Render an excerpt from the best available source map entry.
fn error_excerpt(
    source: &str,
    sources: &super::config::SourceMap,
    path: Option<&PathBuf>,
    line: usize,
    col: usize,
) -> Option<String> {
    match path {
        Some(path) => lock_unpoisoned(sources)
            .get(path)
            .map(|source| excerpt_at(source.as_ref(), line, col)),
        None => Some(excerpt_at(source, line, col)),
    }
}

/// Build the host userdata type definition for mode builders.
fn mode_builder_type() -> HostType {
    HostTypeBuilder::<ModeBuilder>::new("ModeBuilder")
        .method_raw("bind", mode_builder_bind)
        .method_raw("bind_many", mode_builder_bind_many)
        .method_raw("submenu", mode_builder_submenu)
        .method_raw("style", mode_builder_style)
        .method_raw("capture", mode_builder_capture)
        .declaration("declare class ModeBuilder\nend\n")
        .build()
}

/// Build the host userdata type definition for action values.
fn action_value_type() -> HostType {
    HostTypeBuilder::<ActionValue>::new("ActionValue")
        .declaration("declare class ActionValue\nend\n")
        .build()
}

/// Build the host userdata type definition for mode render contexts.
fn mode_context_type() -> HostType {
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
fn action_context_type() -> HostType {
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

/// Implement `menu:bind`.
fn mode_builder_bind<'s>(
    scope: &Scope<'s>,
    receiver: Userdata<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut values = args.into_vec().into_iter();
    let chord = expect_string(scope, values.next(), "menu:bind chord")?;
    let desc = expect_string(scope, values.next(), "menu:bind desc")?;
    let action = values
        .next()
        .ok_or_else(|| RuntimeError::runtime("menu:bind expects an action"))?;
    let opts = values.next().unwrap_or(ScopedValue::Nil);
    expect_no_extra(values.next(), "menu:bind")?;

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
    let mut values = args.into_vec().into_iter();
    let table = expect_table(values.next(), "menu:bind_many entries")?;
    expect_no_extra(values.next(), "menu:bind_many")?;

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
    let mut values = args.into_vec().into_iter();
    let chord = expect_string(scope, values.next(), "menu:submenu chord")?;
    let title = expect_string(scope, values.next(), "menu:submenu title")?;
    let render = expect_function(values.next(), "menu:submenu render")?;
    let opts = values.next().unwrap_or(ScopedValue::Nil);
    expect_no_extra(values.next(), "menu:submenu")?;
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
    let mut values = args.into_vec().into_iter();
    let overlay = values
        .next()
        .ok_or_else(|| RuntimeError::runtime("menu:style expects a style overlay"))?;
    expect_no_extra(values.next(), "menu:style")?;
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
    expect_no_extra(args.into_vec().into_iter().next(), "menu:capture")?;
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
    let mut values = args.into_vec().into_iter();
    let kind = expect_serde::<NotifyKind>(scope, values.next(), "ctx:notify kind")?;
    let title = expect_string(scope, values.next(), "ctx:notify title")?;
    let body = expect_string(scope, values.next(), "ctx:notify body")?;
    expect_no_extra(values.next(), "ctx:notify")?;
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
    let mut values = args.into_vec().into_iter();
    let value = values
        .next()
        .ok_or_else(|| RuntimeError::runtime("ctx:exec expects an action"))?;
    expect_no_extra(values.next(), "ctx:exec")?;
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
    let mut values = args.into_vec().into_iter();
    let render = expect_function(values.next(), "ctx:push render")?;
    let title = match values.next().unwrap_or(ScopedValue::Nil) {
        ScopedValue::Nil => None,
        value => Some(String::from_lua(value, scope)?),
    };
    expect_no_extra(values.next(), "ctx:push")?;
    let mode = ModeRef::from_function(scope, render, title.clone())?;
    receiver
        .borrow::<ActionContextUserData>(scope)?
        .0
        .set_nav(NavRequest::Push { mode, title });
    Ok(MultiValue::new())
}

/// Wrap an action payload as Luau userdata.
fn action_userdata<'s>(
    scope: &Scope<'s>,
    payload: ActionPayload,
) -> Result<MultiValue<'s>, RuntimeError> {
    let userdata = scope.create_userdata(ActionValue { payload })?;
    Ok(MultiValue::from_values(vec![ScopedValue::Userdata(
        userdata,
    )]))
}

/// Decode any Luau action value into its Rust payload.
fn action_payload_from_value<'s>(
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
fn primitive_action_from_value<'s>(
    scope: &Scope<'s>,
    value: ScopedValue<'s>,
) -> Result<Action, RuntimeError> {
    match action_payload_from_value(scope, value)? {
        ActionPayload::Action(action) => Ok(action),
        _ => Err(RuntimeError::runtime("ctx:exec expects a primitive action")),
    }
}

/// Drop the explicit receiver supplied to a colon-call host method.
fn skip_method_receiver<'s>(args: MultiValue<'s>) -> impl Iterator<Item = ScopedValue<'s>> {
    let mut values = args.into_vec().into_iter();
    let _receiver = values.next();
    values
}

/// Require exactly one returned Luau value.
fn single_return<'s>(
    values: MultiValue<'s>,
    context: &str,
) -> Result<ScopedValue<'s>, RuntimeError> {
    let mut values = values.into_vec().into_iter();
    let value = values.next().unwrap_or(ScopedValue::Nil);
    expect_no_extra(values.next(), context)?;
    Ok(value)
}

/// Reject unexpected trailing arguments.
fn expect_no_extra(value: Option<ScopedValue<'_>>, context: &str) -> Result<(), RuntimeError> {
    if value.is_some() {
        return Err(RuntimeError::runtime(format!(
            "{context} got too many arguments"
        )));
    }
    Ok(())
}

/// Decode a required string argument.
fn expect_string<'s>(
    scope: &Scope<'s>,
    value: Option<ScopedValue<'s>>,
    context: &str,
) -> Result<String, RuntimeError> {
    let value = value.ok_or_else(|| RuntimeError::runtime(format!("{context} is required")))?;
    String::from_lua(value, scope)
}

/// Decode a required argument through oxau's `FromLua` bridge.
fn expect_lua<'s, T>(
    scope: &Scope<'s>,
    value: Option<ScopedValue<'s>>,
    context: &str,
) -> Result<T, RuntimeError>
where
    T: FromLua<'s>,
{
    let value = value.ok_or_else(|| RuntimeError::runtime(format!("{context} is required")))?;
    T::from_lua(value, scope)
}

/// Decode a required argument through the serde bridge.
fn expect_serde<'s, T>(
    scope: &Scope<'s>,
    value: Option<ScopedValue<'s>>,
    context: &str,
) -> Result<T, RuntimeError>
where
    T: for<'de> Deserialize<'de>,
{
    let value = value.ok_or_else(|| RuntimeError::runtime(format!("{context} is required")))?;
    from_scoped_value(scope, value).map_err(|err| RuntimeError::runtime(err.message()))
}

/// Decode a required function argument.
fn expect_function<'s>(
    value: Option<ScopedValue<'s>>,
    context: &str,
) -> Result<Function<'s>, RuntimeError> {
    match value {
        Some(ScopedValue::Function(func)) => Ok(func),
        Some(other) => Err(RuntimeError::runtime(format!(
            "{context} must be a function, got {}",
            other.type_name()
        ))),
        None => Err(RuntimeError::runtime(format!("{context} is required"))),
    }
}

/// Decode a required table argument.
fn expect_table<'s>(
    value: Option<ScopedValue<'s>>,
    context: &str,
) -> Result<Table<'s>, RuntimeError> {
    match value {
        Some(ScopedValue::Table(table)) => Ok(table),
        Some(other) => Err(RuntimeError::runtime(format!(
            "{context} must be a table, got {}",
            other.type_name()
        ))),
        None => Err(RuntimeError::runtime(format!("{context} is required"))),
    }
}
