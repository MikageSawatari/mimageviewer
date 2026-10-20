//! Worker-only AVIF first-frame decoder. C owns libavif's ABI; Rust owns allocation
//! admission and the single clap -> irot -> imir display transform.
use std::path::Path;

pub const BUILD_ID: &str = libavif_sys::BUILD_ID;
pub const MAX_DIMENSION: u32 = 32768;
// libavif 1.4.2 rejects larger imageSizeLimit values (read.c).
#[cfg(windows)]
const MAX_PIXELS: u64 = 16384 * 16384;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Read,
    Parse,
    Dimensions,
    Budget,
    Decode,
    Conversion,
}
#[derive(Debug)]
pub struct Error {
    pub stage: Stage,
    pub code: Option<i32>,
    pub message: String,
}
impl Error {
    fn local(stage: Stage, message: impl Into<String>) -> Self {
        Self {
            stage,
            code: None,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "libavif {:?} ({:?}): {}",
            self.stage, self.code, self.message
        )
    }
}
impl std::error::Error for Error {}

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

#[cfg(not(windows))]
fn decode_inner(_: &[u8], _: Workload, _: u32, _: u64) -> Result<image::DynamicImage, Error> {
    Err(Error::local(
        Stage::Parse,
        "libavif is available on Windows only",
    ))
}

#[cfg(windows)]
fn decode_inner(
    bytes: &[u8],
    workload: Workload,
    max_dimension: u32,
    max_alloc: u64,
) -> Result<image::DynamicImage, Error> {
    use libavif_sys::*;
    use std::ffi::{CStr, c_void};
    struct Decoder(*mut c_void);
    impl Drop for Decoder {
        fn drop(&mut self) {
            unsafe { miv_avif_close(self.0) };
        }
    }
    impl Decoder {
        fn error(&self, stage: Stage, code: i32) -> Error {
            let mut text = [0; 512];
            unsafe { miv_avif_error(self.0, code, text.as_mut_ptr(), text.len()) };
            Error {
                stage,
                code: Some(code),
                message: unsafe { CStr::from_ptr(text.as_ptr()) }
                    .to_string_lossy()
                    .into_owned(),
            }
        }
    }
    if max_alloc < 4 {
        return Err(Error::local(
            Stage::Budget,
            "RGBA budget is smaller than one pixel",
        ));
    }
    let pixel_limit = (max_alloc / 4).min(MAX_PIXELS) as u32;
    let decoder = Decoder(unsafe { miv_avif_new(max_dimension, pixel_limit, workload.threads()) });
    if decoder.0.is_null() {
        return Err(Error::local(Stage::Parse, "cannot create decoder"));
    }
    let mut info = Info::default();
    // The shim borrows bytes; decoder is dropped before bytes leaves this scope.
    let code = unsafe { miv_avif_parse(decoder.0, bytes.as_ptr(), bytes.len(), &mut info) };
    if code != 0 {
        let mut error = decoder.error(Stage::Parse, code);
        // These are libavif 1.4.2's three dimension-admission diagnostics,
        // inspected in read.c; ordinary malformed inputs remain Parse failures.
        if error.message.contains("dimensions are too large") {
            error.stage = if max_alloc / 4 < MAX_PIXELS {
                Stage::Budget
            } else {
                Stage::Dimensions
            };
        }
        return Err(error);
    }
    let length = admitted_length(info.width, info.height, max_dimension, max_alloc)?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(length)
        .map_err(|e| Error::local(Stage::Budget, e.to_string()))?;
    pixels.resize(length, 0);
    let mut stage = 0;
    let code = unsafe {
        miv_avif_decode(
            decoder.0,
            pixels.as_mut_ptr(),
            pixels.len(),
            info.width * 4,
            &mut stage,
        )
    };
    if code != 0 {
        return Err(decoder.error(
            if stage == 0 {
                Stage::Decode
            } else {
                Stage::Conversion
            },
            code,
        ));
    }
    let image = image::RgbaImage::from_raw(info.width, info.height, pixels)
        .ok_or_else(|| Error::local(Stage::Conversion, "invalid RGBA output"))?;
    // One allocation at a time beyond the coded raster; cropping in RGB also
    // supports apertures not aligned to subsampled chroma boundaries.
    let mut image = image::DynamicImage::ImageRgba8(image);
    if (info.crop_x, info.crop_y, info.crop_width, info.crop_height)
        != (0, 0, info.width, info.height)
    {
        image = image.crop_imm(info.crop_x, info.crop_y, info.crop_width, info.crop_height);
    }
    // Assign to the same owner so every old raster is dropped immediately after
    // its replacement is ready. Shadowed borrowed transforms retain old buffers
    // until scope exit and can multiply the caller's raster memory footprint.
    match info.rotation {
        1 => image = image.rotate270(),
        2 => image = image.rotate180(),
        3 => image = image.rotate90(),
        _ => {}
    }
    match info.mirror {
        1 => image = image.flipv(),
        2 => image = image.fliph(),
        _ => {}
    }
    Ok(image)
}

#[cfg(windows)]
fn admitted_length(
    width: u32,
    height: u32,
    max_dimension: u32,
    max_alloc: u64,
) -> Result<usize, Error> {
    if width == 0 || height == 0 || width > max_dimension || height > max_dimension {
        return Err(Error::local(
            Stage::Dimensions,
            format!("invalid dimensions {width}x{height}"),
        ));
    }
    let length = u64::from(width) * u64::from(height) * 4;
    if length > max_alloc {
        return Err(Error::local(
            Stage::Budget,
            format!("RGBA {length} exceeds budget {max_alloc}"),
        ));
    }
    usize::try_from(length).map_err(|e| Error::local(Stage::Budget, e.to_string()))
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
        assert_eq!(
            admitted_length(32769, 1, MAX_DIMENSION, u64::MAX)
                .unwrap_err()
                .stage,
            Stage::Dimensions
        );
        assert_eq!(
            admitted_length(64, 64, MAX_DIMENSION, 16383)
                .unwrap_err()
                .stage,
            Stage::Budget
        );
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
