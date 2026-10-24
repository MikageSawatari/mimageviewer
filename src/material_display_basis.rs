//! Immutable fit dimensions accepted with a display material.
//!
//! Legacy views intentionally retain distinct layout contracts. These values
//! are independent of canonical annotation coordinates and sampler allocations.

use crate::{
    fullscreen_specified_scale::{FullscreenFitBasis, LegacySourceLayoutBasis},
    page_dims::DisplayOriginalDimensions,
    source_coverage::SourceCoverage,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MaterialDisplayView {
    Single,
    Spread,
    Continuous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MaterialBasisUnavailable {
    SourceDimensionsUnknown,
    PdfMetadataUnavailable,
    RawDimensionsUnavailable,
    ProducerUnavailable,
    InvalidDimensions,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MaterialLegacyBasis {
    Raster {
        layout_size: egui::Vec2,
        original: DisplayOriginalDimensions,
    },
    /// Existing RAW/PDF/continuous layout units, without a physical-pixel claim.
    SourceLayout {
        basis: LegacySourceLayoutBasis,
    },
    Unavailable(MaterialBasisUnavailable),
}

/// The producer resolves each projection once; consumers only select values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MaterialDisplayBasis {
    single: MaterialLegacyBasis,
    spread: MaterialLegacyBasis,
    continuous: MaterialLegacyBasis,
    specified: Result<FullscreenFitBasis, MaterialBasisUnavailable>,
}

impl MaterialDisplayBasis {
    pub(crate) const fn new(
        single: MaterialLegacyBasis,
        spread: MaterialLegacyBasis,
        continuous: MaterialLegacyBasis,
        specified: Result<FullscreenFitBasis, MaterialBasisUnavailable>,
    ) -> Self {
        Self {
            single,
            spread,
            continuous,
            specified,
        }
    }

    pub(crate) const fn unavailable(reason: MaterialBasisUnavailable) -> Self {
        Self::new(
            MaterialLegacyBasis::Unavailable(reason),
            MaterialLegacyBasis::Unavailable(reason),
            MaterialLegacyBasis::Unavailable(reason),
            Err(reason),
        )
    }

    /// Default for a known producer whose whole sampler is its display raster.
    /// A partial preview instead retains the producer's uncropped raster; never
    /// infer full dimensions by dividing its rounded allocation by coverage.
    /// Source-specific producers replace this default at their acceptance boundary.
    pub(crate) fn from_sampler(size: [usize; 2], coverage: SourceCoverage) -> Self {
        let original = match coverage {
            SourceCoverage::Unknown => {
                return Self::unavailable(MaterialBasisUnavailable::SourceDimensionsUnknown);
            }
            SourceCoverage::Known(geometry) if !geometry.is_valid() => {
                return Self::unavailable(MaterialBasisUnavailable::InvalidDimensions);
            }
            coverage if coverage.is_partial() => coverage.uncropped_raster_dimensions().unwrap(),
            _ => {
                let (Ok(width), Ok(height)) = (u32::try_from(size[0]), u32::try_from(size[1]))
                else {
                    return Self::unavailable(MaterialBasisUnavailable::InvalidDimensions);
                };
                DisplayOriginalDimensions::new(width, height)
            }
        };
        let layout_size = original.as_vec2();
        let aspect_size = if coverage.is_partial() {
            let (width, height) = coverage.coordinate_dims().unwrap().as_tuple();
            egui::vec2(width as f32, height as f32)
        } else {
            layout_size
        };
        let Some(specified) = FullscreenFitBasis::new(aspect_size, layout_size) else {
            return Self::unavailable(MaterialBasisUnavailable::InvalidDimensions);
        };
        let legacy = MaterialLegacyBasis::Raster {
            layout_size,
            original,
        };
        Self::new(legacy, legacy, legacy, Ok(specified))
    }

    pub(crate) const fn legacy(self, view: MaterialDisplayView) -> MaterialLegacyBasis {
        match view {
            MaterialDisplayView::Single => self.single,
            MaterialDisplayView::Spread => self.spread,
            MaterialDisplayView::Continuous => self.continuous,
        }
    }

    pub(crate) const fn specified(self) -> Result<FullscreenFitBasis, MaterialBasisUnavailable> {
        self.specified
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{page_dims::CanonicalPageDimensions, source_coverage::EditPreviewGeometry};

    #[test]
    fn whole_sampler_default_does_not_promote_source_coordinates_to_pixels() {
        let coverage = SourceCoverage::Known(
            EditPreviewGeometry::new(
                CanonicalPageDimensions::new(15000, 20000),
                (6144, 8192),
                (0, 0, 6144, 8192),
            )
            .unwrap(),
        );
        let basis = MaterialDisplayBasis::from_sampler([60, 80], coverage);
        for view in [
            MaterialDisplayView::Single,
            MaterialDisplayView::Spread,
            MaterialDisplayView::Continuous,
        ] {
            assert_eq!(
                basis.legacy(view),
                MaterialLegacyBasis::Raster {
                    layout_size: egui::vec2(60.0, 80.0),
                    original: DisplayOriginalDimensions::new(60, 80),
                }
            );
        }
        assert_eq!(
            basis.specified().unwrap(),
            FullscreenFitBasis::new(egui::vec2(60.0, 80.0), egui::vec2(60.0, 80.0)).unwrap()
        );
    }

    #[test]
    fn partial_sampler_keeps_coordinate_aspect_separate_from_uncropped_pixels() {
        let coverage = SourceCoverage::Known(
            EditPreviewGeometry::new(
                CanonicalPageDimensions::new(15000, 20000),
                (6143, 8192),
                (1000, 500, 3000, 6000),
            )
            .unwrap(),
        );
        let basis = MaterialDisplayBasis::from_sampler([31, 60], coverage);
        assert_eq!(
            basis.legacy(MaterialDisplayView::Single),
            MaterialLegacyBasis::Raster {
                layout_size: egui::vec2(6143.0, 8192.0),
                original: DisplayOriginalDimensions::new(6143, 8192),
            }
        );
        assert_eq!(
            basis.specified().unwrap(),
            FullscreenFitBasis::new(egui::vec2(15000.0, 20000.0), egui::vec2(6143.0, 8192.0),)
                .unwrap()
        );
    }

    #[test]
    fn unknown_and_invalid_producers_remain_explicitly_unavailable() {
        for (size, coverage, reason) in [
            (
                [10, 20],
                SourceCoverage::Unknown,
                MaterialBasisUnavailable::SourceDimensionsUnknown,
            ),
            (
                [0, 20],
                SourceCoverage::FullSource,
                MaterialBasisUnavailable::InvalidDimensions,
            ),
            (
                [10, 20],
                SourceCoverage::Known(EditPreviewGeometry {
                    coordinate_dims: CanonicalPageDimensions::new(100, 200),
                    raster_dims: (10, 20),
                    pixel_bounds: (0, 0, 11, 20),
                }),
                MaterialBasisUnavailable::InvalidDimensions,
            ),
        ] {
            let basis = MaterialDisplayBasis::from_sampler(size, coverage);
            assert_eq!(basis, MaterialDisplayBasis::unavailable(reason));
            assert_eq!(basis.specified(), Err(reason));
        }
    }
}
