//! Fullscreen と派生処理で共有する canonical 静止画 decoder。
//!
//! 通常ファイル / 検証済み bytes / archive entry を本体 fullscreen と同じ順序で
//! デコードし、EXIF orientation 適用済み・GPU clamp 前の native image を返す。
//! panorama は同じ native image から high-res と 8K base を tee する必要があるため、
//! clamp は [`CanonicalStaticImage::into_gpu_raster`] で明示的に後段適用する。

use std::borrow::Cow;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{Arc, atomic::AtomicBool};

use crate::fs_animation::{
    decode_apng_frames, decode_apng_frames_from_bytes, decode_gif_frames,
    decode_gif_frames_from_bytes, decode_webp_frames, decode_webp_frames_from_bytes,
};

pub const CANONICAL_RASTER_MAX_LONG_EDGE: u32 = 8192;

pub enum CanonicalImageSource<'a> {
    File {
        path: &'a Path,
        verified_bytes: Option<&'a [u8]>,
    },
    ArchiveEntry {
        archive_path: &'a Path,
        entry_name: &'a str,
    },
}

#[derive(Clone, Copy)]
pub struct CanonicalDecodeOptions<'a> {
    pub susie_priority: bool,
    pub susie_cancel: Option<&'a Arc<AtomicBool>>,
    pub animation_policy: AnimationPolicy,
}

impl CanonicalDecodeOptions<'_> {
    pub const fn fullscreen(animation_policy: AnimationPolicy) -> Self {
        Self {
            susie_priority: true,
            susie_cancel: None,
            animation_policy,
        }
    }
}

/// canonical decoder がアニメーション対応形式をどう扱うか。
///
/// prefetch は [`FirstFrameOnly`](Self::FirstFrameOnly) で静止画 fallback だけを通し、
/// 現在ページと remote AI の適用可否判定は [`FullFrames`](Self::FullFrames) を使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationPolicy {
    FullFrames,
    FirstFrameOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalAnimatedFormat {
    Gif,
    Apng,
    WebP,
}

/// static decode が返した画素と、後段のアニメーション昇格との関係。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalStaticAnimation {
    Still,
    /// コンテナヘッダで複数フレームを確認したうえで、policy により第1フレームだけを返した。
    FirstFrameOnly(CanonicalAnimatedFormat),
}

impl CanonicalAnimatedFormat {
    pub const fn perf_name(self) -> &'static str {
        match self {
            Self::Gif => "gif_anim",
            Self::Apng => "png_anim",
            Self::WebP => "webp_anim",
        }
    }

    pub const fn exit_reason(self) -> &'static str {
        self.perf_name()
    }

    pub const fn log_label(self) -> &'static str {
        match self {
            Self::Gif => "anim-gif",
            Self::Apng => "anim-png",
            Self::WebP => "anim-webp",
        }
    }
}

pub struct CanonicalStaticImage {
    /// EXIF orientation 適用済み、GPU clamp 前の native image。
    pub image: image::DynamicImage,
    /// EXIF orientation 適用後、GPU clamp 前の寸法。
    pub source_dims: [usize; 2],
    pub animation: CanonicalStaticAnimation,
}

pub struct CanonicalRaster {
    pub pixels: egui::ColorImage,
    pub source_dims: [usize; 2],
    pub animation: CanonicalStaticAnimation,
}

impl CanonicalStaticImage {
    /// 本体の通常 fullscreen static と同じ 8192 Bilinear clamp と RGBA 変換を行う。
    /// panorama はこの method を呼ぶ前の native image から tee する。
    pub fn into_gpu_raster(self) -> CanonicalRaster {
        let image = clamp_dynamic_for_gpu(self.image);
        CanonicalRaster {
            pixels: dynamic_image_to_color_image(&image),
            source_dims: self.source_dims,
            animation: self.animation,
        }
    }
}

pub enum CanonicalImageDecode {
    Static(CanonicalStaticImage),
    Animated {
        format: CanonicalAnimatedFormat,
        frames: Vec<(egui::ColorImage, f64)>,
    },
}

#[derive(Debug)]
pub enum CanonicalDecodeError {
    SourceRead(std::io::Error),
    Decode(image::ImageError),
}

impl std::fmt::Display for CanonicalDecodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceRead(error) => write!(formatter, "source read failed: {error}"),
            Self::Decode(error) => write!(formatter, "image decode failed: {error}"),
        }
    }
}

impl std::error::Error for CanonicalDecodeError {}

/// WIC / Susie の実装を production から分離し、fallback 順と条件を test で固定する seam。
trait FallbackDecoder {
    fn wic_path(&self, path: &Path) -> Option<image::DynamicImage>;
    fn wic_bytes(&self, bytes: &[u8]) -> Option<image::DynamicImage>;
    fn susie_path(
        &self,
        path: &Path,
        options: CanonicalDecodeOptions<'_>,
    ) -> std::io::Result<image::DynamicImage>;
    fn susie_bytes(
        &self,
        filename_hint: &str,
        bytes: &[u8],
        options: CanonicalDecodeOptions<'_>,
    ) -> std::io::Result<image::DynamicImage>;
}

struct SystemFallbackDecoder;

impl FallbackDecoder for SystemFallbackDecoder {
    fn wic_path(&self, path: &Path) -> Option<image::DynamicImage> {
        crate::wic_decoder::decode_to_dynamic_image(path)
    }

    fn wic_bytes(&self, bytes: &[u8]) -> Option<image::DynamicImage> {
        crate::wic_decoder::decode_to_dynamic_image_from_bytes(bytes)
    }

    fn susie_path(
        &self,
        path: &Path,
        options: CanonicalDecodeOptions<'_>,
    ) -> std::io::Result<image::DynamicImage> {
        crate::susie_loader::decode_file(
            path,
            options.susie_priority,
            options.susie_cancel.cloned(),
        )
    }

    fn susie_bytes(
        &self,
        filename_hint: &str,
        bytes: &[u8],
        options: CanonicalDecodeOptions<'_>,
    ) -> std::io::Result<image::DynamicImage> {
        crate::susie_loader::decode_bytes(
            filename_hint,
            bytes,
            options.susie_priority,
            options.susie_cancel.cloned(),
        )
    }
}

struct ResolvedSource<'a> {
    path: &'a Path,
    bytes: Option<Cow<'a, [u8]>>,
    filename_hint: Cow<'a, str>,
    extension: String,
}

impl<'a> ResolvedSource<'a> {
    fn resolve(source: CanonicalImageSource<'a>) -> Result<Self, CanonicalDecodeError> {
        match source {
            CanonicalImageSource::File {
                path,
                verified_bytes,
            } => Ok(Self {
                path,
                bytes: verified_bytes.map(Cow::Borrowed),
                filename_hint: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(Cow::Borrowed)
                    .unwrap_or_else(|| Cow::Borrowed("")),
                extension: path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase(),
            }),
            CanonicalImageSource::ArchiveEntry {
                archive_path,
                entry_name,
            } => {
                let bytes = crate::zip_loader::read_entry_bytes(archive_path, entry_name)
                    .map_err(CanonicalDecodeError::SourceRead)?;
                let basename = crate::zip_loader::entry_basename(entry_name);
                let extension = basename
                    .rsplit('.')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                Ok(Self {
                    path: archive_path,
                    bytes: Some(Cow::Owned(bytes)),
                    filename_hint: Cow::Borrowed(entry_name),
                    extension,
                })
            }
        }
    }
}

/// 本体 fullscreen と同じ source/animation/fallback/EXIF 規則で native image を読む。
/// GIF / APNG / WebP は file / archive entry の別なく同じ animation policy に従う。
pub fn decode_canonical_image(
    source: CanonicalImageSource<'_>,
    options: CanonicalDecodeOptions<'_>,
) -> Result<CanonicalImageDecode, CanonicalDecodeError> {
    decode_canonical_image_with_fallbacks(source, options, &SystemFallbackDecoder)
}

fn decode_canonical_image_with_fallbacks(
    source: CanonicalImageSource<'_>,
    options: CanonicalDecodeOptions<'_>,
    fallbacks: &impl FallbackDecoder,
) -> Result<CanonicalImageDecode, CanonicalDecodeError> {
    let source = ResolvedSource::resolve(source)?;
    let bytes = source.bytes.as_deref();
    let static_animation = if options.animation_policy == AnimationPolicy::FirstFrameOnly {
        probe_static_animation(&source)
    } else {
        CanonicalStaticAnimation::Still
    };

    if options.animation_policy == AnimationPolicy::FullFrames && source.extension == "gif" {
        let frames = match bytes {
            Some(bytes) => decode_gif_frames_from_bytes(bytes),
            None => decode_gif_frames(source.path),
        };
        if let Some(frames) = frames {
            return Ok(CanonicalImageDecode::Animated {
                format: CanonicalAnimatedFormat::Gif,
                frames,
            });
        }
    }

    if options.animation_policy == AnimationPolicy::FullFrames && source.extension == "png" {
        let frames = match bytes {
            Some(bytes) => decode_apng_frames_from_bytes(bytes),
            None => decode_apng_frames(source.path),
        };
        if let Some(frames) = frames {
            return Ok(CanonicalImageDecode::Animated {
                format: CanonicalAnimatedFormat::Apng,
                frames,
            });
        }
    }

    if options.animation_policy == AnimationPolicy::FullFrames && source.extension == "webp" {
        let frames = match bytes {
            Some(bytes) => decode_webp_frames_from_bytes(bytes),
            None => decode_webp_frames(source.path),
        };
        if let Some(frames) = frames {
            return Ok(CanonicalImageDecode::Animated {
                format: CanonicalAnimatedFormat::WebP,
                frames,
            });
        }
    }

    let image = if let Some(bytes) = bytes {
        match image::load_from_memory(bytes) {
            Ok(image) => Ok(image),
            Err(primary_error) => match fallbacks.wic_bytes(bytes) {
                Some(image) => Ok(image),
                None => fallbacks
                    .susie_bytes(&source.filename_hint, bytes, options)
                    .map_err(|_| primary_error),
            },
        }
    } else {
        match image::open(source.path) {
            Ok(image) => Ok(image),
            Err(primary_error) => match fallbacks.wic_path(source.path) {
                Some(image) => Ok(image),
                None => fallbacks
                    .susie_path(source.path, options)
                    .map_err(|_| primary_error),
            },
        }
    }
    .map_err(CanonicalDecodeError::Decode)?;

    // FullFrames のアニメ decoder は各フレームへ EXIF orientation を適用しない。
    // 昇格前後の第1フレームを同じ画素に保つため、アニメと確認済みの FirstFrameOnly
    // も同じ向きのまま返す。通常静止画だけは従来どおり orientation を適用する。
    let image = match (static_animation, bytes) {
        (CanonicalStaticAnimation::FirstFrameOnly(_), _) => image,
        (CanonicalStaticAnimation::Still, Some(bytes)) => {
            crate::thumb_loader::apply_exif_orientation_from_bytes(image, bytes)
        }
        (CanonicalStaticAnimation::Still, None) => {
            crate::thumb_loader::apply_exif_orientation(image, source.path)
        }
    };
    let source_dims = [image.width() as usize, image.height() as usize];
    Ok(CanonicalImageDecode::Static(CanonicalStaticImage {
        image,
        source_dims,
        animation: static_animation,
    }))
}

/// FirstFrameOnly の static fallback が「本当にアニメの第1フレーム」かを、画素展開せず
/// コンテナ構造だけで判定する。通常の静止 PNG/WebP を現ページ化のたびに再 decode しないため、
/// 拡張子だけを sentinel にしない。
fn probe_static_animation(source: &ResolvedSource<'_>) -> CanonicalStaticAnimation {
    let probe = |reader: &mut dyn Read| match source.extension.as_str() {
        "gif" => gif_has_multiple_frames(reader).then_some(CanonicalAnimatedFormat::Gif),
        "png" => apng_has_multiple_frames(reader).then_some(CanonicalAnimatedFormat::Apng),
        "webp" => webp_has_animation(reader).then_some(CanonicalAnimatedFormat::WebP),
        _ => None,
    };
    let format = match source.bytes.as_deref() {
        Some(bytes) => probe(&mut Cursor::new(bytes)),
        None => std::fs::File::open(source.path)
            .ok()
            .and_then(|mut file| probe(&mut file)),
    };
    format.map_or(
        CanonicalStaticAnimation::Still,
        CanonicalStaticAnimation::FirstFrameOnly,
    )
}

fn read_byte(reader: &mut dyn Read) -> Option<u8> {
    let mut byte = [0_u8; 1];
    reader.read_exact(&mut byte).ok()?;
    Some(byte[0])
}

fn skip_bytes(reader: &mut dyn Read, mut len: usize) -> bool {
    let mut scratch = [0_u8; 4096];
    while len > 0 {
        let chunk = len.min(scratch.len());
        if reader.read_exact(&mut scratch[..chunk]).is_err() {
            return false;
        }
        len -= chunk;
    }
    true
}

fn skip_gif_sub_blocks(reader: &mut dyn Read) -> bool {
    loop {
        let Some(len) = read_byte(reader) else {
            return false;
        };
        if len == 0 {
            return true;
        }
        if !skip_bytes(reader, usize::from(len)) {
            return false;
        }
    }
}

fn gif_has_multiple_frames(reader: &mut dyn Read) -> bool {
    let mut header = [0_u8; 13];
    if reader.read_exact(&mut header).is_err() || !matches!(&header[..6], b"GIF87a" | b"GIF89a") {
        return false;
    }
    if header[10] & 0x80 != 0 {
        let color_table_len = 3_usize << (usize::from(header[10] & 0x07) + 1);
        if !skip_bytes(reader, color_table_len) {
            return false;
        }
    }

    let mut frames = 0_u8;
    loop {
        match read_byte(reader) {
            Some(0x2c) => {
                frames += 1;
                if frames > 1 {
                    return true;
                }
                let mut descriptor = [0_u8; 9];
                if reader.read_exact(&mut descriptor).is_err() {
                    return false;
                }
                if descriptor[8] & 0x80 != 0 {
                    let color_table_len = 3_usize << (usize::from(descriptor[8] & 0x07) + 1);
                    if !skip_bytes(reader, color_table_len) {
                        return false;
                    }
                }
                if read_byte(reader).is_none() || !skip_gif_sub_blocks(reader) {
                    return false;
                }
            }
            Some(0x21) => {
                if read_byte(reader).is_none() || !skip_gif_sub_blocks(reader) {
                    return false;
                }
            }
            Some(0x3b) | None => return false,
            Some(0x00) => {}
            Some(_) => return false,
        }
    }
}

fn apng_has_multiple_frames(reader: &mut dyn Read) -> bool {
    let mut signature = [0_u8; 8];
    if reader.read_exact(&mut signature).is_err() || signature != *b"\x89PNG\r\n\x1a\n" {
        return false;
    }
    loop {
        let mut header = [0_u8; 8];
        if reader.read_exact(&mut header).is_err() {
            return false;
        }
        let len = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let kind = &header[4..8];
        if kind == b"acTL" {
            let mut control = [0_u8; 8];
            return len == control.len()
                && reader.read_exact(&mut control).is_ok()
                && u32::from_be_bytes(control[..4].try_into().unwrap()) > 1;
        }
        if kind == b"IDAT" || kind == b"IEND" || !skip_bytes(reader, len.saturating_add(4)) {
            return false;
        }
    }
}

fn webp_has_animation(reader: &mut dyn Read) -> bool {
    let mut header = [0_u8; 12];
    if reader.read_exact(&mut header).is_err() || &header[..4] != b"RIFF" || &header[8..] != b"WEBP"
    {
        return false;
    }
    let mut remaining =
        (u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize).saturating_sub(4);
    while remaining >= 8 {
        let mut chunk = [0_u8; 8];
        if reader.read_exact(&mut chunk).is_err() {
            return false;
        }
        let len = u32::from_le_bytes(chunk[4..8].try_into().unwrap()) as usize;
        let padded = len.saturating_add(len & 1);
        if &chunk[..4] == b"ANIM" || &chunk[..4] == b"ANMF" {
            return true;
        }
        if padded > remaining.saturating_sub(8) || !skip_bytes(reader, padded) {
            return false;
        }
        remaining -= 8 + padded;
    }
    false
}

pub fn dynamic_image_to_color_image(image: &image::DynamicImage) -> egui::ColorImage {
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

/// worker thread 向け canonical 8192 clamp。上限内なら入力を clone せず返す。
pub fn clamp_dynamic_for_gpu(image: image::DynamicImage) -> image::DynamicImage {
    let (width, height) = (image.width(), image.height());
    if width <= CANONICAL_RASTER_MAX_LONG_EDGE && height <= CANONICAL_RASTER_MAX_LONG_EDGE {
        return image;
    }
    let scale = CANONICAL_RASTER_MAX_LONG_EDGE as f64 / width.max(height) as f64;
    let new_width = ((width as f64 * scale).round() as u32).max(1);
    let new_height = ((height as f64 * scale).round() as u32).max(1);
    let started = std::time::Instant::now();
    let resized = crate::fast_resize::resize_dynamic_exact(
        &image,
        new_width,
        new_height,
        crate::fast_resize::Quality::Bilinear,
    );
    crate::logger::log(format!(
        "  clamp_dynamic_for_gpu: {width}x{height} → {new_width}x{new_height} (limit {CANONICAL_RASTER_MAX_LONG_EDGE}) in {:.0}ms",
        started.elapsed().as_secs_f64() * 1000.0
    ));
    resized
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;
    use std::io::{Cursor, Write};
    use std::sync::Mutex;

    fn rgba_fixture(width: u32, height: u32) -> image::RgbaImage {
        image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([
                (x * 53 + y * 7) as u8,
                (x * 11 + y * 61) as u8,
                (x * 29 + y * 17) as u8,
                255,
            ])
        })
    }

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(rgba_fixture(width, height))
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn static_image(result: CanonicalImageDecode) -> CanonicalStaticImage {
        match result {
            CanonicalImageDecode::Static(image) => image,
            CanonicalImageDecode::Animated { .. } => panic!("expected static image"),
        }
    }

    fn assert_same_image(left: &image::DynamicImage, right: &image::DynamicImage) {
        assert_eq!(left.dimensions(), right.dimensions());
        assert_eq!(left.to_rgba8().as_raw(), right.to_rgba8().as_raw());
    }

    fn assert_same_frames(left: &[(egui::ColorImage, f64)], right: &[(egui::ColorImage, f64)]) {
        assert_eq!(left.len(), right.len());
        for ((left_pixels, left_delay), (right_pixels, right_delay)) in
            left.iter().zip(right.iter())
        {
            assert_eq!(left_pixels.size, right_pixels.size);
            assert_eq!(left_pixels.pixels, right_pixels.pixels);
            assert_eq!(left_delay.to_bits(), right_delay.to_bits());
        }
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut output);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, bytes) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        output.into_inner()
    }

    fn orientation_exif_payload(value: u16) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"Exif\0\0");
        payload.extend_from_slice(b"II");
        payload.extend_from_slice(&0x002A_u16.to_le_bytes());
        payload.extend_from_slice(&8_u32.to_le_bytes());
        payload.extend_from_slice(&1_u16.to_le_bytes());
        payload.extend_from_slice(&0x0112_u16.to_le_bytes());
        payload.extend_from_slice(&3_u16.to_le_bytes());
        payload.extend_from_slice(&1_u32.to_le_bytes());
        payload.extend_from_slice(&value.to_le_bytes());
        payload.extend_from_slice(&0_u16.to_le_bytes());
        payload.extend_from_slice(&0_u32.to_le_bytes());
        payload
    }

    fn jpeg_with_orientation(value: u16) -> Vec<u8> {
        let rgb = image::RgbImage::from_fn(2, 3, |x, y| {
            image::Rgb([(x * 90) as u8, (y * 70) as u8, (x * 20 + y * 30) as u8])
        });
        let jpeg = turbojpeg::compress_image(&rgb, 95, turbojpeg::Subsamp::Sub2x2)
            .unwrap()
            .to_vec();
        let payload = orientation_exif_payload(value);
        let len = payload.len() + 2;
        let mut output = Vec::with_capacity(jpeg.len() + payload.len() + 4);
        output.extend_from_slice(&jpeg[..2]);
        output.extend_from_slice(&[0xFF, 0xE1, (len >> 8) as u8, (len & 0xff) as u8]);
        output.extend_from_slice(&payload);
        output.extend_from_slice(&jpeg[2..]);
        output
    }

    fn animated_gif_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            encoder
                .set_repeat(image::codecs::gif::Repeat::Infinite)
                .unwrap();
            let frames = [[255, 0, 0, 255], [0, 255, 0, 255]].map(|rgba| {
                image::Frame::from_parts(
                    image::RgbaImage::from_pixel(1, 1, image::Rgba(rgba)),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(40, 1),
                )
            });
            encoder.encode_frames(frames).unwrap();
        }
        bytes
    }

    fn png_crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffff_u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320_u32 & (0_u32.wrapping_sub(crc & 1)));
            }
        }
        !crc
    }

    fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&(data.len() as u32).to_be_bytes());
        chunk.extend_from_slice(kind);
        chunk.extend_from_slice(data);
        let mut crc_input = Vec::with_capacity(4 + data.len());
        crc_input.extend_from_slice(kind);
        crc_input.extend_from_slice(data);
        chunk.extend_from_slice(&png_crc32(&crc_input).to_be_bytes());
        chunk
    }

    fn compressed_png_pixel(rgba: [u8; 4]) -> Vec<u8> {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder
            .write_all(&[0, rgba[0], rgba[1], rgba[2], rgba[3]])
            .unwrap();
        encoder.finish().unwrap()
    }

    fn animated_apng_bytes() -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1_u32.to_be_bytes());
        ihdr.extend_from_slice(&1_u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        png.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
        let mut actl = Vec::new();
        actl.extend_from_slice(&2_u32.to_be_bytes());
        actl.extend_from_slice(&0_u32.to_be_bytes());
        png.extend_from_slice(&png_chunk(b"acTL", &actl));

        for (frame, rgba) in [[255, 0, 0, 255], [0, 255, 0, 255]].into_iter().enumerate() {
            let sequence = if frame == 0 { 0_u32 } else { 1_u32 };
            let mut fctl = Vec::new();
            fctl.extend_from_slice(&sequence.to_be_bytes());
            for value in [1_u32, 1, 0, 0] {
                fctl.extend_from_slice(&value.to_be_bytes());
            }
            fctl.extend_from_slice(&40_u16.to_be_bytes());
            fctl.extend_from_slice(&1000_u16.to_be_bytes());
            fctl.extend_from_slice(&[0, 0]);
            png.extend_from_slice(&png_chunk(b"fcTL", &fctl));
            let compressed = compressed_png_pixel(rgba);
            if frame == 0 {
                png.extend_from_slice(&png_chunk(b"IDAT", &compressed));
            } else {
                let mut fdat = 2_u32.to_be_bytes().to_vec();
                fdat.extend_from_slice(&compressed);
                png.extend_from_slice(&png_chunk(b"fdAT", &fdat));
            }
        }
        png.extend_from_slice(&png_chunk(b"IEND", &[]));
        png
    }

    fn riff_chunk(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(fourcc);
        output.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        output.extend_from_slice(payload);
        if payload.len() & 1 != 0 {
            output.push(0);
        }
        output
    }

    fn u24(value: u32) -> [u8; 3] {
        [value as u8, (value >> 8) as u8, (value >> 16) as u8]
    }

    fn webp_image_subchunk(rgba: [u8; 4]) -> Vec<u8> {
        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
            .encode(&rgba, 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        let mut position = 12;
        while position + 8 <= webp.len() {
            let size =
                u32::from_le_bytes(webp[position + 4..position + 8].try_into().unwrap()) as usize;
            let end = position + 8 + size;
            if &webp[position..position + 4] == b"VP8L" || &webp[position..position + 4] == b"VP8 "
            {
                return webp[position..end + (size & 1)].to_vec();
            }
            position = end + (size & 1);
        }
        panic!("lossless WebP fixture should contain an image chunk");
    }

    fn animated_webp_bytes() -> Vec<u8> {
        let mut chunks = Vec::new();
        let mut vp8x = vec![0b0000_0010, 0, 0, 0];
        vp8x.extend_from_slice(&u24(0));
        vp8x.extend_from_slice(&u24(0));
        chunks.extend_from_slice(&riff_chunk(b"VP8X", &vp8x));

        let mut anim = vec![0, 0, 0, 0];
        anim.extend_from_slice(&0_u16.to_le_bytes());
        chunks.extend_from_slice(&riff_chunk(b"ANIM", &anim));
        for (rgba, delay_ms) in [([255, 0, 0, 255], 40_u32), ([0, 255, 0, 255], 80)] {
            let mut frame = Vec::new();
            for value in [0_u32, 0, 0, 0, delay_ms] {
                frame.extend_from_slice(&u24(value));
            }
            frame.push(0);
            frame.extend_from_slice(&webp_image_subchunk(rgba));
            chunks.extend_from_slice(&riff_chunk(b"ANMF", &frame));
        }

        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&((4 + chunks.len()) as u32).to_le_bytes());
        webp.extend_from_slice(b"WEBP");
        webp.extend_from_slice(&chunks);
        webp
    }

    #[test]
    fn path_and_verified_bytes_have_identical_native_pixels() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("page.png");
        let bytes = png_bytes(3, 2);
        std::fs::write(&path, &bytes).unwrap();

        let path_image = static_image(
            decode_canonical_image(
                CanonicalImageSource::File {
                    path: &path,
                    verified_bytes: None,
                },
                CanonicalDecodeOptions::fullscreen(AnimationPolicy::FullFrames),
            )
            .unwrap(),
        );
        let bytes_image = static_image(
            decode_canonical_image(
                CanonicalImageSource::File {
                    path: &path,
                    verified_bytes: Some(&bytes),
                },
                CanonicalDecodeOptions::fullscreen(AnimationPolicy::FullFrames),
            )
            .unwrap(),
        );

        assert_eq!(path_image.source_dims, [3, 2]);
        assert_eq!(bytes_image.source_dims, [3, 2]);
        assert_same_image(&path_image.image, &bytes_image.image);
    }

    #[test]
    fn exif_orientation_matches_the_existing_fullscreen_operation() {
        let path = Path::new("oriented.jpg");
        let bytes = jpeg_with_orientation(6);
        let expected = crate::thumb_loader::apply_exif_orientation_from_bytes(
            image::load_from_memory(&bytes).unwrap(),
            &bytes,
        );
        let decoded = static_image(
            decode_canonical_image(
                CanonicalImageSource::File {
                    path,
                    verified_bytes: Some(&bytes),
                },
                CanonicalDecodeOptions::fullscreen(AnimationPolicy::FullFrames),
            )
            .unwrap(),
        );

        assert_eq!(decoded.source_dims, [3, 2]);
        assert_same_image(&decoded.image, &expected);
    }

    #[test]
    fn nested_zip_matches_the_same_verified_source_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("book.cbz");
        let page = png_bytes(4, 3);
        let inner = zip_bytes(&[("page.png", &page)]);
        write_zip(&outer, &[("chapter.zip", &inner)]);

        let archive_image = static_image(
            decode_canonical_image(
                CanonicalImageSource::ArchiveEntry {
                    archive_path: &outer,
                    entry_name: "chapter.zip/page.png",
                },
                CanonicalDecodeOptions::fullscreen(AnimationPolicy::FullFrames),
            )
            .unwrap(),
        );
        let bytes_image = static_image(
            decode_canonical_image(
                CanonicalImageSource::File {
                    path: Path::new("page.png"),
                    verified_bytes: Some(&page),
                },
                CanonicalDecodeOptions::fullscreen(AnimationPolicy::FullFrames),
            )
            .unwrap(),
        );

        assert_eq!(archive_image.source_dims, [4, 3]);
        assert_same_image(&archive_image.image, &bytes_image.image);
    }

    fn assert_first_frame(
        source: CanonicalImageSource<'_>,
        format: CanonicalAnimatedFormat,
        expected: &egui::ColorImage,
    ) {
        let image = static_image(
            decode_canonical_image(
                source,
                CanonicalDecodeOptions::fullscreen(AnimationPolicy::FirstFrameOnly),
            )
            .unwrap(),
        );
        assert_eq!(
            image.animation,
            CanonicalStaticAnimation::FirstFrameOnly(format)
        );
        let actual = dynamic_image_to_color_image(&image.image);
        assert_eq!(actual.size, expected.size);
        assert_eq!(actual.pixels, expected.pixels);
    }

    fn assert_full_frames(
        source: CanonicalImageSource<'_>,
        format: CanonicalAnimatedFormat,
        expected: &[(egui::ColorImage, f64)],
    ) {
        let decoded = decode_canonical_image(
            source,
            CanonicalDecodeOptions::fullscreen(AnimationPolicy::FullFrames),
        )
        .unwrap();
        let CanonicalImageDecode::Animated {
            format: actual_format,
            frames,
        } = decoded
        else {
            panic!("FullFrames should return Animated");
        };
        assert_eq!(actual_format, format);
        assert_same_frames(&frames, expected);
    }

    #[test]
    fn first_frame_only_matches_the_first_decoded_frame_for_all_animated_formats() {
        let temp = tempfile::tempdir().unwrap();
        let gif_path = temp.path().join("animated.gif");
        let apng_path = temp.path().join("animated.png");
        let webp_path = temp.path().join("animated.webp");
        let gif = animated_gif_bytes();
        let apng = animated_apng_bytes();
        let webp = animated_webp_bytes();
        for (path, bytes) in [
            (&gif_path, gif.as_slice()),
            (&apng_path, apng.as_slice()),
            (&webp_path, webp.as_slice()),
        ] {
            std::fs::write(path, bytes).unwrap();
        }

        let gif_frames = decode_gif_frames(&gif_path).unwrap();
        let apng_frames = decode_apng_frames(&apng_path).unwrap();
        let webp_frames = decode_webp_frames(&webp_path).unwrap();
        for (path, format, first) in [
            (&gif_path, CanonicalAnimatedFormat::Gif, &gif_frames[0].0),
            (&apng_path, CanonicalAnimatedFormat::Apng, &apng_frames[0].0),
            (&webp_path, CanonicalAnimatedFormat::WebP, &webp_frames[0].0),
        ] {
            assert_first_frame(
                CanonicalImageSource::File {
                    path,
                    verified_bytes: None,
                },
                format,
                first,
            );
        }
    }

    #[test]
    fn full_frames_returns_animated_for_all_file_formats() {
        let temp = tempfile::tempdir().unwrap();
        let gif_path = temp.path().join("animated.gif");
        let apng_path = temp.path().join("animated.png");
        let webp_path = temp.path().join("animated.webp");
        let gif = animated_gif_bytes();
        let apng = animated_apng_bytes();
        let webp = animated_webp_bytes();
        for (path, bytes) in [
            (&gif_path, gif.as_slice()),
            (&apng_path, apng.as_slice()),
            (&webp_path, webp.as_slice()),
        ] {
            std::fs::write(path, bytes).unwrap();
        }

        let gif_frames = decode_gif_frames(&gif_path).unwrap();
        let apng_frames = decode_apng_frames(&apng_path).unwrap();
        let webp_frames = decode_webp_frames(&webp_path).unwrap();
        for (path, format, expected) in [
            (
                &gif_path,
                CanonicalAnimatedFormat::Gif,
                gif_frames.as_slice(),
            ),
            (
                &apng_path,
                CanonicalAnimatedFormat::Apng,
                apng_frames.as_slice(),
            ),
            (
                &webp_path,
                CanonicalAnimatedFormat::WebP,
                webp_frames.as_slice(),
            ),
        ] {
            assert_full_frames(
                CanonicalImageSource::File {
                    path,
                    verified_bytes: None,
                },
                format,
                expected,
            );
        }
    }

    #[test]
    fn full_frames_returns_animated_for_all_archive_entry_formats() {
        let temp = tempfile::tempdir().unwrap();
        let zip_path = temp.path().join("book.zip");
        let gif = animated_gif_bytes();
        let apng = animated_apng_bytes();
        let webp = animated_webp_bytes();
        write_zip(
            &zip_path,
            &[
                ("animated.gif", &gif),
                ("animated.png", &apng),
                ("animated.webp", &webp),
            ],
        );

        let gif_frames = decode_gif_frames_from_bytes(&gif).unwrap();
        let apng_frames = decode_apng_frames_from_bytes(&apng).unwrap();
        let webp_frames = decode_webp_frames_from_bytes(&webp).unwrap();
        for (entry_name, format, expected) in [
            (
                "animated.gif",
                CanonicalAnimatedFormat::Gif,
                gif_frames.as_slice(),
            ),
            (
                "animated.png",
                CanonicalAnimatedFormat::Apng,
                apng_frames.as_slice(),
            ),
            (
                "animated.webp",
                CanonicalAnimatedFormat::WebP,
                webp_frames.as_slice(),
            ),
        ] {
            assert_full_frames(
                CanonicalImageSource::ArchiveEntry {
                    archive_path: &zip_path,
                    entry_name,
                },
                format,
                expected,
            );
        }
    }

    #[test]
    fn gpu_raster_clamps_after_preserving_native_source_dims() {
        let source_width = CANONICAL_RASTER_MAX_LONG_EDGE + 1;
        let bytes = png_bytes(source_width, 2);
        let decoded = static_image(
            decode_canonical_image(
                CanonicalImageSource::File {
                    path: Path::new("wide.png"),
                    verified_bytes: Some(&bytes),
                },
                CanonicalDecodeOptions::fullscreen(AnimationPolicy::FullFrames),
            )
            .unwrap(),
        );

        // panorama tee receives this native, unclamped image.
        assert_eq!(decoded.image.dimensions(), (source_width, 2));
        let expected_image = crate::fast_resize::resize_dynamic_exact(
            &decoded.image,
            CANONICAL_RASTER_MAX_LONG_EDGE,
            2,
            crate::fast_resize::Quality::Bilinear,
        );
        let expected_pixels = dynamic_image_to_color_image(&expected_image);
        let raster = decoded.into_gpu_raster();

        assert_eq!(raster.source_dims, [source_width as usize, 2]);
        assert_eq!(raster.pixels.size, expected_pixels.size);
        assert_eq!(raster.pixels.pixels, expected_pixels.pixels);
    }

    #[derive(Clone, Copy)]
    enum BackendResult {
        Miss,
        Hit,
    }

    struct RecordingFallbacks {
        calls: Mutex<Vec<&'static str>>,
        wic: BackendResult,
        susie: BackendResult,
    }

    impl RecordingFallbacks {
        fn image() -> image::DynamicImage {
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                2,
                1,
                image::Rgba([12, 34, 56, 255]),
            ))
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl FallbackDecoder for RecordingFallbacks {
        fn wic_path(&self, _path: &Path) -> Option<image::DynamicImage> {
            self.calls.lock().unwrap().push("wic_path");
            matches!(self.wic, BackendResult::Hit).then(Self::image)
        }

        fn wic_bytes(&self, _bytes: &[u8]) -> Option<image::DynamicImage> {
            self.calls.lock().unwrap().push("wic_bytes");
            matches!(self.wic, BackendResult::Hit).then(Self::image)
        }

        fn susie_path(
            &self,
            _path: &Path,
            _options: CanonicalDecodeOptions<'_>,
        ) -> std::io::Result<image::DynamicImage> {
            self.calls.lock().unwrap().push("susie_path");
            match self.susie {
                BackendResult::Hit => Ok(Self::image()),
                BackendResult::Miss => Err(std::io::Error::other("miss")),
            }
        }

        fn susie_bytes(
            &self,
            _filename_hint: &str,
            _bytes: &[u8],
            _options: CanonicalDecodeOptions<'_>,
        ) -> std::io::Result<image::DynamicImage> {
            self.calls.lock().unwrap().push("susie_bytes");
            match self.susie {
                BackendResult::Hit => Ok(Self::image()),
                BackendResult::Miss => Err(std::io::Error::other("miss")),
            }
        }
    }

    #[test]
    fn byte_fallback_order_is_image_then_wic_then_susie() {
        let path = Path::new("invalid.pi");
        let invalid = b"not an image";
        let wic_hit = RecordingFallbacks {
            calls: Mutex::new(Vec::new()),
            wic: BackendResult::Hit,
            susie: BackendResult::Hit,
        };
        let decoded = decode_canonical_image_with_fallbacks(
            CanonicalImageSource::File {
                path,
                verified_bytes: Some(invalid),
            },
            CanonicalDecodeOptions::fullscreen(AnimationPolicy::FullFrames),
            &wic_hit,
        )
        .unwrap();
        assert_eq!(static_image(decoded).source_dims, [2, 1]);
        assert_eq!(wic_hit.calls(), vec!["wic_bytes"]);

        let susie_hit = RecordingFallbacks {
            calls: Mutex::new(Vec::new()),
            wic: BackendResult::Miss,
            susie: BackendResult::Hit,
        };
        let decoded = decode_canonical_image_with_fallbacks(
            CanonicalImageSource::File {
                path,
                verified_bytes: Some(invalid),
            },
            CanonicalDecodeOptions::fullscreen(AnimationPolicy::FullFrames),
            &susie_hit,
        )
        .unwrap();
        assert_eq!(static_image(decoded).source_dims, [2, 1]);
        assert_eq!(susie_hit.calls(), vec!["wic_bytes", "susie_bytes"]);
    }

    #[test]
    fn path_fallback_order_is_image_then_wic_then_susie() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("invalid.pi");
        std::fs::write(&path, b"not an image").unwrap();
        let fallbacks = RecordingFallbacks {
            calls: Mutex::new(Vec::new()),
            wic: BackendResult::Miss,
            susie: BackendResult::Hit,
        };
        let decoded = decode_canonical_image_with_fallbacks(
            CanonicalImageSource::File {
                path: &path,
                verified_bytes: None,
            },
            CanonicalDecodeOptions::fullscreen(AnimationPolicy::FullFrames),
            &fallbacks,
        )
        .unwrap();

        assert_eq!(static_image(decoded).source_dims, [2, 1]);
        assert_eq!(fallbacks.calls(), vec!["wic_path", "susie_path"]);
    }

    #[test]
    fn primary_decode_success_never_calls_fallbacks() {
        let bytes = png_bytes(2, 2);
        let fallbacks = RecordingFallbacks {
            calls: Mutex::new(Vec::new()),
            wic: BackendResult::Hit,
            susie: BackendResult::Hit,
        };
        let decoded = decode_canonical_image_with_fallbacks(
            CanonicalImageSource::File {
                path: Path::new("page.png"),
                verified_bytes: Some(&bytes),
            },
            CanonicalDecodeOptions::fullscreen(AnimationPolicy::FullFrames),
            &fallbacks,
        )
        .unwrap();

        assert_eq!(static_image(decoded).source_dims, [2, 2]);
        assert!(fallbacks.calls().is_empty());
    }

    #[test]
    fn failed_wic_and_susie_preserve_the_primary_decode_error() {
        let invalid = b"not an image";
        let fallbacks = RecordingFallbacks {
            calls: Mutex::new(Vec::new()),
            wic: BackendResult::Miss,
            susie: BackendResult::Miss,
        };
        let result = decode_canonical_image_with_fallbacks(
            CanonicalImageSource::File {
                path: Path::new("invalid.pi"),
                verified_bytes: Some(invalid),
            },
            CanonicalDecodeOptions::fullscreen(AnimationPolicy::FullFrames),
            &fallbacks,
        );

        assert!(matches!(result, Err(CanonicalDecodeError::Decode(_))));
        assert_eq!(fallbacks.calls(), vec!["wic_bytes", "susie_bytes"]);
    }

    #[cfg(windows)]
    #[test]
    fn wic_byte_pixels_match_lossless_png_when_available() {
        let bytes = png_bytes(3, 2);
        let expected = image::load_from_memory(&bytes).unwrap();
        let Some(actual) = SystemFallbackDecoder.wic_bytes(&bytes) else {
            eprintln!("skipping WIC pixel comparison: decoder unavailable");
            return;
        };
        assert_same_image(&actual, &expected);
    }
}
