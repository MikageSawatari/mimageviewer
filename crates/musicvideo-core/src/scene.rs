use music_core::{KeyboardFrame, SPECTRUM_NOTE_MIN_MIDI};

use crate::model::{
    Fill, Fit, Glow, ImageId, LayerKind, NormRect, Project, SpectrumShape, TextAlign, TimeFormat,
};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpectrumFrame {
    pub bands: Vec<f32>,
    pub notes: Vec<f32>,
    pub note_min_midi: u8,
    pub keyboard: KeyboardFrame,
}

impl SpectrumFrame {
    pub fn new(bands: Vec<f32>) -> Self {
        Self {
            bands,
            notes: Vec::new(),
            note_min_midi: SPECTRUM_NOTE_MIN_MIDI,
            keyboard: KeyboardFrame::default(),
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
    },
    Spectrum {
        values: Vec<f32>,
        point_count: usize,
        shape: SpectrumShape,
        fill: Fill,
    },
    PitchKeyboard {
        keyboard: KeyboardFrame,
        background: Fill,
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
        let opacity = layer.opacity.evaluate(time).clamp(0.0, 1.0);
        if opacity <= 0.0 {
            continue;
        }
        let kind = match &layer.kind {
            LayerKind::Solid { fill } => SceneItemKind::Solid {
                fill: fill.evaluate(time),
            },
            LayerKind::Image { image, fit } => SceneItemKind::Image {
                image: image.clone(),
                fit: *fit,
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
                SceneItemKind::Spectrum {
                    values,
                    point_count: layer.point_count.max(1),
                    shape: sanitize_spectrum_shape(layer.shape),
                    fill: layer.fill.evaluate(time),
                }
            }
            LayerKind::PitchKeyboard(layer) => SceneItemKind::PitchKeyboard {
                keyboard: spectrum.keyboard.clone(),
                background: layer.background.evaluate(time),
                fill: layer.fill.evaluate(time),
            },
            LayerKind::ProgressBar(layer) => SceneItemKind::ProgressBar {
                progress,
                border_thickness_px: layer.border_thickness_px.max(0.0),
                fill: layer.fill.evaluate(time),
                border_fill: layer.border_fill.evaluate(time),
            },
            LayerKind::TimeText(layer) => SceneItemKind::TimeText {
                text: format_time(layer.format, time, duration),
                font_key: layer.font_key.clone(),
                size_px: layer.size_px.max(1.0),
                align: layer.align,
                fill: layer.fill.evaluate(time),
            },
        };
        items.push(SceneItem {
            rect: layer.rect.evaluate(time),
            opacity,
            rotation_deg: finite_or(layer.rotation.evaluate(time), 0.0),
            corner_radius_px: finite_or(layer.corner_radius.evaluate(time), 0.0).max(0.0),
            glow: layer.glow.evaluate(time).map(sanitize_glow),
            kind,
        });
    }

    Scene {
        width: project.output.width,
        height: project.output.height,
        items,
    }
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

    #[test]
    fn time_formats_are_stable_at_fractional_seconds() {
        assert_eq!(format_time(TimeFormat::MmSs, 83.99, 296.0), "01:23");
        assert_eq!(
            format_time(TimeFormat::MmSsOfTotal, 83.99, 296.99),
            "01:23 / 04:56"
        );
    }
}
