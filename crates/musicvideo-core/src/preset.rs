use crate::model::{
    Animatable, Fill, Fit, Glow, ImageId, Layer, LayerId, LayerKind, NormRect, Output,
    PitchKeyboardLayer, ProgressBarLayer, Project, Rgba8, SpectrumDirection, SpectrumLayer,
    SpectrumShape, TextAlign, TimeFormat, TimeTextLayer,
};

pub const DEFAULT_BACKGROUND_IMAGE_ID: &str = "background";
pub const DEFAULT_FONT_KEY: &str = "default";
pub const DEFAULT_OUTPUT: Output = Output {
    width: 1920,
    height: 1080,
    fps: 60.0,
};
pub const DEFAULT_SPECTRUM_BANDS: usize = 96;
pub const PROJECT_VERSION: u32 = 1;

/// Builds the shared §1.4 reference scene used by preview and MP4 export.
pub fn default_project(duration_secs: f64) -> Project {
    let rect = |x: f32, y: f32, width: f32, height: f32| {
        Animatable::Const(NormRect::new(
            x / DEFAULT_OUTPUT.width as f32,
            y / DEFAULT_OUTPUT.height as f32,
            width / DEFAULT_OUTPUT.width as f32,
            height / DEFAULT_OUTPUT.height as f32,
        ))
    };
    let layer = |id: u64,
                 name: &str,
                 rect: Animatable<NormRect>,
                 corner_radius: f32,
                 glow: Option<Glow>,
                 kind: LayerKind| Layer {
        id: LayerId(id),
        name: name.to_owned(),
        visible: Animatable::Const(true),
        opacity: Animatable::Const(1.0),
        rect,
        rotation: Animatable::Const(0.0),
        corner_radius: Animatable::Const(corner_radius),
        glow: Animatable::Const(glow),
        kind,
    };
    let duration_secs = if duration_secs.is_finite() {
        duration_secs.max(0.0)
    } else {
        0.0
    };
    Project {
        version: PROJECT_VERSION,
        audio: None,
        images: Vec::new(),
        output: DEFAULT_OUTPUT,
        duration_secs,
        layers: vec![
            layer(
                1,
                "背景画像",
                Animatable::Const(NormRect::FULL),
                0.0,
                None,
                LayerKind::Image {
                    image: ImageId::new(DEFAULT_BACKGROUND_IMAGE_ID),
                    fit: Fit::Cover,
                },
            ),
            layer(
                2,
                "円形スペクトラム",
                rect(410.0, 0.0, 1100.0, 1100.0),
                0.0,
                Some(Glow {
                    color: Rgba8::new(255, 136, 255, 255),
                    radius_px: 20.0,
                    spread_px: 0.0,
                }),
                LayerKind::Spectrum(SpectrumLayer {
                    band_count: DEFAULT_SPECTRUM_BANDS,
                    point_count: 200,
                    shape: SpectrumShape::Radial {
                        radius_px: 380.0,
                        thickness_px: 4.0,
                        start_angle_deg: -90.0,
                        sweep_deg: 360.0,
                        direction: SpectrumDirection::Clockwise,
                    },
                    fill: Animatable::Const(Fill::Solid(Rgba8::new(120, 255, 35, 255))),
                }),
            ),
            layer(
                3,
                "進捗バー",
                rect(60.0, 1000.0, 1800.0, 50.0),
                20.0,
                Some(Glow {
                    color: Rgba8::new(255, 0, 34, 255),
                    radius_px: 20.0,
                    spread_px: 0.0,
                }),
                LayerKind::ProgressBar(ProgressBarLayer {
                    border_thickness_px: 2.0,
                    fill: Animatable::Const(Fill::Solid(Rgba8::WHITE)),
                    border_fill: Animatable::Const(Fill::Solid(Rgba8::WHITE)),
                }),
            ),
            layer(
                4,
                "時間表示",
                rect(1520.0, 920.0, 400.0, 100.0),
                0.0,
                Some(Glow {
                    color: Rgba8::WHITE,
                    radius_px: 5.0,
                    spread_px: 0.0,
                }),
                LayerKind::TimeText(TimeTextLayer {
                    format: TimeFormat::MmSsOfTotal,
                    font_key: DEFAULT_FONT_KEY.to_owned(),
                    size_px: 50.0,
                    align: TextAlign::Center,
                    fill: Animatable::Const(Fill::Solid(Rgba8::BLACK)),
                }),
            ),
        ],
    }
}

pub fn new_pitch_keyboard_layer(project: &Project) -> Layer {
    let next_id = project
        .layers
        .iter()
        .map(|layer| layer.id.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    Layer {
        id: LayerId(next_id),
        name: "ピッチ鍵盤".to_owned(),
        visible: Animatable::Const(true),
        opacity: Animatable::Const(1.0),
        rect: Animatable::Const(NormRect::new(0.05, 0.72, 0.9, 0.08)),
        rotation: Animatable::Const(0.0),
        corner_radius: Animatable::Const(4.0),
        glow: Animatable::Const(None),
        kind: LayerKind::PitchKeyboard(PitchKeyboardLayer {
            background: Animatable::Const(Fill::Solid(Rgba8::new(0, 0, 0, 0))),
            fill: Animatable::Const(Fill::Solid(Rgba8::new(64, 220, 255, 255))),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_default_project_has_the_reference_scene() {
        let project = default_project(10.0);
        assert_eq!(project.output, DEFAULT_OUTPUT);
        assert_eq!(project.layers.len(), 4);
        assert_eq!(project.layers[0].name, "背景画像");
        assert_eq!(project.layers[1].name, "円形スペクトラム");
        assert_eq!(project.layers[2].name, "進捗バー");
        assert_eq!(project.layers[3].name, "時間表示");
        let LayerKind::Spectrum(spectrum) = &project.layers[1].kind else {
            panic!("2 番目のレイヤはスペクトラムである必要があります");
        };
        assert_eq!(spectrum.band_count, DEFAULT_SPECTRUM_BANDS);
        assert_eq!(spectrum.point_count, 200);
        assert!(matches!(spectrum.shape, SpectrumShape::Radial { .. }));
        assert_eq!(project.layers[2].corner_radius, Animatable::Const(20.0));
        assert_eq!(project.layers[3].glow.evaluate(0.0).unwrap().radius_px, 5.0);
    }

    #[test]
    fn new_pitch_keyboard_background_is_transparent() {
        let project = default_project(10.0);
        let layer = new_pitch_keyboard_layer(&project);
        assert_eq!(layer.id, LayerId(5));
        let LayerKind::PitchKeyboard(keyboard) = layer.kind else {
            panic!("追加レイヤはピッチ鍵盤である必要があります");
        };
        assert_eq!(
            keyboard.background,
            Animatable::Const(Fill::Solid(Rgba8::new(0, 0, 0, 0)))
        );
    }
}
