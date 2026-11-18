//! One display-size-independent decode shared by worker, crop dialog and evaluator.
use super::*;
use crate::image_decode::Source;

#[path = "cover_auto_analysis/metadata.rs"]
pub(super) mod metadata;

#[cfg(test)]
pub(super) fn shot_exif_from_bytes(bytes: &[u8]) -> bool {
    metadata::from_bytes(bytes).shot_exif
}

const ANALYSIS_DECODE_HEIGHT: u32 = 1024;

/// Preserve interruption across the decode boundary. Only a data error is a
/// durable decision to leave this cover uncropped.
#[derive(Debug)]
pub(crate) enum CoverAutoAnalysisError {
    Cancelled,
    ReadFailed(String),
    Decode(String),
}

impl std::fmt::Display for CoverAutoAnalysisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cover analysis decode cancelled"),
            Self::ReadFailed(error) | Self::Decode(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for CoverAutoAnalysisError {}

impl CoverAutoAnalysisError {
    fn from_image(error: image::ImageError, cancel: Option<&Arc<AtomicBool>>) -> Self {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            return Self::Cancelled;
        }
        if let image::ImageError::IoError(error) = &error
            && crate::cover_auto_sample::classify_io_error(error)
                == crate::cover_auto_sample::SampleFailureKind::ReadFailed
        {
            return Self::ReadFailed(error.to_string());
        }
        Self::Decode(error.to_string())
    }
}

pub(crate) struct CoverAutoAnalysis {
    pub image: image::DynamicImage,
    pub decoder: crate::stats::DecodeSource,
    /// Dimensions after the released decoder's orientation rules. PDF uses page-box catalog units, because
    /// only the ratio enters the detector (not the display raster's rounding).
    pub native_dims: (u32, u32),
    pub shot_exif: bool,
    pub orientation: u16,
    pub dct_scale_num: Option<u32>,
}

fn check_cancel(cancel: Option<&Arc<AtomicBool>>) -> Result<(), CoverAutoAnalysisError> {
    if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
        Err(CoverAutoAnalysisError::Cancelled)
    } else {
        Ok(())
    }
}

/// `full_decode`, when supplied, already uses the released orientation rules and is full resolution.
/// A JPEG never reuses it: its fixed DCT selection must be identical in every
/// caller even if the display path happened to decode a different resolution.
pub(crate) fn decode_cover_auto_analysis_image(
    source: Source<'_>,
    full_decode: Option<&image::DynamicImage>,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<CoverAutoAnalysis, CoverAutoAnalysisError> {
    check_cancel(cancel)?;
    let jpeg = is_jpeg_extension(source.extension());
    let mut exif = None;
    if jpeg {
        let owned;
        let bytes = match source {
            Source::Bytes { bytes, .. } if bytes.len() as u64 <= MAX_TURBOJPEG_INPUT_SIZE => {
                Some(bytes)
            }
            Source::Path(path) => {
                let size = std::fs::metadata(path)
                    .map_err(|error| CoverAutoAnalysisError::ReadFailed(error.to_string()))?
                    .len();
                if size <= MAX_TURBOJPEG_INPUT_SIZE {
                    owned = std::fs::read(path).map_err(|error| {
                        CoverAutoAnalysisError::from_image(
                            crate::cover_auto_sample::source_read_error(error).into(),
                            cancel,
                        )
                    })?;
                    // Recheck a file that grew between metadata and read.
                    (owned.len() as u64 <= MAX_TURBOJPEG_INPUT_SIZE).then_some(owned.as_slice())
                } else {
                    None
                }
            }
            Source::Bytes { .. } => None,
        };
        if let Some(bytes) = bytes {
            check_cancel(cancel)?;
            let metadata = match source {
                Source::Path(path) => metadata::from_path_bytes(path, bytes),
                Source::Bytes { .. } => metadata::from_bytes(bytes),
            };
            exif = Some(metadata);
            match decode_jpeg_turbo_scaled_with_orientation(
                bytes,
                ANALYSIS_DECODE_HEIGHT,
                None,
                true,
                metadata.orientation,
            ) {
                Ok((image, stats)) => {
                    check_cancel(cancel)?;
                    let orientation = metadata.orientation;
                    let native_dims = stats.source_dims_after_exif(orientation);
                    return Ok(CoverAutoAnalysis {
                        image: apply_orientation(image, orientation),
                        decoder: crate::stats::DecodeSource::Native,
                        native_dims,
                        shot_exif: metadata.shot_exif,
                        orientation,
                        dct_scale_num: Some(stats.scale_num),
                    });
                }
                // Allocation admission is terminal. Falling back to a full
                // decoder here would bypass the DCT raster safety boundary.
                Err(DctDecodeError::TerminalRejection(error)) => {
                    return Err(CoverAutoAnalysisError::Decode(error));
                }
                Err(DctDecodeError::Fallback(_)) => {}
            }
        }
    }
    if full_decode.is_none()
        && let Source::Path(path) = source
        && ["png", "webp"]
            .iter()
            .any(|ext| source.extension().eq_ignore_ascii_case(ext))
    {
        return decode_native_metadata_path(path, cancel);
    }
    let metadata = exif.unwrap_or_else(|| match source {
        Source::Path(path) => metadata::from_path(path),
        Source::Bytes { bytes, .. } => metadata::from_bytes(bytes),
    });
    let orientation = metadata.orientation;
    let mut decoder = crate::stats::DecodeSource::Native;
    let image = if !jpeg && let Some(image) = full_decode {
        image.clone()
    } else {
        let decoded = crate::image_decode::decode(
            source,
            crate::avif_decoder::Workload::Background,
            false,
            cancel,
        )
        .map_err(|error| CoverAutoAnalysisError::from_image(error, cancel))?;
        decoder = decoded.decoder;
        if decoded.orientation == crate::image_decode::Orientation::Applied {
            decoded.image
        } else {
            apply_orientation(decoded.image, orientation)
        }
    };
    check_cancel(cancel)?;
    Ok(CoverAutoAnalysis {
        native_dims: (image.width(), image.height()),
        shot_exif: metadata.shot_exif,
        image,
        decoder,
        orientation,
        dct_scale_num: None,
    })
}

/// When analysis owns the initial PNG/WebP decode, take metadata from that
/// same decoder/file handle. A reused full raster deliberately does not enter
/// this route: an extra file read solely for G1 is outside the design budget.
fn decode_native_metadata_path(
    path: &Path,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<CoverAutoAnalysis, CoverAutoAnalysisError> {
    let source = Source::Path(path);
    let mut metadata = None;
    let decoded = crate::image_decode::primary(
        source,
        crate::avif_decoder::Workload::Background,
        None,
        cancel.map(Arc::as_ref),
        || {
            let mut decoder = image::ImageReader::open(path)
                .map_err(crate::cover_auto_sample::source_read_error)?
                .with_guessed_format()?
                .into_decoder()?;
            metadata = Some(metadata::from_decoder(&mut decoder));
            image::DynamicImage::from_decoder(decoder)
        },
    )
    .or_else(|primary| {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            return Err(primary);
        }
        crate::image_decode::susie(source, false, cancel.cloned()).map_err(|_| primary)
    })
    .map_err(|error| CoverAutoAnalysisError::from_image(error, cancel))?;
    check_cancel(cancel)?;
    let mut metadata = metadata.unwrap_or_default();
    let decoder = decoded.decoder;
    let image = if decoded.orientation == crate::image_decode::Orientation::Applied {
        decoded.image
    } else {
        // Decoder metadata is for G1 only. File orientation must use exactly
        // the same rexif -> WIC fallback as ordinary/manual file decoding.
        metadata.orientation = read_exif_orientation(path);
        apply_orientation(decoded.image, metadata.orientation)
    };
    Ok(CoverAutoAnalysis {
        native_dims: (image.width(), image.height()),
        shot_exif: metadata.shot_exif,
        image,
        decoder,
        orientation: metadata.orientation,
        dct_scale_num: None,
    })
}

fn pdf_analysis_target(width: f32, height: f32) -> Result<u32, String> {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err("invalid cover analysis PDF page size".into());
    }
    Ok(
        (f64::from(ANALYSIS_DECODE_HEIGHT) * f64::from(width.max(height)) / f64::from(height))
            .ceil()
            .min(f64::from(crate::pdf_loader::PDF_RENDER_MAX_LONG_PX)) as u32,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_cover_auto_analysis_pdf(
    path: &Path,
    read: Option<&crate::pdf_loader::ReadTarget>,
    page: u32,
    password: Option<&str>,
    cancel: Option<&Arc<AtomicBool>>,
    priority: crate::pdf_loader::JobPriority,
    validity: crate::pdf_loader::PdfRenderValidity,
    cancel_policy: crate::pdf_loader::CancelWaitPolicy,
) -> Result<CoverAutoAnalysis, CoverAutoAnalysisError> {
    // Harvested PDF renders remain usable even when the caller was cancelled.
    if cancel_policy == crate::pdf_loader::CancelWaitPolicy::AbortOnCancel {
        check_cancel(cancel)?;
    }
    let sizes = cover_pdf_page_sizes(path, read, password)
        .map_err(|error| CoverAutoAnalysisError::ReadFailed(error.to_string()))?;
    let (width, height) = sizes.get(page as usize).copied().ok_or_else(|| {
        CoverAutoAnalysisError::Decode("cover analysis PDF page size missing".into())
    })?;
    let target = pdf_analysis_target(width, height).map_err(CoverAutoAnalysisError::Decode)?;
    let native_dims = crate::pdf_loader::PdfPageSizePoints { width, height }
        .catalog_layout_dims()
        .ok_or_else(|| {
            CoverAutoAnalysisError::Decode("cover analysis PDF page size out of range".into())
        })?;
    let rendered = render_cover_pdf(
        path,
        read,
        page,
        target,
        password,
        cancel,
        priority,
        validity,
        cancel_policy,
    )
    .map_err(|error| {
        if cancel_policy == crate::pdf_loader::CancelWaitPolicy::AbortOnCancel
            && cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed))
        {
            CoverAutoAnalysisError::Cancelled
        } else {
            CoverAutoAnalysisError::ReadFailed(error.to_string())
        }
    })?;
    Ok(CoverAutoAnalysis {
        image: rendered.image,
        decoder: crate::stats::DecodeSource::Native,
        native_dims,
        shot_exif: false,
        orientation: 1,
        dct_scale_num: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_auto_analysis_pdf_target_fixes_height_and_preserves_render_cap() {
        assert_eq!(pdf_analysis_target(700.0, 1000.0).unwrap(), 1024);
        assert_eq!(pdf_analysis_target(892.0, 512.0).unwrap(), 1784);
        assert_eq!(
            pdf_analysis_target(1600.0, 100.0).unwrap(),
            crate::pdf_loader::PDF_RENDER_MAX_LONG_PX
        );
        for (width, height) in [
            (0.0, 1.0),
            (1.0, 0.0),
            (-1.0, 1.0),
            (f32::NAN, 1.0),
            (1.0, f32::INFINITY),
        ] {
            assert!(pdf_analysis_target(width, height).is_err());
        }
    }

    #[test]
    fn cover_auto_analysis_non_jpeg_reuses_upright_full_raster_and_honors_cancel() {
        let image = image::DynamicImage::new_rgb8(73, 129);
        let source = Source::Bytes {
            bytes: b"not decoded when reusing full raster",
            hint: "page.png",
        };
        let result = decode_cover_auto_analysis_image(source, Some(&image), None).unwrap();
        assert_eq!(result.image.to_rgb8(), image.to_rgb8());
        assert_eq!(result.native_dims, (73, 129));
        let cancel = Arc::new(AtomicBool::new(true));
        assert!(matches!(
            decode_cover_auto_analysis_image(source, Some(&image), Some(&cancel)),
            Err(CoverAutoAnalysisError::Cancelled)
        ));
    }

    #[test]
    fn cover_auto_analysis_preserves_os_failure_and_data_decode_failure() {
        let temp = tempfile::tempdir().unwrap();
        assert!(matches!(
            decode_cover_auto_analysis_image(
                Source::Path(&temp.path().join("missing.jpg")),
                None,
                None
            ),
            Err(CoverAutoAnalysisError::ReadFailed(_))
        ));
        assert!(matches!(
            decode_cover_auto_analysis_image(
                Source::Bytes {
                    bytes: b"broken JPEG",
                    hint: "page.jpg"
                },
                None,
                None
            ),
            Err(CoverAutoAnalysisError::Decode(_))
        ));
        for kind in [
            std::io::ErrorKind::InvalidData,
            std::io::ErrorKind::InvalidInput,
            std::io::ErrorKind::UnexpectedEof,
            std::io::ErrorKind::Unsupported,
        ] {
            assert!(matches!(
                CoverAutoAnalysisError::from_image(
                    image::ImageError::IoError(std::io::Error::new(kind, "data")),
                    None
                ),
                CoverAutoAnalysisError::Decode(_)
            ));
        }
        assert!(matches!(
            CoverAutoAnalysisError::from_image(
                image::ImageError::IoError(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "I/O"
                )),
                None
            ),
            CoverAutoAnalysisError::ReadFailed(_)
        ));
        assert!(matches!(
            CoverAutoAnalysisError::from_image(
                image::ImageError::IoError(crate::cover_auto_sample::source_read_error(
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "source read")
                )),
                None
            ),
            CoverAutoAnalysisError::ReadFailed(_)
        ));
    }

    #[test]
    fn cover_auto_analysis_jpeg_ignores_display_decode_and_path_matches_bytes() {
        let pixels = image::RgbImage::from_fn(2048, 1400, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 239) as u8, 123])
        });
        let bytes = turbojpeg::compress_image(&pixels, 85, turbojpeg::Subsamp::Sub2x2)
            .unwrap()
            .to_vec();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cover.jpg");
        std::fs::write(&path, &bytes).unwrap();
        let small_display = image::DynamicImage::new_rgb8(32, 16);
        let from_bytes = decode_cover_auto_analysis_image(
            Source::Bytes {
                bytes: &bytes,
                hint: "cover.jpg",
            },
            Some(&small_display),
            None,
        )
        .unwrap();
        let from_path = decode_cover_auto_analysis_image(Source::Path(&path), None, None).unwrap();
        assert_eq!(from_bytes.dct_scale_num, Some(6));
        assert_eq!(from_bytes.image.height(), 1050);
        assert_eq!(from_bytes.native_dims, (2048, 1400));
        assert!(!from_bytes.shot_exif);
        assert!(!from_path.shot_exif);
        assert_eq!(from_path.dct_scale_num, from_bytes.dct_scale_num);
        assert_eq!(from_path.image.to_rgb8(), from_bytes.image.to_rgb8());
    }

    #[test]
    fn cover_auto_analysis_jpeg_metadata_and_upright_dimensions_match_path_and_bytes() {
        let pixels = image::RgbImage::from_pixel(90, 60, image::Rgb([20, 80, 180]));
        let jpeg = turbojpeg::compress_image(&pixels, 85, turbojpeg::Subsamp::Sub2x2)
            .unwrap()
            .to_vec();
        let bytes = metadata::tests::jpeg_with_exif(
            &jpeg,
            &metadata::tests::tiff_fixture(6, &[33434], true),
        );
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cover.jpg");
        std::fs::write(&path, &bytes).unwrap();
        for source in [
            Source::Path(&path),
            Source::Bytes {
                bytes: &bytes,
                hint: "cover.jpg",
            },
        ] {
            let analysis = decode_cover_auto_analysis_image(source, None, None).unwrap();
            assert_eq!(analysis.orientation, 6);
            assert!(analysis.shot_exif);
            assert_eq!(analysis.native_dims, (60, 90));
            assert_eq!((analysis.image.width(), analysis.image.height()), (60, 90));
        }
    }

    #[test]
    fn cover_auto_analysis_native_png_webp_metadata_uses_initial_decode_and_owned_bytes() {
        let temp = tempfile::tempdir().unwrap();
        for webp in [false, true] {
            let bytes = metadata::tests::native_with_exif(
                webp,
                metadata::tests::tiff_fixture(6, &[33437], true),
            );
            let name = if webp { "cover.webp" } else { "cover.png" };
            let path = temp.path().join(name);
            std::fs::write(&path, &bytes).unwrap();
            for source in [
                Source::Path(&path),
                Source::Bytes {
                    bytes: &bytes,
                    hint: name,
                },
            ] {
                let analysis = decode_cover_auto_analysis_image(source, None, None).unwrap();
                let legacy = match source {
                    Source::Path(path) => read_exif_orientation(path),
                    Source::Bytes { bytes, .. } => read_exif_orientation_from_bytes(bytes),
                };
                let expected = crate::image_decode::decode(
                    source,
                    crate::avif_decoder::Workload::Background,
                    false,
                    None,
                )
                .unwrap()
                .oriented(source);
                assert_eq!(analysis.orientation, legacy);
                assert!(analysis.shot_exif);
                assert_eq!(analysis.native_dims, (expected.width(), expected.height()));
                assert_eq!(analysis.image.to_rgba8(), expected.to_rgba8());
            }
            let display = image::load_from_memory(&bytes).unwrap();
            let owned = decode_cover_auto_analysis_image(
                Source::Bytes {
                    bytes: &bytes,
                    hint: name,
                },
                Some(&display),
                None,
            )
            .unwrap();
            assert_eq!(owned.native_dims, (90, 60));
            assert_eq!(owned.image.to_rgba8(), display.to_rgba8());
            assert!(owned.shot_exif);
            // A supplied upright PNG/WebP file raster is reused. G1 is skipped
            // because collecting it would require another file read.
            let upright = image::DynamicImage::new_rgb8(60, 90);
            let reused =
                decode_cover_auto_analysis_image(Source::Path(&path), Some(&upright), None)
                    .unwrap();
            assert!(!reused.shot_exif);
            assert_eq!(reused.native_dims, (60, 90));
            assert_eq!(reused.image.to_rgb8(), upright.to_rgb8());
        }
    }
}
