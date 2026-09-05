use music_core::{KeyboardFrame, MivSpectrumFrame, SPECTRUM_NOTE_MIN_MIDI};

use crate::lyrics::active_lyric_at;
use crate::model::{
    Fill, Fit, Glow, GradientStops, ImageId, LayerKind, ModulationTarget, ModulatorValue,
    MusicColorMode, NormRect, Project, Rgba8, SpectrumShape, TextAlign, TimeFormat,
};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpectrumFrame {
    pub bands: Vec<f32>,
    pub notes: Vec<f32>,
    pub note_min_midi: u8,
    pub keyboard: KeyboardFrame,
    pub miv_spectrum: MivSpectrumFrame,
    pub modulators: Vec<ModulatorValue>,
}

impl SpectrumFrame {
    pub fn new(bands: Vec<f32>) -> Self {
        Self {
            bands,
            notes: Vec::new(),
            note_min_midi: SPECTRUM_NOTE_MIN_MIDI,
            keyboard: KeyboardFrame::default(),
            miv_spectrum: MivSpectrumFrame::default(),
            modulators: Vec::new(),
        }
    }

    pub fn from_analysis(
        bands: Vec<f32>,
        notes: Vec<f32>,
        note_min_midi: u8,
        keyboard: KeyboardFrame,
    ) -> Self {
        Self {
            bands,
            notes,
            note_min_midi,
            keyboard,
            miv_spectrum: MivSpectrumFrame::default(),
            modulators: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub items: Vec<SceneItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneItem {
    pub rect: NormRect,
    pub opacity: f32,
    pub rotation_deg: f32,
    pub corner_radius_px: f32,
    pub glow: Option<Glow>,
    pub kind: SceneItemKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SceneItemKind {
    Solid {
        fill: Fill,
    },
    Image {
        image: ImageId,
        fit: Fit,
        zoom: f32,
    },
    Spectrum {
        values: Vec<f32>,
        miv_frame: MivSpectrumFrame,
        point_count: usize,
        shape: SpectrumShape,
        color_mode: MusicColorMode,
        miv_brightness: f32,
        fill: Fill,
    },
    PitchKeyboard {
        keyboard: KeyboardFrame,
        background: Fill,
        color_mode: MusicColorMode,
        miv_brightness: f32,
        fill: Fill,
    },
    ProgressBar {
        progress: f32,
        border_thickness_px: f32,
        fill: Fill,
        border_fill: Fill,
    },
    TimeText {
        text: String,
        font_key: String,
        size_px: f32,
        align: TextAlign,
        fill: Fill,
    },
    Text {
        text: String,
        font_key: String,
        size_px: f32,
        align: TextAlign,
        fill: Fill,
    },
}

/// Evaluates all animation and time-dependent values without touching assets.
pub fn evaluate(project: &Project, t_secs: f64, spectrum: &SpectrumFrame) -> Scene {
    let duration = if project.duration_secs.is_finite() {
        project.duration_secs.max(0.0)
    } else {
        0.0
    };
    let time = if t_secs.is_finite() {
        t_secs.clamp(0.0, duration)
    } else {
        0.0
    };
    let progress = if duration > 0.0 {
        (time / duration).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let mut items = Vec::with_capacity(project.layers.len());

    for layer in &project.layers {
        if !layer.visible.evaluate(time) {
            continue;
        }
        let mut opacity = layer.opacity.evaluate(time);
        let mut rect = layer.rect.evaluate(time);
        let mut scale = 1.0_f32;
        let mut image_zoom = match &layer.kind {
            LayerKind::Image { zoom, .. } => finite_or(zoom.evaluate(time), 1.0).max(0.01),
            _ => 1.0,
        };
        let mut rotation_deg = finite_or(layer.rotation.evaluate(time), 0.0);
        let mut corner_radius_px = finite_or(layer.corner_radius.evaluate(time), 0.0);
        let mut glow = layer.glow.evaluate(time).map(sanitize_glow);
        let mut fill_brightness = 0.0_f32;
        let mut spectrum_thickness_px = 0.0_f32;
        let mut spectrum_radius_px = 0.0_f32;
        for binding in &layer.modulations {
            let amount = finite_or(binding.amount, 0.0)
                * binding
                    .curve
                    .apply(modulator_value(spectrum, binding.modulator));
            match binding.target {
                ModulationTarget::Opacity => opacity = (opacity + amount).clamp(0.0, 1.0),
                ModulationTarget::Scale => scale = (scale + amount).max(0.0),
                ModulationTarget::ImageZoom => image_zoom = (image_zoom + amount).max(0.01),
                ModulationTarget::Rotation => rotation_deg += amount,
                ModulationTarget::CornerRadius => {
                    corner_radius_px = (corner_radius_px + amount).max(0.0)
                }
                ModulationTarget::GlowRadius => {
                    if let Some(glow) = &mut glow {
                        glow.radius_px = (glow.radius_px + amount).max(0.0);
                    }
                }
                ModulationTarget::GlowAlpha => {
                    if let Some(glow) = &mut glow {
                        let alpha = f32::from(glow.color.a) / 255.0;
                        glow.color.a = ((alpha + amount).clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                }
                ModulationTarget::FillBrightness => {
                    fill_brightness = (fill_brightness + amount).clamp(-1.0, 1.0)
                }
                ModulationTarget::SpectrumThickness => {
                    spectrum_thickness_px += amount;
                }
                ModulationTarget::SpectrumRadius => spectrum_radius_px += amount,
            }
        }
        rect = scale_rect_from_center(rect, scale);
        let active_lyric = match &layer.kind {
            LayerKind::Lyrics(lyrics) => {
                let Some(active) = active_lyric_at(lyrics, time) else {
                    continue;
                };
                if active.text.is_empty() {
                    continue;
                }
                opacity *= active.opacity;
                Some(active)
            }
            _ => None,
        };
        let opacity = opacity.clamp(0.0, 1.0);
        if opacity <= 0.0 {
            continue;
        }
        let kind = match &layer.kind {
            LayerKind::Solid { fill } => SceneItemKind::Solid {
                fill: brighten_fill(fill.evaluate(time), fill_brightness),
            },
            LayerKind::Image { image, fit, .. } => SceneItemKind::Image {
                image: image.clone(),
                fit: *fit,
                zoom: image_zoom,
            },
            LayerKind::Spectrum(layer) => {
                let mut values = vec![0.0; layer.band_count];
                for (target, source) in values.iter_mut().zip(&spectrum.bands) {
                    *target = if source.is_finite() {
                        source.clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                }
                let shape = modulate_spectrum_shape(
                    sanitize_spectrum_shape(layer.shape),
                    spectrum_thickness_px,
                    spectrum_radius_px,
                );
                let mut miv_frame = if shape == SpectrumShape::MivHorizontal {
                    spectrum.miv_spectrum.clone()
                } else {
                    MivSpectrumFrame::default()
                };
                miv_frame.bands.truncate(layer.band_count);
                SceneItemKind::Spectrum {
                    values,
                    miv_frame,
                    point_count: layer.point_count.max(1),
                    shape,
                    color_mode: layer.color_mode,
                    miv_brightness: fill_brightness,
                    fill: brighten_fill(layer.fill.evaluate(time), fill_brightness),
                }
            }
            LayerKind::PitchKeyboard(layer) => SceneItemKind::PitchKeyboard {
                keyboard: spectrum.keyboard.clone(),
                background: brighten_fill(layer.background.evaluate(time), fill_brightness),
                color_mode: layer.color_mode,
                miv_brightness: fill_brightness,
                fill: brighten_fill(layer.fill.evaluate(time), fill_brightness),
            },
            LayerKind::ProgressBar(layer) => SceneItemKind::ProgressBar {
                progress,
                border_thickness_px: layer.border_thickness_px.max(0.0),
                fill: brighten_fill(layer.fill.evaluate(time), fill_brightness),
                border_fill: brighten_fill(layer.border_fill.evaluate(time), fill_brightness),
            },
            LayerKind::TimeText(layer) => SceneItemKind::TimeText {
                text: format_time(layer.format, time, duration),
                font_key: layer.font_key.clone(),
                size_px: layer.size_px.max(1.0),
                align: layer.align,
                fill: brighten_fill(layer.fill.evaluate(time), fill_brightness),
            },
            LayerKind::Text(layer) => SceneItemKind::Text {
                text: layer.text.clone(),
                font_key: layer.font_key.clone(),
                size_px: layer.size_px.max(1.0),
                align: layer.align,
                fill: brighten_fill(layer.fill.evaluate(time), fill_brightness),
            },
            LayerKind::Lyrics(layer) => SceneItemKind::Text {
                text: active_lyric
                    .expect("lyrics without an active line were skipped")
                    .text
                    .to_owned(),
                font_key: layer.font_key.clone(),
                size_px: layer.size_px.max(1.0),
                align: layer.align,
                fill: brighten_fill(layer.fill.evaluate(time), fill_brightness),
            },
        };
        items.push(SceneItem {
            rect,
            opacity,
            rotation_deg,
            corner_radius_px: corner_radius_px.max(0.0),
            glow,
            kind,
        });
    }

    Scene {
        width: project.output.width,
        height: project.output.height,
        items,
    }
}

fn modulator_value(spectrum: &SpectrumFrame, id: crate::model::ModulatorId) -> f32 {
    spectrum
        .modulators
        .iter()
        .find(|value| value.id == id)
        .map_or(0.0, |value| finite_or(value.value, 0.0).clamp(0.0, 1.0))
}

fn scale_rect_from_center(rect: NormRect, scale: f32) -> NormRect {
    let scale = finite_or(scale, 1.0).max(0.0);
    let width = rect.width * scale;
    let height = rect.height * scale;
    NormRect::new(
        rect.x + (rect.width - width) * 0.5,
        rect.y + (rect.height - height) * 0.5,
        width,
        height,
    )
}

fn modulate_spectrum_shape(
    shape: SpectrumShape,
    thickness_delta: f32,
    radius_delta: f32,
) -> SpectrumShape {
    match shape {
        SpectrumShape::Bars {
            gap_ratio,
            min_height_px,
            thickness_px,
        } => SpectrumShape::Bars {
            gap_ratio,
            min_height_px,
            thickness_px: (thickness_px + thickness_delta).max(0.0),
        },
        SpectrumShape::Radial {
            radius_px,
            thickness_px,
            start_angle_deg,
            sweep_deg,
            direction,
        } => SpectrumShape::Radial {
            radius_px: (radius_px + radius_delta).max(0.0),
            thickness_px: (thickness_px + thickness_delta).max(0.0),
            start_angle_deg,
            sweep_deg,
            direction,
        },
        SpectrumShape::MivHorizontal => SpectrumShape::MivHorizontal,
    }
}

fn brighten_fill(fill: Fill, amount: f32) -> Fill {
    if amount == 0.0 {
        return fill;
    }
    match fill {
        Fill::Solid(color) => Fill::Solid(brighten_color(color, amount)),
        Fill::LinearGradient { stops, angle_deg } => Fill::LinearGradient {
            stops: brighten_stops(stops, amount),
            angle_deg,
        },
        Fill::RadialGradient {
            stops,
            center,
            radius,
        } => Fill::RadialGradient {
            stops: brighten_stops(stops, amount),
            center,
            radius,
        },
        Fill::BandGradient { stops } => Fill::BandGradient {
            stops: brighten_stops(stops, amount),
        },
    }
}

fn brighten_stops(stops: GradientStops, amount: f32) -> GradientStops {
    GradientStops::new(
        stops
            .to_vec()
            .into_iter()
            .map(|mut stop| {
                stop.color = brighten_color(stop.color, amount);
                stop
            })
            .collect(),
    )
    .expect("brightness keeps a non-empty valid stop layout")
}

pub(crate) fn brighten_color(color: Rgba8, amount: f32) -> Rgba8 {
    if amount == 0.0 {
        return color;
    }
    let channel =
        |value: u8| ((f32::from(value) / 255.0 + amount).clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgba8::new(
        channel(color.r),
        channel(color.g),
        channel(color.b),
        color.a,
    )
}

fn sanitize_spectrum_shape(shape: SpectrumShape) -> SpectrumShape {
    match shape {
        SpectrumShape::Bars {
            gap_ratio,
            min_height_px,
            thickness_px,
        } => SpectrumShape::Bars {
            gap_ratio: finite_or(gap_ratio, 0.0).clamp(0.0, 1.0),
            min_height_px: finite_or(min_height_px, 0.0).max(0.0),
            thickness_px: finite_or(thickness_px, 0.0).max(0.0),
        },
        SpectrumShape::Radial {
            radius_px,
            thickness_px,
            start_angle_deg,
            sweep_deg,
            direction,
        } => SpectrumShape::Radial {
            radius_px: finite_or(radius_px, 0.0).max(0.0),
            thickness_px: finite_or(thickness_px, 0.0).max(0.0),
            start_angle_deg: finite_or(start_angle_deg, -90.0),
            sweep_deg: finite_or(sweep_deg, 360.0),
            direction,
        },
        SpectrumShape::MivHorizontal => SpectrumShape::MivHorizontal,
    }
}

fn sanitize_glow(glow: Glow) -> Glow {
    Glow {
        color: glow.color,
        radius_px: finite_or(glow.radius_px, 0.0).max(0.0),
        spread_px: finite_or(glow.spread_px, 0.0).max(0.0),
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

pub fn format_time(format: TimeFormat, t_secs: f64, total_secs: f64) -> String {
    let current = format_mm_ss(t_secs);
    match format {
        TimeFormat::MmSs => current,
        TimeFormat::MmSsOfTotal => format!("{current} / {}", format_mm_ss(total_secs)),
    }
}

fn format_mm_ss(seconds: f64) -> String {
    let whole = if seconds.is_finite() {
        seconds.max(0.0).floor() as u64
    } else {
        0
    };
    format!("{:02}:{:02}", whole / 60, whole % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ModulationBinding, ModulationCurve, ModulationTarget, ModulatorId, ModulatorValue,
        default_project,
    };

    #[test]
    fn time_formats_are_stable_at_fractional_seconds() {
        assert_eq!(format_time(TimeFormat::MmSs, 83.99, 296.0), "01:23");
        assert_eq!(
            format_time(TimeFormat::MmSsOfTotal, 83.99, 296.99),
            "01:23 / 04:56"
        );
    }

    #[test]
    fn empty_bindings_leave_the_scene_byte_for_byte_unchanged() {
        let project = default_project(10.0);
        let plain = SpectrumFrame::new(vec![0.4; 96]);
        let mut with_unused_value = plain.clone();
        with_unused_value.modulators.push(ModulatorValue {
            id: ModulatorId(99),
            value: 1.0,
        });
        assert_eq!(
            evaluate(&project, 3.0, &plain),
            evaluate(&project, 3.0, &with_unused_value)
        );
    }

    #[test]
    fn scalar_bindings_apply_to_the_base_value_and_scale_about_center() {
        let mut project = default_project(10.0);
        let layer = &mut project.layers[1];
        layer.opacity = crate::Animatable::Const(0.8);
        layer.rotation = crate::Animatable::Const(10.0);
        layer.modulations = vec![
            ModulationBinding {
                target: ModulationTarget::Opacity,
                modulator: ModulatorId(1),
                amount: -0.6,
                curve: ModulationCurve::Linear,
            },
            ModulationBinding {
                target: ModulationTarget::Scale,
                modulator: ModulatorId(1),
                amount: 0.2,
                curve: ModulationCurve::Linear,
            },
            ModulationBinding {
                target: ModulationTarget::Rotation,
                modulator: ModulatorId(1),
                amount: 20.0,
                curve: ModulationCurve::Linear,
            },
            ModulationBinding {
                target: ModulationTarget::GlowRadius,
                modulator: ModulatorId(1),
                amount: 20.0,
                curve: ModulationCurve::Linear,
            },
            ModulationBinding {
                target: ModulationTarget::SpectrumThickness,
                modulator: ModulatorId(1),
                amount: 6.0,
                curve: ModulationCurve::Linear,
            },
            ModulationBinding {
                target: ModulationTarget::SpectrumRadius,
                modulator: ModulatorId(1),
                amount: 40.0,
                curve: ModulationCurve::Linear,
            },
        ];
        let mut frame = SpectrumFrame::new(vec![0.4; 96]);
        frame.modulators.push(ModulatorValue {
            id: ModulatorId(1),
            value: 0.5,
        });
        let base = project.layers[1].rect.evaluate(3.0);
        let scene = evaluate(&project, 3.0, &frame);
        let item = &scene.items[1];

        assert!((item.opacity - 0.5).abs() < 1.0e-6);
        assert!((item.rotation_deg - 20.0).abs() < 1.0e-6);
        assert!((item.rect.width - base.width * 1.1).abs() < 1.0e-6);
        assert!((item.rect.height - base.height * 1.1).abs() < 1.0e-6);
        assert!((item.rect.x + item.rect.width * 0.5 - (base.x + base.width * 0.5)).abs() < 1.0e-6);
        assert!(
            (item.rect.y + item.rect.height * 0.5 - (base.y + base.height * 0.5)).abs() < 1.0e-6
        );
        assert_eq!(item.glow.unwrap().radius_px, 30.0);
        let SceneItemKind::Spectrum { shape, .. } = item.kind else {
            panic!("expected spectrum")
        };
        let SpectrumShape::Radial {
            radius_px,
            thickness_px,
            ..
        } = shape
        else {
            panic!("expected radial spectrum")
        };
        assert_eq!(radius_px, 400.0);
        assert_eq!(thickness_px, 7.0);
    }

    #[test]
    fn bindings_are_applied_in_declaration_order() {
        let mut project = default_project(10.0);
        let layer = &mut project.layers[2];
        layer.opacity = crate::Animatable::Const(0.8);
        layer.modulations = vec![
            ModulationBinding {
                target: ModulationTarget::Opacity,
                modulator: ModulatorId(1),
                amount: 0.5,
                curve: ModulationCurve::Linear,
            },
            ModulationBinding {
                target: ModulationTarget::Opacity,
                modulator: ModulatorId(1),
                amount: -0.5,
                curve: ModulationCurve::Linear,
            },
        ];
        let mut frame = SpectrumFrame::default();
        frame.modulators.push(ModulatorValue {
            id: ModulatorId(1),
            value: 1.0,
        });

        let forward = evaluate(&project, 1.0, &frame).items[2].opacity;
        project.layers[2].modulations.reverse();
        let reversed = evaluate(&project, 1.0, &frame).items[2].opacity;

        assert_eq!(forward, 0.5);
        assert_eq!(reversed, 0.8);
    }

    #[test]
    fn image_zoom_modulates_the_picture_without_changing_the_layer_rect() {
        let mut project = default_project(10.0);
        let layer = &mut project.layers[0];
        let base_rect = layer.rect.evaluate(0.0);
        let LayerKind::Image { zoom, .. } = &mut layer.kind else {
            panic!("expected image")
        };
        *zoom = crate::Animatable::Const(1.1);
        layer.modulations.push(ModulationBinding {
            target: ModulationTarget::ImageZoom,
            modulator: ModulatorId(1),
            amount: 0.2,
            curve: ModulationCurve::Linear,
        });
        let mut frame = SpectrumFrame::default();
        frame.modulators.push(ModulatorValue {
            id: ModulatorId(1),
            value: 0.5,
        });

        let item = evaluate(&project, 1.0, &frame).items.remove(0);
        assert_eq!(item.rect, base_rect);
        let SceneItemKind::Image { zoom, .. } = item.kind else {
            panic!("expected image")
        };
        assert!((zoom - 1.2).abs() < 1.0e-6);
    }

    #[test]
    fn lyrics_use_the_existing_text_item_and_common_properties() {
        let mut project = default_project(10.0);
        let mut layer = crate::new_lyrics_layer(&project);
        let LayerKind::Lyrics(lyrics) = &mut layer.kind else {
            unreachable!()
        };
        lyrics.lines = vec![
            crate::LyricLine {
                text: "前".to_owned(),
                time_secs: Some(1.0),
            },
            crate::LyricLine {
                text: "日本語の歌詞".to_owned(),
                time_secs: Some(2.0),
            },
        ];
        lyrics.fill = crate::Animatable::Const(Fill::Solid(Rgba8::new(64, 64, 64, 255)));
        layer.opacity = crate::Animatable::Const(0.8);
        layer.rotation = crate::Animatable::Const(12.0);
        layer.glow = crate::Animatable::Const(Some(Glow {
            color: Rgba8::WHITE,
            radius_px: 8.0,
            spread_px: 1.0,
        }));
        layer.modulations = vec![
            ModulationBinding {
                target: ModulationTarget::Scale,
                modulator: ModulatorId(7),
                amount: 0.2,
                curve: ModulationCurve::Linear,
            },
            ModulationBinding {
                target: ModulationTarget::FillBrightness,
                modulator: ModulatorId(7),
                amount: 0.2,
                curve: ModulationCurve::Linear,
            },
        ];
        let base_rect = layer.rect.evaluate(0.0);
        project.layers.push(layer);

        let mut spectrum = SpectrumFrame::default();
        spectrum.modulators.push(ModulatorValue {
            id: ModulatorId(7),
            value: 0.5,
        });
        let scene = evaluate(&project, 2.06, &spectrum);
        assert_eq!(scene, evaluate(&project, 2.06, &spectrum));
        let item = scene.items.last().unwrap();
        assert!((item.opacity - 0.4).abs() < 1.0e-5);
        assert!((item.rect.width - base_rect.width * 1.1).abs() < 1.0e-6);
        assert_eq!(item.rotation_deg, 12.0);
        assert_eq!(item.glow.as_ref().unwrap().radius_px, 8.0);
        let SceneItemKind::Text { text, fill, .. } = &item.kind else {
            panic!("lyrics must share the text rendering path");
        };
        assert_eq!(text, "日本語の歌詞");
        assert_eq!(*fill, Fill::Solid(Rgba8::new(90, 90, 90, 255)));
    }
}
