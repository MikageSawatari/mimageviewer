//! Continuous 32x32 luma signature and independent comparison metrics.

use super::{Proxy, Sig};

const SIDE: usize = 32;
const PIXEL_COUNT: usize = SIDE * SIDE;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LumaMetrics {
    pub l1: f32,
    pub l1_gain_offset: f32,
    pub grad_l1: f32,
    pub large_diff_area: f32,
    pub aspect_ratio_delta: f32,
}

pub(crate) fn signature(proxy: &Proxy) -> Box<[u8]> {
    proxy.gray32.to_vec().into_boxed_slice()
}

pub(crate) fn metrics(
    a: &Sig,
    b: &Sig,
    a_dims: (u32, u32),
    b_dims: (u32, u32),
    large_diff_threshold: u8,
) -> Option<LumaMetrics> {
    let (Sig::Luma(a), Sig::Luma(b)) = (a, b) else {
        return None;
    };
    if a.len() != PIXEL_COUNT
        || b.len() != PIXEL_COUNT
        || a_dims.0 == 0
        || a_dims.1 == 0
        || b_dims.0 == 0
        || b_dims.1 == 0
    {
        return None;
    }

    let mut absolute_sum = 0.0f64;
    let mut large_diff_count = 0usize;
    for (&left, &right) in a.iter().zip(b.iter()) {
        let difference = (left as i16 - right as i16).unsigned_abs() as f64;
        absolute_sum += difference;
        if difference > large_diff_threshold as f64 {
            large_diff_count += 1;
        }
    }

    let sample_count = PIXEL_COUNT as f64;
    Some(LumaMetrics {
        l1: (absolute_sum / sample_count) as f32,
        l1_gain_offset: gain_offset_l1(a, b),
        grad_l1: gradient_l1(a, b),
        large_diff_area: (large_diff_count as f64 / sample_count) as f32,
        aspect_ratio_delta: ((a_dims.0 as f64 / a_dims.1 as f64)
            - (b_dims.0 as f64 / b_dims.1 as f64))
            .abs() as f32,
    })
}

fn gain_offset_l1(a: &[u8], b: &[u8]) -> f32 {
    let sample_count = a.len() as f64;
    let mean_a = a.iter().map(|&value| value as f64).sum::<f64>() / sample_count;
    let mean_b = b.iter().map(|&value| value as f64).sum::<f64>() / sample_count;

    let mut covariance = 0.0f64;
    let mut variance = 0.0f64;
    for (&left, &right) in a.iter().zip(b.iter()) {
        let centered_a = left as f64 - mean_a;
        covariance += centered_a * (right as f64 - mean_b);
        variance += centered_a * centered_a;
    }

    // For constant A, every gain gives the same fitted constant after choosing
    // the offset. Gain zero is the minimum-norm least-squares solution.
    let gain = if variance > 0.0 {
        covariance / variance
    } else {
        0.0
    };
    let offset = mean_b - gain * mean_a;
    (a.iter()
        .zip(b.iter())
        .map(|(&left, &right)| (gain * left as f64 + offset - right as f64).abs())
        .sum::<f64>()
        / sample_count) as f32
}

fn gradient_l1(a: &[u8], b: &[u8]) -> f32 {
    let mut sum = 0.0f64;
    for y in 0..SIDE {
        for x in 0..SIDE {
            let a_gradient = gradient_magnitude(a, x, y);
            let b_gradient = gradient_magnitude(b, x, y);
            sum += (a_gradient - b_gradient).abs();
        }
    }
    (sum / PIXEL_COUNT as f64) as f32
}

fn gradient_magnitude(image: &[u8], x: usize, y: usize) -> f64 {
    let center = image[y * SIDE + x] as f64;
    let dx = if x + 1 < SIDE {
        image[y * SIDE + x + 1] as f64 - center
    } else {
        0.0
    };
    let dy = if y + 1 < SIDE {
        image[(y + 1) * SIDE + x] as f64 - center
    } else {
        0.0
    };
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luma(values: [u8; PIXEL_COUNT]) -> Sig {
        Sig::Luma(Box::new(values))
    }

    #[test]
    fn gain_and_offset_fit_reduces_luma_error() {
        let mut original = [0u8; PIXEL_COUNT];
        let mut adjusted = [0u8; PIXEL_COUNT];
        for (index, value) in original.iter_mut().enumerate() {
            *value = 20 + (index % 160) as u8;
            adjusted[index] = (1.05 * *value as f32 + 8.0).round() as u8;
        }

        let metrics = metrics(&luma(original), &luma(adjusted), (640, 480), (640, 480), 0).unwrap();
        assert!(metrics.l1_gain_offset < metrics.l1);
    }

    #[test]
    fn small_logo_changes_area_without_dominating_mean_l1() {
        let original = [100u8; PIXEL_COUNT];
        let mut with_logo = original;
        for y in 28..SIDE {
            for x in 28..SIDE {
                with_logo[y * SIDE + x] = 200;
            }
        }

        let unchanged =
            metrics(&luma(original), &luma(original), (100, 100), (100, 100), 0).unwrap();
        let changed =
            metrics(&luma(original), &luma(with_logo), (100, 100), (100, 100), 0).unwrap();
        let changed_everywhere = metrics(
            &luma(original),
            &luma([200; PIXEL_COUNT]),
            (100, 100),
            (100, 100),
            0,
        )
        .unwrap();

        assert!(changed.l1 < changed_everywhere.l1);
        assert!(changed.large_diff_area > unchanged.large_diff_area);
    }

    #[test]
    fn invalid_signature_shape_or_dimensions_returns_none() {
        let valid = luma([0; PIXEL_COUNT]);
        assert!(metrics(&valid, &Sig::Bits(vec![0].into()), (1, 1), (1, 1), 0).is_none());
        assert!(
            metrics(
                &valid,
                &Sig::Luma(vec![0; PIXEL_COUNT - 1].into()),
                (1, 1),
                (1, 1),
                0,
            )
            .is_none()
        );
        assert!(metrics(&valid, &valid, (1, 0), (1, 1), 0).is_none());
    }
}
