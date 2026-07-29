use std::path::{Path, PathBuf};

use fast_image_resize::{FilterType, ResizeAlg, ResizeOptions, Resizer};
use image::{DynamicImage, RgbImage, RgbaImage};
use sha2::{Digest, Sha256};

pub const SUPPORTED_IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "bmp", "gif", "heic", "heif", "avif", "jxl", "tiff", "tif",
    "dng", "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef", "ptx",
    "rwl", "iiq",
];
pub const SUPPORTED_VIDEO_EXTENSIONS: &[&str] = &["mpg", "mpeg", "mp4", "avi", "mov", "mkv", "wmv"];
pub const SUPPORTED_AUDIO_EXTENSIONS: &[&str] =
    &["mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "wma"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageProbe {
    pub raw_width: u32,
    pub raw_height: u32,
    pub orientation: u16,
}

impl ImageProbe {
    pub fn oriented_dimensions(self) -> (u32, u32) {
        if matches!(self.orientation, 5..=8) {
            (self.raw_height, self.raw_width)
        } else {
            (self.raw_width, self.raw_height)
        }
    }
}

/// Keep the catalog v2 location convention identical to
/// `mimageviewer::catalog::db_path_for` without linking the GUI/native runtime.
pub fn catalog_db_path(cache_dir: &Path, folder_path: &Path) -> PathBuf {
    let raw = folder_path.to_string_lossy();
    let normalized = if folder_path.parent().is_none() {
        raw.to_lowercase().replace('\\', "/")
    } else {
        let no_drive = if raw.len() >= 2 && raw.chars().nth(1) == Some(':') {
            &raw[2..]
        } else {
            &raw
        };
        no_drive.to_lowercase().replace('\\', "/")
    };
    let hash = format!("{:x}", Sha256::digest(normalized.as_bytes()));
    cache_dir.join(&hash[..2]).join(format!("{hash}.db"))
}

pub fn decode_oriented(path: &Path) -> Option<DynamicImage> {
    let image = image::open(path)
        .ok()
        .or_else(|| wic_decode_to_dynamic_image(path))?;
    Some(apply_orientation(image, read_orientation(path)))
}

pub fn probe_image(path: &Path) -> Option<ImageProbe> {
    let (raw_width, raw_height) = image::image_dimensions(path)
        .ok()
        .or_else(|| wic_image_dimensions(path))?;
    if raw_width == 0 || raw_height == 0 {
        return None;
    }
    Some(ImageProbe {
        raw_width,
        raw_height,
        orientation: read_orientation(path),
    })
}

pub fn passthrough_content_type(
    path: &Path,
    probe: ImageProbe,
    requested_width: u32,
) -> Option<&'static str> {
    if probe.orientation != 1 || requested_width < probe.raw_width {
        return None;
    }
    match path
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "bmp" => Some("image/bmp"),
        "avif" => Some("image/avif"),
        _ => None,
    }
}

pub fn resize_exact(source: &DynamicImage, width: u32, height: u32) -> DynamicImage {
    match source {
        DynamicImage::ImageRgba8(buffer) => {
            DynamicImage::ImageRgba8(resize_rgba(buffer, width, height))
        }
        DynamicImage::ImageRgb8(buffer) => {
            DynamicImage::ImageRgb8(resize_rgb(buffer, width, height))
        }
        _ => DynamicImage::ImageRgba8(resize_rgba(&source.to_rgba8(), width, height)),
    }
}

fn resize_rgba(source: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    let mut destination = RgbaImage::new(width.max(1), height.max(1));
    let mut resizer = Resizer::new();
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    resizer
        .resize(source, &mut destination, &options)
        .expect("matching RGBA8 resize buffers");
    destination
}

fn resize_rgb(source: &RgbImage, width: u32, height: u32) -> RgbImage {
    let mut destination = RgbImage::new(width.max(1), height.max(1));
    let mut resizer = Resizer::new();
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    resizer
        .resize(source, &mut destination, &options)
        .expect("matching RGB8 resize buffers");
    destination
}

fn read_orientation(path: &Path) -> u16 {
    read_rexif_orientation(path)
        .or_else(|| wic_read_orientation(path))
        .unwrap_or(1)
}

fn read_rexif_orientation(path: &Path) -> Option<u16> {
    let exif = rexif::parse_file(path.to_str()?).ok()?;
    let entry = exif.entries.iter().find(|entry| entry.ifd.tag == 274)?;
    entry
        .value
        .to_i64(0)
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| (1..=8).contains(value))
        .or_else(|| orientation_from_text(&entry.value_more_readable))
}

fn orientation_from_text(text: &str) -> Option<u16> {
    let text = text.to_lowercase();
    if text.contains("straight") || text.contains("normal") {
        Some(1)
    } else if text.contains("rotated to left") || text.contains("90 cw") {
        Some(6)
    } else if text.contains("upside down") || text.contains("180") {
        Some(3)
    } else if text.contains("rotated to right")
        || text.contains("270 cw")
        || text.contains("90 ccw")
    {
        Some(8)
    } else if text.contains("mirrored horizontally") {
        Some(2)
    } else if text.contains("mirrored vertically") {
        Some(4)
    } else {
        None
    }
}

fn apply_orientation(image: DynamicImage, orientation: u16) -> DynamicImage {
    match orientation {
        2 => image.fliph(),
        3 => image.rotate180(),
        4 => image.flipv(),
        5 => image.rotate90().fliph(),
        6 => image.rotate90(),
        7 => image.rotate90().flipv(),
        8 => image.rotate270(),
        _ => image,
    }
}

#[cfg(not(windows))]
fn wic_image_dimensions(_path: &Path) -> Option<(u32, u32)> {
    None
}

#[cfg(windows)]
fn wic_image_dimensions(path: &Path) -> Option<(u32, u32)> {
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, IWICImagingFactory, WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::core::{GUID, PCWSTR};

    let _com = ComScope::init();
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let wide: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let decoder = factory
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                Some(&GUID::zeroed()),
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let mut width = 0;
        let mut height = 0;
        frame.GetSize(&mut width, &mut height).ok()?;
        (width > 0 && height > 0).then_some((width, height))
    }
}

#[cfg(not(windows))]
fn wic_decode_to_dynamic_image(_path: &Path) -> Option<DynamicImage> {
    None
}

#[cfg(windows)]
fn wic_decode_to_dynamic_image(path: &Path) -> Option<DynamicImage> {
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICBitmapSource,
        IWICFormatConverter, IWICImagingFactory, WICBitmapDitherTypeNone,
        WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::core::{GUID, Interface, PCWSTR};

    let _com = ComScope::init();
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let wide: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let decoder = factory
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                Some(&GUID::zeroed()),
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let converter: IWICFormatConverter = factory.CreateFormatConverter().ok()?;
        let source: IWICBitmapSource = frame.cast().ok()?;
        converter
            .Initialize(
                &source,
                &GUID_WICPixelFormat32bppBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .ok()?;
        let mut width = 0;
        let mut height = 0;
        converter.GetSize(&mut width, &mut height).ok()?;
        if width == 0 || height == 0 || width > 32768 || height > 32768 {
            return None;
        }
        let stride = width.checked_mul(4)?;
        let mut pixels = vec![0_u8; (stride as usize).checked_mul(height as usize)?];
        converter
            .CopyPixels(std::ptr::null(), stride, &mut pixels)
            .ok()?;
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        Some(DynamicImage::ImageRgba8(RgbaImage::from_raw(
            width, height, pixels,
        )?))
    }
}

#[cfg(not(windows))]
fn wic_read_orientation(_path: &Path) -> Option<u16> {
    None
}

#[cfg(windows)]
fn wic_read_orientation(path: &Path) -> Option<u16> {
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, IWICImagingFactory, IWICMetadataQueryReader,
        WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::core::{GUID, PCWSTR};

    let _com = ComScope::init();
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let wide: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let decoder = factory
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                Some(&GUID::zeroed()),
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .ok()?;
        let reader: IWICMetadataQueryReader =
            decoder.GetFrame(0).ok()?.GetMetadataQueryReader().ok()?;
        for query in [
            "/app1/ifd/{ushort=274}",
            "/ifd/{ushort=274}",
            "System.Photo.Orientation",
        ] {
            let wide: Vec<u16> = query.encode_utf16().chain(std::iter::once(0)).collect();
            let mut value = PROPVARIANT::default();
            if reader
                .GetMetadataByName(PCWSTR(wide.as_ptr()), &mut value)
                .is_ok()
            {
                if let Ok(value) = <u16>::try_from(&value) {
                    return Some(value);
                }
                if let Ok(value) = <u32>::try_from(&value) {
                    return u16::try_from(value).ok();
                }
                if let Ok(value) = <i32>::try_from(&value) {
                    return u16::try_from(value).ok();
                }
            }
        }
        None
    }
}

#[cfg(windows)]
struct ComScope {
    needs_uninit: bool,
}

#[cfg(windows)]
impl ComScope {
    fn init() -> Self {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self {
            needs_uninit: result.is_ok(),
        }
    }
}

#[cfg(windows)]
impl Drop for ComScope {
    fn drop(&mut self) {
        if self.needs_uninit {
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_path_uses_two_level_sha256_layout() {
        let cache = Path::new("cache");
        let path = catalog_db_path(cache, Path::new(r"C:\Photos\Summer"));
        assert_eq!(
            path,
            cache
                .join("11")
                .join("115509654a9d88d89064533af04ffe209b0635a1f9865ca8e670734eaa3c5586.db")
        );
    }

    #[test]
    fn passthrough_boundary_requires_full_width_identity_and_browser_format() {
        let identity = ImageProbe {
            raw_width: 1200,
            raw_height: 800,
            orientation: 1,
        };
        assert_eq!(
            passthrough_content_type(Path::new("page.jpg"), identity, 1200),
            Some("image/jpeg")
        );
        assert_eq!(
            passthrough_content_type(Path::new("page.jpg"), identity, 1199),
            None
        );
        assert_eq!(
            passthrough_content_type(
                Path::new("page.jpg"),
                ImageProbe {
                    orientation: 6,
                    ..identity
                },
                1200
            ),
            None
        );
        assert_eq!(
            passthrough_content_type(Path::new("page.heic"), identity, 1200),
            None
        );
        assert_eq!(
            passthrough_content_type(Path::new("page.dng"), identity, 1200),
            None
        );
    }
}
