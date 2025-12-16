#[cfg(test)]
mod tests {
    use crate::{
        parse_rgb,
        raw::{Maybe, RawHud, RawNotify, RawNotifyStyle, RawStyle},
        themes::load_theme,
    };

    #[test]
    fn raw_style_merge_empty_is_empty() {
        let left = RawStyle::default();
        let right = RawStyle::default();
        assert_eq!(left.merge(&right), RawStyle::default());
    }

    #[test]
    fn raw_style_merge_prefers_right_side_values() {
        let left = RawStyle {
            hud: Maybe::Value(RawHud {
                font_size: Maybe::Value(10.0),
                ..RawHud::default()
            }),
            ..RawStyle::default()
        };
        let right = RawStyle {
            hud: Maybe::Value(RawHud {
                font_size: Maybe::Value(20.0),
                ..RawHud::default()
            }),
            ..RawStyle::default()
        };

        let merged = left.merge(&right);
        assert_eq!(
            merged
                .hud
                .as_option()
                .and_then(|h| h.font_size.as_option().copied()),
            Some(20.0)
        );
    }

    #[test]
    fn raw_style_merge_combines_nested_sections() {
        let left = RawStyle {
            hud: Maybe::Value(RawHud {
                font_size: Maybe::Value(10.0),
                ..RawHud::default()
            }),
            ..RawStyle::default()
        };
        let right = RawStyle {
            hud: Maybe::Value(RawHud {
                opacity: Maybe::Value(0.5),
                ..RawHud::default()
            }),
            ..RawStyle::default()
        };

        let merged = left.merge(&right);
        let hud = merged.hud.as_option().expect("hud section");
        assert_eq!(hud.font_size.as_option().copied(), Some(10.0));
        assert_eq!(hud.opacity.as_option().copied(), Some(0.5));
    }

    #[test]
    fn raw_style_merge_merges_nested_notify_styles() {
        let left = RawStyle {
            notify: Maybe::Value(RawNotify {
                timeout: Maybe::Value(2.0),
                info: Maybe::Value(RawNotifyStyle {
                    bg: Maybe::Value("#111111".to_string()),
                    ..RawNotifyStyle::default()
                }),
                ..RawNotify::default()
            }),
            ..RawStyle::default()
        };
        let right = RawStyle {
            notify: Maybe::Value(RawNotify {
                timeout: Maybe::Value(3.0),
                info: Maybe::Value(RawNotifyStyle {
                    title_fg: Maybe::Value("white".to_string()),
                    ..RawNotifyStyle::default()
                }),
                ..RawNotify::default()
            }),
            ..RawStyle::default()
        };

        let merged = left.merge(&right);
        let notify = merged.notify.as_option().expect("notify section");
        assert_eq!(notify.timeout.as_option().copied(), Some(3.0));

        let info = notify.info.as_option().expect("notify.info");
        assert_eq!(info.bg.as_option().map(String::as_str), Some("#111111"));
        assert_eq!(info.title_fg.as_option().map(String::as_str), Some("white"));
    }

    #[test]
    fn theme_overlay_hud_fields() {
        let base = load_theme(None);

        // User overrides some HUD fields via raw overlay form
        let user_overlay = RawStyle {
            hud: Maybe::Value(RawHud {
                font_size: Maybe::Value(20.0),
                title_fg: Maybe::Value("red".to_string()),
                bg: Maybe::Value("#222222".to_string()),
                ..RawHud::default()
            }),
            ..RawStyle::default()
        };

        let final_style = base.clone().overlay_raw(&user_overlay);

        assert_eq!(final_style.hud.font_size, 20.0);
        assert_eq!(final_style.hud.title_fg, parse_rgb("red").unwrap());
        assert_eq!(final_style.hud.bg, parse_rgb("#222222").unwrap());
        // A field not overridden should remain from base
        assert_eq!(final_style.hud.opacity, base.hud.opacity);
    }

    #[test]
    fn theme_overlay_notify_fields() {
        let base = load_theme(None);

        // User overrides notification timeout and some style bits (raw form)
        let user_overlay = RawStyle {
            notify: Maybe::Value(RawNotify {
                timeout: Maybe::Value(3.0),
                info: Maybe::Value(RawNotifyStyle {
                    bg: Maybe::Value("#333333".to_string()),
                    ..RawNotifyStyle::default()
                }),
                ..RawNotify::default()
            }),
            ..RawStyle::default()
        };

        let final_style = base.clone().overlay_raw(&user_overlay);

        assert_eq!(final_style.notify.timeout, 3.0);
        let theme = final_style.notify.theme();
        assert_eq!(theme.info.bg, parse_rgb("#333333").unwrap());
        // Width should remain from base
        assert_eq!(final_style.notify.width, base.notify.width);
    }
}
