//! Classical DCT pHash using exactly 63 AC coefficients.

use super::{Proxy, pdq};

pub const PHASH_BITS: usize = 63;

const INPUT_SIDE: usize = 32;
const HASH_SIDE: usize = 8;

/// Computes the classical pHash in the lower 63 bits of a `u64`.
///
/// `proxy.gray32` is derived only from 2x2 averages of `proxy.gray64`; this
/// function performs no independent image preprocessing.
pub(crate) fn compute(proxy: &Proxy) -> (u64, u8) {
    let dct = dct_32_to_8(&proxy.gray32);
    let mut ac = [0.0f64; PHASH_BITS];
    ac.copy_from_slice(&dct[1..]);
    let median = median(&ac);

    let mut hash = 0u64;
    for (bit_index, coefficient) in ac.into_iter().enumerate() {
        if coefficient > median {
            hash |= 1u64 << bit_index;
        }
    }
    debug_assert_eq!(hash >> PHASH_BITS, 0);
    (hash, pdq::quality(&proxy.gray64))
}

fn dct_32_to_8(input: &[u8; INPUT_SIDE * INPUT_SIDE]) -> [f64; HASH_SIDE * HASH_SIDE] {
    let matrix = dct_matrix();
    let mut intermediate = [[0.0f64; INPUT_SIDE]; HASH_SIDE];
    for frequency_y in 0..HASH_SIDE {
        for x in 0..INPUT_SIDE {
            let mut sum = 0.0f64;
            for y in 0..INPUT_SIDE {
                sum += matrix[frequency_y][y] * input[y * INPUT_SIDE + x] as f64;
            }
            intermediate[frequency_y][x] = sum;
        }
    }

    let mut output = [0.0f64; HASH_SIDE * HASH_SIDE];
    for frequency_y in 0..HASH_SIDE {
        for frequency_x in 0..HASH_SIDE {
            let mut sum = 0.0f64;
            for x in 0..INPUT_SIDE {
                sum += intermediate[frequency_y][x] * matrix[frequency_x][x];
            }
            output[frequency_y * HASH_SIDE + frequency_x] = sum;
        }
    }
    output
}

fn dct_matrix() -> [[f64; INPUT_SIDE]; HASH_SIDE] {
    let mut matrix = [[0.0f64; INPUT_SIDE]; HASH_SIDE];
    for (frequency, row) in matrix.iter_mut().enumerate() {
        let scale = if frequency == 0 {
            (1.0 / INPUT_SIDE as f64).sqrt()
        } else {
            (2.0 / INPUT_SIDE as f64).sqrt()
        };
        for (position, value) in row.iter_mut().enumerate() {
            let angle = std::f64::consts::PI * frequency as f64 * (2 * position + 1) as f64
                / (2 * INPUT_SIDE) as f64;
            *value = scale * angle.cos();
        }
    }
    matrix
}

fn median(values: &[f64; PHASH_BITS]) -> f64 {
    let mut sorted = *values;
    sorted.sort_unstable_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterned_proxy() -> Proxy {
        let mut gray64 = [0u8; 64 * 64];
        let mut gray32 = [0u8; INPUT_SIDE * INPUT_SIDE];
        for (index, value) in gray64.iter_mut().enumerate() {
            *value = (index.wrapping_mul(29) ^ (index / 64).wrapping_mul(71)) as u8;
        }
        for (index, value) in gray32.iter_mut().enumerate() {
            *value = (index.wrapping_mul(43) ^ (index / INPUT_SIDE).wrapping_mul(17)) as u8;
        }
        Proxy {
            gray64: Box::new(gray64),
            gray32: Box::new(gray32),
            src_width: 64,
            src_height: 64,
        }
    }

    #[test]
    fn phash_uses_only_the_lower_63_bits() {
        let (hash, _) = compute(&patterned_proxy());
        assert_eq!(PHASH_BITS, 63);
        assert_eq!(hash >> PHASH_BITS, 0);
    }

    #[test]
    fn phash_is_deterministic() {
        let proxy = patterned_proxy();
        assert_eq!(compute(&proxy), compute(&proxy));
    }
}
