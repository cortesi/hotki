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
    vm::{Ambient, CallOptions, ExecError, RuntimeCapabilities, ScriptError, Vm},
};

use super::{
    DynamicConfig, ModeCtx, ModeRef,
    config::SourceMap,
    diagnostics,
    host_hotki::HotkiModule,
    host_runtime::{RuntimeState, SharedRuntimeState, chunk_name},
    host_userdata::{
        ModeBuilder, action_context_type, mode_builder_type, mode_builder_userdata,
        mode_context_type, mode_context_userdata,
    },
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
    let runtime_capabilities = RuntimeCapabilities::default().enable_runtime_compilation();
    let chunk = runtime_capabilities
        .compile_source(source.as_bytes(), &CompileOptions::new())
        .map_err(|err| diagnostics::config_compile_error(source, &err, path.as_deref()))?;
    let mut vm = build_vm(runtime_capabilities, state.clone(), path.as_deref())?;
    let chunk_name = chunk_name(path.as_deref());
    let module = vm
        .load_named(&chunk, chunk_name.as_bytes())
        .map_err(|err| diagnostics::config_validation(path.clone(), err))?;

    match vm.exec(
        &module,
        CallOptions::new().limits(DynamicConfig::entry_limits()),
    ) {
        Ok(_) => {}
        Err(ExecError::Script(err)) => {
            return Err(diagnostics::config_protected_error(
                source,
                path.as_deref(),
                &sources,
                &err,
            ));
        }
        Err(err) => return Err(diagnostics::config_validation(path.clone(), err)),
    }
    vm.collect();

    let root = lock_unpoisoned(&state).root.clone().ok_or_else(|| {
        diagnostics::config_validation(path.clone(), "hotki.root() must be called exactly once")
    })?;
    validate_root(&mut vm, &root, path.as_deref(), &sources)?;
    vm.collect();

    Ok(DynamicConfig {
        root,
        base_style: resolved_style.style,
        style_provenance: resolved_style.provenance,
        vm,
        _root_module: module,
        path,
        sources,
    })
}

/// Build the sandboxed retained VM used by a dynamic config.
fn build_vm(
    runtime_capabilities: RuntimeCapabilities,
    state: SharedRuntimeState,
    path: Option<&Path>,
) -> Result<Vm, Error> {
    Vm::builder()
        .ambient(Ambient::deterministic(0))
        .limits(DynamicConfig::entry_limits())
        .runtime_capabilities(runtime_capabilities)
        .module(Arc::new(HotkiModule { state }))
        .host_type(mode_builder_type())
        .host_type(mode_context_type())
        .host_type(action_context_type())
        .sandboxed()
        .build()
        .map_err(|err| diagnostics::config_validation(path.map(Path::to_path_buf), err))
}

/// Invoke the configured root mode once to validate its output shape.
fn validate_root(
    vm: &mut Vm,
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
    vm.step_with(
        &CallOptions::new().limits(DynamicConfig::entry_limits()),
        |scope| {
            let builder = mode_builder_userdata(scope, builder.clone())?;
            let ctx = mode_context_userdata(scope, ctx.clone())?;
            let root = scope.fetch_function(&root.func)?;
            let result: Result<(), ScriptError<'_>> = scope.call_protected(root, (builder, ctx))?;
            if let Err(err) = result {
                script_error = Some(diagnostics::config_script_error(path, sources, scope, &err));
            }
            Ok(())
        },
    )
    .map_err(|err| diagnostics::config_validation(path.map(Path::to_path_buf), err.message()))?;

    if let Some(err) = script_error {
        return Err(err);
    }
    Ok(())
}
