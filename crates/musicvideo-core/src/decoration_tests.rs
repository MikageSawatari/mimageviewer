use super::*;
use crate::*;
use comic_core::{AnnotationObject, FontSet, LoadedFont, TextBlock, bake_overlay, layout_text};

fn fonts() -> FontSet {
    let mut fonts = FontSet::new();
    // Fixed HEAD hash fixture uses this font; fail visibly if unavailable.
    fonts.insert(
        LoadedFont::from_bytes(
            DEFAULT_FONT_KEY,
            std::fs::read(r"C:\Windows\Fonts\YuGothM.ttc")
                .expect("T1 fixture requires YuGothM.ttc"),
        )
        .unwrap(),
    );
    fonts
}
fn project() -> Project {
    let mut p = default_project(10.0);
    p.output = Output {
        width: 320,
        height: 180,
        fps: 30.0,
    };
    p.layers.clear();
    let mut layer = new_text_layer(&p);
    layer.rect = Animatable::Const(NormRect::new(0.05, 0.1, 0.9, 0.8));
    if let LayerKind::Text(t) = &mut layer.kind {
        t.text = "日本語 Text\n字間と行間".into();
        t.size_px = 28.0;
    }
    p.layers.push(layer);
    p
}
fn style() -> TextStyle {
    TextStyle {
        outline: Some(StrokeStyle {
            color: TextColor::new(255, 0, 0, 255),
            width_px: 3.0,
        }),
        extra_outlines: vec![StrokeStyle {
            color: TextColor::new(0, 0, 255, 255),
            width_px: 7.0,
        }],
        shadow: Some(TextShadowStyle {
            color: TextColor::new(30, 200, 80, 180),
            offset: (-18.0, 12.0),
            blur_px: 8.0,
            spread_px: 2.0,
        }),
        echo: Some(TextEchoStyle {
            color: TextColor::new(200, 50, 100, 160),
            offset: (4.0, -6.0),
            count: 3,
        }),
        background: Some(TextBackgroundStyle {
            fill: TextColor::new(20, 60, 100, 255),
            padding_px: 9.0,
            corner_px: 4.0,
        }),
        letter_gap: 2.0,
        line_gap: 3.0,
    }
}
fn text(p: &mut Project) -> &mut TextLayer {
    let LayerKind::Text(t) = &mut p.layers[0].kind else {
        panic!()
    };
    t
}
fn render(p: &Project, assets: &Assets, t: f64) -> Frame {
    let mut f = Frame::new(p.output.width, p.output.height).unwrap();
    assert!(
        render_frame(&evaluate(p, t, &SpectrumFrame::default()), assets, &mut f)
            .unwrap()
            .unresolved_assets
            .is_empty()
    );
    f
}
fn key(t: f64, value: TextStyle) -> Keyframe<TextStyle> {
    Keyframe {
        t,
        value,
        interp: Interp::Linear,
    }
}
#[test]
fn t1_undecorated_head_hashes_are_unchanged() {
    let assets = Assets::new(fonts());
    let mut p = project();
    for (mode, expected) in [
        0x6a2b0c1edf499979,
        0x56ed70d944cc42ad,
        0x30700679f8e3dc24,
        0x241cf683b2786458,
    ]
    .into_iter()
    .enumerate()
    {
        p.layers[0].opacity = Animatable::Const(if mode >= 1 { 0.5 } else { 1.0 });
        p.layers[0].rotation = Animatable::Const(if mode >= 2 { 27.0 } else { 0.0 });
        p.layers[0].glow = Animatable::Const(if mode >= 3 {
            Some(Glow {
                color: Rgba8::new(30, 255, 20, 160),
                radius_px: 8.0,
                spread_px: 2.0,
            })
        } else {
            None
        });
        let f = render(&p, &assets, 0.0);
        let hash = f.rgba.iter().fold(0xcbf29ce484222325u64, |a, &b| {
            (a ^ b as u64).wrapping_mul(0x100000001b3)
        });
        assert_eq!(hash, expected, "mode {mode}");
    }
}
#[test]
fn t1_transparent_fill_keeps_outline_colors_and_gradient_keeps_both() {
    let assets = Assets::new(fonts());
    let mut p = project();
    text(&mut p).style = Animatable::Const(TextStyle {
        outline: style().outline,
        ..Default::default()
    });
    text(&mut p).fill = Animatable::Const(Fill::Solid(Rgba8::new(255, 255, 255, 0)));
    let hollow = render(&p, &assets, 0.0);
    assert!(
        hollow
            .rgba
            .chunks_exact(4)
            .any(|px| px[0] > 200 && px[1] == 0 && px[2] == 0)
    );
    text(&mut p).fill = Animatable::Const(Fill::LinearGradient {
        stops: GradientStops::new(vec![
            GradientStop {
                position: 0.0,
                color: Rgba8::new(0, 255, 0, 255),
            },
            GradientStop {
                position: 1.0,
                color: Rgba8::new(0, 0, 255, 255),
            },
        ])
        .unwrap(),
        angle_deg: 0.0,
    });
    let gradient = render(&p, &assets, 0.0);
    assert!(
        gradient
            .rgba
            .chunks_exact(4)
            .any(|px| px[0] > 200 && px[1] < 10)
    );
    assert!(
        gradient
            .rgba
            .chunks_exact(4)
            .any(|px| px[0] < 10 && px[1] > 50 && px[2] > 50)
    );
    assert_ne!(hollow, gradient);
}
#[test]
fn t1_overlap_alpha_opacity_and_clip_apply_once() {
    assert_eq!(
        composite_text_pixel(&[0, 0, 255, 255], Rgba8::new(255, 0, 0, 255), 255),
        Rgba8::new(255, 0, 0, 255)
    );
    assert_eq!(
        composite_text_pixel(&[0, 0, 255, 128], Rgba8::new(255, 0, 0, 128), 255).a,
        192
    );
    assert_eq!(
        composite_text_pixel(&[0, 0, 255, 0], Rgba8::new(255, 0, 0, 0), 0).a,
        0
    );
    let assets = Assets::new(fonts());
    let mut p = project();
    text(&mut p).style = Animatable::Const(TextStyle {
        background: Some(TextBackgroundStyle {
            fill: TextColor::WHITE,
            ..Default::default()
        }),
        outline: Some(StrokeStyle {
            color: TextColor::WHITE,
            width_px: 4.0,
        }),
        ..Default::default()
    });
    p.layers[0].opacity = Animatable::Const(0.5);
    p.layers[0].corner_radius = Animatable::Const(12.0);
    let scene = evaluate(&p, 0.0, &SpectrumFrame::default());
    let rect = norm_rect_to_pixels(scene.items[0].rect, 320, 180);
    let mask =
        render_item_mask(rect, &scene.items[0], &assets, &mut RenderReport::default()).unwrap();
    assert_eq!(*mask.alpha.iter().max().unwrap(), 128);
    let f = render(&p, &assets, 0.0);
    assert_eq!(f.rgba.chunks_exact(4).map(|px| px[0]).max(), Some(128));
    for y in 0..mask.height {
        for x in 0..mask.width {
            assert_eq!(
                f.rgba[((y + rect.y0) * 320 + x + rect.x0) * 4],
                mask.alpha[y * mask.width + x]
            );
        }
    }
}
#[test]
fn t1_partial_rounded_clip_on_overlapping_glyph_and_decoration_is_applied_once() {
    let assets = Assets::new(fonts());
    let style = TextStyle {
        background: Some(TextBackgroundStyle {
            fill: TextColor::WHITE,
            padding_px: 8.0,
            corner_px: 0.0,
        }),
        ..Default::default()
    };
    let rect = PixelRect {
        x0: 0,
        y0: 0,
        x1: 64,
        y1: 64,
    };
    let mut frame = Frame::new(64, 64).unwrap();
    let opacity = 0.65;
    assert!(draw_text(
        &mut frame,
        rect,
        &assets,
        "MMMM",
        DEFAULT_FONT_KEY,
        64.0,
        TextAlign::Start,
        &style,
        false,
        &Fill::Solid(Rgba8::WHITE),
        opacity,
        32.0,
        false
    ));
    let mut mask = vec![0; 64 * 64];
    assert!(draw_text_mask(
        &mut mask,
        64,
        64,
        &assets,
        "MMMM",
        DEFAULT_FONT_KEY,
        64.0,
        TextAlign::Start,
        &style,
        false,
        &Fill::Solid(Rgba8::WHITE),
        opacity,
        32.0
    ));
    let bitmap = assets
        .rasterized_text("MMMM", DEFAULT_FONT_KEY, 64.0, &style, false)
        .unwrap();
    let dx = -(bitmap.origin_offset.0 as i32);
    let dy = ((64.0 - bitmap.logical_height) * 0.5).round() as i32 - bitmap.origin_offset.1 as i32;
    let mut checked = 0;
    for y in 0..64 {
        for x in 0..64 {
            let sx = x as i32 - dx;
            let sy = y as i32 - dy;
            if sx < 0 || sy < 0 || sx as usize >= bitmap.width || sy as usize >= bitmap.height {
                continue;
            }
            let source = (sy as usize * bitmap.width + sx as usize) * 4;
            if bitmap.rgba[source + 3] != 255
                || bitmap.decoration.as_ref().unwrap()[source + 3] == 0
            {
                continue;
            }
            // A 64px box with radius 32 is a circle: independent geometric oracle.
            let distance = (x as f32 + 0.5 - 32.0).hypot(y as f32 + 0.5 - 32.0);
            let clip = (32.5 - distance).clamp(0.0, 1.0);
            if clip <= 0.05 || clip >= 0.95 {
                continue;
            }
            let expected = (255.0 * (opacity * clip)).round() as u8;
            assert_eq!(
                frame.rgba[(y * 64 + x) * 4],
                expected,
                "body at {x},{y} clip={clip}"
            );
            assert_eq!(mask[y * 64 + x], expected, "mask at {x},{y} clip={clip}");
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "fixture must overlap glyph/decorations on a partial clip pixel"
    );
}
#[test]
fn t1_rotation_glow_uses_multicolor_combined_silhouette_and_is_deterministic() {
    let assets = Assets::new(fonts());
    let mut p = project();
    text(&mut p).style = Animatable::Const(style());
    text(&mut p).fill = Animatable::Const(Fill::Solid(Rgba8::new(255, 255, 255, 0)));
    p.layers[0].rotation = Animatable::Const(27.0);
    p.layers[0].glow = Animatable::Const(Some(Glow {
        color: Rgba8::new(0, 255, 0, 200),
        radius_px: 9.0,
        spread_px: 2.0,
    }));
    let f = render(&p, &assets, 0.0);
    assert_eq!(f, render(&p, &assets, 0.0));
    assert!(f.rgba.chunks_exact(4).any(|px| px[0] > 200 && px[1] < 40));
    assert!(
        f.rgba
            .chunks_exact(4)
            .any(|px| px[1] > 5 && px[0] == 0 && px[2] == 0)
    );
    p.layers[0].rotation = Animatable::Const(0.0);
    let unrotated = render(&p, &assets, 0.0);
    assert!(
        unrotated
            .rgba
            .chunks_exact(4)
            .any(|px| px[0] > 200 && px[1] < 40)
    ); // solid-body optimization must not erase effects
}
#[test]
fn t1_effect_reach_matches_large_canvas_and_alignment_preserves_glyph_origin() {
    let fonts = fonts();
    let assets = Assets::new(self::fonts());
    let mut s = style();
    s.shadow.as_mut().unwrap().offset = (-65.0, 45.0);
    s.outline.as_mut().unwrap().width_px = 20.0;
    let bitmap = assets
        .rasterized_text("あ A", DEFAULT_FONT_KEY, 32.0, &s, false)
        .unwrap();
    let plain = assets
        .rasterized_text(
            "あ A",
            DEFAULT_FONT_KEY,
            32.0,
            &TextStyle {
                letter_gap: s.letter_gap,
                line_gap: s.line_gap,
                ..Default::default()
            },
            false,
        )
        .unwrap();
    assert_eq!(bitmap.logical_width, plain.logical_width);
    assert_eq!(bitmap.logical_height, plain.logical_height);
    let block = TextBlock {
        text: "あ A".into(),
        font_key: DEFAULT_FONT_KEY.into(),
        size_px: 32.0,
        color: TextColor::TRANSPARENT,
        outline: s.outline,
        extra_outlines: s.extra_outlines.clone(),
        shadow: s.shadow,
        echo: s.echo,
        background: s.background,
        letter_gap: s.letter_gap,
        line_gap: s.line_gap,
        ..Default::default()
    };
    let margin = 100;
    let big = bake_overlay(
        &[AnnotationObject::new_text(
            1,
            (
                (bitmap.origin_offset.0 + margin) as f32,
                (bitmap.origin_offset.1 + margin) as f32,
            ),
            block,
        )],
        bitmap.width + margin * 2,
        bitmap.height + margin * 2,
        &fonts,
    );
    let decoration = bitmap.decoration.as_ref().unwrap();
    for y in 0..big.h {
        for x in 0..big.w {
            let expected = if x >= margin
                && x < margin + bitmap.width
                && y >= margin
                && y < margin + bitmap.height
            {
                decoration[((y - margin) * bitmap.width + x - margin) * 4 + 3]
            } else {
                0
            };
            assert_eq!(big.pixels[(y * big.w + x) * 4 + 3], expected, "{x},{y}");
        }
    }
    let mut p = project();
    text(&mut p).text = "あ A".into();
    text(&mut p).size_px = 32.0;
    // Color-disjoint effects leave opaque white glyph positions unchanged.
    s.background = None;
    for align in [TextAlign::Start, TextAlign::Center, TextAlign::End] {
        text(&mut p).align = align;
        text(&mut p).style = Animatable::Const(TextStyle {
            letter_gap: s.letter_gap,
            line_gap: s.line_gap,
            ..Default::default()
        });
        let plain = render(&p, &assets, 0.0);
        text(&mut p).style = Animatable::Const(s.clone());
        let decorated = render(&p, &assets, 0.0);
        let mut opaque = 0;
        for (a, b) in plain
            .rgba
            .chunks_exact(4)
            .zip(decorated.rgba.chunks_exact(4))
        {
            if a[..3] == [255, 255, 255] {
                opaque += 1;
                assert_eq!(a, b);
            }
        }
        assert!(opaque > 30);
    }
}
#[test]
fn t1_spacing_changes_both_bitmaps_using_comic_layout() {
    let fonts = fonts();
    let assets = Assets::new(self::fonts());
    let s = style();
    let bitmap = assets
        .rasterized_text("AB\nCD", DEFAULT_FONT_KEY, 30.0, &s, false)
        .unwrap();
    let layout = layout_text(
        &TextBlock {
            text: "AB\nCD".into(),
            size_px: 30.0,
            letter_gap: s.letter_gap,
            line_gap: s.line_gap,
            ..Default::default()
        },
        fonts.get(DEFAULT_FONT_KEY).unwrap(),
    );
    assert_eq!((bitmap.logical_width, bitmap.logical_height), layout.bounds);
    let no_spacing = assets
        .rasterized_text(
            "AB\nCD",
            DEFAULT_FONT_KEY,
            30.0,
            &TextStyle {
                letter_gap: 0.0,
                line_gap: 0.0,
                ..s
            },
            false,
        )
        .unwrap();
    assert!(bitmap.logical_width > no_spacing.logical_width);
    assert!(bitmap.logical_height > no_spacing.logical_height);
}
#[test]
fn t1_matching_structure_interpolates_every_continuous_field() {
    let a = style();
    let mut b = a.clone();
    b.letter_gap = 12.0;
    b.line_gap = 13.0;
    b.outline.as_mut().unwrap().width_px = 13.0;
    b.outline.as_mut().unwrap().color = TextColor::new(0, 0, 255, 255);
    b.extra_outlines[0].width_px = 17.0;
    b.shadow.as_mut().unwrap().offset = (2.0, 22.0);
    b.shadow.as_mut().unwrap().blur_px = 18.0;
    b.shadow.as_mut().unwrap().spread_px = 12.0;
    b.shadow.as_mut().unwrap().color = TextColor::WHITE;
    b.echo.as_mut().unwrap().offset = (14.0, 4.0);
    b.echo.as_mut().unwrap().color = TextColor::WHITE;
    b.background.as_mut().unwrap().padding_px = 19.0;
    b.background.as_mut().unwrap().corner_px = 14.0;
    b.background.as_mut().unwrap().fill = TextColor::WHITE;
    let m = TextStyle::interpolate(&a, &b, 0.5);
    assert_eq!(m.letter_gap, 7.0);
    assert_eq!(m.line_gap, 8.0);
    assert_eq!(m.outline.unwrap().width_px, 8.0);
    assert_eq!(m.outline.unwrap().color, TextColor::new(128, 0, 128, 255));
    assert_eq!(m.extra_outlines[0].width_px, 12.0);
    assert_eq!(m.shadow.unwrap().offset, (-8.0, 17.0));
    assert_eq!(m.shadow.unwrap().blur_px, 13.0);
    assert_eq!(m.shadow.unwrap().spread_px, 7.0);
    assert_eq!(m.shadow.unwrap().color, TextColor::new(143, 228, 168, 218));
    assert_eq!(m.echo.unwrap().offset, (9.0, -1.0));
    assert_eq!(m.echo.unwrap().color, TextColor::new(228, 153, 178, 208));
    assert_eq!(m.background.unwrap().padding_px, 14.0);
    assert_eq!(m.background.unwrap().corner_px, 9.0);
    assert_eq!(
        m.background.unwrap().fill,
        TextColor::new(138, 158, 178, 255)
    );
}
#[test]
fn t1_any_structure_mismatch_steps_entire_style() {
    let a = style();
    for change in 0..6 {
        let mut b = a.clone();
        b.letter_gap = 100.0;
        b.line_gap = 100.0;
        match change {
            0 => b.outline = None,
            1 => b.extra_outlines.clear(),
            2 => b.shadow = None,
            3 => b.echo = None,
            4 => b.echo.as_mut().unwrap().count += 1,
            _ => b.background = None,
        }
        assert_eq!(TextStyle::interpolate(&a, &b, 0.9), a, "change {change}");
        let track =
            Animatable::Track(Track::new(vec![key(0.0, a.clone()), key(1.0, b.clone())]).unwrap());
        assert_eq!(track.evaluate(0.5), a);
        assert_eq!(track.evaluate(1.0), b);
    }
}
#[test]
fn t1_style_track_single_duplicate_and_endpoints() {
    let a = style();
    let b = TextStyle {
        letter_gap: 99.0,
        ..a.clone()
    };
    let one = Animatable::Track(Track::new(vec![key(2.0, a.clone())]).unwrap());
    assert_eq!(one.evaluate(-10.0), a);
    assert_eq!(one.evaluate(30.0), a);
    let duplicate =
        Animatable::Track(Track::new(vec![key(2.0, a.clone()), key(2.0, b.clone())]).unwrap());
    assert_eq!(duplicate.evaluate(1.0), a);
    assert_eq!(duplicate.evaluate(2.0), b);
}
#[test]
fn t1_all_text_kinds_share_style_and_lyrics_fade_modulates_combined_alpha() {
    let mut p = project();
    let s = style();
    text(&mut p).style = Animatable::Const(s.clone());
    let mut time = new_time_text_layer(&p);
    if let LayerKind::TimeText(t) = &mut time.kind {
        t.style = Animatable::Const(s.clone());
    }
    p.layers.push(time);
    let mut lyrics = new_lyrics_layer(&p);
    if let LayerKind::Lyrics(t) = &mut lyrics.kind {
        t.style = Animatable::Const(s.clone());
        t.lines = vec![LyricLine {
            text: "歌詞".into(),
            time_secs: Some(0.0),
        }];
        t.fade_in_secs = 1.0;
    }
    p.layers.push(lyrics);
    let scene = evaluate(&p, 0.5, &SpectrumFrame::default());
    assert_eq!(scene.items.len(), 3);
    for item in &scene.items {
        match &item.kind {
            SceneItemKind::Text {
                style,
                transient_style,
                ..
            }
            | SceneItemKind::TimeText {
                style,
                transient_style,
                ..
            } => {
                assert_eq!(style, &s);
                assert!(!transient_style);
            }
            _ => panic!(),
        }
    }
    assert_eq!(scene.items[2].opacity, 0.5);
    let assets = Assets::new(fonts());
    let item = &scene.items[2];
    let rect = norm_rect_to_pixels(item.rect, 320, 180);
    let mask = render_item_mask(rect, item, &assets, &mut RenderReport::default()).unwrap();
    assert_eq!(*mask.alpha.iter().max().unwrap(), 128);
}
#[test]
fn t1_old_json_and_decorated_tracks_round_trip() {
    let mut p = project();
    let mut time = new_time_text_layer(&p);
    let mut lyrics = new_lyrics_layer(&p);
    if let LayerKind::TimeText(t) = &mut time.kind {
        t.style = Animatable::Const(style());
    }
    if let LayerKind::Lyrics(t) = &mut lyrics.kind {
        t.style = Animatable::Const(style());
    }
    text(&mut p).style = Animatable::Track(
        Track::new(vec![
            key(0.0, style()),
            key(
                2.0,
                TextStyle {
                    letter_gap: 10.0,
                    ..style()
                },
            ),
        ])
        .unwrap(),
    );
    p.layers.extend([time, lyrics]);
    let bytes = serde_json::to_vec(&p).unwrap();
    assert_eq!(serde_json::from_slice::<Project>(&bytes).unwrap(), p);
    let mut value = serde_json::to_value(&p).unwrap();
    for layer in value["layers"].as_array_mut().unwrap() {
        let kind = layer["kind"].as_object_mut().unwrap();
        kind.values_mut()
            .next()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("style");
    }
    let old: Project = serde_json::from_value(value).unwrap();
    for item in evaluate(&old, 0.0, &SpectrumFrame::default()).items {
        match item.kind {
            SceneItemKind::Text { style, .. } | SceneItemKind::TimeText { style, .. } => {
                assert_eq!(style, TextStyle::default())
            }
            _ => panic!(),
        }
    }
}
#[test]
fn t1_intermediate_style_uses_transient_lane_but_step_and_endpoints_are_stable() {
    let mut p = project();
    let a = style();
    let b = TextStyle {
        letter_gap: 30.0,
        ..a.clone()
    };
    text(&mut p).style =
        Animatable::Track(Track::new(vec![key(0.0, a.clone()), key(1.0, b)]).unwrap());
    let transient = |p: &Project, t| match &evaluate(p, t, &SpectrumFrame::default()).items[0].kind
    {
        SceneItemKind::Text {
            transient_style, ..
        } => *transient_style,
        _ => panic!(),
    };
    assert!(!transient(&p, 0.0));
    assert!(transient(&p, 0.5));
    assert!(!transient(&p, 1.0));
    text(&mut p).style =
        Animatable::Track(Track::new(vec![key(0.0, a), key(1.0, TextStyle::default())]).unwrap());
    assert!(!transient(&p, 0.5));
}

#[test]
fn t1_negative_spacing_keeps_ink_outside_logical_bounds_in_both_surfaces() {
    let fonts = fonts();
    let assets = Assets::new(self::fonts());
    for (letter_gap, line_gap) in [(-64.0, 0.0), (0.0, -64.0)] {
        let s = TextStyle {
            letter_gap,
            line_gap,
            ..style()
        };
        let bitmap = assets
            .rasterized_text("ABC\nDE", DEFAULT_FONT_KEY, 32.0, &s, false)
            .unwrap();
        if letter_gap < 0.0 {
            assert!(
                bitmap.height < 200,
                "horizontal reach must not inflate height"
            );
        }
        let glyph = TextBlock {
            text: "ABC\nDE".into(),
            font_key: DEFAULT_FONT_KEY.into(),
            size_px: 32.0,
            color: TextColor::WHITE,
            letter_gap,
            line_gap,
            ..Default::default()
        };
        let decoration = TextBlock {
            color: TextColor::TRANSPARENT,
            outline: s.outline,
            extra_outlines: s.extra_outlines,
            shadow: s.shadow,
            echo: s.echo,
            background: s.background,
            ..glyph.clone()
        };
        for (block, small) in [
            (glyph, &bitmap.rgba),
            (decoration, bitmap.decoration.as_ref().unwrap()),
        ] {
            let margin = 200;
            let big = bake_overlay(
                &[AnnotationObject::new_text(
                    1,
                    (
                        (bitmap.origin_offset.0 + margin) as f32,
                        (bitmap.origin_offset.1 + margin) as f32,
                    ),
                    block,
                )],
                bitmap.width + margin * 2,
                bitmap.height + margin * 2,
                &fonts,
            );
            for y in 0..big.h {
                for x in 0..big.w {
                    let expected = if x >= margin
                        && x < margin + bitmap.width
                        && y >= margin
                        && y < margin + bitmap.height
                    {
                        small[((y - margin) * bitmap.width + x - margin) * 4 + 3]
                    } else {
                        0
                    };
                    assert_eq!(
                        big.pixels[(y * big.w + x) * 4 + 3],
                        expected,
                        "spacing {letter_gap},{line_gap}: {x},{y}"
                    );
                }
            }
        }
    }
}

#[test]
fn t1_painted_glyph_overhang_is_not_clipped_with_zero_spacing() {
    let fonts = fonts();
    let assets = Assets::new(self::fonts());
    let font = fonts.get(DEFAULT_FONT_KEY).unwrap();
    let size = 128.0;
    let text = [
        'f', 'j', 'J', '(', ')', 'T', 'r', 'y', 'Á', 'ç', 'é', 'Å', 'ß', '*',
    ]
    .into_iter()
    .find_map(|ch| {
        let text = ch.to_string();
        let block = TextBlock {
            text: text.clone(),
            size_px: size,
            ..Default::default()
        };
        let layout = layout_text(&block, font);
        let g = &layout.glyphs[0];
        let (x, y, w, h) = font.glyph_px_bounds_gid(g.glyph_id, size)?;
        ((-(g.x + x))
            .max(-(g.y + y))
            .max(g.x + x + w - layout.bounds.0)
            .max(g.y + y + h - layout.bounds.1)
            > 1.0)
            .then_some(text)
    })
    .expect("fixture has an overhanging glyph");
    let s = TextStyle {
        outline: Some(StrokeStyle {
            width_px: 8.0,
            color: TextColor::WHITE,
        }),
        ..Default::default()
    };
    let bitmap = assets
        .rasterized_text(&text, DEFAULT_FONT_KEY, size, &s, false)
        .unwrap();
    let margin = 100;
    let block = TextBlock {
        text: text.clone(),
        font_key: DEFAULT_FONT_KEY.into(),
        size_px: size,
        color: TextColor::TRANSPARENT,
        outline: s.outline,
        ..Default::default()
    };
    let big = bake_overlay(
        &[AnnotationObject::new_text(
            1,
            (
                (bitmap.origin_offset.0 + margin) as f32,
                (bitmap.origin_offset.1 + margin) as f32,
            ),
            block,
        )],
        bitmap.width + margin * 2,
        bitmap.height + margin * 2,
        &fonts,
    );
    let small = bitmap.decoration.as_ref().unwrap();
    for y in 0..big.h {
        for x in 0..big.w {
            let expected = if x >= margin
                && x < margin + bitmap.width
                && y >= margin
                && y < margin + bitmap.height
            {
                small[((y - margin) * bitmap.width + x - margin) * 4 + 3]
            } else {
                0
            };
            assert_eq!(
                big.pixels[(y * big.w + x) * 4 + 3],
                expected,
                "glyph {text}: {x},{y}"
            );
        }
    }
}
