//! Luau configuration validation helpers.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use oxau::{
    compile::{self, CompileError, CompileOptions},
    diagnostic::{DiagnosticLocation, DiagnosticSeverity, TypeDiagnostic},
    embed::{
        ModuleBinding, ModuleBuilder, ModuleBuilderExt, ModuleValue, MultiValue, NativeModule,
        RuntimeError, Scope, ScopedHostFunction,
    },
    profile::Profile,
    source::AnalysisMode,
    surface::SurfaceSpec,
    types::CheckerConfig,
};
use regex::Regex;

use crate::{
    Error,
    error::excerpt_at,
    luau_api,
    script::{
        imports::{self, ImportRole},
        load_dynamic_config_from_string,
    },
    themes,
};

/// Summary of a successful Luau validation run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LuauCheckReport {
    /// Number of imported role files validated in isolation.
    pub imports: usize,
    /// Number of user theme files validated from the sibling `themes/` directory.
    pub themes: usize,
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
        check_module(path, &ImportRole::Style.analysis_source(&source))?;
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
        let resolved = imports::resolve_path(root_dir, import_text.as_str())
            .map_err(|err| err.into_config_error(root_dir))?;
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

/// Collect all literal import calls matched by `re` into `out`, keyed by start offset.
fn collect_literal_imports(
    re: &Regex,
    source: &str,
    out: &mut BTreeMap<usize, (ImportRole, String)>,
) {
    for captures in re.captures_iter(source) {
        let Some(full) = captures.get(0) else {
            continue;
        };
        let Some(role_match) = captures.get(1) else {
            continue;
        };
        let Some(path_match) = captures.get(2) else {
            continue;
        };
        let Some(role) = ImportRole::from_function_name(role_match.as_str()) else {
            continue;
        };
        out.insert(full.start(), (role, path_match.as_str().to_string()));
    }
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
    collect_literal_imports(&literal_double, source, &mut literal_by_start);
    collect_literal_imports(&literal_single, source, &mut literal_by_start);

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
    check_module(path, source)
}

/// Analyze each imported module against its declared role alias.
fn analyze_imports(imports: &BTreeSet<ImportSpec>) -> Result<(), Error> {
    for import in imports {
        let source = fs::read_to_string(&import.path).map_err(|err| Error::Read {
            path: Some(import.path.clone()),
            message: err.to_string(),
        })?;
        check_module(&import.path, &import.role.analysis_source(&source))?;
    }
    Ok(())
}

/// Build a synthetic in-memory wrapper path rooted at the checked config directory.
fn synthetic_wrapper_path(root_dir: &Path, role: ImportRole) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let suffix = role.synthetic_suffix();
    root_dir.join(format!("__hotki_check_{suffix}_{id}.luau"))
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

/// Build the static Hotki script surface used by the oxau checker.
fn checker_surface() -> Result<SurfaceSpec, Error> {
    SurfaceSpec::builder(Profile::full().with_runtime_compilation())
        .module(Arc::new(StaticHotkiApiModule))
        .build()
        .map_err(|err| Error::Validation {
            path: None,
            line: None,
            col: None,
            message: err.to_string(),
            excerpt: None,
        })
}

/// API type aliases prepended to checked user modules so generic aliases are user-visible.
fn checker_type_prelude() -> &'static str {
    static PRELUDE: OnceLock<String> = OnceLock::new();
    PRELUDE.get_or_init(|| {
        luau_api()
            .split_once("\ndeclare hotki:")
            .map_or_else(|| luau_api().trim_end(), |(types, _)| types.trim_end())
            .to_string()
    })
}

/// Run oxau's checker and bytecode compiler on one source module.
fn check_module(path: &Path, source: &str) -> Result<(), Error> {
    let surface = checker_surface()?;
    let mut checker = surface.new_checker();
    let prelude = checker_type_prelude();
    let checked_source = format!("{prelude}\n{source}");
    let line_offset = prelude.lines().count();
    let checked = checker.check_source_bytes_with_config(
        checked_source.as_bytes(),
        CheckerConfig {
            default_mode: AnalysisMode::Nonstrict,
            source_mode_override: Some(AnalysisMode::Nonstrict),
            ..CheckerConfig::default()
        },
    );
    let errors = checked
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
        .cloned()
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(diagnostics_to_config(path, source, &errors, line_offset));
    }

    compile::compile_for(
        surface.profile(),
        source.as_bytes(),
        &CompileOptions::for_vm_execution(),
    )
    .map(|_| ())
    .map_err(|err| compile_error_to_config(path, source, &err))
}

/// Convert structured checker diagnostics into the stable config error shape.
fn diagnostics_to_config(
    path: &Path,
    source: &str,
    diagnostics: &[TypeDiagnostic],
    line_offset: usize,
) -> Error {
    let (line, col, excerpt) = diagnostics
        .first()
        .and_then(|diagnostic| source_position(diagnostic.primary_location, line_offset))
        .map(|(line, col)| (Some(line), Some(col), Some(excerpt_at(source, line, col))))
        .unwrap_or((None, None, None));
    Error::Validation {
        path: Some(path.to_path_buf()),
        line,
        col,
        message: render_diagnostics(path, diagnostics, line_offset),
        excerpt,
    }
}

/// Render checker diagnostics using user-source line numbers instead of prelude offsets.
fn render_diagnostics(path: &Path, diagnostics: &[TypeDiagnostic], line_offset: usize) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let site = source_position(diagnostic.primary_location, line_offset).map_or_else(
                || format!("{}:?:?", path.display()),
                |(line, col)| format!("{}:{}:{}", path.display(), line, col),
            );
            format!(
                "{} {}: {}",
                site,
                diagnostic.category,
                diagnostic
                    .context
                    .as_deref()
                    .unwrap_or("type checker diagnostic")
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Convert a checker source location into 1-based line and column coordinates.
fn source_position(location: DiagnosticLocation, line_offset: usize) -> Option<(usize, usize)> {
    if location == DiagnosticLocation::missing() {
        None
    } else {
        let line = location.begin.line as usize + 1;
        (line > line_offset).then_some((line - line_offset, location.begin.column as usize + 1))
    }
}

/// Convert a structured oxau compile error into a config error.
fn compile_error_to_config(path: &Path, source: &str, err: &CompileError) -> Error {
    let Some(location) = err.location() else {
        return Error::Validation {
            path: Some(path.to_path_buf()),
            line: None,
            col: None,
            message: err.message().to_string(),
            excerpt: None,
        };
    };

    let line = location.begin.line as usize + 1;
    let col = location.begin.column as usize + 1;
    Error::Parse {
        path: Some(path.to_path_buf()),
        line,
        col,
        message: err.message().to_string(),
        excerpt: excerpt_at(source, line, col),
    }
}

/// Checker-only native module that declares and audits the full Hotki host API surface.
struct StaticHotkiApiModule;

impl NativeModule for StaticHotkiApiModule {
    fn name(&self) -> &str {
        "hotki"
    }

    fn declaration(&self) -> &str {
        luau_api()
    }

    fn build(&self, builder: &mut dyn ModuleBuilder) {
        install_hotki_api_shape(builder);
        install_action_api_shape(builder);
        install_themes_api_shape(builder);
    }
}

/// Install the `hotki` library bindings for checker surface auditing.
fn install_hotki_api_shape(builder: &mut dyn ModuleBuilder) {
    let binding = ModuleBinding::library("hotki");
    for name in [
        "root",
        "applications",
        "import_mode",
        "import_items",
        "import_handler",
        "import_style",
    ] {
        builder.scoped_function(name, binding.clone(), Box::new(StaticHostFunction));
    }
}

/// Install the `action` library bindings for checker surface auditing.
fn install_action_api_shape(builder: &mut dyn ModuleBuilder) {
    let binding = ModuleBinding::library("action");
    for name in [
        "pop",
        "exit",
        "show_root",
        "hide_hud",
        "reload_config",
        "clear_notifications",
        "theme_next",
        "theme_prev",
    ] {
        builder.constant(
            name,
            binding.clone(),
            ModuleValue::LightUserdata { handle: 0, tag: 0 },
        );
    }
    for name in [
        "shell",
        "open",
        "relay",
        "show_details",
        "theme_set",
        "set_volume",
        "change_volume",
        "mute",
        "run",
        "selector",
    ] {
        builder.scoped_function(name, binding.clone(), Box::new(StaticHostFunction));
    }
}

/// Install the `themes` library bindings for checker surface auditing.
fn install_themes_api_shape(builder: &mut dyn ModuleBuilder) {
    let binding = ModuleBinding::library("themes");
    for name in ["use", "current", "list", "get", "register", "remove"] {
        builder.scoped_function(name, binding.clone(), Box::new(StaticHostFunction));
    }
}

/// Function placeholder used only for native-module shape auditing.
struct StaticHostFunction;

impl ScopedHostFunction for StaticHostFunction {
    fn call<'s>(
        &self,
        _scope: &Scope<'s>,
        _args: MultiValue<'s>,
    ) -> Result<MultiValue<'s>, RuntimeError> {
        Err(RuntimeError::runtime("checker-only Hotki API surface"))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsStr,
        fs,
        os::unix::fs as unix_fs,
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
    fn check_rejects_imports_that_escape_root_via_symlink() {
        let root = test_dir("escaped-import");
        let outside = test_dir("escaped-import-target");
        fs::write(
            root.join("config.luau"),
            r#"
local child = hotki.import_mode("alias")

hotki.root(child)
"#,
        )
        .expect("write root config");
        fs::write(
            outside.join("child.luau"),
            "return function(menu, ctx)\nend\n",
        )
        .expect("write external child import");
        unix_fs::symlink(outside.join("child.luau"), root.join("alias.luau"))
            .expect("symlink external child import");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        assert!(err.pretty().contains("import escapes the config directory"));
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
