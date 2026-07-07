//! Resolved style aliases, overlay helpers, and style-file loading.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use ruau::{
    bytecode::CompileOptions,
    vm::{
        Ambient, CallOptions, Limits, RuntimeCapabilities, Scope, ScopedValue, ScriptError, Vm,
        serde::from_scoped_value,
    },
};
use serde::{Deserialize, Serialize};

use crate::{Error, error::excerpt_at, raw, script::diagnostics};

/// Visual theme configuration grouping HUD and notification settings.
pub type Style = hotki_protocol::Style;

/// HUD configuration section.
pub type Hud = hotki_protocol::HudStyle;

/// Notification configuration section.
pub type Notify = hotki_protocol::NotifyConfig;

/// Selector configuration section.
pub type Selector = hotki_protocol::SelectorStyle;

/// Name of the optional style override searched next to `config.luau`.
pub const STYLE_FILE_NAME: &str = "style.luau";

/// Embedded default style source users can dump and copy.
const DEFAULT_STYLE_SOURCE: &str = include_str!("../styles/default.luau");

/// Gas budget for evaluating a single style source.
const STYLE_GAS_LIMIT: u64 = 1_000_000;

/// Heap budget for the short-lived VM used to evaluate one style source.
const STYLE_MEMORY_LIMIT: usize = 16 * 1024 * 1024;

/// Source provenance for the resolved base style.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum StyleProvenance {
    /// Only the embedded default style was applied.
    #[default]
    DefaultOnly,
    /// A sibling `style.luau` file was merged over the embedded default.
    Override {
        /// Filesystem path to the loaded override.
        path: PathBuf,
    },
}

/// Resolved base style plus provenance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolvedStyle {
    /// Fully resolved style applied to the app.
    pub style: Style,
    /// Source of the resolved style.
    pub provenance: StyleProvenance,
}

/// Resolver for Hotki's embedded default style and optional sibling override.
#[derive(Clone, Debug)]
pub struct StyleResolver {
    /// Eagerly evaluated default style.
    default: Style,
    /// Optional sibling `style.luau` path.
    override_path: Option<PathBuf>,
}

impl StyleResolver {
    /// Build a resolver from an active config path.
    pub fn from_config_path(config_path: &Path) -> Result<Self, Error> {
        let dir = config_path.parent().unwrap_or_else(|| Path::new("."));
        Self::with_override_path(dir.join(STYLE_FILE_NAME))
    }

    /// Build a resolver with an explicit override path.
    pub fn with_override_path(path: PathBuf) -> Result<Self, Error> {
        Self::new(Some(path))
    }

    /// Build a resolver that can only return the embedded default.
    pub fn default_only() -> Result<Self, Error> {
        Self::new(None)
    }

    /// Internal constructor that loads the embedded default immediately.
    fn new(override_path: Option<PathBuf>) -> Result<Self, Error> {
        let default = resolve_default_style()?;
        Ok(Self {
            default,
            override_path,
        })
    }

    /// Resolve the effective style by merging `style.luau` over the default when it exists.
    pub fn resolve(&self) -> Result<ResolvedStyle, Error> {
        let Some(path) = &self.override_path else {
            return Ok(self.default_result());
        };
        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(self.default_result());
            }
            Err(error) => {
                return Err(Error::Read {
                    path: Some(path.clone()),
                    message: error.to_string(),
                });
            }
        };
        let overlay = eval_style_source(&source, path)?;
        let provenance_path = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        Ok(ResolvedStyle {
            style: overlay_raw(self.default.clone(), &overlay),
            provenance: StyleProvenance::Override {
                path: provenance_path,
            },
        })
    }

    /// Return the default-only result.
    fn default_result(&self) -> ResolvedStyle {
        ResolvedStyle {
            style: self.default.clone(),
            provenance: StyleProvenance::DefaultOnly,
        }
    }
}

/// Return the embedded default style source.
pub fn default_style_source() -> &'static str {
    DEFAULT_STYLE_SOURCE
}

/// Resolve the embedded default style.
pub fn default_style() -> Result<Style, Error> {
    StyleResolver::default_only().map(|resolver| resolver.default)
}

/// Overlay raw style overrides onto this base style using current values as defaults.
pub fn overlay_raw(mut style: Style, overrides: &raw::RawStyle) -> Style {
    style.hud = raw::apply_optional_overlay(
        overrides.hud.as_option().cloned(),
        &style.hud,
        |hud, base| hud.into_hud_over(base),
    );
    style.notify = raw::apply_optional_overlay(
        overrides.notify.as_option().cloned(),
        &style.notify,
        |notify, base| notify.into_notify_over(base),
    );
    style.selector = raw::apply_optional_overlay(
        overrides.selector.as_option().cloned(),
        &style.selector,
        |selector, base| selector.into_selector_over(base),
    );
    style
}

/// Evaluate the embedded default style and apply it over protocol defaults.
fn resolve_default_style() -> Result<Style, Error> {
    let path = Path::new("<builtin:default-style>");
    let raw = eval_style_source(DEFAULT_STYLE_SOURCE, path)?;
    Ok(overlay_raw(Style::default(), &raw))
}

/// Evaluate one Luau style source file into a raw style overlay.
pub(crate) fn eval_style_source(source: &str, path: &Path) -> Result<raw::RawStyle, Error> {
    let runtime_capabilities = RuntimeCapabilities::default().enable_runtime_compilation();
    let chunk = runtime_capabilities
        .compile_source(source.as_bytes(), &CompileOptions::new())
        .map_err(|err| diagnostics::config_compile_error(source, &err, Some(path)))?;
    let chunk_name = format!("@{}", path.display());
    let mut vm = build_style_vm(runtime_capabilities, path)?;
    let module = vm
        .load_named(&chunk, chunk_name.as_bytes())
        .map_err(|err| diagnostics::config_validation(Some(path.to_path_buf()), err))?;

    let mut parsed = None;
    let mut script_error = None;
    let mut decode_error = None;
    vm.step_with(&CallOptions::new().limits(style_limits()), |scope| {
        let main = scope.module_function(&module);
        let result: Result<ScopedValue<'_>, ScriptError<'_>> = scope.call_protected(main, ())?;
        match result {
            Ok(value) => match from_scoped_value::<raw::RawStyle>(scope, value) {
                Ok(style) => match style.validate() {
                    Ok(()) => parsed = Some(style),
                    Err(message) => decode_error = Some(format!("invalid style: {message}")),
                },
                Err(err) => decode_error = Some(err.message().to_string()),
            },
            Err(err) => script_error = Some(style_script_error(source, path, scope, &err)),
        }
        Ok(())
    })
    .map_err(|err| diagnostics::config_validation(Some(path.to_path_buf()), err.message()))?;

    if let Some(err) = script_error {
        return Err(err);
    }
    if let Some(message) = decode_error {
        return Err(Error::Validation {
            path: Some(path.to_path_buf()),
            line: None,
            col: None,
            message: format!("invalid style table: {message}"),
            excerpt: None,
        });
    }

    parsed.ok_or_else(|| {
        diagnostics::config_validation(Some(path.to_path_buf()), "style script returned no value")
    })
}

/// Build the sandboxed VM used to evaluate one style file.
fn build_style_vm(runtime_capabilities: RuntimeCapabilities, path: &Path) -> Result<Vm, Error> {
    Vm::builder()
        .ambient(Ambient::deterministic(0))
        .limits(style_limits())
        .runtime_capabilities(runtime_capabilities)
        .sandboxed()
        .build()
        .map_err(|err| diagnostics::config_validation(Some(path.to_path_buf()), err))
}

/// Return the per-style execution limits.
fn style_limits() -> Limits {
    Limits::production(STYLE_GAS_LIMIT, STYLE_MEMORY_LIMIT)
}

/// Convert a protected style script failure into a located validation error.
fn style_script_error<'s>(
    source: &str,
    path: &Path,
    scope: &Scope<'s>,
    err: &ScriptError<'s>,
) -> Error {
    let message = from_scoped_value::<String>(scope, err.value())
        .unwrap_or_else(|_| format!("style script raised a {} value", err.value().type_name()));
    let (line, col, excerpt) = style_traceback_location(err.traceback(), path)
        .map(|(line, col)| (Some(line), Some(col), Some(excerpt_at(source, line, col))))
        .unwrap_or((None, None, None));
    Error::Validation {
        path: Some(path.to_path_buf()),
        line,
        col,
        message,
        excerpt,
    }
}

/// Extract the first traceback line that matches one style path.
fn style_traceback_location(traceback: Option<&str>, path: &Path) -> Option<(usize, usize)> {
    let expected = path.to_string_lossy();
    traceback?.lines().find_map(|line| {
        let line = line.trim();
        let index = line
            .char_indices()
            .find_map(|(index, ch)| (ch == ':').then_some(index))?;
        let found = &line[..index];
        if found != expected {
            return None;
        }
        let rest = &line[index + 1..];
        let digits = rest
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        let line_no = digits.parse::<usize>().ok()?;
        Some((line_no, 1))
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{StyleProvenance, StyleResolver};

    fn test_dir(name: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tmp")
            .join(format!("style-resolver-{name}-{id}"));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale tmp dir");
        }
        fs::create_dir_all(&root).expect("create tmp dir");
        root
    }

    #[test]
    fn default_only_resolves_embedded_style() {
        let resolved = StyleResolver::default_only()
            .expect("resolver")
            .resolve()
            .expect("style");

        assert_eq!(resolved.provenance, StyleProvenance::DefaultOnly);
        assert!(resolved.style.hud.font_size > 0.0);
    }

    #[test]
    fn missing_sibling_style_uses_default_only_provenance() {
        let root = test_dir("missing-sibling");
        let config_path = root.join("config.luau");
        let resolved = StyleResolver::from_config_path(&config_path)
            .expect("resolver")
            .resolve()
            .expect("style");

        assert_eq!(resolved.provenance, StyleProvenance::DefaultOnly);
    }

    #[test]
    fn sibling_style_overrides_default() {
        let root = test_dir("sibling-override");
        let style_path = root.join("style.luau");
        fs::write(&style_path, r##"return { hud = { bg = "#010203" } }"##).expect("write style");

        let resolved = StyleResolver::with_override_path(style_path)
            .expect("resolver")
            .resolve()
            .expect("style");

        assert_eq!(resolved.style.hud.bg, (1, 2, 3));
        assert!(matches!(
            resolved.provenance,
            StyleProvenance::Override { path } if path.ends_with("style.luau")
        ));
    }

    #[test]
    fn explicit_config_path_resolves_sibling_style() {
        let root = test_dir("config-path-sibling");
        fs::write(
            root.join("style.luau"),
            r##"return { notify = { timeout = 2.5 } }"##,
        )
        .expect("write style");

        let resolved = StyleResolver::from_config_path(&root.join("config.luau"))
            .expect("resolver")
            .resolve()
            .expect("style");

        assert_eq!(resolved.style.notify.timeout, 2.5);
    }

    #[test]
    fn invalid_sibling_style_reports_style_path() {
        let root = test_dir("invalid-sibling");
        let style_path = root.join("style.luau");
        fs::write(&style_path, "return { hud = @ }").expect("write style");

        let err = StyleResolver::with_override_path(style_path.clone())
            .expect("resolver")
            .resolve()
            .expect_err("style should fail");

        assert_eq!(err.path(), Some(style_path.as_path()));
    }

    #[test]
    fn non_finite_style_values_are_rejected() {
        let root = test_dir("nonfinite");
        let style_path = root.join("style.luau");
        fs::write(&style_path, "return { notify = { timeout = 0 / 0 } }").expect("write style");

        let err = StyleResolver::with_override_path(style_path)
            .expect("resolver")
            .resolve()
            .expect_err("style should fail");

        assert!(err.pretty().contains("notify.timeout must be finite"));
    }
}
