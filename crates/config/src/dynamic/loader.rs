use std::{
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use rhai::{
    AST, Dynamic, Engine, EvalAltResult, Module, ModuleResolver, Position, Scope,
    module_resolvers::FileModuleResolver,
};
use tracing::{debug, info};

use crate::{Error, error::excerpt_at};

use super::{DynamicConfig, ModeCtx};
use super::dsl::{DynamicConfigScriptState, ModeBuilder, ValidationError, register_dsl};

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

pub(crate) fn load_dynamic_config_from_string(
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

    let (root, base_theme, user_style) = {
        let mut guard = lock_unpoisoned(&state);

        let root = guard.root.take().ok_or_else(|| Error::Validation {
            path: path.clone(),
            line: None,
            col: None,
            message: "hotki.mode() must be called exactly once".to_string(),
            excerpt: None,
        })?;

        (root, guard.base_theme.take(), guard.user_style.take())
    };

    validate_root_closure(&engine, &ast, &source, path.as_deref(), &root)?;

    Ok(DynamicConfig {
        root,
        base_theme,
        user_style,
        engine,
        ast,
        source: Arc::from(source.into_boxed_str()),
        path,
    })
}

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
        .call::<()>(engine, ast, (builder, ctx))
        .map_err(|e| error_from_rhai(source, &e, path))
}

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
struct ConfigModuleResolver {
    root: PathBuf,
    root_canon: PathBuf,
    inner: FileModuleResolver,
}

impl ConfigModuleResolver {
    fn new(root: PathBuf) -> Self {
        let root_canon = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        Self {
            inner: FileModuleResolver::new_with_path(root.clone()),
            root,
            root_canon,
        }
    }

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

    fn invalid_import_path(&self, message: String, pos: Position) -> Box<EvalAltResult> {
        Box::new(EvalAltResult::ErrorRuntime(Dynamic::from(message), pos))
    }

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
            Box::new(EvalAltResult::ErrorRuntime(Dynamic::from(e.to_string()), pos))
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

fn compile(engine: &Engine, source: &str, path: Option<&Path>) -> Result<AST, Error> {
    engine.compile(source).map_err(|err| {
        let err: EvalAltResult = err.into();
        error_from_rhai(source, &err, path)
    })
}

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

fn lock_unpoisoned<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn error_from_rhai(source: &str, err: &EvalAltResult, path: Option<&Path>) -> Error {
    if let Some((pos, message)) = validation_error_from_rhai(err) {
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

fn pos_to_line_col(pos: Position) -> Option<(usize, usize)> {
    let line = pos.line()?;
    let col = pos.position().unwrap_or(1);
    Some((line.max(1), col.max(1)))
}
