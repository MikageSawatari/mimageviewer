use crate::model::{Fit, ImageId, LayerKind, NormRect, Project, Rgba8, TextAlign, TimeFormat};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpectrumFrame {
    pub bands: Vec<f32>,
}

impl SpectrumFrame {
    pub fn new(bands: Vec<f32>) -> Self {
        Self { bands }
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
    pub kind: SceneItemKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SceneItemKind {
    Solid {
        color: Rgba8,
    },
    Image {
        image: ImageId,
        fit: Fit,
    },
    Spectrum {
        values: Vec<f32>,
        gap_ratio: f32,
        min_height_px: f32,
        thickness: f32,
        color: Rgba8,
    },
    ProgressBar {
        progress: f32,
        border_thickness_px: f32,
        color: Rgba8,
        border_color: Rgba8,
    },
    TimeText {
        text: String,
        font_key: String,
        size_px: f32,
        align: TextAlign,
        color: Rgba8,
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
                color: fill.evaluate(time).color(),
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
                    gap_ratio: layer.gap_ratio.clamp(0.0, 1.0),
                    min_height_px: layer.min_height_px.max(0.0),
                    thickness: layer.thickness.max(0.0),
                    color: layer.fill.evaluate(time).color(),
                }
            }
            LayerKind::ProgressBar(layer) => SceneItemKind::ProgressBar {
                progress,
                border_thickness_px: layer.border_thickness_px.max(0.0),
                color: layer.fill.evaluate(time).color(),
                border_color: layer.border_fill.evaluate(time).color(),
            },
            LayerKind::TimeText(layer) => SceneItemKind::TimeText {
                text: format_time(layer.format, time, duration),
                font_key: layer.font_key.clone(),
                size_px: layer.size_px.max(1.0),
                align: layer.align,
                color: layer.fill.evaluate(time).color(),
            },
        };
        items.push(SceneItem {
            rect: layer.rect.evaluate(time),
            opacity,
            kind,
        });
    }

    Scene {
        width: project.output.width,
        height: project.output.height,
        items,
    }
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
