//! Theme registry and helpers.
//!
//! Important: deadlock avoidance when loading themes.
//!
//! This module uses a `OnceLock<HashMap<..>>` to store compiled themes. When
//! populating the map inside `get_or_init`, we must not call any function that
//! (directly or indirectly) tries to access the same `themes()` initializer,
//! or we’d re-enter the `OnceLock` initialization and deadlock.
//!
//! Therefore, theme files are parsed via `ron::from_str` into a `RawStyle`
//! and then converted into a `Style`, instead of going through the higher-level
//! loader API. Do not replace this with a call that resolves themes dynamically.
use std::{collections::HashMap, sync::OnceLock};

use crate::{Hud, Notify, Style, raw::RawStyle};

/// All available themes, loaded at compile time
fn themes() -> &'static HashMap<String, Style> {
    static THEMES: OnceLock<HashMap<String, Style>> = OnceLock::new();
    THEMES.get_or_init(|| {
        let mut themes = HashMap::new();

        // Load each theme at compile time
        macro_rules! load_theme {
            ($name:expr, $file:expr) => {
                let content = include_str!(concat!("../../themes/", $file));
                // Parse theme content via RawStyle to avoid re-entering theme loading.
                // Using loader here would call load_theme() again during OnceLock init and deadlock.
                let raw = ron::from_str::<RawStyle>(content)
                    .expect(concat!("Failed to parse theme: ", $file));
                let hud = raw
                    .hud
                    .into_option()
                    .map(|h| h.into_hud())
                    .unwrap_or_else(Hud::default);
                let notify = raw
                    .notify
                    .into_option()
                    .map(|n| n.into_notify())
                    .unwrap_or_else(Notify::default);
                let theme = Style { hud, notify };
                themes.insert($name.to_string(), theme);
            };
        }

        load_theme!("default", "default.ron");
        load_theme!("charcoal", "charcoal.ron");
        load_theme!("dark-blue", "dark-blue.ron");
        load_theme!("solarized-dark", "solarized-dark.ron");
        load_theme!("solarized-light", "solarized-light.ron");

        themes
    })
}

/// Get a theme by name (tests only)
#[cfg(test)]
pub fn get_theme(name: &str) -> Option<&'static Style> {
    themes().get(name)
}

/// List all available theme names
pub fn list_themes() -> Vec<&'static str> {
    let mut names: Vec<_> = themes().keys().map(|s| s.as_str()).collect();
    names.sort();
    names
}

/// Get the next theme in the sorted list
pub fn get_next_theme(current: &str) -> &'static str {
    let theme_list = list_themes();
    let current_idx = theme_list.iter().position(|&t| t == current);

    match current_idx {
        Some(idx) => {
            let next_idx = (idx + 1) % theme_list.len();
            theme_list[next_idx]
        }
        None => theme_list.first().copied().unwrap_or("default"),
    }
}

/// Get the previous theme in the sorted list
pub fn get_prev_theme(current: &str) -> &'static str {
    let theme_list = list_themes();
    let current_idx = theme_list.iter().position(|&t| t == current);

    match current_idx {
        Some(idx) => {
            let prev_idx = if idx == 0 {
                theme_list.len() - 1
            } else {
                idx - 1
            };
            theme_list[prev_idx]
        }
        None => theme_list.first().copied().unwrap_or("default"),
    }
}

/// Check if a theme exists
pub fn theme_exists(name: &str) -> bool {
    themes().contains_key(name)
}

/// Load a theme as the base configuration
pub fn load_theme(theme_name: Option<&str>) -> Style {
    let theme = match theme_name {
        Some(name) => themes().get(name).unwrap_or_else(|| {
            eprintln!("Warning: Theme '{}' not found, using default", name);
            themes()
                .get("default")
                .expect("Default theme must always exist")
        }),
        None => themes()
            .get("default")
            .expect("Default theme must always exist"),
    };
    theme.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::RawStyle;

    #[test]
    fn test_default_theme_exists() {
        assert!(get_theme("default").is_some());
    }

    #[test]
    fn test_get_theme() {
        assert!(get_theme("default").is_some());
        assert!(get_theme("dark-blue").is_some());
        assert!(get_theme("charcoal").is_some());
        assert!(get_theme("nonexistent").is_none());
    }

    #[test]
    fn test_list_themes() {
        let themes = list_themes();
        assert!(themes.contains(&"default"));
        assert!(themes.contains(&"charcoal"));
        assert!(themes.contains(&"dark-blue"));
        assert_eq!(themes.len(), 5); // We have exactly 5 themes
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
        assert_eq!(get_prev_theme(first_theme), last_theme);

        // Test with unknown theme defaults to first
        assert_eq!(get_next_theme("nonexistent"), first_theme);
        assert_eq!(get_prev_theme("nonexistent"), first_theme);
    }

    #[test]
    fn test_theme_merging_with_user_config() {
        use crate::ConfigInput;

        // Load default theme as base
        let theme_base = load_theme(None);

        // Create a minimal user config that overrides some theme values
        let user_config_str = r#"(
            keys: [
                ("a", "Test action", shell("echo test")),
            ],
            style: (hud: (font_size: 20.0, title_fg: "red")),
        )"#;

        // Parse user config
        let user_raw: ConfigInput = ron::from_str(user_config_str).unwrap();

        // Build base theme, then overlay user theme
        let mut final_theme = theme_base;
        if let Some(user_theme_raw) = &user_raw.style {
            final_theme = final_theme.overlay_raw(user_theme_raw);
        }
        // Use user keys since provided
        let final_keys = user_raw.keys;

        // Verify user overrides are applied
        assert_eq!(final_theme.hud.font_size, 20.0);
        assert_eq!(final_theme.hud.title_fg, (255, 0, 0));

        // Verify theme defaults are preserved for unspecified fields
        assert_eq!(final_theme.hud.bg, (0x10, 0x10, 0x10)); // from default theme
        assert_eq!(final_theme.hud.opacity, 1.0); // from default theme

        // Verify keys are from user config
        assert_eq!(final_keys.keys().count(), 1);
    }

    #[test]
    fn test_empty_user_config_uses_theme_defaults() {
        use crate::ConfigInput;

        // Load default theme as base
        let theme_base = load_theme(None);

        // Create an empty user config
        let user_config_str = r#"(keys: [])"#;

        // Parse user config
        let _user_raw: ConfigInput = ron::from_str(user_config_str).unwrap();

        // Verify all theme defaults are used on the base theme directly
        assert_eq!(theme_base.hud.font_size, 14.0); // from default theme
        assert_eq!(theme_base.hud.title_fg, (0xd0, 0xd0, 0xd0)); // from default theme
        assert_eq!(theme_base.hud.bg, (0x10, 0x10, 0x10)); // from default theme
        assert_eq!(theme_base.hud.opacity, 1.0); // from default theme
    }

    #[test]
    fn test_theme_field_parsing() {
        use crate::ConfigInput;

        // Test parsing with theme specified
        let config_with_theme = r#"(
            base_theme: charcoal,
            keys: [],
        )"#;

        let parsed: ConfigInput = ron::from_str(config_with_theme).unwrap();
        let t = parsed.base_theme.expect("theme parsed");
        assert_eq!(t, "charcoal");

        // Test parsing without theme specified
        let config_without_theme = r#"(
            keys: [],
        )"#;

        let parsed: ConfigInput = ron::from_str(config_without_theme).unwrap();
        assert!(parsed.base_theme.is_none());
    }

    #[test]
    fn test_different_theme_as_base() {
        use crate::ConfigInput;

        // Load dark-blue theme as base
        let theme_base = load_theme(Some("dark-blue"));

        // Create a minimal user config
        let user_config_str = r#"(
            keys: [],
            style: (hud: (font_size: 18.0)),
        )"#;

        // Parse user config
        let user_raw: ConfigInput = ron::from_str(user_config_str).unwrap();

        // Build base theme and overlay user theme
        let mut final_theme = theme_base;
        if let Some(user_theme_raw) = &user_raw.style {
            final_theme = final_theme.overlay_raw(user_theme_raw);
        }

        // Verify user override is applied
        assert_eq!(final_theme.hud.font_size, 18.0);

        // Verify dark-blue theme values are used
        assert_eq!(final_theme.hud.title_fg, (0xa0, 0xc4, 0xff)); // from dark-blue theme
        assert_eq!(final_theme.hud.bg, (0x0a, 0x16, 0x28)); // from dark-blue theme
    }

    #[test]
    fn test_theme_overlay_on_base_switch() {
        use crate::ConfigInput;

        // User overrides only font_size; other fields should follow base theme
        let user_config_str = r#"(
            keys: [],
            style: (
                hud: (font_size: 20.0),
            ),
        )"#;

        let user_raw: ConfigInput = ron::from_str(user_config_str).unwrap();
        let user_theme_raw: &RawStyle = user_raw.style.as_ref().expect("user theme present");

        // Base: default
        let base_default = load_theme(None);
        let merged_default = base_default.overlay_raw(user_theme_raw);
        assert_eq!(merged_default.hud.font_size, 20.0);
        // Title color should come from base (not overridden by user)
        assert_eq!(merged_default.hud.title_fg, (0xd0, 0xd0, 0xd0));

        // Switch base: dark-blue
        let base_dark = load_theme(Some("dark-blue"));
        let merged_dark = base_dark.overlay_raw(user_theme_raw);
        assert_eq!(merged_dark.hud.font_size, 20.0);
        assert_eq!(merged_dark.hud.title_fg, (0xa0, 0xc4, 0xff));
    }
}
