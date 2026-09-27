use image::{DynamicImage, RgbImage};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[cfg(windows)]
use std::sync::Mutex;

#[derive(Clone, Copy)]
pub enum RawSource<'a> {
    Path(&'a Path),
    Bytes(&'a [u8]),
}

#[derive(Clone)]
pub enum RawOwnedSource {
    Path(PathBuf),
    Bytes(Arc<[u8]>),
}

impl RawOwnedSource {
    pub fn as_source(&self) -> RawSource<'_> {
        match self {
            Self::Path(path) => RawSource::Path(path),
            Self::Bytes(bytes) => RawSource::Bytes(bytes),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawDevelopSupport {
    Supported,
    Unsupported(RawUnsupportedReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawUnsupportedReason {
    Decoder,
    Platform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawDevelopScale {
    Full,
    Half,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawBrightness {
    Auto001,
    Auto0001,
    None,
}

impl RawBrightness {
    #[cfg(windows)]
    fn code(self) -> i32 {
        match self {
            Self::Auto001 => 0,
            Self::Auto0001 => 1,
            Self::None => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawPreviewFormat {
    Jpeg,
    Bitmap,
}

#[derive(Debug, Clone)]
pub struct RawPreviewInfo {
    pub index: u32,
    pub format: RawPreviewFormat,
    pub recorded_dims: [u32; 2],
    pub dims: [u32; 2],
    pub tflip: Option<u8>,
    pub length: u32,
}

#[derive(Debug, Clone)]
pub struct RawInfo {
    pub developed_dims: [u32; 2],
    pub flip: u8,
    pub previews: Vec<RawPreviewInfo>,
    pub develop_support: RawDevelopSupport,
    pub make_model: Option<String>,
}

pub struct RawPreview {
    pub image: DynamicImage,
    pub info: RawPreviewInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawPreviewUnavailableReason {
    NoSupportedCandidate,
    DecodeFailed,
    OrientationMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawError {
    Io(String),
    Corrupt(String),
    Unsupported(RawUnsupportedReason),
    NoUsablePreview(RawPreviewUnavailableReason),
    OutOfMemory,
    Cancelled,
    TooLarge,
    Internal(i32),
}

impl std::fmt::Display for RawError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for RawError {}

pub(crate) struct RawCancellation {
    pub flag: AtomicBool,
    #[cfg(windows)]
    handle: Mutex<usize>,
}

impl RawCancellation {
    pub(crate) fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
            #[cfg(windows)]
            handle: Mutex::new(0),
        }
    }

    pub(crate) fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
        #[cfg(windows)]
        {
            let handle = *self.handle.lock().unwrap();
            if handle != 0 {
                unsafe { libraw_sys::miv_raw_set_cancel_flag(handle as *mut _) };
            }
        }
    }

    #[cfg(windows)]
    fn bind(&self, handle: *mut std::ffi::c_void) -> CancelBinding<'_> {
        let mut slot = self.handle.lock().unwrap();
        *slot = handle as usize;
        if self.flag.load(Ordering::Acquire) {
            unsafe { libraw_sys::miv_raw_set_cancel_flag(handle) };
        }
        CancelBinding(self)
    }
}

#[cfg(windows)]
struct CancelBinding<'a>(&'a RawCancellation);

#[cfg(windows)]
impl Drop for CancelBinding<'_> {
    fn drop(&mut self) {
        *self.0.handle.lock().unwrap() = 0;
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use libraw_sys as ffi;
    use std::ffi::{CStr, c_char, c_void};
    use std::marker::PhantomData;
    use std::os::windows::ffi::OsStrExt;

    struct Handle<'a> {
        raw: *mut c_void,
        _source: PhantomData<RawSource<'a>>,
    }

    impl<'a> Handle<'a> {
        fn open(source: RawSource<'a>) -> Result<Self, RawError> {
            let raw = unsafe { ffi::miv_raw_new() };
            if raw.is_null() {
                return Err(RawError::OutOfMemory);
            }
            let handle = Self {
                raw,
                _source: PhantomData,
            };
            let code = match source {
                RawSource::Path(path) => {
                    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
                    if wide.contains(&0) {
                        return Err(RawError::Io("RAW path contains NUL".into()));
                    }
                    wide.push(0);
                    unsafe { ffi::miv_raw_open_path(raw, wide.as_ptr()) }
                }
                RawSource::Bytes(bytes) => unsafe {
                    ffi::miv_raw_open_buffer(raw, bytes.as_ptr(), bytes.len())
                },
            };
            if code == 0 {
                Ok(handle)
            } else {
                Err(map_error(code))
            }
        }
    }

    impl Drop for Handle<'_> {
        fn drop(&mut self) {
            unsafe { ffi::miv_raw_close(self.raw) };
        }
    }

    fn map_error(code: i32) -> RawError {
        match code {
            -100007 | -100013 => RawError::OutOfMemory,
            -100009 | -7 => RawError::Io(format!("LibRaw error {code}")),
            -100008 | -2 => RawError::Corrupt(format!("LibRaw error {code}")),
            -100010 => RawError::Cancelled,
            -100012 => RawError::TooLarge,
            -8 => RawError::Unsupported(RawUnsupportedReason::Decoder),
            other => RawError::Internal(other),
        }
    }

    fn text(array: &[c_char; 128]) -> String {
        unsafe { CStr::from_ptr(array.as_ptr()) }
            .to_string_lossy()
            .trim()
            .to_string()
    }

    fn extract(handle: &Handle<'_>, index: u32) -> Result<(ffi::MivRawBuffer, Vec<u8>), RawError> {
        let mut result = ffi::MivRawBuffer {
            data: std::ptr::null_mut(),
            length: 0,
            width: 0,
            height: 0,
            colors: 0,
            format: 0,
        };
        let code = unsafe { ffi::miv_raw_preview_extract(handle.raw, index, &mut result) };
        if code != 0 {
            return Err(map_error(code));
        }
        if result.data.is_null() || result.length == 0 || result.length > 512 * 1024 * 1024 {
            unsafe { ffi::miv_raw_free(result.data) };
            return Err(RawError::Corrupt("Invalid preview buffer".into()));
        }
        let bytes = unsafe { std::slice::from_raw_parts(result.data, result.length).to_vec() };
        unsafe { ffi::miv_raw_free(result.data) };
        Ok((result, bytes))
    }

    pub fn info(source: RawSource<'_>) -> Result<RawInfo, RawError> {
        let handle = Handle::open(source)?;
        let mut data = ffi::MivRawInfo {
            width: 0,
            height: 0,
            flip: 0,
            unsupported: 0,
            make: [0; 128],
            model: [0; 128],
            preview_count: 0,
        };
        let code = unsafe { ffi::miv_raw_info(handle.raw, &mut data) };
        if code != 0 {
            return Err(map_error(code));
        }
        if data.width == 0 || data.height == 0 || data.width > 100_000 || data.height > 100_000 {
            return Err(RawError::Corrupt("Invalid developed dimensions".into()));
        }
        let preview_handle = Handle::open(source)?;
        let mut previews = Vec::new();
        for index in 0..data.preview_count.min(8) {
            let mut meta = ffi::MivRawPreviewInfo {
                format: 0,
                width: 0,
                height: 0,
                tflip: 0xffff,
                length: 0,
            };
            let code = unsafe { ffi::miv_raw_preview_info(preview_handle.raw, index, &mut meta) };
            if code != 0 {
                continue;
            }
            let format = match meta.format {
                1 => RawPreviewFormat::Jpeg,
                2 => RawPreviewFormat::Bitmap,
                _ => continue,
            };
            let dims = match format {
                RawPreviewFormat::Jpeg => extract(&preview_handle, index)
                    .ok()
                    .and_then(|(_, bytes)| turbojpeg::read_header(&bytes).ok())
                    .map(|header| [header.width as u32, header.height as u32])
                    .unwrap_or([0, 0]),
                RawPreviewFormat::Bitmap => extract(&preview_handle, index)
                    .map(|(buffer, _)| [buffer.width, buffer.height])
                    .unwrap_or([0, 0]),
            };
            previews.push(RawPreviewInfo {
                index,
                format,
                recorded_dims: [meta.width, meta.height],
                dims,
                tflip: (meta.tflip <= 7).then_some(meta.tflip as u8),
                length: meta.length,
            });
        }
        let make_model = format!("{} {}", text(&data.make), text(&data.model))
            .trim()
            .to_string();
        Ok(RawInfo {
            developed_dims: [data.width, data.height],
            flip: data.flip as u8,
            previews,
            develop_support: if data.unsupported != 0 {
                RawDevelopSupport::Unsupported(RawUnsupportedReason::Decoder)
            } else {
                RawDevelopSupport::Supported
            },
            make_model: (!make_model.is_empty()).then_some(make_model),
        })
    }

    fn orient(image: DynamicImage, flip: u8) -> Result<DynamicImage, RawError> {
        if flip > 7 {
            return Err(RawError::Corrupt("Invalid RAW orientation".into()));
        }
        if flip == 0 {
            return Ok(image);
        }
        let src = image.into_rgb8();
        let (width, height) = src.dimensions();
        let (out_width, out_height) = if flip & 4 != 0 {
            (height, width)
        } else {
            (width, height)
        };
        let mut out = RgbImage::new(out_width, out_height);
        for y in 0..out_height {
            for x in 0..out_width {
                let (mut row, mut col) = (y, x);
                if flip & 4 != 0 {
                    std::mem::swap(&mut row, &mut col);
                }
                if flip & 2 != 0 {
                    row = height - 1 - row;
                }
                if flip & 1 != 0 {
                    col = width - 1 - col;
                }
                out.put_pixel(x, y, *src.get_pixel(col, row));
            }
        }
        Ok(DynamicImage::ImageRgb8(out))
    }

    fn orientation_mismatch(preview: [u32; 2], developed: [u32; 2]) -> bool {
        let not_square = |[width, height]: [u32; 2]| {
            let long = u64::from(width.max(height));
            let short = u64::from(width.min(height));
            (long - short) * 100 > long * 5
        };
        not_square(preview)
            && not_square(developed)
            && (preview[0] > preview[1]) != (developed[0] > developed[1])
    }

    pub fn preview(source: RawSource<'_>) -> Result<RawPreview, RawError> {
        let raw_info = info(source)?;
        let mut candidates = raw_info.previews;
        let had_candidate = !candidates.is_empty();
        let mut orientation_mismatched = false;
        candidates.sort_by_key(|preview| {
            std::cmp::Reverse(u64::from(preview.dims[0]) * u64::from(preview.dims[1]))
        });
        for meta in candidates {
            if meta.dims == [0, 0] {
                continue;
            }
            let handle = Handle::open(source)?;
            let Ok((buffer, bytes)) = extract(&handle, meta.index) else {
                continue;
            };
            let image = match meta.format {
                RawPreviewFormat::Jpeg => {
                    turbojpeg::decompress(&bytes, turbojpeg::PixelFormat::RGB)
                        .ok()
                        .and_then(|decoded| {
                            RgbImage::from_raw(
                                decoded.width as u32,
                                decoded.height as u32,
                                decoded.pixels,
                            )
                        })
                        .map(DynamicImage::ImageRgb8)
                }
                RawPreviewFormat::Bitmap => {
                    let count = usize::try_from(buffer.width)
                        .ok()
                        .and_then(|w| w.checked_mul(buffer.height as usize))
                        .and_then(|pixels| pixels.checked_mul(buffer.colors as usize));
                    if count != Some(bytes.len()) {
                        None
                    } else if buffer.colors == 3 {
                        RgbImage::from_raw(buffer.width, buffer.height, bytes)
                            .map(DynamicImage::ImageRgb8)
                    } else if buffer.colors == 1 {
                        image::GrayImage::from_raw(buffer.width, buffer.height, bytes)
                            .map(DynamicImage::ImageLuma8)
                    } else {
                        None
                    }
                }
            };
            if let Some(image) = image {
                let flip = meta
                    .tflip
                    .filter(|&flip| flip != 0)
                    .unwrap_or(raw_info.flip);
                let image = orient(image, flip)?;
                if orientation_mismatch([image.width(), image.height()], raw_info.developed_dims) {
                    orientation_mismatched = true;
                    continue;
                }
                return Ok(RawPreview { image, info: meta });
            }
        }
        let reason = if orientation_mismatched {
            RawPreviewUnavailableReason::OrientationMismatch
        } else if had_candidate {
            RawPreviewUnavailableReason::DecodeFailed
        } else {
            RawPreviewUnavailableReason::NoSupportedCandidate
        };
        Err(RawError::NoUsablePreview(reason))
    }

    struct CallbackState<'a> {
        cancel: &'a RawCancellation,
        progress: &'a AtomicU8,
    }

    unsafe extern "C" fn on_progress(
        user: *mut c_void,
        stage: i32,
        iteration: i32,
        expected: i32,
    ) -> i32 {
        let state = unsafe { &*(user as *const CallbackState<'_>) };
        let value = if stage == (1 << 3) {
            if iteration <= 0 { 5 } else { 35 }
        } else if stage == (1 << 11) {
            let fraction = if expected > 0 {
                (iteration.max(0) as f32 / expected as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            35 + (fraction * 50.0) as u8
        } else if stage < (1 << 3) {
            5
        } else if stage < (1 << 11) {
            35
        } else {
            85
        };
        state.progress.fetch_max(value, Ordering::Relaxed);
        state.cancel.flag.load(Ordering::Acquire) as i32
    }

    pub(in crate::raw) fn develop(
        source: RawSource<'_>,
        scale: RawDevelopScale,
        brightness: RawBrightness,
        cancel: &RawCancellation,
        progress: &AtomicU8,
    ) -> Result<DynamicImage, RawError> {
        if cancel.flag.load(Ordering::Acquire) {
            return Err(RawError::Cancelled);
        }
        let handle = Handle::open(source)?;
        let _binding = cancel.bind(handle.raw);
        let callback_state = CallbackState { cancel, progress };
        let code = unsafe {
            ffi::miv_raw_develop(
                handle.raw,
                i32::from(scale == RawDevelopScale::Half),
                brightness.code(),
                Some(on_progress),
                (&callback_state as *const CallbackState<'_>)
                    .cast_mut()
                    .cast(),
            )
        };
        if cancel.flag.load(Ordering::Acquire) {
            return Err(RawError::Cancelled);
        }
        if code != 0 {
            return Err(map_error(code));
        }
        let mut width = 0;
        let mut height = 0;
        let code = unsafe { ffi::miv_raw_image_info(handle.raw, &mut width, &mut height) };
        if code != 0 {
            return Err(map_error(code));
        }
        let length = usize::try_from(width)
            .ok()
            .and_then(|w| w.checked_mul(height as usize))
            .and_then(|pixels| pixels.checked_mul(3))
            .filter(|length| *length <= 2 * 1024 * 1024 * 1024)
            .ok_or(RawError::TooLarge)?;
        let mut rgb = Vec::new();
        rgb.try_reserve_exact(length)
            .map_err(|_| RawError::OutOfMemory)?;
        rgb.resize(length, 0);
        let code =
            unsafe { ffi::miv_raw_copy_rgb(handle.raw, rgb.as_mut_ptr(), width as usize * 3) };
        if cancel.flag.load(Ordering::Acquire) {
            return Err(RawError::Cancelled);
        }
        if code != 0 {
            return Err(map_error(code));
        }
        progress.store(100, Ordering::Release);
        RgbImage::from_raw(width, height, rgb)
            .map(DynamicImage::ImageRgb8)
            .ok_or(RawError::Internal(-1))
    }

    #[cfg(test)]
    #[test]
    fn flip_five_and_six_follow_libraw_flip_index() {
        let source = RgbImage::from_fn(2, 3, |x, y| image::Rgb([(y * 2 + x) as u8, 0, 0]));
        let flipped_five = orient(DynamicImage::ImageRgb8(source.clone()), 5)
            .unwrap()
            .into_rgb8();
        let flipped_six = orient(DynamicImage::ImageRgb8(source), 6)
            .unwrap()
            .into_rgb8();
        assert_eq!(flipped_five.dimensions(), (3, 2));
        assert_eq!(flipped_six.dimensions(), (3, 2));
        assert_eq!(flipped_five.get_pixel(0, 0)[0], 1);
        assert_eq!(flipped_five.get_pixel(2, 0)[0], 5);
        assert_eq!(flipped_six.get_pixel(0, 0)[0], 4);
        assert_eq!(flipped_six.get_pixel(2, 0)[0], 0);
    }

    #[cfg(test)]
    #[test]
    fn orientation_mismatch_rejects_only_clear_landscape_portrait_conflicts() {
        assert!(orientation_mismatch([4928, 3280], [3292, 4940]));
        assert!(!orientation_mismatch([3280, 4928], [3292, 4940]));
        assert!(!orientation_mismatch([1000, 951], [3292, 4940]));
        assert!(!orientation_mismatch([1000, 950], [3292, 4940]));
        assert!(orientation_mismatch([1000, 949], [949, 1000]));
    }
}

#[cfg(windows)]
pub(in crate::raw) use windows::develop;
#[cfg(windows)]
pub use windows::{info, preview};

#[cfg(not(windows))]
pub fn info(_source: RawSource<'_>) -> Result<RawInfo, RawError> {
    Err(RawError::Unsupported(RawUnsupportedReason::Platform))
}

#[cfg(not(windows))]
pub fn preview(_source: RawSource<'_>) -> Result<RawPreview, RawError> {
    Err(RawError::Unsupported(RawUnsupportedReason::Platform))
}

#[cfg(not(windows))]
pub(in crate::raw) fn develop(
    _source: RawSource<'_>,
    _scale: RawDevelopScale,
    _brightness: RawBrightness,
    _cancel: &RawCancellation,
    _progress: &AtomicU8,
) -> Result<DynamicImage, RawError> {
    Err(RawError::Unsupported(RawUnsupportedReason::Platform))
}
