use super::*;
use crate::*;

fn review_project() -> Project {
    let mut project = default_project(10.0);
    let mut layer = new_text_layer(&project);
    layer.rect = Animatable::Const(NormRect::FULL);
    layer.glow = Animatable::Const(None);
    layer.modulations = vec![ModulationBinding {
        target: ModulationTarget::Scale,
        amount: 99.0,
        modulator: ModulatorId(1),
        curve: ModulationCurve::Linear,
    }];
    let LayerKind::Text(text) = &mut layer.kind else {
        unreachable!()
    };
    text.text = "春夏秋冬東西南北天地山川海空風雨雪花鳥月".into();
    text.align = TextAlign::Center;
    text.size_px = 10.0;
    text.style = Animatable::Const(TextStyle {
        outline: Some(StrokeStyle {
            width_px: 2.0,
            ..Default::default()
        }),
        shadow: Some(TextShadowStyle {
            spread_px: 2.0,
            blur_px: 48.0,
            ..Default::default()
        }),
        ..Default::default()
    });
    project.layers = vec![layer];
    project
}

#[test]
fn d17_eleven_review_repro_rejects_before_any_coverage_and_reports() {
    let project = review_project();
    let scene = evaluate(&project, 1.0, &crate::p5a_tests::frame());
    assert_eq!((scene.width, scene.height), (1920, 1080));
    let assets = crate::p5a_tests::assets();
    let mut frame = Frame::new(1, 1).unwrap();
    let ((a, b), calls) = glyph_preparation_probe::with_preparations(true, || {
        let a = render_frame(&scene, &assets, &mut frame).unwrap();
        assert!(frame.rgba.chunks_exact(4).all(|p| p == [0, 0, 0, 255]));
        let b = render_frame(&scene, &assets, &mut frame).unwrap();
        (a, b)
    });
    assert_eq!(calls, 0);
    assert_eq!(a, b);
    assert_eq!(a.unresolved_assets.len(), 1);
    let UnresolvedAsset::TextPreparationBudget {
        font_key,
        required_bytes,
        budget_bytes,
    } = &a.unresolved_assets[0]
    else {
        panic!("wrong report: {a:?}")
    };
    assert_eq!(font_key, DEFAULT_FONT_KEY);
    assert_eq!(*budget_bytes, MAX_TEXT_PREPARATION_BYTES);
    assert!(*required_bytes > *budget_bytes);
    assert_eq!(assets.text_cache_stats().prepared_bytes, 0);
}

#[test]
fn d17_eleven_eligible_program_is_prepared_once_across_visible_tiles() {
    let mut project = default_project(10.0);
    project.output.width = 320;
    project.output.height = 180;
    let mut layer = new_text_layer(&project);
    layer.rect = Animatable::Const(NormRect::new(-49.5, -49.5, 100.0, 100.0));
    layer.glow = Animatable::Const(None);
    let LayerKind::Text(text) = &mut layer.kind else {
        unreachable!()
    };
    text.text = "AB".repeat(600);
    text.size_px = 64.0;
    text.align = TextAlign::Center;
    text.style = Animatable::Const(TextStyle::default());
    project.layers = vec![layer];
    let scene = evaluate(&project, 1.0, &crate::p5a_tests::frame());
    let assets = crate::p5a_tests::assets();
    let mut first = Frame::new(1, 1).unwrap();
    let mut next = first.clone();
    let ((a, b), calls) = glyph_preparation_probe::with_preparations(false, || {
        (
            render_frame(&scene, &assets, &mut first).unwrap(),
            render_frame(&scene, &assets, &mut next).unwrap(),
        )
    });
    assert!(a.unresolved_assets.is_empty());
    assert_eq!(a, b);
    assert_eq!(
        calls, 1,
        "all tiles and the following frame must share the program"
    );
    assert_eq!(first, next);
    assert!(first.rgba.chunks_exact(4).any(|p| p[..3] != [0, 0, 0]));
    assert!(assets.text_cache_stats().prepared_bytes > 0);
    assert!(assets.text_cache_stats().prepared_bytes <= MAX_TEXT_PREPARATION_BYTES);
}

#[test]
fn d17_eleven_user_effect_canvas_uses_bounded_rgba_tiles_without_asserting() {
    let mut fonts = FontSet::new();
    fonts.insert(
        comic_core::LoadedFont::from_bytes(
            "font",
            include_bytes!("../../../tools/comic_lab/assets/fonts/ZenKurenaido-Regular.ttf")
                .to_vec(),
        )
        .unwrap(),
    );
    let cache = TextCache::default();
    let style = TextStyle {
        background: Some(TextBackgroundStyle {
            padding_px: 4096.0,
            fill: TextColor::WHITE,
            ..Default::default()
        }),
        ..Default::default()
    };
    let rect = PixelRect {
        x0: 0,
        y0: 0,
        x1: 2200,
        y1: 1100,
    };
    let mut largest = 0;
    cache
        .visit_window(
            &fonts,
            "曲名 AB",
            "font",
            128.0,
            &style,
            false,
            TextAlign::Center,
            rect,
            rect,
            |bitmap, _, _| {
                let bytes = bitmap.rgba.len() + bitmap.decoration.as_ref().map_or(0, Vec::len);
                largest = largest.max(bytes);
                assert!(bytes <= MAX_TEXT_RASTER_BYTES);
            },
        )
        .unwrap();
    assert_eq!(largest, MAX_TEXT_RASTER_BYTES);
}
