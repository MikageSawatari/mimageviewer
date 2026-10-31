use crate::*;

fn box_project() -> Project {
    let mut project = default_project(10.0);
    project.layers = vec![new_solid_layer(&project)];
    project.output.width = 200;
    project.output.height = 100;
    project.layers[0].rect = Animatable::Const(NormRect::new(0.2, 0.2, 0.4, 0.2));
    project
}

fn bounds(project: &Project, time: f64, values: &[ModulatorValue]) -> LayerBounds {
    evaluate_layer_bounds(project, project.layers[0].id, time, values).unwrap()
}

fn agrees_with_scene(project: &Project, time: f64, values: &[ModulatorValue]) {
    let frame = SpectrumFrame {
        modulators: values.to_vec(),
        ..Default::default()
    };
    let scene = evaluate(project, time, &frame);
    let bounds = bounds(project, time, values);
    assert_eq!(scene.items.len(), usize::from(bounds.visible));
    if bounds.visible {
        assert_eq!(bounds.rect, scene.items[0].rect);
        assert_eq!(bounds.rotation_deg, scene.items[0].rotation_deg);
    }
}

#[test]
fn selection_constant_rect_and_time_clamping_match_scene() {
    let project = box_project();
    let bounds = bounds(&project, 3.0, &[]);
    assert_eq!(bounds.rect, NormRect::new(0.2, 0.2, 0.4, 0.2));
    assert_eq!(bounds.rotation_deg, 0.0);
    assert!(bounds.visible);
    assert_eq!(
        bounds.corners,
        [(0.2, 0.2), (0.6, 0.2), (0.6, 0.4), (0.2, 0.4)]
    );
    for t in [-1.0, 3.0, 20.0, f64::NAN] {
        agrees_with_scene(&project, t, &[]);
    }
}

#[test]
fn selection_keyframe_between_keys_uses_scene_time_clamping() {
    let mut project = box_project();
    project.layers[0].rect = Animatable::Track(
        Track::new(vec![
            Keyframe {
                t: 0.0,
                value: NormRect::new(0.0, 0.0, 0.2, 0.2),
                interp: Interp::Linear,
            },
            Keyframe {
                t: 10.0,
                value: NormRect::new(1.0, 0.5, 0.4, 0.4),
                interp: Interp::Linear,
            },
        ])
        .unwrap(),
    );
    assert_eq!(
        bounds(&project, 5.0, &[]).rect,
        NormRect::new(0.5, 0.25, 0.3, 0.3)
    );
    assert_eq!(
        bounds(&project, 20.0, &[]).rect,
        NormRect::new(1.0, 0.5, 0.4, 0.4)
    );
    for t in [-1.0, 5.0, 20.0, f64::NAN] {
        agrees_with_scene(&project, t, &[]);
    }
}

#[test]
fn selection_scale_rotation_opacity_bindings_match_scene_in_order() {
    let mut project = box_project();
    project.layers[0].modulations = vec![
        ModulationBinding {
            target: ModulationTarget::Scale,
            modulator: ModulatorId(1),
            amount: 1.0,
            curve: ModulationCurve::Squared,
        },
        ModulationBinding {
            target: ModulationTarget::Rotation,
            modulator: ModulatorId(1),
            amount: 40.0,
            curve: ModulationCurve::Linear,
        },
        ModulationBinding {
            target: ModulationTarget::Opacity,
            modulator: ModulatorId(1),
            amount: 1.0,
            curve: ModulationCurve::Linear,
        },
        ModulationBinding {
            target: ModulationTarget::Opacity,
            modulator: ModulatorId(1),
            amount: -2.0,
            curve: ModulationCurve::Linear,
        },
    ];
    let values = [ModulatorValue {
        id: ModulatorId(1),
        value: 0.5,
    }];
    let b = bounds(&project, 3.0, &values);
    assert!((b.rect.width - 0.5).abs() < 1e-6);
    assert!((b.rect.x - 0.15).abs() < 1e-6);
    assert_eq!(b.rotation_deg, 20.0);
    assert!(!b.visible);
    agrees_with_scene(&project, 3.0, &values);
    project.layers[0].modulations.truncate(2);
    agrees_with_scene(&project, 3.0, &values);
}

#[test]
fn selection_rotation_uses_output_pixel_aspect_ratio() {
    let mut project = box_project();
    project.layers[0].rotation = Animatable::Const(90.0);
    let b = bounds(&project, 3.0, &[]);
    for (actual, expected) in
        b.corners
            .iter()
            .zip([(0.45, -0.1), (0.45, 0.7), (0.35, 0.7), (0.35, -0.1)])
    {
        assert!((actual.0 - expected.0).abs() < 1e-6);
        assert!((actual.1 - expected.1).abs() < 1e-6);
    }
    agrees_with_scene(&project, 3.0, &[]);
}

#[test]
fn selection_hidden_visibility_still_evaluates_box_at_frame_time() {
    let mut project = box_project();
    project.layers[0].visible = Animatable::Track(
        Track::new(vec![
            Keyframe {
                t: 0.0,
                value: true,
                interp: Interp::Step,
            },
            Keyframe {
                t: 5.0,
                value: false,
                interp: Interp::Step,
            },
        ])
        .unwrap(),
    );
    assert!(bounds(&project, 4.0, &[]).visible);
    assert!(!bounds(&project, 5.0, &[]).visible);
    assert_eq!(
        bounds(&project, 5.0, &[]).rect,
        project.layers[0].rect.evaluate(5.0)
    );
    agrees_with_scene(&project, 5.0, &[]);
}

#[test]
fn selection_zero_opacity_and_lyrics_gap_are_hidden() {
    let mut project = box_project();
    project.layers[0].opacity = Animatable::Const(0.0);
    assert!(!bounds(&project, 3.0, &[]).visible);
    agrees_with_scene(&project, 3.0, &[]);
    let mut lyrics = new_lyrics_layer(&project);
    let LayerKind::Lyrics(content) = &mut lyrics.kind else {
        unreachable!()
    };
    content.lines = vec![LyricLine {
        text: "歌詞".into(),
        time_secs: Some(2.0),
    }];
    project.layers = vec![lyrics];
    assert!(!bounds(&project, 1.0, &[]).visible);
    assert!(bounds(&project, 3.0, &[]).visible);
    agrees_with_scene(&project, 1.0, &[]);
    agrees_with_scene(&project, 3.0, &[]);
}

#[test]
fn selection_render_hash_uses_unclipped_layer_geometry() {
    // D17.6 intentionally changes this fixture: the default radial layer extends
    // below the canvas and the first animated/rotated solid starts beyond its left.
    // Their geometry now comes from the full boxes before the final canvas crop.
    let mut project = default_project(10.0);
    project.output.width = 160;
    project.output.height = 90;
    let assets = Assets::default();
    assets.insert_image(
        ImageId::new(DEFAULT_BACKGROUND_IMAGE_ID),
        ImageAsset::new(
            2,
            2,
            vec![
                40, 80, 120, 255, 180, 90, 20, 255, 20, 200, 60, 255, 70, 20, 190, 255,
            ],
        )
        .unwrap(),
    );
    let mut solid = new_solid_layer(&project);
    solid.rect = Animatable::Track(
        Track::new(vec![
            Keyframe {
                t: 0.0,
                value: NormRect::new(-0.1, 0.1, 0.3, 0.25),
                interp: Interp::Linear,
            },
            Keyframe {
                t: 10.0,
                value: NormRect::new(0.6, 0.4, 0.3, 0.25),
                interp: Interp::Linear,
            },
        ])
        .unwrap(),
    );
    solid.rotation = Animatable::Const(23.0);
    solid.opacity = Animatable::Const(0.7);
    project.layers.push(solid);
    let spectrum = SpectrumFrame::new((0..96).map(|i| (i % 11) as f32 / 10.0).collect());
    let hashes: Vec<u64> = [0.0, 3.0, 9.0]
        .into_iter()
        .map(|time| {
            let mut frame = Frame::new(160, 90).unwrap();
            let _ =
                render_frame(&evaluate(&project, time, &spectrum), &assets, &mut frame).unwrap();
            frame
                .rgba
                .iter()
                .fold(0xcbf29ce484222325_u64, |hash, byte| {
                    (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
                })
        })
        .collect();
    assert_eq!(
        hashes,
        vec![
            7673554586247693965,
            13406108200308214447,
            11219486302341893356
        ]
    );
}
