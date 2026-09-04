//! Canonical grayscale proxy shared by every duplicate-signature algorithm.

pub const PROXY_VERSION: u32 = 1;

const GRAY64_SIDE: usize = 64;
const GRAY32_SIDE: usize = 32;

/// Canonical grayscale inputs for duplicate-signature algorithms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proxy {
    pub gray64: Box<[u8; GRAY64_SIDE * GRAY64_SIDE]>,
    pub gray32: Box<[u8; GRAY32_SIDE * GRAY32_SIDE]>,
    pub src_width: u32,
    pub src_height: u32,
}

/// Builds the canonical proxy from EXIF-orientation-applied RGBA8 pixels.
///
/// Pixels are composited over opaque white, converted to Rec.601 luma in the
/// sRGB-encoded domain, and area-averaged into a stretched 64x64 image. The
/// 32x32 proxy is then derived only from 2x2 averages of that 64x64 image.
///
/// # Panics
///
/// Panics when either dimension is zero, their pixel count overflows, or
/// `rgba` is not exactly `width * height * 4` bytes.
pub fn build(rgba: &[u8], width: u32, height: u32) -> Proxy {
    assert!(
        width > 0 && height > 0,
        "proxy input dimensions must be non-zero"
    );
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .expect("proxy input dimensions overflow address space");
    assert_eq!(
        rgba.len(),
        expected_len,
        "proxy RGBA length does not match dimensions"
    );

    let gray64 = resize_rgba_to_gray64(rgba, width as usize, height as usize);
    let gray32 = downsample_gray64_to_gray32(&gray64);
    Proxy {
        gray64,
        gray32,
        src_width: width,
        src_height: height,
    }
}

fn resize_rgba_to_gray64(
    rgba: &[u8],
    src_width: usize,
    src_height: usize,
) -> Box<[u8; GRAY64_SIDE * GRAY64_SIDE]> {
    let mut out = Box::new([0; GRAY64_SIDE * GRAY64_SIDE]);

    for out_y in 0..GRAY64_SIDE {
        let src_y0 = out_y as f32 * src_height as f32 / GRAY64_SIDE as f32;
        let src_y1 = (out_y + 1) as f32 * src_height as f32 / GRAY64_SIDE as f32;
        let first_y = src_y0.floor() as usize;
        let last_y = (src_y1.ceil() as usize).min(src_height);

        for out_x in 0..GRAY64_SIDE {
            let src_x0 = out_x as f32 * src_width as f32 / GRAY64_SIDE as f32;
            let src_x1 = (out_x + 1) as f32 * src_width as f32 / GRAY64_SIDE as f32;
            let first_x = src_x0.floor() as usize;
            let last_x = (src_x1.ceil() as usize).min(src_width);

            let mut weighted_sum = 0.0f32;
            let mut area = 0.0f32;
            for src_y in first_y..last_y {
                let y_overlap =
                    (src_y1.min((src_y + 1) as f32) - src_y0.max(src_y as f32)).max(0.0);
                for src_x in first_x..last_x {
                    let x_overlap =
                        (src_x1.min((src_x + 1) as f32) - src_x0.max(src_x as f32)).max(0.0);
                    let weight = x_overlap * y_overlap;
                    let offset = (src_y * src_width + src_x) * 4;
                    weighted_sum += composited_luma(&rgba[offset..offset + 4]) * weight;
                    area += weight;
                }
            }

            debug_assert!(area > 0.0);
            out[out_y * GRAY64_SIDE + out_x] = round_u8(weighted_sum / area);
        }
    }

    out
}

fn composited_luma(rgba: &[u8]) -> f32 {
    let alpha = rgba[3] as f32 / 255.0;
    let inverse_alpha = 1.0 - alpha;
    let red = rgba[0] as f32 * alpha + 255.0 * inverse_alpha;
    let green = rgba[1] as f32 * alpha + 255.0 * inverse_alpha;
    let blue = rgba[2] as f32 * alpha + 255.0 * inverse_alpha;
    0.299 * red + 0.587 * green + 0.114 * blue
}

fn downsample_gray64_to_gray32(
    gray64: &[u8; GRAY64_SIDE * GRAY64_SIDE],
) -> Box<[u8; GRAY32_SIDE * GRAY32_SIDE]> {
    let mut out = Box::new([0; GRAY32_SIDE * GRAY32_SIDE]);
    for y in 0..GRAY32_SIDE {
        for x in 0..GRAY32_SIDE {
            let top_left = y * 2 * GRAY64_SIDE + x * 2;
            let sum = gray64[top_left] as u16
                + gray64[top_left + 1] as u16
                + gray64[top_left + GRAY64_SIDE] as u16
                + gray64[top_left + GRAY64_SIDE + 1] as u16;
            out[y * GRAY32_SIDE + x] = round_u8(sum as f32 / 4.0);
        }
    }
    out
}

fn round_u8(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_rgba(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        rgba.repeat((width * height) as usize)
    }

    #[test]
    fn proxy_is_deterministic_and_preserves_source_dimensions() {
        let mut rgba = Vec::with_capacity(7 * 5 * 4);
        for y in 0..5u8 {
            for x in 0..7u8 {
                rgba.extend_from_slice(&[
                    x.wrapping_mul(31),
                    y.wrapping_mul(47),
                    x.wrapping_mul(y).wrapping_mul(13),
                    x.wrapping_mul(19).wrapping_add(y.wrapping_mul(23)),
                ]);
            }
        }

        let first = build(&rgba, 7, 5);
        let second = build(&rgba, 7, 5);
        assert_eq!(first, second);
        assert_eq!((first.src_width, first.src_height), (7, 5));
    }

    #[test]
    fn transparent_rgb_is_canonicalized_to_white() {
        let transparent_black = build(&solid_rgba(1, 1, [0, 0, 0, 0]), 1, 1);
        let transparent_color = build(&solid_rgba(1, 1, [12, 99, 201, 0]), 1, 1);
        let opaque_white = build(&solid_rgba(1, 1, [255, 255, 255, 255]), 1, 1);

        assert_eq!(transparent_black, transparent_color);
        assert_eq!(transparent_black.gray64, opaque_white.gray64);
        assert!(transparent_black.gray64.iter().all(|&value| value == 255));
    }

    #[test]
    fn rec601_uses_srgb_encoded_values() {
        let red = build(&solid_rgba(1, 1, [255, 0, 0, 255]), 1, 1);
        let green = build(&solid_rgba(1, 1, [0, 255, 0, 255]), 1, 1);
        let blue = build(&solid_rgba(1, 1, [0, 0, 255, 255]), 1, 1);

        assert!(red.gray64.iter().all(|&value| value == 76));
        assert!(green.gray64.iter().all(|&value| value == 150));
        assert!(blue.gray64.iter().all(|&value| value == 29));
    }

    #[test]
    fn area_average_handles_downscale_and_stretches_axes() {
        let mut rgba = Vec::with_capacity(128 * 64 * 4);
        for _y in 0..64 {
            for x in 0..128 {
                let value = if x % 2 == 0 { 0 } else { 255 };
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }

        let proxy = build(&rgba, 128, 64);
        assert!(proxy.gray64.iter().all(|&value| value == 128));
        assert!(proxy.gray32.iter().all(|&value| value == 128));
    }

    #[test]
    fn area_average_handles_upscale() {
        let rgba = [
            0, 0, 0, 255, 64, 64, 64, 255, 128, 128, 128, 255, 255, 255, 255, 255,
        ];
        let proxy = build(&rgba, 2, 2);

        assert_eq!(proxy.gray64[0], 0);
        assert_eq!(proxy.gray64[31], 0);
        assert_eq!(proxy.gray64[32], 64);
        assert_eq!(proxy.gray64[63], 64);
        assert_eq!(proxy.gray64[63 * 64], 128);
        assert_eq!(proxy.gray64[63 * 64 + 63], 255);
    }

    #[test]
    fn gray32_is_rounded_two_by_two_average_of_gray64() {
        let mut gray64 = [0u8; GRAY64_SIDE * GRAY64_SIDE];
        gray64[0] = 0;
        gray64[1] = 1;
        gray64[GRAY64_SIDE] = 2;
        gray64[GRAY64_SIDE + 1] = 3;

        let gray32 = downsample_gray64_to_gray32(&gray64);
        assert_eq!(gray32[0], 2);
    }

    #[test]
    #[should_panic(expected = "proxy RGBA length does not match dimensions")]
    fn rejects_mismatched_buffer_length() {
        let _ = build(&[0; 3], 1, 1);
    }
}
