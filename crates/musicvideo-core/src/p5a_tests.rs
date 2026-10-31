use crate::*;
use comic_core::{FontSet, LoadedFont};

pub(crate) fn frame() -> SpectrumFrame {
    let mut frame = SpectrumFrame::new(vec![0.6; 96]);
    frame.modulators.push(ModulatorValue {
        id: ModulatorId(1),
        value: 1.0,
    });
    frame
}

fn binding(target: ModulationTarget, amount: f32) -> ModulationBinding {
    ModulationBinding {
        target,
        amount,
        modulator: ModulatorId(1),
        curve: ModulationCurve::Linear,
    }
}

pub(crate) fn decorated_style() -> TextStyle {
    TextStyle {
        outline: Some(StrokeStyle {
            width_px: 2.0,
            color: TextColor::WHITE,
        }),
        extra_outlines: vec![StrokeStyle {
            width_px: 3.0,
            color: TextColor::WHITE,
        }],
        shadow: Some(TextShadowStyle {
            offset: (-4.0, 5.0),
            blur_px: 6.0,
            spread_px: 7.0,
            color: TextColor::WHITE,
        }),
        echo: Some(TextEchoStyle {
            offset: (8.0, -9.0),
            count: 3,
            color: TextColor::WHITE,
        }),
        background: Some(TextBackgroundStyle {
            padding_px: 10.0,
            corner_px: 11.0,
            fill: TextColor::WHITE,
        }),
        letter_gap: 12.0,
        line_gap: 13.0,
    }
}

fn text_properties(layer: &mut Layer) -> (&mut f32, &mut Animatable<TextStyle>) {
    match &mut layer.kind {
        LayerKind::Text(text) => (&mut text.size_px, &mut text.style),
        LayerKind::TimeText(text) => (&mut text.size_px, &mut text.style),
        LayerKind::Lyrics(text) => (&mut text.size_px, &mut text.style),
        _ => panic!("text fixture"),
    }
}

#[test]
fn p5a_scale_text_time_and_lyrics_includes_every_decoration_dimension() {
    let mut project = default_project(10.0);
    project.layers = vec![
        new_text_layer(&project),
        new_time_text_layer(&project),
        new_lyrics_layer(&project),
    ];
    for layer in &mut project.layers {
        if let LayerKind::Lyrics(text) = &mut layer.kind {
            text.lines = vec![LyricLine {
                text: "Lyrics".into(),
                time_secs: Some(0.0),
            }];
        }
        let (size, style) = text_properties(layer);
        *size = 20.0;
        *style = Animatable::Const(decorated_style());
        layer.glow = Animatable::Const(Some(Glow {
            color: Rgba8::WHITE,
            radius_px: 4.0,
            spread_px: 5.0,
        }));
        layer.corner_radius = Animatable::Const(6.0);
        layer.modulations = vec![binding(ModulationTarget::Scale, 1.0)];
    }
    let scene = evaluate(&project, 1.0, &frame());
    assert_eq!(scene.items.len(), 3);
    for item in &scene.items {
        let (SceneItemKind::Text { size_px, style, .. }
        | SceneItemKind::TimeText { size_px, style, .. }) = &item.kind
        else {
            panic!("text item")
        };
        assert_eq!(*size_px, 40.0);
        assert_eq!(style.outline.unwrap().width_px, 4.0);
        assert_eq!(style.extra_outlines[0].width_px, 6.0);
        let shadow = style.shadow.unwrap();
        assert_eq!(
            (shadow.offset, shadow.blur_px, shadow.spread_px),
            ((-8.0, 10.0), 12.0, 14.0)
        );
        assert_eq!(style.echo.unwrap().offset, (16.0, -18.0));
        assert_eq!(style.echo.unwrap().count, 3);
        assert_eq!(style.background.unwrap().padding_px, 20.0);
        assert_eq!(style.background.unwrap().corner_px, 22.0);
        assert_eq!((style.letter_gap, style.line_gap), (24.0, 26.0));
        assert_eq!(item.glow.unwrap().radius_px, 8.0);
        assert_eq!(item.glow.unwrap().spread_px, 10.0);
        assert_eq!(item.corner_radius_px, 12.0);
        assert_eq!(style.outline.unwrap().color, TextColor::WHITE);
    }
    // Width/height editing alone is only a box edit, even with decorated text.
    for layer in &mut project.layers {
        layer.modulations.clear();
        layer.rect = Animatable::Const(NormRect::FULL);
    }
    for item in evaluate(&project, 1.0, &frame()).items {
        let (SceneItemKind::Text { size_px, style, .. }
        | SceneItemKind::TimeText { size_px, style, .. }) = item.kind
        else {
            panic!("text item")
        };
        assert_eq!(size_px, 20.0);
        assert_eq!(style, decorated_style());
    }
}

#[test]
fn p5a_scale_geometry_includes_keyboard_miv_and_preserves_ratios() {
    let mut project = default_project(10.0);
    project.layers = vec![
        new_linear_spectrum_layer(&project),
        new_radial_spectrum_layer(&project),
        new_miv_spectrum_layer(&project),
        new_pitch_keyboard_layer(&project),
        new_progress_bar_layer(&project),
    ];
    if let LayerKind::Spectrum(s) = &mut project.layers[0].kind {
        s.shape = SpectrumShape::Bars {
            gap_ratio: 0.2,
            min_height_px: 3.0,
            thickness_px: 4.0,
        };
    }
    for layer in &mut project.layers {
        layer.modulations = vec![binding(ModulationTarget::Scale, 1.0)];
    }
    let scene = evaluate(&project, 2.5, &frame());
    let SceneItemKind::Spectrum {
        shape:
            SpectrumShape::Bars {
                gap_ratio,
                min_height_px,
                thickness_px,
            },
        minimum_thickness_px,
        ..
    } = scene.items[0].kind
    else {
        panic!("bars")
    };
    assert_eq!(
        (gap_ratio, min_height_px, thickness_px, minimum_thickness_px),
        (0.2, 6.0, 8.0, 2.0)
    );
    let SceneItemKind::Spectrum {
        shape:
            SpectrumShape::Radial {
                inner_ratio,
                thickness_px,
                start_angle_deg,
                sweep_deg,
                ..
            },
        ..
    } = scene.items[1].kind
    else {
        panic!("radial")
    };
    assert_eq!(
        (inner_ratio, thickness_px, start_angle_deg, sweep_deg),
        (0.75, 8.0, -90.0, 360.0)
    );
    let SceneItemKind::Spectrum {
        miv_dimensions: d, ..
    } = scene.items[2].kind
    else {
        panic!("miv")
    };
    assert_eq!(
        (
            d.band_inset_px,
            d.min_width_px,
            d.corner_threshold_px,
            d.corner_radius_px
        ),
        (0.5, 1.5, 4.0, 2.0)
    );
    assert_eq!(
        (
            d.bottom_inset_px,
            d.height_inset_px,
            d.ghost_expand_px,
            d.trail_expand_px
        ),
        (4.0, 6.0, 0.9, 0.4)
    );
    let SceneItemKind::PitchKeyboard { dimensions: d, .. } = scene.items[3].kind else {
        panic!("keyboard")
    };
    assert_eq!(
        (
            d.key_inset_px,
            d.min_octave_width_px,
            d.black_corner_px,
            d.black_stroke_px,
            d.white_stroke_px,
            d.guide_half_width_px
        ),
        (0.5, 2.0, 2.0, 1.5, 1.6, 0.8)
    );
    let SceneItemKind::ProgressBar {
        progress,
        border_thickness_px,
        ..
    } = scene.items[4].kind
    else {
        panic!("progress")
    };
    assert_eq!(progress, 0.25);
    assert_eq!(border_thickness_px, 4.0);
    // Shrinking also scales the old one-pixel spectrum floor, not just expansion.
    project.layers[0].modulations[0].amount = -0.5;
    let scene = evaluate(&project, 2.5, &frame());
    let SceneItemKind::Spectrum {
        minimum_thickness_px,
        ..
    } = scene.items[0].kind
    else {
        panic!("bars")
    };
    assert_eq!(minimum_thickness_px, 0.5);
}

#[test]
fn p5a_inner_ratio_clamps_after_modulation_and_defaults_preserve_ring() {
    let mut project = default_project(10.0);
    let LayerKind::Spectrum(s) = &project.layers[1].kind else {
        panic!("radial")
    };
    let SpectrumShape::Radial { inner_ratio, .. } = s.shape else {
        panic!("radial")
    };
    assert_eq!(inner_ratio * 550.0, 380.0);
    for (base, amount, expected) in [
        (-0.5, 0.0, 0.0),
        (1.5, 0.0, 1.0),
        (0.5, 0.8, 1.0),
        (0.5, -0.8, 0.0),
        (0.5, 0.03, 0.53),
    ] {
        let layer = &mut project.layers[1];
        if let LayerKind::Spectrum(s) = &mut layer.kind {
            if let SpectrumShape::Radial { inner_ratio, .. } = &mut s.shape {
                *inner_ratio = base;
            }
        }
        layer.modulations = vec![binding(ModulationTarget::SpectrumRadius, amount)];
        let scene = evaluate(&project, 1.0, &frame());
        let SceneItemKind::Spectrum {
            shape: SpectrumShape::Radial { inner_ratio, .. },
            ..
        } = scene.items[1].kind
        else {
            panic!("radial")
        };
        assert_eq!(inner_ratio, expected);
    }
}

#[test]
fn p5a_radial_scale_changes_inner_diameter_in_export_rgba() {
    let mut project = default_project(10.0);
    project.output.width = 240;
    project.output.height = 240;
    project.layers = vec![new_radial_spectrum_layer(&project)];
    let layer = &mut project.layers[0];
    layer.rect = Animatable::Const(NormRect::new(0.25, 0.25, 0.5, 0.5));
    layer.glow = Animatable::Const(None);
    layer.modulations = vec![binding(ModulationTarget::Scale, 1.0)];
    if let LayerKind::Spectrum(s) = &mut layer.kind {
        s.point_count = 1;
        s.fill = Animatable::Const(Fill::Solid(Rgba8::WHITE));
        s.shape = SpectrumShape::Radial {
            inner_ratio: 0.5,
            thickness_px: 4.0,
            start_angle_deg: 0.0,
            sweep_deg: 360.0,
            direction: SpectrumDirection::Clockwise,
        };
    }
    let mut audio = frame();
    audio.bands.fill(1.0);
    for (value, first, last) in [(0.0, 148, 179), (1.0, 176, 239)] {
        audio.modulators[0].value = value;
        let mut output = Frame::new(1, 1).unwrap();
        let _ = render_frame(
            &evaluate(&project, 1.0, &audio),
            &Assets::default(),
            &mut output,
        )
        .unwrap();
        let lit: Vec<usize> = (120..240)
            .filter(|&x| output.rgba[(120 * 240 + x) * 4] > 127)
            .collect();
        assert_eq!(lit.first().copied(), Some(first));
        assert_eq!(lit.last().copied(), Some(last));
    }
}

pub(crate) fn assets() -> Assets {
    let mut fonts = FontSet::new();
    fonts.insert(
        LoadedFont::from_bytes(
            DEFAULT_FONT_KEY,
            std::fs::read(r"C:\Windows\Fonts\YuGothM.ttc")
                .expect("P5a hash fixture needs YuGothM.ttc"),
        )
        .unwrap(),
    );
    let assets = Assets::new(fonts);
    assets.insert_image(
        ImageId::new(DEFAULT_BACKGROUND_IMAGE_ID),
        ImageAsset::new(2, 2, vec![25, 75, 150, 255].repeat(4)).unwrap(),
    );
    assets
}

// Pin all kinds against the original renderer as well as a neutral Scale binding.
#[test]
fn p5a_scale_one_keeps_all_layer_rgba_hashes() {
    let assets = assets();
    let mut project = default_project(10.0);
    project.output.width = 320;
    project.output.height = 180;
    let layers = vec![
        new_image_layer(&project, ImageId::new(DEFAULT_BACKGROUND_IMAGE_ID)),
        new_solid_layer(&project),
        new_radial_spectrum_layer(&project),
        new_linear_spectrum_layer(&project),
        new_miv_spectrum_layer(&project),
        new_pitch_keyboard_layer(&project),
        new_progress_bar_layer(&project),
        new_time_text_layer(&project),
        new_text_layer(&project),
        new_lyrics_layer(&project),
    ];
    let mut observed = Vec::new();
    let mut audio = frame();
    audio.miv_spectrum.bands = vec![
        music_core::MivSpectrumBandFrame {
            value: 0.6,
            trail: 0.8,
            onset: 0.7
        };
        96
    ];
    for mut layer in layers {
        layer.rect = Animatable::Const(NormRect::new(0.2, 0.2, 0.6, 0.6));
        if let LayerKind::Spectrum(s) = &mut layer.kind {
            if let SpectrumShape::Radial { inner_ratio, .. } = &mut s.shape {
                *inner_ratio = 18.0 / 54.0;
            }
        }
        if let LayerKind::Lyrics(text) = &mut layer.kind {
            text.lines = vec![LyricLine {
                text: "Lyrics".into(),
                time_secs: Some(0.0),
            }];
        }
        if matches!(
            layer.kind,
            LayerKind::Text(_) | LayerKind::TimeText(_) | LayerKind::Lyrics(_)
        ) {
            let (size, style) = text_properties(&mut layer);
            *size = 18.0;
            *style = Animatable::Const(TextStyle {
                outline: decorated_style().outline,
                ..TextStyle::default()
            });
        }
        project.layers = vec![layer];
        let mut plain = Frame::new(1, 1).unwrap();
        let _ = render_frame(&evaluate(&project, 2.5, &audio), &assets, &mut plain).unwrap();
        project.layers[0].modulations = vec![binding(ModulationTarget::Scale, 0.0)];
        let mut neutral = Frame::new(1, 1).unwrap();
        let _ = render_frame(&evaluate(&project, 2.5, &audio), &assets, &mut neutral).unwrap();
        assert_eq!(plain, neutral);
        assert!(plain.rgba.chunks_exact(4).any(|p| p[..3] != [0, 0, 0]));
        observed.push(plain.rgba.iter().fold(0xcbf29ce484222325u64, |hash, &b| {
            (hash ^ u64::from(b)).wrapping_mul(0x100000001b3)
        }));
    }
    // Captured with the pre-P5a HEAD renderer (same pixel radius and font).
    assert_eq!(
        observed,
        vec![
            11929547718503071525,
            15146039803181395157,
            6361361498803147436,
            4182883237258884901,
            6267659157819206974,
            10560538526189459923,
            8809012161789232701,
            8334601442569208685,
            16398903515838879327,
            18382634090672618910
        ]
    );
}

#[test]
fn p5a_keyboard_cache_owns_evaluated_pixel_dimensions() {
    use music_core::{
        KeyboardFrame, KeyboardLayoutCache, KeyboardRect, layout_keyboard,
        layout_keyboard_cached_with_dimensions,
    };
    let frame = KeyboardFrame::default();
    let rect = KeyboardRect::new(0.0, 0.0, 1000.0, 100.0);
    let mut cache = KeyboardLayoutCache::default();
    let original = layout_keyboard(&frame, rect);
    assert_eq!(
        *layout_keyboard_cached_with_dimensions(&frame, rect, 0.25, 1.0, &mut cache),
        original
    );
    let changed =
        layout_keyboard_cached_with_dimensions(&frame, rect, 0.5, 1.0, &mut cache).clone();
    assert_eq!(cache.rebuilds(), 2);
    assert_eq!(changed.keys.len(), original.keys.len());
    assert_eq!(
        changed.keys[10].rect.left,
        original.keys[10].rect.left + 0.25
    );
    assert_eq!(
        changed.keys[10].rect.right,
        original.keys[10].rect.right - 0.25
    );
    let hidden = layout_keyboard_cached_with_dimensions(&frame, rect, 0.5, 2000.0, &mut cache);
    assert!(hidden.keys.is_empty());
    assert_eq!(
        *layout_keyboard_cached_with_dimensions(&frame, rect, 0.25, 1.0, &mut cache),
        original
    );
    assert_eq!(cache.rebuilds(), 3);
}

// D17.6: the default 1100px box exceeds the 1080px canvas. Its geometry
// and center must remain the complete evaluated box, including after Scale.
#[test]
fn p5a_default_project_radial_scale_preserves_proportional_inner_edge() {
    let mut project = default_project(10.0);
    let mut radial = project.layers[1].clone();
    radial.glow = Animatable::Const(None);
    radial.modulations = vec![binding(ModulationTarget::Scale, 0.06)];
    if let LayerKind::Spectrum(spectrum) = &mut radial.kind {
        spectrum.point_count = 4;
        spectrum.fill = Animatable::Const(Fill::Solid(Rgba8::WHITE));
    }
    project.layers = vec![radial];
    let mut audio = frame();
    audio.bands.fill(0.0);
    let mut inner_edges = Vec::new();
    for value in [0.0, 1.0] {
        audio.modulators[0].value = value;
        let scene = evaluate(&project, 1.0, &audio);
        let rect = norm_rect_to_pixels(scene.items[0].rect, scene.width, scene.height);
        let mut pixels = Frame::new(1, 1).unwrap();
        let _ = render_frame(&scene, &Assets::default(), &mut pixels).unwrap();
        let evaluated = scene.items[0].rect;
        let center_x =
            ((evaluated.x + evaluated.width * 0.5) * scene.width as f32).round() as usize;
        let center_y =
            ((evaluated.y + evaluated.height * 0.5) * scene.height as f32).round() as usize;
        let first = (center_x..rect.x1)
            .find(|&x| pixels.rgba[(center_y * pixels.width as usize + x) * 4] > 127)
            .expect("rightward radial bar");
        inner_edges.push(first - center_x);
        eprintln!(
            "D17 default: modulator={value}, rect={rect:?}, first_x={first}, inner_edge={}",
            first - center_x
        );
    }
    assert!(
        (inner_edges[1] as f32 - inner_edges[0] as f32 * 1.06).abs() <= 1.0,
        "D17 default ring must expand by 6%; measured inner edges {inner_edges:?}"
    );
}

#[test]
fn p5a_offscreen_translucent_keyboard_preserves_underlying_canvas() {
    let mut project = default_project(10.0);
    project.output.width = 64;
    project.output.height = 64;
    let mut background = new_solid_layer(&project);
    background.rect = Animatable::Const(NormRect::FULL);
    background.corner_radius = Animatable::Const(0.0);
    background.kind = LayerKind::Solid {
        fill: Animatable::Const(Fill::Solid(Rgba8::new(17, 23, 31, 255))),
    };
    let mut keyboard = new_pitch_keyboard_layer(&project);
    keyboard.rect = Animatable::Const(NormRect::new(-0.25, 0.25, 1.0, 0.5));
    keyboard.opacity = Animatable::Const(0.65);
    if let LayerKind::PitchKeyboard(content) = &mut keyboard.kind {
        content.background = Animatable::Const(Fill::Solid(Rgba8::new(160, 30, 90, 130)));
        content.fill = Animatable::Const(Fill::Solid(Rgba8::new(45, 200, 170, 190)));
    }
    project.layers = vec![background, keyboard];
    let mut actual = Frame::new(1, 1).unwrap();
    let _ = render_frame(
        &evaluate(&project, 1.0, &frame()),
        &Assets::default(),
        &mut actual,
    )
    .unwrap();
    let mut reference = project.clone();
    reference.output.height = 32;
    reference.layers[1].rect = Animatable::Const(NormRect::FULL);
    let mut full = Frame::new(1, 1).unwrap();
    let _ = render_frame(
        &evaluate(&reference, 1.0, &frame()),
        &Assets::default(),
        &mut full,
    )
    .unwrap();
    for y in 0..64 {
        for x in 0..64 {
            let expected = if (16..48).contains(&y) && x < 48 {
                let offset = (((y - 16) * 64 + x + 16) * 4) as usize;
                &full.rgba[offset..offset + 4]
            } else {
                &[17, 23, 31, 255]
            };
            let offset = ((y * 64 + x) * 4) as usize;
            assert_eq!(&actual.rgba[offset..offset + 4], expected, "({x}, {y})");
        }
    }
}
