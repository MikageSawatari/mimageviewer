use std::sync::Arc;

use comic_core::{FontSet, LoadedFont};
use eframe::egui;
use musicvideo_core::{Assets, ImageAsset, LayerKind, Project};

pub use musicvideo_core::{
    DEFAULT_BACKGROUND_IMAGE_ID as BACKGROUND_ID, DEFAULT_FONT_KEY, default_project,
    new_pitch_keyboard_layer,
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
    musicvideo_ui::install_input_policy(ctx);
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

#[cfg(test)]
pub fn test_context() -> egui::Context {
    let context = egui::Context::default();
    setup_egui(&context, None);
    context
}

#[cfg(test)]
mod input_policy_tests {
    use super::*;

    #[test]
    fn setup_registers_tab_and_ime_before_first_pass_and_preserves_composition_on_reregistration() {
        let context = egui::Context::default();
        setup_egui(&context, None);
        let first_id = egui::Id::new("host-input-first");
        let second_id = egui::Id::new("host-input-second");
        let mut first = "original".to_owned();
        let mut second = String::new();
        let mut frame = |events: Vec<egui::Event>, request_focus: bool| {
            let _ = crate::project::headless_paint_output(context.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let response = ui.add(egui::TextEdit::singleline(&mut first).id(first_id));
                        ui.add(egui::TextEdit::singleline(&mut second).id(second_id));
                        if request_focus {
                            response.request_focus();
                        }
                    });
                },
            ));
            assert_eq!(context.memory(|memory| memory.focused()), Some(first_id));
        };
        frame(vec![], true);
        for modifiers in [egui::Modifiers::NONE, egui::Modifiers::SHIFT] {
            frame(
                vec![egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                false,
            );
        }
        frame(
            vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "unconfirmed".into(),
                    active_range_chars: None,
                }),
            ],
            false,
        );
        setup_egui(&context, None);
        frame(
            vec![
                egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Ime(egui::ImeEvent::Disabled),
            ],
            false,
        );
        assert_eq!(first, "original");
        assert!(second.is_empty());
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

/// Keep paint/platform observations from tests that have no renderer.
#[cfg(test)]
pub(crate) fn headless_paint_output(mut output: egui::FullOutput) -> egui::FullOutput {
    egui::FullOutput {
        textures_delta: std::mem::take(&mut output.textures_delta),
        ..Default::default()
    }
    .drop_without_applying_deltas();
    output
}
