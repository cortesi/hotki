//! Luau config loading orchestration.

use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use ruau::{
    bytecode::CompileOptions,
    host::{RetainedLoadTarget, RetainedRuntime},
    surface::{Surface, VmConfig},
    vm::{Ambient, Limits, ScriptError},
};

use super::{
    DynamicConfig, ModeCtx, ModeRef,
    config::SourceMap,
    diagnostics,
    host_hotki::build_hotki_module,
    host_runtime::{RuntimeState, chunk_name},
    host_userdata::{ModeBuilder, mode_builder_userdata, mode_context_userdata},
    util::lock_unpoisoned,
};
use crate::{Error, StyleResolver};

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

    let state = RuntimeState::default();
    let resolved_style = match path.as_deref() {
        Some(path) => StyleResolver::from_config_path(path)?.resolve()?,
        None => StyleResolver::default_only()?.resolve()?,
    };

    let state = Arc::new(Mutex::new(state));
    let callbacks = DynamicConfig::callback_registry();
    let module = build_hotki_module(state.clone())
        .map_err(|err| diagnostics::config_validation(path.clone(), err))?;
    let surface = Surface::builder()
        .enable_runtime_compilation()
        .module(module)
        .build()
        .map_err(|err| diagnostics::config_validation(path.clone(), err))?;
    let chunk = surface
        .runtime_capabilities()
        .compile_source(source.as_bytes(), &CompileOptions::new())
        .map_err(|err| diagnostics::config_compile_error(source, &err, path.as_deref()))?;
    let mut runtime = build_runtime(surface, path.as_deref())?;
    let chunk_name = chunk_name(path.as_deref());
    let loaded = runtime
        .load_compiled(&chunk, &RetainedLoadTarget::named(chunk_name.into_bytes()))
        .map_err(|err| diagnostics::config_retained_error(path.clone(), &err))?;
    let mut context = super::callback::CallbackContext::new(Arc::clone(&callbacks));
    let options = DynamicConfig::entry_options();
    let mut script_error = None;
    let run = runtime.step_root_with_context(&loaded, &mut context, &options, |scope, root| {
        let result: Result<(), ScriptError<'_>> = scope.call_protected(root, ())?;
        if let Err(err) = result {
            script_error = Some(diagnostics::config_script_error(
                path.as_deref(),
                &sources,
                scope,
                &err,
            ));
        }
        Ok(())
    });
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

    let root = lock_unpoisoned(&state).root.clone().ok_or_else(|| {
        diagnostics::config_validation(path.clone(), "hotki.root() must be called exactly once")
    })?;
    validate_root(&mut runtime, &callbacks, &root, path.as_deref(), &sources)?;

    Ok(DynamicConfig::new(
        root,
        resolved_style.style,
        resolved_style.provenance,
        runtime,
        callbacks,
        path,
        sources,
    ))
}

/// Build the sandboxed retained runtime used by a dynamic config.
fn build_runtime(surface: Surface, path: Option<&Path>) -> Result<RetainedRuntime, Error> {
    RetainedRuntime::new(
        surface,
        &VmConfig::untrusted(Ambient::deterministic(0), Limits::unlimited()),
    )
    .map_err(|err| diagnostics::config_validation(path.map(Path::to_path_buf), err))
}

/// Invoke the configured root mode once to validate its output shape.
fn validate_root(
    runtime: &mut RetainedRuntime,
    callbacks: &super::callback::SharedCallbackRegistry,
    root: &ModeRef,
    path: Option<&Path>,
    sources: &SourceMap,
) -> Result<(), Error> {
    let builder = ModeBuilder::new_for_render(false);
    let ctx = ModeCtx {
        app: String::new(),
        title: String::new(),
        pid: 0,
        hud: false,
        depth: 0,
    };
    let mut script_error = None;
    let options = DynamicConfig::entry_options();
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
