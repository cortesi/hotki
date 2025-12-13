#[cfg(test)]
mod tests {
    use crate::{Config, parse_rgb, raw::RawStyle};

    #[test]
    fn theme_overlay_hud_fields() {
        // Base from default theme
        let base = Config::default();

        // User overrides some HUD fields via raw overlay form
        let user_overlay = ron::from_str::<RawStyle>(
            "(hud: (font_size: 20.0, title_fg: \"red\", bg: \"#222222\"))",
        )
        .unwrap();

        let final_style = base.style.clone().overlay_raw(&user_overlay);

        assert_eq!(final_style.hud.font_size, 20.0);
        assert_eq!(final_style.hud.title_fg, parse_rgb("red").unwrap());
        assert_eq!(final_style.hud.bg, parse_rgb("#222222").unwrap());
        // A field not overridden should remain from base
        assert_eq!(final_style.hud.opacity, base.style.hud.opacity);
    }

    #[test]
    fn theme_overlay_notify_fields() {
        // Base from default theme
        let base = Config::default();

        // User overrides notification timeout and some style bits (raw form)
        let user_overlay =
            ron::from_str::<RawStyle>("(notify: (timeout: 3.0, info: (bg: \"#333333\")))").unwrap();

        let final_style = base.style.clone().overlay_raw(&user_overlay);

        assert_eq!(final_style.notify.timeout, 3.0);
        let theme = final_style.notify.theme();
        assert_eq!(theme.info.bg, parse_rgb("#333333").unwrap());
        // Width should remain from base
        assert_eq!(final_style.notify.width, base.style.notify.width);
    }
}
