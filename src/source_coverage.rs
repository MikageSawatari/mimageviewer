//! The source region actually present in a display material, independent of current crop settings.
//!
//! Coordinates are EXIF-oriented, before saved rotation and free rotation. PDF page-box layout
//! dimensions are deliberately absent: canonical coordinates and the producer raster have distinct
//! sizes, and only the latter determines the exact floor/ceil crop boundaries.

use crate::page_dims::CanonicalPageDimensions;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditPreviewGeometry {
    #[serde(
        serialize_with = "serialize_coordinate_dims",
        deserialize_with = "deserialize_coordinate_dims"
    )]
    pub coordinate_dims: CanonicalPageDimensions,
    pub raster_dims: (u32, u32),
    /// Actual (x, y, width, height) returned by CropRect::pixel_bounds on the pre-crop raster.
    pub pixel_bounds: (u32, u32, u32, u32),
}

impl EditPreviewGeometry {
    pub fn new(
        coordinate_dims: CanonicalPageDimensions,
        raster_dims: (u32, u32),
        pixel_bounds: (u32, u32, u32, u32),
    ) -> Option<Self> {
        let geometry = Self {
            coordinate_dims,
            raster_dims,
            pixel_bounds,
        };
        geometry.is_valid().then_some(geometry)
    }

    pub fn is_valid(self) -> bool {
        let (cw, ch) = self.coordinate_dims.as_tuple();
        let (rw, rh) = self.raster_dims;
        let (x, y, w, h) = self.pixel_bounds;
        cw > 0
            && ch > 0
            && rw > 0
            && rh > 0
            && w > 0
            && h > 0
            && x.checked_add(w).is_some_and(|end| end <= rw)
            && y.checked_add(h).is_some_and(|end| end <= rh)
    }

    pub fn source_uv_rect(self) -> egui::Rect {
        let (rw, rh) = self.raster_dims;
        let (x, y, w, h) = self.pixel_bounds;
        egui::Rect::from_min_max(
            egui::pos2(x as f32 / rw as f32, y as f32 / rh as f32),
            egui::pos2((x + w) as f32 / rw as f32, (y + h) as f32 / rh as f32),
        )
    }
}

/// FullSource is used by ordinary catalog/raw materials. Newly generated edit previews always
/// carry Known geometry, even when no crop was applied. Only retained legacy rows are Unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceCoverage {
    #[default]
    FullSource,
    Known(EditPreviewGeometry),
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceCoverageUnavailable {
    UnknownWithCurrentCrop,
    InvalidGeometry,
}

impl SourceCoverage {
    pub fn is_partial(self) -> bool {
        match self {
            Self::Known(geometry) if geometry.is_valid() => {
                geometry.pixel_bounds != (0, 0, geometry.raster_dims.0, geometry.raster_dims.1)
            }
            _ => false,
        }
    }

    /// Current legacy compatibility strategy, called through App's single policy entry.
    /// Listing continues displaying retained legacy previews without this restriction.
    pub fn resolve_for_display(
        self,
        current_crop_exists: bool,
    ) -> Result<Self, SourceCoverageUnavailable> {
        match self {
            Self::Unknown if current_crop_exists => {
                Err(SourceCoverageUnavailable::UnknownWithCurrentCrop)
            }
            Self::Unknown => Ok(Self::FullSource),
            Self::Known(geometry) if !geometry.is_valid() => {
                Err(SourceCoverageUnavailable::InvalidGeometry)
            }
            _ => Ok(self),
        }
    }

    pub fn source_uv_rect(self) -> Option<egui::Rect> {
        match self {
            Self::FullSource => Some(egui::Rect::from_min_max(
                egui::Pos2::ZERO,
                egui::pos2(1.0, 1.0),
            )),
            Self::Known(geometry) if geometry.is_valid() => Some(geometry.source_uv_rect()),
            Self::Known(_) | Self::Unknown => None,
        }
    }

    pub fn coordinate_dims(self) -> Option<CanonicalPageDimensions> {
        match self {
            Self::Known(geometry) if geometry.is_valid() => Some(geometry.coordinate_dims),
            _ => None,
        }
    }
}

fn serialize_coordinate_dims<S: serde::Serializer>(
    dims: &CanonicalPageDimensions,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    dims.as_tuple().serialize(serializer)
}

fn deserialize_coordinate_dims<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<CanonicalPageDimensions, D::Error> {
    let (width, height) = <(u32, u32)>::deserialize(deserializer)?;
    Ok(CanonicalPageDimensions::new(width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_compatibility_policy_requires_the_current_crop_to_be_absent() {
        assert_eq!(
            SourceCoverage::Unknown.resolve_for_display(true),
            Err(SourceCoverageUnavailable::UnknownWithCurrentCrop)
        );
        assert_eq!(
            SourceCoverage::Unknown.resolve_for_display(false),
            Ok(SourceCoverage::FullSource)
        );
        assert_eq!(SourceCoverage::Unknown.source_uv_rect(), None);
    }

    #[test]
    fn generated_geometry_is_independent_of_current_crop_and_retains_coordinate_units() {
        let geometry = EditPreviewGeometry::new(
            CanonicalPageDimensions::new(6000, 4000),
            (1200, 800),
            (100, 50, 700, 600),
        )
        .unwrap();
        let coverage = SourceCoverage::Known(geometry);
        assert_eq!(coverage.resolve_for_display(true), Ok(coverage));
        assert_eq!(coverage.resolve_for_display(false), Ok(coverage));
        assert_eq!(coverage.coordinate_dims().unwrap().as_tuple(), (6000, 4000));
        assert_eq!(
            coverage.source_uv_rect(),
            Some(egui::Rect::from_min_max(
                egui::pos2(100.0 / 1200.0, 50.0 / 800.0),
                egui::pos2(800.0 / 1200.0, 650.0 / 800.0)
            ))
        );
    }

    #[test]
    fn geometry_validation_rejects_empty_out_of_bounds_and_overflow() {
        let dims = CanonicalPageDimensions::new(100, 100);
        assert!(EditPreviewGeometry::new(dims, (10, 10), (0, 0, 0, 10)).is_none());
        assert!(EditPreviewGeometry::new(dims, (10, 10), (9, 0, 2, 10)).is_none());
        assert!(EditPreviewGeometry::new(dims, (10, 10), (u32::MAX, 0, 2, 10)).is_none());
    }

    #[test]
    fn partial_classification_does_not_change_uncropped_material_sizing_policy() {
        let dims = CanonicalPageDimensions::new(1200, 800);
        let full = EditPreviewGeometry::new(dims, (12, 8), (0, 0, 12, 8)).unwrap();
        let partial = EditPreviewGeometry::new(dims, (12, 8), (1, 1, 10, 6)).unwrap();
        assert!(!SourceCoverage::Known(full).is_partial());
        assert!(SourceCoverage::Known(partial).is_partial());
        assert!(!SourceCoverage::FullSource.is_partial());
        assert!(!SourceCoverage::Unknown.is_partial());
    }
}
