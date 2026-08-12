//! Video display metadata shared by playback, auxiliary thumbnails, and capture.
//!
//! FFmpeg exposes container rotation/reflection as a 3x3 display matrix. Playback
//! keeps decoded pixels in their encoded orientation and applies this metadata in
//! DirectComposition. Pixel-producing auxiliary paths use the same normalized
//! value to materialize display-oriented RGBA output.

use ffmpeg_the_third::codec::packet::side_data::Type as PacketSideDataType;

const DISPLAY_MATRIX_SCALE: f64 = 65_536.0;
const ORTHOGONAL_EPSILON: f64 = 0.01;

/// A normalized member of the eight orthogonal image orientations.
///
/// `rotation_degrees` is the clockwise screen-space angle obtained from
/// `atan2(m[1], m[0])`, snapped to 0/90/180/270. `reflected` preserves a
/// negative determinant; together these fields represent all four rotations
/// with and without reflection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VideoOrientation {
    rotation_degrees: u16,
    reflected: bool,
}

impl VideoOrientation {
    pub const IDENTITY: Self = Self {
        rotation_degrees: 0,
        reflected: false,
    };

    pub const fn new(rotation_degrees: u16, reflected: bool) -> Self {
        Self {
            rotation_degrees: match rotation_degrees % 360 {
                90 => 90,
                180 => 180,
                270 => 270,
                _ => 0,
            },
            reflected,
        }
    }

    pub const fn rotation_degrees(self) -> u16 {
        self.rotation_degrees
    }

    pub const fn is_reflected(self) -> bool {
        self.reflected
    }

    /// Integer 2x2 matrix in the same row-vector convention as DComp
    /// `Matrix3x2`: `x' = x*M11 + y*M21`, `y' = x*M12 + y*M22`.
    pub const fn matrix_2x2(self) -> (i8, i8, i8, i8) {
        let (m11, m12) = match self.rotation_degrees {
            90 => (0, 1),
            180 => (-1, 0),
            270 => (0, -1),
            _ => (1, 0),
        };
        let (m21, m22) = if self.reflected {
            (m12, -m11)
        } else {
            (-m12, m11)
        };
        (m11, m12, m21, m22)
    }

    pub const fn swaps_axes(self) -> bool {
        let (m11, _, m21, _) = self.matrix_2x2();
        m11 == 0 && m21 != 0
    }
}

/// `av_display_rotation_get` equivalent for the linear portion of an FFmpeg
/// display matrix. The result uses mIV's clockwise screen-space convention.
/// A degenerate matrix has no meaningful rotation and returns `None`.
pub fn display_rotation_degrees(matrix: &[i32; 9]) -> Option<f64> {
    let m11 = matrix[0] as f64 / DISPLAY_MATRIX_SCALE;
    let m12 = matrix[1] as f64 / DISPLAY_MATRIX_SCALE;
    let scale = m11.hypot(m12);
    if !scale.is_finite() || scale <= f64::EPSILON {
        return None;
    }
    Some(
        (m12 / scale)
            .atan2(m11 / scale)
            .to_degrees()
            .rem_euclid(360.0),
    )
}

/// Normalize an FFmpeg display matrix to one of the eight supported orthogonal
/// orientations. Arbitrary scale/shear and malformed matrices fall back to the
/// identity, matching `normalize_sar`'s fail-safe behavior.
pub fn normalize_display_matrix(matrix: &[i32; 9]) -> VideoOrientation {
    let values = [matrix[0], matrix[1], matrix[3], matrix[4]]
        .map(|value| value as f64 / DISPLAY_MATRIX_SCALE);
    let [mut m11, mut m12, mut m21, mut m22] = values;
    let row_x_scale = m11.hypot(m12);
    let row_y_scale = m21.hypot(m22);
    if !row_x_scale.is_finite()
        || !row_y_scale.is_finite()
        || row_x_scale <= f64::EPSILON
        || row_y_scale <= f64::EPSILON
    {
        return VideoOrientation::IDENTITY;
    }
    m11 /= row_x_scale;
    m12 /= row_x_scale;
    m21 /= row_y_scale;
    m22 /= row_y_scale;

    let dot = m11 * m21 + m12 * m22;
    let determinant = m11 * m22 - m12 * m21;
    if dot.abs() > ORTHOGONAL_EPSILON || (determinant.abs() - 1.0).abs() > ORTHOGONAL_EPSILON {
        return VideoOrientation::IDENTITY;
    }

    let Some(angle) = display_rotation_degrees(matrix) else {
        return VideoOrientation::IDENTITY;
    };
    let quarter_turns = ((angle / 90.0).round() as i32).rem_euclid(4) as u16;
    let orientation = VideoOrientation::new(quarter_turns * 90, determinant < 0.0);
    let (expected_m11, expected_m12, expected_m21, expected_m22) = orientation.matrix_2x2();
    let expected = [expected_m11, expected_m12, expected_m21, expected_m22].map(f64::from);
    if [m11, m12, m21, m22]
        .into_iter()
        .zip(expected)
        .any(|(actual, expected)| (actual - expected).abs() > ORTHOGONAL_EPSILON)
    {
        VideoOrientation::IDENTITY
    } else {
        orientation
    }
}

/// Read `AV_PKT_DATA_DISPLAYMATRIX` from a selected video stream.
pub fn orientation_from_stream(
    stream: &ffmpeg_the_third::format::stream::Stream<'_>,
) -> VideoOrientation {
    stream
        .side_data()
        .find(|side_data| side_data.kind() == PacketSideDataType::DisplayMatrix)
        .and_then(|side_data| display_matrix_from_bytes(side_data.data()))
        .map(|matrix| normalize_display_matrix(&matrix))
        .unwrap_or_default()
}

fn display_matrix_from_bytes(data: &[u8]) -> Option<[i32; 9]> {
    if data.len() < 9 * std::mem::size_of::<i32>() {
        return None;
    }
    let mut matrix = [0_i32; 9];
    for (value, bytes) in matrix.iter_mut().zip(data.chunks_exact(4)) {
        *value = i32::from_ne_bytes(bytes.try_into().ok()?);
    }
    Some(matrix)
}

/// Display-space dimensions after SAR and orientation, before viewport fit.
pub fn display_dimensions(
    width: u32,
    height: u32,
    sar_num: u32,
    sar_den: u32,
    orientation: VideoOrientation,
) -> (f64, f64) {
    let raw_w = width.max(1) as f64 * sar_num.max(1) as f64 / sar_den.max(1) as f64;
    let raw_h = height.max(1) as f64;
    let (m11, m12, m21, m22) = orientation.matrix_2x2();
    let display_w = f64::from(m11.abs()) * raw_w + f64::from(m21.abs()) * raw_h;
    let display_h = f64::from(m12.abs()) * raw_w + f64::from(m22.abs()) * raw_h;
    (display_w, display_h)
}

/// Integer display dimensions for layout and metadata UI.
pub fn display_pixel_dimensions(
    width: u32,
    height: u32,
    sar_num: u32,
    sar_den: u32,
    orientation: VideoOrientation,
) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (width, height);
    }
    let (display_w, display_h) = display_dimensions(width, height, sar_num, sar_den, orientation);
    (
        display_w.round().clamp(1.0, u32::MAX as f64) as u32,
        display_h.round().clamp(1.0, u32::MAX as f64) as u32,
    )
}

/// Fit display-oriented output within a pixel bounding box.
pub fn fit_display_within(
    width: u32,
    height: u32,
    sar_num: u32,
    sar_den: u32,
    orientation: VideoOrientation,
    max_w: u32,
    max_h: u32,
) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (max_w.max(1), max_h.max(1));
    }
    let (display_w, display_h) = display_dimensions(width, height, sar_num, sar_den, orientation);
    let scale = (max_w.max(1) as f64 / display_w).min(max_h.max(1) as f64 / display_h);
    let fitted_w = (display_w * scale).round().max(1.0) as u32;
    let fitted_h = (display_h * scale).round().max(1.0) as u32;
    (fitted_w, fitted_h)
}

pub fn orient_rgba(
    width: u32,
    height: u32,
    rgba: &[u8],
    orientation: VideoOrientation,
) -> Result<(u32, u32, Vec<u8>), String> {
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "invalid RGBA buffer for video orientation".to_string())?;
    if width == 0 || height == 0 || rgba.len() != expected_len {
        return Err("invalid RGBA buffer for video orientation".to_string());
    }
    if orientation == VideoOrientation::IDENTITY {
        return Ok((width, height, rgba.to_vec()));
    }
    orient_non_identity_rgba(width, height, rgba, orientation, expected_len)
}

fn orient_non_identity_rgba(
    width: u32,
    height: u32,
    rgba: &[u8],
    orientation: VideoOrientation,
    expected_len: usize,
) -> Result<(u32, u32, Vec<u8>), String> {
    let (m11, m12, m21, m22) = orientation.matrix_2x2();
    let max_x = width as i64 - 1;
    let max_y = height as i64 - 1;
    let corners = [(0_i64, 0_i64), (max_x, 0), (0, max_y), (max_x, max_y)];
    let transform = |x: i64, y: i64| {
        (
            x * i64::from(m11) + y * i64::from(m21),
            x * i64::from(m12) + y * i64::from(m22),
        )
    };
    let transformed = corners.map(|(x, y)| transform(x, y));
    let min_x = transformed.iter().map(|(x, _)| *x).min().unwrap_or(0);
    let max_x = transformed.iter().map(|(x, _)| *x).max().unwrap_or(0);
    let min_y = transformed.iter().map(|(_, y)| *y).min().unwrap_or(0);
    let max_y = transformed.iter().map(|(_, y)| *y).max().unwrap_or(0);
    let output_w = (max_x - min_x + 1) as u32;
    let output_h = (max_y - min_y + 1) as u32;
    let mut output = vec![0_u8; expected_len];

    for y in 0..height as usize {
        for x in 0..width as usize {
            let (dst_x, dst_y) = transform(x as i64, y as i64);
            let dst_x = (dst_x - min_x) as usize;
            let dst_y = (dst_y - min_y) as usize;
            let src_offset = (y * width as usize + x) * 4;
            let dst_offset = (dst_y * output_w as usize + dst_x) * 4;
            output[dst_offset..dst_offset + 4].copy_from_slice(&rgba[src_offset..src_offset + 4]);
        }
    }
    Ok((output_w, output_h, output))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP_ONE: i32 = 1 << 16;
    const FP_W: i32 = 1 << 30;

    fn matrix(m11: i32, m12: i32, m21: i32, m22: i32) -> [i32; 9] {
        [m11, m12, 0, m21, m22, 0, 0, 0, FP_W]
    }

    #[test]
    fn display_rotation_reads_all_quarter_turns() {
        let cases = [
            (matrix(FP_ONE, 0, 0, FP_ONE), 0),
            (matrix(0, FP_ONE, -FP_ONE, 0), 90),
            (matrix(-FP_ONE, 0, 0, -FP_ONE), 180),
            (matrix(0, -FP_ONE, FP_ONE, 0), 270),
        ];
        for (input, expected) in cases {
            let rotation = display_rotation_degrees(&input).expect("valid rotation");
            assert!((rotation - f64::from(expected)).abs() < 1.0e-6);
            assert_eq!(
                normalize_display_matrix(&input),
                VideoOrientation::new(expected, false)
            );
        }
    }

    #[test]
    fn display_matrix_bytes_read_the_repository_iphone_shape() {
        // testimage/iphone/IMG_1197.MOV の tkhd display matrix。
        // translation (m[6]) は orientation の線形部分に影響しない。
        let expected = [0, FP_ONE, 0, -FP_ONE, 0, 0, 47_185_920, 0, FP_W];
        let bytes: Vec<u8> = expected.into_iter().flat_map(i32::to_ne_bytes).collect();
        let parsed = display_matrix_from_bytes(&bytes).expect("36-byte display matrix");
        assert_eq!(parsed, expected);
        assert_eq!(
            normalize_display_matrix(&parsed),
            VideoOrientation::new(90, false)
        );
    }

    #[test]
    fn display_matrix_preserves_reflections() {
        let cases = [
            matrix(-FP_ONE, 0, 0, FP_ONE),
            matrix(FP_ONE, 0, 0, -FP_ONE),
            matrix(0, FP_ONE, FP_ONE, 0),
            matrix(0, -FP_ONE, -FP_ONE, 0),
        ];
        for input in cases {
            let orientation = normalize_display_matrix(&input);
            assert!(orientation.is_reflected());
            assert_eq!(
                orientation.matrix_2x2(),
                (
                    (input[0] / FP_ONE) as i8,
                    (input[1] / FP_ONE) as i8,
                    (input[3] / FP_ONE) as i8,
                    (input[4] / FP_ONE) as i8,
                )
            );
        }
    }

    #[test]
    fn malformed_or_non_quadrant_matrix_falls_back_to_identity() {
        assert_eq!(
            normalize_display_matrix(&[0; 9]),
            VideoOrientation::IDENTITY
        );
        assert_eq!(
            normalize_display_matrix(&matrix(FP_ONE, FP_ONE, 0, FP_ONE)),
            VideoOrientation::IDENTITY
        );
        let diagonal = (std::f64::consts::FRAC_1_SQRT_2 * FP_ONE as f64) as i32;
        assert_eq!(
            normalize_display_matrix(&matrix(diagonal, diagonal, -diagonal, diagonal)),
            VideoOrientation::IDENTITY
        );
    }

    #[test]
    fn display_dimensions_apply_sar_before_rotation() {
        assert_eq!(
            display_dimensions(720, 480, 97, 80, VideoOrientation::IDENTITY),
            (873.0, 480.0)
        );
        assert_eq!(
            display_dimensions(1280, 720, 1, 1, VideoOrientation::new(90, false)),
            (720.0, 1280.0)
        );
        assert_eq!(
            display_pixel_dimensions(1280, 720, 1, 1, VideoOrientation::new(90, false)),
            (720, 1280)
        );
    }

    #[test]
    fn orient_rgba_rotates_clockwise_and_reflects() {
        let pixels = [
            1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255, 5, 0, 0, 255, 6, 0, 0, 255,
        ];
        let (w, h, rotated) = orient_rgba(3, 2, &pixels, VideoOrientation::new(90, false)).unwrap();
        assert_eq!((w, h), (2, 3));
        let red: Vec<u8> = rotated.chunks_exact(4).map(|pixel| pixel[0]).collect();
        assert_eq!(red, vec![4, 1, 5, 2, 6, 3]);

        let (_, _, reflected) =
            orient_rgba(3, 2, &pixels, VideoOrientation::new(180, true)).unwrap();
        let red: Vec<u8> = reflected.chunks_exact(4).map(|pixel| pixel[0]).collect();
        assert_eq!(red, vec![3, 2, 1, 6, 5, 4]);
    }
}
