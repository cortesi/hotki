#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{
        NotifyPos, parse_rgb,
        style::{default_style, eval_style_source, overlay_raw},
    };

    #[test]
    fn style_overlay_hud_fields() {
        let base = default_style().expect("default style");

        let user_overlay = eval_style_source(
            r##"
                return {
                    hud = {
                        font_size = 20,
                        title_fg = "red",
                        bg = "#222222",
                    },
                }
            "##,
            Path::new("<test:hud-overlay>"),
        )
        .expect("HUD overlay");

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

        let user_overlay = eval_style_source(
            r##"
                return {
                    notify = {
                        pos = "left",
                        timeout = 3,
                        info = { bg = "#333333" },
                    },
                }
            "##,
            Path::new("<test:notify-overlay>"),
        )
        .expect("notification overlay");

        let final_style = overlay_raw(base.clone(), &user_overlay);

        assert_eq!(final_style.notify.timeout, 3.0);
        assert_eq!(final_style.notify.pos, NotifyPos::Left);
        let theme = final_style.notify.theme.clone();
        assert_eq!(theme.info.bg, parse_rgb("#333333").unwrap());
        // Width should remain from base
        assert_eq!(final_style.notify.width, base.notify.width);
    }
}
