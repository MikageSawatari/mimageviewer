use image::{DynamicImage, GenericImageView};
use std::sync::OnceLock;

const MAX_MEDIAN_SAMPLES: u64 = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchFallback {
    NoPreview,
    ZeroOrInvalidMedian,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MatchDecision {
    Gain {
        gain: f64,
        unclamped: f64,
        clamped: bool,
    },
    Fallback(MatchFallback),
}

pub fn match_gain(preview_median: Option<f64>, developed_median: Option<f64>) -> MatchDecision {
    let Some(preview) = preview_median else {
        return MatchDecision::Fallback(MatchFallback::NoPreview);
    };
    let Some(developed) = developed_median else {
        return MatchDecision::Fallback(MatchFallback::ZeroOrInvalidMedian);
    };
    if !preview.is_finite() || !developed.is_finite() || preview <= 0.0 || developed <= 0.0 {
        return MatchDecision::Fallback(MatchFallback::ZeroOrInvalidMedian);
    }
    let unclamped = preview / developed;
    if !unclamped.is_finite() {
        return MatchDecision::Fallback(MatchFallback::ZeroOrInvalidMedian);
    }
    let gain = unclamped.clamp(0.125, 8.0);
    MatchDecision::Gain {
        gain,
        unclamped,
        clamped: gain != unclamped,
    }
}

/// Median of up to 100,000 evenly spaced pixels in linear sRGB luminance.
pub fn median_linear_luma(image: &DynamicImage) -> Option<f64> {
    let (width, height) = image.dimensions();
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 {
        return None;
    }
    let stride = (pixels / MAX_MEDIAN_SAMPLES).max(1);
    let lut = SRGB_LINEAR.get_or_init(|| {
        std::array::from_fn(|value| {
            let srgb = value as f64 / 255.0;
            if srgb <= 0.04045 {
                srgb / 12.92
            } else {
                ((srgb + 0.055) / 1.055).powf(2.4)
            }
        })
    });
    let mut values = Vec::with_capacity(pixels.div_ceil(stride) as usize);
    for index in (0..pixels).step_by(stride as usize) {
        let pixel = image.get_pixel(
            (index % u64::from(width)) as u32,
            (index / u64::from(width)) as u32,
        );
        values.push(
            0.2126 * lut[pixel[0] as usize]
                + 0.7152 * lut[pixel[1] as usize]
                + 0.0722 * lut[pixel[2] as usize],
        );
    }
    values.sort_unstable_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}

static SRGB_LINEAR: OnceLock<[f64; 256]> = OnceLock::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_median_uses_rec709_weights_after_srgb_decode() {
        let image = DynamicImage::ImageRgb8(
            image::RgbImage::from_raw(3, 1, vec![0, 0, 0, 128, 128, 128, 255, 255, 255]).unwrap(),
        );
        let expected = ((128.0_f64 / 255.0 + 0.055) / 1.055).powf(2.4);
        assert!((median_linear_luma(&image).unwrap() - expected).abs() < 1e-12);
    }

    #[test]
    fn gain_and_clamps_are_in_linear_light() {
        assert_eq!(
            match_gain(Some(0.4), Some(0.2)),
            MatchDecision::Gain {
                gain: 2.0,
                unclamped: 2.0,
                clamped: false,
            }
        );
        assert!(matches!(
            match_gain(Some(0.9), Some(0.01)),
            MatchDecision::Gain {
                gain: 8.0,
                clamped: true,
                ..
            }
        ));
        assert!(matches!(
            match_gain(Some(0.01), Some(0.9)),
            MatchDecision::Gain {
                gain: 0.125,
                clamped: true,
                ..
            }
        ));
    }

    #[test]
    fn absent_zero_and_invalid_medians_fall_back() {
        assert_eq!(
            match_gain(None, Some(0.1)),
            MatchDecision::Fallback(MatchFallback::NoPreview)
        );
        for (preview, developed) in [
            (Some(0.0), Some(0.1)),
            (Some(0.1), Some(0.0)),
            (Some(f64::NAN), Some(0.1)),
            (Some(0.1), None),
        ] {
            assert_eq!(
                match_gain(preview, developed),
                MatchDecision::Fallback(MatchFallback::ZeroOrInvalidMedian)
            );
        }
    }
}
