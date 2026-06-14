//! Native Luau `themes` library implementation.

use oxau::{
    decl::DeclSource,
    embed::{
        IntoLuaMulti, ModuleBinding, ModuleBuilder, ModuleBuilderExt, MultiValue, NativeModule,
        RuntimeError, Scope, ScopedHostFunction, serde::to_scoped_value,
    },
};

use super::{
    host_args::HostArgs, host_parse::parse_raw_style, host_runtime::SharedRuntimeState,
    util::lock_unpoisoned,
};

/// Native module backing the `themes` global library.
pub(super) struct ThemesModule {
    /// Shared loader state containing the theme registry and active selection.
    pub(super) state: SharedRuntimeState,
}

impl NativeModule for ThemesModule {
    fn name(&self) -> &str {
        "themes"
    }

    fn declaration(&self) -> DeclSource<'_> {
        DeclSource::Text(crate::luau_api())
    }

    fn build(&self, builder: &mut dyn ModuleBuilder) {
        let binding = ModuleBinding::library("themes");
        for kind in [
            ThemesFunctionKind::Use,
            ThemesFunctionKind::Current,
            ThemesFunctionKind::List,
            ThemesFunctionKind::Get,
            ThemesFunctionKind::Register,
            ThemesFunctionKind::Remove,
        ] {
            builder.scoped_function(
                kind.name(),
                binding.clone(),
                Box::new(ThemesFunction {
                    state: self.state.clone(),
                    kind,
                }),
            );
        }
    }
}

/// Host methods exposed on the `themes` global library.
#[derive(Clone, Copy)]
enum ThemesFunctionKind {
    /// Select the active theme.
    Use,
    /// Return the active theme name.
    Current,
    /// List known theme names.
    List,
    /// Return one theme style overlay.
    Get,
    /// Register or replace a script-defined theme.
    Register,
    /// Remove a script-defined theme.
    Remove,
}

impl ThemesFunctionKind {
    /// Return the method name installed in the `themes` library.
    fn name(self) -> &'static str {
        match self {
            Self::Use => "use",
            Self::Current => "current",
            Self::List => "list",
            Self::Get => "get",
            Self::Register => "register",
            Self::Remove => "remove",
        }
    }
}

/// Host implementation shared by the `themes` library methods.
struct ThemesFunction {
    /// Shared loader state containing theme data.
    state: SharedRuntimeState,
    /// Concrete theme operation to execute.
    kind: ThemesFunctionKind,
}

impl ScopedHostFunction for ThemesFunction {
    fn call<'s>(
        &self,
        scope: &Scope<'s>,
        args: MultiValue<'s>,
    ) -> Result<MultiValue<'s>, RuntimeError> {
        let mut args = HostArgs::method(args);
        match self.kind {
            ThemesFunctionKind::Use => {
                let name = args.string(scope, "themes:use name")?;
                args.finish("themes:use")?;
                let mut guard = lock_unpoisoned(&self.state);
                if !guard.themes.contains_key(name.as_str()) {
                    return Err(RuntimeError::runtime(format!("unknown theme: {name}")));
                }
                guard.active_theme = name;
                Ok(MultiValue::new())
            }
            ThemesFunctionKind::Current => {
                args.finish("themes:current")?;
                lock_unpoisoned(&self.state)
                    .active_theme
                    .clone()
                    .into_lua_multi(scope)
            }
            ThemesFunctionKind::List => {
                args.finish("themes:list")?;
                let mut names = lock_unpoisoned(&self.state)
                    .themes
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>();
                names.sort();
                names.into_lua_multi(scope)
            }
            ThemesFunctionKind::Get => {
                let name = args.string(scope, "themes:get name")?;
                args.finish("themes:get")?;
                let raw = lock_unpoisoned(&self.state)
                    .themes
                    .get(name.as_str())
                    .cloned()
                    .ok_or_else(|| RuntimeError::runtime(format!("unknown theme: {name}")))?;
                Ok(MultiValue::from_values(vec![to_scoped_value(scope, &raw)?]))
            }
            ThemesFunctionKind::Register => {
                let name = args.string(scope, "themes:register name")?;
                let style = args.required_with_message("themes:register expects a style")?;
                args.finish("themes:register")?;
                let raw = parse_raw_style(scope, style)?;
                lock_unpoisoned(&self.state).themes.insert(name, raw);
                Ok(MultiValue::new())
            }
            ThemesFunctionKind::Remove => {
                let name = args.string(scope, "themes:remove name")?;
                args.finish("themes:remove")?;
                let mut guard = lock_unpoisoned(&self.state);
                if name == "default" {
                    return Err(RuntimeError::runtime(
                        "themes.remove: cannot remove 'default'",
                    ));
                }
                if guard.themes.remove(name.as_str()).is_none() {
                    return Err(RuntimeError::runtime(format!(
                        "themes.remove: unknown theme: {name}"
                    )));
                }
                if guard.active_theme == name {
                    guard.active_theme = "default".to_string();
                }
                Ok(MultiValue::new())
            }
        }
    }
}
