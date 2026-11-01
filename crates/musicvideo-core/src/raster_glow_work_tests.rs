use super::*;
use crate::*;

fn glow_fixture(radius_px: f32) -> Glow {
    Glow {
        radius_px,
        spread_px: radius_px / 3.0,
        color: Rgba8::new(90, 220, 140, 190),
    }
}

fn backdrop(width: u32, height: u32) -> Frame {
    let mut frame = Frame::new(width, height).unwrap();
    for pixel in frame.rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[17, 23, 31, 255]);
    }
    frame
}

#[test]
fn d17_ten_remote_nonzero_cannot_change_visible_glow() {
    let mut near = MaskSurface {
        origin_x: -800,
        origin_y: 0,
        width: 1024,
        height: 64,
        alpha: vec![0; 1024 * 64],
    };
    near.alpha[32 * near.width + 784] = 255;
    let mut distant = MaskSurface {
        origin_x: near.origin_x,
        origin_y: near.origin_y,
        width: near.width,
        height: near.height,
        alpha: near.alpha.clone(),
    };
    distant.alpha[32 * distant.width + 1] = 255;
    let glow = Glow {
        radius_px: 16.0,
        spread_px: 0.0,
        color: Rgba8::WHITE,
    };
    let mut a = Frame::new(16, 64).unwrap();
    let mut b = a.clone();
    draw_glow(&mut a, &near, glow).unwrap();
    draw_glow(&mut b, &distant, glow).unwrap();
    assert!(a.rgba.chunks_exact(4).any(|pixel| pixel[0] > 0));
    assert_eq!(&a.rgba[32 * 16 * 4..32 * 16 * 4 + 4], &[10, 10, 10, 255]);
    assert_eq!(a.rgba, b.rgba, "global (-799,32) is outside filter support");
}

#[test]
fn d17_ten_one_pixel_translation_carries_glow_with_layer_anchor() {
    for angle in [0.0, 23.0, 90.0] {
        let source = MaskSurface {
            origin_x: 47,
            origin_y: 39,
            width: 31,
            height: 27,
            alpha: (0..31 * 27)
                .map(|index| {
                    let (x, y) = (index % 31, index / 31);
                    if (3..13).contains(&x) && (4..18).contains(&y) {
                        200
                    } else if (17..24).contains(&x) && (8..23).contains(&y) {
                        125
                    } else {
                        0
                    }
                })
                .collect(),
        };
        let shifted = MaskSurface {
            origin_x: source.origin_x + 1,
            origin_y: source.origin_y,
            width: source.width,
            height: source.height,
            alpha: source.alpha.clone(),
        };
        let rotated = rotate_mask(&source, angle).unwrap();
        let rotated_shifted = rotate_mask(&shifted, angle).unwrap();
        assert_eq!(rotated.alpha, rotated_shifted.alpha, "rotation {angle}");
        let mut a = Frame::new(160, 128).unwrap();
        let mut b = a.clone();
        let glow = glow_fixture(16.0);
        draw_glow_at_origin(&mut a, &rotated, glow, (source.origin_x, source.origin_y)).unwrap();
        draw_glow_at_origin(
            &mut b,
            &rotated_shifted,
            glow,
            (shifted.origin_x, shifted.origin_y),
        )
        .unwrap();
        assert!(a.rgba.chunks_exact(4).any(|pixel| pixel[1] > 0));
        for y in 0..a.height as usize {
            for x in 0..a.width as usize - 1 {
                let from = a.offset(x, y);
                let to = b.offset(x + 1, y);
                assert_eq!(
                    &a.rgba[from..from + 4],
                    &b.rgba[to..to + 4],
                    "rotation {angle}, source pixel ({x},{y})"
                );
            }
        }
    }
}

fn all_kind_glow_layers(project: &Project) -> Vec<Layer> {
    let mut layers = vec![
        new_image_layer(project, ImageId::new(DEFAULT_BACKGROUND_IMAGE_ID)),
        new_solid_layer(project),
        new_radial_spectrum_layer(project),
        new_linear_spectrum_layer(project),
        new_miv_spectrum_layer(project),
        new_pitch_keyboard_layer(project),
        new_progress_bar_layer(project),
        new_time_text_layer(project),
        new_text_layer(project),
        new_lyrics_layer(project),
    ];
    for layer in &mut layers {
        layer.opacity = Animatable::Const(0.65);
        layer.corner_radius = Animatable::Const(5.0);
        layer.glow = Animatable::Const(None);
        if let LayerKind::Lyrics(text) = &mut layer.kind {
            text.lines = vec![LyricLine {
                text: "Lyrics".into(),
                time_secs: Some(0.0),
            }];
        }
        let text = match &mut layer.kind {
            LayerKind::Text(text) => Some((&mut text.size_px, &mut text.style)),
            LayerKind::TimeText(text) => Some((&mut text.size_px, &mut text.style)),
            LayerKind::Lyrics(text) => Some((&mut text.size_px, &mut text.style)),
            _ => None,
        };
        if let Some((size, style)) = text {
            *size = 18.0;
            *style = Animatable::Const(crate::p5a_tests::decorated_style());
        }
    }
    layers
}

#[test]
fn d17_ten_forced_bounded_glow_matches_full_geometry_all_kinds() {
    let assets = crate::p5a_tests::assets();
    let mut project = default_project(10.0);
    project.output.width = 80;
    project.output.height = 48;
    let mut audio = crate::p5a_tests::frame();
    audio.miv_spectrum.bands = vec![
        music_core::MivSpectrumBandFrame {
            value: 0.6,
            trail: 0.8,
            onset: 0.7,
        };
        96
    ];
    for (kind, mut layer) in all_kind_glow_layers(&project).into_iter().enumerate() {
        for rect in [
            NormRect::new(-0.4, -0.25, 1.5, 1.4),
            NormRect::new(1.05, 0.1, 0.5, 0.8),
        ] {
            layer.rect = Animatable::Const(rect);
            project.layers = vec![layer.clone()];
            let scene = evaluate(&project, 2.5, &audio);
            let item = &scene.items[0];
            let geometry = LayerPixelRect::new(item.rect, scene.width, scene.height).unwrap();
            for angle in [0.0, 23.0, 90.0] {
                for radius in [1.0, 8.0, 24.0, 128.0] {
                    let glow = Some(glow_fixture(radius));
                    let mut full = backdrop(scene.width, scene.height);
                    let mut tiled = full.clone();
                    let mut a = RenderReport::default();
                    let mut b = a.clone();
                    draw_unclipped_item_full(
                        &mut full, geometry, item, &assets, &mut a, angle, glow,
                    )
                    .unwrap();
                    // Force the bounded renderer even though this tiny fixture
                    // could fit the ordinary full-allocation budget.
                    draw_tiled_item(
                        &mut tiled,
                        WorkPlan::new(geometry, scene.width, scene.height, angle),
                        item,
                        &assets,
                        &mut b,
                        glow,
                    )
                    .unwrap();
                    assert_eq!(a, b, "report kind {kind}");
                    assert_eq!(
                        full.rgba, tiled.rgba,
                        "kind {kind}, rect {rect:?}, angle {angle}, radius {radius}"
                    );
                }
            }
        }
    }
}

#[test]
fn d17_ten_scale_99_glow_bounds_frames_and_native_reduced_alpha() {
    // Establish that the shared pre-allocation probe observes both alpha
    // resolutions, rather than merely recording the RGBA work Frames.
    let mask = MaskSurface {
        origin_x: 0,
        origin_y: 0,
        width: 8,
        height: 8,
        alpha: vec![255; 64],
    };
    let glow = Glow {
        radius_px: 16.0,
        spread_px: 0.0,
        color: Rgba8::WHITE,
    };
    let mut tiny = Frame::new(16, 16).unwrap();
    let (result, dimensions) =
        frame_allocation_probe::with_limits(10_000, 100, || draw_glow(&mut tiny, &mask, glow));
    result.unwrap();
    // The existing 8x reduction uses three guard/filter blocks on each side.
    assert_eq!(dimensions, [(56, 56), (7, 7)]);
    let original = tiny.rgba.clone();
    let (denied, dimensions) =
        frame_allocation_probe::with_limits(48 * 48, 48, || draw_glow(&mut tiny, &mask, glow));
    assert_eq!(denied, Err(RasterError::DimensionsOverflow));
    assert_eq!(dimensions, [(56, 56)]);
    assert_eq!(tiny.rgba, original, "denial must precede rendering");

    let assets = crate::p5a_tests::assets();
    let mut project = default_project(10.0);
    project.output.width = 1920;
    project.output.height = 1080;
    let output_pixels = u64::from(project.output.width) * u64::from(project.output.height);
    let edge_limit = project.output.width.max(project.output.height) * 4;
    for (kind, mut layer) in all_kind_glow_layers(&project).into_iter().enumerate() {
        layer.rect = Animatable::Const(NormRect::FULL);
        layer.glow = Animatable::Const(Some(glow_fixture(128.0)));
        layer.modulations = vec![ModulationBinding {
            target: ModulationTarget::Scale,
            amount: 99.0,
            modulator: ModulatorId(1),
            curve: ModulationCurve::Linear,
        }];
        // Render ordinary glyph coverage while the layer geometry remains 100x.
        match &mut layer.kind {
            LayerKind::Text(text) => {
                text.size_px = 1.0;
                text.style = Animatable::Const(TextStyle::default());
            }
            LayerKind::TimeText(text) => {
                text.size_px = 1.0;
                text.style = Animatable::Const(TextStyle::default());
            }
            LayerKind::Lyrics(text) => {
                text.size_px = 1.0;
                text.style = Animatable::Const(TextStyle::default());
            }
            _ => {}
        }
        project.layers = vec![layer];
        for angle in [0.0, 23.0, 90.0] {
            project.layers[0].rotation = Animatable::Const(angle);
            let scene = evaluate(&project, 2.5, &crate::p5a_tests::frame());
            assert_eq!(
                scene.items[0].rect,
                NormRect::new(-49.5, -49.5, 100.0, 100.0)
            );
            assert_eq!(scene.items[0].glow.unwrap().radius_px, 128.0);
            assert_eq!(scene.items[0].glow.unwrap().spread_px, 128.0);
            let mut frame = Frame::new(1, 1).unwrap();
            let (result, dimensions) =
                frame_allocation_probe::with_limits(output_pixels * 3, edge_limit, || {
                    render_frame(&scene, &assets, &mut frame)
                });
            let report = result.unwrap_or_else(|error| {
                panic!("kind {kind}, angle {angle}: {error:?}; requested {dimensions:?}")
            });
            assert!(
                report.unresolved_assets.is_empty(),
                "kind {kind}: {report:?}"
            );
            assert_eq!(dimensions.first(), Some(&(scene.width, scene.height)));
            assert!(
                dimensions.len() > 3,
                "no body/Glow work observed for kind {kind}"
            );
            for &(width, height) in &dimensions {
                assert!(u64::from(width) * u64::from(height) <= output_pixels * 3);
                assert!(width.max(height) <= edge_limit);
            }
            if kind <= 1 {
                // The enormous image/solid still covers the viewport. Other
                // kinds can legitimately move all painted geometry offscreen.
                assert!(
                    frame
                        .rgba
                        .chunks_exact(4)
                        .any(|pixel| pixel[..3] != [0, 0, 0])
                );
            }
        }
    }
}

#[test]
fn d17_ten_scale_99_max_glyph_glow_checks_coverage_before_preparation() {
    use crate::render_limits::*;
    use crate::text::glyph_preparation_probe;

    let mut project = default_project(10.0);
    project.output.width = 1920;
    project.output.height = 1080;
    let output_pixels = u64::from(project.output.width) * u64::from(project.output.height);
    let edge_limit = project.output.width.max(project.output.height) * 4;
    let layers = [
        new_text_layer(&project),
        new_time_text_layer(&project),
        new_lyrics_layer(&project),
    ];
    for (kind, mut layer) in layers.into_iter().enumerate() {
        layer.rect = Animatable::Const(NormRect::FULL);
        layer.glow = Animatable::Const(Some(glow_fixture(128.0)));
        layer.modulations = vec![ModulationBinding {
            target: ModulationTarget::Scale,
            amount: 99.0,
            modulator: ModulatorId(1),
            curve: ModulationCurve::Linear,
        }];
        let (font_key, size, align, style) = match &mut layer.kind {
            LayerKind::Text(text) => {
                text.text = "H".into();
                (
                    &mut text.font_key,
                    &mut text.size_px,
                    &mut text.align,
                    &mut text.style,
                )
            }
            LayerKind::TimeText(text) => (
                &mut text.font_key,
                &mut text.size_px,
                &mut text.align,
                &mut text.style,
            ),
            LayerKind::Lyrics(text) => {
                text.lines = vec![LyricLine {
                    text: "H".into(),
                    time_secs: Some(0.0),
                }];
                (
                    &mut text.font_key,
                    &mut text.size_px,
                    &mut text.align,
                    &mut text.style,
                )
            }
            _ => unreachable!(),
        };
        *font_key = "font".into();
        *size = MAX_TEXT_SIZE_PX;
        *align = TextAlign::Center;
        *style = Animatable::Const(TextStyle {
            outline: Some(StrokeStyle {
                width_px: MAX_TEXT_OUTLINE_PX,
                ..Default::default()
            }),
            extra_outlines: vec![StrokeStyle {
                width_px: MAX_TEXT_OUTLINE_PX,
                ..Default::default()
            }],
            shadow: Some(TextShadowStyle {
                offset: (0.0, 0.0),
                spread_px: MAX_TEXT_SHADOW_SPREAD_PX,
                blur_px: 48.0,
                ..Default::default()
            }),
            ..Default::default()
        });
        project.layers = vec![layer];
        for angle in [0.0, 23.0, 90.0] {
            project.layers[0].rotation = Animatable::Const(angle);
            // A fresh font/cache owner makes glyph planning observable on every route.
            let mut fonts = comic_core::FontSet::default();
            fonts.insert(
                comic_core::LoadedFont::from_bytes(
                    "font",
                    include_bytes!(
                        "../../../tools/comic_lab/assets/fonts/ZenKurenaido-Regular.ttf"
                    )
                    .to_vec(),
                )
                .unwrap(),
            );
            let assets = Assets::new(fonts);
            let scene = evaluate(&project, 1.0, &crate::p5a_tests::frame());
            let mut frame = Frame::new(1, 1).unwrap();
            let ((result, completed), dimensions) =
                frame_allocation_probe::with_limits(output_pixels * 3, edge_limit, || {
                    glyph_preparation_probe::with_dry_run(|| {
                        render_frame(&scene, &assets, &mut frame)
                    })
                });
            let report = result.unwrap_or_else(|error| {
                panic!("kind {kind}, angle {angle}: {error:?}; requested {dimensions:?}")
            });
            // D17.11 gates the whole layer, including Glow, before coverage preparation.
            assert_eq!(completed, 0, "kind {kind}, angle {angle}");
            assert!(
                matches!(report.unresolved_assets.as_slice(),
                [UnresolvedAsset::TextPreparationBudget { required_bytes, budget_bytes, .. }]
                if *budget_bytes == MAX_TEXT_PREPARATION_BYTES && required_bytes > budget_bytes),
                "kind {kind}, angle {angle}: {report:?}"
            );
            assert_eq!(
                dimensions,
                vec![(1920, 1080)],
                "no glyph or Glow intermediates"
            );
            for &(width, height) in &dimensions {
                assert!(u64::from(width) * u64::from(height) <= output_pixels * 3);
                assert!(width.max(height) <= edge_limit);
            }
        }
    }
}
