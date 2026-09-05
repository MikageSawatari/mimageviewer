use crate::model::{
    Animatable, BandCalibration, BandRange, BandWeighting, Fill, Fit, Glow, ImageId, Layer,
    LayerId, LayerKind, LyricsLayer, Modulator, ModulatorId, ModulatorSource, MusicColorMode,
    NormRect, Output, PitchKeyboardLayer, ProgressBarLayer, Project, Rgba8, SpectrumDirection,
    SpectrumLayer, SpectrumShape, TextAlign, TextLayer, TimeFormat, TimeTextLayer,
};
use crate::spectrum::Smoothing;

pub const DEFAULT_BACKGROUND_IMAGE_ID: &str = "background";
pub const DEFAULT_FONT_KEY: &str = "default";
pub const DEFAULT_OUTPUT: Output = Output {
    width: 1920,
    height: 1080,
    fps: 60.0,
};
pub const DEFAULT_SPECTRUM_BANDS: usize = 96;
pub const MIV_SPECTRUM_BANDS: usize = music_core::SPECTRUM_BAND_COUNT;
pub const PROJECT_VERSION: u32 = 1;
pub const KICK_ATTACK_SECS: f32 = 0.015;
pub const KICK_RELEASE_SECS: f32 = 0.18;
pub const KICK_THRESHOLD: f32 = 0.08;
pub const KICK_CALIBRATION_LOW_PERCENTILE: f32 = 50.0;
pub const KICK_CALIBRATION_HIGH_PERCENTILE: f32 = 99.0;
pub const BAND_ENERGY_CALIBRATION_LOW_PERCENTILE: f32 = 5.0;
pub const BAND_ENERGY_CALIBRATION_HIGH_PERCENTILE: f32 = 95.0;

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
        modulations: Vec::new(),
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
        display_gain_db: 0.0,
        modulators: Vec::new(),
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
                    zoom: Animatable::Const(1.0),
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
                    color_mode: MusicColorMode::Custom,
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
    new_layer(
        project,
        "ピッチ鍵盤",
        NormRect::new(0.05, 0.72, 0.9, 0.08),
        4.0,
        LayerKind::PitchKeyboard(PitchKeyboardLayer {
            background: Animatable::Const(Fill::Solid(Rgba8::new(0, 0, 0, 0))),
            color_mode: MusicColorMode::Custom,
            fill: Animatable::Const(Fill::Solid(Rgba8::new(64, 220, 255, 255))),
        }),
    )
}

pub fn new_image_layer(project: &Project, image: ImageId) -> Layer {
    new_layer(
        project,
        "画像",
        NormRect::FULL,
        0.0,
        LayerKind::Image {
            image,
            fit: Fit::Cover,
            zoom: Animatable::Const(1.0),
        },
    )
}

pub fn new_solid_layer(project: &Project) -> Layer {
    new_layer(
        project,
        "単色",
        NormRect::new(0.25, 0.25, 0.5, 0.5),
        12.0,
        LayerKind::Solid {
            fill: Animatable::Const(Fill::Solid(Rgba8::new(64, 120, 220, 255))),
        },
    )
}

pub fn new_linear_spectrum_layer(project: &Project) -> Layer {
    new_spectrum_layer(
        project,
        "直線スペクトラム",
        NormRect::new(0.08, 0.62, 0.84, 0.25),
        SpectrumShape::Bars {
            gap_ratio: 0.3,
            min_height_px: 3.0,
            thickness_px: 12.0,
        },
        96,
    )
}

pub fn new_miv_spectrum_layer(project: &Project) -> Layer {
    let mut layer = new_spectrum_layer(
        project,
        "mIV 横長スペクトラム",
        NormRect::new(0.05, 0.55, 0.9, 0.24),
        SpectrumShape::MivHorizontal,
        MIV_SPECTRUM_BANDS,
    );
    let LayerKind::Spectrum(spectrum) = &mut layer.kind else {
        unreachable!("new_spectrum_layer always returns a spectrum")
    };
    spectrum.band_count = MIV_SPECTRUM_BANDS;
    spectrum.color_mode = MusicColorMode::Miv;
    layer
}

/// 同じ対数周波数軸と矩形幅を共有する、mIV 横長スペクトラム + 鍵盤の組を返す。
pub fn new_miv_spectrum_keyboard_layers(project: &Project) -> [Layer; 2] {
    let spectrum = new_miv_spectrum_layer(project);
    let mut with_spectrum = project.clone();
    with_spectrum.layers.push(spectrum.clone());
    let mut keyboard = new_pitch_keyboard_layer(&with_spectrum);
    keyboard.name = unique_layer_name(&with_spectrum, "mIV ピッチ鍵盤");
    keyboard.rect = Animatable::Const(NormRect::new(0.05, 0.8, 0.9, 0.08));
    let LayerKind::PitchKeyboard(keyboard_layer) = &mut keyboard.kind else {
        unreachable!("new_pitch_keyboard_layer always returns a keyboard")
    };
    keyboard_layer.color_mode = MusicColorMode::Miv;
    [spectrum, keyboard]
}

pub fn new_radial_spectrum_layer(project: &Project) -> Layer {
    let radius_px = project.output.width.min(project.output.height) as f32 * 0.24;
    new_spectrum_layer(
        project,
        "円形スペクトラム",
        NormRect::new(0.18, 0.08, 0.64, 0.64),
        SpectrumShape::Radial {
            radius_px,
            thickness_px: 4.0,
            start_angle_deg: -90.0,
            sweep_deg: 360.0,
            direction: SpectrumDirection::Clockwise,
        },
        200,
    )
}

fn new_spectrum_layer(
    project: &Project,
    name: &str,
    rect: NormRect,
    shape: SpectrumShape,
    point_count: usize,
) -> Layer {
    new_layer(
        project,
        name,
        rect,
        0.0,
        LayerKind::Spectrum(SpectrumLayer {
            band_count: DEFAULT_SPECTRUM_BANDS,
            point_count,
            shape,
            color_mode: MusicColorMode::Custom,
            fill: Animatable::Const(Fill::Solid(Rgba8::new(120, 255, 35, 255))),
        }),
    )
}

pub fn new_progress_bar_layer(project: &Project) -> Layer {
    new_layer(
        project,
        "進捗バー",
        NormRect::new(0.05, 0.91, 0.9, 0.035),
        12.0,
        LayerKind::ProgressBar(ProgressBarLayer {
            border_thickness_px: 2.0,
            fill: Animatable::Const(Fill::Solid(Rgba8::WHITE)),
            border_fill: Animatable::Const(Fill::Solid(Rgba8::WHITE)),
        }),
    )
}

pub fn new_time_text_layer(project: &Project) -> Layer {
    new_layer(
        project,
        "時間表示",
        NormRect::new(0.74, 0.82, 0.24, 0.08),
        0.0,
        LayerKind::TimeText(TimeTextLayer {
            format: TimeFormat::MmSsOfTotal,
            font_key: DEFAULT_FONT_KEY.to_owned(),
            size_px: 50.0,
            align: TextAlign::Center,
            fill: Animatable::Const(Fill::Solid(Rgba8::WHITE)),
        }),
    )
}

pub fn new_text_layer(project: &Project) -> Layer {
    new_layer(
        project,
        "テキスト",
        NormRect::new(0.2, 0.42, 0.6, 0.12),
        0.0,
        LayerKind::Text(TextLayer {
            text: "テキスト".to_owned(),
            font_key: DEFAULT_FONT_KEY.to_owned(),
            size_px: 64.0,
            align: TextAlign::Center,
            fill: Animatable::Const(Fill::Solid(Rgba8::WHITE)),
        }),
    )
}

pub fn new_lyrics_layer(project: &Project) -> Layer {
    new_layer(
        project,
        "歌詞",
        NormRect::new(0.08, 0.78, 0.84, 0.1),
        0.0,
        LayerKind::Lyrics(LyricsLayer {
            lines: Vec::new(),
            fade_in_secs: 0.12,
            fade_out_secs: 0.12,
            font_key: DEFAULT_FONT_KEY.to_owned(),
            size_px: 64.0,
            align: TextAlign::Center,
            fill: Animatable::Const(Fill::Solid(Rgba8::WHITE)),
        }),
    )
}

fn new_layer(
    project: &Project,
    base_name: &str,
    rect: NormRect,
    corner_radius: f32,
    kind: LayerKind,
) -> Layer {
    Layer {
        id: LayerId(next_layer_id(project)),
        name: unique_layer_name(project, base_name),
        visible: Animatable::Const(true),
        opacity: Animatable::Const(1.0),
        rect: Animatable::Const(rect),
        rotation: Animatable::Const(0.0),
        corner_radius: Animatable::Const(corner_radius),
        glow: Animatable::Const(None),
        modulations: Vec::new(),
        kind,
    }
}

fn unique_layer_name(project: &Project, base: &str) -> String {
    if project.layers.iter().all(|layer| layer.name != base) {
        return base.to_owned();
    }
    (2..)
        .map(|number| format!("{base} {number}"))
        .find(|candidate| project.layers.iter().all(|layer| layer.name != *candidate))
        .expect("レイヤー名の番号は u32 より先にメモリ上限へ達する")
}

pub fn new_band_energy_modulator(project: &Project) -> Modulator {
    let next_id = project
        .modulators
        .iter()
        .map(|modulator| modulator.id.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    Modulator {
        id: ModulatorId(next_id),
        name: if next_id == 1 {
            "キックに合わせる動き".to_owned()
        } else {
            format!("キックに合わせる動き {next_id}")
        },
        source: ModulatorSource::BandOnset {
            range: BandRange::Hz {
                low_hz: 40.0,
                high_hz: 120.0,
            },
            smoothing: Smoothing::Envelope {
                attack_secs: KICK_ATTACK_SECS,
                release_secs: KICK_RELEASE_SECS,
            },
            weighting: BandWeighting::Raw,
            gain_db: 3.0,
            floor: 0.03,
            ceiling: 0.8,
            calibration: BandCalibration::Auto {
                low_percentile: KICK_CALIBRATION_LOW_PERCENTILE,
                high_percentile: KICK_CALIBRATION_HIGH_PERCENTILE,
            },
            threshold: KICK_THRESHOLD,
        },
    }
}

fn next_layer_id(project: &Project) -> u64 {
    project
        .layers
        .iter()
        .map(|layer| layer.id.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_default_project_has_the_reference_scene() {
        let project = default_project(10.0);
        assert_eq!(project.output, DEFAULT_OUTPUT);
        assert_eq!(project.display_gain_db, 0.0);
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
        assert_eq!(spectrum.color_mode, MusicColorMode::Custom);
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
        assert_eq!(keyboard.color_mode, MusicColorMode::Custom);
    }

    #[test]
    fn new_image_layer_is_full_frame_and_uses_a_unique_layer_id() {
        let project = default_project(10.0);
        let image = ImageId::new("image-1");
        let layer = new_image_layer(&project, image.clone());
        assert_eq!(layer.id, LayerId(5));
        assert_eq!(layer.rect, Animatable::Const(NormRect::FULL));
        assert_eq!(layer.opacity, Animatable::Const(1.0));
        assert_eq!(layer.name, "画像");
        assert!(matches!(
            layer.kind,
            LayerKind::Image { image: stored, fit: Fit::Cover, zoom: Animatable::Const(1.0) } if stored == image
        ));
    }

    #[test]
    fn added_layer_names_are_japanese_and_unique_and_all_presets_are_visible() {
        let mut project = default_project(10.0);
        let image = new_image_layer(&project, ImageId::new("asset-1"));
        assert_eq!(image.name, "画像");
        project.layers.push(image);
        assert_eq!(
            new_image_layer(&project, ImageId::new("asset-2")).name,
            "画像 2"
        );

        let presets = [
            new_radial_spectrum_layer(&project),
            new_linear_spectrum_layer(&project),
            new_miv_spectrum_layer(&project),
            new_pitch_keyboard_layer(&project),
            new_progress_bar_layer(&project),
            new_time_text_layer(&project),
            new_text_layer(&project),
            new_lyrics_layer(&project),
            new_solid_layer(&project),
        ];
        for layer in &presets {
            let rect = layer.rect.evaluate(0.0);
            assert!(rect.width > 0.0 && rect.height > 0.0, "{}", layer.name);
            assert!(
                rect.x < 1.0
                    && rect.y < 1.0
                    && rect.x + rect.width > 0.0
                    && rect.y + rect.height > 0.0,
                "{}",
                layer.name
            );
        }
        assert!(matches!(
            &presets[0].kind,
            LayerKind::Spectrum(SpectrumLayer {
                shape: SpectrumShape::Radial { .. },
                ..
            })
        ));
        assert!(matches!(
            &presets[1].kind,
            LayerKind::Spectrum(SpectrumLayer {
                shape: SpectrumShape::Bars { .. },
                ..
            })
        ));
        assert!(matches!(
            &presets[2].kind,
            LayerKind::Spectrum(SpectrumLayer {
                shape: SpectrumShape::MivHorizontal,
                color_mode: MusicColorMode::Miv,
                ..
            })
        ));
        assert_eq!(
            presets.map(|layer| layer.name),
            [
                "円形スペクトラム 2",
                "直線スペクトラム",
                "mIV 横長スペクトラム",
                "ピッチ鍵盤",
                "進捗バー 2",
                "時間表示 2",
                "テキスト",
                "歌詞",
                "単色",
            ]
        );
    }

    #[test]
    fn miv_spectrum_keyboard_preset_shares_width_axis_and_unique_ids() {
        let project = default_project(10.0);
        let [spectrum, keyboard] = new_miv_spectrum_keyboard_layers(&project);
        let spectrum_rect = spectrum.rect.evaluate(0.0);
        let keyboard_rect = keyboard.rect.evaluate(0.0);
        assert_eq!(spectrum.id, LayerId(5));
        assert_eq!(keyboard.id, LayerId(6));
        assert_eq!(spectrum_rect.x, keyboard_rect.x);
        assert_eq!(spectrum_rect.width, keyboard_rect.width);
        assert_eq!(spectrum.name, "mIV 横長スペクトラム");
        assert_eq!(keyboard.name, "mIV ピッチ鍵盤");
        assert!(matches!(
            keyboard.kind,
            LayerKind::PitchKeyboard(PitchKeyboardLayer {
                color_mode: MusicColorMode::Miv,
                ..
            })
        ));
    }

    #[test]
    fn new_band_energy_modulator_is_ready_for_kick_driven_motion() {
        let project = default_project(10.0);
        let modulator = new_band_energy_modulator(&project);
        assert_eq!(modulator.name, "キックに合わせる動き");
        assert_eq!(
            modulator.source,
            ModulatorSource::BandOnset {
                range: BandRange::Hz {
                    low_hz: 40.0,
                    high_hz: 120.0,
                },
                smoothing: Smoothing::Envelope {
                    attack_secs: KICK_ATTACK_SECS,
                    release_secs: KICK_RELEASE_SECS,
                },
                weighting: BandWeighting::Raw,
                gain_db: 3.0,
                floor: 0.03,
                ceiling: 0.8,
                calibration: BandCalibration::Auto {
                    low_percentile: KICK_CALIBRATION_LOW_PERCENTILE,
                    high_percentile: KICK_CALIBRATION_HIGH_PERCENTILE,
                },
                threshold: KICK_THRESHOLD,
            }
        );
    }
}
