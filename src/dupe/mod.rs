//! Duplicate-image signature primitives.
//!
//! This module is deliberately independent of application state, UI, I/O, and
//! persistence. Callers supply decoded, EXIF-oriented pixels to [`proxy`].

pub mod blockhash;
pub mod dct_phash;
pub mod luma;
pub mod pdq;
pub mod proxy;

pub use luma::LumaMetrics;
pub use proxy::{PROXY_VERSION, Proxy};

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Algo {
    Pdq256,
    Pdq64,
    Phash63,
    Blockhash256,
    Luma32,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Sig {
    /// Packed bits in most-significant-bit-first byte order.
    Bits(Box<[u8]>),
    Luma(Box<[u8]>),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Signature {
    pub algo: Algo,
    pub sig: Sig,
    pub quality: u8,
}

pub fn compute(algo: Algo, proxy: &Proxy) -> Signature {
    let (sig, quality) = match algo {
        Algo::Pdq256 => {
            let hashes = pdq::compute(proxy);
            (Sig::Bits(hashes.full), hashes.quality)
        }
        Algo::Pdq64 => {
            let hashes = pdq::compute(proxy);
            (Sig::Bits(hashes.subset), hashes.quality)
        }
        Algo::Phash63 => {
            let (hash, quality) = dct_phash::compute(proxy);
            (Sig::Bits(Box::new(hash.to_be_bytes())), quality)
        }
        Algo::Blockhash256 => (
            Sig::Bits(blockhash::compute(proxy)),
            pdq::quality(&proxy.gray64),
        ),
        Algo::Luma32 => (
            Sig::Luma(luma::signature(proxy)),
            pdq::quality(&proxy.gray64),
        ),
    };
    Signature { algo, sig, quality }
}

pub fn all_algos() -> &'static [Algo] {
    const ALL: &[Algo] = &[
        Algo::Pdq256,
        Algo::Pdq64,
        Algo::Phash63,
        Algo::Blockhash256,
        Algo::Luma32,
    ];
    ALL
}

pub fn hamming(a: &Sig, b: &Sig) -> Option<u32> {
    let (Sig::Bits(a), Sig::Bits(b)) = (a, b) else {
        return None;
    };
    if a.len() != b.len() {
        return None;
    }
    Some(
        a.iter()
            .zip(b.iter())
            .map(|(left, right)| (left ^ right).count_ones())
            .sum(),
    )
}

pub fn luma_metrics(
    a: &Sig,
    b: &Sig,
    a_dims: (u32, u32),
    b_dims: (u32, u32),
    large_diff_threshold: u8,
) -> Option<LumaMetrics> {
    luma::metrics(a, b, a_dims, b_dims, large_diff_threshold)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_rgba(side: usize, unrelated: bool) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(side * side * 4);
        for y in 0..side {
            for x in 0..side {
                let value = if unrelated {
                    ((x * 11) ^ (y * 23) ^ ((x / 7 + y / 5) * 61)) as u8
                } else {
                    (x * 3 + y * 5 + ((x / 16 + y / 16) % 2) * 80) as u8
                };
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }
        rgba
    }

    fn half_size_rgba(source: &[u8], side: usize) -> Vec<u8> {
        let output_side = side / 2;
        let mut output = Vec::with_capacity(output_side * output_side * 4);
        for y in 0..output_side {
            for x in 0..output_side {
                let mut sum = 0u16;
                for dy in 0..2 {
                    for dx in 0..2 {
                        sum += source[((y * 2 + dy) * side + x * 2 + dx) * 4] as u16;
                    }
                }
                let value = (sum as f32 / 4.0).round() as u8;
                output.extend_from_slice(&[value, value, value, 255]);
            }
        }
        output
    }

    #[test]
    fn every_signature_is_deterministic() {
        let rgba = synthetic_rgba(96, false);
        let first_proxy = proxy::build(&rgba, 96, 96);
        let second_proxy = proxy::build(&rgba, 96, 96);
        assert_eq!(first_proxy, second_proxy);
        for &algo in all_algos() {
            assert_eq!(compute(algo, &first_proxy), compute(algo, &second_proxy));
        }
    }

    #[test]
    fn signature_bit_widths_are_explicit() {
        let proxy = proxy::build(&synthetic_rgba(64, false), 64, 64);
        let bits = |algo| match compute(algo, &proxy).sig {
            Sig::Bits(bits) => bits,
            Sig::Luma(_) => panic!("expected bit signature"),
        };

        assert_eq!(bits(Algo::Pdq256).len() * 8, 256);
        assert_eq!(bits(Algo::Pdq64).len() * 8, 64);
        let phash = bits(Algo::Phash63);
        assert_eq!(dct_phash::PHASH_BITS, 63);
        assert_eq!(phash.len(), 8);
        assert_eq!(phash[0] & 0x80, 0);
        assert_eq!(bits(Algo::Blockhash256).len() * 8, 256);
        match compute(Algo::Luma32, &proxy).sig {
            Sig::Luma(luma) => assert_eq!(luma.len(), 32 * 32),
            Sig::Bits(_) => panic!("expected luma signature"),
        }
    }

    #[test]
    fn half_scale_is_closer_than_an_unrelated_image_for_every_algorithm() {
        let original_rgba = synthetic_rgba(128, false);
        let half_rgba = half_size_rgba(&original_rgba, 128);
        let unrelated_rgba = synthetic_rgba(128, true);
        let original = proxy::build(&original_rgba, 128, 128);
        let half = proxy::build(&half_rgba, 64, 64);
        let unrelated = proxy::build(&unrelated_rgba, 128, 128);

        for &algo in all_algos() {
            let original_sig = compute(algo, &original);
            let half_sig = compute(algo, &half);
            let unrelated_sig = compute(algo, &unrelated);
            if algo == Algo::Luma32 {
                let related =
                    luma_metrics(&original_sig.sig, &half_sig.sig, (128, 128), (64, 64), 0)
                        .unwrap();
                let unrelated = luma_metrics(
                    &original_sig.sig,
                    &unrelated_sig.sig,
                    (128, 128),
                    (128, 128),
                    0,
                )
                .unwrap();
                assert!(related.l1 < unrelated.l1, "{algo:?}");
            } else {
                assert!(
                    hamming(&original_sig.sig, &half_sig.sig).unwrap()
                        < hamming(&original_sig.sig, &unrelated_sig.sig).unwrap(),
                    "{algo:?}"
                );
            }
        }
    }

    #[test]
    fn transparent_background_rgb_does_not_change_any_signature() {
        let side = 32usize;
        let mut first = Vec::with_capacity(side * side * 4);
        let mut second = Vec::with_capacity(side * side * 4);
        for y in 0..side {
            for x in 0..side {
                if (8..24).contains(&x) && (8..24).contains(&y) {
                    let value = (x * 9 + y * 5) as u8;
                    let pixel = [value, 255 - value, value / 2, 255];
                    first.extend_from_slice(&pixel);
                    second.extend_from_slice(&pixel);
                } else {
                    first.extend_from_slice(&[0, 0, 0, 0]);
                    second.extend_from_slice(&[201, 37, 119, 0]);
                }
            }
        }

        let first = proxy::build(&first, side as u32, side as u32);
        let second = proxy::build(&second, side as u32, side as u32);
        for &algo in all_algos() {
            assert_eq!(compute(algo, &first), compute(algo, &second));
        }
    }

    #[test]
    fn mismatched_distance_types_and_bit_widths_return_none() {
        let bits = Sig::Bits(vec![0; 8].into_boxed_slice());
        let other_width = Sig::Bits(vec![0; 32].into_boxed_slice());
        let luma = Sig::Luma(vec![0; 32 * 32].into_boxed_slice());

        assert_eq!(hamming(&bits, &luma), None);
        assert_eq!(hamming(&bits, &other_width), None);
        assert_eq!(luma_metrics(&bits, &luma, (1, 1), (1, 1), 0), None);
    }

    #[test]
    fn all_algos_lists_each_candidate_once() {
        assert_eq!(
            all_algos(),
            &[
                Algo::Pdq256,
                Algo::Pdq64,
                Algo::Phash63,
                Algo::Blockhash256,
                Algo::Luma32,
            ]
        );
    }
}
