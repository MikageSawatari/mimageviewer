//! SNS のカルーセル投稿向け分割枠の純粋な幾何。
//!
//! グループ矩形は利用者が操作する自由比率の矩形で、実際の描画と書き出しには
//! [SnsSplitLayout::frames] が返す整数ピクセル境界の枠を使う。

use crate::export_crop::CropRect;

pub const MIN_COUNT: u8 = 2;
pub const MAX_COUNT: u8 = 4;
pub const MAX_SEAM_PERMILLE: u16 = 100;

const PERMILLE_DENOMINATOR: u128 = 1000;

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

    /// 投稿先で一般的な継ぎ目幅を、枠幅に対する比として表した整数の組。
    ///
    /// X の 17/1000 (= 1.7%) の根拠: 2026-09-01 の実測は PC ブラウザ 1.588%、
    /// iOS アプリ 1.869%、モバイル Web 2.652%。隙間の絶対値は環境ごとに違う
    /// (Web 5.33 CSS px / iOS アプリ 4.00 CSS px) ので単一の正解が無い。1.7% なら
    /// PC ブラウザと iOS アプリの誤差がともに 0.4 CSS px 以内に収まる。
    /// 詳細は docs/sns-split-export-plan.md §2.1。
    /// これは新しいレイアウトの既定値だけを決める。現在の値は
    /// [SnsSplitLayout::seam_permille] が所有する。
    pub fn seam_ratio_parts(self) -> (u128, u128) {
        match self {
            Self::X => (17, PERMILLE_DENOMINATOR),
            Self::Instagram => (0, PERMILLE_DENOMINATOR),
        }
    }

    pub fn default_seam_permille(self) -> u16 {
        let (num, den) = self.seam_ratio_parts();
        ((num * PERMILLE_DENOMINATOR / den) as u16).min(MAX_SEAM_PERMILLE)
    }

    /// 投稿先の既定値を f32 で返す。現在のレイアウト値には
    /// [SnsSplitLayout::seam_ratio] を使う。
    pub fn seam_ratio(self) -> f32 {
        let (num, den) = self.seam_ratio_parts();
        num as f32 / den as f32
    }
}

/// パネルで選ぶ1枠の比率。永続化には [SnsFrameRatio::stable_key] を使い、
/// レイアウト自体は結果のグループ矩形だけを所有する。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SnsFrameRatio {
    #[default]
    Free,
    Ratio3x4,
    Ratio4x5,
    Ratio1x1,
}

impl SnsFrameRatio {
    pub const ALL: [Self; 4] = [Self::Free, Self::Ratio3x4, Self::Ratio4x5, Self::Ratio1x1];

    pub fn label(self) -> &'static str {
        match self {
            Self::Free => "自由",
            Self::Ratio3x4 => "3:4",
            Self::Ratio4x5 => "4:5",
            Self::Ratio1x1 => "1:1",
        }
    }

    pub fn stable_key(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Ratio3x4 => "3:4",
            Self::Ratio4x5 => "4:5",
            Self::Ratio1x1 => "1:1",
        }
    }

    pub fn from_stable_key(key: &str) -> Self {
        match key {
            "3:4" => Self::Ratio3x4,
            "4:5" => Self::Ratio4x5,
            "1:1" => Self::Ratio1x1,
            _ => Self::Free,
        }
    }

    /// 1枠の横/縦を整数比で返す。自由では固定値を持たない。
    pub fn frame_ratio_parts(self) -> Option<(u128, u128)> {
        match self {
            Self::Free => None,
            Self::Ratio3x4 => Some((3, 4)),
            Self::Ratio4x5 => Some((4, 5)),
            Self::Ratio1x1 => Some((1, 1)),
        }
    }

    /// 1枠の横/縦。自由では固定値を持たない。
    pub fn frame_aspect(self) -> Option<f32> {
        let (width, height) = self.frame_ratio_parts()?;
        Some(width as f32 / height as f32)
    }

    /// 枠を横一列に並べたグループ矩形の横/縦。
    pub fn group_aspect(self, count: u8, seam_permille: u16) -> Option<f32> {
        let frame_aspect = self.frame_aspect()?;
        let count = f32::from(clamped_count(count));
        let seam_ratio =
            f32::from(clamped_seam_permille(seam_permille)) / PERMILLE_DENOMINATOR as f32;
        Some(frame_aspect * (count + (count - 1.0) * seam_ratio))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnsSplitLayout {
    pub target: SnsTarget,
    pub count: u8,
    /// 枠幅に対する継ぎ目の千分率 (0..=100)。
    pub seam_permille: u16,
    pub group: CropRect,
}

#[derive(Clone, Copy, Debug)]
struct FrameMetrics {
    extent_width: usize,
    height: usize,
    total_units: u128,
    step_units: u128,
}

impl SnsSplitLayout {
    pub fn seam_ratio_parts(self) -> (u128, u128) {
        (
            u128::from(clamped_seam_permille(self.seam_permille)),
            PERMILLE_DENOMINATOR,
        )
    }

    pub fn seam_ratio(self) -> f32 {
        f32::from(clamped_seam_permille(self.seam_permille)) / PERMILLE_DENOMINATOR as f32
    }

    /// 画像全体に対して最大サイズかつ中央のグループ矩形を作る。
    pub fn centered_max(target: SnsTarget, count: u8, image_size: [usize; 2]) -> Self {
        let count = clamped_count(count);

        Self {
            target,
            count,
            seam_permille: target.default_seam_permille(),
            group: CropRect::full(image_size[0], image_size[1]),
        }
        .clamped(image_size)
    }

    /// 描画と書き出しの正になる、整数ピクセル境界の枠を返す。
    ///
    /// 公開フィールドから極小のグループを直接渡された場合も、各枠の幅と高さは
    /// 1px 以上になる。割り切れない幅は累積境界を丸めて全枠へ配るため、枠幅と
    /// 継ぎ目幅はそれぞれ最大 1px の差を持ち得るが、欠落や重複は作らない。
    pub fn frames(&self) -> Vec<CropRect> {
        let count = clamped_count(self.count);
        let seam_permille = clamped_seam_permille(self.seam_permille);
        let metrics = frame_metrics(
            count,
            seam_permille,
            self.group.width(),
            self.group.height(),
        );
        let x0 = finite_or(self.group.min_x, 0.0).round();
        let y0 = finite_or(self.group.min_y, 0.0).round();

        (0..count)
            .map(|index| {
                let start_units = u128::from(index).saturating_mul(metrics.step_units);
                let end_units = start_units.saturating_add(PERMILLE_DENOMINATOR);
                let min_x = x0
                    + partition_offset(metrics.extent_width, start_units, metrics.total_units)
                        as f32;
                let max_x = x0
                    + partition_offset(metrics.extent_width, end_units, metrics.total_units) as f32;
                CropRect {
                    min_x,
                    min_y: y0,
                    max_x,
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

    /// グループ矩形を保ちながら投稿先を差し替え、継ぎ目を投稿先の既定へ戻す。
    pub fn with_target(&self, target: SnsTarget, image_size: [usize; 2]) -> Self {
        Self {
            target,
            seam_permille: target.default_seam_permille(),
            ..*self
        }
        .clamped(image_size)
    }

    /// グループ矩形と継ぎ目を保ちながら枚数を差し替える。
    pub fn with_count(&self, count: u8, image_size: [usize; 2]) -> Self {
        Self { count, ..*self }.clamped(image_size)
    }

    /// グループ矩形を保ちながら継ぎ目を差し替える。
    pub fn with_seam_permille(&self, seam_permille: u16, image_size: [usize; 2]) -> Self {
        Self {
            seam_permille,
            ..*self
        }
        .clamped(image_size)
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

        let seam_permille = clamped_seam_permille(self.seam_permille);
        let requested = frame_metrics(
            count,
            seam_permille,
            self.group.width(),
            self.group.height(),
        );
        let minimum_width = usize::from(count);
        let extent_width = if image_width < minimum_width {
            minimum_width
        } else {
            requested.extent_width.clamp(minimum_width, image_width)
        };
        let extent_height = requested.height.clamp(1, image_height);
        let metrics = frame_metrics_for_width(count, seam_permille, extent_width, extent_height);

        let min_x = axis_origin(center_x, metrics.extent_width, image_width);
        let min_y = axis_origin(center_y, metrics.height, image_height);

        Self {
            target: self.target,
            count,
            seam_permille,
            group: CropRect {
                min_x: min_x as f32,
                min_y: min_y as f32,
                max_x: min_x.saturating_add(metrics.extent_width) as f32,
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
}

fn clamped_count(count: u8) -> u8 {
    count.clamp(MIN_COUNT, MAX_COUNT)
}

pub fn clamped_seam_permille(seam_permille: u16) -> u16 {
    seam_permille.min(MAX_SEAM_PERMILLE)
}

fn frame_metrics(
    count: u8,
    seam_permille: u16,
    group_width: f32,
    group_height: f32,
) -> FrameMetrics {
    let count = clamped_count(count);
    let extent_width =
        finite_rounded_usize(finite_positive_or(group_width, 1.0) as f64).max(usize::from(count));
    let height = finite_rounded_usize(finite_positive_or(group_height, 1.0) as f64);
    frame_metrics_for_width(count, seam_permille, extent_width, height)
}

fn frame_metrics_for_width(
    count: u8,
    seam_permille: u16,
    extent_width: usize,
    height: usize,
) -> FrameMetrics {
    let count = clamped_count(count);
    let seam_units = u128::from(clamped_seam_permille(seam_permille));
    let count_units = u128::from(count);
    let total_units = PERMILLE_DENOMINATOR
        .saturating_mul(count_units)
        .saturating_add(seam_units.saturating_mul(count_units - 1));
    FrameMetrics {
        extent_width: extent_width.max(usize::from(count)),
        height: height.max(1),
        total_units,
        step_units: PERMILLE_DENOMINATOR.saturating_add(seam_units),
    }
}

fn partition_offset(extent_width: usize, units: u128, total_units: u128) -> usize {
    rounded_ratio(extent_width, units.min(total_units), total_units).min(extent_width)
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

    fn raw_width(rect: CropRect) -> usize {
        (rect.max_x - rect.min_x).round() as usize
    }

    fn raw_height(rect: CropRect) -> usize {
        (rect.max_y - rect.min_y).round() as usize
    }

    fn gap_width(left: CropRect, right: CropRect) -> usize {
        (right.min_x - left.max_x).round() as usize
    }

    fn layout_for_extent(
        target: SnsTarget,
        count: u8,
        seam_permille: u16,
        width: usize,
        height: usize,
    ) -> SnsSplitLayout {
        SnsSplitLayout {
            target,
            count,
            seam_permille,
            group: CropRect::full(width, height),
        }
        .clamped([width, height])
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

    fn assert_partition_invariants(layout: SnsSplitLayout) {
        let frames = layout.frames();
        assert_eq!(frames.len(), usize::from(clamped_count(layout.count)));
        assert_eq!(layout.frames_extent(), layout.group);

        let widths = frames.iter().copied().map(raw_width).collect::<Vec<_>>();
        let heights = frames.iter().copied().map(raw_height).collect::<Vec<_>>();
        let gaps = frames
            .windows(2)
            .map(|pair| gap_width(pair[0], pair[1]))
            .collect::<Vec<_>>();

        assert!(widths.iter().all(|&width| width >= 1));
        assert!(heights.iter().all(|&height| height >= 1));
        assert!(heights.iter().all(|&height| height == heights[0]));
        assert!(widths.iter().max().unwrap() - widths.iter().min().unwrap() <= 1);
        if !gaps.is_empty() {
            assert!(gaps.iter().max().unwrap() - gaps.iter().min().unwrap() <= 1);
        }
        assert_eq!(
            widths.iter().sum::<usize>() + gaps.iter().sum::<usize>(),
            raw_width(layout.group)
        );
    }

    #[test]
    fn measured_full_image_splits_cover_every_source_pixel() {
        let three = SnsSplitLayout::centered_max(SnsTarget::X, 3, [832, 1216])
            .with_seam_permille(0, [832, 1216]);
        let three_frames = three.frames();
        assert_eq!(
            three_frames
                .iter()
                .copied()
                .map(raw_width)
                .collect::<Vec<_>>(),
            vec![277, 278, 277]
        );
        assert!(
            three_frames
                .iter()
                .copied()
                .all(|frame| raw_height(frame) == 1216)
        );
        assert_eq!(three.group, CropRect::full(832, 1216));
        assert_eq!(three.frames_extent(), three.group);
        assert_eq!(three_frames[0].min_x, 0.0);
        assert_eq!(three_frames[0].max_x, three_frames[1].min_x);
        assert_eq!(three_frames[1].max_x, three_frames[2].min_x);
        assert_eq!(three_frames[2].max_x, 832.0);

        let four = SnsSplitLayout::centered_max(SnsTarget::X, 4, [1216, 832])
            .with_seam_permille(0, [1216, 832]);
        let four_frames = four.frames();
        assert!(
            four_frames
                .iter()
                .copied()
                .all(|frame| (raw_width(frame), raw_height(frame)) == (304, 832))
        );
        assert_eq!(four.group, CropRect::full(1216, 832));
        assert_eq!(four.frames_extent(), four.group);
    }

    #[test]
    fn zero_seam_partitions_the_group_without_gaps_or_overlap() {
        for count in MIN_COUNT..=MAX_COUNT {
            for width in usize::from(count)..=1031 {
                let layout = layout_for_extent(SnsTarget::X, count, 0, width, 79);
                let frames = layout.frames();
                for pair in frames.windows(2) {
                    assert_eq!(pair[0].max_x, pair[1].min_x);
                }
                assert_partition_invariants(layout);
            }
        }
    }

    #[test]
    fn seam_values_follow_the_frame_width_ratio_exactly_when_divisible() {
        for seam_permille in [0, 17, 100] {
            for count in MIN_COUNT..=MAX_COUNT {
                let width =
                    usize::from(count) * 1000 + usize::from(count - 1) * seam_permille as usize;
                let layout = layout_for_extent(SnsTarget::X, count, seam_permille, width, 777);
                let frames = layout.frames();
                assert!(
                    frames
                        .iter()
                        .copied()
                        .all(|frame| (raw_width(frame), raw_height(frame)) == (1000, 777))
                );
                assert!(
                    frames
                        .windows(2)
                        .all(|pair| gap_width(pair[0], pair[1]) == seam_permille as usize)
                );
                assert_eq!(layout.seam_ratio(), seam_permille as f32 / 1000.0);
                assert_eq!(layout.seam_ratio_parts(), (u128::from(seam_permille), 1000));
                assert_partition_invariants(layout);
            }
        }
    }

    #[test]
    fn cumulative_rounding_keeps_arbitrary_width_error_to_one_pixel() {
        for seam_permille in [0, 1, 17, 53, 100] {
            for count in MIN_COUNT..=MAX_COUNT {
                for width in usize::from(count)..=2049 {
                    assert_partition_invariants(layout_for_extent(
                        SnsTarget::X,
                        count,
                        seam_permille,
                        width,
                        313,
                    ));
                }
            }
        }
    }

    #[test]
    fn centered_max_uses_the_whole_image_at_any_aspect() {
        for target in SnsTarget::ALL {
            for count in MIN_COUNT..=MAX_COUNT {
                for image_size in [[4000, 1000], [1000, 4000], [832, 1216], [1920, 1080]] {
                    let layout = SnsSplitLayout::centered_max(target, count, image_size);
                    assert_eq!(layout.group, CropRect::full(image_size[0], image_size[1]));
                    assert_eq!(layout.frames_extent(), layout.group);
                    assert!(layout.fits(image_size));
                    for frame in layout.frames() {
                        assert_rect_inside(frame, image_size);
                    }
                }
            }
        }
    }

    #[test]
    fn target_change_preserves_group_and_resets_the_seam_default() {
        let image_size = [4000, 3000];
        let original = SnsSplitLayout {
            target: SnsTarget::X,
            count: 3,
            seam_permille: 63,
            group: CropRect {
                min_x: 401.0,
                min_y: 503.0,
                max_x: 3407.0,
                max_y: 2709.0,
            },
        }
        .clamped(image_size);

        let instagram = original.with_target(SnsTarget::Instagram, image_size);
        assert_eq!(instagram.group, original.group);
        assert_eq!(instagram.count, original.count);
        assert_eq!(instagram.seam_permille, 0);

        let restored = instagram.with_target(SnsTarget::X, image_size);
        assert_eq!(restored.group, original.group);
        assert_eq!(restored.count, original.count);
        assert_eq!(restored.seam_permille, 17);

        let mut repeated = restored;
        for _ in 0..20 {
            repeated = repeated
                .with_target(SnsTarget::Instagram, image_size)
                .with_target(SnsTarget::X, image_size);
        }
        assert_eq!(repeated.group, original.group);
    }

    #[test]
    fn count_and_seam_changes_preserve_the_group() {
        let image_size = [4000, 3000];
        let original = SnsSplitLayout {
            target: SnsTarget::X,
            count: 2,
            seam_permille: 17,
            group: CropRect {
                min_x: 401.0,
                min_y: 503.0,
                max_x: 3407.0,
                max_y: 2709.0,
            },
        }
        .clamped(image_size);

        let four = original.with_count(4, image_size);
        assert_eq!(four.group, original.group);
        assert_eq!(four.seam_permille, original.seam_permille);
        let custom_seam = four.with_seam_permille(73, image_size);
        assert_eq!(custom_seam.group, original.group);
        assert_eq!(custom_seam.seam_permille, 73);
    }

    #[test]
    fn clamped_limits_width_and_height_independently() {
        let wide = SnsSplitLayout {
            target: SnsTarget::X,
            count: 3,
            seam_permille: 17,
            group: CropRect {
                min_x: -2000.0,
                min_y: 200.0,
                max_x: 3000.0,
                max_y: 323.0,
            },
        }
        .clamped([1000, 800]);
        assert_eq!((raw_width(wide.group), raw_height(wide.group)), (1000, 123));
        assert!(wide.fits([1000, 800]));

        let tall = SnsSplitLayout {
            target: SnsTarget::X,
            count: 3,
            seam_permille: 17,
            group: CropRect {
                min_x: 200.0,
                min_y: -2000.0,
                max_x: 521.0,
                max_y: 3000.0,
            },
        }
        .clamped([1000, 800]);
        assert_eq!((raw_width(tall.group), raw_height(tall.group)), (321, 800));
        assert!(tall.fits([1000, 800]));
    }

    #[test]
    fn clamped_moves_overflow_without_resizing() {
        let image_size = [1000, 800];
        let layout = SnsSplitLayout {
            target: SnsTarget::X,
            count: 3,
            seam_permille: 17,
            group: CropRect {
                min_x: 900.0,
                min_y: 700.0,
                max_x: 1200.0,
                max_y: 900.0,
            },
        };
        let moved = layout.clamped(image_size);
        assert_eq!(
            (raw_width(moved.group), raw_height(moved.group)),
            (300, 200)
        );
        assert_eq!(moved.group.max_x, 1000.0);
        assert_eq!(moved.group.max_y, 800.0);
        assert!(moved.fits(image_size));
        assert_eq!(moved.group, moved.frames_extent());
    }

    #[test]
    fn count_and_seam_are_clamped_at_every_entry_point() {
        for (input, expected) in [(0, 2), (1, 2), (5, 4), (255, 4)] {
            let layout = SnsSplitLayout::centered_max(SnsTarget::X, input, [4000, 3000]);
            assert_eq!(layout.count, expected);
            assert_eq!(layout.frames().len(), usize::from(expected));

            let direct = SnsSplitLayout {
                target: SnsTarget::X,
                count: input,
                seam_permille: u16::MAX,
                group: CropRect::full(4000, 3000),
            };
            let clamped = direct.clamped([4000, 3000]);
            assert_eq!(clamped.count, expected);
            assert_eq!(clamped.seam_permille, MAX_SEAM_PERMILLE);
            assert_eq!(direct.frames(), clamped.frames());
        }
        let layout = SnsSplitLayout::centered_max(SnsTarget::X, 3, [4000, 3000])
            .with_seam_permille(u16::MAX, [4000, 3000]);
        assert_eq!(layout.seam_permille, MAX_SEAM_PERMILLE);
    }

    #[test]
    fn minimum_non_degenerate_row_reports_when_it_cannot_fit() {
        for target in SnsTarget::ALL {
            for count in MIN_COUNT..=MAX_COUNT {
                let too_narrow = [usize::from(count) - 1, 3];
                let layout = SnsSplitLayout::centered_max(target, count, too_narrow);
                assert!(!layout.fits(too_narrow));
                assert_eq!(raw_width(layout.group), usize::from(count));
                assert_partition_invariants(layout);

                let just_wide_enough = [usize::from(count), 1];
                let layout = SnsSplitLayout::centered_max(target, count, just_wide_enough);
                assert!(layout.fits(just_wide_enough));
                assert_partition_invariants(layout);
            }
        }
    }

    #[test]
    fn zero_image_dimensions_are_treated_as_one() {
        for target in SnsTarget::ALL {
            let layout = SnsSplitLayout::centered_max(target, MAX_COUNT, [0, 0]);
            assert!(!layout.fits([0, 0]));
            assert_eq!(raw_height(layout.group), 1);
            assert_partition_invariants(layout);
        }
    }

    #[test]
    fn tiny_or_invalid_groups_never_degenerate() {
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
                for seam_permille in [0, 17, 100, u16::MAX] {
                    for group in groups {
                        let layout = SnsSplitLayout {
                            target,
                            count,
                            seam_permille,
                            group,
                        };
                        let frames = layout.frames();
                        assert!(
                            frames
                                .iter()
                                .copied()
                                .all(|frame| { raw_width(frame) >= 1 && raw_height(frame) >= 1 })
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn extreme_requested_extent_terminates_with_a_canonical_layout() {
        let layout = SnsSplitLayout {
            target: SnsTarget::X,
            count: 4,
            seam_permille: 100,
            group: CropRect {
                min_x: -1.0e30,
                min_y: -1.0e30,
                max_x: 1.0e30,
                max_y: 1.0e30,
            },
        }
        .clamped([1_000_000, 1]);

        assert!(layout.fits([1_000_000, 1]));
        assert_eq!(layout.group, layout.frames_extent());
        assert_eq!(layout.clamped([1_000_000, 1]), layout);
        assert_partition_invariants(layout);
    }

    #[test]
    fn frames_extent_tracks_a_rounded_offset_group() {
        let layout = SnsSplitLayout {
            target: SnsTarget::X,
            count: 4,
            seam_permille: 53,
            group: CropRect {
                min_x: 12.4,
                min_y: 34.6,
                max_x: 935.4,
                max_y: 812.6,
            },
        }
        .clamped([2000, 1600]);
        let frames = layout.frames();
        let extent = layout.frames_extent();
        assert_eq!(extent, layout.group);
        assert_eq!(extent.min_x, frames[0].min_x);
        assert_eq!(extent.min_y, frames[0].min_y);
        assert_eq!(extent.max_x, frames.last().unwrap().max_x);
        assert_eq!(extent.max_y, frames[0].max_y);
    }

    #[test]
    fn stable_keys_and_target_seam_defaults_round_trip() {
        assert_eq!(SnsTarget::from_stable_key("unknown"), SnsTarget::X);
        for target in SnsTarget::ALL {
            assert_eq!(SnsTarget::from_stable_key(target.stable_key()), target);
        }
        assert_eq!(SnsTarget::X.label(), "X");
        assert_eq!(SnsTarget::Instagram.label(), "Instagram");
        assert_eq!(SnsTarget::X.seam_ratio_parts(), (17, 1000));
        assert_eq!(SnsTarget::Instagram.seam_ratio_parts(), (0, 1000));
        assert_eq!(SnsTarget::X.default_seam_permille(), 17);
        assert_eq!(SnsTarget::Instagram.default_seam_permille(), 0);
        assert_eq!(SnsTarget::X.seam_ratio(), 0.017);
        assert_eq!(SnsTarget::Instagram.seam_ratio(), 0.0);
    }

    #[test]
    fn frame_ratio_keys_and_group_aspects_are_canonical() {
        for ratio in SnsFrameRatio::ALL {
            assert_eq!(SnsFrameRatio::from_stable_key(ratio.stable_key()), ratio);
        }
        assert_eq!(
            SnsFrameRatio::from_stable_key("future-ratio"),
            SnsFrameRatio::Free
        );
        assert_eq!(SnsFrameRatio::Free.group_aspect(3, 17), None);
        assert_eq!(SnsFrameRatio::Ratio3x4.frame_ratio_parts(), Some((3, 4)));
        assert_eq!(SnsFrameRatio::Ratio4x5.frame_ratio_parts(), Some((4, 5)));
        assert_eq!(SnsFrameRatio::Ratio1x1.frame_ratio_parts(), Some((1, 1)));
        assert_eq!(SnsFrameRatio::Ratio1x1.group_aspect(3, 0), Some(3.0));
        assert!((SnsFrameRatio::Ratio3x4.group_aspect(4, 100).unwrap() - 3.225).abs() < 0.000_001);
    }
}
