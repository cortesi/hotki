// Embedded 0xProto Nerd Font Mono
static PROTO_REGULAR_TTF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/0xProtoNerdFontMono-Regular.ttf"
));
static PROTO_BOLD_TTF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/0xProtoNerdFontMono-Bold.ttf"
));
static PROTO_ITALIC_TTF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/0xProtoNerdFontMono-Italic.ttf"
));

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // Register available faces
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

    // Map weight names to available font faces (we only have Regular and Bold)
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

    // Global families: prefix our fonts but preserve egui's default fallback chain
    // so symbols/emoji can still resolve if not present in our face.
    let mut prepend = |fam: egui::FontFamily| {
        let mut v = fonts.families.get(&fam).cloned().unwrap_or_else(Vec::new);
        // Prepend in priority order, avoiding duplicates if present
        for name in ["Regular", "Bold", "Italic"].iter().rev() {
            if let Some(pos) = v.iter().position(|s| s == *name) {
                v.remove(pos);
            }
            v.insert(0, (*name).to_owned());
        }
        fonts.families.insert(fam, v);
    };
    prepend(egui::FontFamily::Proportional);
    prepend(egui::FontFamily::Monospace);
    ctx.set_fonts(fonts);
}

use config::FontWeight;

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
