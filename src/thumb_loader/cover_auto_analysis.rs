//! One display-size-independent decode shared by worker, crop dialog and evaluator.
use super::*;
use crate::image_decode::Source;

const ANALYSIS_DECODE_HEIGHT: u32 = 1024;

pub(crate) struct CoverAutoAnalysis {
    pub image: image::DynamicImage,
    /// EXIF-upright image dimensions; PDF uses page-box catalog units, because
    /// only the ratio enters the detector (not the display raster's rounding).
    pub native_dims: (u32, u32),
    pub orientation: u16,
    pub dct_scale_num: Option<u32>,
}

fn check_cancel(cancel: Option<&Arc<AtomicBool>>) -> Result<(), String> {
    if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
        Err("cover analysis decode cancelled".into())
    } else {
        Ok(())
    }
}

/// `full_decode`, when supplied, is already EXIF-upright and full resolution.
/// A JPEG never reuses it: its fixed DCT selection must be identical in every
/// caller even if the display path happened to decode a different resolution.
pub(crate) fn decode_cover_auto_analysis_image(
    source: Source<'_>,
    full_decode: Option<&image::DynamicImage>,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<CoverAutoAnalysis, String> {
    check_cancel(cancel)?;
    let jpeg = is_jpeg_extension(source.extension());
    if jpeg {
        let owned;
        let bytes = match source {
            Source::Bytes { bytes, .. } if bytes.len() as u64 <= MAX_TURBOJPEG_INPUT_SIZE => {
                Some(bytes)
            }
            Source::Path(path) => {
                let size = std::fs::metadata(path)
                    .map_err(|error| error.to_string())?
                    .len();
                if size <= MAX_TURBOJPEG_INPUT_SIZE {
                    owned = std::fs::read(path).map_err(|error| error.to_string())?;
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
            match decode_jpeg_turbo_scaled_with_axis(bytes, ANALYSIS_DECODE_HEIGHT, None, true) {
                Ok((image, stats)) => {
                    check_cancel(cancel)?;
                    let orientation = read_exif_orientation_from_bytes(bytes);
                    return Ok(CoverAutoAnalysis {
                        image: apply_orientation(image, orientation),
                        native_dims: stats.source_dims_after_exif(orientation),
                        orientation,
                        dct_scale_num: Some(stats.scale_num),
                    });
                }
                // Allocation admission is terminal. Falling back to a full
                // decoder here would bypass the DCT raster safety boundary.
                Err(DctDecodeError::TerminalRejection(error)) => return Err(error),
                Err(DctDecodeError::Fallback(_)) => {}
            }
        }
    }
    let orientation = match source {
        Source::Path(path) => read_exif_orientation(path),
        Source::Bytes { bytes, .. } => read_exif_orientation_from_bytes(bytes),
    };
    let image = if !jpeg && let Some(image) = full_decode {
        image.clone()
    } else {
        crate::image_decode::decode(
            source,
            crate::avif_decoder::Workload::Background,
            false,
            cancel,
        )
        .map_err(|error| error.to_string())?
        .oriented(source)
    };
    check_cancel(cancel)?;
    Ok(CoverAutoAnalysis {
        native_dims: (image.width(), image.height()),
        image,
        orientation,
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
) -> Result<CoverAutoAnalysis, String> {
    let sizes = cover_pdf_page_sizes(path, read, password).map_err(|error| error.to_string())?;
    let (width, height) = sizes
        .get(page as usize)
        .copied()
        .ok_or_else(|| "cover analysis PDF page size missing".to_owned())?;
    let target = pdf_analysis_target(width, height)?;
    let native_dims = crate::pdf_loader::PdfPageSizePoints { width, height }
        .catalog_layout_dims()
        .ok_or_else(|| "cover analysis PDF page size out of range".to_owned())?;
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
    .map_err(|error| error.to_string())?;
    Ok(CoverAutoAnalysis {
        image: rendered.image,
        native_dims,
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
        assert!(decode_cover_auto_analysis_image(source, Some(&image), Some(&cancel)).is_err());
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
        assert_eq!(from_path.dct_scale_num, from_bytes.dct_scale_num);
        assert_eq!(from_path.image.to_rgb8(), from_bytes.image.to_rgb8());
    }
}
