use std::sync::Arc;

pub const USER_TEXT_FAMILY_NAME: &str = "miv-user-text";
const DERIVED_Y_OFFSET_CLAMP: f32 = 0.24;

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
    // Text-presentation symbols such as ✉ and ⋈ should follow browser-like
    // text fallback instead of being captured by emoji/math fonts.
    FallbackFont {
        name: "text_symbols",
        path: r"C:\Windows\Fonts\meiryo.ttc",
        scale: 1.0,
        y_offset: FallbackYOffset::Fixed(-0.20),
    },
    // Mathematical alphanumeric symbols such as 𝓈𝒸𝓇𝑒𝒶𝓂 are not covered by
    // Yu Gothic or Meiryo. Keep this before emoji so script letters stay texty.
    FallbackFont {
        name: "math",
        path: r"C:\Windows\Fonts\cambria.ttc",
        scale: 0.98,
        y_offset: FallbackYOffset::Fixed(0.04),
    },
    FallbackFont {
        name: "emoji",
        path: r"C:\Windows\Fonts\seguiemj.ttf",
        scale: 0.90,
        // Target: leading emoji in metadata lines should visually sit on the
        // same center line as Japanese body text, not below it. Compute the
        // tweak from real glyph bounds to avoid hand-tuning magic numbers.
        y_offset: FallbackYOffset::AlignGlyphCenter {
            samples: &['🐾', '🧠', '🍧', '💗'],
            fallback: -0.12,
        },
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
                // egui 0.33 applies y_offset_factor after multiplying it by
                // FontTweak::scale. Font coordinates are positive-up, so aligning
                // normalized glyph centers yields:
                //   factor * size * scale - fallback_center * size * scale
                //     == -target_center * size
                let factor = (fallback_center - target.center_y / scale)
                    .clamp(-DERIVED_Y_OFFSET_CLAMP, DERIVED_Y_OFFSET_CLAMP);
                crate::logger::log(format!(
                    "ui_fonts: {} alignment derived target_center={:.4} fallback_center={:.4} scale={:.3} factor={:.4} samples={}",
                    fallback.name,
                    target.center_y,
                    fallback_center,
                    scale,
                    factor,
                    samples.iter().collect::<String>()
                ));
                Some(factor)
            })
            .unwrap_or(fallback_offset),
    }
}

fn font_center_y_for_samples(data: &[u8], samples: &[char]) -> Option<f32> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    let mut centers = samples
        .iter()
        .filter_map(|sample| glyph_center_y(&face, *sample))
        .collect::<Vec<_>>();
    if centers.is_empty() {
        return None;
    }
    centers.sort_by(|a, b| a.total_cmp(b));
    let mid = centers.len() / 2;
    if centers.len() % 2 == 0 {
        Some((centers[mid - 1] + centers[mid]) * 0.5)
    } else {
        Some(centers[mid])
    }
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
        let body_text = font_center_y_for_samples(&japanese, &['今', 'あ'])
            .or_else(|| font_metric_center_y(&japanese))
            .map(|center_y| FontAlignmentTarget { center_y })
            .expect("Japanese glyph center should be measurable");
        let emoji_font = USER_TEXT_FALLBACKS
            .iter()
            .find(|font| font.name == "emoji")
            .expect("emoji fallback definition should exist");

        let samples = ['🐾', '🧠', '🍧', '💗'];
        let fallback_center =
            font_center_y_for_samples(&emoji, &samples).expect("emoji center should be measurable");
        let offset = fallback_y_offset_factor(emoji_font, &emoji, Some(body_text));
        let expected = (fallback_center - body_text.center_y / emoji_font.scale)
            .clamp(-DERIVED_Y_OFFSET_CLAMP, DERIVED_Y_OFFSET_CLAMP);
        assert!(
            (offset - expected).abs() < f32::EPSILON,
            "emoji offset should follow egui's scaled y_offset formula: {offset} vs {expected}",
        );
        assert!(
            (-0.24..=0.0).contains(&offset),
            "emoji should be nudged upward from measured glyph centers, got {offset}",
        );
    }

    #[test]
    fn user_text_prefers_text_symbols_before_emoji() {
        let mut fonts = egui::FontDefinitions::default();
        install_mimageviewer_fonts(&mut fonts);
        let family = fonts
            .families
            .get(&egui::FontFamily::Name(Arc::<str>::from(
                USER_TEXT_FAMILY_NAME,
            )))
            .expect("user text family should be registered");
        let text_symbols = family
            .iter()
            .position(|name| name == "text_symbols")
            .expect("text symbol fallback should be registered");
        let emoji = family
            .iter()
            .position(|name| name == "emoji")
            .expect("emoji fallback should be registered");
        let math = family
            .iter()
            .position(|name| name == "math")
            .expect("math fallback should be registered");
        assert!(
            text_symbols < math && math < emoji,
            "text-presentation symbols should be tried before math and emoji: {family:?}",
        );
    }
}
