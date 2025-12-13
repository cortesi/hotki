//! Rhai-based user configuration loader and DSL bindings.

use std::{
    collections::BTreeMap,
    env,
    error::Error as StdError,
    fmt, fs, mem,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use mac_keycode::Chord;
use rhai::{
    AST, Dynamic, Engine, EvalAltResult, FnPtr, Map, Module, ModuleResolver, NativeCallContext,
    Position, Scope, module_resolvers::FileModuleResolver, serde::from_dynamic,
};
use tracing::{debug, info};

use crate::{
    Action, Config, Cursor, Error, FontWeight, Keys, KeysAttrs, Mode, NotifyKind, NotifyPos, Pos,
    ServerTunables, Toggle, error::excerpt_at, raw, themes,
};

#[derive(Clone)]
/// A Rhai-exposed handle for building a mode's bindings.
struct DslMode {
    /// Shared mutable state containing bindings for this mode.
    state: Arc<Mutex<ModeState>>,
}

#[derive(Clone)]
/// A Rhai-exposed handle for mutating binding attributes.
struct DslBinding {
    /// Shared state for the mode that owns the binding.
    state: Arc<Mutex<ModeState>>,
    /// Index of the binding within the mode.
    index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Indicates whether a binding is a leaf action or a sub-mode.
enum BindingKind {
    /// A leaf binding created via `.bind(...)`.
    Bind,
    /// A mode binding created via `.mode(...)`.
    Mode,
}

#[derive(Debug, Clone)]
/// Internal representation of a Rhai DSL binding entry.
struct BindingEntry {
    /// The chord for the binding.
    chord: Chord,
    /// Description shown in the HUD.
    desc: String,
    /// Action executed for this binding.
    action: Action,
    /// Additional binding attributes.
    attrs: KeysAttrs,
    /// Distinguishes `.bind` from `.mode` for attribute validation.
    kind: BindingKind,
    /// Callsite position where this binding entry was defined.
    pos: Position,
}

#[derive(Debug, Default)]
/// Internal mode state built up by DSL calls.
struct ModeState {
    /// Entries in the order they were defined.
    entries: Vec<BindingEntry>,
}

#[derive(Debug, Default)]
/// Mutable builder state populated while evaluating the config script.
struct BuilderState {
    /// Optional base theme name set by `base_theme(...)`.
    base_theme: Option<String>,
    /// Optional user style overlay set by `style(...)`.
    style: Option<raw::RawStyle>,
    /// Optional server tunables set by `server(...)`.
    server: Option<ServerTunables>,
    /// Root mode state that becomes `Config::keys`.
    root: Arc<Mutex<ModeState>>,
    /// Next stable identifier to assign to a script action.
    next_script_id: u64,
    /// Registry of action callables captured during load, keyed by script action id.
    script_actions: BTreeMap<u64, FnPtr>,
}

/// Result of loading a Rhai config, including an optional runtime for script actions.
pub struct RhaiLoad {
    /// The resolved configuration produced by the script.
    pub(crate) config: Config,
    /// A runtime for executing `Action::Rhai` bindings, when any are present.
    pub(crate) runtime: Option<RhaiRuntime>,
}

/// Load a Rhai config from `path`, producing both the resolved config and an optional runtime.
pub fn load_from_path_with_runtime(path: &Path) -> Result<RhaiLoad, Error> {
    let source = fs::read_to_string(path).map_err(|e| Error::Read {
        path: Some(path.to_path_buf()),
        message: e.to_string(),
    })?;
    load_from_str_with_runtime(&source, Some(path))
}

/// Load a Rhai config from an in-memory `source` string.
pub fn load_from_str_with_runtime(source: &str, path: Option<&Path>) -> Result<RhaiLoad, Error> {
    let builder = Arc::new(Mutex::new(BuilderState {
        root: Arc::new(Mutex::new(ModeState::default())),
        next_script_id: 1,
        ..Default::default()
    }));

    let mut engine = Engine::new();
    configure_engine(&mut engine, path);
    register_dsl(&mut engine, builder.clone());
    register_constants(&mut engine, &builder);

    let mut scope = Scope::new();

    let ast = compile(&engine, source, path)?;
    eval(&engine, &mut scope, &ast, source, path)?;

    let mut builder_guard = lock_unpoisoned(&builder);
    let root = builder_guard.root.clone();
    let entries = lock_unpoisoned(&root).entries.clone();
    validate_mode_chords(&entries).map_err(|err| error_from_rhai(source, &err, path))?;
    let keys = build_keys(&entries);

    let style_base = themes::load_theme(builder_guard.base_theme.as_deref());
    let mut cfg = Config::from_parts(keys, style_base);
    cfg.user_overlay = builder_guard.style.clone();
    if let Some(tunables) = &builder_guard.server {
        cfg.server = tunables.clone();
    }

    let script_actions = mem::take(&mut builder_guard.script_actions);
    let runtime = if script_actions.is_empty() {
        None
    } else {
        // Clear builder state retained by engine-registered functions to avoid holding two copies
        // of the parsed config in memory.
        builder_guard.base_theme = None;
        builder_guard.style = None;
        builder_guard.server = None;
        lock_unpoisoned(&root).entries.clear();

        Some(RhaiRuntime::new(
            engine,
            ast,
            script_actions,
            source.to_string(),
            path.map(Path::to_path_buf),
        ))
    };

    Ok(RhaiLoad {
        config: cfg,
        runtime,
    })
}

/// Configure a Rhai engine for loading Hotki configs and executing script actions.
fn configure_engine(engine: &mut Engine, path: Option<&Path>) {
    engine.on_print(|s| info!(target: "config::rhai", "{}", s));
    engine.on_debug(|s, src, pos| {
        debug!(target: "config::rhai", "{} @ {:?}:{:?}", s, src, pos);
    });

    if let Some(path) = path
        && let Some(dir) = path.parent()
    {
        engine.set_module_resolver(ConfigModuleResolver::new(dir.to_path_buf()));
    }

    // Enforce conservative sandbox limits for both load-time and runtime execution.
    engine.set_max_operations(200_000);
    engine.set_max_call_levels(64);
    engine.set_max_expr_depths(128, 64);
}

#[derive(Debug)]
/// Module resolver for Rhai `import` statements rooted at the config directory.
///
/// This wrapper prevents path traversal (e.g. `..`) and rejects symlinks that resolve outside the
/// configured root, keeping config imports local and predictable.
struct ConfigModuleResolver {
    /// Root directory that contains the config file.
    root: PathBuf,
    /// Canonicalized root directory for robust prefix checks.
    root_canon: PathBuf,
    /// Underlying file-based module resolver (handles caching and compilation).
    inner: FileModuleResolver,
}

impl ConfigModuleResolver {
    /// Create a resolver rooted at `root`.
    fn new(root: PathBuf) -> Self {
        let root_canon = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        Self {
            inner: FileModuleResolver::new_with_path(root.clone()),
            root,
            root_canon,
        }
    }

    /// Validate an import path string, rejecting absolute paths and `..` segments.
    fn validate_import_path(&self, path: &str, pos: Position) -> Result<(), Box<EvalAltResult>> {
        let p = Path::new(path);
        if p.is_absolute() {
            return Err(self.invalid_import_path(
                format!("absolute module paths are not allowed: {}", path),
                pos,
            ));
        }

        for comp in p.components() {
            match comp {
                Component::ParentDir => {
                    return Err(self.invalid_import_path(
                        format!(
                            "parent directory segments ('..') are not allowed in imports: {}",
                            path
                        ),
                        pos,
                    ));
                }
                Component::Prefix(_) | Component::RootDir => {
                    return Err(self.invalid_import_path(
                        format!("invalid module path component in import: {}", path),
                        pos,
                    ));
                }
                Component::CurDir | Component::Normal(_) => {}
            }
        }

        Ok(())
    }

    /// Construct a Rhai runtime error for an invalid import path at `pos`.
    fn invalid_import_path(&self, message: String, pos: Position) -> Box<EvalAltResult> {
        Box::new(EvalAltResult::ErrorRuntime(Dynamic::from(message), pos))
    }

    /// Enforce that a resolved module file path stays within the configured root.
    fn ensure_within_root(
        &self,
        file_path: &Path,
        path: &str,
        pos: Position,
    ) -> Result<(), Box<EvalAltResult>> {
        if !file_path.exists() {
            return Ok(());
        }

        let canon = fs::canonicalize(file_path).map_err(|e| {
            Box::new(EvalAltResult::ErrorRuntime(
                Dynamic::from(e.to_string()),
                pos,
            ))
        })?;

        if canon.starts_with(&self.root_canon) {
            return Ok(());
        }

        Err(self.invalid_import_path(
            format!(
                "imported module escapes config directory: {} (root: {})",
                path,
                self.root.display()
            ),
            pos,
        ))
    }
}

impl ModuleResolver for ConfigModuleResolver {
    fn resolve(
        &self,
        engine: &Engine,
        source: Option<&str>,
        path: &str,
        pos: Position,
    ) -> Result<rhai::Shared<Module>, Box<EvalAltResult>> {
        self.validate_import_path(path, pos)?;
        let file_path = self.inner.get_file_path(path, source.map(Path::new));
        self.ensure_within_root(&file_path, path, pos)?;
        self.inner.resolve(engine, source, path, pos)
    }

    fn resolve_ast(
        &self,
        engine: &Engine,
        source: Option<&str>,
        path: &str,
        pos: Position,
    ) -> Option<Result<AST, Box<EvalAltResult>>> {
        if let Err(err) = self.validate_import_path(path, pos) {
            return Some(Err(err));
        }

        let file_path = self.inner.get_file_path(path, source.map(Path::new));
        if let Err(err) = self.ensure_within_root(&file_path, path, pos) {
            return Some(Err(err));
        }

        self.inner.resolve_ast(engine, source, path, pos)
    }
}

/// Compile `source` into a Rhai AST, converting errors into `config::Error`.
fn compile(engine: &Engine, source: &str, path: Option<&Path>) -> Result<AST, Error> {
    engine.compile(source).map_err(|err| {
        let err: EvalAltResult = err.into();
        error_from_rhai(source, &err, path)
    })
}

/// Evaluate a compiled AST in `scope`, converting errors into `config::Error`.
fn eval(
    engine: &Engine,
    scope: &mut Scope,
    ast: &AST,
    source: &str,
    path: Option<&Path>,
) -> Result<(), Error> {
    engine
        .eval_ast_with_scope::<Dynamic>(scope, ast)
        .map(|_| ())
        .map_err(|err| error_from_rhai(source, &err, path))
}

/// Lock a mutex and recover the guard even if it is poisoned.
fn lock_unpoisoned<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poison) => poison.into_inner(),
    }
}

/// Enforce that chord overloads within a mode are only used alongside match guards.
///
/// The underlying config engine supports multiple entries with the same chord and resolves the
/// first entry whose match predicates accept the focused app/title.
///
/// To keep configs predictable, we reject duplicates unless every overload provides at least one
/// guard (`match_app` or `match_title`).
fn validate_mode_chords(entries: &[BindingEntry]) -> Result<(), Box<EvalAltResult>> {
    let mut by_chord: BTreeMap<String, Vec<&BindingEntry>> = BTreeMap::new();
    for entry in entries {
        by_chord
            .entry(entry.chord.to_string())
            .or_default()
            .push(entry);
    }

    for (chord, group) in by_chord {
        if group.len() <= 1 {
            continue;
        }

        let all_guarded = group.iter().all(|entry| {
            entry.attrs.match_app.as_option().is_some()
                || entry.attrs.match_title.as_option().is_some()
        });
        if all_guarded {
            continue;
        }

        let offender = group
            .into_iter()
            .find(|entry| {
                entry.attrs.match_app.as_option().is_none()
                    && entry.attrs.match_title.as_option().is_none()
            })
            .unwrap_or_else(|| unreachable!("duplicate chord group contains unguarded entries"));

        return Err(boxed_validation_error(
            format!(
                "duplicate chord requires match_app or match_title: {}",
                chord
            ),
            offender.pos,
        ));
    }

    Ok(())
}

/// Convert DSL binding entries into the user-facing `Keys` structure.
fn build_keys(entries: &[BindingEntry]) -> Keys {
    Keys {
        keys: entries
            .iter()
            .cloned()
            .map(|e| (e.chord, e.desc, e.action, e.attrs))
            .collect(),
    }
}

/// Convert a Rhai eval/parse error into a `config::Error` with line/col and an excerpt.
fn error_from_rhai(source: &str, err: &EvalAltResult, path: Option<&Path>) -> Error {
    // Treat "validation" errors emitted by our DSL as Validation; everything else is Parse.
    if let Some((pos, message)) = validation_error_from_rhai(err) {
        let (line, col, excerpt) = match pos_to_line_col(pos) {
            Some((line, col)) => (Some(line), Some(col), Some(excerpt_at(source, line, col))),
            None => (None, None, None),
        };
        return Error::Validation {
            path: path.map(|p| p.to_path_buf()),
            line,
            col,
            message,
            excerpt,
        };
    }

    let (line, col) = err_position(err).unwrap_or((1, 1));
    let excerpt = excerpt_at(source, line, col);
    Error::Parse {
        path: path.map(|p| p.to_path_buf()),
        line,
        col,
        message: err.to_string(),
        excerpt,
    }
}

/// Extract a best-effort (line, col) from a Rhai error.
fn err_position(err: &EvalAltResult) -> Option<(usize, usize)> {
    pos_to_line_col(err.position())
}

/// Convert a Rhai `Position` into a 1-based (line, col) pair.
fn pos_to_line_col(pos: Position) -> Option<(usize, usize)> {
    let line = pos.line()?;
    let col = pos.position().unwrap_or(1);
    Some((line.max(1), col.max(1)))
}

/// Extract a custom DSL validation error message and position from a Rhai error tree.
fn validation_error_from_rhai(err: &EvalAltResult) -> Option<(Position, String)> {
    match err {
        EvalAltResult::ErrorRuntime(d, pos) if d.is::<ValidationError>() => {
            let ve: ValidationError = d.clone_cast();
            Some((*pos, ve.message))
        }
        EvalAltResult::ErrorInFunctionCall(_, _, inner, _)
        | EvalAltResult::ErrorInModule(_, inner, _) => validation_error_from_rhai(inner),
        _ => None,
    }
}

#[derive(Debug, Clone)]
/// Error type used to tag load-time DSL validation failures.
struct ValidationError {
    /// Human-readable validation error message.
    message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl StdError for ValidationError {}

/// Register the Phase 1/2 config DSL and action constructors into the Rhai engine.
fn register_dsl(engine: &mut Engine, builder: Arc<Mutex<BuilderState>>) {
    engine.register_type::<DslMode>();
    engine.register_type::<DslBinding>();
    engine.register_type::<Action>();
    engine.register_type::<Toggle>();
    engine.register_type::<NotifyKind>();
    engine.register_type::<Pos>();
    engine.register_type::<NotifyPos>();
    engine.register_type::<Mode>();
    engine.register_type::<FontWeight>();
    engine.register_type::<ActionCtx>();
    engine.register_get("app", |ctx: &mut ActionCtx| ctx.app.clone());
    engine.register_get("title", |ctx: &mut ActionCtx| ctx.title.clone());
    engine.register_get("pid", |ctx: &mut ActionCtx| ctx.pid);
    engine.register_get("depth", |ctx: &mut ActionCtx| ctx.depth);
    engine.register_get("path", |ctx: &mut ActionCtx| ctx.path.clone());

    // Global setters.
    {
        let builder = builder.clone();
        engine.register_fn("base_theme", move |name: &str| {
            lock_unpoisoned(&builder).base_theme = Some(name.to_string());
        });
    }
    {
        let builder = builder.clone();
        engine.register_fn(
            "style",
            move |ctx: NativeCallContext, map: Map| -> Result<(), Box<EvalAltResult>> {
                let dyn_map = Dynamic::from_map(map);
                let style: raw::RawStyle = from_dynamic(&dyn_map).map_err(|e| {
                    boxed_validation_error(format!("invalid style map: {}", e), ctx.call_position())
                })?;
                lock_unpoisoned(&builder).style = Some(style);
                Ok(())
            },
        );
    }
    {
        let builder = builder.clone();
        engine.register_fn(
            "server",
            move |ctx: NativeCallContext, map: Map| -> Result<(), Box<EvalAltResult>> {
                let dyn_map = Dynamic::from_map(map);
                let raw_tunables: raw::RawServerTunables = from_dynamic(&dyn_map).map_err(|e| {
                    boxed_validation_error(
                        format!("invalid server map: {}", e),
                        ctx.call_position(),
                    )
                })?;
                lock_unpoisoned(&builder).server = Some(raw_tunables.into_server_tunables());
                Ok(())
            },
        );
    }

    // Helper: env(var) -> String
    engine.register_fn("env", |name: &str| -> String {
        env::var(name).unwrap_or_default()
    });

    // Action constructors.
    engine.register_fn("shell", |cmd: &str| {
        Action::Shell(crate::ShellSpec::Cmd(cmd.to_string()))
    });
    engine.register_fn("relay", |spec: &str| Action::Relay(spec.to_string()));
    engine.register_fn("show_details", |t: Toggle| Action::ShowDetails(t));
    engine.register_fn("theme_set", |name: &str| Action::ThemeSet(name.to_string()));
    engine.register_fn(
        "set_volume",
        |ctx: NativeCallContext, level: i64| -> Result<Action, Box<EvalAltResult>> {
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
        |ctx: NativeCallContext, delta: i64| -> Result<Action, Box<EvalAltResult>> {
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
    engine.register_fn("mute", |t: Toggle| Action::Mute(t));
    engine.register_fn("user_style", |t: Toggle| Action::UserStyle(t));

    // Action fluent methods.
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

    // Mode methods.
    engine.register_fn(
        "bind",
        |ctx: NativeCallContext,
         mode: &mut DslMode,
         chord: &str,
         desc: &str,
         action: Action|
         -> Result<DslBinding, Box<EvalAltResult>> {
            mode_bind(&ctx, mode, chord, desc, action)
        },
    );
    {
        let builder = builder;
        engine.register_fn(
            "bind",
            move |ctx: NativeCallContext,
                  mode: &mut DslMode,
                  chord: &str,
                  desc: &str,
                  f: FnPtr|
                  -> Result<DslBinding, Box<EvalAltResult>> {
                mode_bind_script(&ctx, mode, chord, desc, f, &builder)
            },
        );
    }

    engine.register_fn(
        "mode",
        |ctx: NativeCallContext,
         mode: &mut DslMode,
         chord: &str,
         desc: &str,
         builder_fn: FnPtr|
         -> Result<DslBinding, Box<EvalAltResult>> {
            mode_mode(&ctx, mode, chord, desc, &builder_fn)
        },
    );

    // Binding fluent methods.
    engine.register_fn("global", |b: DslBinding| {
        binding_set_bool(&b, BindingFlag::Global)
    });
    engine.register_fn("hidden", |b: DslBinding| {
        binding_set_bool(&b, BindingFlag::Hidden)
    });
    engine.register_fn("hud_only", |b: DslBinding| {
        binding_set_bool(&b, BindingFlag::HudOnly)
    });
    engine.register_fn(
        "no_exit",
        |ctx: NativeCallContext, b: DslBinding| -> Result<DslBinding, Box<EvalAltResult>> {
            binding_no_exit(&ctx, b)
        },
    );
    engine.register_fn(
        "repeat",
        |ctx: NativeCallContext, b: DslBinding| -> Result<DslBinding, Box<EvalAltResult>> {
            binding_repeat(&ctx, b)
        },
    );
    engine.register_fn(
        "repeat_ms",
        |ctx: NativeCallContext,
         b: DslBinding,
         delay: i64,
         interval: i64|
         -> Result<DslBinding, Box<EvalAltResult>> {
            binding_repeat_ms(&ctx, b, delay, interval)
        },
    );
    engine.register_fn(
        "capture",
        |ctx: NativeCallContext, b: DslBinding| -> Result<DslBinding, Box<EvalAltResult>> {
            binding_capture(&ctx, b)
        },
    );
    engine.register_fn("match_app", |b: DslBinding, pattern: &str| -> DslBinding {
        binding_set_string(&b, BindingStringField::MatchApp, pattern)
    });
    engine.register_fn(
        "match_title",
        |b: DslBinding, pattern: &str| -> DslBinding {
            binding_set_string(&b, BindingStringField::MatchTitle, pattern)
        },
    );
    engine.register_fn(
        "style",
        |ctx: NativeCallContext,
         b: DslBinding,
         map: Map|
         -> Result<DslBinding, Box<EvalAltResult>> { binding_style(&ctx, b, map) },
    );
}

/// Register Hotki's predefined constants (toggles, notify kinds, positions, actions, etc.).
fn register_constants(engine: &mut Engine, builder: &Arc<Mutex<BuilderState>>) {
    let root = lock_unpoisoned(builder).root.clone();

    let mut module = Module::new();
    module.set_var("global", DslMode { state: root });

    module.set_var("on", Toggle::On);
    module.set_var("off", Toggle::Off);
    module.set_var("toggle", Toggle::Toggle);

    module.set_var("ignore", NotifyKind::Ignore);
    module.set_var("info", NotifyKind::Info);
    module.set_var("warn", NotifyKind::Warn);
    module.set_var("error", NotifyKind::Error);
    module.set_var("success", NotifyKind::Success);

    // Style-related enums are represented as strings so `from_dynamic`
    // can deserialize them into the existing serde-driven config types.
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

    module.set_var("hud_full", "hud");
    module.set_var("hud_mini", "mini");
    module.set_var("hud_hide", "hide");

    module.set_var("thin", "thin");
    module.set_var("extralight", "extralight");
    module.set_var("light", "light");
    module.set_var("regular", "regular");
    module.set_var("medium", "medium");
    module.set_var("semibold", "semibold");
    module.set_var("bold", "bold");
    module.set_var("extrabold", "extrabold");
    module.set_var("black", "black");

    module.set_var("pop", Action::Pop);
    module.set_var("exit", Action::Exit);
    module.set_var("reload_config", Action::ReloadConfig);
    module.set_var("clear_notifications", Action::ClearNotifications);
    module.set_var("theme_next", Action::ThemeNext);
    module.set_var("theme_prev", Action::ThemePrev);
    module.set_var("show_hud_root", Action::ShowHudRoot);

    engine.register_global_module(module.into());
}

/// Parse a chord string and produce a validation error at the call site on failure.
fn parse_chord(ctx: &NativeCallContext, chord: &str) -> Result<Chord, Box<EvalAltResult>> {
    Chord::parse(chord).ok_or_else(|| {
        boxed_validation_error(
            format!("invalid chord string: {}", chord),
            ctx.call_position(),
        )
    })
}

/// Add a leaf binding (`.bind(...)`) to a mode.
fn mode_bind(
    ctx: &NativeCallContext,
    mode: &DslMode,
    chord: &str,
    desc: &str,
    action: Action,
) -> Result<DslBinding, Box<EvalAltResult>> {
    let chord = parse_chord(ctx, chord)?;
    let mut guard = lock_unpoisoned(&mode.state);

    let idx = guard.entries.len();
    guard.entries.push(BindingEntry {
        chord,
        desc: desc.to_string(),
        action,
        attrs: KeysAttrs::default(),
        kind: BindingKind::Bind,
        pos: ctx.call_position(),
    });
    Ok(DslBinding {
        state: mode.state.clone(),
        index: idx,
    })
}

/// Register a Rhai callable as a stable script action id and bind it as `Action::Rhai`.
fn mode_bind_script(
    ctx: &NativeCallContext,
    mode: &DslMode,
    chord: &str,
    desc: &str,
    f: FnPtr,
    builder: &Arc<Mutex<BuilderState>>,
) -> Result<DslBinding, Box<EvalAltResult>> {
    let id = {
        let mut guard = lock_unpoisoned(builder);
        let id = guard.next_script_id;
        guard.next_script_id = guard.next_script_id.checked_add(1).ok_or_else(|| {
            boxed_validation_error("too many script actions".to_string(), ctx.call_position())
        })?;
        guard.script_actions.insert(id, f);
        id
    };

    mode_bind(ctx, mode, chord, desc, Action::Rhai { id })
}

/// Add a sub-mode binding (`.mode(...)`) by invoking a builder closure to populate the child mode.
fn mode_mode(
    ctx: &NativeCallContext,
    mode: &DslMode,
    chord: &str,
    desc: &str,
    builder_fn: &FnPtr,
) -> Result<DslBinding, Box<EvalAltResult>> {
    let chord = parse_chord(ctx, chord)?;

    let child_state = Arc::new(Mutex::new(ModeState::default()));
    let child_mode = DslMode {
        state: child_state.clone(),
    };

    // Invoke the closure with the new mode.
    let _ignored: Dynamic = builder_fn.call_within_context(ctx, (child_mode,))?;

    let child_entries = lock_unpoisoned(&child_state).entries.clone();
    validate_mode_chords(&child_entries)?;
    let action = Action::Keys(build_keys(&child_entries));

    let mut guard = lock_unpoisoned(&mode.state);

    let idx = guard.entries.len();
    guard.entries.push(BindingEntry {
        chord,
        desc: desc.to_string(),
        action,
        attrs: KeysAttrs::default(),
        kind: BindingKind::Mode,
        pos: ctx.call_position(),
    });
    Ok(DslBinding {
        state: mode.state.clone(),
        index: idx,
    })
}

#[derive(Debug, Clone, Copy)]
/// Boolean binding flags settable via fluent DSL methods.
enum BindingFlag {
    /// Binding is active in this mode and all submodes.
    Global,
    /// Binding works but is hidden from the HUD.
    Hidden,
    /// Binding only activates while the HUD is visible.
    HudOnly,
}

/// Set a boolean binding attribute and return the binding for fluent chaining.
fn binding_set_bool(b: &DslBinding, flag: BindingFlag) -> DslBinding {
    let mut guard = lock_unpoisoned(&b.state);
    if let Some(entry) = guard.entries.get_mut(b.index) {
        match flag {
            BindingFlag::Global => entry.attrs.global = raw::Maybe::Value(true),
            BindingFlag::Hidden => entry.attrs.hide = raw::Maybe::Value(true),
            BindingFlag::HudOnly => entry.attrs.hud_only = raw::Maybe::Value(true),
        }
    }
    b.clone()
}

/// Mark a binding as `no_exit`, rejecting calls on mode bindings.
fn binding_no_exit(
    ctx: &NativeCallContext,
    b: DslBinding,
) -> Result<DslBinding, Box<EvalAltResult>> {
    {
        let mut guard = lock_unpoisoned(&b.state);
        let entry = guard.entries.get_mut(b.index).ok_or_else(|| {
            boxed_validation_error("invalid binding handle".to_string(), ctx.call_position())
        })?;
        if entry.kind != BindingKind::Bind {
            return Err(boxed_validation_error(
                "no_exit is only valid on .bind() bindings".to_string(),
                ctx.call_position(),
            ));
        }
        entry.attrs.noexit = raw::Maybe::Value(true);
    }
    Ok(b)
}

/// Enable hold-to-repeat with default timings, rejecting calls on mode bindings.
fn binding_repeat(
    ctx: &NativeCallContext,
    b: DslBinding,
) -> Result<DslBinding, Box<EvalAltResult>> {
    {
        let mut guard = lock_unpoisoned(&b.state);
        let entry = guard.entries.get_mut(b.index).ok_or_else(|| {
            boxed_validation_error("invalid binding handle".to_string(), ctx.call_position())
        })?;
        if entry.kind != BindingKind::Bind {
            return Err(boxed_validation_error(
                "repeat is only valid on .bind() bindings".to_string(),
                ctx.call_position(),
            ));
        }
        entry.attrs.repeat = raw::Maybe::Value(true);
    }
    Ok(b)
}

#[derive(Debug, Clone, Copy)]
/// String-valued binding fields settable via fluent DSL methods.
enum BindingStringField {
    /// Regex pattern for filtering by focused application name.
    MatchApp,
    /// Regex pattern for filtering by focused window title.
    MatchTitle,
}

/// Set a string binding attribute and return the binding for fluent chaining.
fn binding_set_string(b: &DslBinding, field: BindingStringField, value: &str) -> DslBinding {
    let mut guard = lock_unpoisoned(&b.state);
    if let Some(entry) = guard.entries.get_mut(b.index) {
        match field {
            BindingStringField::MatchApp => {
                entry.attrs.match_app = raw::Maybe::Value(value.to_string())
            }
            BindingStringField::MatchTitle => {
                entry.attrs.match_title = raw::Maybe::Value(value.to_string())
            }
        }
    }
    b.clone()
}

/// Enable hold-to-repeat with custom timings (in milliseconds).
fn binding_repeat_ms(
    ctx: &NativeCallContext,
    b: DslBinding,
    delay: i64,
    interval: i64,
) -> Result<DslBinding, Box<EvalAltResult>> {
    if delay < 0 || interval < 0 {
        return Err(boxed_validation_error(
            "repeat_ms expects non-negative millisecond values".to_string(),
            ctx.call_position(),
        ));
    }
    let delay_u64: u64 = delay.try_into().map_err(|_| {
        boxed_validation_error("repeat_ms delay too large".to_string(), ctx.call_position())
    })?;
    let interval_u64: u64 = interval.try_into().map_err(|_| {
        boxed_validation_error(
            "repeat_ms interval too large".to_string(),
            ctx.call_position(),
        )
    })?;

    {
        let mut guard = lock_unpoisoned(&b.state);
        let entry = guard.entries.get_mut(b.index).ok_or_else(|| {
            boxed_validation_error("invalid binding handle".to_string(), ctx.call_position())
        })?;
        if entry.kind != BindingKind::Bind {
            return Err(boxed_validation_error(
                "repeat_ms is only valid on .bind() bindings".to_string(),
                ctx.call_position(),
            ));
        }
        entry.attrs.repeat = raw::Maybe::Value(true);
        entry.attrs.repeat_delay = raw::Maybe::Value(delay_u64);
        entry.attrs.repeat_interval = raw::Maybe::Value(interval_u64);
    }
    Ok(b)
}

/// Enable capture mode for a `.mode(...)` binding, rejecting calls on leaf bindings.
fn binding_capture(
    ctx: &NativeCallContext,
    b: DslBinding,
) -> Result<DslBinding, Box<EvalAltResult>> {
    {
        let mut guard = lock_unpoisoned(&b.state);
        let entry = guard.entries.get_mut(b.index).ok_or_else(|| {
            boxed_validation_error("invalid binding handle".to_string(), ctx.call_position())
        })?;
        if entry.kind != BindingKind::Mode {
            return Err(boxed_validation_error(
                "capture is only valid on .mode() bindings".to_string(),
                ctx.call_position(),
            ));
        }
        entry.attrs.capture = raw::Maybe::Value(true);
    }
    Ok(b)
}

/// Apply a per-mode style overlay to a `.mode(...)` binding.
fn binding_style(
    ctx: &NativeCallContext,
    b: DslBinding,
    map: Map,
) -> Result<DslBinding, Box<EvalAltResult>> {
    {
        let mut guard = lock_unpoisoned(&b.state);
        let entry = guard.entries.get_mut(b.index).ok_or_else(|| {
            boxed_validation_error("invalid binding handle".to_string(), ctx.call_position())
        })?;
        if entry.kind != BindingKind::Mode {
            return Err(boxed_validation_error(
                "style is only valid on .mode() bindings".to_string(),
                ctx.call_position(),
            ));
        }

        let dyn_map = Dynamic::from_map(map);
        let style: raw::RawStyle = from_dynamic(&dyn_map)
            .map_err(|e| boxed_validation_error(e.to_string(), ctx.call_position()))?;
        entry.attrs.style = raw::Maybe::Value(style);
    }
    Ok(b)
}

/// Construct a Rhai runtime error tagged as a DSL validation failure.
fn boxed_validation_error(message: String, pos: Position) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(ValidationError { message }),
        pos,
    ))
}

#[derive(Debug, Clone)]
/// Context passed to a script action callable at runtime.
struct ActionCtx {
    /// Name of the focused application.
    app: String,
    /// Title of the focused window.
    title: String,
    /// Process identifier of the focused application.
    pid: i64,
    /// Current mode depth (0 = root).
    depth: i64,
    /// Cursor path as indices from the root.
    path: Vec<Dynamic>,
}

impl ActionCtx {
    /// Build a new context from the focused app/window and current cursor.
    fn from_parts(app: &str, title: &str, pid: i32, cursor: &Cursor) -> Self {
        let path = cursor
            .path()
            .iter()
            .map(|idx| Dynamic::from(*idx as i64))
            .collect();
        Self {
            app: app.to_string(),
            title: title.to_string(),
            pid: pid as i64,
            depth: cursor.depth() as i64,
            path,
        }
    }
}

/// A compiled Rhai runtime capable of executing script actions declared in `config.rhai`.
pub struct RhaiRuntime {
    /// The engine configured with Hotki's DSL types and sandbox limits.
    engine: Engine,
    /// The compiled AST containing helper functions and closures.
    ast: AST,
    /// Mapping of script action ids to compiled callables.
    actions: BTreeMap<u64, FnPtr>,
    /// Source used for formatting error excerpts.
    source: String,
    /// Optional source path used in error messages.
    path: Option<PathBuf>,
}

impl RhaiRuntime {
    /// Create a new runtime from a compiled script and extracted action registry.
    fn new(
        engine: Engine,
        ast: AST,
        actions: BTreeMap<u64, FnPtr>,
        source: String,
        path: Option<PathBuf>,
    ) -> Self {
        Self {
            engine,
            ast,
            actions,
            source,
            path,
        }
    }

    pub fn eval_action(
        &self,
        id: u64,
        app: &str,
        title: &str,
        pid: i32,
        cursor: &Cursor,
    ) -> Result<Vec<Action>, String> {
        let f = self
            .actions
            .get(&id)
            .ok_or_else(|| format!("unknown script action id: {}", id))?;

        let ctx = ActionCtx::from_parts(app, title, pid, cursor);

        let result = match f.call::<Dynamic>(&self.engine, &self.ast, (ctx,)) {
            Ok(v) => v,
            Err(err) => match err.as_ref() {
                EvalAltResult::ErrorFunctionNotFound(sig, _) if sig.starts_with(f.fn_name()) => f
                    .call::<Dynamic>(&self.engine, &self.ast, ())
                    .map_err(|err| self.format_action_error(&err))?,
                _ => return Err(self.format_action_error(&err)),
            },
        };

        coerce_actions(result)
    }

    /// Format a Rhai runtime error with an excerpt when line/col are available.
    fn format_action_error(&self, err: &EvalAltResult) -> String {
        let loc = err_position(err);
        let msg = match (&self.path, loc) {
            (Some(path), Some((line, col))) => format!(
                "Script action error at {}:{}:{}\n{}",
                path.display(),
                line,
                col,
                err
            ),
            (Some(path), None) => format!("Script action error in {}\n{}", path.display(), err),
            (None, Some((line, col))) => format!(
                "Script action error at line {}, column {}\n{}",
                line, col, err
            ),
            (None, None) => format!("Script action error\n{}", err),
        };

        let Some((line, col)) = loc else {
            return msg;
        };
        let excerpt = excerpt_at(&self.source, line, col);
        format!("{}\n{}", msg, excerpt)
    }
}

/// Coerce a script action return value into a concrete list of `Action`s.
fn coerce_actions(result: Dynamic) -> Result<Vec<Action>, String> {
    if result.is::<Action>() {
        let action: Action = result.clone_cast();
        return Ok(vec![action]);
    }

    if result.is::<rhai::Array>() {
        let arr: rhai::Array = result.cast();
        let mut out = Vec::with_capacity(arr.len());
        for item in arr {
            if !item.is::<Action>() {
                return Err(format!(
                    "script action array element must be Action, got {}",
                    item.type_name()
                ));
            }
            out.push(item.cast::<Action>());
        }
        return Ok(out);
    }

    Err(format!(
        "script action must return Action or [Action], got {}",
        result.type_name()
    ))
}
