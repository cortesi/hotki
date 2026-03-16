//! Luau configuration validation helpers.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use luau_analyze::{CheckOptions, Checker};
use regex::Regex;

use crate::{Error, error::excerpt_at, luau_api, script::load_dynamic_config_from_string, themes};

/// Timeout applied to one analyzer check invocation.
const ANALYZE_TIMEOUT: Duration = Duration::from_secs(2);

/// Summary of a successful Luau validation run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LuauCheckReport {
    /// Number of imported role files validated in isolation.
    pub imports: usize,
    /// Number of user theme files validated from the sibling `themes/` directory.
    pub themes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
/// Role-specific import kinds supported by the Luau host API.
enum ImportRole {
    /// Imported mode renderer.
    Mode,
    /// Imported selector item provider or static item array.
    Items,
    /// Imported action handler closure.
    Handler,
    /// Imported style overlay module.
    Style,
}

impl ImportRole {
    /// Convert a Luau import function name into its role.
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "import_mode" => Some(Self::Mode),
            "import_items" => Some(Self::Items),
            "import_handler" => Some(Self::Handler),
            "import_style" => Some(Self::Style),
            _ => None,
        }
    }

    /// Build a synthetic root config that validates one imported module in isolation.
    fn wrapper_source(self, rel_path: &Path) -> String {
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
    fn analysis_source(self, module_source: &str) -> String {
        let target = match self {
            Self::Mode => "ModeModule",
            Self::Items => "ItemsProvider<any>",
            Self::Handler => "HandlerModule",
            Self::Style => "StyleModule",
        };
        format!("local _module = (function(): {target}\n{module_source}\nend)()\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
/// One discovered import edge in the checked config graph.
struct ImportSpec {
    /// Expected role for the imported file.
    role: ImportRole,
    /// Canonical filesystem path to the imported module.
    path: PathBuf,
}

/// Validate a filesystem-backed Luau config, its reachable imports, and any sibling user themes.
pub fn check_luau_config(path: &Path) -> Result<LuauCheckReport, Error> {
    let canonical = fs::canonicalize(path).map_err(|err| Error::Read {
        path: Some(path.to_path_buf()),
        message: err.to_string(),
    })?;
    let root_dir = canonical
        .parent()
        .ok_or_else(|| Error::Read {
            path: Some(canonical.clone()),
            message: "config path must have a parent directory".to_string(),
        })?
        .to_path_buf();
    let source = fs::read_to_string(&canonical).map_err(|err| Error::Read {
        path: Some(canonical.clone()),
        message: err.to_string(),
    })?;
    let imports = discover_imports(&canonical, &root_dir)?;
    analyze_root_config(&canonical, &source)?;
    analyze_imports(&imports)?;
    let theme_count = check_luau_theme_dir(&root_dir.join("themes"))?;

    crate::load_dynamic_config(&canonical)?;
    validate_imports(&imports, &root_dir)?;

    Ok(LuauCheckReport {
        imports: imports.len(),
        themes: theme_count,
    })
}

/// Analyze and validate every `*.luau` theme file in `dir`.
pub fn check_luau_theme_dir(dir: &Path) -> Result<usize, Error> {
    let mut count = 0usize;
    if !dir.exists() {
        return Ok(0);
    }

    let mut paths = fs::read_dir(dir)
        .map_err(|err| Error::Read {
            path: Some(dir.to_path_buf()),
            message: err.to_string(),
        })?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "luau"))
        .collect::<Vec<_>>();
    paths.sort();

    for path in &paths {
        let source = fs::read_to_string(path).map_err(|err| Error::Read {
            path: Some(path.clone()),
            message: err.to_string(),
        })?;
        analyze_module(path, &ImportRole::Style.analysis_source(&source))?;
        count += 1;
    }

    themes::validate_theme_dir(dir)?;
    Ok(count)
}

/// Discover every reachable role-specific import from `path`.
fn discover_imports(path: &Path, root_dir: &Path) -> Result<BTreeSet<ImportSpec>, Error> {
    let mut imports = BTreeSet::new();
    let mut visited = BTreeSet::new();
    visit_imports(path, root_dir, &mut visited, &mut imports)?;
    Ok(imports)
}

/// Walk one Luau source file, collecting its imports recursively.
fn visit_imports(
    path: &Path,
    root_dir: &Path,
    visited: &mut BTreeSet<PathBuf>,
    imports: &mut BTreeSet<ImportSpec>,
) -> Result<(), Error> {
    let canonical = fs::canonicalize(path).map_err(|err| Error::Read {
        path: Some(path.to_path_buf()),
        message: err.to_string(),
    })?;
    if !visited.insert(canonical.clone()) {
        return Ok(());
    }

    let source = fs::read_to_string(&canonical).map_err(|err| Error::Read {
        path: Some(canonical.clone()),
        message: err.to_string(),
    })?;

    for (role, import_text, offset) in parse_import_calls(&source, &canonical)? {
        let resolved = resolve_import_path(root_dir, import_text.as_str())?;
        let spec = ImportSpec {
            role,
            path: resolved.clone(),
        };
        imports.insert(spec);
        visit_imports(&resolved, root_dir, visited, imports).map_err(|err| {
            if err.path().is_some() {
                err
            } else {
                error_at(
                    &canonical,
                    &source,
                    offset,
                    format!(
                        "failed to validate import '{}': {}",
                        import_text,
                        err.pretty()
                    ),
                )
            }
        })?;
    }

    Ok(())
}

/// Parse literal `hotki.import_*("...")` calls from a Luau source file.
fn parse_import_calls(
    source: &str,
    path: &Path,
) -> Result<Vec<(ImportRole, String, usize)>, Error> {
    let any = Regex::new(r#"hotki\.(import_mode|import_items|import_handler|import_style)\s*\("#)
        .expect("valid import matcher");
    let literal_double = Regex::new(
        r#"(?s)hotki\.(import_mode|import_items|import_handler|import_style)\s*\(\s*"([^"\\\r\n]+)"\s*\)"#,
    )
    .expect("valid double-quoted import matcher");
    let literal_single = Regex::new(
        r#"(?s)hotki\.(import_mode|import_items|import_handler|import_style)\s*\(\s*'([^'\\\r\n]+)'\s*\)"#,
    )
    .expect("valid single-quoted import matcher");

    let mut literal_by_start = BTreeMap::new();
    for captures in literal_double.captures_iter(source) {
        let Some(full) = captures.get(0) else {
            continue;
        };
        let Some(role_match) = captures.get(1) else {
            continue;
        };
        let Some(path_match) = captures.get(2) else {
            continue;
        };
        let Some(role) = ImportRole::from_name(role_match.as_str()) else {
            continue;
        };
        literal_by_start.insert(full.start(), (role, path_match.as_str().to_string()));
    }
    for captures in literal_single.captures_iter(source) {
        let Some(full) = captures.get(0) else {
            continue;
        };
        let Some(role_match) = captures.get(1) else {
            continue;
        };
        let Some(path_match) = captures.get(2) else {
            continue;
        };
        let Some(role) = ImportRole::from_name(role_match.as_str()) else {
            continue;
        };
        literal_by_start.insert(full.start(), (role, path_match.as_str().to_string()));
    }

    let mut imports = Vec::new();
    for import_call in any.find_iter(source) {
        let Some((role, import_path)) = literal_by_start.get(&import_call.start()).cloned() else {
            return Err(error_at(
                path,
                source,
                import_call.start(),
                "hotki imports must use literal relative path strings".to_string(),
            ));
        };
        imports.push((role, import_path, import_call.start()));
    }
    Ok(imports)
}

/// Validate each imported file by wrapping it in a synthetic root config.
fn validate_imports(imports: &BTreeSet<ImportSpec>, root_dir: &Path) -> Result<(), Error> {
    for import in imports {
        let rel_path = import
            .path
            .strip_prefix(root_dir)
            .map_err(|_| Error::Validation {
                path: Some(import.path.clone()),
                line: None,
                col: None,
                message: "import resolved outside the config root".to_string(),
                excerpt: None,
            })?;
        let wrapper_path = synthetic_wrapper_path(root_dir, import.role);
        let wrapper_source = import.role.wrapper_source(rel_path);
        load_dynamic_config_from_string(&wrapper_source, Some(wrapper_path))?;
    }
    Ok(())
}

/// Analyze the root config source against the checked-in Luau API definitions.
fn analyze_root_config(path: &Path, source: &str) -> Result<(), Error> {
    analyze_module(path, source)
}

/// Analyze each imported module against its declared role alias.
fn analyze_imports(imports: &BTreeSet<ImportSpec>) -> Result<(), Error> {
    for import in imports {
        let source = fs::read_to_string(&import.path).map_err(|err| Error::Read {
            path: Some(import.path.clone()),
            message: err.to_string(),
        })?;
        analyze_module(&import.path, &import.role.analysis_source(&source))?;
    }
    Ok(())
}

/// Build a synthetic in-memory wrapper path rooted at the checked config directory.
fn synthetic_wrapper_path(root_dir: &Path, role: ImportRole) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let suffix = match role {
        ImportRole::Mode => "mode",
        ImportRole::Items => "items",
        ImportRole::Handler => "handler",
        ImportRole::Style => "style",
    };
    root_dir.join(format!("__hotki_check_{suffix}_{id}.luau"))
}

/// Resolve a relative import path using the same filesystem rules as the runtime loader.
fn resolve_import_path(root_dir: &Path, raw_path: &str) -> Result<PathBuf, Error> {
    let path = Path::new(raw_path);
    if path.is_absolute() {
        return Err(Error::Validation {
            path: Some(root_dir.to_path_buf()),
            line: None,
            col: None,
            message: "absolute import paths are not allowed".to_string(),
            excerpt: None,
        });
    }

    for component in path.components() {
        if matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        ) {
            return Err(Error::Validation {
                path: Some(root_dir.to_path_buf()),
                line: None,
                col: None,
                message: "parent traversal is not allowed in imports".to_string(),
                excerpt: None,
            });
        }
    }

    let candidate = if path.extension().is_some() {
        root_dir.join(path)
    } else {
        root_dir.join(path).with_extension("luau")
    };
    fs::canonicalize(&candidate).map_err(|err| Error::Read {
        path: Some(candidate),
        message: err.to_string(),
    })
}

/// Attach a source location and excerpt to a validation error at `offset`.
fn error_at(path: &Path, source: &str, offset: usize, message: String) -> Error {
    let (line, col) = line_col_at(source, offset);
    Error::Validation {
        path: Some(path.to_path_buf()),
        line: Some(line),
        col: Some(col),
        message,
        excerpt: Some(excerpt_at(source, line, col)),
    }
}

/// Convert a byte offset into 1-based line and column coordinates.
fn line_col_at(source: &str, offset: usize) -> (usize, usize) {
    let clamped = offset.min(source.len());
    let prefix = &source[..clamped];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let col = prefix
        .rsplit_once('\n')
        .map_or(prefix.chars().count() + 1, |(_, tail)| {
            tail.chars().count() + 1
        });
    (line, col)
}

/// Create a fresh Luau analyzer with the checked-in Hotki API definitions loaded.
fn new_checker() -> Result<Checker, Error> {
    let mut checker = Checker::new().map_err(|err| Error::Validation {
        path: None,
        line: None,
        col: None,
        message: err.to_string(),
        excerpt: None,
    })?;
    checker
        .add_definitions(luau_api())
        .map_err(|err| Error::Validation {
            path: None,
            line: None,
            col: None,
            message: err.to_string(),
            excerpt: None,
        })?;
    Ok(checker)
}

/// Run the analyzer on one source module and convert diagnostics into `config::Error`.
fn analyze_module(path: &Path, source: &str) -> Result<(), Error> {
    let mut checker = new_checker()?;
    let module_name = path.to_string_lossy();
    let result = checker.check_with_options(
        source,
        CheckOptions {
            timeout: Some(ANALYZE_TIMEOUT),
            module_name: Some(module_name.as_ref()),
            cancellation_token: None,
        },
    );

    if result.timed_out() {
        return Err(Error::Validation {
            path: Some(path.to_path_buf()),
            line: None,
            col: None,
            message: "Luau analysis timed out".to_string(),
            excerpt: None,
        });
    }

    let errors = result.errors();
    let Some(first) = errors.first() else {
        return Ok(());
    };
    let line = usize::try_from(first.line).unwrap_or(0) + 1;
    let col = usize::try_from(first.col).unwrap_or(0) + 1;
    let message = result
        .diagnostics
        .iter()
        .map(|diag| {
            let severity = match diag.severity {
                luau_analyze::Severity::Error => "error",
                luau_analyze::Severity::Warning => "warning",
            };
            format!(
                "{}:{}:{}: {}: {}",
                path.display(),
                diag.line + 1,
                diag.col + 1,
                severity,
                diag.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Err(Error::Validation {
        path: Some(path.to_path_buf()),
        line: Some(line),
        col: Some(col),
        message,
        excerpt: Some(excerpt_at(source, line, col)),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsStr,
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::check_luau_config;

    fn test_dir(name: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tmp")
            .join(format!("config-check-{name}-{id}"));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale tmp dir");
        }
        fs::create_dir_all(&root).expect("create tmp dir");
        root
    }

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn check_validates_nested_role_imports() {
        let root = test_dir("nested-imports");
        fs::write(
            root.join("config.luau"),
            r#"
local child = hotki.import_mode("child")

hotki.root(function(menu, ctx)
    menu:submenu("a", "Child", child)
end)
"#,
        )
        .expect("write root config");
        fs::write(
            root.join("child.luau"),
            r#"
local handler = hotki.import_handler("handler")

return function(menu, ctx)
    menu:bind("b", "Run", action.run(handler))
end
"#,
        )
        .expect("write child import");
        fs::write(
            root.join("handler.luau"),
            r#"
return function(actx)
    actx:notify("info", "ok", "done")
end
"#,
        )
        .expect("write handler import");

        let report = check_luau_config(&root.join("config.luau")).expect("check config");
        assert_eq!(report.imports, 2);
        assert_eq!(report.themes, 0);
    }

    #[test]
    fn check_rejects_computed_imports() {
        let root = test_dir("computed-import");
        fs::write(
            root.join("config.luau"),
            r#"
local part = "child"
local child = hotki.import_mode(part)

hotki.root(function(menu, ctx)
    menu:submenu("a", "Child", child)
end)
"#,
        )
        .expect("write root config");
        fs::write(root.join("child.luau"), "return function(menu, ctx)\nend\n")
            .expect("write child import");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("literal relative path strings"));
        assert!(pretty.contains("config.luau"));
    }

    #[test]
    fn check_validates_user_themes() {
        let root = test_dir("theme-dir");
        fs::create_dir_all(root.join("themes")).expect("create theme dir");
        fs::write(
            root.join("config.luau"),
            r#"
themes:use("custom")
hotki.root(function(menu, ctx) end)
"#,
        )
        .expect("write root config");
        fs::write(
            root.join("themes/custom.luau"),
            r##"return { hud = { bg = "#010203" } }"##,
        )
        .expect("write theme");

        let report = check_luau_config(&root.join("config.luau")).expect("check config");
        assert_eq!(report.themes, 1);
    }

    #[test]
    fn check_validates_all_workspace_examples() {
        let examples_dir = workspace_root().join("examples");
        let mut example_paths = fs::read_dir(&examples_dir)
            .expect("read examples dir")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension() == Some(OsStr::new("luau")))
            .collect::<Vec<_>>();
        example_paths.sort();

        assert!(
            !example_paths.is_empty(),
            "no Luau examples found in {}",
            examples_dir.display()
        );

        for path in &example_paths {
            if let Err(err) = check_luau_config(path) {
                panic!(
                    "failed to validate example {}:\n{}",
                    path.display(),
                    err.pretty()
                );
            }
        }
    }
}
