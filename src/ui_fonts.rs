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
    y_offset: FallbackYOffset,
}

#[derive(Clone, Copy)]
enum FallbackYOffset {
    Fixed(f32),
    AlignGlyphCenter {
        samples: &'static [char],
        fallback: f32,
    },
}

#[derive(Clone, Copy)]
struct FontAlignmentTarget {
    center_y: f32,
}

const USER_TEXT_FALLBACKS: &[FallbackFont] = &[
    FallbackFont {
        name: "emoji",
        path: r"C:\Windows\Fonts\seguiemj.ttf",
        scale: 0.90,
        // Target: leading emoji in metadata lines should visually sit on the
        // same center line as Japanese body text, not below it. Compute the
        // tweak from real glyph bounds to avoid hand-tuning magic numbers.
        y_offset: FallbackYOffset::AlignGlyphCenter {
            samples: &['🧠', '🍧', '💗'],
            fallback: -0.12,
        },
    },
    // Mathematical alphanumeric symbols such as 𝓈𝒸𝓇𝑒𝒶𝓂 are not covered by
    // Yu Gothic. A small downward tweak keeps them on the same visual baseline.
    FallbackFont {
        name: "math",
        path: r"C:\Windows\Fonts\cambria.ttc",
        scale: 0.98,
        y_offset: FallbackYOffset::Fixed(0.04),
    },
    FallbackFont {
        name: "historic",
        path: r"C:\Windows\Fonts\seguihis.ttf",
        scale: 0.96,
        y_offset: FallbackYOffset::Fixed(0.04),
    },
    FallbackFont {
        name: "symbols",
        path: r"C:\Windows\Fonts\seguisym.ttf",
        scale: 0.96,
        y_offset: FallbackYOffset::Fixed(0.05),
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
    let alignment_target = install_japanese_font(fonts);
    if alignment_target.is_some() {
        primary_names.push("japanese".to_owned());
    }

    let user_text_fallback_names = USER_TEXT_FALLBACKS
        .iter()
        .filter_map(|fallback| {
            install_fallback_font(fonts, fallback, alignment_target)
                .then(|| fallback.name.to_owned())
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

fn install_japanese_font(fonts: &mut egui::FontDefinitions) -> Option<FontAlignmentTarget> {
    for path in JAPANESE_FONT_PATHS {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        let alignment_target = font_center_y_for_samples(&data, &['今', 'あ'])
            .or_else(|| font_metric_center_y(&data))
            .map(|center_y| FontAlignmentTarget { center_y });
        fonts.font_data.insert(
            "japanese".to_owned(),
            Arc::new(egui::FontData::from_owned(data)),
        );
        return alignment_target;
    }
    None
}

fn install_fallback_font(
    fonts: &mut egui::FontDefinitions,
    fallback: &FallbackFont,
    alignment_target: Option<FontAlignmentTarget>,
) -> bool {
    let Ok(data) = std::fs::read(fallback.path) else {
        return false;
    };
    let y_offset_factor = fallback_y_offset_factor(fallback, &data, alignment_target);
    fonts.font_data.insert(
        fallback.name.to_owned(),
        Arc::new(egui::FontData::from_owned(data).tweak(egui::FontTweak {
            scale: fallback.scale,
            y_offset_factor,
            ..Default::default()
        })),
    );
    true
}

fn fallback_y_offset_factor(
    fallback: &FallbackFont,
    data: &[u8],
    alignment_target: Option<FontAlignmentTarget>,
) -> f32 {
    match fallback.y_offset {
        FallbackYOffset::Fixed(value) => value,
        FallbackYOffset::AlignGlyphCenter {
            samples,
            fallback: fallback_offset,
        } => alignment_target
            .and_then(|target| {
                let fallback_center = font_center_y_for_samples(data, samples)?;
                let scale = fallback.scale.max(0.01);
                // egui applies y_offset_factor in screen-space points after glyph scaling.
                // Font coordinates are positive-up, so aligning normalized glyph centers yields:
                //   -fallback_center * size * scale + size * scale * factor
                //     == -target_center * size
                Some((fallback_center - target.center_y / scale).clamp(-0.24, 0.24))
            })
            .unwrap_or(fallback_offset),
    }
}

fn font_center_y_for_samples(data: &[u8], samples: &[char]) -> Option<f32> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    let mut total = 0.0;
    let mut count = 0.0;
    for sample in samples {
        if let Some(center_y) = glyph_center_y(&face, *sample) {
            total += center_y;
            count += 1.0;
        }
    }
    (count > 0.0).then_some(total / count)
}

fn font_metric_center_y(data: &[u8]) -> Option<f32> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    let units_per_em = f32::from(face.units_per_em());
    let ascender = face
        .typographic_ascender()
        .unwrap_or_else(|| face.ascender()) as f32;
    let descender = face
        .typographic_descender()
        .unwrap_or_else(|| face.descender()) as f32;
    Some((ascender + descender) * 0.5 / units_per_em)
}

fn glyph_center_y(face: &ttf_parser::Face<'_>, sample: char) -> Option<f32> {
    let glyph_id = face.glyph_index(sample)?;
    let units_per_em = f32::from(face.units_per_em());
    if let Some(rect) = face.glyph_bounding_box(glyph_id) {
        return Some((f32::from(rect.y_min) + f32::from(rect.y_max)) * 0.5 / units_per_em);
    }
    let image = face.glyph_raster_image(glyph_id, face.units_per_em())?;
    Some((f32::from(image.y) + f32::from(image.height) * 0.5) / f32::from(image.pixels_per_em))
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn emoji_offset_is_derived_from_real_glyph_centers() {
        let japanese = JAPANESE_FONT_PATHS
            .iter()
            .find_map(|path| std::fs::read(path).ok())
            .expect("Windows Japanese font should be available");
        let emoji = std::fs::read(r"C:\Windows\Fonts\seguiemj.ttf")
            .expect("Segoe UI Emoji should be available");
        let target = font_center_y_for_samples(&japanese, &['今', 'あ'])
            .or_else(|| font_metric_center_y(&japanese))
            .map(|center_y| FontAlignmentTarget { center_y })
            .expect("Japanese glyph center should be measurable");
        let emoji_font = USER_TEXT_FALLBACKS
            .iter()
            .find(|font| font.name == "emoji")
            .expect("emoji fallback definition should exist");

        assert!(font_center_y_for_samples(&emoji, &['🧠', '🍧', '💗']).is_some());
        let offset = fallback_y_offset_factor(emoji_font, &emoji, Some(target));
        assert!(
            (-0.24..=0.0).contains(&offset),
            "emoji should be nudged upward from measured glyph centers, got {offset}",
        );
    }
}
