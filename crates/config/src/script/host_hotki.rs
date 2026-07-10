//! Native Luau `hotki` library implementation.

use std::sync::Arc;

use ruau::{
    decl::DeclSource,
    module::{NativeBinding, NativeModuleBuilder, NativeModuleBuilderError},
    vm::{MultiValue, RuntimeError, Scope, ScopedValue, Table},
    vm_api::NativeModule,
};

use super::{
    ModeRef, SelectorItem, apps, host_args::HostArgs, host_runtime::SharedRuntimeState,
    util::lock_unpoisoned,
};

/// Build the declaration-coupled native module backing the `hotki` library.
pub(super) fn build_hotki_module(
    state: SharedRuntimeState,
) -> Result<Arc<dyn NativeModule>, NativeModuleBuilderError> {
    let mut builder =
        NativeModuleBuilder::from_declaration("hotki", DeclSource::Text(crate::luau_api()));
    let root_state = Arc::clone(&state);
    builder.borrowed_function(
        "root",
        NativeBinding::declared_library("hotki"),
        move |scope, args| hotki_root(&root_state, scope, args),
    );
    builder.borrowed_function(
        "applications",
        NativeBinding::declared_library("hotki"),
        move |scope, args| hotki_applications(&state, scope, args),
    );
    builder.declared_host_type(Arc::new(super::host_userdata::mode_builder_type()));
    builder.declared_host_type(Arc::new(super::host_userdata::mode_context_type()));
    builder.declared_host_type(Arc::new(super::host_userdata::action_context_type()));
    builder.build()
}

/// Host implementation of `hotki.root`.
fn hotki_root<'s>(
    state: &SharedRuntimeState,
    scope: &Scope<'s>,
    args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let mut args = HostArgs::new(args);
    let render = args.function("hotki.root render")?;
    args.finish("hotki.root")?;
    let mode = ModeRef::from_function(scope, render, None)?;
    let mut guard = lock_unpoisoned(state);
    if guard.root.is_some() {
        return Err(RuntimeError::runtime(
            "hotki.root() must be called exactly once",
        ));
    }
    guard.root = Some(mode);
    Ok(MultiValue::new())
}

/// Host implementation of `hotki.applications`.
fn hotki_applications<'s>(
    state: &SharedRuntimeState,
    scope: &Scope<'s>,
    _args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let cached = { lock_unpoisoned(state).applications_cache.clone() };
    let items = if let Some(cached) = cached {
        cached
    } else {
        let apps = apps::application_items(scope)?;
        let shared: Arc<[SelectorItem]> = apps.into();
        lock_unpoisoned(state).applications_cache = Some(shared.clone());
        shared
    };
    let table = selector_items_table(scope, items.as_ref())?;
    Ok(MultiValue::from_values(vec![ScopedValue::Table(table)]))
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
