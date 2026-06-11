//! Luau config loading orchestration.

use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use oxau::{
    compile::{self, CompileOptions},
    embed::ScriptError,
    profile::Profile,
    session::{Ambient, Vm},
};

use super::{
    DynamicConfig, ModeCtx, ModeRef,
    config::SourceMap,
    diagnostics,
    host_action::{ActionModule, action_value_type},
    host_hotki::HotkiModule,
    host_runtime::{RuntimeState, SharedRuntimeState, chunk_name},
    host_themes::ThemesModule,
    host_userdata::{
        ModeBuilder, action_context_type, mode_builder_type, mode_builder_userdata,
        mode_context_type, mode_context_userdata,
    },
    util::lock_unpoisoned,
};
use crate::{Error, themes};

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
    .map_err(|err| diagnostics::config_compile_error(source, &err, path.as_deref()))?;
    let mut vm = build_vm(profile, state.clone(), path.as_deref())?;
    let chunk_name = chunk_name(path.as_deref());
    let module = vm
        .load_named(&chunk, chunk_name.as_bytes())
        .map_err(|err| diagnostics::config_validation(path.clone(), err))?;

    match vm
        .call_protected_with_limits(&module, DynamicConfig::entry_limits())
        .map_err(|err| diagnostics::config_validation(path.clone(), format!("{err:?}")))?
    {
        Ok(_) => {}
        Err(err) => {
            return Err(diagnostics::config_protected_error(
                source,
                path.as_deref(),
                &sources,
                &err,
            ));
        }
    }

    let root = lock_unpoisoned(&state).root.clone().ok_or_else(|| {
        diagnostics::config_validation(path.clone(), "hotki.root() must be called exactly once")
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
        .map_err(|err| diagnostics::config_validation(path.map(Path::to_path_buf), err))
}

/// Invoke the configured root mode once to validate its output shape.
fn validate_root(
    vm: &mut Vm,
    root: &ModeRef,
    path: Option<&Path>,
    sources: &SourceMap,
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
            script_error = Some(diagnostics::config_script_error(path, sources, scope, &err));
        }
        Ok(())
    })
    .map_err(|err| diagnostics::config_validation(path.map(Path::to_path_buf), err.message()))?;

    if let Some(err) = script_error {
        return Err(err);
    }
    Ok(())
}
