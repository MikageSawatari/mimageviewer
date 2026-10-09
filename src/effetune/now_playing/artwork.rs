//! All source discovery, disk access, first-frame decode, encoding, and base64 run here.
use super::{NowPlayingArtworkSource, NowPlayingInput};
use crate::audio_thumbnail::metadata;
use base64::Engine;
use image::{DynamicImage, ImageDecoder, ImageFormat};
use std::{
    io::{Cursor, Read},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
pub(super) const MAX_ENCODED_INPUT: usize = 64 * 1024 * 1024;
pub(super) const MAX_ARTWORK_BYTES: usize = 8 * 1024 * 1024;
pub(super) struct EncodedArt {
    pub base64: Arc<str>,
    pub mime: &'static str,
    pub warning: Option<String>,
}
fn check(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("canceled".into())
    } else {
        Ok(())
    }
}

fn read_bounded(path: &Path, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let initial = usize::try_from(size).map_err(|_| "encoded source exceeds 64 MiB")?;
    read_bounded_stream(&mut file, initial, MAX_ENCODED_INPUT, cancel)
}
fn read_bounded_stream(
    reader: &mut impl Read,
    initial: usize,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    if initial > limit {
        return Err("encoded source exceeds 64 MiB".into());
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(initial)
        .map_err(|e| e.to_string())?;
    let mut chunk = [0u8; 8192];
    loop {
        check(cancel)?;
        let remaining = limit - bytes.len();
        if remaining == 0 {
            let mut probe = [0u8; 1];
            if reader.read(&mut probe).map_err(|e| e.to_string())? != 0 {
                return Err("encoded source grew beyond 64 MiB".into());
            }
            break;
        }
        let want = remaining.min(chunk.len());
        let count = reader.read(&mut chunk[..want]).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        bytes.try_reserve_exact(count).map_err(|e| e.to_string())?;
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(bytes)
}

fn stamp(path: &Path) -> Result<(u64, std::time::SystemTime), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    Ok((meta.len(), meta.modified().map_err(|e| e.to_string())?))
}

pub(super) fn prepare(
    input: &NowPlayingInput,
    cancel: &Arc<AtomicBool>,
) -> Result<Option<EncodedArt>, String> {
    check(cancel)?;
    let (settings, sidecar, raw_executor, io, video, pinned) = match &input.artwork {
        NowPlayingArtworkSource::None => return Ok(None),
        NowPlayingArtworkSource::Audio {
            settings,
            sidecar,
            raw_executor,
            io_semaphore,
        } => (settings, sidecar, raw_executor, io_semaphore, false, None),
        NowPlayingArtworkSource::Video {
            settings,
            sidecar,
            raw_executor,
            io_semaphore,
            pinned_webp,
        } => (
            settings,
            sidecar,
            raw_executor,
            io_semaphore,
            true,
            pinned_webp.as_deref(),
        ),
    };
    let Some(_permit) = io.acquire_cancellable(crate::io_semaphore::IoPriority::Normal, cancel)
    else {
        return Err("canceled".into());
    };
    check(cancel)?;
    let original = stamp(&input.path)?;
    let mut source_error = None;
    if video {
        // Pins precede sidecars, exactly as the listing source chain. The DB is read-only.
        let pin = if pinned.is_none() {
            match read_pin_bounded(&input.path) {
                Ok(pin) => pin,
                Err(error) => {
                    source_error = Some(error);
                    None
                }
            }
        } else {
            None
        };
        if let Some(bytes) = pinned.or(pin.as_deref()) {
            check(cancel)?;
            match normalize(bytes, "webp", cancel) {
                Ok(art) => {
                    check(cancel)?;
                    if stamp(&input.path)? != original {
                        return Err("media changed during artwork preparation".into());
                    }
                    return Ok(Some(art));
                }
                Err(error) if cancel.load(Ordering::Acquire) => return Err(error),
                Err(error) => {
                    source_error = Some(error);
                }
            }
        }
    }
    let discovered;
    let sidecar = if settings.video_thumb_use_sidecar_image && settings.skip_image_if_video_exists {
        if let Some(path) = sidecar.as_deref() {
            Some(path)
        } else {
            discovered = crate::app::folder_scan::discover_aggregate_video_sidecars_while(
                settings,
                std::slice::from_ref(&input.path),
                1,
                || !cancel.load(Ordering::Acquire),
            );
            if let Some(found) = &discovered
                && let Some((_, error)) = found.scan_errors.first()
            {
                source_error.get_or_insert(error.clone());
            }
            discovered
                .as_ref()
                .and_then(|found| {
                    found
                        .by_video_path
                        .get(&crate::path_key::normalize_keep_drive(&input.path))
                })
                .map(|p| p.as_path())
        }
    } else {
        None
    };
    if let Some(path) = sidecar {
        check(cancel)?;
        match image_file(path, raw_executor, cancel) {
            Ok(mut art) => {
                art.warning = source_error;
                check(cancel)?;
                if stamp(&input.path)? != original {
                    return Err("media changed during artwork preparation".into());
                }
                return Ok(Some(art));
            }
            Err(error) if cancel.load(Ordering::Acquire) => return Err(error),
            Err(error) => {
                source_error.get_or_insert(error);
            }
        }
    }
    check(cancel)?;
    let mut result = if video {
        let (image, diag) = crate::video_thumb::get_video_thumbnail_bounded(
            &input.path,
            1024,
            metadata::DECODE_BUDGET,
        );
        match image {
            Some(image) => Some(encode(color_image(image)?, cancel)?),
            None => {
                if let Some(error) = shell_failure_warning(&diag) {
                    source_error.get_or_insert(error);
                }
                None
            }
        }
    } else if input
        .path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp3"))
    {
        let mut file = std::fs::File::open(&input.path).map_err(|e| e.to_string())?;
        let started = std::time::Instant::now();
        let mut checkpoint = || {
            if cancel.load(Ordering::Acquire) {
                Err(crate::audio_album_art::Error::Canceled)
            } else if started.elapsed() > std::time::Duration::from_secs(10) {
                Err(crate::audio_album_art::Error::Timeout)
            } else {
                Ok(())
            }
        };
        crate::audio_album_art::extract(&mut file, original.0, &mut checkpoint, &mut |bytes| {
            match normalize(bytes, "", cancel) {
                Ok(art) => Ok(Some(art)),
                Err(_) if cancel.load(Ordering::Acquire) => {
                    Err(crate::audio_album_art::Error::Canceled)
                }
                Err(error) => {
                    source_error.get_or_insert(error);
                    Ok(None)
                }
            }
        })
        .map_err(|e| format!("APIC: {e:?}"))?
    } else {
        None
    };
    check(cancel)?;
    if stamp(&input.path)? != original {
        return Err("media changed during artwork preparation".into());
    }
    if let Some(art) = &mut result {
        art.warning = source_error;
    } else if let Some(error) = source_error {
        return Err(error);
    }
    Ok(result)
}
fn shell_failure_warning(diag: &crate::video_thumb::VideoThumbDiag) -> Option<String> {
    diag.fail_stage.map(|_| {
        format!(
            "Shell representative {}: {}",
            diag.stage_label(),
            diag.hresult_hex()
        )
    })
}
fn raster_within_budget(w: u32, h: u32, total_bytes: u64) -> bool {
    w > 0
        && h > 0
        && u64::from(w)
            .checked_mul(u64::from(h))
            .and_then(|n| n.checked_mul(4))
            .is_some_and(|n| n <= metadata::DECODE_BUDGET)
        && total_bytes <= metadata::DECODE_BUDGET
}
fn read_pin_bounded(path: &Path) -> Result<Option<Vec<u8>>, String> {
    read_pin_bounded_at(
        &crate::video_pins::VideoPinDb::db_path(),
        path,
        MAX_ENCODED_INPUT,
    )
}
fn read_pin_bounded_at(
    db_path: &Path,
    path: &Path,
    max_input: usize,
) -> Result<Option<Vec<u8>>, String> {
    use rusqlite::OptionalExtension;
    if !db_path.is_file() {
        return Ok(None);
    }
    let db = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    db.busy_timeout(std::time::Duration::from_millis(750))
        .map_err(|e| e.to_string())?;
    // The listing preserves a previous nonempty pin while replacement extraction
    // is pending. Use that same representative, with length checked by SQL before
    // rusqlite creates the owned BLOB. Do not introduce a pts-based priority change.
    db.query_row(
        "SELECT thumb_webp FROM video_pins WHERE path=?1 AND length(thumb_webp)>0 AND length(thumb_webp)<=?2",
        rusqlite::params![crate::path_key::normalize_keep_drive(path),max_input.min(MAX_ENCODED_INPUT) as i64],
        |row|row.get(0),
    ).optional().map_err(|e|e.to_string())
}

fn image_file(
    path: &Path,
    executor: &crate::raw::RawDevelopExecutor,
    cancel: &Arc<AtomicBool>,
) -> Result<EncodedArt, String> {
    let original = stamp(path)?;
    let art = if crate::raw_format::is_raw_path(path) {
        let raster = crate::thumb_loader::decode_raw_thumbnail_on_bounded_worker(
            crate::raw::RawOwnedSource::Path(path.to_owned()),
            1024,
            executor,
            cancel,
            metadata::DECODE_BUDGET,
        )
        .map_err(|e| e.to_string())?;
        encode(raster.image, cancel)?
    } else {
        let bytes = read_bounded(path, cancel)?;
        check(cancel)?;
        normalize(
            &bytes,
            path.extension().and_then(|s| s.to_str()).unwrap_or(""),
            cancel,
        )?
    };
    check(cancel)?;
    if stamp(path)? != original {
        return Err("image source changed during preparation".into());
    }
    Ok(art)
}
fn color_image(image: egui::ColorImage) -> Result<DynamicImage, String> {
    let [w, h] = image.size;
    if w.checked_mul(h)
        .and_then(|n| n.checked_mul(4))
        .is_none_or(|n| n as u64 > metadata::DECODE_BUDGET)
    {
        return Err("raster exceeds decode budget".into());
    }
    let bytes = image.pixels.iter().flat_map(|p| p.to_array()).collect();
    image::RgbaImage::from_raw(w as u32, h as u32, bytes)
        .map(DynamicImage::ImageRgba8)
        .ok_or("invalid raster".into())
}
fn orientation(
    bytes: &[u8],
    check: &mut impl FnMut() -> Result<(), crate::audio_album_art::Error>,
) -> Result<u16, crate::audio_album_art::Error> {
    if bytes.starts_with(b"II") || bytes.starts_with(b"MM") {
        return Ok(
            metadata::tiff_orientation(&bytes[..bytes.len().min(64 * 1024)], check)?.unwrap_or(1),
        );
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        let mut pos = 12usize;
        while let Some(header) = bytes.get(pos..pos.saturating_add(8)) {
            check()?;
            let len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
            let start = pos + 8;
            let Some(end) = start.checked_add(len).filter(|end| *end <= bytes.len()) else {
                return Err(crate::audio_album_art::Error::Malformed);
            };
            if &header[..4] == b"EXIF" {
                let data = &bytes[start..end];
                let data = data.strip_prefix(b"Exif\0\0").unwrap_or(data);
                return Ok(metadata::tiff_orientation(data, check)?.unwrap_or(1));
            }
            pos = end
                .checked_add(len % 2)
                .ok_or(crate::audio_album_art::Error::Malformed)?;
        }
    }
    metadata::orientation(bytes, check)
}
fn normalize(bytes: &[u8], extension: &str, cancel: &AtomicBool) -> Result<EncodedArt, String> {
    check(cancel)?;
    if bytes.len() > MAX_ENCODED_INPUT {
        return Err("encoded source exceeds 64 MiB".into());
    }
    let format = image::guess_format(bytes).ok();
    let mut checkpoint = || {
        if cancel.load(Ordering::Acquire) {
            Err(crate::audio_album_art::Error::Canceled)
        } else {
            Ok(())
        }
    };
    let orientation =
        orientation(bytes, &mut checkpoint).map_err(|e| format!("orientation: {e:?}"))?;
    let sanitized;
    let input = if format == Some(ImageFormat::Jpeg) {
        metadata::jpeg_budget(bytes, metadata::DECODE_BUDGET, &mut checkpoint)
            .map_err(|e| format!("JPEG budget: {e:?}"))?;
        sanitized = metadata::jpeg_without_metadata(bytes, &mut checkpoint)
            .map_err(|e| format!("JPEG metadata: {e:?}"))?;
        sanitized.as_slice()
    } else {
        bytes
    };
    let mut wic_orientation_applied = false;
    let decoded = if let Some(format) = format {
        let mut reader = image::ImageReader::with_format(Cursor::new(input), format);
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(metadata::DECODE_BUDGET);
        reader.limits(limits.clone());
        match reader.into_decoder() {
            Ok(mut decoder) => {
                let (w, h) = decoder.dimensions();
                if !raster_within_budget(w, h, decoder.total_bytes()) {
                    return Err("raster exceeds decode budget".into());
                }
                limits
                    .reserve(decoder.total_bytes())
                    .map_err(|e| e.to_string())?;
                decoder.set_limits(limits).map_err(|e| e.to_string())?;
                check(cancel)?;
                match DynamicImage::from_decoder(decoder) {
                    Ok(decoded) => decoded,
                    Err(error @ image::ImageError::Unsupported(_)) => {
                        // Some TIFF compression modes are recognized at construction
                        // but unsupported by the pixel decoder. Preserve the prior
                        // raster admission and delegate once to the bounded WIC path.
                        check(cancel)?;
                        wic_orientation_applied = true;
                        crate::wic_decoder::decode_to_dynamic_image_from_bytes_bounded(
                            input,
                            extension,
                            metadata::DECODE_BUDGET,
                        )
                        .ok_or_else(|| error.to_string())?
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
            Err(error) => {
                wic_orientation_applied = true;
                crate::wic_decoder::decode_to_dynamic_image_from_bytes_bounded(
                    input,
                    extension,
                    metadata::DECODE_BUDGET,
                )
                .ok_or_else(|| error.to_string())?
            }
        }
    } else {
        wic_orientation_applied = true;
        crate::wic_decoder::decode_to_dynamic_image_from_bytes_bounded(
            input,
            extension,
            metadata::DECODE_BUDGET,
        )
        .ok_or("image decoder unavailable")?
    };
    check(cancel)?;
    let decoded = if wic_orientation_applied {
        decoded
    } else {
        crate::thumb_loader::apply_orientation(decoded, orientation)
    };
    // Static, bounded PNG/JPEG/WebP can retain their original encoded bytes.
    // An animation marker conservatively selects reencoding of the first frame.
    let mime = match format {
        Some(ImageFormat::Png) => Some("image/png"),
        Some(ImageFormat::Jpeg) => Some("image/jpeg"),
        Some(ImageFormat::WebP) => Some("image/webp"),
        _ => None,
    };
    let animated = (format == Some(ImageFormat::Png) && bytes.windows(4).any(|s| s == b"acTL"))
        || (format == Some(ImageFormat::WebP) && bytes.windows(4).any(|s| s == b"ANIM"));
    if let Some(mime) = mime
        && !wic_orientation_applied
        && !animated
        && orientation == 1
        && decoded.width().max(decoded.height()) <= 1024
        && bytes.len() <= MAX_ARTWORK_BYTES
    {
        return finish(bytes.to_vec(), mime, cancel);
    }
    encode(decoded, cancel)
}
fn encode(image: DynamicImage, cancel: &AtomicBool) -> Result<EncodedArt, String> {
    check(cancel)?;
    let image = if image.width().max(image.height()) > 1024 {
        image.thumbnail(1024, 1024)
    } else {
        image
    };
    let mut bytes = Vec::new();
    let mime = if image.has_alpha() {
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        "image/png"
    } else {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 85)
            .encode_image(&image)
            .map_err(|e| e.to_string())?;
        "image/jpeg"
    };
    finish(bytes, mime, cancel)
}
fn finish(bytes: Vec<u8>, mime: &'static str, cancel: &AtomicBool) -> Result<EncodedArt, String> {
    check(cancel)?;
    if bytes.len() > MAX_ARTWORK_BYTES {
        return Err("output artwork exceeds 8 MiB".into());
    }
    let base64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    check(cancel)?;
    Ok(EncodedArt {
        base64: base64.into(),
        mime,
        warning: None,
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn ccitt3_tiff_fixture() -> Vec<u8> {
        // Pillow/libtiff-generated 16x8 bilevel checkerboard, compression=Group 3.
        // Embedded bytes keep the regression independent of external image tools.
        base64::engine::general_purpose::STANDARD
            .decode("SUkqACYAAAAAG3bABt2wATV27ABNXbsAG3bABt2wATV27ABNXbsJAAABAwABAAAAEAAAAAEBAwABAAAACAAAAAIBAwABAAAAAQAAAAMBAwABAAAAAwAAAAYBAwABAAAAAQAAABEBBAABAAAACAAAABYBAwABAAAACAAAABcBBAABAAAAHgAAABwBAwABAAAAAQAAAAAAAAA=")
            .unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn now_playing_ccitt3_pixel_unsupported_uses_bounded_wic_once() {
        let bytes = ccitt3_tiff_fixture();
        let decoder = image::ImageReader::with_format(Cursor::new(&bytes), ImageFormat::Tiff)
            .into_decoder()
            .expect("CCITT3 TIFF construction must succeed");
        assert_eq!(decoder.dimensions(), (16, 8));
        assert!(matches!(
            DynamicImage::from_decoder(decoder),
            Err(image::ImageError::Unsupported(_))
        ));
        let wic = crate::wic_decoder::decode_to_dynamic_image_from_bytes_bounded(
            &bytes,
            "tiff",
            metadata::DECODE_BUDGET,
        )
        .expect("Windows WIC must support this small CCITT3 TIFF");
        assert_eq!((wic.width(), wic.height()), (16, 8));
        let art = normalize(&bytes, "tiff", &AtomicBool::new(false)).unwrap();
        let decoded = image::load_from_memory(
            &base64::engine::general_purpose::STANDARD
                .decode(&*art.base64)
                .unwrap(),
        )
        .unwrap()
        .to_rgba8();
        assert_eq!((decoded.width(), decoded.height()), (16, 8));
        assert_eq!(decoded.get_pixel(0, 0).0, [0, 0, 0, 255]);
        assert_eq!(decoded.get_pixel(4, 0).0, [255, 255, 255, 255]);
        assert_eq!(decoded, wic.to_rgba8());
    }

    #[cfg(windows)]
    #[test]
    fn now_playing_ccitt3_raster_over_budget_rejects_before_pixel_fallback() {
        let mut bytes = ccitt3_tiff_fixture();
        let ifd = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let count = u16::from_le_bytes(bytes[ifd..ifd + 2].try_into().unwrap()) as usize;
        for entry in bytes[ifd + 2..ifd + 2 + count * 12].chunks_exact_mut(12) {
            let tag = u16::from_le_bytes(entry[..2].try_into().unwrap());
            let dimension = match tag {
                256 => 8193u16,
                257 | 278 => 5120u16,
                _ => continue,
            };
            entry[8..10].copy_from_slice(&dimension.to_le_bytes());
        }
        let decoder = image::ImageReader::with_format(Cursor::new(&bytes), ImageFormat::Tiff)
            .into_decoder()
            .expect("budget test must reach raster admission after construction");
        assert_eq!(decoder.dimensions(), (8193, 5120));
        assert!(!raster_within_budget(8193, 5120, decoder.total_bytes()));
        assert_eq!(
            normalize(&bytes, "tiff", &AtomicBool::new(false))
                .err()
                .as_deref(),
            Some("raster exceeds decode budget"),
        );
        assert!(
            crate::wic_decoder::decode_to_dynamic_image_from_bytes_bounded(
                &ccitt3_tiff_fixture(),
                "tiff",
                16 * 8 * 4 - 1,
            )
            .is_none()
        );
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            w,
            h,
            image::Rgba([255, 10, 20, 100]),
        ));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }
    #[test]
    fn now_playing_static_webp_and_png_reuse_respects_1024_boundary() {
        let cancel = AtomicBool::new(false);
        for format in [ImageFormat::Png, ImageFormat::WebP] {
            let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                1024,
                10,
                image::Rgba([10, 20, 30, 200]),
            ));
            let mut bytes = Vec::new();
            image
                .write_to(&mut Cursor::new(&mut bytes), format)
                .unwrap();
            let art = normalize(&bytes, "", &cancel).unwrap();
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(&*art.base64)
                    .unwrap(),
                bytes
            );
        }
        let art = normalize(&png(2048, 512), "png", &cancel).unwrap();
        let decoded = image::load_from_memory(
            &base64::engine::general_purpose::STANDARD
                .decode(&*art.base64)
                .unwrap(),
        )
        .unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1024, 256));
        assert_eq!(art.mime, "image/png");
    }
    #[test]
    fn now_playing_large_encoded_input_can_shrink_and_output_limit_is_exact() {
        let cancel = AtomicBool::new(false);
        let mut bytes = png(16, 8);
        bytes.resize(MAX_ARTWORK_BYTES + 1, 0);
        let art = normalize(&bytes, "png", &cancel).unwrap();
        assert!(art.base64.len() < 1000);
        assert!(finish(vec![0; MAX_ARTWORK_BYTES], "image/png", &cancel).is_ok());
        assert!(finish(vec![0; MAX_ARTWORK_BYTES + 1], "image/png", &cancel).is_err());
        assert!(normalize(b"invalid image", "png", &cancel).is_err());
    }
    #[test]
    fn now_playing_animated_gif_uses_first_frame_and_retains_transparency() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            encoder
                .encode_frames([
                    image::Frame::new(image::RgbaImage::from_pixel(
                        2,
                        1,
                        image::Rgba([255, 0, 0, 255]),
                    )),
                    image::Frame::new(image::RgbaImage::from_pixel(
                        2,
                        1,
                        image::Rgba([0, 0, 255, 255]),
                    )),
                ])
                .unwrap();
        }
        let art = normalize(&bytes, "gif", &AtomicBool::new(false)).unwrap();
        let decoded = image::load_from_memory(
            &base64::engine::general_purpose::STANDARD
                .decode(&*art.base64)
                .unwrap(),
        )
        .unwrap()
        .to_rgba8();
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(art.mime, "image/png");
    }
    #[test]
    fn now_playing_jpeg_orientation_is_bounded_and_applied_before_encoding() {
        let image = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            200,
            100,
            image::Rgb([20, 30, 40]),
        ));
        let mut jpeg = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut jpeg), ImageFormat::Jpeg)
            .unwrap();
        let exif = [
            b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0,
            0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
        ];
        let mut bytes = jpeg[..2].to_vec();
        bytes.extend_from_slice(&[0xff, 0xe1]);
        bytes.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        bytes.extend_from_slice(&exif);
        bytes.extend_from_slice(&jpeg[2..]);
        let art = normalize(&bytes, "jpg", &AtomicBool::new(false)).unwrap();
        let decoded = image::load_from_memory(
            &base64::engine::general_purpose::STANDARD
                .decode(&*art.base64)
                .unwrap(),
        )
        .unwrap();
        assert_eq!((decoded.width(), decoded.height()), (100, 200));
        assert_eq!(art.mime, "image/jpeg");
    }
    #[test]
    fn now_playing_source_selection_sidecar_off_non_mp3_and_cancellation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("song.flac");
        std::fs::write(&path, b"audio").unwrap();
        let sidecar = temp.path().join("song.png");
        std::fs::write(&sidecar, png(32, 16)).unwrap();
        let mut settings = crate::settings::Settings::default();
        settings.skip_image_if_video_exists = true;
        settings.video_thumb_use_sidecar_image = true;
        let make = |settings| NowPlayingInput {
            path: path.clone(),
            title: None,
            album: None,
            artist: None,
            artwork: NowPlayingArtworkSource::Audio {
                sidecar: Some(sidecar.clone()),
                settings: Arc::new(settings),
                raw_executor: Arc::new(crate::raw::RawDevelopExecutor::new(1).unwrap()),
                io_semaphore: Arc::new(crate::io_semaphore::GlobalIoSemaphore::new(1)),
            },
        };
        let cancel = Arc::new(AtomicBool::new(false));
        assert!(prepare(&make(settings.clone()), &cancel).unwrap().is_some());
        settings.video_thumb_use_sidecar_image = false;
        assert!(prepare(&make(settings), &cancel).unwrap().is_none());
        cancel.store(true, Ordering::Release);
        assert!(prepare(&make(crate::settings::Settings::default()), &cancel).is_err());
    }
    #[test]
    fn now_playing_file_input_limit_is_checked_before_read() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large.png");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(MAX_ENCODED_INPUT as u64 + 1)
            .unwrap();
        assert!(
            read_bounded(&path, &AtomicBool::new(false))
                .unwrap_err()
                .contains("64 MiB")
        );
    }
    #[test]
    fn now_playing_png_raster_budget_rejects_oversized_header_before_pixels() {
        let mut bytes = png(1, 1);
        bytes[16..20].copy_from_slice(&20_000u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&20_000u32.to_be_bytes());
        let mut crc = !0u32;
        for byte in &bytes[12..29] {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
            }
        }
        bytes[29..33].copy_from_slice(&(!crc).to_be_bytes());
        assert!(normalize(&bytes, "png", &AtomicBool::new(false)).is_err());
    }
    #[test]
    fn now_playing_rejected_sidecar_reports_failure_without_losing_no_art_semantics() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("song.flac");
        std::fs::write(&path, b"audio").unwrap();
        let sidecar = temp.path().join("song.png");
        std::fs::write(&sidecar, b"invalid image").unwrap();
        let mut settings = crate::settings::Settings::default();
        settings.skip_image_if_video_exists = true;
        settings.video_thumb_use_sidecar_image = true;
        let mut input = NowPlayingInput {
            path,
            title: None,
            album: None,
            artist: None,
            artwork: NowPlayingArtworkSource::Audio {
                sidecar: Some(sidecar),
                settings: Arc::new(settings.clone()),
                raw_executor: Arc::new(crate::raw::RawDevelopExecutor::new(1).unwrap()),
                io_semaphore: Arc::new(crate::io_semaphore::GlobalIoSemaphore::new(1)),
            },
        };
        let cancel = Arc::new(AtomicBool::new(false));
        assert!(prepare(&input, &cancel).is_err());
        if let NowPlayingArtworkSource::Audio { sidecar, .. } = &mut input.artwork {
            *sidecar = None;
        }
        // Remove the invalid sidecar so shared discovery also finds no image.
        std::fs::remove_file(temp.path().join("song.png")).unwrap();
        assert!(prepare(&input, &cancel).unwrap().is_none());
    }
    #[test]
    fn now_playing_encoded_growth_and_raster_overflow_reject_before_allocation() {
        let cancel = AtomicBool::new(false);
        assert_eq!(
            read_bounded_stream(&mut Cursor::new(b"abc"), 3, 3, &cancel).unwrap(),
            b"abc"
        );
        assert!(read_bounded_stream(&mut Cursor::new(b"abcd"), 3, 3, &cancel).is_err());
        assert!(read_bounded_stream(&mut Cursor::new(b"abcd"), 0, 3, &cancel).is_err());
        assert!(raster_within_budget(4096, 10240, metadata::DECODE_BUDGET));
        assert!(!raster_within_budget(4097, 10240, metadata::DECODE_BUDGET));
        assert!(!raster_within_budget(u32::MAX, u32::MAX, 1));
    }
    #[test]
    fn now_playing_shell_diagnostic_failures_warn_but_absent_art_does_not() {
        let mut diag = crate::video_thumb::VideoThumbDiag {
            hresult: 0,
            get_image_ms: 0,
            dims: None,
            avg_rgb: None,
            span_rgb: None,
            fail_stage: None,
        };
        assert!(shell_failure_warning(&diag).is_none());
        diag.fail_stage = Some(crate::video_thumb::VideoThumbFailStage::GetDibits);
        assert!(
            shell_failure_warning(&diag)
                .unwrap()
                .contains("GetDIBits-fail")
        );
    }
    #[test]
    fn now_playing_pending_pin_matches_listing_and_sql_rejects_empty_null_over_budget() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("pins.db");
        let media = temp.path().join("song.mp4");
        let db = crate::video_pins::VideoPinDb::open_at(&db_path).unwrap();
        let mut webp = Vec::new();
        DynamicImage::new_rgb8(3, 2)
            .write_to(&mut Cursor::new(&mut webp), ImageFormat::WebP)
            .unwrap();
        db.set_pin(&media, 1.0, &webp).unwrap();
        db.set_pin(&media, 2.0, &[]).unwrap();
        assert!(!db.lookup_meta(&media).unwrap().thumb_is_current());
        let listing = db.lookup_webps_many([media.clone()]);
        assert_eq!(listing.get(&media), Some(&webp));
        assert_eq!(
            read_pin_bounded_at(&db_path, &media, webp.len()).unwrap(),
            Some(webp.clone())
        );
        assert!(
            read_pin_bounded_at(&db_path, &media, webp.len() - 1)
                .unwrap()
                .is_none()
        );
        let writer = rusqlite::Connection::open(&db_path).unwrap();
        writer
            .execute("UPDATE video_pins SET thumb_webp=X''", [])
            .unwrap();
        assert!(
            read_pin_bounded_at(&db_path, &media, MAX_ENCODED_INPUT)
                .unwrap()
                .is_none()
        );
        writer
            .execute("UPDATE video_pins SET thumb_webp=NULL", [])
            .unwrap();
        assert!(
            read_pin_bounded_at(&db_path, &media, MAX_ENCODED_INPUT)
                .unwrap()
                .is_none()
        );
        assert!(
            read_pin_bounded_at(&db_path, &temp.path().join("other.mp4"), MAX_ENCODED_INPUT)
                .unwrap()
                .is_none()
        );
        assert!(
            read_pin_bounded_at(&temp.path().join("missing.db"), &media, MAX_ENCODED_INPUT)
                .unwrap()
                .is_none()
        );
    }
}
