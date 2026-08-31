//! SNS のカルーセル投稿向け分割枠の純粋な幾何。
//!
//! グループ矩形は利用者が操作する比率固定矩形で、実際の描画と書き出しには
//! [SnsSplitLayout::frames] が返す整数ピクセルの枠を使う。

use crate::export_crop::CropRect;

pub const MIN_COUNT: u8 = 2;
pub const MAX_COUNT: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnsTarget {
    X,
    Instagram,
}

impl SnsTarget {
    pub const ALL: [Self; 2] = [Self::X, Self::Instagram];

    pub fn label(self) -> &'static str {
        match self {
            Self::X => "X",
            Self::Instagram => "Instagram",
        }
    }

    pub fn stable_key(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Instagram => "instagram",
        }
    }

    pub fn from_stable_key(key: &str) -> Self {
        match key {
            "instagram" => Self::Instagram,
            _ => Self::X,
        }
    }

    /// 枠の比率 (横:縦) を約分した整数の組。
    ///
    /// **投稿先ごとの比率と隙間は、この 2 つの整数組だけが持ち主**にする。f32 の
    /// [Self::frame_aspect] / [Self::seam_ratio] も、枠寸法を決める整数演算も、
    /// すべてここから導出する。同じ数値を別の場所へ書き写すと、片方だけ直した
    /// ときに描画と出力が食い違う。
    pub fn frame_aspect_parts(self) -> (u128, u128) {
        match self {
            Self::X => (3, 4),
            Self::Instagram => (4, 5),
        }
    }

    /// 継ぎ目で捨てる帯の幅を、枠幅に対する比として表した整数の組。
    ///
    /// X の 17/1000 (= 1.7%) の根拠: 2026-09-01 の実測は PC ブラウザ 1.588%、
    /// iOS アプリ 1.869%、モバイル Web 2.652%。隙間の絶対値は環境ごとに違う
    /// (Web 5.33 CSS px / iOS アプリ 4.00 CSS px) ので単一の正解が無い。1.7% なら
    /// PC ブラウザと iOS アプリの誤差がともに 0.4 CSS px 以内に収まる。
    /// 詳細は docs/sns-split-export-plan.md §2.1。
    pub fn seam_ratio_parts(self) -> (u128, u128) {
        match self {
            Self::X => (17, 1000),
            Self::Instagram => (0, 1),
        }
    }

    pub fn frame_aspect(self) -> f32 {
        let (num, den) = self.frame_aspect_parts();
        num as f32 / den as f32
    }

    pub fn seam_ratio(self) -> f32 {
        let (num, den) = self.seam_ratio_parts();
        num as f32 / den as f32
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnsSplitLayout {
    pub target: SnsTarget,
    pub count: u8,
    pub group: CropRect,
}

#[derive(Clone, Copy, Debug)]
struct FrameMetrics {
    width: usize,
    height: usize,
    step: usize,
}

impl SnsSplitLayout {
    /// グループ矩形の比率 (横/縦)。
    pub fn group_aspect(target: SnsTarget, count: u8) -> f32 {
        let count = clamped_count(count);
        let n = f32::from(count);
        target.frame_aspect() * (n + (n - 1.0) * target.seam_ratio())
    }

    /// 画像全体に対して最大サイズかつ中央のグループ矩形を作る。
    pub fn centered_max(target: SnsTarget, count: u8, image_size: [usize; 2]) -> Self {
        let count = clamped_count(count);
        let image_width = image_size[0].max(1) as f32;
        let image_height = image_size[1].max(1) as f32;
        let aspect = Self::group_aspect(target, count);
        let (group_width, group_height) = if image_width / image_height > aspect {
            (image_height * aspect, image_height)
        } else {
            (image_width, image_width / aspect)
        };
        let min_x = (image_width - group_width) * 0.5;
        let min_y = (image_height - group_height) * 0.5;

        Self {
            target,
            count,
            group: CropRect {
                min_x,
                min_y,
                max_x: min_x + group_width,
                max_y: min_y + group_height,
            },
        }
        .clamped(image_size)
    }

    /// 描画と書き出しの正になる、同じ整数ピクセル寸法の枠を返す。
    ///
    /// 公開フィールドから極小のグループを直接渡された場合も、各枠の幅と高さは
    /// 1px 以上、左端同士の間隔は枠幅以上になる。
    pub fn frames(&self) -> Vec<CropRect> {
        let count = clamped_count(self.count);
        let metrics = frame_metrics(self.target, count, self.group.width());
        let x0 = finite_or(self.group.min_x, 0.0).round();
        let y0 = finite_or(self.group.min_y, 0.0).round();

        (0..count)
            .map(|index| {
                let offset = usize::from(index).saturating_mul(metrics.step) as f32;
                let min_x = x0 + offset;
                CropRect {
                    min_x,
                    min_y: y0,
                    max_x: min_x + metrics.width as f32,
                    max_y: y0 + metrics.height as f32,
                }
            })
            .collect()
    }

    /// 丸め後の全枠を囲む最小の矩形を返す。
    pub fn frames_extent(&self) -> CropRect {
        let frames = self.frames();
        let first = frames[0];
        let last = frames[frames.len() - 1];
        CropRect {
            min_x: first.min_x,
            min_y: first.min_y,
            max_x: last.max_x,
            max_y: first.max_y,
        }
    }

    /// 中心と高さを保ちながら投稿先を差し替える。
    pub fn with_target(&self, target: SnsTarget, image_size: [usize; 2]) -> Self {
        self.with_target_and_count(target, self.count, image_size)
    }

    /// 中心と高さを保ちながら枚数を差し替える。
    pub fn with_count(&self, count: u8, image_size: [usize; 2]) -> Self {
        self.with_target_and_count(self.target, count, image_size)
    }

    /// 実際の整数ピクセル枠を画像の整数バジェットから決定的に構成する。
    ///
    /// 結果の group は frames_extent() と同じ矩形にスナップされる。画像が小さく、
    /// 幅・高さ 1px の枠を必要枚数並べることさえできない場合も枠を退化させず、
    /// 最小配置を返す。その配置が画像内かは fits() で判定できる。
    pub fn clamped(&self, image_size: [usize; 2]) -> Self {
        let count = clamped_count(self.count);
        let image_width = image_size[0].max(1);
        let image_height = image_size[1].max(1);
        let center_x = finite_or(
            (self.group.min_x + self.group.max_x) * 0.5,
            image_width as f32 * 0.5,
        );
        let center_y = finite_or(
            (self.group.min_y + self.group.max_y) * 0.5,
            image_height as f32 * 0.5,
        );

        let requested_width = frame_metrics(self.target, count, self.group.width()).width;
        let width_budget = max_frame_width_for_image_width(self.target, count, image_width);
        let height_budget = max_frame_width_for_image_height(self.target, image_height);
        let mut frame_width = requested_width
            .min(width_budget.max(1))
            .min(height_budget.max(1));

        let metrics = loop {
            let metrics = frame_metrics_for_width(self.target, frame_width);
            let total_width = frames_total_width(count, metrics);
            if (total_width <= image_width && metrics.height <= image_height) || frame_width == 1 {
                break metrics;
            }
            frame_width -= 1;
        };

        let total_width = frames_total_width(count, metrics);
        let min_x = axis_origin(center_x, total_width, image_width);
        let min_y = axis_origin(center_y, metrics.height, image_height);

        Self {
            target: self.target,
            count,
            group: CropRect {
                min_x: min_x as f32,
                min_y: min_y as f32,
                max_x: min_x.saturating_add(total_width) as f32,
                max_y: min_y.saturating_add(metrics.height) as f32,
            },
        }
    }

    /// 実際に描画・書き出しする全枠が画像内に収まる場合だけ true を返す。
    ///
    /// 画像寸法の 0 は 1 として扱う。false は、clamped() が返した最小の
    /// 非退化配置さえ物理的に収まらないことを呼び出し側へ伝える。
    pub fn fits(&self, image_size: [usize; 2]) -> bool {
        let image_width = image_size[0].max(1) as f32;
        let image_height = image_size[1].max(1) as f32;
        rect_inside(self.frames_extent(), image_width, image_height)
    }

    fn with_target_and_count(&self, target: SnsTarget, count: u8, image_size: [usize; 2]) -> Self {
        let count = clamped_count(count);
        let center_x = (self.group.min_x + self.group.max_x) * 0.5;
        let center_y = (self.group.min_y + self.group.max_y) * 0.5;
        let group_height = rect_height(self.group);
        let group_width = group_height * Self::group_aspect(target, count);

        Self {
            target,
            count,
            group: CropRect {
                min_x: center_x - group_width * 0.5,
                min_y: center_y - group_height * 0.5,
                max_x: center_x + group_width * 0.5,
                max_y: center_y + group_height * 0.5,
            },
        }
        .clamped(image_size)
    }
}

fn clamped_count(count: u8) -> u8 {
    count.clamp(MIN_COUNT, MAX_COUNT)
}

fn frame_metrics(target: SnsTarget, count: u8, group_width: f32) -> FrameMetrics {
    let count = clamped_count(count);
    let group_width = finite_positive_or(group_width, 1.0);
    // clamped() は丸め済み枠の外接幅へ group をスナップする。隙間の丸めで
    // 理論幅より小さくなる場合も同じ整数枠幅を復元できるよう、最近傍へ丸める。
    // 1px の下限は公開 frames() を通る全経路でここから適用される。
    let width = finite_rounded_usize(group_width as f64 / frame_divisor(target, count));
    frame_metrics_for_width(target, width)
}

fn frame_divisor(target: SnsTarget, count: u8) -> f64 {
    let count = f64::from(clamped_count(count));
    let (seam_num, seam_den) = target.seam_ratio_parts();
    let seam_ratio = seam_num as f64 / seam_den as f64;
    count + (count - 1.0) * seam_ratio
}

fn frame_metrics_for_width(target: SnsTarget, width: usize) -> FrameMetrics {
    let width = width.max(1);
    // 比率も隙間も SnsTarget の整数組から導く。step は (1 + seam) 倍なので
    // 分子へ分母を足した比で一度に丸める。usize の加算を挟むと極端な幅で
    // 桁あふれするため、飽和処理を持つ rounded_ratio の中で完結させる。
    let (aspect_num, aspect_den) = target.frame_aspect_parts();
    let (seam_num, seam_den) = target.seam_ratio_parts();
    let height = rounded_ratio(width, aspect_den, aspect_num);
    let step = rounded_ratio(width, seam_den + seam_num, seam_den);
    FrameMetrics {
        width,
        height: height.max(1),
        step: step.max(width),
    }
}

fn rounded_ratio(value: usize, numerator: u128, denominator: u128) -> usize {
    let rounded = ((value as u128) * numerator + denominator / 2) / denominator;
    usize_from_u128(rounded)
}

fn usize_from_u128(value: u128) -> usize {
    value.min(usize::MAX as u128) as usize
}

fn finite_rounded_usize(value: f64) -> usize {
    if !value.is_finite() || value <= 1.0 {
        1
    } else if value >= usize::MAX as f64 {
        usize::MAX
    } else {
        value.round() as usize
    }
}

fn frames_total_width(count: u8, metrics: FrameMetrics) -> usize {
    usize::from(clamped_count(count) - 1)
        .saturating_mul(metrics.step)
        .saturating_add(metrics.width)
}

fn max_frame_width_for_image_width(target: SnsTarget, count: u8, image_width: usize) -> usize {
    let count = u128::from(clamped_count(count));
    let seam_numerator = match target {
        SnsTarget::X => 17,
        SnsTarget::Instagram => 0,
    };
    let divisor_numerator = 1000 * count + seam_numerator * (count - 1);
    usize_from_u128((image_width as u128) * 1000 / divisor_numerator)
}

fn max_frame_width_for_image_height(target: SnsTarget, image_height: usize) -> usize {
    let image_height = image_height as u128;
    let width = match target {
        SnsTarget::X => (3 * image_height + 1) / 4,
        SnsTarget::Instagram => (4 * image_height + 1) / 5,
    };
    usize_from_u128(width)
}

fn axis_origin(center: f32, occupied: usize, available: usize) -> usize {
    if occupied > available {
        return 0;
    }
    let max_origin = available - occupied;
    let desired = (center as f64 - occupied as f64 * 0.5).round();
    if desired <= 0.0 {
        0
    } else if desired >= max_origin as f64 {
        max_origin
    } else {
        desired as usize
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

fn finite_positive_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

fn rect_height(rect: CropRect) -> f32 {
    finite_positive_or(rect.max_y - rect.min_y, 1.0)
}

fn rect_inside(rect: CropRect, image_width: f32, image_height: f32) -> bool {
    rect.min_x.is_finite()
        && rect.min_y.is_finite()
        && rect.max_x.is_finite()
        && rect.max_y.is_finite()
        && rect.max_x - rect.min_x >= 1.0
        && rect.max_y - rect.min_y >= 1.0
        && rect.min_x >= 0.0
        && rect.min_y >= 0.0
        && rect.max_x <= image_width
        && rect.max_y <= image_height
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1.0e-3;

    fn raw_width(rect: CropRect) -> f32 {
        rect.max_x - rect.min_x
    }

    fn raw_height(rect: CropRect) -> f32 {
        rect.max_y - rect.min_y
    }

    fn center(rect: CropRect) -> [f32; 2] {
        [
            (rect.min_x + rect.max_x) * 0.5,
            (rect.min_y + rect.max_y) * 0.5,
        ]
    }

    fn integer_width(rect: CropRect) -> i64 {
        raw_width(rect).round() as i64
    }

    fn integer_height(rect: CropRect) -> i64 {
        raw_height(rect).round() as i64
    }

    fn integer_gap(left: CropRect, right: CropRect) -> i64 {
        (right.min_x - left.max_x).round() as i64
    }

    fn layout_for_frame_width(target: SnsTarget, count: u8, frame_width: f32) -> SnsSplitLayout {
        let n = f32::from(clamped_count(count));
        let group_width = frame_width * (n + (n - 1.0) * target.seam_ratio());
        SnsSplitLayout {
            target,
            count,
            group: CropRect {
                min_x: 0.0,
                min_y: 0.0,
                max_x: group_width,
                max_y: frame_width / target.frame_aspect(),
            },
        }
    }

    fn assert_rect_inside(rect: CropRect, image_size: [usize; 2]) {
        let image_width = image_size[0].max(1) as f32;
        let image_height = image_size[1].max(1) as f32;
        assert!(rect.min_x >= 0.0, "left edge is outside: {rect:?}");
        assert!(rect.min_y >= 0.0, "top edge is outside: {rect:?}");
        assert!(
            rect.max_x <= image_width,
            "right edge is outside: {rect:?}, image width: {image_width}"
        );
        assert!(
            rect.max_y <= image_height,
            "bottom edge is outside: {rect:?}, image height: {image_height}"
        );
    }

    fn assert_layout_aspect(layout: &SnsSplitLayout) {
        let expected_aspect = SnsSplitLayout::group_aspect(layout.target, layout.count);
        let residual = (raw_width(layout.group) - expected_aspect * raw_height(layout.group)).abs();
        let rounding_allowance =
            (f32::from(clamped_count(layout.count)) - 1.0) * 0.5 + expected_aspect * 0.5;
        assert!(
            residual <= rounding_allowance + EPSILON,
            "residual: {residual}, allowance: {rounding_allowance}, layout: {layout:?}"
        );
    }

    fn assert_non_degenerate(layout: &SnsSplitLayout) {
        let frames = layout.frames();
        for frame in &frames {
            assert!(integer_width(*frame) >= 1, "zero-width frame: {frame:?}");
            assert!(integer_height(*frame) >= 1, "zero-height frame: {frame:?}");
        }
        for pair in frames.windows(2) {
            let step = (pair[1].min_x - pair[0].min_x).round() as i64;
            assert!(step >= integer_width(pair[0]));
        }
    }

    #[test]
    fn group_aspect_matches_every_target_and_count() {
        let cases = [
            (SnsTarget::X, 2, 1.51275),
            (SnsTarget::X, 3, 2.2755),
            (SnsTarget::X, 4, 3.03825),
            (SnsTarget::Instagram, 2, 1.6),
            (SnsTarget::Instagram, 3, 2.4),
            (SnsTarget::Instagram, 4, 3.2),
        ];
        for (target, count, expected) in cases {
            let actual = SnsSplitLayout::group_aspect(target, count);
            assert!((actual - expected).abs() < EPSILON);
        }
        let x_four = SnsSplitLayout::group_aspect(SnsTarget::X, 4);
        assert!((x_four - 3.0383).abs() < EPSILON);
    }

    #[test]
    fn frames_count_matches_count() {
        for target in SnsTarget::ALL {
            for count in MIN_COUNT..=MAX_COUNT {
                let layout = layout_for_frame_width(target, count, 1536.0);
                assert_eq!(layout.frames().len(), usize::from(count));
            }
        }
    }

    #[test]
    fn every_frame_has_exactly_the_same_integer_dimensions() {
        for target in SnsTarget::ALL {
            for count in MIN_COUNT..=MAX_COUNT {
                let frames = layout_for_frame_width(target, count, 1536.0).frames();
                let expected_width = integer_width(frames[0]);
                let expected_height = integer_height(frames[0]);
                for frame in &frames[1..] {
                    assert_eq!(integer_width(*frame), expected_width);
                    assert_eq!(integer_height(*frame), expected_height);
                }
            }
        }
    }

    #[test]
    fn adjacent_gaps_are_constant_and_follow_the_shared_step() {
        for target in SnsTarget::ALL {
            for count in MIN_COUNT..=MAX_COUNT {
                let frames = layout_for_frame_width(target, count, 1536.0).frames();
                let width = integer_width(frames[0]);
                let step = (width as f32 * (1.0 + target.seam_ratio())).round() as i64;
                let expected_gap = step - width;
                for pair in frames.windows(2) {
                    assert_eq!(integer_gap(pair[0], pair[1]), expected_gap);
                }
            }
        }
    }

    #[test]
    fn instagram_frames_are_contiguous() {
        for count in MIN_COUNT..=MAX_COUNT {
            let frames = layout_for_frame_width(SnsTarget::Instagram, count, 1536.0).frames();
            for pair in frames.windows(2) {
                assert_eq!(pair[0].max_x, pair[1].min_x);
            }
        }
    }

    #[test]
    fn x_gap_is_positive_and_close_to_the_preset_ratio() {
        for count in MIN_COUNT..=MAX_COUNT {
            let frames = layout_for_frame_width(SnsTarget::X, count, 1536.0).frames();
            let width = integer_width(frames[0]) as f32;
            for pair in frames.windows(2) {
                let gap = integer_gap(pair[0], pair[1]) as f32;
                assert!(gap > 0.0);
                assert!((gap / width - SnsTarget::X.seam_ratio()).abs() <= 1.0 / width);
            }
        }
    }

    #[test]
    fn x_1536_pixel_frame_uses_about_26_pixels_for_the_seam() {
        let frames = layout_for_frame_width(SnsTarget::X, 4, 1536.0).frames();
        let gap = integer_gap(frames[0], frames[1]);
        assert!((25..=27).contains(&gap));
    }

    #[test]
    fn frames_extent_is_the_union_of_the_rounded_frames() {
        let mut layout = layout_for_frame_width(SnsTarget::X, 4, 1536.0);
        layout.group.min_x += 12.4;
        layout.group.max_x += 12.4;
        layout.group.min_y += 34.6;
        layout.group.max_y += 34.6;
        let frames = layout.frames();
        let extent = layout.frames_extent();
        assert_eq!(extent.min_x, frames[0].min_x);
        assert_eq!(extent.min_y, frames[0].min_y);
        assert_eq!(extent.max_x, frames.last().unwrap().max_x);
        assert_eq!(extent.max_y, frames[0].max_y);
    }

    #[test]
    fn centered_max_stays_inside_and_keeps_the_group_aspect() {
        let image_sizes = [[4000, 1000], [1000, 4000], [1920, 1080]];
        for target in SnsTarget::ALL {
            for count in MIN_COUNT..=MAX_COUNT {
                for image_size in image_sizes {
                    let layout = SnsSplitLayout::centered_max(target, count, image_size);
                    assert!(layout.fits(image_size));
                    assert_rect_inside(layout.group, image_size);
                    assert_layout_aspect(&layout);
                    for frame in layout.frames() {
                        assert_rect_inside(frame, image_size);
                    }
                }
            }
        }
    }

    #[test]
    fn with_count_and_target_keep_center_height_bounds_and_aspect() {
        let image_size = [8000, 6000];
        let layout = SnsSplitLayout {
            target: SnsTarget::X,
            count: 2,
            group: CropRect {
                min_x: 3000.0,
                min_y: 2500.0,
                max_x: 3000.0 + SnsSplitLayout::group_aspect(SnsTarget::X, 2) * 500.0,
                max_y: 3000.0,
            },
        };
        let original_center = center(layout.group);
        let original_height = raw_height(layout.group);
        let layouts = [
            layout.with_count(4, image_size),
            layout.with_target(SnsTarget::Instagram, image_size),
        ];
        for changed in layouts {
            let changed_center = center(changed.group);
            assert!((changed_center[0] - original_center[0]).abs() <= 0.5);
            assert!((changed_center[1] - original_center[1]).abs() <= 0.5);
            assert!((raw_height(changed.group) - original_height).abs() <= 1.0);
            assert!(changed.fits(image_size));
            assert_rect_inside(changed.group, image_size);
            assert_layout_aspect(&changed);
        }
    }

    #[test]
    fn count_is_clamped_at_every_layout_entry_point() {
        for (input, expected) in [(0, 2), (1, 2), (5, 4), (255, 4)] {
            let layout = SnsSplitLayout::centered_max(SnsTarget::X, input, [4000, 3000]);
            assert_eq!(layout.count, expected);
            assert_eq!(layout.frames().len(), usize::from(expected));

            let changed = layout.with_count(input, [4000, 3000]);
            assert_eq!(changed.count, expected);

            let unclamped = SnsSplitLayout {
                target: SnsTarget::X,
                count: input,
                group: layout.group,
            };
            assert_eq!(unclamped.clamped([4000, 3000]).count, expected);
            assert_eq!(unclamped.frames().len(), usize::from(expected));
        }
    }

    #[test]
    fn clamped_moves_overflow_without_resizing() {
        let image_size = [1000, 800];
        let aspect = SnsSplitLayout::group_aspect(SnsTarget::X, 2);
        let layout = SnsSplitLayout {
            target: SnsTarget::X,
            count: 2,
            group: CropRect {
                min_x: 900.0,
                min_y: 300.0,
                max_x: 900.0 + aspect * 200.0,
                max_y: 500.0,
            },
        };
        let original_frames = layout.frames();
        let moved = layout.clamped(image_size);
        let moved_frames = moved.frames();
        assert_eq!(
            integer_width(moved_frames[0]),
            integer_width(original_frames[0])
        );
        assert_eq!(
            integer_height(moved_frames[0]),
            integer_height(original_frames[0])
        );
        assert!(moved.fits(image_size));
        assert_rect_inside(moved.group, image_size);
        assert_layout_aspect(&moved);
    }

    #[test]
    fn clamped_shrinks_oversize_and_keeps_frames_inside() {
        let image_size = [1000, 800];
        let aspect = SnsSplitLayout::group_aspect(SnsTarget::X, 2);
        let layout = SnsSplitLayout {
            target: SnsTarget::X,
            count: 2,
            group: CropRect {
                min_x: -1000.0,
                min_y: -1000.0,
                max_x: -1000.0 + aspect * 2000.0,
                max_y: 1000.0,
            },
        };
        let shrunk = layout.clamped(image_size);
        assert!(raw_height(shrunk.group) < raw_height(layout.group));
        assert!(shrunk.fits(image_size));
        assert_eq!(shrunk.group, shrunk.frames_extent());
        assert_rect_inside(shrunk.group, image_size);
        assert_layout_aspect(&shrunk);
        for frame in shrunk.frames() {
            assert_rect_inside(frame, image_size);
        }
    }

    #[test]
    fn centered_max_accounts_for_extent_rounding_at_image_edge() {
        for image_size in [[122, 40], [863, 284]] {
            let layout = SnsSplitLayout::centered_max(SnsTarget::X, 4, image_size);
            assert!(layout.fits(image_size));
            assert_eq!(layout.group, layout.frames_extent());
            assert_eq!(layout.clamped(image_size), layout);
            assert_layout_aspect(&layout);
            for frame in layout.frames() {
                assert_rect_inside(frame, image_size);
            }
        }
    }

    #[test]
    fn centered_max_stays_inside_across_floor_boundaries() {
        for target in SnsTarget::ALL {
            for count in MIN_COUNT..=MAX_COUNT {
                let aspect = SnsSplitLayout::group_aspect(target, count);
                for image_height in 1..=1024 {
                    let boundary_width = (aspect * image_height as f32).round() as usize;
                    let first_width = boundary_width.saturating_sub(2).max(1);
                    for image_width in first_width..=boundary_width + 2 {
                        let image_size = [image_width, image_height];
                        let layout = SnsSplitLayout::centered_max(target, count, image_size);
                        assert_non_degenerate(&layout);
                        assert_layout_aspect(&layout);
                        if layout.fits(image_size) {
                            assert_rect_inside(layout.group, image_size);
                            for frame in layout.frames() {
                                assert_rect_inside(frame, image_size);
                            }
                        } else {
                            assert!(image_width < usize::from(count));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn with_count_keeps_minimum_frames_non_degenerate_in_a_narrow_image() {
        let image_size = [2, 4000];
        let layout = SnsSplitLayout::centered_max(SnsTarget::X, 4, image_size);
        assert!(!layout.fits(image_size));
        assert_non_degenerate(&layout);
        assert_eq!(raw_height(layout.group), 1.0);

        let changed = layout.with_count(2, image_size);
        assert!(changed.fits(image_size));
        assert_non_degenerate(&changed);
        assert_eq!(raw_height(changed.group), 1.0);
        assert_layout_aspect(&changed);
        for frame in changed.frames() {
            assert_rect_inside(frame, image_size);
        }
    }

    #[test]
    fn extremely_tall_image_produces_in_bounds_frames_without_panicking() {
        let image_size = [100, 4000];
        for target in SnsTarget::ALL {
            for count in MIN_COUNT..=MAX_COUNT {
                let layout = SnsSplitLayout::centered_max(target, count, image_size);
                assert!(layout.fits(image_size));
                assert_rect_inside(layout.group, image_size);
                assert_layout_aspect(&layout);
                for frame in layout.frames() {
                    assert_rect_inside(frame, image_size);
                }
            }
        }
    }

    #[test]
    fn zero_image_dimensions_are_treated_as_one() {
        for target in SnsTarget::ALL {
            let layout = SnsSplitLayout::centered_max(target, MAX_COUNT, [0, 0]);
            assert!(!layout.fits([0, 0]));
            assert_non_degenerate(&layout);
            assert_eq!(layout.group, layout.frames_extent());
            assert_layout_aspect(&layout);
        }
    }

    #[test]
    fn reproduced_tiny_x_four_layout_never_returns_zero_sized_frames() {
        let layout = SnsSplitLayout {
            target: SnsTarget::X,
            count: 4,
            group: CropRect {
                min_x: 10.0,
                min_y: 10.0,
                max_x: 13.0,
                max_y: 11.0,
            },
        };

        assert_non_degenerate(&layout);
        for frame in layout.frames() {
            assert!(raw_width(frame) >= 1.0);
            assert!(raw_height(frame) >= 1.0);
        }
    }

    #[test]
    fn tiny_groups_never_degenerate_for_any_target_or_count() {
        let groups = [
            CropRect {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 0.0,
                max_y: 0.0,
            },
            CropRect {
                min_x: 4.75,
                min_y: 8.25,
                max_x: 5.0,
                max_y: 8.5,
            },
            CropRect {
                min_x: 12.0,
                min_y: 7.0,
                max_x: 11.0,
                max_y: 6.0,
            },
        ];

        for target in SnsTarget::ALL {
            for count in MIN_COUNT..=MAX_COUNT {
                for group in groups {
                    assert_non_degenerate(&SnsSplitLayout {
                        target,
                        count,
                        group,
                    });
                }
            }
        }
    }

    #[test]
    fn image_too_small_reports_not_fitting_without_degenerate_frames() {
        let image_size = [3, 3];
        for target in SnsTarget::ALL {
            let layout = SnsSplitLayout::centered_max(target, 4, image_size);
            assert!(!layout.fits(image_size));
            assert_non_degenerate(&layout);
            assert_eq!(layout.group, layout.frames_extent());
        }
    }

    #[test]
    fn clamped_layout_fits_whenever_the_minimum_frame_row_can_fit() {
        let image_sizes = [[2, 1], [3, 3], [4, 1], [17, 4], [122, 40], [1920, 1080]];
        for target in SnsTarget::ALL {
            for count in MIN_COUNT..=MAX_COUNT {
                for image_size in image_sizes {
                    if image_size[0] < usize::from(count) {
                        continue;
                    }
                    let layout = SnsSplitLayout {
                        target,
                        count,
                        group: CropRect {
                            min_x: -10_000.0,
                            min_y: -10_000.0,
                            max_x: 20_000.0,
                            max_y: 20_000.0,
                        },
                    }
                    .clamped(image_size);
                    assert!(layout.fits(image_size), "{target:?}/{count}/{image_size:?}");
                    assert_eq!(layout.group, layout.frames_extent());
                    assert_non_degenerate(&layout);
                }
            }
        }
    }

    #[test]
    fn extreme_width_and_one_pixel_height_terminate_with_a_canonical_layout() {
        let layout = SnsSplitLayout {
            target: SnsTarget::X,
            count: 4,
            group: CropRect {
                min_x: -1.0e30,
                min_y: -1.0e30,
                max_x: 1.0e30,
                max_y: 1.0e30,
            },
        }
        .clamped([1_000_000, 1]);

        assert!(layout.fits([1_000_000, 1]));
        assert_non_degenerate(&layout);
        assert_eq!(layout.group, layout.frames_extent());
        assert_eq!(layout.clamped([1_000_000, 1]), layout);
    }

    #[test]
    fn stable_keys_round_trip_and_unknown_keys_fall_back_to_x() {
        assert_eq!(SnsTarget::from_stable_key("unknown"), SnsTarget::X);
        for target in SnsTarget::ALL {
            assert_eq!(SnsTarget::from_stable_key(target.stable_key()), target);
        }
        assert_eq!(SnsTarget::X.label(), "X");
        assert_eq!(SnsTarget::Instagram.label(), "Instagram");
        assert_eq!(SnsTarget::X.frame_aspect(), 0.75);
        assert_eq!(SnsTarget::Instagram.frame_aspect(), 0.8);
        assert_eq!(SnsTarget::X.seam_ratio(), 0.017);
        assert_eq!(SnsTarget::Instagram.seam_ratio(), 0.0);
    }
}
