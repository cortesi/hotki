//! Luau configuration validation helpers.

use std::{fs, io::ErrorKind, path::Path};

use ruau::{
    bytecode::CompileOptions,
    source::{ModuleId, Source},
    surface::{CheckOptions, Surface},
    typecheck::{Mode, Severity},
};

use crate::{
    Error, LuauApiSurface, StyleResolver, luau_api_surface,
    script::{diagnostics, loader::load_dynamic_config_with_style},
    style::{ResolvedStyle, eval_style_source},
};

/// Summary of a successful Luau validation run.
#[derive(Debug, Clone, PartialEq)]
pub struct LuauCheckReport {
    /// Number of checked behavior modules, including the entry module.
    pub modules: usize,
    /// Whether a sibling `style.luau` file was validated.
    pub style: bool,
    /// Effective style resolved while validating this candidate.
    pub resolved_style: ResolvedStyle,
}

/// Validate a filesystem-backed Luau config and optional sibling style.
pub fn check_luau_config(path: &Path) -> Result<LuauCheckReport, Error> {
    let canonical = fs::canonicalize(path).map_err(|err| Error::Read {
        path: Some(path.to_path_buf()),
        message: err.to_string(),
    })?;
    let style_candidate = StyleResolver::from_config_path(&canonical)?.read_candidate()?;
    let style = if let Some((path, source)) = &style_candidate.override_source {
        analyze_style_file(path, source)?;
        true
    } else {
        false
    };
    let config = load_dynamic_config_with_style(&canonical, style_candidate.resolve()?)?;
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
        resolved_style: config.resolved_style(),
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
    fn check_reports_unknown_hotki_fields() {
        let root = test_dir("unknown-hotki-field");
        fs::write(
            root.join("config.luau"),
            r#"
local value = hotki.unknown_helper
assert(value == nil)

return function(menu, ctx)
end
"#,
        )
        .expect("write root config");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(
            pretty.contains("unknown_helper"),
            "unexpected error: {pretty}"
        );
        assert!(
            pretty.contains("missing property"),
            "unexpected error: {pretty}"
        );
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
    local window = ctx.window
    local label = if window ~= nil then window.app else "No focused window"
    menu:bind("a", label, hotki.actions.pop)
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
    local window: number = ctx.window
end
"#,
        )
        .expect("write root config");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("number"), "unexpected error: {pretty}");
        assert!(pretty.contains("ctx.window"), "unexpected error: {pretty}");
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
    local window: number = ctx.window
end

return render
"#,
        )
        .expect("write root config");

        let err = check_luau_config(&root.join("config.luau")).expect_err("check should fail");
        let pretty = err.pretty();
        assert!(pretty.contains("number"), "unexpected error: {pretty}");
        assert!(pretty.contains("ctx.window"), "unexpected error: {pretty}");
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
    local window = ctx.window
    local app = if window ~= nil then window.app else "No focused window"
    return {
        { label = app, data = app },
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
    local window = ctx.window
    local app = if window ~= nil then window.app else "No focused window"
    return { app, "Fallback" }
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
    fn check_accepts_current_config_with_unreserved_local_names() {
        let root = test_dir("unreserved-local-names");
        fs::write(
            root.join("config.luau"),
            r#"
local hotki = { root = 1, import_style = 2 }
local action = { theme_next = 3 }
local themes = { current = "plain" }
function themes:style()
    return self.current
end

assert(hotki.root + hotki.import_style + action.theme_next == 6)
assert(themes:style() == "plain")

return function(menu, ctx)
end
"#,
        )
        .expect("write root config");

        check_luau_config(&root.join("config.luau")).expect("check current config");
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
