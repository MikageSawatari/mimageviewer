//! Worker-only application wrapper: workload policy, path I/O and diagnostics.
//! The shared libavif-sys safe API owns admission and display transforms.
use std::path::Path;

pub const BUILD_ID: &str = libavif_sys::BUILD_ID;
pub const MAX_DIMENSION: u32 = 32768;
#[derive(Clone, Copy, Debug)]
pub enum Workload {
    Background,
    Display,
}
impl Workload {
    pub const fn threads(self) -> i32 {
        match self {
            Self::Background => 1,
            Self::Display => 4,
        }
    }
}

pub use libavif_sys::{Error, Stage};

pub fn decode_path(path: &Path, workload: Workload) -> Result<image::DynamicImage, Error> {
    decode_path_bounded(path, workload, u64::MAX)
}
pub fn decode_path_bounded(
    path: &Path,
    workload: Workload,
    max_alloc: u64,
) -> Result<image::DynamicImage, Error> {
    let started = std::time::Instant::now();
    let result = std::fs::read(path)
        .map_err(|e| Error::local(Stage::Read, e.to_string()))
        .and_then(|bytes| decode_inner(&bytes, workload, MAX_DIMENSION, max_alloc));
    report(result, workload, started)
}
pub fn decode_bytes(bytes: &[u8], workload: Workload) -> Result<image::DynamicImage, Error> {
    decode_bytes_bounded(bytes, workload, u64::MAX)
}
pub fn decode_bytes_bounded(
    bytes: &[u8],
    workload: Workload,
    max_alloc: u64,
) -> Result<image::DynamicImage, Error> {
    let started = std::time::Instant::now();
    let result = decode_inner(bytes, workload, MAX_DIMENSION, max_alloc);
    report(result, workload, started)
}
fn report(
    result: Result<image::DynamicImage, Error>,
    workload: Workload,
    started: std::time::Instant,
) -> Result<image::DynamicImage, Error> {
    if crate::perf::is_enabled() {
        crate::perf::event(
            "decode",
            "backend",
            None,
            0,
            &[
                ("decoder", "libavif".into()),
                ("ms", (started.elapsed().as_secs_f64() * 1000.0).into()),
                ("threads", workload.threads().into()),
                ("success", result.is_ok().into()),
                (
                    "stage",
                    result
                        .as_ref()
                        .err()
                        .map(|e| format!("{:?}", e.stage))
                        .into(),
                ),
                ("code", result.as_ref().err().and_then(|e| e.code).into()),
                (
                    "error",
                    result.as_ref().err().map(ToString::to_string).into(),
                ),
            ],
        );
    }
    if let Err(error) = &result {
        crate::logger::log(error.to_string());
    }
    result
}

fn decode_inner(
    bytes: &[u8],
    workload: Workload,
    max_dimension: u32,
    max_alloc: u64,
) -> Result<image::DynamicImage, Error> {
    libavif_sys::decode(
        bytes,
        libavif_sys::Limits {
            max_dimension,
            max_alloc,
        },
        workload.threads(),
    )
    .map(image::DynamicImage::ImageRgba8)
}

#[cfg(all(test, windows))]
pub(crate) mod tests {
    use super::*;
    use image::GenericImageView;

    pub(crate) fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/avif")
            .join(format!("{name}.avif"))
    }
    pub(crate) fn assert_quadrants(image: &image::DynamicImage, order: [usize; 4]) {
        let colors = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];
        let (w, h) = image.dimensions();
        for (i, color) in order.into_iter().enumerate() {
            let p = image.get_pixel(
                w * (1 + 2 * (i as u32 % 2)) / 4,
                h * (1 + 2 * (i as u32 / 2)) / 4,
            );
            for (actual, expected) in p.0[..3].iter().zip(colors[color]) {
                assert!(
                    (i32::from(*actual) - expected).abs() <= 16,
                    "quadrant {i}: {p:?}, expected {:?}",
                    colors[color]
                );
            }
        }
    }
    fn check_fixture(name: &str, size: u32, order: [usize; 4]) {
        let path = fixture(name);
        let from_path = decode_path(&path, Workload::Background).unwrap();
        let from_bytes = decode_bytes(&std::fs::read(path).unwrap(), Workload::Display).unwrap();
        assert_eq!(from_path.dimensions(), (size, size));
        assert_eq!(from_path.to_rgba8(), from_bytes.to_rgba8());
        assert_quadrants(&from_path, order);
    }
    macro_rules! quadrant_test {
        ($name:ident, $size:expr, $order:expr) => {
            #[test]
            fn $name() {
                check_fixture(stringify!($name), $size, $order);
            }
        };
    }
    quadrant_test!(quadrant_8bit_420, 64, [0, 1, 2, 3]);
    quadrant_test!(quadrant_10bit_420, 64, [0, 1, 2, 3]);
    quadrant_test!(quadrant_12bit_444, 64, [0, 1, 2, 3]);
    quadrant_test!(quadrant_8bit_422, 64, [0, 1, 2, 3]);
    quadrant_test!(quadrant_full_range_444, 64, [0, 1, 2, 3]);
    quadrant_test!(quadrant_grid_2x2, 128, [0, 1, 2, 3]);
    quadrant_test!(quadrant_irot90cw, 64, [2, 0, 3, 1]);
    quadrant_test!(quadrant_imir_horizontal, 64, [1, 0, 3, 2]);
    quadrant_test!(animated_first_quadrant, 64, [0, 1, 2, 3]);

    #[test]
    fn quadrant_alpha_444() {
        let path = fixture("quadrant_alpha_444");
        let image = decode_path(&path, Workload::Background).unwrap();
        assert_eq!(
            image.to_rgba8(),
            decode_bytes(&std::fs::read(path).unwrap(), Workload::Display)
                .unwrap()
                .to_rgba8()
        );
        assert_eq!(image.dimensions(), (64, 64));
        for y in 0..64 {
            for x in 0..64 {
                assert_eq!(image.get_pixel(x, y)[3], if x < 32 { 0 } else { 255 });
            }
        }
        // Straight alpha keeps the original RGB even in fully transparent pixels.
        assert_quadrants(&image, [0, 1, 2, 3]);
    }
    #[test]
    fn gray_128() {
        let path = fixture("gray_128");
        let image = decode_path(&path, Workload::Background).unwrap();
        assert_eq!(
            image.to_rgba8(),
            decode_bytes(&std::fs::read(path).unwrap(), Workload::Display)
                .unwrap()
                .to_rgba8()
        );
        assert_eq!(image.dimensions(), (64, 64));
        for pixel in image.to_rgba8().pixels() {
            assert_eq!(pixel[3], 255);
            assert!(pixel.0[..3].iter().all(|v| v.abs_diff(128) <= 1));
        }
    }
    #[test]
    fn avif_invalid_inputs_are_typed() {
        for name in ["truncated", "not_avif"] {
            let path = fixture(name);
            let error = decode_path(&path, Workload::Background).unwrap_err();
            assert!(matches!(error.stage, Stage::Parse | Stage::Decode));
            assert!(error.code.is_some());
            let bytes_error =
                decode_bytes(&std::fs::read(path).unwrap(), Workload::Background).unwrap_err();
            assert_eq!(bytes_error.stage, error.stage);
            assert_eq!(bytes_error.code, error.code);
        }
    }
    #[test]
    fn avif_limits_reject_before_decode_and_rgba_allocation() {
        let bytes = std::fs::read(fixture("quadrant_8bit_420")).unwrap();
        assert_eq!(
            decode_inner(&bytes, Workload::Background, 63, u64::MAX)
                .unwrap_err()
                .stage,
            Stage::Dimensions
        );
        assert_eq!(
            decode_bytes_bounded(&bytes, Workload::Background, 64 * 64 * 4 - 1)
                .unwrap_err()
                .stage,
            Stage::Budget
        );
        assert!(decode_bytes_bounded(&bytes, Workload::Background, 64 * 64 * 4).is_ok());
    }
    pub(crate) fn with_clap_and_mirror(valid: bool) -> Vec<u8> {
        let mut bytes = std::fs::read(fixture("quadrant_irot90cw")).unwrap();
        let offset = |name: &[u8]| bytes.windows(4).position(|part| part == name).unwrap() - 4;
        let meta = offset(b"meta");
        let iprp = offset(b"iprp");
        let ipco = offset(b"ipco");
        let ipma = offset(b"ipma");
        let iloc = offset(b"iloc");
        fn add(bytes: &mut [u8], offset: usize, amount: u32) {
            let n = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap());
            bytes[offset..offset + 4].copy_from_slice(&(n + amount).to_be_bytes());
        }
        let mut properties = 40u32.to_be_bytes().to_vec();
        properties.extend_from_slice(b"clap");
        for value in [
            32u32,
            if valid { 1 } else { 0 },
            64,
            1,
            (-16i32) as u32,
            1,
            0,
            1,
        ] {
            properties.extend_from_slice(&value.to_be_bytes());
        }
        properties.extend_from_slice(&9u32.to_be_bytes());
        properties.extend_from_slice(b"imir");
        properties.push(1); // left <-> right, after rotation
        assert_eq!(properties.len(), 49);
        add(&mut bytes, meta, 51);
        add(&mut bytes, iprp, 51);
        add(&mut bytes, ipco, 49);
        add(&mut bytes, ipma, 2);
        // This known single-item fixture uses iloc v0, 32-bit offsets/lengths.
        assert_eq!(&bytes[iloc + 8..iloc + 14], &[0, 0, 0, 0, 0x44, 0]);
        add(&mut bytes, iloc + 22, 51);
        assert_eq!(bytes[ipma + 18], 5);
        bytes[ipma + 18] = 7;
        bytes.splice(ipma + 19..ipma + 24, [1, 2, 0x83, 4, 0x86, 0x85, 0x87]);
        bytes.splice(ipma..ipma, properties);
        bytes
    }
    #[test]
    fn avif_clap_then_rotation_then_mirror_and_invalid_clap_only_ignored() {
        let mut plain = std::fs::read(fixture("quadrant_irot90cw")).unwrap();
        let angle = plain.windows(4).position(|part| part == b"irot").unwrap() + 4;
        plain[angle] = 0;
        let plain = decode_bytes(&plain, Workload::Background).unwrap();
        for valid in [true, false] {
            let image = decode_bytes(&with_clap_and_mirror(valid), Workload::Background).unwrap();
            let cropped = if valid {
                plain.crop_imm(0, 0, 32, 64)
            } else {
                plain.clone()
            };
            let expected = cropped.rotate90().fliph();
            assert_eq!(image.dimensions(), if valid { (64, 32) } else { (64, 64) });
            assert_eq!(image.to_rgba8(), expected.to_rgba8());
        }
    }
}

#[cfg(all(test, windows))]
mod shared_probe_tests {
    use super::*;
    #[test]
    fn avif_display_dimensions_match_transformed_pixels_at_every_probe() {
        let bytes = tests::with_clap_and_mirror(true);
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("page.avif");
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(
            libavif_sys::dimensions(&bytes, libavif_sys::Limits::default(), 1).unwrap(),
            (64, 32)
        );
        assert_eq!(crate::fast_resize::probe_dims(&path), Some([64, 32]));
        assert_eq!(
            crate::fast_resize::probe_dims_from_bytes(&bytes, "AVIF"),
            Some([64, 32])
        );
    }
}
