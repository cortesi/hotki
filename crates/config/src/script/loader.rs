//! Luau config loading orchestration.

use std::{
    collections::HashMap,
    ffi::OsStr,
    fmt::Write,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use ruau::{
    bytecode::{BytecodeChunk, CompileOptions},
    session::{LoadTarget, Runtime},
    source::{ModuleId, Source, SourceMetadata, SourceProvider, fs::Directory},
    surface::{PrepareGraphError, PrepareOptions, PreparedGraph, Surface, VmConfig},
    vm::{Ambient, Limits, MultiValue, NativeModule, ScopedValue, ScriptError},
};

use super::{
    LoadedConfig, ModeCtx, ModeRef,
    config::SourceMap,
    diagnostics,
    host_hotki::build_hotki_module,
    host_runtime::{ApplicationCache, chunk_name},
    host_userdata::{ModeBuilder, mode_builder_userdata, mode_context_userdata},
    module_source::ConfigModuleSource,
    util::lock_unpoisoned,
};
use crate::{Error, ResolvedStyle, StyleResolver, error::excerpt_at};

/// Load a Luau config from a file at `path`.
pub fn load_dynamic_config(path: &Path) -> Result<LoadedConfig, Error> {
    let resolved_style = StyleResolver::from_config_path(path)?.resolve()?;
    load_dynamic_config_with_style(path, resolved_style)
}

/// Load a filesystem config with a style already resolved for the same candidate.
pub fn load_dynamic_config_with_style(
    path: &Path,
    resolved_style: ResolvedStyle,
) -> Result<LoadedConfig, Error> {
    if path.extension() != Some(OsStr::new("luau")) {
        return Err(Error::Read {
            path: Some(path.to_path_buf()),
            message: "Unsupported config format (expected a .luau file)".to_string(),
        });
    }

    let path = fs::canonicalize(path).map_err(|err| Error::Read {
        path: Some(path.to_path_buf()),
        message: err.to_string(),
    })?;
    let source = fs::read_to_string(&path).map_err(|err| Error::Read {
        path: Some(path.clone()),
        message: err.to_string(),
    })?;

    load_dynamic_config_from_string_with_style(&source, Some(path), resolved_style)
}

/// Executable entry artifact for filesystem and in-memory configs.
enum RootProgram {
    /// Exact checked graph for a filesystem-backed config.
    Prepared(Box<PreparedGraph>),
    /// Standalone bytecode for an in-memory config without `require`.
    Compiled(BytecodeChunk),
}

/// Load a Luau config from source text and an optional origin path.
#[cfg(test)]
pub fn load_dynamic_config_from_string(
    source: &str,
    path: Option<PathBuf>,
) -> Result<LoadedConfig, Error> {
    let resolved_style = match path.as_deref() {
        Some(path) => StyleResolver::from_config_path(path)?.resolve()?,
        None => StyleResolver::default_only()?.resolve()?,
    };
    load_dynamic_config_from_string_with_style(source, path, resolved_style)
}

/// Load config source with its already-resolved style.
fn load_dynamic_config_from_string_with_style(
    source: &str,
    path: Option<PathBuf>,
    resolved_style: ResolvedStyle,
) -> Result<LoadedConfig, Error> {
    let sources = Arc::new(Mutex::new(HashMap::new()));
    let source_key = path.clone().unwrap_or_else(|| PathBuf::from("<memory>"));
    lock_unpoisoned(&sources).insert(source_key, Arc::from(source.to_string().into_boxed_str()));

    let applications = Arc::new(Mutex::new(ApplicationCache::default()));
    let callbacks = LoadedConfig::callback_registry();
    let module = build_hotki_module(applications)
        .map_err(|err| diagnostics::config_validation(path.clone(), err))?;
    let (surface, program, module_source, module_count) = if let Some(path) = path.as_deref() {
        let (surface, prepared, module_source, module_count) =
            prepare_filesystem_config(source, path, module, &sources)?;
        (
            surface,
            RootProgram::Prepared(Box::new(prepared)),
            Some(module_source),
            module_count,
        )
    } else {
        let surface = build_surface(module, None, path.as_deref())?;
        let chunk = surface
            .runtime_capabilities()
            .compile_source(source.as_bytes(), &CompileOptions::new())
            .map_err(|err| diagnostics::config_compile_error(source, &err, path.as_deref()))?;
        (surface, RootProgram::Compiled(chunk), None, 1)
    };
    let mut runtime = build_runtime(surface, path.as_deref())?;
    let loaded = match &program {
        RootProgram::Prepared(prepared) => runtime.load_prepared(prepared),
        RootProgram::Compiled(chunk) => runtime.load_compiled(
            chunk,
            &LoadTarget::named(chunk_name(path.as_deref()).into_bytes()),
        ),
    }
    .map_err(|err| diagnostics::config_retained_error(path.clone(), &err))?;
    let mut context = super::callback::CallbackContext::new(Arc::clone(&callbacks));
    let options = LoadedConfig::entry_options();
    let mut script_error = None;
    let mut root = None;
    let mut invalid_root = false;
    let run = runtime.step_root_with_context(&loaded, &mut context, &options, |scope, entry| {
        let result: Result<MultiValue<'_>, ScriptError<'_>> = scope.call_protected(entry, ())?;
        match result {
            Ok(values) => match values.into_vec().as_slice() {
                [ScopedValue::Function(function)] => {
                    root = Some(ModeRef::from_function(scope, *function, None)?);
                }
                _ => invalid_root = true,
            },
            Err(err) => {
                script_error = Some(diagnostics::config_script_error(
                    path.as_deref(),
                    &sources,
                    scope,
                    &err,
                ));
            }
        }
        Ok(())
    });
    let entry_gas = runtime.gas_spent();
    let synchronized = super::callback::CallbackRegistry::synchronize(&callbacks, &mut runtime)
        .map_err(|err| diagnostics::config_retained_error(path.clone(), &err));
    let unloaded = runtime
        .unload(&loaded)
        .map_err(|err| diagnostics::config_retained_error(path.clone(), &err));
    run.map_err(|err| diagnostics::config_retained_error(path.clone(), &err))?;
    synchronized?;
    unloaded?;
    if let Some(error) = script_error {
        return Err(error);
    }
    if invalid_root {
        return Err(diagnostics::config_validation(
            path.clone(),
            "config.luau must return a ModeRenderer",
        ));
    }
    let root = root.ok_or_else(|| {
        diagnostics::config_validation(path.clone(), "config.luau must return a ModeRenderer")
    })?;
    if let Some(module_source) = module_source {
        module_source.seal();
    }
    validate_root(&mut runtime, &callbacks, &root, path.as_deref(), &sources)?;
    let validation_gas = runtime.gas_spent();

    Ok(LoadedConfig {
        root,
        base_style: resolved_style.style,
        style_provenance: resolved_style.provenance,
        runtime,
        callbacks,
        path,
        sources,
        module_count,
        entry_gas,
        validation_gas,
    })
}

/// Build the shared typed surface, optionally granting filesystem modules.
fn build_surface(
    module: Arc<dyn NativeModule>,
    source: Option<Arc<dyn SourceProvider>>,
    path: Option<&Path>,
) -> Result<Surface, Error> {
    let mut builder = Surface::builder()
        .enable_runtime_compilation()
        .module(module)
        .require_return("ModeRenderer");
    if let Some(source) = source {
        builder = builder.module_source(source);
    }
    builder
        .build()
        .map_err(|error| diagnostics::config_validation(path.map(Path::to_path_buf), error))
}

/// Prepare a contextual root and its exact cached filesystem graph.
fn prepare_filesystem_config(
    source: &str,
    path: &Path,
    module: Arc<dyn NativeModule>,
    sources: &SourceMap,
) -> Result<(Surface, PreparedGraph, Arc<ConfigModuleSource>, usize), Error> {
    let root_dir = path.parent().ok_or_else(|| Error::Read {
        path: Some(path.to_path_buf()),
        message: "config path must have a parent directory".to_string(),
    })?;
    let stem = path
        .file_stem()
        .and_then(OsStr::to_str)
        .ok_or_else(|| Error::Read {
            path: Some(path.to_path_buf()),
            message: "config filename must be valid UTF-8".to_string(),
        })?;
    let entry_id = ModuleId::canonicalized(stem);
    let filesystem: Arc<dyn SourceProvider> = Arc::new(Directory::new(root_dir));
    let module_source = Arc::new(ConfigModuleSource::new(
        filesystem,
        entry_id.clone(),
        root_dir.to_path_buf(),
        Arc::clone(sources),
    ));
    let surface = build_surface(
        module,
        Some(Arc::clone(&module_source) as Arc<dyn SourceProvider>),
        Some(path),
    )?;
    let root = Source::text(entry_id, source.to_owned())
        .with_metadata(SourceMetadata::new(path.display().to_string()));
    let prepared = surface
        .prepare_graph_blocking_with_options(root, PrepareOptions::new().reject_errors())
        .map_err(|error| graph_prepare_error(path, sources, &error))?;
    module_source.allow_only(
        prepared
            .graph()
            .checked_modules()
            .keys()
            .map(ModuleId::from),
    );
    let module_count = prepared.graph().checked_modules().len();
    Ok((surface, prepared, module_source, module_count))
}

/// Convert ordered graph diagnostics into Hotki's located error shape.
fn graph_prepare_error(path: &Path, sources: &SourceMap, error: &PrepareGraphError) -> Error {
    let Some(diagnostics) = error.diagnostics() else {
        return diagnostics::config_validation(Some(path.to_path_buf()), error);
    };
    let mut views = diagnostics.views();
    let Some(view) = views.next() else {
        return diagnostics::config_validation(Some(path.to_path_buf()), error);
    };
    let display_path = PathBuf::from(view.display_name);
    let diagnostic_path = if display_path.is_absolute() {
        display_path
    } else {
        path.parent()
            .unwrap_or_else(|| Path::new(""))
            .join(display_path)
    };
    let location = view.diagnostic.primary_location;
    let (line, col) = if location.is_missing() {
        (None, None)
    } else {
        (
            Some(location.begin.line as usize),
            Some(location.begin.column as usize),
        )
    };
    let excerpt = line.zip(col).and_then(|(line, col)| {
        lock_unpoisoned(sources)
            .get(&diagnostic_path)
            .map(|source| excerpt_at(source, line, col))
    });
    let mut message = view.diagnostic.message;
    for additional in views {
        let location = additional.diagnostic.primary_location;
        if location.is_missing() {
            write!(
                message,
                "\n{}: {}",
                additional.display_name, additional.diagnostic.message
            )
            .expect("writing a diagnostic to String cannot fail");
        } else {
            write!(
                message,
                "\n{}:{}:{}: {}",
                additional.display_name,
                location.begin.line,
                location.begin.column,
                additional.diagnostic.message
            )
            .expect("writing a diagnostic to String cannot fail");
        }
    }
    Error::Validation {
        path: Some(diagnostic_path),
        line,
        col,
        message,
        excerpt,
    }
}

/// Build the sandboxed retained runtime used by a dynamic config.
fn build_runtime(surface: Surface, path: Option<&Path>) -> Result<Runtime, Error> {
    Runtime::new(
        surface,
        &VmConfig::untrusted(Ambient::deterministic(0), Limits::unlimited()),
    )
    .map_err(|err| diagnostics::config_validation(path.map(Path::to_path_buf), err))
}

/// Invoke the configured root mode once to validate its output shape.
fn validate_root(
    runtime: &mut Runtime,
    callbacks: &super::callback::SharedCallbackRegistry,
    root: &ModeRef,
    path: Option<&Path>,
    sources: &SourceMap,
) -> Result<(), Error> {
    let builder = ModeBuilder::new_for_render(false);
    let ctx = ModeCtx {
        window: None,
        hud: false,
        depth: 0,
    };
    let mut script_error = None;
    let options = LoadedConfig::entry_options();
    let mut context = super::callback::CallbackContext::new(Arc::clone(callbacks));
    let step = runtime.step_with_context(&mut context, &options, |scope| {
        let builder = mode_builder_userdata(scope, builder.clone())?;
        let ctx = mode_context_userdata(scope, ctx.clone())?;
        let root = root.func.resolve(scope)?;
        let result: Result<(), ScriptError<'_>> = scope.call_protected(root, (builder, ctx))?;
        if let Err(err) = result {
            script_error = Some(diagnostics::config_script_error(path, sources, scope, &err));
        }
        Ok(())
    });
    drop(builder);
    super::callback::CallbackRegistry::synchronize(callbacks, runtime)
        .map_err(|err| diagnostics::config_retained_error(path.map(Path::to_path_buf), &err))?;
    step.map_err(|err| diagnostics::config_retained_error(path.map(Path::to_path_buf), &err))?;

    if let Some(err) = script_error {
        return Err(err);
    }
    Ok(())
}
