//! Native Luau `hotki` library implementation.

use std::{fs, path::PathBuf, sync::Arc};

use oxau::{
    decl::DeclSource,
    embed::{
        ModuleBinding, ModuleBuilder, ModuleBuilderExt, MultiValue, NativeModule, RuntimeError,
        Scope, ScopedHostFunction, ScopedValue, Table, serde::to_scoped_value,
    },
};

use super::{
    HandlerRef, ModeRef, SelectorItem, apps, diagnostics,
    host_args::{HostArgs, expect_function_value, single_return},
    host_parse::parse_raw_style,
    host_runtime::{
        ImportedItems, ImportedValue, RuntimeState, SharedRuntimeState, chunk_name, clone_sources,
    },
    imports::{self, ImportRole},
    selector,
    util::lock_unpoisoned,
};

/// Native module backing the `hotki` global library.
pub(super) struct HotkiModule {
    /// Shared loader state mutated by root, application, and import functions.
    pub(super) state: SharedRuntimeState,
}

impl NativeModule for HotkiModule {
    fn name(&self) -> &str {
        "hotki"
    }

    fn declaration(&self) -> DeclSource<'_> {
        DeclSource::Text(crate::luau_api())
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
        let mut args = HostArgs::new(args);
        let render = args.function("hotki.root render")?;
        args.finish("hotki.root")?;
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
            let shared: Arc<[SelectorItem]> = apps.into();
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
        let mut args = HostArgs::new(args);
        let path = args.string(scope, "import path")?;
        args.finish(self.role.function_name())?;
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
    let sources = clone_sources(state);
    lock_unpoisoned(&sources).insert(resolved.clone(), Arc::from(source.clone().into_boxed_str()));

    let chunk = scope.load_chunk(source.as_bytes(), chunk_name(Some(&resolved)).as_bytes())?;
    let results = match scope.call_protected::<_, MultiValue<'s>>(chunk, ())? {
        Ok(results) => results,
        Err(err) => {
            let error = diagnostics::config_script_error(Some(&resolved), &sources, scope, &err);
            return Err(diagnostics::config_error_payload(error));
        }
    };
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
            let func = expect_function_value(value, "import_mode return value")?;
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
            let func = expect_function_value(value, "import_handler return value")?;
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

/// Resolve a role import path within the current config directory.
fn resolve_import_path(state: &RuntimeState, raw_path: &str) -> Result<PathBuf, RuntimeError> {
    let root = state
        .config_dir
        .as_deref()
        .ok_or_else(|| RuntimeError::runtime("imports require a filesystem-backed config"))?;
    imports::resolve_path(root, raw_path).map_err(|err| err.into_runtime_error())
}

/// Convert selector items into a Luau array table.
fn selector_items_table<'s>(
    scope: &Scope<'s>,
    items: &[SelectorItem],
) -> Result<Table<'s>, RuntimeError> {
    let table = scope.create_table()?;
    for (idx, item) in items.iter().enumerate() {
        let row = scope.create_table()?;
        row.set(scope, "label", item.label.clone())?;
        row.set(scope, "sublabel", item.sublabel.clone())?;
        row.set(scope, "data", item.data.fetch(scope)?)?;
        table.set(scope, (idx + 1) as f64, row)?;
    }
    Ok(table)
}
