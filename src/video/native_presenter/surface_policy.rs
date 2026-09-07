use crate::settings::VideoScaleFilter;
use crate::video::display_metadata::{VideoOrientation, display_dimensions};

use super::{VideoVisualLayout, compute_video_visual_target_rect};

pub(super) const VIDEO_DISPLAY_SURFACE_MAX_DIMENSION: u32 = 8192;
pub(super) const VIDEO_DISPLAY_SURFACE_MAX_PIXELS: u64 = 4096 * 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum VideoScaleFallbackReason {
    DisplaySizeLimitExceeded {
        requested_width: u32,
        requested_height: u32,
        max_dimension: u32,
        max_pixels: u64,
    },
    ResamplePipelineUnavailable {
        error: String,
    },
    PanoramaPipelineUnavailable {
        error: String,
    },
    PanoramaOrientationIntermediateCreationFailed {
        width: u32,
        height: u32,
        error: String,
    },
    ResampleIntermediateCreationFailed {
        width: u32,
        height: u32,
        error: String,
    },
    Anime4kPipelineUnavailable {
        variant: &'static str,
        error: String,
    },
    Anime4kIntermediateCreationFailed {
        variant: &'static str,
        pass_index: usize,
        width: u32,
        height: u32,
        error: String,
    },
    GradeIntermediateCreationFailed {
        width: u32,
        height: u32,
        error: String,
    },
    DisplaySwapChainCreationFailed {
        width: u32,
        height: u32,
        error: String,
    },
    DisplayBackbufferCreationFailed {
        width: u32,
        height: u32,
        error: String,
    },
}

impl VideoScaleFallbackReason {
    pub(super) const fn code(&self) -> &'static str {
        match self {
            Self::DisplaySizeLimitExceeded { .. } => "display_size_limit_exceeded",
            Self::ResamplePipelineUnavailable { .. } => "resample_pipeline_unavailable",
            Self::PanoramaPipelineUnavailable { .. } => "panorama_pipeline_unavailable",
            Self::PanoramaOrientationIntermediateCreationFailed { .. } => {
                "panorama_orientation_intermediate_creation_failed"
            }
            Self::ResampleIntermediateCreationFailed { .. } => {
                "resample_intermediate_creation_failed"
            }
            Self::Anime4kPipelineUnavailable { .. } => "anime4k_pipeline_unavailable",
            Self::Anime4kIntermediateCreationFailed { .. } => {
                "anime4k_intermediate_creation_failed"
            }
            Self::GradeIntermediateCreationFailed { .. } => "grade_intermediate_creation_failed",
            Self::DisplaySwapChainCreationFailed { .. } => "display_swap_chain_creation_failed",
            Self::DisplayBackbufferCreationFailed { .. } => "display_backbuffer_creation_failed",
        }
    }

    pub(super) fn detail(&self) -> String {
        match self {
            Self::DisplaySizeLimitExceeded {
                requested_width,
                requested_height,
                max_dimension,
                max_pixels,
            } => format!(
                "requested={requested_width}x{requested_height} max_dimension={max_dimension} max_pixels={max_pixels}"
            ),
            Self::ResamplePipelineUnavailable { error } => error.clone(),
            Self::PanoramaPipelineUnavailable { error } => error.clone(),
            Self::PanoramaOrientationIntermediateCreationFailed {
                width,
                height,
                error,
            } => format!("size={width}x{height} error={error}"),
            Self::ResampleIntermediateCreationFailed {
                width,
                height,
                error,
            } => format!("size={width}x{height} error={error}"),
            Self::Anime4kPipelineUnavailable { variant, error } => {
                format!("variant={variant} error={error}")
            }
            Self::Anime4kIntermediateCreationFailed {
                variant,
                pass_index,
                width,
                height,
                error,
            } => format!("variant={variant} pass={pass_index} size={width}x{height} error={error}"),
            Self::GradeIntermediateCreationFailed {
                width,
                height,
                error,
            } => format!("size={width}x{height} error={error}"),
            Self::DisplaySwapChainCreationFailed {
                width,
                height,
                error,
            } => format!("size={width}x{height} error={error}"),
            Self::DisplayBackbufferCreationFailed {
                width,
                height,
                error,
            } => format!("size={width}x{height} error={error}"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct VideoSurfaceSizeInput {
    pub filter: VideoScaleFilter,
    pub panorama_active: bool,
    pub video_zoom_active: bool,
    pub source_width: u32,
    pub source_height: u32,
    pub sar_num: u32,
    pub sar_den: u32,
    pub orientation: VideoOrientation,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub layout: VideoVisualLayout,
    pub resizing: bool,
    pub current_surface_width: u32,
    pub current_surface_height: u32,
    pub current_surface_is_display_resolved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum VideoSurfaceSizeDecision {
    LegacySource {
        width: u32,
        height: u32,
    },
    KeepCurrentDuringResize {
        width: u32,
        height: u32,
    },
    DisplayResolution {
        width: u32,
        height: u32,
    },
    FallbackToLegacy {
        width: u32,
        height: u32,
        reason: VideoScaleFallbackReason,
    },
}

pub(super) fn decide_video_surface_size(input: VideoSurfaceSizeInput) -> VideoSurfaceSizeDecision {
    let source_width = input.source_width.max(1);
    let source_height = input.source_height.max(1);
    let full_display_region = input.panorama_active || input.video_zoom_active;
    if !full_display_region && input.filter == VideoScaleFilter::OsDefault {
        return VideoSurfaceSizeDecision::LegacySource {
            width: source_width,
            height: source_height,
        };
    }

    let target_rect =
        compute_video_visual_target_rect(input.viewport_width, input.viewport_height, input.layout);
    let (target_width, target_height) = if full_display_region {
        // Interactive display modes own the full video display region. Fitting
        // the encoded frame here would retain ordinary-playback letterbox bars
        // and force a swap-chain resize on every normal-video zoom step.
        (
            target_rect.width.round().clamp(1.0, u32::MAX as f32) as u32,
            target_rect.height.round().clamp(1.0, u32::MAX as f32) as u32,
        )
    } else {
        let (display_width, display_height) = display_dimensions(
            source_width,
            source_height,
            input.sar_num,
            input.sar_den,
            input.orientation,
        );
        let display_scale = (f64::from(target_rect.width) / display_width)
            .min(f64::from(target_rect.height) / display_height);

        // At physical 1:1 the existing source-sized copy and DComp geometry path is
        // both exact and cheaper than materializing an equivalent shader output.
        if (display_scale - 1.0).abs() <= 1.0e-9 {
            return VideoSurfaceSizeDecision::LegacySource {
                width: source_width,
                height: source_height,
            };
        }
        (
            (display_width * display_scale)
                .round()
                .clamp(1.0, f64::from(u32::MAX)) as u32,
            (display_height * display_scale)
                .round()
                .clamp(1.0, f64::from(u32::MAX)) as u32,
        )
    };

    // Window resize is the only reason to retain a stale display-sized surface.
    // A source change still has to replace a legacy source-sized surface whose
    // dimensions no longer match the new decoded frame.
    let current_matches_source = input.current_surface_width == source_width
        && input.current_surface_height == source_height;
    if input.resizing && (input.current_surface_is_display_resolved || current_matches_source) {
        return VideoSurfaceSizeDecision::KeepCurrentDuringResize {
            width: input.current_surface_width.max(1),
            height: input.current_surface_height.max(1),
        };
    }

    let target_pixels = u64::from(target_width) * u64::from(target_height);
    if target_width > VIDEO_DISPLAY_SURFACE_MAX_DIMENSION
        || target_height > VIDEO_DISPLAY_SURFACE_MAX_DIMENSION
        || target_pixels > VIDEO_DISPLAY_SURFACE_MAX_PIXELS
    {
        return VideoSurfaceSizeDecision::FallbackToLegacy {
            width: source_width,
            height: source_height,
            reason: VideoScaleFallbackReason::DisplaySizeLimitExceeded {
                requested_width: target_width,
                requested_height: target_height,
                max_dimension: VIDEO_DISPLAY_SURFACE_MAX_DIMENSION,
                max_pixels: VIDEO_DISPLAY_SURFACE_MAX_PIXELS,
            },
        };
    }

    VideoSurfaceSizeDecision::DisplayResolution {
        width: target_width,
        height: target_height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::native_presenter::render_core::{HUD_BOTTOM_HEIGHT, HUD_TOP_HEIGHT};

    fn input(filter: VideoScaleFilter) -> VideoSurfaceSizeInput {
        VideoSurfaceSizeInput {
            filter,
            panorama_active: false,
            video_zoom_active: false,
            source_width: 640,
            source_height: 360,
            sar_num: 1,
            sar_den: 1,
            orientation: VideoOrientation::IDENTITY,
            viewport_width: 1280,
            viewport_height: 720,
            layout: VideoVisualLayout::from(false),
            resizing: false,
            current_surface_width: 640,
            current_surface_height: 360,
            current_surface_is_display_resolved: false,
        }
    }

    #[test]
    fn os_default_keeps_the_legacy_source_surface() {
        assert_eq!(
            decide_video_surface_size(input(VideoScaleFilter::OsDefault)),
            VideoSurfaceSizeDecision::LegacySource {
                width: 640,
                height: 360,
            }
        );
    }

    #[test]
    fn panorama_uses_display_resolution_even_with_os_default() {
        let mut case = input(VideoScaleFilter::OsDefault);
        case.panorama_active = true;
        assert_eq!(
            decide_video_surface_size(case),
            VideoSurfaceSizeDecision::DisplayResolution {
                width: 1280,
                height: 720,
            }
        );
    }

    #[test]
    fn panorama_fills_the_display_region_instead_of_letterboxing_two_to_one_source() {
        let mut case = input(VideoScaleFilter::Standard);
        case.panorama_active = true;
        case.source_width = 3840;
        case.source_height = 1920;
        case.viewport_width = 1920;
        case.viewport_height = 1080;
        assert_eq!(
            decide_video_surface_size(case),
            VideoSurfaceSizeDecision::DisplayResolution {
                width: 1920,
                height: 1080,
            }
        );
    }

    #[test]
    fn video_zoom_uses_the_full_display_region_even_with_os_default() {
        let mut case = input(VideoScaleFilter::OsDefault);
        case.video_zoom_active = true;
        case.source_width = 1920;
        case.source_height = 1080;
        case.viewport_width = 1000;
        case.viewport_height = 1000;
        assert_eq!(
            decide_video_surface_size(case),
            VideoSurfaceSizeDecision::DisplayResolution {
                width: 1000,
                height: 1000,
            }
        );
    }

    #[test]
    fn every_shader_filter_uses_the_display_resolution() {
        for filter in [
            VideoScaleFilter::Standard,
            VideoScaleFilter::Sharp,
            VideoScaleFilter::Nearest,
            VideoScaleFilter::Anime,
        ] {
            assert_eq!(
                decide_video_surface_size(input(filter)),
                VideoSurfaceSizeDecision::DisplayResolution {
                    width: 1280,
                    height: 720,
                }
            );
        }
    }

    #[test]
    fn fractional_display_scale_rounds_to_physical_pixels() {
        let mut case = input(VideoScaleFilter::Standard);
        case.viewport_width = 1000;
        case.viewport_height = 1000;
        assert_eq!(
            decide_video_surface_size(case),
            VideoSurfaceSizeDecision::DisplayResolution {
                width: 1000,
                height: 563,
            }
        );
    }

    #[test]
    fn display_surface_upper_limit_has_a_typed_fallback() {
        let mut case = input(VideoScaleFilter::Standard);
        case.viewport_width = 9000;
        case.viewport_height = 9000;
        let VideoSurfaceSizeDecision::FallbackToLegacy { reason, .. } =
            decide_video_surface_size(case)
        else {
            panic!("expected size-limit fallback");
        };
        assert!(matches!(
            reason,
            VideoScaleFallbackReason::DisplaySizeLimitExceeded {
                requested_width: 9000,
                ..
            }
        ));
    }

    #[test]
    fn resizing_reuses_the_current_surface() {
        let mut case = input(VideoScaleFilter::Standard);
        case.viewport_width = 1600;
        case.viewport_height = 900;
        case.resizing = true;
        case.current_surface_width = 1280;
        case.current_surface_height = 720;
        case.current_surface_is_display_resolved = true;
        assert_eq!(
            decide_video_surface_size(case),
            VideoSurfaceSizeDecision::KeepCurrentDuringResize {
                width: 1280,
                height: 720,
            }
        );
    }

    #[test]
    fn physical_one_to_one_preserves_the_source_copy_surface() {
        let mut case = input(VideoScaleFilter::Standard);
        case.viewport_width = 640;
        case.viewport_height = 360;
        assert_eq!(
            decide_video_surface_size(case),
            VideoSurfaceSizeDecision::LegacySource {
                width: 640,
                height: 360,
            }
        );
    }

    #[test]
    fn locked_bars_size_the_display_surface_to_the_reserved_target_rect() {
        let pixels_per_point = 2.0;
        let fixed_bar_gap_px = 5;
        let top_reserved = (HUD_TOP_HEIGHT + fixed_bar_gap_px as f32) * pixels_per_point;
        let bottom_reserved = (HUD_BOTTOM_HEIGHT + fixed_bar_gap_px as f32) * pixels_per_point;

        for (top_bar_locked, bottom_lock, reserved_height) in [
            (true, crate::settings::BottomBarLock::None, top_reserved),
            (
                false,
                crate::settings::BottomBarLock::BarOnly,
                bottom_reserved,
            ),
            (
                true,
                crate::settings::BottomBarLock::BarOnly,
                top_reserved + bottom_reserved,
            ),
        ] {
            let mut case = input(VideoScaleFilter::Standard);
            case.layout = VideoVisualLayout {
                compact: false,
                pixels_per_point,
                top_bar_locked,
                bottom_lock,
                bottom_bar_height: HUD_BOTTOM_HEIGHT,
                seek_strip_visible_points: 0.0,
                fixed_bar_gap_px,
                info_panel_reserved: false,
            };
            let VideoSurfaceSizeDecision::DisplayResolution { height, .. } =
                decide_video_surface_size(case)
            else {
                panic!();
            };

            assert_eq!(height, case.viewport_height - reserved_height as u32);
        }
    }

    #[test]
    fn anime4k_build_and_intermediate_failures_have_distinct_perf_codes() {
        let unavailable = VideoScaleFallbackReason::Anime4kPipelineUnavailable {
            variant: "Anime4K x2 VL",
            error: "CreatePixelShader failed".to_string(),
        };
        assert_eq!(unavailable.code(), "anime4k_pipeline_unavailable");
        assert!(unavailable.detail().contains("variant=Anime4K x2 VL"));

        let allocation = VideoScaleFallbackReason::Anime4kIntermediateCreationFailed {
            variant: "Anime4K x2 VL",
            pass_index: 7,
            width: 1920,
            height: 1080,
            error: "out of memory".to_string(),
        };
        assert_eq!(allocation.code(), "anime4k_intermediate_creation_failed");
        assert!(allocation.detail().contains("pass=7 size=1920x1080"));
    }
}
