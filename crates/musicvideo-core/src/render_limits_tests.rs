use crate::render_limits::*;
use crate::*;

fn text_project() -> Project {
    let mut project = default_project(10.0);
    project.layers = vec![
        new_text_layer(&project),
        new_time_text_layer(&project),
        new_lyrics_layer(&project),
    ];
    if let LayerKind::Lyrics(lyrics) = &mut project.layers[2].kind {
        lyrics.lines = vec![LyricLine {
            text: "Lyrics".into(),
            time_secs: Some(0.0),
        }];
    }
    for layer in &mut project.layers {
        let (size, style) = match &mut layer.kind {
            LayerKind::Text(text) => (&mut text.size_px, &mut text.style),
            LayerKind::TimeText(text) => (&mut text.size_px, &mut text.style),
            LayerKind::Lyrics(text) => (&mut text.size_px, &mut text.style),
            _ => unreachable!(),
        };
        *size = MAX_TEXT_SIZE_PX;
        let mut decoration = crate::p5a_tests::decorated_style();
        decoration.outline.as_mut().unwrap().width_px = MAX_TEXT_OUTLINE_PX;
        decoration.extra_outlines[0].width_px = MAX_TEXT_OUTLINE_PX;
        decoration.shadow.as_mut().unwrap().spread_px = MAX_TEXT_SHADOW_SPREAD_PX;
        *style = Animatable::Const(decoration);
        layer.glow = Animatable::Const(Some(Glow {
            color: Rgba8::WHITE,
            radius_px: MAX_GLOW_RADIUS_PX,
            spread_px: MAX_GLOW_SPREAD_PX,
        }));
    }
    project
}

fn assert_limits(item: &SceneItem) {
    assert_eq!(item.glow.unwrap().radius_px, MAX_GLOW_RADIUS_PX);
    assert_eq!(item.glow.unwrap().spread_px, MAX_GLOW_SPREAD_PX);
    let (SceneItemKind::Text { size_px, style, .. }
    | SceneItemKind::TimeText { size_px, style, .. }) = &item.kind
    else {
        panic!("text item")
    };
    assert_eq!(*size_px, MAX_TEXT_SIZE_PX);
    assert_eq!(style.outline.unwrap().width_px, MAX_TEXT_OUTLINE_PX);
    assert_eq!(style.extra_outlines[0].width_px, MAX_TEXT_OUTLINE_PX);
    assert_eq!(style.shadow.unwrap().spread_px, MAX_TEXT_SHADOW_SPREAD_PX);
}

#[test]
fn d17_nine_scale_two_caps_glow_and_glyph_coverage_for_all_text_kinds() {
    let mut project = text_project();
    for layer in &mut project.layers {
        layer.modulations = vec![ModulationBinding {
            target: ModulationTarget::Scale,
            modulator: ModulatorId(1),
            amount: 1.0,
            curve: ModulationCurve::Linear,
        }];
    }
    let scene = evaluate(&project, 1.0, &crate::p5a_tests::frame());
    assert_eq!(scene.items.len(), 3);
    for item in &scene.items {
        assert_limits(item);
        let (SceneItemKind::Text { style, .. } | SceneItemKind::TimeText { style, .. }) =
            &item.kind
        else {
            unreachable!()
        };
        // Non-coverage dimensions still scale, and comic-core retains ownership
        // of its existing 48px shadow blur limit.
        assert_eq!(style.shadow.unwrap().offset, (-8.0, 10.0));
        assert_eq!(style.shadow.unwrap().blur_px, 12.0);
        assert_eq!(style.echo.unwrap().offset, (16.0, -18.0));
        assert_eq!(style.background.unwrap().padding_px, 20.0);
        assert_eq!((style.letter_gap, style.line_gap), (24.0, 26.0));
    }
}

#[test]
fn d17_nine_scale_one_still_bounds_out_of_range_project_values() {
    let mut project = text_project();
    for layer in &mut project.layers {
        let (size, style) = match &mut layer.kind {
            LayerKind::Text(text) => (&mut text.size_px, &mut text.style),
            LayerKind::TimeText(text) => (&mut text.size_px, &mut text.style),
            LayerKind::Lyrics(text) => (&mut text.size_px, &mut text.style),
            _ => unreachable!(),
        };
        *size *= 100.0;
        let mut decoration = style.evaluate(0.0);
        decoration.outline.as_mut().unwrap().width_px *= 100.0;
        decoration.extra_outlines[0].width_px *= 100.0;
        decoration.shadow.as_mut().unwrap().spread_px *= 100.0;
        *style = Animatable::Const(decoration);
        layer.glow = Animatable::Const(Some(Glow {
            color: Rgba8::WHITE,
            radius_px: MAX_GLOW_RADIUS_PX * 100.0,
            spread_px: MAX_GLOW_SPREAD_PX * 100.0,
        }));
    }
    let scene = evaluate(&project, 1.0, &SpectrumFrame::default());
    assert_eq!(scene.items.len(), 3);
    for item in &scene.items {
        assert_limits(item);
    }
}
