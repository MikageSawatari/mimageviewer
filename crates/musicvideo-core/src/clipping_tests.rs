//! D17.6: canvas clipping must not change a layer's geometry or local paint.
//!
//! Ordinary layers are rendered in a same-sized local canvas, then placed and
//! cropped without resampling. Rotation/glow use a translated larger canvas so
//! their pixels outside the original box survive. Coordinates are exact binary
//! fractions, so normalization cannot introduce a different edge-rounding result.

use crate::*;
use comic_core::{FontSet, LoadedFont};

const VIEW: u32 = 128;
const PADDED: u32 = 512;
const OFFSET: i32 = 128;
const IMAGE: &str = "d17-clipping-pattern";
const PARTIAL_RECTS: [(i32, i32, u32, u32); 2] = [(-32, -16, 128, 96), (32, 48, 128, 96)];

fn pixel_rect(x: i32, y: i32, width: u32, height: u32, canvas: u32) -> NormRect {
    NormRect::new(
        x as f32 / canvas as f32,
        y as f32 / canvas as f32,
        width as f32 / canvas as f32,
        height as f32 / canvas as f32,
    )
}

fn gradient() -> Fill {
    Fill::LinearGradient {
        stops: GradientStops::new(vec![
            GradientStop {
                position: 0.0,
                color: Rgba8::new(240, 32, 80, 255),
            },
            GradientStop {
                position: 1.0,
                color: Rgba8::new(24, 220, 248, 255),
            },
        ])
        .unwrap(),
        angle_deg: 35.0,
    }
}

fn assets(needs_font: bool) -> Assets {
    let mut fonts = FontSet::new();
    if needs_font {
        fonts.insert(
            LoadedFont::from_bytes(
                DEFAULT_FONT_KEY,
                std::fs::read(r"C:\Windows\Fonts\YuGothM.ttc")
                    .expect("D17 clipping text fixture needs YuGothM.ttc"),
            )
            .unwrap(),
        );
    }
    let assets = Assets::new(fonts);
    let mut rgba = Vec::new();
    for y in 0..8 {
        for x in 0..12 {
            rgba.extend_from_slice(&[
                (20 + x * 19) as u8,
                (30 + y * 27) as u8,
                if (x + y) % 2 == 0 { 245 } else { 45 },
                255,
            ]);
        }
    }
    assets.insert_image(ImageId::new(IMAGE), ImageAsset::new(12, 8, rgba).unwrap());
    assets
}

fn audio() -> SpectrumFrame {
    let mut frame = SpectrumFrame::new(
        (0..12)
            .map(|index| 0.3 + (index % 4) as f32 * 0.2)
            .collect(),
    );
    frame.miv_spectrum.bands = (0..MIV_SPECTRUM_BANDS)
        .map(|index| music_core::MivSpectrumBandFrame {
            value: 0.35 + (index % 4) as f32 * 0.18,
            trail: 0.8,
            onset: 0.6,
        })
        .collect();
    for (index, levels) in frame.keyboard.levels.iter_mut().enumerate() {
        levels.center = if index % 3 == 0 { 0.9 } else { 0.25 };
        levels.even = 0.4;
        levels.odd = 0.2;
    }
    frame
}

fn render(project: &Project, assets: &Assets) -> Frame {
    let mut frame = Frame::new(1, 1).unwrap();
    let report = render_frame(&evaluate(project, 3.25, &audio()), assets, &mut frame).unwrap();
    assert!(report.unresolved_assets.is_empty(), "{report:?}");
    frame
}

fn assert_matches_crop(layer: Layer, rectangles: &[(i32, i32, u32, u32)]) {
    let assets = assets(matches!(
        &layer.kind,
        LayerKind::Text(_) | LayerKind::TimeText(_) | LayerKind::Lyrics(_)
    ));
    let transformed = layer.rotation.evaluate(3.25) != 0.0 || layer.glow.evaluate(3.25).is_some();
    let mut project = default_project(10.0);
    project.output.width = VIEW;
    project.output.height = VIEW;
    project.layers = vec![layer];
    for &(x, y, width, height) in rectangles {
        project.layers[0].rect = Animatable::Const(pixel_rect(x, y, width, height, VIEW));
        let actual = render(&project, &assets);
        let mut reference_project = project.clone();
        let (crop, oracle) = if transformed {
            reference_project.output.width = PADDED;
            reference_project.output.height = PADDED;
            reference_project.layers[0].rect =
                Animatable::Const(pixel_rect(x + OFFSET, y + OFFSET, width, height, PADDED));
            let reference = render(&reference_project, &assets);
            let mut crop = Vec::with_capacity(actual.rgba.len());
            for row in 0..VIEW as usize {
                let start = ((row + OFFSET as usize) * PADDED as usize + OFFSET as usize) * 4;
                crop.extend_from_slice(&reference.rgba[start..start + VIEW as usize * 4]);
            }
            (crop, "translated")
        } else {
            // The geometry oracle has the original width and height at origin
            // zero. Placement only copies pixels; it never rescales a clipped
            // rectangle or changes the AA coordinates used to draw its content.
            reference_project.output.width = width;
            reference_project.output.height = height;
            reference_project.layers[0].rect = Animatable::Const(NormRect::FULL);
            let reference = render(&reference_project, &assets);
            let mut crop = vec![0, 0, 0, 255].repeat((VIEW * VIEW) as usize);
            for destination_y in 0..VIEW as i32 {
                let source_y = destination_y - y;
                if !(0..height as i32).contains(&source_y) {
                    continue;
                }
                for destination_x in 0..VIEW as i32 {
                    let source_x = destination_x - x;
                    if !(0..width as i32).contains(&source_x) {
                        continue;
                    }
                    let source = (source_y as usize * width as usize + source_x as usize) * 4;
                    let destination =
                        (destination_y as usize * VIEW as usize + destination_x as usize) * 4;
                    crop[destination..destination + 4]
                        .copy_from_slice(&reference.rgba[source..source + 4]);
                }
            }
            (crop, "local")
        };
        assert!(
            crop.chunks_exact(4).any(|pixel| pixel[..3] != [0, 0, 0]),
            "{} at ({x}, {y}, {width}, {height}) must visibly intersect the crop",
            project.layers[0].name
        );
        if let Some(index) = actual.rgba.iter().zip(&crop).position(|(a, b)| a != b) {
            let pixel = index / 4;
            panic!(
                "{} at ({x}, {y}, {width}, {height}): pixel ({}, {}) actual {:?}, {oracle} crop {:?}",
                project.layers[0].name,
                pixel % VIEW as usize,
                pixel / VIEW as usize,
                &actual.rgba[pixel * 4..pixel * 4 + 4],
                &crop[pixel * 4..pixel * 4 + 4],
            );
        }
    }
}

fn solid() -> Layer {
    let mut layer = new_solid_layer(&default_project(10.0));
    layer.kind = LayerKind::Solid {
        fill: Animatable::Const(gradient()),
    };
    layer
}

fn spectrum(shape: SpectrumShape) -> Layer {
    let mut layer = new_linear_spectrum_layer(&default_project(10.0));
    let LayerKind::Spectrum(content) = &mut layer.kind else {
        unreachable!()
    };
    content.band_count = if shape == SpectrumShape::MivHorizontal {
        MIV_SPECTRUM_BANDS
    } else {
        12
    };
    content.point_count = 16;
    content.shape = shape;
    content.fill = Animatable::Const(gradient());
    layer
}

fn text_style() -> Animatable<TextStyle> {
    Animatable::Const(TextStyle {
        outline: Some(StrokeStyle {
            width_px: 1.0,
            color: TextColor::WHITE,
        }),
        letter_gap: 1.0,
        ..TextStyle::default()
    })
}

#[test]
fn d17_clipping_image_fit_and_zoom_match_local_crop() {
    let project = default_project(10.0);
    for fit in [Fit::Cover, Fit::Contain, Fit::Stretch] {
        for zoom in [1.0, 1.5] {
            let mut layer = new_image_layer(&project, ImageId::new(IMAGE));
            layer.name = format!("{fit:?}, zoom {zoom}");
            layer.kind = LayerKind::Image {
                image: ImageId::new(IMAGE),
                fit,
                zoom: Animatable::Const(zoom),
            };
            assert_matches_crop(layer.clone(), &PARTIAL_RECTS);
            // A covering image is still larger than the viewport. The first-image
            // opaque fast path must not rescale it to the clipped canvas size.
            assert_matches_crop(layer, &[(-32, -32, 192, 192)]);
        }
    }
}

#[test]
fn d17_clipping_solid_geometry_and_fill_match_local_crop() {
    assert_matches_crop(solid(), &PARTIAL_RECTS);
}

#[test]
fn d17_clipping_bars_match_local_crop() {
    assert_matches_crop(
        spectrum(SpectrumShape::Bars {
            gap_ratio: 0.25,
            min_height_px: 5.0,
            thickness_px: 5.0,
        }),
        &PARTIAL_RECTS,
    );
}

#[test]
fn d17_clipping_radial_matches_local_crop() {
    assert_matches_crop(
        spectrum(SpectrumShape::Radial {
            inner_ratio: 0.5,
            thickness_px: 4.0,
            start_angle_deg: -90.0,
            sweep_deg: 360.0,
            direction: SpectrumDirection::Clockwise,
        }),
        &PARTIAL_RECTS,
    );
}

#[test]
fn d17_clipping_miv_horizontal_matches_local_crop() {
    assert_matches_crop(spectrum(SpectrumShape::MivHorizontal), &PARTIAL_RECTS);
}

#[test]
fn d17_clipping_pitch_keyboard_matches_local_crop() {
    let mut layer = new_pitch_keyboard_layer(&default_project(10.0));
    let LayerKind::PitchKeyboard(content) = &mut layer.kind else {
        unreachable!()
    };
    content.fill = Animatable::Const(gradient());
    assert_matches_crop(layer, &PARTIAL_RECTS);
}

#[test]
fn d17_clipping_progress_bar_matches_local_crop() {
    let mut layer = new_progress_bar_layer(&default_project(10.0));
    let LayerKind::ProgressBar(content) = &mut layer.kind else {
        unreachable!()
    };
    content.fill = Animatable::Const(gradient());
    content.border_fill = Animatable::Const(Fill::Solid(Rgba8::new(180, 220, 32, 255)));
    content.border_thickness_px = 3.0;
    assert_matches_crop(layer, &PARTIAL_RECTS);
}

#[test]
fn d17_clipping_text_matches_local_crop() {
    let mut layer = new_text_layer(&default_project(10.0));
    let LayerKind::Text(content) = &mut layer.kind else {
        unreachable!()
    };
    content.text = "Clip 浮".into();
    content.size_px = 18.0;
    content.style = text_style();
    content.fill = Animatable::Const(gradient());
    assert_matches_crop(layer, &PARTIAL_RECTS);
}

#[test]
fn d17_clipping_time_text_matches_local_crop() {
    let mut layer = new_time_text_layer(&default_project(10.0));
    let LayerKind::TimeText(content) = &mut layer.kind else {
        unreachable!()
    };
    content.format = TimeFormat::MmSs;
    content.size_px = 18.0;
    content.style = text_style();
    content.fill = Animatable::Const(gradient());
    assert_matches_crop(layer, &PARTIAL_RECTS);
}

#[test]
fn d17_clipping_lyrics_match_local_crop() {
    let mut layer = new_lyrics_layer(&default_project(10.0));
    let LayerKind::Lyrics(content) = &mut layer.kind else {
        unreachable!()
    };
    content.lines = vec![LyricLine {
        text: "歌詞 Clip".into(),
        time_secs: Some(0.0),
    }];
    content.size_px = 18.0;
    content.style = text_style();
    content.fill = Animatable::Const(gradient());
    assert_matches_crop(layer, &PARTIAL_RECTS);
}

#[test]
fn d17_clipping_rotation_and_glow_match_translated_crop() {
    let mut layer = solid();
    layer.rotation = Animatable::Const(90.0);
    layer.glow = Animatable::Const(Some(Glow {
        color: Rgba8::new(50, 180, 240, 200),
        radius_px: 6.0,
        spread_px: 2.0,
    }));
    assert_matches_crop(layer, &PARTIAL_RECTS);
}

#[test]
fn d17_clipping_fully_outside_rect_can_rotate_into_canvas() {
    let mut layer = solid();
    layer.rotation = Animatable::Const(90.0);
    // Unrotated right edge = -16; rotation moves the right edge to +16.
    assert_matches_crop(layer, &[(-48, 24, 32, 96)]);
}

#[test]
fn d17_clipping_fully_outside_rect_can_glow_into_canvas() {
    let mut layer = solid();
    layer.corner_radius = Animatable::Const(0.0);
    layer.glow = Animatable::Const(Some(Glow {
        color: Rgba8::new(50, 180, 240, 255),
        radius_px: 12.0,
        spread_px: 4.0,
    }));
    // The body ends at -8; only its halo is visible in the viewport.
    assert_matches_crop(layer, &[(-24, 32, 16, 64)]);
}

#[test]
fn d17_clipping_selection_keeps_unclipped_rect_and_rotated_corners() {
    let mut project = default_project(10.0);
    project.output.width = VIEW;
    project.output.height = VIEW;
    project.layers = vec![solid()];
    for (rect, corners) in [
        (
            (-32, -16, 128, 96),
            [(80, -32), (80, 96), (-16, 96), (-16, -32)],
        ),
        (
            (32, 48, 128, 96),
            [(144, 32), (144, 160), (48, 160), (48, 32)],
        ),
    ] {
        let (x, y, width, height) = rect;
        let expected = pixel_rect(x, y, width, height, VIEW);
        project.layers[0].rect = Animatable::Const(expected);
        project.layers[0].rotation = Animatable::Const(90.0);
        let bounds = evaluate_layer_bounds(&project, project.layers[0].id, 3.25, &[]).unwrap();
        let scene = evaluate(&project, 3.25, &audio());
        assert!(bounds.visible);
        assert_eq!(bounds.rect, expected);
        assert_eq!(bounds.rect, scene.items[0].rect);
        assert_eq!(bounds.rotation_deg, scene.items[0].rotation_deg);
        for (actual, (expected_x, expected_y)) in bounds.corners.iter().zip(corners) {
            assert!((actual.0 * VIEW as f32 - expected_x as f32).abs() < 0.0001);
            assert!((actual.1 * VIEW as f32 - expected_y as f32).abs() < 0.0001);
        }
        assert_matches_crop(project.layers[0].clone(), &[rect]);
    }
}
