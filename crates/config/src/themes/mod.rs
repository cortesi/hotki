//! Theme registry and helpers.
//!
//! Important: deadlock avoidance when loading themes.
//!
//! This module uses a `OnceLock<HashMap<..>>` to store compiled themes. When
//! populating the map inside `get_or_init`, we must not call any function that
//! (directly or indirectly) tries to access the same `themes()` initializer,
//! or we’d re-enter the `OnceLock` initialization and deadlock.
//!
//! Therefore, themes are defined as Rust values and inserted directly into the
//! registry, avoiding any dynamic theme loading during initialization.
use std::{collections::HashMap, sync::OnceLock};

use crate::{FontWeight, NotifyPos, Offset, Pos, Style, parse_rgb, raw::RawNotifyWindowStyle};

/// All available themes, loaded at compile time
fn themes() -> &'static HashMap<String, Style> {
    static THEMES: OnceLock<HashMap<String, Style>> = OnceLock::new();
    THEMES.get_or_init(|| {
        fn rgb(s: &str) -> (u8, u8, u8) {
            parse_rgb(s).unwrap_or_else(|| panic!("invalid theme color: {}", s))
        }

        fn set_notify_style(
            style: &mut RawNotifyWindowStyle,
            bg: &str,
            title_fg: &str,
            body_fg: &str,
            title_font_weight: FontWeight,
        ) {
            style.bg = Some(bg.to_string());
            style.title_fg = Some(title_fg.to_string());
            style.body_fg = Some(body_fg.to_string());
            style.title_font_weight = Some(title_font_weight);
        }

        fn theme_default() -> Style {
            let mut style = Style::default();

            style.hud.radius = 8.0;
            style.hud.pos = Pos::Center;
            style.hud.offset = Offset { x: 0.0, y: 0.0 };
            style.hud.font_size = 14.0;
            style.hud.title_font_weight = FontWeight::Regular;
            style.hud.key_font_size = 19.0;
            style.hud.tag_font_size = 20.0;
            style.hud.tag_font_weight = FontWeight::Regular;
            style.hud.title_fg = rgb("#d0d0d0");
            style.hud.bg = rgb("#101010");
            style.hud.key_radius = 4.0;
            style.hud.key_fg = rgb("#d0d0d0");
            style.hud.key_bg = rgb("#2c3471");
            style.hud.key_font_weight = FontWeight::Bold;
            style.hud.mod_fg = rgb("white");
            style.hud.mod_font_weight = FontWeight::Regular;
            style.hud.mod_bg = rgb("#43414d");
            style.hud.tag_fg = rgb("#374f8a");
            style.hud.opacity = 1.0;

            style.notify.width = 420.0;
            style.notify.pos = NotifyPos::Right;
            style.notify.opacity = 0.95;
            style.notify.timeout = 4.0;
            style.notify.buffer = 200;

            set_notify_style(
                &mut style.notify.info,
                "#222222",
                "white",
                "white",
                FontWeight::Bold,
            );
            set_notify_style(
                &mut style.notify.warn,
                "#442a00",
                "#ffc100",
                "#ffc100",
                FontWeight::Bold,
            );
            set_notify_style(
                &mut style.notify.error,
                "#3a0000",
                "#ff6666",
                "#ff6666",
                FontWeight::Bold,
            );
            set_notify_style(
                &mut style.notify.success,
                "#0c2d0c",
                "#8bff8b",
                "#8bff8b",
                FontWeight::Bold,
            );

            style
        }

        fn theme_charcoal() -> Style {
            let mut style = Style::default();

            style.hud.pos = Pos::Center;
            style.hud.offset = Offset { x: 0.0, y: 0.0 };
            style.hud.font_size = 14.0;
            style.hud.title_font_weight = FontWeight::Regular;
            style.hud.key_font_size = 14.0;
            style.hud.key_font_weight = FontWeight::Regular;
            style.hud.tag_font_size = 14.0;
            style.hud.tag_font_weight = FontWeight::Regular;
            style.hud.title_fg = rgb("white");
            style.hud.bg = rgb("#202020");
            style.hud.key_fg = rgb("white");
            style.hud.key_bg = rgb("#505050");
            style.hud.mod_fg = rgb("white");
            style.hud.mod_font_weight = FontWeight::Regular;
            style.hud.mod_bg = rgb("#404040");
            style.hud.tag_fg = rgb("white");
            style.hud.opacity = 1.0;

            style.notify.width = 420.0;
            style.notify.pos = NotifyPos::Right;
            style.notify.opacity = 0.95;
            style.notify.timeout = 4.0;
            style.notify.buffer = 200;

            set_notify_style(
                &mut style.notify.info,
                "#222222",
                "white",
                "white",
                FontWeight::Bold,
            );
            set_notify_style(
                &mut style.notify.warn,
                "#442a00",
                "#ffc100",
                "#ffc100",
                FontWeight::Bold,
            );
            set_notify_style(
                &mut style.notify.error,
                "#3a0000",
                "#ff6666",
                "#ff6666",
                FontWeight::Bold,
            );
            set_notify_style(
                &mut style.notify.success,
                "#0c2d0c",
                "#8bff8b",
                "#8bff8b",
                FontWeight::Bold,
            );

            style
        }

        fn theme_dark_blue() -> Style {
            let mut style = Style::default();

            style.hud.pos = Pos::Center;
            style.hud.font_size = 16.0;
            style.hud.title_fg = rgb("#a0c4ff");
            style.hud.bg = rgb("#0a1628");
            style.hud.key_fg = rgb("#ffffff");
            style.hud.key_bg = rgb("#1e3a5f");
            style.hud.mod_fg = rgb("#a0c4ff");
            style.hud.mod_bg = rgb("#2c5282");
            style.hud.tag_fg = rgb("#63b3ed");
            style.hud.opacity = 0.95;

            style.notify.width = 420.0;
            style.notify.pos = NotifyPos::Right;
            style.notify.opacity = 0.9;
            style.notify.timeout = 4.0;
            style.notify.buffer = 200;

            set_notify_style(
                &mut style.notify.info,
                "#1a365d",
                "#a0c4ff",
                "#a0c4ff",
                FontWeight::Bold,
            );
            set_notify_style(
                &mut style.notify.warn,
                "#451a03",
                "#fbbf24",
                "#fbbf24",
                FontWeight::Bold,
            );
            set_notify_style(
                &mut style.notify.error,
                "#450a0a",
                "#f87171",
                "#f87171",
                FontWeight::Bold,
            );
            set_notify_style(
                &mut style.notify.success,
                "#052e16",
                "#86efac",
                "#86efac",
                FontWeight::Bold,
            );

            style
        }

        fn theme_solarized_dark() -> Style {
            let mut style = Style::default();

            style.hud.radius = 8.0;
            style.hud.pos = Pos::Center;
            style.hud.offset = Offset { x: 0.0, y: 0.0 };
            style.hud.font_size = 14.0;
            style.hud.title_font_weight = FontWeight::Regular;
            style.hud.key_font_size = 19.0;
            style.hud.tag_font_size = 20.0;
            style.hud.tag_font_weight = FontWeight::Regular;
            style.hud.title_fg = rgb("#93a1a1");
            style.hud.bg = rgb("#002b36");
            style.hud.key_radius = 4.0;
            style.hud.key_fg = rgb("#fdf6e3");
            style.hud.key_bg = rgb("#268bd2");
            style.hud.key_font_weight = FontWeight::Bold;
            style.hud.mod_fg = rgb("#eee8d5");
            style.hud.mod_font_weight = FontWeight::Regular;
            style.hud.mod_bg = rgb("#b58900");
            style.hud.tag_fg = rgb("#2aa198");
            style.hud.opacity = 0.95;

            style.notify.width = 420.0;
            style.notify.pos = NotifyPos::Right;
            style.notify.opacity = 0.9;
            style.notify.timeout = 4.0;
            style.notify.buffer = 200;

            set_notify_style(
                &mut style.notify.info,
                "#073642",
                "#93a1a1",
                "#839496",
                FontWeight::Bold,
            );
            set_notify_style(
                &mut style.notify.warn,
                "#073642",
                "#cb4b16",
                "#b58900",
                FontWeight::Bold,
            );
            set_notify_style(
                &mut style.notify.error,
                "#073642",
                "#dc322f",
                "#dc322f",
                FontWeight::Bold,
            );
            set_notify_style(
                &mut style.notify.success,
                "#073642",
                "#859900",
                "#859900",
                FontWeight::Bold,
            );

            style
        }

        fn theme_solarized_light() -> Style {
            let mut style = Style::default();

            style.hud.radius = 8.0;
            style.hud.pos = Pos::Center;
            style.hud.offset = Offset { x: 0.0, y: 0.0 };
            style.hud.font_size = 14.0;
            style.hud.title_font_weight = FontWeight::Regular;
            style.hud.key_font_size = 19.0;
            style.hud.tag_font_size = 20.0;
            style.hud.tag_font_weight = FontWeight::Regular;
            style.hud.title_fg = rgb("#586e75");
            style.hud.bg = rgb("#fdf6e3");
            style.hud.key_radius = 4.0;
            style.hud.key_fg = rgb("#fdf6e3");
            style.hud.key_bg = rgb("#6c71c4");
            style.hud.key_font_weight = FontWeight::Bold;
            style.hud.mod_fg = rgb("#073642");
            style.hud.mod_font_weight = FontWeight::Regular;
            style.hud.mod_bg = rgb("#b58900");
            style.hud.tag_fg = rgb("#2aa198");
            style.hud.opacity = 0.95;

            style.notify.width = 420.0;
            style.notify.pos = NotifyPos::Right;
            style.notify.opacity = 0.9;
            style.notify.timeout = 4.0;
            style.notify.buffer = 200;

            set_notify_style(
                &mut style.notify.info,
                "#eee8d5",
                "#586e75",
                "#657b83",
                FontWeight::Bold,
            );
            set_notify_style(
                &mut style.notify.warn,
                "#eee8d5",
                "#cb4b16",
                "#b58900",
                FontWeight::Bold,
            );
            set_notify_style(
                &mut style.notify.error,
                "#eee8d5",
                "#dc322f",
                "#dc322f",
                FontWeight::Bold,
            );
            set_notify_style(
                &mut style.notify.success,
                "#eee8d5",
                "#859900",
                "#859900",
                FontWeight::Bold,
            );

            style
        }

        let mut themes = HashMap::new();
        themes.insert("default".to_string(), theme_default());
        themes.insert("charcoal".to_string(), theme_charcoal());
        themes.insert("dark-blue".to_string(), theme_dark_blue());
        themes.insert("solarized-dark".to_string(), theme_solarized_dark());
        themes.insert("solarized-light".to_string(), theme_solarized_light());

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
    use crate::dynamic::load_dynamic_config_from_string;

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
        let cfg = load_dynamic_config_from_string(
            r#"
            style(#{
              hud: #{ font_size: 20.0, title_fg: "red" },
            });

            hotki.mode(|m, _ctx| {
              m.bind("a", "Test action", action.shell("echo test"));
            });
            "#
            .to_string(),
            None,
        )
        .expect("loads");

        let style = cfg.base_style(None, true);
        let hud = style.hud;

        // Verify user overrides are applied.
        assert_eq!(hud.font_size, 20.0);
        assert_eq!(hud.title_fg, (255, 0, 0));

        // Verify theme defaults are preserved for unspecified fields.
        assert_eq!(hud.bg, (0x10, 0x10, 0x10));
        assert_eq!(hud.opacity, 1.0);
    }

    #[test]
    fn test_empty_user_config_uses_theme_defaults() {
        let cfg =
            load_dynamic_config_from_string(r#"hotki.mode(|_m, _ctx| {});"#.to_string(), None)
                .expect("loads");

        let hud = cfg.base_style(None, true).hud;

        // Verify all theme defaults are used when there is no user overlay.
        assert_eq!(hud.font_size, 14.0);
        assert_eq!(hud.title_fg, (0xd0, 0xd0, 0xd0));
        assert_eq!(hud.bg, (0x10, 0x10, 0x10));
        assert_eq!(hud.opacity, 1.0);
    }

    #[test]
    fn test_theme_field_parsing() {
        let cfg_charcoal = load_dynamic_config_from_string(
            r#"
            base_theme("charcoal");
            hotki.mode(|_m, _ctx| {});
            "#
            .to_string(),
            None,
        )
        .expect("loads");
        let cfg_default =
            load_dynamic_config_from_string(r#"hotki.mode(|_m, _ctx| {});"#.to_string(), None)
                .expect("loads");

        let charcoal = cfg_charcoal.base_style(None, true).hud;
        let default = cfg_default.base_style(None, true).hud;

        // Sanity check: different base themes should result in different style values.
        assert_ne!(charcoal.title_fg, default.title_fg);
    }

    #[test]
    fn test_different_theme_as_base() {
        let cfg = load_dynamic_config_from_string(
            r#"
            base_theme("dark-blue");
            style(#{ hud: #{ font_size: 18.0 } });
            hotki.mode(|_m, _ctx| {});
            "#
            .to_string(),
            None,
        )
        .expect("loads");

        let hud = cfg.base_style(None, true).hud;

        // Verify user override is applied.
        assert_eq!(hud.font_size, 18.0);

        // Verify dark-blue theme values are used.
        assert_eq!(hud.title_fg, (0xa0, 0xc4, 0xff));
        assert_eq!(hud.bg, (0x0a, 0x16, 0x28));
    }

    #[test]
    fn test_theme_overlay_on_base_switch() {
        let default_cfg = load_dynamic_config_from_string(
            r#"
            base_theme("default");
            style(#{ hud: #{ font_size: 20.0 } });
            hotki.mode(|_m, _ctx| {});
            "#
            .to_string(),
            None,
        )
        .expect("loads");
        let dark_cfg = load_dynamic_config_from_string(
            r#"
            base_theme("dark-blue");
            style(#{ hud: #{ font_size: 20.0 } });
            hotki.mode(|_m, _ctx| {});
            "#
            .to_string(),
            None,
        )
        .expect("loads");

        let default_hud = default_cfg.base_style(None, true).hud;
        let dark_hud = dark_cfg.base_style(None, true).hud;

        // User override is applied on both base themes.
        assert_eq!(default_hud.font_size, 20.0);
        assert_eq!(dark_hud.font_size, 20.0);

        // Title color should come from the base theme (not overridden by user).
        assert_eq!(default_hud.title_fg, (0xd0, 0xd0, 0xd0));
        assert_eq!(dark_hud.title_fg, (0xa0, 0xc4, 0xff));
    }
}
