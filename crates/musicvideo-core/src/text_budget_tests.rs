use super::*;
use comic_core::{
    Rgba, StrokeStyle, TextBackgroundStyle, TextBlock, TextEchoStyle, TextShadowStyle, layout_text,
    prepare_text_regions,
};
use std::collections::HashSet;
use std::sync::Arc;

fn fonts() -> Vec<LoadedFont> {
    let mut fonts = vec![
        LoadedFont::from_bytes(
            "embedded ZenKurenaido",
            include_bytes!("../../../tools/comic_lab/assets/fonts/ZenKurenaido-Regular.ttf")
                .to_vec(),
        )
        .unwrap(),
    ];
    // The embedded face makes the regression portable. Exercise TTC and a
    // second outline geometry on Windows too, without requiring system fonts
    // on other hosts.
    for path in [
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\arial.ttf",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.push(LoadedFont::from_bytes(path, bytes).unwrap());
        }
    }
    fonts
}

fn stroke(width_px: f32) -> StrokeStyle {
    StrokeStyle {
        color: Rgba::BLACK,
        width_px,
    }
}

fn styled() -> TextStyle {
    TextStyle {
        outline: Some(stroke(2.25)),
        extra_outlines: vec![stroke(4.5), stroke(2.25)],
        shadow: Some(TextShadowStyle {
            color: Rgba::BLACK,
            offset: (3.0, 5.0),
            spread_px: 3.5,
            blur_px: 48.0,
        }),
        echo: Some(TextEchoStyle {
            color: Rgba::BLACK,
            offset: (-2.0, 1.0),
            count: 99,
        }),
        background: Some(TextBackgroundStyle {
            fill: Rgba::BLACK,
            padding_px: 5.0,
            corner_px: 8.0,
        }),
        ..TextStyle::default()
    }
}

/// Compute actual unique cached coverage and peak float scratch from real
/// rasterized dimensions. This deliberately does not use the estimator's
/// pass/dimension helpers. The tested shadow has 8 blur passes plus its core.
fn measured_cache_and_scratch(font: &LoadedFont, layout: &TextLayout, style: &TextStyle) -> usize {
    let mut dilation = vec![0.0];
    for outline in style.extra_outlines.iter().chain(style.outline.iter()) {
        if outline.width_px > 0.0 && outline.color.a > 0 {
            dilation.push(outline.width_px);
        }
    }
    if let Some(shadow) = style.shadow {
        let spread = shadow.spread_px.max(0.0);
        let blur = shadow.blur_px.clamp(0.0, 48.0);
        dilation.push(spread);
        if blur > 0.1 {
            let steps = ((blur / 3.0).ceil() as usize).clamp(2, 8);
            for step in 1..=steps {
                dilation.push(spread + blur * (step as f32 / steps as f32));
            }
        }
    }
    let mut seen = HashSet::new();
    let mut cached = 0;
    let mut scratch = 0;
    let mut rotated = 0;
    for glyph in &layout.glyphs {
        for &d in &dilation {
            if let Some(bitmap) = font.rasterize_gid(glyph.glyph_id, glyph.size, d) {
                let bytes = bitmap.coverage.capacity() * 4;
                if seen.insert(Arc::as_ptr(&bitmap)) {
                    cached += bytes;
                }
                scratch = scratch.max(if d == 0.0 { bytes + 16 } else { bytes });
                if glyph.form == GlyphForm::Sideways {
                    rotated += bytes;
                    if d == 0.0 {
                        rotated += bytes
                            * style
                                .echo
                                .map_or(0, |echo| echo.count.clamp(1, 12) as usize);
                    }
                }
            }
        }
    }
    cached + rotated + scratch
}

#[test]
fn d17_eleven_preparation_bound_covers_measured_fonts_styles_and_sideways() {
    for font in fonts() {
        for size in [23.5, 48.0, 100.0] {
            for style in [
                TextStyle::default(),
                styled(),
                TextStyle {
                    shadow: Some(TextShadowStyle {
                        blur_px: 0.1,
                        ..styled().shadow.unwrap()
                    }),
                    echo: Some(TextEchoStyle {
                        count: 0,
                        ..styled().echo.unwrap()
                    }),
                    ..TextStyle::default()
                },
                TextStyle {
                    shadow: Some(TextShadowStyle {
                        blur_px: 999.0,
                        ..styled().shadow.unwrap()
                    }),
                    ..TextStyle::default()
                },
            ] {
                let glyph = TextBlock {
                    text: "春夏秋冬 ABC Ágj 春春".into(),
                    size_px: size,
                    color: Rgba::WHITE,
                    ..TextBlock::default()
                };
                let decoration = TextBlock {
                    color: Rgba::TRANSPARENT,
                    outline: style.outline,
                    extra_outlines: style.extra_outlines.clone(),
                    shadow: style.shadow,
                    echo: style.echo,
                    background: style.background,
                    ..glyph.clone()
                };
                let mut layout = layout_text(&glyph, &font);
                for glyph in layout.glyphs.iter_mut().step_by(3) {
                    glyph.form = GlyphForm::Sideways;
                    glyph.sideways = true;
                }
                let bound = preparation_bound(&font, &layout, &style);
                let prepared =
                    prepare_text_regions(&layout, &font, (0.0, 0.0), &[&glyph, &decoration]);
                let retained = prepared.iter().map(|p| p.retained_bytes()).sum::<usize>();
                let cache_and_scratch = measured_cache_and_scratch(&font, &layout, &style);
                eprintln!(
                    "{} size={size}: bound={bound}, retained={retained}, cache+scratch={cache_and_scratch}",
                    font.key
                );
                assert!(bound >= retained, "{} size={size}", font.key);
                assert!(bound >= cache_and_scratch, "{} size={size}", font.key);
                // This matrix includes 100px text, 12 echoes and maximum blur;
                // the no-sharing upper bound may legitimately reject it.
            }
        }
    }
}

#[test]
fn d17_eleven_preparation_bound_rejects_scale_99_repro_without_rasterizing() {
    for font in fonts() {
        let block = TextBlock {
            text: "春夏秋冬東西南北天地山川海空風雨雪花鳥月".into(),
            size_px: 1000.0,
            ..TextBlock::default()
        };
        let style = TextStyle {
            outline: Some(stroke(128.0)),
            shadow: Some(TextShadowStyle {
                spread_px: 128.0,
                blur_px: 48.0,
                ..styled().shadow.unwrap()
            }),
            ..TextStyle::default()
        };
        let bound = preparation_bound(&font, &layout_text(&block, &font), &style);
        eprintln!("{} repro bound={bound}", font.key);
        assert!(bound > MAX_TEXT_PREPARATION_BYTES);
    }
}

#[test]
fn d17_eleven_preparation_bound_counts_every_repeated_upright_occurrence() {
    let font = fonts().remove(0);
    let block = TextBlock {
        text: "春".repeat(120),
        size_px: 100.0,
        ..TextBlock::default()
    };
    let layout = layout_text(&block, &font);
    let style = styled();
    let bound = preparation_bound(&font, &layout, &style);
    assert!(bound > MAX_TEXT_PREPARATION_BYTES);
    let mut sideways = layout;
    for glyph in &mut sideways.glyphs {
        glyph.form = GlyphForm::Sideways;
    }
    assert!(preparation_bound(&font, &sideways, &style) > bound);
}

#[test]
fn d17_eleven_ordinary_lyrics_and_titles_fit_without_assuming_shared_coverage() {
    for font in fonts() {
        for text in [
            "春夏秋冬東西南北天地山川海空風雨雪花鳥月",
            "夜空に響く歌 - Song Title",
        ] {
            for size in [48.0, 72.0] {
                let block = TextBlock {
                    text: text.into(),
                    size_px: size,
                    ..TextBlock::default()
                };
                let style = TextStyle {
                    outline: Some(stroke(2.0)),
                    shadow: Some(TextShadowStyle {
                        spread_px: 1.0,
                        blur_px: 8.0,
                        ..styled().shadow.unwrap()
                    }),
                    ..TextStyle::default()
                };
                let bound = preparation_bound(&font, &layout_text(&block, &font), &style);
                eprintln!("ordinary {} size={size}: bound={bound}", font.key);
                assert!(bound < MAX_TEXT_PREPARATION_BYTES);
            }
        }
    }
}

#[test]
fn d17_eleven_preparation_bound_covers_generation_reset_before_every_call() {
    let font = fonts().remove(0);
    let block = TextBlock {
        text: "春".repeat(20),
        size_px: 48.0,
        ..TextBlock::default()
    };
    let style = TextStyle {
        outline: Some(stroke(2.0)),
        echo: Some(TextEchoStyle {
            count: 12,
            ..TextEchoStyle::default()
        }),
        ..TextStyle::default()
    };
    let layout = layout_text(&block, &font);
    let bound = preparation_bound(&font, &layout, &style);
    let mut held = Vec::new();
    let mut output_bytes = 0;
    let mut largest_base = 0;
    let mut scratch = 0;
    // White body, twelve echoes and one outline. Reset immediately before
    // every call: this is the deterministic no-sharing schedule permitted by
    // concurrent users of the public global generation reset API.
    for dilation in std::iter::repeat_n(0.0, 13).chain(std::iter::once(2.0)) {
        for glyph in &layout.glyphs {
            comic_core::font::reset_glyph_cache();
            let output = font
                .rasterize_gid(glyph.glyph_id, glyph.size, dilation)
                .unwrap();
            let base = font.rasterize_gid(glyph.glyph_id, glyph.size, 0.0).unwrap();
            let output_len = output.coverage.capacity() * 4;
            let base_len = base.coverage.capacity() * 4;
            largest_base = largest_base.max(base_len);
            scratch = scratch.max(base_len + 16);
            if dilation > 0.0 {
                scratch = scratch.max(output_len);
            }
            output_bytes += output_len;
            held.push(output);
        }
    }
    let unique: HashSet<_> = held.iter().map(Arc::as_ptr).collect();
    assert_eq!(unique.len(), layout.glyphs.len() * 14);
    let headers = held.len()
        * (std::mem::size_of::<comic_core::GlyphBitmap>() + 2 * std::mem::size_of::<usize>());
    let measured = output_bytes + largest_base + scratch + headers;
    eprintln!("generation resets: bound={bound}, measured_coverage_scratch_headers={measured}");
    assert!(bound >= measured);
    assert!(measured < 4 * 1024 * 1024, "fixture stays small");
    assert!(bound < MAX_TEXT_PREPARATION_BYTES);
}

#[test]
fn d17_eleven_preparation_bound_fails_closed_for_invalid_dimensions() {
    let font = fonts().remove(0);
    let mut layout = layout_text(
        &TextBlock {
            text: "春".into(),
            ..TextBlock::default()
        },
        &font,
    );
    for size in [f32::NAN, f32::INFINITY, f32::MAX, 1_000_000.0] {
        layout.glyphs[0].size = size;
        assert_eq!(
            preparation_bound(&font, &layout, &TextStyle::default()),
            usize::MAX
        );
    }
    layout.glyphs[0].size = 48.0;
    assert_eq!(
        preparation_bound(
            &font,
            &layout,
            &TextStyle {
                outline: Some(stroke(f32::INFINITY)),
                ..TextStyle::default()
            }
        ),
        usize::MAX
    );
}

#[test]
fn d17_eleven_tiny_sideways_repetitions_include_bitmap_arc_metadata() {
    let font = fonts().remove(0);
    let glyph = TextBlock {
        text: "A".repeat(5462),
        size_px: 1.0,
        color: Rgba::WHITE,
        ..TextBlock::default()
    };
    let style = TextStyle {
        echo: Some(TextEchoStyle {
            count: 12,
            ..TextEchoStyle::default()
        }),
        ..TextStyle::default()
    };
    let decoration = TextBlock {
        color: Rgba::TRANSPARENT,
        echo: style.echo,
        ..glyph.clone()
    };
    let mut layout = layout_text(&glyph, &font);
    for glyph in &mut layout.glyphs {
        glyph.form = GlyphForm::Sideways;
        glyph.sideways = true;
        let (_, _, w, h) = font
            .glyph_px_bounds_gid(glyph.glyph_id, glyph.size)
            .unwrap();
        assert_eq!((w.ceil() as usize, h.ceil() as usize), (1, 1));
    }
    let bound = preparation_bound(&font, &layout, &style);
    let prepared = prepare_text_regions(&layout, &font, (0.0, 0.0), &[&glyph, &decoration]);
    let retained = prepared
        .iter()
        .map(|program| program.retained_bytes())
        .sum::<usize>();
    // Every Sideways occurrence/pass owns a fresh Arc allocation. retained_bytes
    // includes GlyphBitmap and coverage, but not Arc's two atomic refcounts.
    let arc_headers = layout.glyphs.len() * 13 * 2 * std::mem::size_of::<usize>();
    // Publishing measures retained_bytes while all coverage is live. Its
    // largest temporary pointer set contains every decoration bitmap. Count
    // only capacity * pointer size here: the actual table also owns spare
    // buckets/control bytes, so this is an independently observed lower bound.
    let pointers: HashSet<usize> = (0..layout.glyphs.len() * 12).collect();
    let accounting_scratch = pointers.capacity() * std::mem::size_of::<usize>();
    eprintln!(
        "tiny sideways: bound={bound}, retained={retained}, arc_headers={arc_headers}, accounting_scratch_min={accounting_scratch}"
    );
    assert!(bound >= retained + arc_headers + accounting_scratch);
    // Tens of thousands of calls may now exceed the budget: cache sharing is
    // an optimization, so its absence must remain covered by the preflight.
}
