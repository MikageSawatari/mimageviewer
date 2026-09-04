//! PDQ-256 and its literal low-frequency 64-bit subset.

use super::Proxy;

const INPUT_SIDE: usize = 64;
const DCT_SIDE: usize = 16;
const SUBSET_SIDE: usize = 8;

pub(crate) struct PdqHashes {
    pub full: Box<[u8; 32]>,
    pub subset: Box<[u8; 8]>,
    pub quality: u8,
}

/// Computes PDQ's 256 DCT bits and the top-left 8x8 literal subset.
pub(crate) fn compute(proxy: &Proxy) -> PdqHashes {
    let coefficients = dct_64_to_16(&proxy.gray64);
    let median = lower_median(&coefficients);
    let mut full = Box::new([0u8; 32]);
    let mut subset = Box::new([0u8; 8]);

    for row in 0..DCT_SIDE {
        for column in 0..DCT_SIDE {
            let full_index = row * DCT_SIDE + column;
            let bit = coefficients[full_index] > median;
            set_bit(&mut full[..], full_index, bit);
            if row < SUBSET_SIDE && column < SUBSET_SIDE {
                let subset_index = row * SUBSET_SIDE + column;
                set_bit(&mut subset[..], subset_index, bit);
            }
        }
    }

    PdqHashes {
        full,
        subset,
        quality: quality(&proxy.gray64),
    }
}

/// Meta PDQ's image-domain quality metric on the canonical 64x64 proxy.
pub(crate) fn quality(gray64: &[u8; INPUT_SIDE * INPUT_SIDE]) -> u8 {
    let mut gradient_sum = 0u32;
    for row in 0..INPUT_SIDE - 1 {
        for column in 0..INPUT_SIDE {
            gradient_sum += quantized_difference(
                gray64[row * INPUT_SIDE + column],
                gray64[(row + 1) * INPUT_SIDE + column],
            );
        }
    }
    for row in 0..INPUT_SIDE {
        for column in 0..INPUT_SIDE - 1 {
            gradient_sum += quantized_difference(
                gray64[row * INPUT_SIDE + column],
                gray64[row * INPUT_SIDE + column + 1],
            );
        }
    }

    (gradient_sum / 90).min(100) as u8
}

fn quantized_difference(a: u8, b: u8) -> u32 {
    (((a as i32 - b as i32) * 100) / 255).unsigned_abs()
}

fn dct_64_to_16(input: &[u8; INPUT_SIDE * INPUT_SIDE]) -> [f32; DCT_SIDE * DCT_SIDE] {
    let matrix = dct_matrix();
    let mut intermediate = [[0.0f32; INPUT_SIDE]; DCT_SIDE];

    for frequency_y in 0..DCT_SIDE {
        for x in 0..INPUT_SIDE {
            let mut sum = 0.0f32;
            for y in 0..INPUT_SIDE {
                sum += matrix[frequency_y][y] * input[y * INPUT_SIDE + x] as f32;
            }
            intermediate[frequency_y][x] = sum;
        }
    }

    let mut output = [0.0f32; DCT_SIDE * DCT_SIDE];
    for frequency_y in 0..DCT_SIDE {
        for frequency_x in 0..DCT_SIDE {
            let mut sum = 0.0f32;
            for x in 0..INPUT_SIDE {
                sum += intermediate[frequency_y][x] * matrix[frequency_x][x];
            }
            output[frequency_y * DCT_SIDE + frequency_x] = sum;
        }
    }
    output
}

fn dct_matrix() -> [[f32; INPUT_SIDE]; DCT_SIDE] {
    let mut matrix = [[0.0f32; INPUT_SIDE]; DCT_SIDE];
    let scale = (2.0f64 / INPUT_SIDE as f64).sqrt();
    for frequency_index in 0..DCT_SIDE {
        let frequency = frequency_index + 1;
        for position in 0..INPUT_SIDE {
            let angle = std::f64::consts::PI * frequency as f64 * (2 * position + 1) as f64
                / (2 * INPUT_SIDE) as f64;
            matrix[frequency_index][position] = (scale * angle.cos()) as f32;
        }
    }
    matrix
}

fn lower_median(values: &[f32; DCT_SIDE * DCT_SIDE]) -> f32 {
    let mut sorted = *values;
    sorted.sort_unstable_by(f32::total_cmp);
    sorted[sorted.len() / 2 - 1]
}

fn set_bit(bytes: &mut [u8], index: usize, value: bool) {
    if value {
        bytes[index / 8] |= 1 << (7 - index % 8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy_with_gray64(gray64: [u8; INPUT_SIDE * INPUT_SIDE]) -> Proxy {
        Proxy {
            gray64: Box::new(gray64),
            gray32: Box::new([0; 32 * 32]),
            src_width: INPUT_SIDE as u32,
            src_height: INPUT_SIDE as u32,
        }
    }

    fn hamming(a: &[u8], b: &[u8]) -> u32 {
        a.iter()
            .zip(b)
            .map(|(left, right)| (left ^ right).count_ones())
            .sum()
    }

    fn bit(bytes: &[u8], index: usize) -> bool {
        bytes[index / 8] & (1 << (7 - index % 8)) != 0
    }

    struct Rng(u64);

    impl Rng {
        fn next_u8(&mut self) -> u8 {
            let mut value = self.0;
            value ^= value << 13;
            value ^= value >> 7;
            value ^= value << 17;
            self.0 = value;
            value as u8
        }
    }

    #[test]
    fn pdq64_is_the_literal_top_left_subset() {
        let mut gray = [0u8; INPUT_SIDE * INPUT_SIDE];
        for (index, value) in gray.iter_mut().enumerate() {
            *value = ((index * 37 + index / INPUT_SIDE * 19) & 0xff) as u8;
        }
        let hashes = compute(&proxy_with_gray64(gray));

        assert_eq!(hashes.full.len() * 8, 256);
        assert_eq!(hashes.subset.len() * 8, 64);
        for row in 0..SUBSET_SIDE {
            for column in 0..SUBSET_SIDE {
                assert_eq!(
                    bit(&hashes.subset[..], row * SUBSET_SIDE + column),
                    bit(&hashes.full[..], row * DCT_SIDE + column)
                );
            }
        }
    }

    #[test]
    fn pdq64_distance_preserves_pdq256_recall_radius() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut implication_cases = 0usize;
        for pair_index in 0..96 {
            let mut left = [0u8; INPUT_SIDE * INPUT_SIDE];
            for value in &mut left {
                *value = rng.next_u8();
            }
            let mut right = left;
            let mutation_count = pair_index % 25;
            for _ in 0..mutation_count {
                let index = rng.next_u8() as usize * 16 + rng.next_u8() as usize % 16;
                right[index] = right[index].wrapping_add(rng.next_u8() / 8);
            }

            let left_hashes = compute(&proxy_with_gray64(left));
            let right_hashes = compute(&proxy_with_gray64(right));
            let distance256 = hamming(&left_hashes.full[..], &right_hashes.full[..]);
            let distance64 = hamming(&left_hashes.subset[..], &right_hashes.subset[..]);
            assert!(distance64 <= distance256);

            for radius in 0..=32 {
                if distance256 <= radius {
                    implication_cases += 1;
                    assert!(distance64 <= radius);
                }
            }
        }
        assert!(implication_cases > 0);
    }

    #[test]
    fn quality_orders_featureless_below_textured_images() {
        let gray = [127u8; INPUT_SIDE * INPUT_SIDE];
        let white = [255u8; INPUT_SIDE * INPUT_SIDE];
        let mut tiny_noise = gray;
        let mut textured = gray;
        for (index, value) in tiny_noise.iter_mut().enumerate() {
            *value = if index % 2 == 0 { 127 } else { 128 };
        }
        for row in 0..INPUT_SIDE {
            for column in 0..INPUT_SIDE {
                textured[row * INPUT_SIDE + column] = if (row / 4 + column / 4) % 2 == 0 {
                    16
                } else {
                    240
                };
            }
        }

        let textured_quality = compute(&proxy_with_gray64(textured)).quality;
        for featureless in [gray, white, tiny_noise] {
            assert!(textured_quality > compute(&proxy_with_gray64(featureless)).quality);
        }
    }

    #[test]
    fn pdq_is_deterministic() {
        let mut gray = [0u8; INPUT_SIDE * INPUT_SIDE];
        for (index, value) in gray.iter_mut().enumerate() {
            *value = index.wrapping_mul(73) as u8;
        }
        let proxy = proxy_with_gray64(gray);
        let first = compute(&proxy);
        let second = compute(&proxy);
        assert_eq!(first.full, second.full);
        assert_eq!(first.subset, second.subset);
        assert_eq!(first.quality, second.quality);
    }
}
