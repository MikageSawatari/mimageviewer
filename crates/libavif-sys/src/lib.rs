//! Opaque libavif decoder and the small, project-owned shim ABI.
pub const BUILD_ID: &str = env!("MIV_LIBAVIF_BUILD_ID");
#[cfg(windows)]
mod windows {
    use std::ffi::{c_char, c_int, c_void};

    #[repr(C)]
    #[derive(Default)]
    pub struct Info {
        pub width: u32,
        pub height: u32,
        pub depth: u32,
        pub alpha: u32,
        pub crop_x: u32,
        pub crop_y: u32,
        pub crop_width: u32,
        pub crop_height: u32,
        pub rotation: u32,
        pub mirror: u32,
    }

    unsafe extern "C" {
        pub fn miv_avif_new(dimension: u32, pixels: u32, threads: c_int) -> *mut c_void;
        pub fn miv_avif_parse(
            handle: *mut c_void,
            data: *const u8,
            length: usize,
            info: *mut Info,
        ) -> c_int;
        pub fn miv_avif_decode(
            handle: *mut c_void,
            pixels: *mut u8,
            length: usize,
            stride: u32,
            stage: *mut c_int,
        ) -> c_int;
        pub fn miv_avif_error(handle: *mut c_void, code: c_int, out: *mut c_char, length: usize);
        pub fn miv_avif_close(handle: *mut c_void);
        pub fn miv_avif_is_compatible(data: *const u8, length: usize) -> c_int;
    }
}
#[cfg(windows)]
use windows::*;

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
    pub fn local(stage: Stage, message: impl Into<String>) -> Self {
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

/// Display dimensions after the same clap/irot/imir metadata used by decode.
/// Parses and checks admission, without allocating or decoding a pixel raster.
pub fn dimensions(bytes: &[u8], limits: Limits, threads: i32) -> Result<(u32, u32), Error> {
    #[cfg(windows)]
    {
        with_parsed(
            bytes,
            threads,
            limits.max_dimension,
            limits.max_alloc,
            |_, info, _| {
                Ok(if info.rotation % 2 == 1 {
                    (info.crop_height, info.crop_width)
                } else {
                    (info.crop_width, info.crop_height)
                })
            },
        )
    }
    #[cfg(not(windows))]
    {
        let _ = (bytes, limits, threads);
        Err(Error::local(
            Stage::Parse,
            "libavif is available on Windows only",
        ))
    }
}

/// Admission limits apply before the coded RGBA raster is allocated.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_dimension: u32,
    pub max_alloc: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_dimension: 32768,
            max_alloc: u64::MAX,
        }
    }
}
/// Decode the first frame to straight RGBA8, with clap -> irot -> imir applied.
/// The decoder borrows the input only for the duration of this call.
pub fn decode(bytes: &[u8], limits: Limits, threads: i32) -> Result<image::RgbaImage, Error> {
    decode_inner(bytes, threads, limits.max_dimension, limits.max_alloc)
        .map(image::DynamicImage::into_rgba8)
}
/// Recognize AVIF still and sequence major/compatible brands without decoding.
pub fn is_avif(bytes: &[u8]) -> bool {
    #[cfg(windows)]
    {
        unsafe { miv_avif_is_compatible(bytes.as_ptr(), bytes.len()) != 0 }
    }
    #[cfg(not(windows))]
    {
        let _ = bytes;
        false
    }
}

#[cfg(windows)]
const MAX_PIXELS: u64 = 16384 * 16384;
#[cfg(not(windows))]
fn decode_inner(_: &[u8], _: i32, _: u32, _: u64) -> Result<image::DynamicImage, Error> {
    Err(Error::local(
        Stage::Parse,
        "libavif is available on Windows only",
    ))
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::ffi::{CStr, c_void};
    pub(super) struct Decoder(std::ptr::NonNull<c_void>);
    impl Drop for Decoder {
        fn drop(&mut self) {
            unsafe { miv_avif_close(self.0.as_ptr()) };
        }
    }
    impl Decoder {
        fn error(&self, stage: Stage, code: i32) -> Error {
            let mut text = [0; 512];
            unsafe { miv_avif_error(self.0.as_ptr(), code, text.as_mut_ptr(), text.len()) };
            Error {
                stage,
                code: Some(code),
                message: unsafe { CStr::from_ptr(text.as_ptr()) }
                    .to_string_lossy()
                    .into_owned(),
            }
        }
    }
    #[cfg(windows)]
    pub(super) fn with_parsed<T>(
        bytes: &[u8],
        threads: i32,
        max_dimension: u32,
        max_alloc: u64,
        consume: impl FnOnce(&Decoder, Info, usize) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if max_alloc < 4 {
            return Err(Error::local(
                Stage::Budget,
                "RGBA budget is smaller than one pixel",
            ));
        }
        let pixel_limit = (max_alloc / 4).min(MAX_PIXELS) as u32;
        let handle = unsafe { miv_avif_new(max_dimension, pixel_limit, threads.max(1)) };
        let handle = std::ptr::NonNull::new(handle)
            .ok_or_else(|| Error::local(Stage::Parse, "cannot create decoder"))?;
        let decoder = Decoder(handle);
        let mut info = Info::default();
        // The shim borrows bytes; decoder is dropped before bytes leaves this scope.
        let code =
            unsafe { miv_avif_parse(decoder.0.as_ptr(), bytes.as_ptr(), bytes.len(), &mut info) };
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
        consume(&decoder, info, length)
    }
    #[cfg(windows)]
    pub(super) fn decode_inner(
        bytes: &[u8],
        threads: i32,
        max_dimension: u32,
        max_alloc: u64,
    ) -> Result<image::DynamicImage, Error> {
        with_parsed(
            bytes,
            threads,
            max_dimension,
            max_alloc,
            |decoder, info, length| {
                let mut pixels = Vec::new();
                pixels
                    .try_reserve_exact(length)
                    .map_err(|e| Error::local(Stage::Budget, e.to_string()))?;
                pixels.resize(length, 0);
                let mut stage = 0;
                let code = unsafe {
                    miv_avif_decode(
                        decoder.0.as_ptr(),
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
                    image =
                        image.crop_imm(info.crop_x, info.crop_y, info.crop_width, info.crop_height);
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
            },
        )
    }
}
#[cfg(windows)]
use native::{decode_inner, with_parsed};

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
mod tests {
    use super::*;
    #[test]
    fn avif_shim_close_accepts_null_without_destroying_a_decoder() {
        unsafe { miv_avif_close(std::ptr::null_mut()) };
    }
    #[test]
    fn avif_admission_rejects_invalid_dimensions_and_budget() {
        assert_eq!(
            admitted_length(32769, 1, 32768, u64::MAX)
                .unwrap_err()
                .stage,
            Stage::Dimensions
        );
        assert_eq!(
            admitted_length(64, 64, 32768, 16383).unwrap_err().stage,
            Stage::Budget
        );
    }
}
