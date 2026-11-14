//! Photo evidence is source metadata, independent of saved display rotation.
use crate::cover_auto_sample::{PageAspectSample, PageDimensionUnits};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PhotoEvidence {
    pub shot_exif: bool,
    /// EXIF-upright native pixels, or actual PDF page-box points.
    pub dimensions: (f64, f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PhotoLikeReason {
    ShotExif,
    TransposedPage,
}

impl PhotoLikeReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ShotExif => "shot-exif",
            Self::TransposedPage => "transposed-page",
        }
    }
}

/// Called after the aspect gates. Reuses every selected sample, including
/// dimensions that the page-aspect estimator subsequently trimmed.
pub(crate) fn photo_like_reason(
    evidence: PhotoEvidence,
    sample: &PageAspectSample,
) -> Option<PhotoLikeReason> {
    if evidence.shot_exif {
        return Some(PhotoLikeReason::ShotExif);
    }
    let aspect = *sample.aspect.as_ref().ok()?;
    let paper_aspect = std::f64::consts::FRAC_1_SQRT_2;
    if !aspect.is_finite() || aspect <= 0.0 || (aspect / paper_aspect - 1.0).abs() <= 0.01 {
        return None;
    }
    let (width, height) = evidence.dimensions;
    if !valid_dimensions((width, height)) {
        return None;
    }
    sample
        .dimensions
        .iter()
        .flatten()
        .copied()
        .filter(|&dims| valid_dimensions(dims))
        .any(|(page_width, page_height)| {
            let equal = |cover: f64, page: f64| match sample.units {
                PageDimensionUnits::Pixels => (cover - page).abs() <= 1.0,
                PageDimensionUnits::PdfPoints => {
                    (cover - page).abs() <= page * 0.005 + f64::EPSILON * cover.max(page)
                }
            };
            equal(height, page_width) && equal(width, page_height)
        })
        .then_some(PhotoLikeReason::TransposedPage)
}

fn valid_dimensions((width, height): (f64, f64)) -> bool {
    width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(units: PageDimensionUnits, dims: &[(f64, f64)]) -> PageAspectSample {
        PageAspectSample {
            aspect: Ok(2.0 / 3.0),
            dimensions: dims.iter().copied().map(Some).collect(),
            units,
        }
    }

    fn evidence(dimensions: (f64, f64)) -> PhotoEvidence {
        PhotoEvidence {
            shot_exif: false,
            dimensions,
        }
    }

    #[test]
    fn cover_auto_photo_pixels_accept_one_pixel_and_keep_all_sample_slots() {
        let mut pages = sample(
            PageDimensionUnits::Pixels,
            &[(599.0, 901.0), (333.0, 500.0)],
        );
        pages.dimensions.insert(0, None);
        assert_eq!(
            photo_like_reason(evidence((900.0, 600.0)), &pages),
            Some(PhotoLikeReason::TransposedPage)
        );
        for dims in [(598.9, 900.0), (600.0, 901.1), (900.0, 600.0)] {
            assert_eq!(
                photo_like_reason(
                    evidence((900.0, 600.0)),
                    &sample(PageDimensionUnits::Pixels, &[dims])
                ),
                None
            );
        }
    }

    #[test]
    fn cover_auto_photo_pdf_preserves_points_and_relative_tolerance() {
        let exact = sample(PageDimensionUnits::PdfPoints, &[(600.0, 900.0)]);
        assert_eq!(
            photo_like_reason(evidence((904.5, 603.0)), &exact),
            Some(PhotoLikeReason::TransposedPage)
        );
        assert_eq!(photo_like_reason(evidence((904.5001, 603.0)), &exact), None);
        let pages = sample(PageDimensionUnits::PdfPoints, &[(600.25, 900.75)]);
        assert_eq!(
            photo_like_reason(evidence((905.2, 603.2)), &pages),
            Some(PhotoLikeReason::TransposedPage)
        );
        assert_eq!(photo_like_reason(evidence((905.3, 600.25)), &pages), None);
        assert_eq!(photo_like_reason(evidence((900.75, 603.3)), &pages), None);
    }

    #[test]
    fn cover_auto_photo_paper_aspect_exempts_image_and_pdf_two_panel_covers() {
        for (units, dims) in [
            (PageDimensionUnits::Pixels, (2480.0, 3508.0)),
            (PageDimensionUnits::PdfPoints, (595.28, 841.89)),
        ] {
            let mut pages = sample(units, &[dims]);
            pages.aspect = Ok(dims.0 / dims.1);
            assert_eq!(photo_like_reason(evidence((dims.1, dims.0)), &pages), None);
            for factor in [0.9901, 1.0099] {
                pages.aspect = Ok(std::f64::consts::FRAC_1_SQRT_2 * factor);
                assert_eq!(photo_like_reason(evidence((dims.1, dims.0)), &pages), None);
            }
            pages.aspect = Ok(std::f64::consts::FRAC_1_SQRT_2 * 1.011);
            assert_eq!(
                photo_like_reason(evidence((dims.1, dims.0)), &pages),
                Some(PhotoLikeReason::TransposedPage)
            );
        }
    }

    #[test]
    fn cover_auto_photo_shooting_metadata_precedes_paper_exemption() {
        let mut pages = sample(PageDimensionUnits::Pixels, &[]);
        pages.aspect = Ok(std::f64::consts::FRAC_1_SQRT_2);
        assert_eq!(
            photo_like_reason(
                PhotoEvidence {
                    shot_exif: true,
                    dimensions: (3508.0, 2480.0)
                },
                &pages
            ),
            Some(PhotoLikeReason::ShotExif)
        );
    }

    #[test]
    fn cover_auto_photo_rejects_invalid_dimensions() {
        for dims in [
            (0.0, 900.0),
            (-600.0, 900.0),
            (f64::NAN, 900.0),
            (600.0, f64::INFINITY),
        ] {
            assert_eq!(
                photo_like_reason(
                    evidence((900.0, 600.0)),
                    &sample(PageDimensionUnits::Pixels, &[dims])
                ),
                None
            );
            assert_eq!(
                photo_like_reason(
                    evidence(dims),
                    &sample(PageDimensionUnits::Pixels, &[(900.0, 600.0)])
                ),
                None
            );
        }
    }
}
