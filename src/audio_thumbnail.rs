//! Worker-only audio representative generation. Sidecar provenance is prepared by listing scans.
use crate::{
    catalog::{
        AudioArtCached, AudioArtCatalogScope, AudioArtSourceStamp, CacheEntry, CatalogAdmission,
    },
    thumb_loader::{CacheDecision, LoadSourcePolicy, ThumbLoadOrigin},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) struct AudioThumbnailOptions {
    pub thumb_px: u32,
    pub thumb_quality: f32,
    pub cache_dir: PathBuf,
    pub cache_decision: CacheDecision,
    pub raw_executor: Option<Arc<crate::raw::RawDevelopExecutor>>,
}
pub(crate) struct AudioThumbnailPixels {
    pub image: egui::ColorImage,
    pub source_dims: (u32, u32),
    pub origin: ThumbLoadOrigin,
}
#[derive(Debug)]
pub(crate) enum AudioThumbnailError {
    Failed(String),
    Canceled,
}
fn stamp(path: &Path) -> Result<AudioArtSourceStamp, AudioThumbnailError> {
    let m = std::fs::metadata(path).map_err(|e| AudioThumbnailError::Failed(e.to_string()))?;
    if !m.is_file() {
        return Err(AudioThumbnailError::Failed(
            "audio source is not a file".into(),
        ));
    }
    Ok(AudioArtSourceStamp {
        mtime_secs: m
            .modified()
            .map_err(|e| AudioThumbnailError::Failed(e.to_string()))?
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
        file_size: i64::try_from(m.len())
            .map_err(|e| AudioThumbnailError::Failed(e.to_string()))?,
    })
}
pub(crate) fn generate(
    path: &Path,
    sidecar: Option<&Path>,
    settings: &crate::settings::Settings,
    policy: LoadSourcePolicy,
    admission: CatalogAdmission,
    display_px: u32,
    should_cancel: &impl Fn() -> bool,
) -> Result<Option<AudioThumbnailPixels>, AudioThumbnailError> {
    generate_with_options(
        path,
        sidecar,
        &AudioThumbnailOptions {
            thumb_px: settings.thumb_px,
            thumb_quality: settings.thumb_quality as f32,
            cache_dir: crate::catalog::default_cache_dir(),
            cache_decision: CacheDecision::from_settings(settings),
            raw_executor: None,
        },
        policy,
        admission,
        display_px,
        should_cancel,
    )
}
/// Caller holds the shared I/O permit through extraction and decoding. No ActivityGate wait.
pub(crate) fn generate_with_options(
    path: &Path,
    sidecar: Option<&Path>,
    options: &AudioThumbnailOptions,
    policy: LoadSourcePolicy,
    admission: CatalogAdmission,
    display_px: u32,
    should_cancel: &impl Fn() -> bool,
) -> Result<Option<AudioThumbnailPixels>, AudioThumbnailError> {
    use crate::audio_album_art::{self, Error};
    let started = Instant::now();
    let source_started = std::cell::Cell::new(None::<Instant>);
    let mut check = || {
        if should_cancel() {
            Err(Error::Canceled)
        } else if source_started
            .get()
            .is_some_and(|t| t.elapsed() >= Duration::from_secs(10))
        {
            Err(Error::Timeout)
        } else {
            Ok(())
        }
    };
    let map_error = |error: Error| match error {
        Error::Canceled => AudioThumbnailError::Canceled,
        Error::Io(e) => AudioThumbnailError::Failed(e.to_string()),
        other => AudioThumbnailError::Failed(format!("audio art: {other:?}")),
    };
    check().map_err(map_error)?;
    let original = stamp(path)?;
    if policy.allows_source()
        && let Some(image_path) = sidecar
    {
        source_started.set(Some(Instant::now()));
        let sidecar_present = match std::fs::metadata(image_path) {
            Ok(meta) => {
                if !meta.is_file() {
                    return Err(AudioThumbnailError::Failed("sidecar is not a file".into()));
                }
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(AudioThumbnailError::Failed(error.to_string())),
        };
        if sidecar_present {
            let before = stamp(image_path)?;
            let executor = options.raw_executor.as_ref().ok_or_else(|| {
                AudioThumbnailError::Failed("sidecar decoder is unavailable".into())
            })?;
            match crate::thumb_loader::decode_image_for_thumb_with_dims(
                image_path, display_px, executor,
            ) {
                Ok(Some((image, source_dims))) => {
                    check().map_err(map_error)?;
                    if stamp(image_path)? != before || stamp(path)? != original {
                        return Err(AudioThumbnailError::Failed(
                            "thumbnail source changed during generation".into(),
                        ));
                    }
                    return Ok(Some(AudioThumbnailPixels {
                        source_dims,
                        image,
                        origin: ThumbLoadOrigin::SourceGenerated {
                            evaluated_display_px: display_px,
                        },
                    }));
                }
                Ok(None) => {
                    crate::logger::log(format!("audio sidecar has no image: {image_path:?}"))
                }
                Err(crate::raw::RawError::Cancelled | crate::raw::RawError::Stale) => {
                    return Err(AudioThumbnailError::Canceled);
                }
                Err(
                    e @ (crate::raw::RawError::Corrupt(_)
                    | crate::raw::RawError::Unsupported(_)
                    | crate::raw::RawError::NoUsablePreview(_)
                    | crate::raw::RawError::TooLarge),
                ) => crate::logger::log(format!("audio sidecar fallback: {e:?}")),
                Err(e) => return Err(AudioThumbnailError::Failed(format!("audio sidecar: {e:?}"))),
            }
        }
    }
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mp3"))
    {
        return Ok(None);
    }
    let parent = path
        .parent()
        .ok_or_else(|| AudioThumbnailError::Failed("audio has no parent".into()))?;
    let scope = AudioArtCatalogScope::new(parent);
    let key = scope.key_for(path);
    let access = crate::catalog::CatalogAccess::for_cache_dir(&options.cache_dir);
    let lookup_admission =
        if policy == LoadSourcePolicy::CacheOnly && !access.is_admitted(admission) {
            access
                .wait_read_admission_with(should_cancel)
                .ok_or(AudioThumbnailError::Canceled)?
        } else {
            admission
        };
    let report_cache_error = |error: &rusqlite::Error, proof: CatalogAdmission| {
        if access.is_admitted(proof) {
            crate::logger::log(format!("audio art catalog: {error}"));
            access.record_audio_art_cache_error(error.to_string());
        }
    };
    if policy.allows_cache() {
        match crate::catalog::lookup_audio_art(
            &options.cache_dir,
            &scope,
            &key,
            original,
            lookup_admission,
        ) {
            Ok(AudioArtCached::Pixels(entry)) => {
                if let Some(image) = crate::catalog::decode_thumb_to_color_image(&entry.jpeg_data) {
                    check().map_err(map_error)?;
                    if stamp(path)? != original {
                        return Err(AudioThumbnailError::Failed(
                            "thumbnail source changed during generation".into(),
                        ));
                    }
                    return Ok(Some(AudioThumbnailPixels {
                        source_dims: entry
                            .source_dims
                            .unwrap_or((image.size[0] as u32, image.size[1] as u32)),
                        image,
                        origin: ThumbLoadOrigin::UpgradeableCache,
                    }));
                }
            }
            Ok(AudioArtCached::NoArt) if access.is_admitted(lookup_admission) => {
                check().map_err(map_error)?;
                if stamp(path)? != original {
                    return Err(AudioThumbnailError::Failed(
                        "thumbnail source changed during generation".into(),
                    ));
                }
                return Ok(None);
            }
            Ok(AudioArtCached::Miss | AudioArtCached::NoArt) => {}
            Err(e) => report_cache_error(&e, lookup_admission),
        }
    }
    if !policy.allows_source() {
        return Err(AudioThumbnailError::Failed("audio art cache miss".into()));
    }
    check().map_err(map_error)?;
    if source_started.get().is_none() {
        source_started.set(Some(Instant::now()));
    }
    let mut file =
        std::fs::File::open(path).map_err(|e| AudioThumbnailError::Failed(e.to_string()))?;
    let target = display_px.max(options.thumb_px).max(1);
    let decoded = audio_album_art::extract(
        &mut file,
        original.file_size as u64,
        &mut check,
        &mut |bytes| Ok(decode_picture(bytes, target)),
    );
    let decoded = match decoded {
        Ok(result) => result,
        Err(error @ (Error::Malformed | Error::Limit)) => {
            crate::logger::log(format!("audio art rejected: {path:?}: {error:?}"));
            None
        }
        Err(e) => return Err(map_error(e)),
    };
    check().map_err(map_error)?;
    if stamp(path)? != original {
        return Err(AudioThumbnailError::Failed(
            "thumbnail source changed during generation".into(),
        ));
    }
    if let Some((image, dims)) = decoded {
        let display =
            crate::thumb_loader::resize_to_display_color_image(&image, display_px, Some(dims));
        check().map_err(map_error)?;
        if options.cache_decision.should_cache(
            path,
            original.file_size,
            started.elapsed().as_secs_f64() * 1000.0,
            0.0,
        ) && let Some((bytes, _, _)) = crate::catalog::encode_thumb_webp_with_source_dims(
            &image,
            options.thumb_px,
            options.thumb_quality,
            dims,
        ) {
            check().map_err(map_error)?;
            if stamp(path)? != original {
                return Err(AudioThumbnailError::Failed(
                    "thumbnail source changed during generation".into(),
                ));
            }
            let entry = CacheEntry {
                mtime: original.mtime_secs,
                file_size: original.file_size,
                jpeg_data: bytes,
                source_dims: Some(dims),
                layout_dims: None,
                folder_provenance: None,
                selection_proof: None,
            };
            if let Err(e) = crate::catalog::save_audio_art_pixels_with_cancel_check(
                &options.cache_dir,
                &scope,
                &key,
                &entry,
                admission,
                should_cancel,
            ) {
                report_cache_error(&e, admission);
            }
        }
        check().map_err(map_error)?;
        if stamp(path)? != original {
            return Err(AudioThumbnailError::Failed(
                "thumbnail source changed during generation".into(),
            ));
        }
        Ok(Some(AudioThumbnailPixels {
            image: display,
            source_dims: dims,
            origin: ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: display_px,
            },
        }))
    } else {
        crate::logger::log(format!("audio art: no usable embedded picture {path:?}"));
        if options.cache_decision.should_cache(
            path,
            original.file_size,
            started.elapsed().as_secs_f64() * 1000.0,
            0.0,
        ) {
            check().map_err(map_error)?;
            if let Err(e) = crate::catalog::save_audio_art_absence_with_cancel_check(
                &options.cache_dir,
                &scope,
                &key,
                original,
                admission,
                should_cancel,
            ) {
                report_cache_error(&e, admission);
            }
        }
        check().map_err(map_error)?;
        if stamp(path)? != original {
            return Err(AudioThumbnailError::Failed(
                "thumbnail source changed during generation".into(),
            ));
        }
        Ok(None)
    }
}
fn decode_picture(bytes: &[u8], target: u32) -> Option<(image::DynamicImage, (u32, u32))> {
    use image::ImageDecoder;
    let format = image::guess_format(bytes).ok()?;
    if !matches!(format, image::ImageFormat::Jpeg | image::ImageFormat::Png) {
        return None;
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(160 * 1024 * 1024);
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder().ok()?;
    let dims = decoder.dimensions();
    if dims.0 == 0
        || dims.1 == 0
        || u64::from(dims.0) * u64::from(dims.1) > 40_000_000
        || decoder.total_bytes() > 160 * 1024 * 1024
    {
        return None;
    }
    limits.reserve(decoder.total_bytes()).ok()?;
    decoder.set_limits(limits).ok()?;
    let image = if format == image::ImageFormat::Jpeg {
        match crate::thumb_loader::decode_jpeg_turbo_scaled_from_bytes(bytes, target) {
            Ok((image, _)) => image,
            Err(crate::thumb_loader::DctDecodeError::TerminalRejection(_)) => return None,
            Err(_) => image::DynamicImage::from_decoder(decoder).ok()?,
        }
    } else {
        image::DynamicImage::from_decoder(decoder).ok()?
    };
    let before = (image.width(), image.height());
    let image = crate::thumb_loader::apply_exif_orientation_from_bytes(image, bytes);
    let dims = if before.0 != before.1 && (image.width(), image.height()) == (before.1, before.0) {
        (dims.1, dims.0)
    } else {
        dims
    };
    Some((image, dims))
}

#[cfg(test)]
mod tests;
