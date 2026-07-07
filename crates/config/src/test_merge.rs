#[cfg(test)]
mod tests {
    use crate::{
        NotifyPos, parse_rgb,
        raw::{Maybe, RawHud, RawNotify, RawNotifyStyle, RawStyle},
        style::{default_style, overlay_raw},
    };

    #[test]
    fn style_overlay_hud_fields() {
        let base = default_style().expect("default style");

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

        let final_style = overlay_raw(base.clone(), &user_overlay);

        assert_eq!(final_style.hud.font_size, 20.0);
        assert_eq!(final_style.hud.title_fg, parse_rgb("red").unwrap());
        assert_eq!(final_style.hud.bg, parse_rgb("#222222").unwrap());
        // A field not overridden should remain from base
        assert_eq!(final_style.hud.opacity, base.hud.opacity);
    }

    #[test]
    fn style_overlay_notify_fields() {
        let base = default_style().expect("default style");

        // User overrides notification timeout and some style bits (raw form)
        let user_overlay = RawStyle {
            notify: Maybe::Value(RawNotify {
                pos: Maybe::Value(NotifyPos::Left),
                timeout: Maybe::Value(3.0),
                info: Maybe::Value(RawNotifyStyle {
                    bg: Maybe::Value("#333333".to_string()),
                    ..RawNotifyStyle::default()
                }),
                ..RawNotify::default()
            }),
            ..RawStyle::default()
        };

        let final_style = overlay_raw(base.clone(), &user_overlay);

        assert_eq!(final_style.notify.timeout, 3.0);
        assert_eq!(final_style.notify.pos, NotifyPos::Left);
        let theme = final_style.notify.theme.clone();
        assert_eq!(theme.info.bg, parse_rgb("#333333").unwrap());
        // Width should remain from base
        assert_eq!(final_style.notify.width, base.notify.width);
    }
}
