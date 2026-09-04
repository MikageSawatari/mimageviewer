use std::sync::Arc;

use comic_core::{FontSet, LoadedFont};
use eframe::egui;
use musicvideo_core::{
    Animatable, Assets, Fill, ImageAsset, Layer, LayerId, LayerKind, NormRect, PitchKeyboardLayer,
    Project, Rgba8,
};

pub use musicvideo_core::{
    DEFAULT_BACKGROUND_IMAGE_ID as BACKGROUND_ID, DEFAULT_FONT_KEY, default_project,
};

pub const DEFAULT_DURATION_SECS: f64 = 10.0;

pub fn spectrum_band_count(project: &Project) -> usize {
    project
        .layers
        .iter()
        .filter_map(|layer| match &layer.kind {
            LayerKind::Spectrum(spectrum) => Some(spectrum.band_count),
            _ => None,
        })
        .max()
        .unwrap_or(1)
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
            fill: Animatable::Const(Fill::Solid(Rgba8::new(64, 220, 255, 255))),
        }),
    }
}

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
