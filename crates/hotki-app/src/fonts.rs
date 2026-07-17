//! Font helpers and embedded faces.
use hotki_protocol::FontWeight;

/// Embedded 0xProto Nerd Font Mono (Regular).
static PROTO_REGULAR_TTF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/0xProtoNerdFontMono-Regular.ttf"
));
/// Embedded 0xProto Nerd Font Mono (Bold).
static PROTO_BOLD_TTF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/0xProtoNerdFontMono-Bold.ttf"
));
/// Embedded 0xProto Nerd Font Mono (Italic).
static PROTO_ITALIC_TTF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/0xProtoNerdFontMono-Italic.ttf"
));

/// Install the embedded fonts and set family mappings in the `egui` context.
pub fn install_fonts(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
}

/// Build Hotki's named font families without replacing normal proportional text.
fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "Regular".to_owned(),
        egui::FontData::from_static(PROTO_REGULAR_TTF).into(),
    );
    fonts.font_data.insert(
        "Bold".to_owned(),
        egui::FontData::from_static(PROTO_BOLD_TTF).into(),
    );
    fonts.font_data.insert(
        "Italic".to_owned(),
        egui::FontData::from_static(PROTO_ITALIC_TTF).into(),
    );

    for name in ["Light", "Regular", "Medium"] {
        fonts.families.insert(
            egui::FontFamily::Name(name.into()),
            vec!["Regular".to_owned()],
        );
    }
    for name in ["SemiBold", "Bold", "ExtraBold"] {
        fonts
            .families
            .insert(egui::FontFamily::Name(name.into()), vec!["Bold".to_owned()]);
    }
    fonts.families.insert(
        egui::FontFamily::Name("Italic".into()),
        vec!["Italic".to_owned()],
    );

    let monospace = fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default();
    monospace.retain(|name| name != "Regular");
    monospace.insert(0, "Regular".to_owned());
    fonts
}

/// Choose an `egui::FontFamily` name for a configuration weight.
pub fn weight_family(w: FontWeight) -> egui::FontFamily {
    egui::FontFamily::Name(
        match w {
            FontWeight::Thin => "Light",
            FontWeight::ExtraLight => "Light",
            FontWeight::Light => "Light",
            FontWeight::Regular => "Regular",
            FontWeight::Medium => "Medium",
            FontWeight::SemiBold => "SemiBold",
            FontWeight::Bold => "Bold",
            FontWeight::ExtraBold => "ExtraBold",
            FontWeight::Black => "ExtraBold",
        }
        .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::font_definitions;

    #[test]
    fn ordinary_text_keeps_default_proportional_face() {
        let fonts = font_definitions();
        let proportional = fonts
            .families
            .get(&egui::FontFamily::Proportional)
            .expect("default proportional family");
        let monospace = fonts
            .families
            .get(&egui::FontFamily::Monospace)
            .expect("default monospace family");

        assert!(!proportional.iter().any(|name| name == "Regular"));
        assert_eq!(monospace.first().map(String::as_str), Some("Regular"));
    }
}
