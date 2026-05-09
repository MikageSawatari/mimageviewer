use std::sync::Arc;

pub const USER_TEXT_FAMILY_NAME: &str = "miv-user-text";

const JAPANESE_FONT_PATHS: &[&str] = &[
    r"C:\Windows\Fonts\YuGothM.ttc",
    r"C:\Windows\Fonts\meiryo.ttc",
    r"C:\Windows\Fonts\msgothic.ttc",
];

struct FallbackFont {
    name: &'static str,
    path: &'static str,
    scale: f32,
    y_offset_factor: f32,
}

const USER_TEXT_FALLBACKS: &[FallbackFont] = &[
    FallbackFont {
        name: "emoji",
        path: r"C:\Windows\Fonts\seguiemj.ttf",
        scale: 0.90,
        y_offset_factor: 0.06,
    },
    // Mathematical alphanumeric symbols such as 𝓈𝒸𝓇𝑒𝒶𝓂 are not covered by
    // Yu Gothic. A small downward tweak keeps them on the same visual baseline.
    FallbackFont {
        name: "math",
        path: r"C:\Windows\Fonts\cambria.ttc",
        scale: 0.98,
        y_offset_factor: 0.04,
    },
    FallbackFont {
        name: "historic",
        path: r"C:\Windows\Fonts\seguihis.ttf",
        scale: 0.96,
        y_offset_factor: 0.04,
    },
    FallbackFont {
        name: "symbols",
        path: r"C:\Windows\Fonts\seguisym.ttf",
        scale: 0.96,
        y_offset_factor: 0.05,
    },
];

pub fn user_text_font(size: f32) -> egui::FontId {
    egui::FontId::new(
        size,
        egui::FontFamily::Name(Arc::<str>::from(USER_TEXT_FAMILY_NAME)),
    )
}

pub fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    install_mimageviewer_fonts(&mut fonts);
    ctx.set_fonts(fonts);
}

pub fn install_mimageviewer_fonts(fonts: &mut egui::FontDefinitions) {
    let base_proportional = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut primary_names: Vec<String> = Vec::new();
    if install_japanese_font(fonts) {
        primary_names.push("japanese".to_owned());
    }

    let user_text_fallback_names = USER_TEXT_FALLBACKS
        .iter()
        .filter_map(|fallback| {
            install_fallback_font(fonts, fallback).then(|| fallback.name.to_owned())
        })
        .collect::<Vec<_>>();

    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let family_fonts = fonts.families.entry(family).or_default();
        for name in primary_names.iter().rev() {
            remove_font_name(family_fonts, name);
            family_fonts.insert(0, name.clone());
        }
        for name in &user_text_fallback_names {
            remove_font_name(family_fonts, name);
            family_fonts.push(name.clone());
        }
    }

    let mut user_text_fonts = Vec::new();
    extend_unique(&mut user_text_fonts, primary_names.iter().cloned());
    extend_unique(
        &mut user_text_fonts,
        user_text_fallback_names.iter().cloned(),
    );
    extend_unique(&mut user_text_fonts, base_proportional);
    fonts.families.insert(
        egui::FontFamily::Name(Arc::<str>::from(USER_TEXT_FAMILY_NAME)),
        user_text_fonts,
    );
}

fn install_japanese_font(fonts: &mut egui::FontDefinitions) -> bool {
    for path in JAPANESE_FONT_PATHS {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        fonts.font_data.insert(
            "japanese".to_owned(),
            Arc::new(egui::FontData::from_owned(data)),
        );
        return true;
    }
    false
}

fn install_fallback_font(fonts: &mut egui::FontDefinitions, fallback: &FallbackFont) -> bool {
    let Ok(data) = std::fs::read(fallback.path) else {
        return false;
    };
    fonts.font_data.insert(
        fallback.name.to_owned(),
        Arc::new(egui::FontData::from_owned(data).tweak(egui::FontTweak {
            scale: fallback.scale,
            y_offset_factor: fallback.y_offset_factor,
            ..Default::default()
        })),
    );
    true
}

fn remove_font_name(fonts: &mut Vec<String>, name: &str) {
    fonts.retain(|existing| existing != name);
}

fn extend_unique(fonts: &mut Vec<String>, names: impl IntoIterator<Item = String>) {
    for name in names {
        if !fonts.iter().any(|existing| existing == &name) {
            fonts.push(name);
        }
    }
}
