//! Native Luau `hotki` library implementation.

use std::sync::Arc;

use ruau::{
    decl::DeclSource,
    vm::{
        ModuleBuilderExt, MultiValue, RuntimeError, Scope, ScopedHostFunction, ScopedValue, Table,
    },
    vm_api::{ModuleBinding, ModuleBuilder, NativeModule},
};

use super::{
    ModeRef, SelectorItem, apps, host_args::HostArgs, host_runtime::SharedRuntimeState,
    util::lock_unpoisoned,
};

/// Native module backing the `hotki` global library.
pub(super) struct HotkiModule {
    /// Shared loader state mutated by root and application functions.
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
            binding,
            Box::new(HotkiApplications {
                state: self.state.clone(),
            }),
        );
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
