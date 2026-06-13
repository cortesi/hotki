//! Luau configuration validation helpers.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use oxau::{
    compile::{self, CompileOptions},
    diagnostic::DiagnosticSeverity,
    embed::{
        ModuleBinding, ModuleBuilder, ModuleBuilderExt, ModuleValue, MultiValue, NativeModule,
        RuntimeError, Scope, ScopedHostFunction,
    },
    profile::Profile,
    source::AnalysisMode,
    surface::SurfaceSpec,
    types::CheckerConfig,
};

use crate::{
    Error, luau_api,
    script::{
        diagnostics,
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
                diagnostics::config_error_at_offset(
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
    let mut cursor = 0;
    let mut imports = Vec::new();

    while cursor < source.len() {
        if let Some(next) = skip_ignored_luau(source, cursor) {
            cursor = next;
            continue;
        }

        let Some((role, after_name)) = import_role_at(source, cursor) else {
            cursor = next_char_boundary(source, cursor);
            continue;
        };
        let Some(open_paren) = skip_whitespace(source, after_name).and_then(|next| {
            source[next..]
                .starts_with('(')
                .then_some(next + '('.len_utf8())
        }) else {
            cursor = next_char_boundary(source, cursor);
            continue;
        };
        let Some((import_path, after_literal)) = parse_import_literal(
            source,
            skip_whitespace(source, open_paren).unwrap_or(open_paren),
        ) else {
            return Err(import_literal_error(path, source, cursor));
        };
        let Some(after_args) = skip_whitespace(source, after_literal) else {
            return Err(import_literal_error(path, source, cursor));
        };
        if !source[after_args..].starts_with(')') {
            return Err(import_literal_error(path, source, cursor));
        }
        imports.push((role, import_path, cursor));
        cursor = after_args + ')'.len_utf8();
    }

    Ok(imports)
}

/// Build the checker error used when an import call is not a single literal string.
fn import_literal_error(path: &Path, source: &str, offset: usize) -> Error {
    diagnostics::config_error_at_offset(
        path,
        source,
        offset,
        "hotki imports must use literal relative path strings".to_string(),
    )
}

/// Return the import role and the offset after its function name at `offset`.
fn import_role_at(source: &str, offset: usize) -> Option<(ImportRole, usize)> {
    let after_prefix = source[offset..].strip_prefix("hotki.")?;
    let name_offset = offset + "hotki.".len();
    for role in ImportRole::ALL {
        let name = role.function_name();
        if !after_prefix.starts_with(name) {
            continue;
        }
        let after_name = name_offset + name.len();
        if source[after_name..]
            .chars()
            .next()
            .is_some_and(is_identifier_continue)
        {
            continue;
        }
        return Some((role, after_name));
    }
    None
}

/// Parse one short quoted import literal without escapes or newlines.
fn parse_import_literal(source: &str, offset: usize) -> Option<(String, usize)> {
    let quote = match source[offset..].chars().next()? {
        '"' => '"',
        '\'' => '\'',
        _ => return None,
    };
    let mut cursor = offset + quote.len_utf8();
    let mut value = String::new();

    while cursor < source.len() {
        let ch = source[cursor..].chars().next()?;
        if ch == quote {
            return (!value.is_empty()).then_some((value, cursor + ch.len_utf8()));
        }
        if matches!(ch, '\\' | '\r' | '\n') {
            return None;
        }
        value.push(ch);
        cursor += ch.len_utf8();
    }

    None
}

/// Skip whitespace from `offset`, returning `None` when already at end of source.
fn skip_whitespace(source: &str, mut offset: usize) -> Option<usize> {
    while offset < source.len() {
        let ch = source[offset..].chars().next()?;
        if !ch.is_whitespace() {
            break;
        }
        offset += ch.len_utf8();
    }
    (offset < source.len()).then_some(offset)
}

/// Skip comments and strings that should not be scanned for import calls.
fn skip_ignored_luau(source: &str, offset: usize) -> Option<usize> {
    if source[offset..].starts_with("--") {
        if let Some(end) = long_bracket_end(source, offset + "--".len()) {
            return Some(end);
        }
        return Some(skip_line_comment(source, offset + "--".len()));
    }

    match source[offset..].chars().next()? {
        '"' => Some(skip_short_string(source, offset, '"')),
        '\'' => Some(skip_short_string(source, offset, '\'')),
        '`' => Some(skip_short_string(source, offset, '`')),
        '[' => long_bracket_end(source, offset),
        _ => None,
    }
}

/// Skip a line comment, stopping at the newline or end of source.
fn skip_line_comment(source: &str, mut offset: usize) -> usize {
    while offset < source.len() {
        let ch = source[offset..]
            .chars()
            .next()
            .expect("offset is in bounds");
        offset += ch.len_utf8();
        if ch == '\n' {
            break;
        }
    }
    offset
}

/// Skip a short quoted string, including escapes, until its terminator or line end.
fn skip_short_string(source: &str, offset: usize, quote: char) -> usize {
    let mut cursor = offset + quote.len_utf8();
    while cursor < source.len() {
        let ch = source[cursor..]
            .chars()
            .next()
            .expect("cursor is in bounds");
        cursor += ch.len_utf8();
        if ch == quote || matches!(ch, '\r' | '\n') {
            break;
        }
        if ch == '\\' && cursor < source.len() {
            let escaped = source[cursor..]
                .chars()
                .next()
                .expect("cursor is in bounds");
            cursor += escaped.len_utf8();
        }
    }
    cursor
}

/// Return the end offset of a Luau long bracket string/comment at `offset`.
fn long_bracket_end(source: &str, offset: usize) -> Option<usize> {
    if !source[offset..].starts_with('[') {
        return None;
    }

    let mut cursor = offset + '['.len_utf8();
    while source[cursor..].starts_with('=') {
        cursor += '='.len_utf8();
    }
    if !source[cursor..].starts_with('[') {
        return None;
    }

    let close = format!("]{}]", "=".repeat(cursor - offset - '['.len_utf8()));
    let body_start = cursor + '['.len_utf8();
    Some(
        source[body_start..]
            .find(&close)
            .map_or(source.len(), |relative| body_start + relative + close.len()),
    )
}

/// Move to the next UTF-8 character boundary after `offset`.
fn next_char_boundary(source: &str, offset: usize) -> usize {
    let ch = source[offset..]
        .chars()
        .next()
        .expect("offset is in bounds");
    offset + ch.len_utf8()
}

/// Return true for Luau identifier continuation characters used in host names.
fn is_identifier_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
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
        return Err(diagnostics::config_type_error(
            path,
            source,
            &errors,
            line_offset,
        ));
    }

    compile::compile_for(
        surface.profile(),
        source.as_bytes(),
        &CompileOptions::for_vm_execution(),
    )
    .map(|_| ())
    .map_err(|err| diagnostics::config_compile_error(source, &err, Some(path)))
}

/// Checker-only native module that declares and audits the full Hotki host API surface.
struct StaticHotkiApiModule;

impl NativeModule for StaticHotkiApiModule {
    fn name(&self) -> &str {
        "hotki"
    }

    fn declaration(&self) -> oxau::decl::DeclSource<'_> {
        oxau::decl::DeclSource::Text(luau_api())
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
    fn check_ignores_import_text_in_comments_and_strings() {
        let root = test_dir("inert-import-text");
        fs::write(
            root.join("config.luau"),
            r#"
local one = "hotki.import_mode('missing-mode')"
local two = [[hotki.import_handler("missing-handler")]]
-- hotki.import_items("missing-items")
--[=[
hotki.import_style("missing-block-style")
]=]

hotki.root(function(menu, ctx)
end)
"#,
        )
        .expect("write root config");

        let report = check_luau_config(&root.join("config.luau")).expect("check config");
        assert_eq!(report.imports, 0);
        assert_eq!(report.themes, 0);
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
