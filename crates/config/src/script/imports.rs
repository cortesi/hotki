//! Shared Luau import roles and filesystem policy.

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

use oxau::embed::RuntimeError;

use crate::Error;

/// Role-specific import kinds accepted by the Luau host API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ImportRole {
    /// Imported mode renderer.
    Mode,
    /// Imported selector items provider or static list.
    Items,
    /// Imported action handler.
    Handler,
    /// Imported style overlay.
    Style,
}

impl ImportRole {
    /// All import roles exposed by the `hotki` host module.
    pub(crate) const ALL: [Self; 4] = [Self::Mode, Self::Items, Self::Handler, Self::Style];

    /// Return the host import function name for this role.
    pub(crate) fn function_name(self) -> &'static str {
        match self {
            Self::Mode => "import_mode",
            Self::Items => "import_items",
            Self::Handler => "import_handler",
            Self::Style => "import_style",
        }
    }

    /// Convert a Luau import function name into its role.
    pub(crate) fn from_function_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|role| role.function_name() == name)
    }

    /// Build a synthetic root config that validates one imported module in isolation.
    pub(crate) fn wrapper_source(self, rel_path: &Path) -> String {
        let rel_path = rel_path.to_string_lossy().replace('\\', "/");
        match self {
            Self::Mode => format!(
                "local imported = hotki.import_mode(\"{rel_path}\")\n\
                 hotki.root(imported)\n"
            ),
            Self::Items => format!(
                "local imported = hotki.import_items(\"{rel_path}\")\n\
                 hotki.root(function(menu, ctx)\n\
                 \tmenu:bind(\"a\", \"Select\", action.selector({{\n\
                 \t\titems = imported,\n\
                 \t\ton_select = function(actx, item, query)\n\
                 \t\tend,\n\
                 \t}}))\n\
                 end)\n"
            ),
            Self::Handler => format!(
                "local imported = hotki.import_handler(\"{rel_path}\")\n\
                 hotki.root(function(menu, ctx)\n\
                 \tmenu:bind(\"a\", \"Run\", action.run(imported))\n\
                 end)\n"
            ),
            Self::Style => format!(
                "local imported = hotki.import_style(\"{rel_path}\")\n\
                 hotki.root(function(menu, ctx)\n\
                 \tmenu:style(imported)\n\
                 end)\n"
            ),
        }
    }

    /// Build an analyzer harness that type-checks one imported module in isolation.
    pub(crate) fn analysis_source(self, module_source: &str) -> String {
        let target = match self {
            Self::Mode => "ModeModule",
            Self::Items => "ItemsProvider<any>",
            Self::Handler => "HandlerModule",
            Self::Style => "any",
        };
        format!("local _module = (function(): {target}\n{module_source}\nend)()\n")
    }

    /// Stable suffix used for synthetic checker wrapper paths.
    pub(crate) fn synthetic_suffix(self) -> &'static str {
        match self {
            Self::Mode => "mode",
            Self::Items => "items",
            Self::Handler => "handler",
            Self::Style => "style",
        }
    }
}

/// Import path resolution failure shared by checker and runtime import loading.
#[derive(Debug)]
pub enum ImportPathError {
    /// Absolute import paths are forbidden.
    Absolute,
    /// Parent traversal or root/prefix components are forbidden.
    ParentTraversal,
    /// Canonicalized import target escapes the config root.
    EscapesRoot,
    /// Filesystem resolution failed.
    Read {
        /// Candidate path that failed to resolve.
        path: PathBuf,
        /// Underlying IO error.
        source: io::Error,
    },
}

impl ImportPathError {
    /// Convert to the checker-facing config error shape.
    pub(crate) fn into_config_error(self, root_dir: &Path) -> Error {
        match self {
            Self::Absolute => validation_error(root_dir, "absolute import paths are not allowed"),
            Self::ParentTraversal => {
                validation_error(root_dir, "parent traversal is not allowed in imports")
            }
            Self::EscapesRoot => validation_error(root_dir, "import escapes the config directory"),
            Self::Read { path, source } => Error::Read {
                path: Some(path),
                message: source.to_string(),
            },
        }
    }

    /// Convert to the runtime loader error shape.
    pub(crate) fn into_runtime_error(self) -> RuntimeError {
        match self {
            Self::Absolute => RuntimeError::runtime("absolute import paths are not allowed"),
            Self::ParentTraversal => {
                RuntimeError::runtime("parent traversal is not allowed in imports")
            }
            Self::EscapesRoot => RuntimeError::runtime("import escapes the config directory"),
            Self::Read { source, .. } => RuntimeError::external(source),
        }
    }
}

/// Resolve a relative import path within the config root.
pub fn resolve_path(root_dir: &Path, raw_path: &str) -> Result<PathBuf, ImportPathError> {
    let path = Path::new(raw_path);
    if path.is_absolute() {
        return Err(ImportPathError::Absolute);
    }

    for component in path.components() {
        if matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        ) {
            return Err(ImportPathError::ParentTraversal);
        }
    }

    let candidate = if path.extension().is_some() {
        root_dir.join(path)
    } else {
        root_dir.join(path).with_extension("luau")
    };
    let root_canon = fs::canonicalize(root_dir).unwrap_or_else(|_| root_dir.to_path_buf());
    let canon = fs::canonicalize(&candidate).map_err(|source| ImportPathError::Read {
        path: candidate,
        source,
    })?;
    if !canon.starts_with(root_canon) {
        return Err(ImportPathError::EscapesRoot);
    }
    Ok(canon)
}

/// Build a validation error tied to the config root.
fn validation_error(root_dir: &Path, message: &str) -> Error {
    Error::Validation {
        path: Some(root_dir.to_path_buf()),
        line: None,
        col: None,
        message: message.to_string(),
        excerpt: None,
    }
}
