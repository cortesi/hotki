use std::{
    ffi::OsStr,
    fs, mem,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

use rhai::{
    AST, Dynamic, Engine, EvalAltResult, Module, ModuleResolver, Position, Scope,
    module_resolvers::FileModuleResolver,
};
use tracing::{debug, info};

use super::{
    DynamicConfig, ModeCtx,
    dsl::{DynamicConfigScriptState, ModeBuilder, register_dsl},
    util::lock_unpoisoned,
    validation::extract_validation_error,
};
use crate::{Error, error::excerpt_at};

/// Load a dynamic config from a Rhai file at `path`.
pub fn load_dynamic_config(path: &Path) -> Result<DynamicConfig, Error> {
    if path.extension() != Some(OsStr::new("rhai")) {
        return Err(Error::Read {
            path: Some(path.to_path_buf()),
            message: "Unsupported config format (expected a .rhai file)".to_string(),
        });
    }

    let source = fs::read_to_string(path).map_err(|e| Error::Read {
        path: Some(path.to_path_buf()),
        message: e.to_string(),
    })?;

    load_dynamic_config_from_string(source, Some(path.to_path_buf()))
}

/// Load a dynamic config from Rhai source text and an optional origin path.
pub fn load_dynamic_config_from_string(
    source: String,
    path: Option<PathBuf>,
) -> Result<DynamicConfig, Error> {
    let state = Arc::new(Mutex::new(DynamicConfigScriptState::default()));

    let mut engine = Engine::new();
    configure_engine(&mut engine, path.as_deref());
    register_dsl(&mut engine, state.clone());

    let mut scope = Scope::new();

    let ast = compile(&engine, &source, path.as_deref())?;
    eval(&engine, &mut scope, &ast, &source, path.as_deref())?;

    let (root, active_theme, themes) = {
        let mut guard = lock_unpoisoned(&state);

        let root = guard.root.take().ok_or_else(|| Error::Validation {
            path: path.clone(),
            line: None,
            col: None,
            message: "hotki.mode() must be called exactly once".to_string(),
            excerpt: None,
        })?;

        let active_theme = mem::take(&mut guard.active_theme);
        let themes = mem::take(&mut guard.themes);

        (root, active_theme, themes)
    };

    validate_root_closure(&engine, &ast, &source, path.as_deref(), &root)?;

    Ok(DynamicConfig {
        root,
        active_theme,
        themes,
        engine,
        ast,
        source: Arc::from(source.into_boxed_str()),
        path,
    })
}

/// Validate that the root mode closure can be executed successfully.
fn validate_root_closure(
    engine: &Engine,
    ast: &AST,
    source: &str,
    path: Option<&Path>,
    root: &super::ModeRef,
) -> Result<(), Error> {
    let builder = ModeBuilder::new();
    let ctx = ModeCtx {
        app: String::new(),
        title: String::new(),
        pid: 0,
        hud: false,
        depth: 0,
    };

    root.func
        .call::<Dynamic>(engine, ast, (builder, ctx))
        .map(|_| ())
        .map_err(|e| error_from_rhai(source, &e, path))
}

/// Configure the Rhai engine with logging, module resolution, and resource limits.
fn configure_engine(engine: &mut Engine, path: Option<&Path>) {
    engine.on_print(|s| info!(target: "config::dynamic", "{}", s));
    engine.on_debug(|s, src, pos| {
        debug!(target: "config::dynamic", "{} @ {:?}:{:?}", s, src, pos);
    });

    if let Some(path) = path
        && let Some(dir) = path.parent()
    {
        engine.set_module_resolver(ConfigModuleResolver::new(dir.to_path_buf()));
    }

    engine.set_max_operations(200_000);
    engine.set_max_call_levels(64);
    engine.set_max_expr_depths(128, 64);
}

#[derive(Debug)]
/// Module resolver that restricts imports to within the config directory tree.
struct ConfigModuleResolver {
    /// Root config directory as provided.
    root: PathBuf,
    /// Canonicalized root directory for escape detection.
    root_canon: PathBuf,
    /// Underlying file resolver used for actual module loading.
    inner: FileModuleResolver,
}

impl ConfigModuleResolver {
    /// Create a resolver rooted at the given directory.
    fn new(root: PathBuf) -> Self {
        let root_canon = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        Self {
            inner: FileModuleResolver::new_with_path(root.clone()),
            root,
            root_canon,
        }
    }

    /// Validate an import path for security and portability.
    fn validate_import_path(&self, path: &str, pos: Position) -> Result<(), Box<EvalAltResult>> {
        if Path::new(path).is_absolute() {
            return Err(self.invalid_import_path(
                format!("absolute paths are not allowed in imports: {}", path),
                pos,
            ));
        }

        for comp in Path::new(path).components() {
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

    /// Construct a Rhai runtime error for invalid imports.
    fn invalid_import_path(&self, message: String, pos: Position) -> Box<EvalAltResult> {
        Box::new(EvalAltResult::ErrorRuntime(Dynamic::from(message), pos))
    }

    /// Ensure a resolved file does not escape the configured root directory.
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

/// Compile source code into an AST, converting errors to `config::Error`.
fn compile(engine: &Engine, source: &str, path: Option<&Path>) -> Result<AST, Error> {
    engine.compile(source).map_err(|err| {
        let err: EvalAltResult = err.into();
        error_from_rhai(source, &err, path)
    })
}

/// Evaluate a previously compiled AST, converting errors to `config::Error`.
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

/// Convert a Rhai execution error into a user-facing config error with an excerpt.
fn error_from_rhai(source: &str, err: &EvalAltResult, path: Option<&Path>) -> Error {
    if let Some((pos, message)) = extract_validation_error(err) {
        let (line, col, excerpt) = match pos_to_line_col(pos) {
            Some((line, col)) => (Some(line), Some(col), Some(excerpt_at(source, line, col))),
            None => (None, None, None),
        };
        return Error::Validation {
            path: path.map(Path::to_path_buf),
            line,
            col,
            message,
            excerpt,
        };
    }

    let (line, col) = pos_to_line_col(err.position()).unwrap_or((1, 1));
    Error::Parse {
        path: path.map(Path::to_path_buf),
        line,
        col,
        message: err.to_string(),
        excerpt: excerpt_at(source, line, col),
    }
}

/// Convert a Rhai source position into 1-based line/column.
fn pos_to_line_col(pos: Position) -> Option<(usize, usize)> {
    let line = pos.line()?;
    let col = pos.position().unwrap_or(1);
    Some((line.max(1), col.max(1)))
}
