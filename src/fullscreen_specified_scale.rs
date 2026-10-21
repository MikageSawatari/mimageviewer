//! Pure fit inputs. Aspect/layout and physical original pixels have separate owners.
use eframe::egui;

use crate::displayed_image_transform::{FullscreenFitScaleLimits, physical_pixel_scale};
use crate::page_dims::DisplayOriginalDimensions;
use crate::pdf_loader::{PDF_ORIGINAL_RENDER_MIN_LONG_PX, PdfPageContentType};
use crate::settings::{
    FULLSCREEN_SPECIFIED_SCALE_MAX_MAX_TENTHS, FULLSCREEN_SPECIFIED_SCALE_MAX_MIN_TENTHS,
    FullscreenFitMode, FullscreenSpecifiedScale, SpecifiedScaleAxis,
};

/// Full unrotated, untrimmed page basis; never a current AI/PDF output texture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FullscreenFitBasis {
    pub(crate) aspect_size: egui::Vec2,
    pub(crate) original_size: egui::Vec2,
}

impl FullscreenFitBasis {
    pub(crate) fn new(aspect_size: egui::Vec2, original_size: egui::Vec2) -> Option<Self> {
        if !valid_size(aspect_size) || !valid_size(original_size) {
            return None;
        }
        Some(Self {
            aspect_size,
            original_size,
        })
    }

    /// Normalize canonical aspect to the original pixel long edge, not its layout unit.
    pub(crate) fn normalized_original_size(self) -> egui::Vec2 {
        self.aspect_size / self.aspect_size.max_elem() * self.original_size.max_elem()
    }

    pub(crate) fn from_pdf(aspect_size: egui::Vec2, content: PdfPageContentType) -> Option<Self> {
        let long_edge = match content {
            PdfPageContentType::Vector => PDF_ORIGINAL_RENDER_MIN_LONG_PX,
            PdfPageContentType::Raster { w, h } if w > 0 && h > 0 => {
                PDF_ORIGINAL_RENDER_MIN_LONG_PX.min(w.max(h))
            }
            PdfPageContentType::Raster { .. } => return None,
        };
        if !valid_size(aspect_size) {
            return None;
        }
        let original = aspect_size / aspect_size.max_elem() * long_edge as f32;
        Self::new(aspect_size, original)
    }
}

/// A capture carries this whole descriptor; replay never reads live settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FullscreenFitInput {
    Legacy(FullscreenFitMode),
    /// Ordinary image/ZIP display raster's 100% basis, retained with captures.
    /// Canonical annotation coordinates and the current sampler stay separate.
    Raster {
        mode: FullscreenFitMode,
        original: DisplayOriginalDimensions,
    },
    /// Single-page Z uses unrestricted Page fit, retaining canonical aspect.
    CanonicalPage {
        basis: FullscreenFitBasis,
    },
    SpecifiedScale {
        size: FullscreenSpecifiedScale,
        basis: FullscreenFitBasis,
        max_tenths: u32,
    },
}

impl FullscreenFitInput {
    pub(crate) const fn legacy(mode: FullscreenFitMode) -> Self {
        Self::Legacy(mode)
    }

    pub(crate) const fn with_original_display_dimensions(
        self,
        original: DisplayOriginalDimensions,
    ) -> Self {
        match self {
            Self::Legacy(
                mode @ (FullscreenFitMode::Page
                | FullscreenFitMode::Width
                | FullscreenFitMode::Height
                | FullscreenFitMode::Original),
            )
            | Self::Raster {
                mode:
                    mode @ (FullscreenFitMode::Page
                    | FullscreenFitMode::Width
                    | FullscreenFitMode::Height
                    | FullscreenFitMode::Original),
                ..
            } => Self::Raster { mode, original },
            _ => self,
        }
    }

    pub(crate) const fn original_display_dimensions(self) -> Option<DisplayOriginalDimensions> {
        match self {
            Self::Raster { original, .. } => Some(original),
            _ => None,
        }
    }

    pub(crate) const fn from_mode(
        mode: FullscreenFitMode,
        size: FullscreenSpecifiedScale,
        basis: FullscreenFitBasis,
        max_tenths: u32,
    ) -> Self {
        match mode {
            FullscreenFitMode::SpecifiedScale => Self::SpecifiedScale {
                size,
                basis,
                max_tenths,
            },
            _ => Self::Legacy(mode),
        }
    }

    pub(crate) const fn for_single_page_z(self) -> Self {
        match self {
            Self::SpecifiedScale { basis, .. } | Self::CanonicalPage { basis } => {
                Self::CanonicalPage { basis }
            }
            Self::Legacy(_) => Self::Legacy(FullscreenFitMode::Page),
            Self::Raster { original, .. } => Self::Raster {
                mode: FullscreenFitMode::Page,
                original,
            },
        }
    }

    pub(crate) const fn mode(self) -> FullscreenFitMode {
        match self {
            Self::Legacy(mode) | Self::Raster { mode, .. } => mode,
            Self::CanonicalPage { .. } => FullscreenFitMode::Page,
            Self::SpecifiedScale { .. } => FullscreenFitMode::SpecifiedScale,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResolvedSpecifiedScale {
    /// Points per physical-original pixel, before manual zoom.
    pub(crate) scale: f32,
    /// Display-only gap; the persisted gap is never changed.
    pub(crate) gap: f32,
}

/// Resolve an already rotated/trimmed, height-matched unit in O-normalized pixels.
/// The fixed point gap participates in width fit, never in the unit pixel size.
pub(crate) fn resolve_specified_scale(
    viewport_size: egui::Vec2,
    unit_size: egui::Vec2,
    saved_gap: f32,
    size: FullscreenSpecifiedScale,
    limits: FullscreenFitScaleLimits,
    max_tenths: u32,
) -> Option<ResolvedSpecifiedScale> {
    if !valid_size(viewport_size)
        || !valid_size(unit_size)
        || !saved_gap.is_finite()
        || !size.is_valid()
    {
        return None;
    }
    let width = size
        .width
        .active()
        .map(|axis| resolve_axis(axis, viewport_size.x));
    let height = size
        .height
        .active()
        .map(|axis| resolve_axis(axis, viewport_size.y));
    let gap = width.map_or(saved_gap.max(0.0), |width| {
        saved_gap
            .max(0.0)
            .min((width - physical_pixel_scale(limits.pixels_per_point)).max(0.0))
    });
    let requested = match (width, height) {
        (Some(w), Some(h)) => ((w - gap).max(0.0) / unit_size.x).min(h / unit_size.y),
        (Some(w), None) => (w - gap).max(0.0) / unit_size.x,
        (None, Some(h)) => h / unit_size.y,
        // Preserve legacy Page's minimum usable width and its unmodified gap.
        (None, None) => {
            ((viewport_size.x - gap).max(1.0) / unit_size.x).min(viewport_size.y / unit_size.y)
        }
    };
    let scale = apply_specified_scale_limits(requested, limits, max_tenths);
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    Some(ResolvedSpecifiedScale { scale, gap })
}

/// Apply the shared original-pixel limits and new-mode cap to an automatic request.
/// Both-disabled callers may reuse their existing Page fit formula, including its
/// composition-specific subpoint minimum width, before reaching this boundary.
pub(crate) fn apply_specified_scale_limits(
    requested: f32,
    limits: FullscreenFitScaleLimits,
    max_tenths: u32,
) -> f32 {
    let max_upscale = max_tenths.clamp(
        FULLSCREEN_SPECIFIED_SCALE_MAX_MIN_TENTHS,
        FULLSCREEN_SPECIFIED_SCALE_MAX_MAX_TENTHS,
    ) as f32
        / 10.0;
    limits
        .apply(requested)
        .min(max_upscale * physical_pixel_scale(limits.pixels_per_point))
}

fn resolve_axis(axis: SpecifiedScaleAxis, viewport_length: f32) -> f32 {
    match axis {
        SpecifiedScaleAxis::PercentTenths(value) => viewport_length * (value as f32 / 1000.0),
        SpecifiedScaleAxis::LogicalPixels(value) => value as f32,
    }
}

fn valid_size(size: egui::Vec2) -> bool {
    size.x.is_finite() && size.y.is_finite() && size.x > 0.0 && size.y > 0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{
        FULLSCREEN_SPECIFIED_SCALE_MAX_DEFAULT_TENTHS, SpecifiedScaleAxisSetting,
    };

    #[test]
    fn raster_fit_keeps_typed_original_basis_through_capture_policy_and_single_page_z() {
        let original = DisplayOriginalDimensions::new(8192, 6144);
        for mode in [
            FullscreenFitMode::Page,
            FullscreenFitMode::Width,
            FullscreenFitMode::Height,
            FullscreenFitMode::Original,
        ] {
            let fit = FullscreenFitInput::legacy(mode).with_original_display_dimensions(original);
            assert_eq!(fit.mode(), mode);
            assert_eq!(fit.original_display_dimensions(), Some(original));
            assert_eq!(
                fit.for_single_page_z(),
                FullscreenFitInput::Raster {
                    mode: FullscreenFitMode::Page,
                    original
                }
            );
            assert_eq!(
                fit.for_single_page_z().for_single_page_z(),
                fit.for_single_page_z()
            );
            let replacement = DisplayOriginalDimensions::new(4000, 3000);
            assert_eq!(
                fit.with_original_display_dimensions(replacement)
                    .original_display_dimensions(),
                Some(replacement)
            );
        }
        let basis =
            FullscreenFitBasis::new(egui::vec2(600000.0, 800000.0), egui::vec2(3072.0, 4096.0))
                .unwrap();
        for fit in [
            FullscreenFitInput::SpecifiedScale {
                size: FullscreenSpecifiedScale::default(),
                basis,
                max_tenths: 127,
            },
            FullscreenFitInput::CanonicalPage { basis },
            FullscreenFitInput::legacy(FullscreenFitMode::SpecifiedScale),
            FullscreenFitInput::legacy(FullscreenFitMode::MarginFit),
        ] {
            assert_eq!(fit.with_original_display_dimensions(original), fit);
            assert_eq!(fit.original_display_dimensions(), None);
        }
    }

    fn close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= expected.abs().max(1.0) * 1.0e-5,
            "{actual} != {expected}"
        );
    }

    fn fit(
        view: egui::Vec2,
        unit: egui::Vec2,
        size: FullscreenSpecifiedScale,
        ppp: f32,
        no_upscale: bool,
        no_downscale: bool,
    ) -> ResolvedSpecifiedScale {
        resolve_specified_scale(
            view,
            unit,
            0.0,
            size,
            FullscreenFitScaleLimits {
                pixels_per_point: ppp,
                no_upscale,
                no_downscale,
            },
            FULLSCREEN_SPECIFIED_SCALE_MAX_DEFAULT_TENTHS,
        )
        .unwrap()
    }

    #[test]
    fn single_page_z_policy_keeps_basis_and_discards_specified_axes() {
        let basis =
            FullscreenFitBasis::new(egui::vec2(600000.0, 800000.0), egui::vec2(3072.0, 4096.0))
                .unwrap();
        let specified = FullscreenFitInput::SpecifiedScale {
            size: FullscreenSpecifiedScale::default(),
            basis,
            max_tenths: 127,
        };
        assert_eq!(
            specified.for_single_page_z(),
            FullscreenFitInput::CanonicalPage { basis }
        );
        assert_eq!(
            specified.for_single_page_z().for_single_page_z(),
            FullscreenFitInput::CanonicalPage { basis }
        );
        assert_eq!(
            specified.for_single_page_z().mode(),
            FullscreenFitMode::Page
        );
        for mode in [
            FullscreenFitMode::Page,
            FullscreenFitMode::Width,
            FullscreenFitMode::Height,
            FullscreenFitMode::Original,
            FullscreenFitMode::MarginFit,
        ] {
            assert_eq!(
                FullscreenFitInput::legacy(mode).for_single_page_z(),
                FullscreenFitInput::legacy(FullscreenFitMode::Page)
            );
        }
    }

    #[test]
    fn default_and_optional_mixed_axes_preserve_aspect() {
        let view = egui::vec2(1000.0, 800.0);
        let unit = egui::vec2(1000.0, 2000.0);
        close(
            fit(
                view,
                unit,
                FullscreenSpecifiedScale::default(),
                1.0,
                false,
                false,
            )
            .scale,
            0.32,
        );
        for (size, expected) in [
            (
                FullscreenSpecifiedScale {
                    width: SpecifiedScaleAxisSetting::enabled(SpecifiedScaleAxis::PercentTenths(
                        500,
                    )),
                    height: SpecifiedScaleAxisSetting::disabled(SpecifiedScaleAxis::PercentTenths(
                        800,
                    )),
                },
                0.5,
            ),
            (
                FullscreenSpecifiedScale {
                    width: SpecifiedScaleAxisSetting::disabled(SpecifiedScaleAxis::PercentTenths(
                        800,
                    )),
                    height: SpecifiedScaleAxisSetting::enabled(SpecifiedScaleAxis::LogicalPixels(
                        800,
                    )),
                },
                0.4,
            ),
            (
                FullscreenSpecifiedScale {
                    width: SpecifiedScaleAxisSetting::enabled(SpecifiedScaleAxis::PercentTenths(
                        500,
                    )),
                    height: SpecifiedScaleAxisSetting::enabled(SpecifiedScaleAxis::LogicalPixels(
                        800,
                    )),
                },
                0.4,
            ),
            (
                FullscreenSpecifiedScale {
                    width: SpecifiedScaleAxisSetting::disabled(SpecifiedScaleAxis::PercentTenths(
                        800,
                    )),
                    height: SpecifiedScaleAxisSetting::disabled(SpecifiedScaleAxis::PercentTenths(
                        800,
                    )),
                },
                0.4,
            ),
        ] {
            close(fit(view, unit, size, 1.0, false, false).scale, expected);
        }
        // An unspecified width does not secretly add a viewport width constraint.
        close(
            fit(
                view,
                egui::vec2(4000.0, 1000.0),
                FullscreenSpecifiedScale {
                    width: SpecifiedScaleAxisSetting::disabled(SpecifiedScaleAxis::PercentTenths(
                        800,
                    )),
                    height: SpecifiedScaleAxisSetting::enabled(SpecifiedScaleAxis::LogicalPixels(
                        800,
                    )),
                },
                1.0,
                false,
                false,
            )
            .scale,
            0.8,
        );
    }

    #[test]
    fn logical_pixels_are_points_for_all_effective_dpi_and_ui_scales() {
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            let result = fit(
                egui::vec2(1000.0, 1000.0),
                egui::vec2(1000.0, 2000.0),
                FullscreenSpecifiedScale {
                    width: SpecifiedScaleAxisSetting::disabled(SpecifiedScaleAxis::PercentTenths(
                        800,
                    )),
                    height: SpecifiedScaleAxisSetting::enabled(SpecifiedScaleAxis::LogicalPixels(
                        800,
                    )),
                },
                ppp,
                false,
                false,
            );
            close(result.scale * 2000.0, 800.0);
            close(result.scale * 2000.0 * ppp, 800.0 * ppp);
        }
    }

    #[test]
    fn configurable_cap_and_original_limits_cover_all_four_combinations() {
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            for max_tenths in [0, 10, 11, 35, 50, 127, 200, u32::MAX] {
                let cap = max_tenths.clamp(10, 200) as f32 / 10.0;
                for requested_physical in [0.1, 0.999, 1.0, 1.001, 4.999, 5.0, 5.001, 20.0, 50.0] {
                    for (up, down) in [(false, false), (true, false), (false, true), (true, true)] {
                        let mut expected: f32 = requested_physical;
                        if up {
                            expected = expected.min(1.0);
                        }
                        if down {
                            expected = expected.max(1.0);
                        }
                        expected = expected.min(cap);
                        let limits = FullscreenFitScaleLimits {
                            pixels_per_point: ppp,
                            no_upscale: up,
                            no_downscale: down,
                        };
                        close(
                            apply_specified_scale_limits(
                                requested_physical / ppp,
                                limits,
                                max_tenths,
                            ) * ppp,
                            expected,
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn changed_cap_applies_to_active_axes_and_both_disabled_page_fit() {
        let both_disabled = FullscreenSpecifiedScale {
            width: SpecifiedScaleAxisSetting::disabled(SpecifiedScaleAxis::LogicalPixels(1)),
            height: SpecifiedScaleAxisSetting::disabled(SpecifiedScaleAxis::PercentTenths(1)),
        };
        for size in [both_disabled, FullscreenSpecifiedScale::default()] {
            for max_tenths in [10, 35, 50, 127, 200] {
                for ppp in [1.0, 1.25, 1.5, 2.0] {
                    let result = resolve_specified_scale(
                        egui::vec2(1000.0, 1000.0),
                        egui::vec2(10.0, 10.0),
                        40.0,
                        size,
                        FullscreenFitScaleLimits {
                            pixels_per_point: ppp,
                            ..Default::default()
                        },
                        max_tenths,
                    )
                    .unwrap();
                    close(result.scale * ppp, max_tenths as f32 / 10.0);
                    close(result.gap, 40.0);
                }
            }
        }
    }

    #[test]
    fn spread_width_includes_gap_and_small_width_only_adjusts_display_gap() {
        let size = FullscreenSpecifiedScale {
            width: SpecifiedScaleAxisSetting::enabled(SpecifiedScaleAxis::LogicalPixels(800)),
            height: SpecifiedScaleAxisSetting::disabled(SpecifiedScaleAxis::PercentTenths(800)),
        };
        let result = resolve_specified_scale(
            egui::vec2(1000.0, 1000.0),
            egui::vec2(2000.0, 2000.0),
            40.0,
            size,
            FullscreenFitScaleLimits::default(),
            FULLSCREEN_SPECIFIED_SCALE_MAX_DEFAULT_TENTHS,
        )
        .unwrap();
        close(result.scale * 2000.0 + result.gap, 800.0);
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            let tiny = FullscreenSpecifiedScale {
                width: SpecifiedScaleAxisSetting::enabled(SpecifiedScaleAxis::LogicalPixels(1)),
                height: SpecifiedScaleAxisSetting::disabled(SpecifiedScaleAxis::PercentTenths(800)),
            };
            let result = resolve_specified_scale(
                egui::vec2(1000.0, 1000.0),
                egui::vec2(2000.0, 2000.0),
                40.0,
                tiny,
                FullscreenFitScaleLimits {
                    pixels_per_point: ppp,
                    ..Default::default()
                },
                FULLSCREEN_SPECIFIED_SCALE_MAX_DEFAULT_TENTHS,
            )
            .unwrap();
            close(result.gap, (1.0 - 1.0 / ppp).max(0.0));
            close(result.scale * 2000.0 * ppp, 1.0);
        }
        let fallback = resolve_specified_scale(
            egui::vec2(20.0, 1000.0),
            egui::vec2(2000.0, 2000.0),
            40.0,
            FullscreenSpecifiedScale {
                width: SpecifiedScaleAxisSetting::disabled(SpecifiedScaleAxis::PercentTenths(800)),
                height: SpecifiedScaleAxisSetting::disabled(SpecifiedScaleAxis::PercentTenths(800)),
            },
            FullscreenFitScaleLimits::default(),
            FULLSCREEN_SPECIFIED_SCALE_MAX_DEFAULT_TENTHS,
        )
        .unwrap();
        close(fallback.gap, 40.0);
        close(fallback.scale, 1.0 / 2000.0);
    }

    #[test]
    fn range_endpoints_and_invalid_input_have_no_non_finite_fit() {
        for axis in [
            SpecifiedScaleAxis::PercentTenths(1),
            SpecifiedScaleAxis::PercentTenths(10000),
            SpecifiedScaleAxis::LogicalPixels(1),
            SpecifiedScaleAxis::LogicalPixels(32768),
        ] {
            assert!(
                fit(
                    egui::vec2(1000.0, 1000.0),
                    egui::vec2(1000.0, 1000.0),
                    FullscreenSpecifiedScale {
                        width: SpecifiedScaleAxisSetting::enabled(axis),
                        height: SpecifiedScaleAxisSetting::disabled(
                            SpecifiedScaleAxis::PercentTenths(800)
                        )
                    },
                    1.0,
                    false,
                    false
                )
                .scale
                    > 0.0
            );
        }
        for size in [
            egui::vec2(0.0, 1.0),
            egui::vec2(-1.0, 1.0),
            egui::vec2(f32::NAN, 1.0),
            egui::vec2(1.0, f32::INFINITY),
        ] {
            assert!(FullscreenFitBasis::new(size, egui::vec2(1.0, 1.0)).is_none());
            assert!(
                resolve_specified_scale(
                    size,
                    egui::vec2(1.0, 1.0),
                    0.0,
                    FullscreenSpecifiedScale::default(),
                    FullscreenFitScaleLimits::default(),
                    FULLSCREEN_SPECIFIED_SCALE_MAX_DEFAULT_TENTHS,
                )
                .is_none()
            );
        }
    }

    #[test]
    fn canonical_layout_unit_is_not_an_original_pixel_count() {
        let basis = FullscreenFitBasis::new(egui::vec2(20000.0, 10000.0), egui::vec2(800.0, 401.0))
            .unwrap();
        assert_eq!(basis.normalized_original_size(), egui::vec2(800.0, 400.0));
        let scaled = FullscreenFitBasis::new(basis.aspect_size * 3.0, basis.original_size).unwrap();
        assert_eq!(
            basis.normalized_original_size(),
            scaled.normalized_original_size()
        );
    }

    #[test]
    fn pdf_original_is_fixed_by_metadata_including_small_native_rasters() {
        for aspect in [egui::vec2(600.0, 800.0), egui::vec2(600000.0, 800000.0)] {
            assert_eq!(
                FullscreenFitBasis::from_pdf(aspect, PdfPageContentType::Vector)
                    .unwrap()
                    .normalized_original_size(),
                egui::vec2(3072.0, 4096.0)
            );
            for (native, expected) in [(100, 100.0), (2000, 2000.0), (8192, 4096.0)] {
                let basis = FullscreenFitBasis::from_pdf(
                    aspect,
                    PdfPageContentType::Raster {
                        w: native,
                        h: native,
                    },
                )
                .unwrap();
                close(basis.normalized_original_size().max_elem(), expected);
                close(
                    basis.normalized_original_size().x / basis.normalized_original_size().y,
                    0.75,
                );
            }
        }
        assert!(
            FullscreenFitBasis::from_pdf(
                egui::vec2(600.0, 800.0),
                PdfPageContentType::Raster { w: 0, h: 100 }
            )
            .is_none()
        );
    }
}
