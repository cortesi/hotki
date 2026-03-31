//! Theme registry and helpers.
//!
//! Built-in themes are defined as Luau source files embedded at compile time, then evaluated into
//! `RawStyle` overlays at startup (or lazily on first access).
use std::{collections::HashMap, path::Path, sync::OnceLock};

use crate::{Style, raw, style};

/// Theme error types and conversions.
mod error;
/// Theme script loading and evaluation.
mod loader;

pub use error::ThemeError;

/// Cached evaluated built-in theme overlays loaded from embedded Luau sources.
static BUILTIN_THEMES: OnceLock<HashMap<&'static str, raw::RawStyle>> = OnceLock::new();

/// Force initialization of the embedded built-in theme registry.
///
/// The Hotki app calls this at startup so failures surface immediately.
pub fn init_builtins() {
    let _ignored = builtin_raw_themes();
}

/// Return the evaluated embedded built-in themes (initialized on first access).
pub(crate) fn builtin_raw_themes() -> &'static HashMap<&'static str, raw::RawStyle> {
    BUILTIN_THEMES.get_or_init(|| {
        loader::load_builtin_raw_themes().expect("embedded built-in themes must load successfully")
    })
}

/// Load and evaluate user theme files from a directory.
pub(crate) fn load_user_themes(dir: &Path) -> Result<HashMap<String, raw::RawStyle>, ThemeError> {
    loader::load_user_raw_themes(dir)
}

/// Validate all user theme files in `dir` and return the number of loaded themes.
pub fn validate_theme_dir(dir: &Path) -> Result<usize, ThemeError> {
    Ok(load_user_themes(dir)?.len())
}

/// List all available built-in theme names.
pub fn list_themes() -> Vec<&'static str> {
    let mut names = builtin_raw_themes().keys().copied().collect::<Vec<_>>();
    names.sort();
    names
}

/// Navigate the sorted theme list by `offset` positions from `current`, wrapping around.
fn navigate_theme(current: &str, offset: isize) -> &'static str {
    let theme_list = list_themes();
    let Some(idx) = theme_list.iter().position(|&t| t == current) else {
        return theme_list.first().copied().unwrap_or("default");
    };
    let next = (idx as isize + offset).rem_euclid(theme_list.len() as isize) as usize;
    theme_list[next]
}

/// Get the next built-in theme in the sorted list.
pub fn get_next_theme(current: &str) -> &'static str {
    navigate_theme(current, 1)
}

/// Get the previous built-in theme in the sorted list.
pub fn get_prev_theme(current: &str) -> &'static str {
    navigate_theme(current, -1)
}

/// Check if a built-in theme exists.
pub fn theme_exists(name: &str) -> bool {
    builtin_raw_themes().contains_key(name)
}

/// Load a built-in theme as the base configuration.
pub fn load_theme(theme_name: Option<&str>) -> Style {
    let themes = builtin_raw_themes();
    let raw = match theme_name {
        Some(name) => themes.get(name).unwrap_or_else(|| {
            eprintln!("Warning: Theme '{}' not found, using default", name);
            themes.get("default").expect("default theme must exist")
        }),
        None => themes.get("default").expect("default theme must exist"),
    };

    style::overlay_raw(Style::default(), raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::load_dynamic_config_from_string;

    #[test]
    fn test_default_theme_exists() {
        assert!(builtin_raw_themes().contains_key("default"));
    }

    #[test]
    fn test_get_theme() {
        assert!(builtin_raw_themes().contains_key("default"));
        assert!(builtin_raw_themes().contains_key("dark-blue"));
        assert!(builtin_raw_themes().contains_key("charcoal"));
        assert!(!builtin_raw_themes().contains_key("nonexistent"));
    }

    #[test]
    fn test_list_themes() {
        let themes = list_themes();
        assert!(themes.contains(&"default"));
        assert!(themes.contains(&"charcoal"));
        assert!(themes.contains(&"dark-blue"));
        assert_eq!(themes.len(), 5); // We have exactly 5 built-in themes
    }

    #[test]
    fn test_load_theme_config() {
        // Load default theme
        let theme = load_theme(None);
        assert_eq!(theme.hud.font_size, 14.0);

        // Load specific theme
        let theme = load_theme(Some("dark-blue"));
        assert_eq!(theme.hud.title_fg, (0xa0, 0xc4, 0xff));
    }

    #[test]
    fn test_load_nonexistent_theme_falls_back() {
        // Should fall back to default
        let theme = load_theme(Some("nonexistent"));
        assert_eq!(theme.hud.title_fg, (0xd0, 0xd0, 0xd0)); // default theme value
    }

    #[test]
    fn test_theme_navigation() {
        let theme_list = list_themes();
        assert!(
            theme_list.len() >= 2,
            "Need at least 2 themes for navigation test"
        );

        // Test next theme navigation
        let first_theme = theme_list[0];
        let second_theme = theme_list[1];
        assert_eq!(get_next_theme(first_theme), second_theme);

        // Test wrap around from last to first
        let last_theme = theme_list[theme_list.len() - 1];
        assert_eq!(get_next_theme(last_theme), first_theme);

        // Test previous theme navigation
        assert_eq!(get_prev_theme(second_theme), first_theme);

        // Test wrap around from first to last
        let last_theme = theme_list[theme_list.len() - 1];
        assert_eq!(get_prev_theme(first_theme), last_theme);

        // Test with unknown theme defaults to first
        assert_eq!(get_next_theme("nonexistent"), first_theme);
        assert_eq!(get_prev_theme("nonexistent"), first_theme);
    }

    #[test]
    fn test_dynamic_config_defaults_to_default_theme() {
        let cfg = load_dynamic_config_from_string(r#"hotki.root(function(menu, ctx) end)"#, None)
            .expect("loads");

        let expected = load_theme(None).hud;
        let actual = cfg.base_style(None).hud;

        assert_eq!(actual.title_fg, expected.title_fg);
        assert_eq!(actual.bg, expected.bg);
        assert_eq!(actual.font_size, expected.font_size);
    }

    #[test]
    fn test_dynamic_config_theme_function_selects_builtins() {
        let cfg = load_dynamic_config_from_string(
            r#"
            themes:use("dark-blue")
            hotki.root(function(menu, ctx) end)
            "#,
            None,
        )
        .expect("loads");

        let expected = load_theme(Some("dark-blue")).hud;
        let actual = cfg.base_style(None).hud;

        assert_eq!(actual.title_fg, expected.title_fg);
        assert_eq!(actual.bg, expected.bg);
        assert_eq!(actual.font_size, expected.font_size);
    }
}
