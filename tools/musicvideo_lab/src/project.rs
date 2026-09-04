use std::sync::Arc;

use comic_core::{FontSet, LoadedFont};
use eframe::egui;
use musicvideo_core::{
    Animatable, Assets, Fill, Fit, ImageAsset, ImageId, Layer, LayerId, LayerKind, NormRect,
    Output, ProgressBarLayer, Project, Rgba8, SpectrumLayer, TextAlign, TimeFormat, TimeTextLayer,
};

pub const BACKGROUND_ID: &str = "background";
pub const DEFAULT_FONT_KEY: &str = "default";
pub const OUTPUT_WIDTH: u32 = 1920;
pub const OUTPUT_HEIGHT: u32 = 1080;
pub const OUTPUT_FPS: f64 = 60.0;
pub const DEFAULT_DURATION_SECS: f64 = 10.0;
pub const SPECTRUM_BANDS: usize = 96;

#[derive(Clone)]
pub struct FontData {
    pub bytes: Vec<u8>,
}

pub fn load_default_font() -> Option<FontData> {
    for path in [
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
        r"C:\Windows\Fonts\YuGothR.ttc",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            return Some(FontData { bytes });
        }
    }
    None
}

pub fn create_assets(font: Option<&FontData>) -> Result<Arc<Assets>, String> {
    let mut fonts = FontSet::new();
    if let Some(font) = font {
        let loaded = LoadedFont::from_bytes(DEFAULT_FONT_KEY, font.bytes.clone())
            .map_err(|error| format!("フォントを読み込めません: {error}"))?;
        fonts.insert(loaded);
    }
    Ok(Arc::new(Assets::new(fonts)))
}

pub fn setup_egui(ctx: &egui::Context, font: Option<&FontData>) {
    if let Some(font) = font {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "musicvideo-japanese".to_owned(),
            Arc::new(egui::FontData::from_owned(font.bytes.clone())),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, "musicvideo-japanese".to_owned());
        }
        ctx.set_fonts(fonts);
    }
    ctx.set_visuals(egui::Visuals::dark());
}

pub fn default_project(duration_secs: f64) -> Project {
    let rect = |x: f32, y: f32, width: f32, height: f32| {
        Animatable::Const(NormRect::new(
            x / OUTPUT_WIDTH as f32,
            y / OUTPUT_HEIGHT as f32,
            width / OUTPUT_WIDTH as f32,
            height / OUTPUT_HEIGHT as f32,
        ))
    };
    let common = |id: u64, name: &str, rect: Animatable<NormRect>, kind: LayerKind| Layer {
        id: LayerId(id),
        name: name.to_owned(),
        visible: Animatable::Const(true),
        opacity: Animatable::Const(1.0),
        rect,
        kind,
    };
    Project {
        version: 1,
        output: Output {
            width: OUTPUT_WIDTH,
            height: OUTPUT_HEIGHT,
            fps: OUTPUT_FPS,
        },
        duration_secs: duration_secs.max(0.0),
        layers: vec![
            common(
                1,
                "背景画像",
                Animatable::Const(NormRect::FULL),
                LayerKind::Image {
                    image: ImageId::new(BACKGROUND_ID),
                    fit: Fit::Cover,
                },
            ),
            common(
                2,
                "直線スペクトラム",
                rect(160.0, 220.0, 1600.0, 600.0),
                LayerKind::Spectrum(SpectrumLayer {
                    band_count: SPECTRUM_BANDS,
                    gap_ratio: 0.30,
                    min_height_px: 3.0,
                    thickness: 12.0,
                    fill: Animatable::Const(Fill::Solid(Rgba8::new(120, 255, 35, 220))),
                }),
            ),
            common(
                3,
                "進捗バー",
                rect(60.0, 1000.0, 1800.0, 50.0),
                LayerKind::ProgressBar(ProgressBarLayer {
                    border_thickness_px: 2.0,
                    fill: Animatable::Const(Fill::Solid(Rgba8::WHITE)),
                    border_fill: Animatable::Const(Fill::Solid(Rgba8::WHITE)),
                }),
            ),
            common(
                4,
                "時間表示",
                rect(1520.0, 920.0, 400.0, 100.0),
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

pub fn synthetic_background(width: u32, height: u32) -> ImageAsset {
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let r = 28 + (x * 52 / width) as u8;
            let g = 38 + (y * 48 / height) as u8;
            let b = 70 + ((x + y) * 72 / (width + height)) as u8;
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    ImageAsset::new(width, height, rgba).expect("fixed synthetic image dimensions")
}
