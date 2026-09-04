//! Pure view-state and source-rectangle math for normal-video zoom and pan.
//!
//! The native presenter consumes the resulting oriented-source rectangle, but this module owns
//! no GPU, window, or [`crate::app::App`] state. See `docs/video-zoom-pan-plan.md`.

use super::display_metadata::VideoOrientation;

pub const VIDEO_ZOOM_MIN_SCALE: f32 = 1.0;
pub const VIDEO_ZOOM_MAX_SCALE: f32 = 16.0;

const WHEEL_DELTA_PER_NOTCH: f32 = 120.0;
const WHEEL_SCALE_PER_NOTCH: f32 = 1.2;
const MIN_VISIBLE_SOURCE_PIXELS: f32 = 1.0;

/// Source geometry expressed in display-oriented pixel axes.
///
/// `pixel_aspect` converts an oriented source-pixel distance to square-pixel display distance.
/// SAR applies to the encoded X axis, so a quarter turn moves it to oriented Y.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoZoomSourceGeometry {
    pub oriented_size: [f32; 2],
    pub pixel_aspect: [f32; 2],
}

impl VideoZoomSourceGeometry {
    pub fn new(
        width: u32,
        height: u32,
        sar_num: u32,
        sar_den: u32,
        orientation: VideoOrientation,
    ) -> Self {
        let width = width.max(1) as f32;
        let height = height.max(1) as f32;
        let sar = sar_num.max(1) as f32 / sar_den.max(1) as f32;
        if orientation.swaps_axes() {
            Self {
                oriented_size: [height, width],
                pixel_aspect: [1.0, sar],
            }
        } else {
            Self {
                oriented_size: [width, height],
                pixel_aspect: [sar, 1.0],
            }
        }
    }

    fn is_valid(self) -> bool {
        self.oriented_size
            .into_iter()
            .chain(self.pixel_aspect)
            .all(|value| value.is_finite() && value > 0.0)
    }
}

/// Visible rectangle in display-oriented source-pixel coordinates.
///
/// At fit scale one extent can exceed the encoded image and its origin becomes negative. Those
/// coordinates deliberately describe letterbox pixels; the resolver returns black outside the
/// source rather than clamping them to the edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoZoomSourceRect {
    pub origin: [f32; 2],
    pub extent: [f32; 2],
}

impl VideoZoomSourceRect {
    pub fn source_at_region_fraction(self, fraction: [f32; 2]) -> [f32; 2] {
        [
            self.origin[0] + fraction[0] * self.extent[0],
            self.origin[1] + fraction[1] * self.extent[1],
        ]
    }
}

/// Interactive state for one normal-video item.
///
/// Scale is relative to aspect-preserving fit. The center is normalized against the oriented
/// source size so a presenter resize does not move the viewed subject.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoZoomState {
    scale: f32,
    center_normalized: [f32; 2],
}

impl Default for VideoZoomState {
    fn default() -> Self {
        Self {
            scale: VIDEO_ZOOM_MIN_SCALE,
            center_normalized: [0.5, 0.5],
        }
    }
}

impl VideoZoomState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn scale(self) -> f32 {
        self.scale
    }

    pub fn center_normalized(self) -> [f32; 2] {
        self.center_normalized
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Resolve this state to one affine source rectangle for the supplied display region.
    pub fn source_rect(
        self,
        region_size: [f32; 2],
        source: VideoZoomSourceGeometry,
    ) -> Option<VideoZoomSourceRect> {
        let extent = visible_source_extent(self.scale, region_size, source)?;
        let center = [
            self.center_normalized[0] * source.oriented_size[0],
            self.center_normalized[1] * source.oriented_size[1],
        ];
        Some(VideoZoomSourceRect {
            origin: [center[0] - extent[0] * 0.5, center[1] - extent[1] * 0.5],
            extent,
        })
    }

    /// Apply a wheel delta while keeping the source coordinate under the pointer fixed.
    ///
    /// `pointer_in_region` is relative to the top-left of the actual video display region, not the
    /// whole HWND. Clamp can intentionally break the fixed-point invariant at a pan boundary or on
    /// an axis where the image fits and therefore must remain centered.
    pub fn apply_wheel(
        &mut self,
        delta: f32,
        pointer_in_region: [f32; 2],
        region_size: [f32; 2],
        source: VideoZoomSourceGeometry,
    ) -> bool {
        if !delta.is_finite()
            || delta == 0.0
            || !valid_pair(pointer_in_region)
            || !valid_size(region_size)
        {
            return false;
        }
        let Some(before) = self.source_rect(region_size, source) else {
            return false;
        };
        let next_scale = (self.scale * WHEEL_SCALE_PER_NOTCH.powf(delta / WHEEL_DELTA_PER_NOTCH))
            .clamp(VIDEO_ZOOM_MIN_SCALE, VIDEO_ZOOM_MAX_SCALE);
        if next_scale == self.scale {
            return false;
        }
        let fraction = [
            (pointer_in_region[0] / region_size[0]).clamp(0.0, 1.0),
            (pointer_in_region[1] / region_size[1]).clamp(0.0, 1.0),
        ];
        let anchor = before.source_at_region_fraction(fraction);
        let Some(next_extent) = visible_source_extent(next_scale, region_size, source) else {
            return false;
        };
        let next_center = [
            anchor[0] + (0.5 - fraction[0]) * next_extent[0],
            anchor[1] + (0.5 - fraction[1]) * next_extent[1],
        ];
        self.scale = next_scale;
        self.set_center_source_pixels_clamped(next_center, next_extent, source);
        true
    }

    /// Pan by a point delta, moving the image in the same direction as the drag.
    pub fn apply_drag(
        &mut self,
        delta_points: [f32; 2],
        region_size: [f32; 2],
        source: VideoZoomSourceGeometry,
    ) -> bool {
        if !valid_pair(delta_points) || delta_points == [0.0, 0.0] || !valid_size(region_size) {
            return false;
        }
        let Some(rect) = self.source_rect(region_size, source) else {
            return false;
        };
        let previous = self.center_normalized;
        let center = [
            self.center_normalized[0] * source.oriented_size[0]
                - delta_points[0] * rect.extent[0] / region_size[0],
            self.center_normalized[1] * source.oriented_size[1]
                - delta_points[1] * rect.extent[1] / region_size[1],
        ];
        self.set_center_source_pixels_clamped(center, rect.extent, source);
        self.center_normalized != previous
    }

    fn set_center_source_pixels_clamped(
        &mut self,
        mut center: [f32; 2],
        extent: [f32; 2],
        source: VideoZoomSourceGeometry,
    ) {
        for axis in 0..2 {
            let source_axis = source.oriented_size[axis];
            if extent[axis] >= source_axis {
                center[axis] = source_axis * 0.5;
            } else {
                let half_extent = extent[axis] * 0.5;
                let min_center = MIN_VISIBLE_SOURCE_PIXELS - half_extent;
                let max_center = source_axis - MIN_VISIBLE_SOURCE_PIXELS + half_extent;
                center[axis] = center[axis].clamp(min_center, max_center);
            }
            self.center_normalized[axis] = center[axis] / source_axis;
        }
    }
}

fn visible_source_extent(
    scale: f32,
    region_size: [f32; 2],
    source: VideoZoomSourceGeometry,
) -> Option<[f32; 2]> {
    if !scale.is_finite()
        || scale < VIDEO_ZOOM_MIN_SCALE
        || !valid_size(region_size)
        || !source.is_valid()
    {
        return None;
    }
    let display_source_size = [
        source.oriented_size[0] * source.pixel_aspect[0],
        source.oriented_size[1] * source.pixel_aspect[1],
    ];
    let fit =
        (region_size[0] / display_source_size[0]).min(region_size[1] / display_source_size[1]);
    if !fit.is_finite() || fit <= 0.0 {
        return None;
    }
    let divisor = fit * scale;
    Some([
        region_size[0] / divisor / source.pixel_aspect[0],
        region_size[1] / divisor / source.pixel_aspect[1],
    ])
}

fn valid_pair(values: [f32; 2]) -> bool {
    // Pure "finite-value" gate shared by pointer and drag input.
    values.into_iter().all(f32::is_finite)
}

fn valid_size(values: [f32; 2]) -> bool {
    values
        .into_iter()
        .all(|value| value.is_finite() && value > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 0.001;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= EPSILON,
            "actual={actual} expected={expected}"
        );
    }

    fn square_source(size: u32) -> VideoZoomSourceGeometry {
        VideoZoomSourceGeometry::new(size, size, 1, 1, VideoOrientation::IDENTITY)
    }

    #[test]
    fn wheel_keeps_the_source_coordinate_under_the_pointer_fixed() {
        let source = square_source(1000);
        let region = [800.0, 800.0];
        let pointer = [200.0, 600.0];
        let fraction = [pointer[0] / region[0], pointer[1] / region[1]];
        let mut state = VideoZoomState::new();
        let before = state
            .source_rect(region, source)
            .unwrap()
            .source_at_region_fraction(fraction);

        assert!(state.apply_wheel(120.0, pointer, region, source));

        let after = state
            .source_rect(region, source)
            .unwrap()
            .source_at_region_fraction(fraction);
        assert_close(after[0], before[0]);
        assert_close(after[1], before[1]);
    }

    #[test]
    fn pan_clamps_at_both_complete_exit_boundaries() {
        let source = square_source(1000);
        let region = [100.0, 100.0];
        let mut state = VideoZoomState::new();
        assert!(state.apply_wheel(120.0 * 8.0, [50.0, 50.0], region, source));

        assert!(state.apply_drag([100_000.0, 100_000.0], region, source));
        let first = state.source_rect(region, source).unwrap();
        assert_close(first.origin[0] + first.extent[0], 1.0);
        assert_close(first.origin[1] + first.extent[1], 1.0);

        assert!(state.apply_drag([-200_000.0, -200_000.0], region, source));
        let last = state.source_rect(region, source).unwrap();
        assert_close(last.origin[0], 999.0);
        assert_close(last.origin[1], 999.0);
    }

    #[test]
    fn fit_rect_represents_letterbox_as_outside_source_coordinates() {
        let source = VideoZoomSourceGeometry::new(1920, 1080, 1, 1, VideoOrientation::IDENTITY);
        let rect = VideoZoomState::new()
            .source_rect([1000.0, 1000.0], source)
            .unwrap();
        assert_close(rect.origin[0], 0.0);
        assert_close(rect.extent[0], 1920.0);
        assert_close(rect.origin[1], -420.0);
        assert_close(rect.extent[1], 1920.0);
    }

    #[test]
    fn sar_axis_follows_display_orientation() {
        let unrotated = VideoZoomSourceGeometry::new(720, 480, 4, 3, VideoOrientation::IDENTITY);
        let unrotated_rect = VideoZoomState::new()
            .source_rect([1000.0, 1000.0], unrotated)
            .unwrap();
        assert_close(unrotated_rect.origin[0], 0.0);
        assert_close(unrotated_rect.extent[0], 720.0);
        assert_close(unrotated_rect.origin[1], -240.0);
        assert_close(unrotated_rect.extent[1], 960.0);

        let rotated =
            VideoZoomSourceGeometry::new(720, 480, 4, 3, VideoOrientation::new(90, false));
        let rotated_rect = VideoZoomState::new()
            .source_rect([1000.0, 1000.0], rotated)
            .unwrap();
        assert_close(rotated_rect.origin[0], -240.0);
        assert_close(rotated_rect.extent[0], 960.0);
        assert_close(rotated_rect.origin[1], 0.0);
        assert_close(rotated_rect.extent[1], 720.0);
    }

    #[test]
    fn scale_is_clamped_to_fit_and_sixteen_times_fit() {
        let source = square_source(1000);
        let region = [1000.0, 1000.0];
        let mut state = VideoZoomState::new();

        assert!(!state.apply_wheel(-120.0, [500.0, 500.0], region, source));
        assert_close(state.scale(), VIDEO_ZOOM_MIN_SCALE);
        assert!(state.apply_wheel(120.0 * 100.0, [500.0, 500.0], region, source));
        assert_close(state.scale(), VIDEO_ZOOM_MAX_SCALE);
        assert!(!state.apply_wheel(120.0, [500.0, 500.0], region, source));
    }

    #[test]
    fn fitting_axis_recenters_when_zooming_back_to_minimum() {
        let source = square_source(1000);
        let region = [1000.0, 1000.0];
        let mut state = VideoZoomState::new();
        state.apply_wheel(480.0, [500.0, 500.0], region, source);
        state.apply_drag([300.0, -200.0], region, source);
        assert_ne!(state.center_normalized(), [0.5, 0.5]);

        state.apply_wheel(-12_000.0, [500.0, 500.0], region, source);

        assert_close(state.scale(), VIDEO_ZOOM_MIN_SCALE);
        assert_eq!(state.center_normalized(), [0.5, 0.5]);
    }

    #[test]
    fn reset_restores_fit_and_center() {
        let source = square_source(1000);
        let region = [1000.0, 1000.0];
        let mut state = VideoZoomState::new();
        state.apply_wheel(480.0, [250.0, 250.0], region, source);
        state.apply_drag([100.0, 100.0], region, source);

        state.reset();

        assert_eq!(state, VideoZoomState::new());
    }
}
