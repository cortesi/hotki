//! Native Luau `hotki` library implementation.

use std::sync::Arc;

use ruau::{
    declaration::DeclarationSource,
    module::{self, Binding},
    vm::{MultiValue, NativeModule, RuntimeError, Scope, ScopedValue, Table},
};

use super::{SelectorItem, apps, host_runtime::SharedApplicationCache, util::lock_unpoisoned};

/// Pure-Luau implementation installed as the typed `hotki.actions` value.
const ACTIONS_SOURCE: &[u8] = include_bytes!("../../luau/actions.luau");

/// Build the declaration-coupled native module backing the `hotki` library.
pub(super) fn build_hotki_module(
    applications: SharedApplicationCache,
) -> Result<Arc<dyn NativeModule>, module::BuildError> {
    let mut builder =
        module::Builder::from_declaration("hotki", DeclarationSource::Text(crate::luau_api()));
    builder.source_value(
        "actions",
        Binding::declared_library("hotki"),
        ACTIONS_SOURCE,
    );
    builder.borrowed_function(
        "applications",
        Binding::declared_library("hotki"),
        move |scope, args| hotki_applications(&applications, scope, args),
    );
    builder.declared_host_type(Arc::new(super::host_userdata::mode_builder_type()));
    builder.declared_host_type(Arc::new(super::host_userdata::mode_context_type()));
    builder.declared_host_type(Arc::new(super::host_userdata::action_context_type()));
    builder.build()
}

/// Host implementation of `hotki.applications`.
fn hotki_applications<'s>(
    applications: &SharedApplicationCache,
    scope: &Scope<'s>,
    _args: MultiValue<'s>,
) -> Result<MultiValue<'s>, RuntimeError> {
    let cached = { lock_unpoisoned(applications).items.clone() };
    let items = if let Some(cached) = cached {
        cached
    } else {
        let apps = apps::application_items(scope)?;
        let shared: Arc<[SelectorItem]> = apps.into();
        lock_unpoisoned(applications).items = Some(shared.clone());
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
