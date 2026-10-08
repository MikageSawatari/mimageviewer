//! blockhash.io-style 256-bit block-median hash.

use super::Proxy;

const PROXY_SIDE: usize = 64;
const BLOCKS_PER_SIDE: usize = 16;
const PIXELS_PER_BLOCK_SIDE: usize = PROXY_SIDE / BLOCKS_PER_SIDE;
const PIXELS_PER_BLOCK: u32 = (PIXELS_PER_BLOCK_SIDE * PIXELS_PER_BLOCK_SIDE) as u32;
const BLOCK_COUNT: usize = BLOCKS_PER_SIDE * BLOCKS_PER_SIDE;
const BAND_COUNT: usize = 4;
const BLOCKS_PER_BAND: usize = BLOCK_COUNT / BAND_COUNT;

pub(crate) fn compute(proxy: &Proxy) -> Box<[u8; 32]> {
    let mut block_values = [0u32; BLOCK_COUNT];
    for block_y in 0..BLOCKS_PER_SIDE {
        for block_x in 0..BLOCKS_PER_SIDE {
            let mut sum = 0u32;
            for inner_y in 0..PIXELS_PER_BLOCK_SIDE {
                for inner_x in 0..PIXELS_PER_BLOCK_SIDE {
                    let x = block_x * PIXELS_PER_BLOCK_SIDE + inner_x;
                    let y = block_y * PIXELS_PER_BLOCK_SIDE + inner_y;
                    sum += proxy.gray64[y * PROXY_SIDE + x] as u32;
                }
            }
            block_values[block_y * BLOCKS_PER_SIDE + block_x] = sum;
        }
    }

    let mut hash = Box::new([0u8; 32]);
    for band in 0..BAND_COUNT {
        let start = band * BLOCKS_PER_BAND;
        let end = start + BLOCKS_PER_BAND;
        let median = integer_median(&block_values[start..end]);
        // The reference resolves values equal to the median toward the side of
        // the luminance midpoint occupied by that median.
        let midpoint = PIXELS_PER_BLOCK * 256 / 2;
        for (offset, &value) in block_values[start..end].iter().enumerate() {
            if value > median || (value == median && median > midpoint) {
                set_bit(&mut hash[..], start + offset);
            }
        }
    }
    hash
}

fn integer_median(values: &[u32]) -> u32 {
    assert!(!values.is_empty());
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    if sorted.len() % 2 == 0 {
        (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2]) / 2
    } else {
        sorted[sorted.len() / 2]
    }
}

fn set_bit(bytes: &mut [u8], index: usize) {
    bytes[index / 8] |= 1 << (7 - index % 8);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy_from_gray(gray64: [u8; PROXY_SIDE * PROXY_SIDE]) -> Proxy {
        Proxy {
            gray64: Box::new(gray64),
            gray32: Box::new([0; 32 * 32]),
            src_width: PROXY_SIDE as u32,
            src_height: PROXY_SIDE as u32,
        }
    }

    #[test]
    fn blockhash_is_256_bits_and_uses_four_band_medians() {
        let mut gray64 = [0u8; PROXY_SIDE * PROXY_SIDE];
        for block_y in 0..BLOCKS_PER_SIDE {
            for block_x in 0..BLOCKS_PER_SIDE {
                let value = ((block_y % 4) * BLOCKS_PER_SIDE + block_x) as u8;
                for inner_y in 0..PIXELS_PER_BLOCK_SIDE {
                    for inner_x in 0..PIXELS_PER_BLOCK_SIDE {
                        let x = block_x * PIXELS_PER_BLOCK_SIDE + inner_x;
                        let y = block_y * PIXELS_PER_BLOCK_SIDE + inner_y;
                        gray64[y * PROXY_SIDE + x] = value;
                    }
                }
            }
        }

        let hash = compute(&proxy_from_gray(gray64));
        assert_eq!(hash.len() * 8, 256);
        assert_eq!(hash.iter().map(|byte| byte.count_ones()).sum::<u32>(), 128);
    }

    #[test]
    fn equal_to_median_follows_the_median_luminance_side() {
        let dark = compute(&proxy_from_gray([0; PROXY_SIDE * PROXY_SIDE]));
        let bright = compute(&proxy_from_gray([255; PROXY_SIDE * PROXY_SIDE]));
        assert!(dark.iter().all(|&byte| byte == 0));
        assert!(bright.iter().all(|&byte| byte == u8::MAX));
    }
}
