//! Windows system-font regressions for mIV §1.443. No window or application is started.

use super::*;
use crate::{FontFamily, FontId, Mesh, Pos2, Tessellator, TextShape, text::*};

/// A missing font fails the test instead of skipping it: mIV is Windows-only and these
/// fonts ship with Windows, so a skip would silently report a regression gate as passed.
fn font_bytes(filename: &str) -> Blob {
    let path =
        std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:/Windows".into()))
            .join("Fonts")
            .join(filename);
    match std::fs::read(&path) {
        Ok(bytes) => Arc::new(bytes),
        Err(error) => panic!(
            "Cannot read {} ({error}); the §1.443 regression needs this Windows font",
            path.display()
        ),
    }
}

fn check_font(filename: &str, index: u32, name: &str, emoji: bool) {
    let bytes = font_bytes(filename);
    let text = if emoji { "😀" } else { "A1あ日" };
    let mut face = FontFace::new(
        TextOptions::default(),
        name.into(),
        Arc::clone(&bytes),
        index,
        FontTweak::default(),
    )
    .unwrap();
    let mut definitions = FontDefinitions::empty();
    definitions
        .font_data
        .insert(name.into(), Arc::new(FontData::from_blob(bytes, index)));
    definitions
        .families
        .insert(FontFamily::Proportional, vec![name.into()]);
    let mut fonts = Fonts::new(TextOptions::default(), definitions);

    for pixels_per_point in [1.0, 1.5, 2.0] {
        for size in [12.0, 14.0, 16.0, 20.0, 32.0] {
            let metrics = face.styled_metrics(pixels_per_point, size, &Default::default());
            for chr in text.chars() {
                // Check the requested face directly: fallback/tofu must not make this pass.
                let GlyphIdResolution::Glyph(id) = face.glyph_id_resolution(chr).unwrap() else {
                    panic!("{name}: missing {chr}");
                };
                assert_ne!(id, GlyphId::NOTDEF, "{name}: {chr}");
                assert_eq!(face.is_color_glyph(&metrics, id), emoji, "{name}: {chr}");
                let bitmap = face
                    .rasterize_glyph(&metrics, id, SubpixelBin::Zero)
                    .unwrap();
                assert_eq!(bitmap.is_color, emoji, "{name}: {chr}");
                assert!(bitmap.image.pixels.iter().any(|p| p.a() != 0));
                if emoji {
                    // Real color survives; a white/gray coverage image cannot satisfy this.
                    assert!(
                        bitmap
                            .image
                            .pixels
                            .iter()
                            .any(|p| { p.a() != 0 && (p.r() != p.g() || p.g() != p.b()) })
                    );
                }
            }

            // Ordinary text, selection color, and disabled color all use the same mask.
            for color in [
                Color32::BLACK,
                Color32::from_rgb(20, 80, 160),
                Color32::GRAY,
            ] {
                let galley = fonts
                    .with_pixels_per_point(pixels_per_point)
                    .layout_no_wrap(text.into(), FontId::proportional(size), color);
                for override_color in [None, Some(Color32::from_rgb(160, 30, 60))] {
                    let mut shape = TextShape::new(Pos2::ZERO, Arc::clone(&galley), color);
                    shape.override_text_color = override_color;
                    let mut mesh = Mesh::default();
                    Tessellator::new(pixels_per_point, Default::default(), [2048, 2048], vec![])
                        .tessellate_text(&shape, &mut mesh);
                    assert!(!mesh.vertices.is_empty(), "{name}");
                    let expected = if emoji {
                        Color32::WHITE
                    } else {
                        override_color.unwrap_or(color)
                    };
                    assert!(mesh.vertices.iter().all(|v| v.color == expected), "{name}");
                    let glyphs: Vec<_> = galley.rows.iter().flat_map(|r| &r.row.glyphs).collect();
                    assert_eq!(glyphs.len(), text.chars().count(), "{name}");
                    for glyph in glyphs {
                        assert!(!glyph.uv_rect.is_nothing(), "{name}: {}", glyph.chr);
                        assert_eq!(glyph.is_color, emoji, "{name}: {}", glyph.chr);
                    }
                }
            }
        }
    }
    eprintln!("PASS: {name} ({filename} face {index}), 5 sizes × 3 scales");
}

macro_rules! font_test {
    ($test:ident, $file:literal, $index:literal, $name:literal) => {
        #[test]
        fn $test() {
            check_font($file, $index, $name, false);
        }
    };
}

font_test!(ms_gothic, "msgothic.ttc", 0, "MS Gothic");
font_test!(ms_pgothic, "msgothic.ttc", 2, "MS PGothic");
font_test!(ms_ui_gothic, "msgothic.ttc", 1, "MS UI Gothic");
font_test!(ms_mincho, "msmincho.ttc", 0, "MS Mincho");
font_test!(ms_pmincho, "msmincho.ttc", 1, "MS PMincho");
font_test!(simsun, "simsun.ttc", 0, "SimSun");
font_test!(nsimsun, "simsun.ttc", 1, "NSimSun");
font_test!(meiryo_control, "meiryo.ttc", 0, "Meiryo");

#[test]
fn segoe_ui_emoji_keeps_color() {
    check_font("seguiemj.ttf", 0, "Segoe UI Emoji", true);
}
