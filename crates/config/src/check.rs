//! Luau configuration validation helpers.

use std::{fs, io::ErrorKind, path::Path};

use ruau::{
    bytecode::CompileOptions,
    source::{ModuleId, Source},
    surface::{CheckOptions, Surface},
    typecheck::{Mode, Severity},
};

use crate::{
    Error, LuauApiSurface, STYLE_FILE_NAME, luau_api_surface, script::diagnostics,
    style::eval_style_source,
};

/// Summary of a successful Luau validation run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LuauCheckReport {
    /// Number of checked behavior modules, including the entry module.
    pub modules: usize,
    /// Whether a sibling `style.luau` file was validated.
    pub style: bool,
}

/// Cursor for scanning Luau code while skipping comments and strings.
struct LuauCodeScanner<'a> {
    /// Source text being scanned.
    source: &'a str,
    /// Current byte offset.
    cursor: usize,
}

impl<'a> LuauCodeScanner<'a> {
    /// Build a scanner for source text.
    fn new(source: &'a str) -> Self {
        Self { source, cursor: 0 }
    }

    /// Return the next byte offset that belongs to code rather than an ignored region.
    fn next_code_offset(&mut self) -> Option<usize> {
        while self.cursor < self.source.len() {
            if let Some(next) = skip_ignored_luau(self.source, self.cursor) {
                self.cursor = next;
                continue;
            }

            let offset = self.cursor;
            self.cursor = next_char_boundary(self.source, self.cursor);
            return Some(offset);
        }
        None
    }
}

/// Validate a filesystem-backed Luau config and optional sibling style.
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
    reject_removed_config_surface(&canonical, &source)?;
    let style = check_luau_style_file(&root_dir.join(STYLE_FILE_NAME))?;
    let config = crate::load_dynamic_config(&canonical)?;
    let (entry_gas, validation_gas, retained_heap) = config.load_metrics();
    tracing::debug!(
        path = %canonical.display(),
        entry_gas,
        validation_gas,
        retained_heap,
        "validated Luau config graph"
    );

    Ok(LuauCheckReport {
        modules: config.module_count(),
        style,
    })
}

/// Analyze and validate one optional standalone `style.luau` file.
pub fn check_luau_style_file(path: &Path) -> Result<bool, Error> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            return Err(Error::Read {
                path: Some(path.to_path_buf()),
                message: err.to_string(),
            });
        }
    };
    check_luau_style_source(path, &source)?;
    Ok(true)
}

/// Analyze and evaluate style source text under a diagnostic path.
pub fn check_luau_style_source(path: &Path, source: &str) -> Result<(), Error> {
    analyze_style_file(path, source)?;
    eval_style_source(source, path)?;
    Ok(())
}

/// Reject removed style and theme APIs with migration-oriented diagnostics.
fn reject_removed_config_surface(path: &Path, source: &str) -> Result<(), Error> {
    let mut scanner = LuauCodeScanner::new(source);
    while let Some(cursor) = scanner.next_code_offset() {
        if source[cursor..].starts_with("hotki.root") {
            return Err(migration_error(
                path,
                source,
                cursor,
                "hotki.root was removed; return the renderer from config.luau".to_string(),
            ));
        }
        if source[cursor..].starts_with("hotki.import_style") {
            return Err(migration_error(
                path,
                source,
                cursor,
                "hotki.import_style was removed; put global style overrides in sibling style.luau"
                    .to_string(),
            ));
        }
        if source[cursor..].starts_with("action.theme_") {
            return Err(migration_error(
                path,
                source,
                cursor,
                "theme actions were removed; edit sibling style.luau and reload the config"
                    .to_string(),
            ));
        }
        if themes_reference_at(source, cursor) {
            return Err(migration_error(
                path,
                source,
                cursor,
                "the themes registry was removed; put global style overrides in sibling style.luau"
                    .to_string(),
            ));
        }
        if source[cursor..].starts_with(":style") {
            return Err(migration_error(
                path,
                source,
                cursor,
                "local render styling was removed; put global style overrides in sibling style.luau"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

/// Return true if `themes` is used as a global property or method target at this offset.
fn themes_reference_at(source: &str, offset: usize) -> bool {
    let Some(after_prefix) = source[offset..].strip_prefix("themes") else {
        return false;
    };
    if offset > 0
        && source[..offset]
            .chars()
            .next_back()
            .is_some_and(is_identifier_continue)
    {
        return false;
    }
    matches!(after_prefix.chars().next(), Some(':') | Some('.'))
}

/// Build a migration diagnostic at the removed API call site.
fn migration_error(path: &Path, source: &str, offset: usize, message: String) -> Error {
    diagnostics::config_error_at_offset(path, source, offset, message)
}

/// Skip comments and strings that should not be scanned for removed APIs.
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

/// Analyze a standalone style file against the style-only declaration surface.
fn analyze_style_file(path: &Path, source: &str) -> Result<(), Error> {
    check_module_with_surface(path, &style_analysis_source(source), LuauApiSurface::Style)
}

/// Wrap style source so top-level `return` is checked as a `Style` value.
fn style_analysis_source(source: &str) -> String {
    format!("local _style = ((function()\n{source}\nend)() :: Style)\n")
}

/// Build the static Hotki script surface used by the ruau checker.
fn checker_surface() -> Result<Surface, Error> {
    Surface::builder()
        .enable_runtime_compilation()
        .analysis_mode(Mode::Strict)
        .build()
        .map_err(|err| Error::Validation {
            path: None,
            line: None,
            col: None,
            message: err.to_string(),
            excerpt: None,
        })
}

/// Run ruau's checker and bytecode compiler on one source module.
fn check_module_with_surface(
    path: &Path,
    source: &str,
    api_surface: LuauApiSurface,
) -> Result<(), Error> {
    let surface = checker_surface()?;
    let api = luau_api_surface(api_surface);
    let prelude = api.trim_end();
    let checked_source = format!("{prelude}\n{source}");
    let line_offset = prelude.lines().count();
    let check_source = Source::text(
        ModuleId::new(path.to_string_lossy().into_owned()),
        checked_source,
    );
    let checked = surface.check(&check_source, CheckOptions::default());
    let errors = checked
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
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

    let compile_source = Source::text(ModuleId::new(path.to_string_lossy().into_owned()), source);
    surface
        .compile(&compile_source, &CompileOptions::default())
        .map(|_| ())
        .map_err(|err| diagnostics::config_compile_error(source, &err, Some(path)))
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsStr,
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{check_luau_config, check_luau_style_file};

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

    fn example_config_paths(root: &Path) -> Vec<PathBuf> {
        fn visit(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(directory).expect("read example directory") {
                let path = entry.expect("read example entry").path();
                if path.is_dir() {
                    visit(root, &path, paths);
                    continue;
                }
                let is_top_level = path.parent() == Some(root);
                let is_top_level_example = is_top_level
                    && path.extension() == Some(OsStr::new("luau"))
                    && path.file_name() != Some(OsStr::new("style.luau"));
                let is_nested_entry =
                    !is_top_level && path.file_name() == Some(OsStr::new("config.luau"));
                if is_top_level_example || is_nested_entry {
                    paths.push(path);
                }
            }
        }

        let mut paths = Vec::new();
        visit(root, root, &mut paths);
        paths
    }

    #[test]
    fn check_rejects_hotki_root_with_migration_hint() {
        let root = test_dir("removed-hotki-root");
        fs::write(
            root.join("config.luau"),
            "hotki.root(function(menu, ctx) end)",
        )
        .expect("write root config");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("hotki.root was removed"));
        assert!(pretty.contains("return the renderer from config.luau"));
    }

    #[test]
    fn check_rejects_removed_role_imports_as_missing_api_fields() {
        for function_name in ["import_mode", "import_items", "import_handler"] {
            let root = test_dir(function_name);
            fs::write(
                root.join("config.luau"),
                format!(
                    r#"
local imported = hotki.{function_name}("child")

return function(menu, ctx)
end
"#
                ),
            )
            .expect("write root config");

            let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
            let pretty = err.pretty();
            assert!(pretty.contains(function_name), "unexpected error: {pretty}");
            assert!(
                !pretty.contains("literal relative path strings"),
                "old scanner diagnostic leaked: {pretty}"
            );
            assert!(
                !pretty.contains("style.luau"),
                "role imports should not get a migration hint: {pretty}"
            );
            assert!(
                !pretty.contains("was removed"),
                "role imports should fail through the API surface: {pretty}"
            );
        }
    }

    #[test]
    fn check_rejects_bare_module_requests() {
        let root = test_dir("bare-require");
        fs::write(
            root.join("config.luau"),
            r#"
local child = require("child")

return function(menu, ctx)
    menu:submenu("a", "Child", child)
end
"#,
        )
        .expect("write root config");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(
            pretty.contains("must begin with ./ or ../"),
            "unexpected error: {pretty}"
        );
    }

    #[test]
    fn check_validates_nested_relative_module_graphs() {
        let root = test_dir("nested-relative-require");
        fs::create_dir_all(root.join("apps")).expect("create module directory");
        fs::write(
            root.join("config.luau"),
            r#"
local child = require("./apps/child")
return function(menu, ctx)
    child(menu, ctx)
end
"#,
        )
        .expect("write root config");
        fs::write(
            root.join("apps/child.luau"),
            r#"
return function(menu: MenuBuilder, ctx: ModeContext)
    menu:bind("a", ctx.app, hotki.actions.pop)
end
"#,
        )
        .expect("write child module");

        let report = check_luau_config(&root.join("config.luau")).expect("check graph");
        assert_eq!(report.modules, 2);
    }

    #[test]
    fn check_allows_computed_requests_already_in_the_graph() {
        let root = test_dir("computed-checked-require");
        fs::write(
            root.join("config.luau"),
            r#"
local child = require("./child")
local request = "./child"
local computed = require(request)
assert(child == computed)

return function(menu)
    computed(menu)
end
"#,
        )
        .expect("write root config");
        fs::write(
            root.join("child.luau"),
            "return function(menu: MenuBuilder) end",
        )
        .expect("write child module");

        let report = check_luau_config(&root.join("config.luau")).expect("check graph");
        assert_eq!(report.modules, 2);
    }

    #[test]
    fn check_rejects_computed_requests_outside_the_graph() {
        let root = test_dir("computed-unchecked-require");
        fs::write(
            root.join("config.luau"),
            r#"
local child = require("./child")
local function late_request()
    return "./late"
end
require(late_request())

return function(menu)
    child(menu)
end
"#,
        )
        .expect("write root config");
        for name in ["child", "late"] {
            fs::write(
                root.join(format!("{name}.luau")),
                "return function(menu: MenuBuilder) end",
            )
            .expect("write child module");
        }

        let error = check_luau_config(&root.join("config.luau"))
            .expect_err("unchecked computed request should fail");
        assert!(
            error.pretty().contains("outside the checked config graph"),
            "unexpected error: {}",
            error.pretty()
        );
    }

    #[test]
    fn check_reports_module_graph_resolution_failures() {
        let missing = test_dir("missing-require");
        fs::write(
            missing.join("config.luau"),
            "require('./missing')\nreturn function(menu) end",
        )
        .expect("write missing config");
        let error = check_luau_config(&missing.join("config.luau"))
            .expect_err("missing module should fail");
        assert!(
            error.pretty().contains("missing"),
            "unexpected error: {}",
            error.pretty()
        );

        let ambiguous = test_dir("ambiguous-require");
        fs::create_dir_all(ambiguous.join("child")).expect("create child directory");
        fs::write(ambiguous.join("child.luau"), "return {}").expect("write child file");
        fs::write(ambiguous.join("child/init.luau"), "return {}").expect("write child init");
        fs::write(
            ambiguous.join("config.luau"),
            "require('./child')\nreturn function(menu) end",
        )
        .expect("write ambiguous config");
        let error = check_luau_config(&ambiguous.join("config.luau"))
            .expect_err("ambiguous module should fail");
        assert!(
            error.pretty().contains("ambiguous"),
            "unexpected error: {}",
            error.pretty()
        );
    }

    #[test]
    fn check_reports_cycles_child_errors_and_root_escapes() {
        let cycle = test_dir("require-cycle");
        fs::write(
            cycle.join("config.luau"),
            "require('./a')\nreturn function(menu) end",
        )
        .expect("write cycle config");
        fs::write(cycle.join("a.luau"), "return require('./b')").expect("write module a");
        fs::write(cycle.join("b.luau"), "return require('./a')").expect("write module b");
        let error =
            check_luau_config(&cycle.join("config.luau")).expect_err("module cycle should fail");
        let pretty = error.pretty().to_ascii_lowercase();
        assert!(
            pretty.contains("circular") || pretty.contains("cyclic") || pretty.contains("cycle"),
            "unexpected error: {}",
            error.pretty()
        );

        let child_error = test_dir("child-type-error");
        fs::write(
            child_error.join("config.luau"),
            "require('./child')\nreturn function(menu) end",
        )
        .expect("write child-error config");
        fs::write(
            child_error.join("child.luau"),
            "local value: number = 'bad'\nreturn value",
        )
        .expect("write invalid child");
        let error = check_luau_config(&child_error.join("config.luau"))
            .expect_err("child type error should fail");
        let child_path = fs::canonicalize(child_error.join("child.luau"))
            .expect("canonicalize invalid child path");
        assert_eq!(error.path(), Some(child_path.as_path()));

        let escape = test_dir("root-escape");
        fs::write(
            escape.join("config.luau"),
            "require('../outside')\nreturn function(menu) end",
        )
        .expect("write escaping config");
        let error =
            check_luau_config(&escape.join("config.luau")).expect_err("root escape should fail");
        assert!(
            error.pretty().contains("escape") || error.pretty().contains("outside"),
            "unexpected error: {}",
            error.pretty()
        );
    }

    #[test]
    fn check_reports_all_child_diagnostics_in_dependency_order() {
        let root = test_dir("ordered-child-errors");
        fs::write(
            root.join("config.luau"),
            "require('./a')\nrequire('./b')\nreturn function(menu) end",
        )
        .expect("write root config");
        for name in ["a", "b"] {
            fs::write(
                root.join(format!("{name}.luau")),
                format!("local {name}: number = 'bad'\nreturn {name}"),
            )
            .expect("write invalid dependency");
        }

        let error = check_luau_config(&root.join("config.luau"))
            .expect_err("child diagnostics should reject graph");
        let pretty = error.pretty();
        let a = pretty.find("a.luau").expect("a diagnostic path");
        let b = pretty.find("b.luau").expect("b diagnostic path");
        assert!(a < b, "diagnostics are not in dependency order: {pretty}");
    }

    #[test]
    fn check_follows_symlinks_inside_the_trusted_config_directory() {
        use std::os::unix::fs::symlink;

        let root = test_dir("symlink-require");
        fs::write(
            root.join("config.luau"),
            "local child = require('./alias')\nreturn function(menu) child(menu) end",
        )
        .expect("write root config");
        fs::write(
            root.join("child.luau"),
            "return function(menu: MenuBuilder) end",
        )
        .expect("write child module");
        symlink("child.luau", root.join("alias.luau")).expect("create module symlink");

        let report = check_luau_config(&root.join("config.luau")).expect("check symlink graph");
        assert_eq!(report.modules, 2);
    }

    #[test]
    fn check_enforces_strict_root_context_types() {
        let root = test_dir("strict-root-context");
        fs::write(
            root.join("config.luau"),
            r#"
return function(menu, ctx)
    local app: number = ctx.app
end
"#,
        )
        .expect("write root config");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("number"), "unexpected error: {pretty}");
        assert!(pretty.contains("string"), "unexpected error: {pretty}");
    }

    #[test]
    fn check_enforces_strict_action_handler_types() {
        let root = test_dir("strict-action-handler");
        fs::write(
            root.join("config.luau"),
            r#"
return function(menu, ctx)
    menu:bind("a", "Run", function(actx)
        local depth: string = actx.depth
    end)
end
"#,
        )
        .expect("write root config");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("string"), "unexpected error: {pretty}");
        assert!(pretty.contains("number"), "unexpected error: {pretty}");
    }

    #[test]
    fn check_enforces_strict_mode_renderer_types() {
        let root = test_dir("strict-mode-renderer");
        fs::write(
            root.join("config.luau"),
            r#"
local render: ModeRenderer = function(menu, ctx)
    local app: number = ctx.app
end

return render
"#,
        )
        .expect("write root config");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("number"), "unexpected error: {pretty}");
        assert!(pretty.contains("string"), "unexpected error: {pretty}");
    }

    #[test]
    fn check_enforces_selector_item_declarations() {
        let root = test_dir("selector-items");
        fs::write(
            root.join("config.luau"),
            r#"
return function(menu, ctx)
    menu:bind("a", "Select", function(actx)
        actx:select({
            items = {
                { label = 123, data = "bad" },
            },
            on_select = function(select_ctx, item, query)
            end,
        })
    end)
end
"#,
        )
        .expect("write root config");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("number"), "unexpected error: {pretty}");
        assert!(pretty.contains("string"), "unexpected error: {pretty}");
    }

    #[test]
    fn check_accepts_static_selector_item_tables() {
        let root = test_dir("static-selector-items");
        fs::write(
            root.join("config.luau"),
            r#"
local items: SelectorItemList<string> = {
    { label = "Alpha", data = "alpha" },
    { label = "Beta", sublabel = "second", data = "beta" },
}

return function(menu, ctx)
    menu:bind("a", "Select", function(actx)
        actx:select({
            items = items,
            on_select = function(select_ctx, item: SelectorItem<string>, query)
                select_ctx:notify("info", item.label, item.data)
            end,
        })
    end)
end
"#,
        )
        .expect("write root config");

        let report = check_luau_config(&root.join("config.luau")).expect("check config");
        assert!(!report.style);
    }

    #[test]
    fn check_accepts_static_selector_string_lists() {
        let root = test_dir("static-selector-strings");
        fs::write(
            root.join("config.luau"),
            r#"
local items: SelectorStringList = { "Alpha", "Beta" }

return function(menu, ctx)
    menu:bind("a", "Select", function(actx)
        actx:select({
            items = items,
            on_select = function(select_ctx, item: SelectorItem<string>, query)
                select_ctx:notify("info", item.label, item.data)
            end,
        })
    end)
end
"#,
        )
        .expect("write root config");

        let report = check_luau_config(&root.join("config.luau")).expect("check config");
        assert!(!report.style);
    }

    #[test]
    fn check_accepts_selector_item_providers() {
        let root = test_dir("selector-provider");
        fs::write(
            root.join("config.luau"),
            r#"
local function items(ctx: ModeContext): SelectorItemList<string>
    return {
        { label = ctx.app, data = ctx.app },
    }
end

local provider: SelectorItemProvider<string> = items

return function(menu, ctx)
    menu:bind("a", "Select", function(actx)
        actx:select({
            items = provider,
            on_select = function(select_ctx, item: SelectorItem<string>, query)
                select_ctx:notify("info", item.label, item.data)
            end,
        })
    end)
end
"#,
        )
        .expect("write root config");

        let report = check_luau_config(&root.join("config.luau")).expect("check config");
        assert!(!report.style);
    }

    #[test]
    fn check_accepts_selector_string_providers() {
        let root = test_dir("selector-string-provider");
        fs::write(
            root.join("config.luau"),
            r#"
local function items(ctx: ModeContext): SelectorStringList
    return { ctx.app, "Fallback" }
end

local provider: SelectorStringProvider = items

return function(menu, ctx)
    menu:bind("a", "Select", function(actx)
        actx:select({
            items = provider,
            on_select = function(select_ctx, item: SelectorItem<string>, query)
                select_ctx:notify("info", item.label, item.data)
            end,
        })
    end)
end
"#,
        )
        .expect("write root config");

        let report = check_luau_config(&root.join("config.luau")).expect("check config");
        assert!(!report.style);
    }

    #[test]
    fn check_accepts_filtering_applications_provider_items() {
        let root = test_dir("filter-applications-provider");
        fs::write(
            root.join("config.luau"),
            r#"
local function visible_apps(ctx: ModeContext): SelectorItemList<ApplicationInfo>
    local out: { SelectorItem<ApplicationInfo> } = {}
    for _, item in hotki.applications(ctx) do
        local path: string = item.data.path
        if item.label ~= "Skip" and path ~= "" then
            table.insert(out, {
                label = item.label,
                sublabel = item.sublabel,
                data = item.data,
            })
        end
    end
    return out
end

return function(menu, ctx)
    menu:bind("a", "Apps", function(actx)
        actx:select({
            items = visible_apps,
            on_select = function(select_ctx, item: SelectorItem<ApplicationInfo>, query)
                select_ctx:open(item.data.path)
            end,
        })
    end)
end
"#,
        )
        .expect("write root config");

        let report = check_luau_config(&root.join("config.luau")).expect("check config");
        assert!(!report.style);
    }

    #[test]
    fn check_rejects_action_global_as_missing_config_surface() {
        let root = test_dir("removed-action-global");
        fs::write(
            root.join("config.luau"),
            r#"
return function(menu, ctx)
    menu:bind("a", "Bad", action.reload_config)
end
"#,
        )
        .expect("write root config");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("action"), "unexpected error: {pretty}");
    }

    #[test]
    fn check_rejects_ctx_exec_as_missing_action_context_surface() {
        let root = test_dir("removed-ctx-exec");
        fs::write(
            root.join("config.luau"),
            r#"
return function(menu, ctx)
    menu:bind("a", "Bad", function(actx)
        actx:exec(function(inner)
        end)
    end)
end
"#,
        )
        .expect("write root config");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("exec"), "unexpected error: {pretty}");
    }

    #[test]
    fn check_accepts_closure_actions_and_context_effects() {
        let root = test_dir("closure-actions-and-context-effects");
        fs::write(
            root.join("config.luau"),
            r#"
return function(menu, ctx)
    menu:bind("a", "Handler", function(actx)
        actx:shell("true")
        actx:pop()
    end)
    menu:bind("b", "Selector", function(actx)
        actx:select({
            items = { "One" },
            on_select = function(select_ctx, item, query)
            end,
        })
    end)
    menu:bind("c", "Reload", function(actx)
        actx:reload_config()
    end)
end
"#,
        )
        .expect("write root config");

        let report = check_luau_config(&root.join("config.luau")).expect("check config");
        assert!(!report.style);
    }

    #[test]
    fn check_validates_sibling_style() {
        let root = test_dir("sibling-style");
        fs::write(
            root.join("config.luau"),
            r#"
return function(menu, ctx) end
"#,
        )
        .expect("write root config");
        fs::write(
            root.join("style.luau"),
            r##"return { hud = { bg = "#010203" } }"##,
        )
        .expect("write style");

        let report = check_luau_config(&root.join("config.luau")).expect("check config");
        assert!(report.style);
    }

    #[test]
    fn check_rejects_old_theme_registry_with_migration_hint() {
        let root = test_dir("old-themes");
        fs::write(
            root.join("config.luau"),
            r#"
themes:use("default")
return function(menu, ctx) end
"#,
        )
        .expect("write root config");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("themes registry was removed"));
        assert!(pretty.contains("style.luau"));
    }

    #[test]
    fn check_rejects_old_theme_actions_with_migration_hint() {
        let root = test_dir("old-theme-action");
        fs::write(
            root.join("config.luau"),
            r#"
return function(menu, ctx)
    menu:bind("t", "Theme", action.theme_next)
end
"#,
        )
        .expect("write root config");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("theme actions were removed"));
        assert!(pretty.contains("style.luau"));
    }

    #[test]
    fn check_rejects_old_menu_style_with_migration_hint() {
        let root = test_dir("old-menu-style");
        fs::write(
            root.join("config.luau"),
            r##"
return function(menu, ctx)
    menu:style({ hud = { bg = "#000000" } })
end
"##,
        )
        .expect("write root config");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("local render styling was removed"));
        assert!(pretty.contains("style.luau"));
    }

    #[test]
    fn check_rejects_old_import_style_with_migration_hint() {
        let root = test_dir("old-import-style");
        fs::write(
            root.join("config.luau"),
            r#"
local local_style = hotki.import_style("local-style")
return function(menu, ctx) end
"#,
        )
        .expect("write root config");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("hotki.import_style was removed"));
        assert!(pretty.contains("style.luau"));
    }

    #[test]
    fn style_file_rejects_config_globals() {
        let root = test_dir("style-config-global");
        let path = root.join("style.luau");
        fs::write(
            &path,
            r#"
action.shell("true")
return {}
"#,
        )
        .expect("write style");

        let err = check_luau_style_file(&path).expect_err("check should fail");
        assert!(err.pretty().contains("action"));
    }

    #[test]
    fn check_validates_all_workspace_examples() {
        let examples_dir = workspace_root().join("examples");
        let mut example_paths = example_config_paths(&examples_dir);
        example_paths.sort();

        assert!(
            !example_paths.is_empty(),
            "no Luau examples found in {}",
            examples_dir.display()
        );
        assert!(
            example_paths
                .iter()
                .any(|path| path.ends_with("examples/cortesi/config.luau")),
            "nested Cortesi entry was not discovered"
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
