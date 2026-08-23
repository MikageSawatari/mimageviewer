//! 動画シークストリップの軸・窓・ジェスチャに関する純粋ロジック。
//!
//! 描画、サムネイル復号、設定、presenter との接続は後続の増分で扱う。本モジュールは
//! コンテナ索引を読む短い FFI 境界を除き、副作用を持たない値変換だけを所有する。

use std::ops::RangeInclusive;

use eframe::egui::Pos2;
use ffmpeg_the_third as ffmpeg;

/// シークストリップの横軸。
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StripAxis {
    /// コンテナ索引から得た全キーフレームと、最小間隔で採用した添字。
    KeyframeIndex {
        keyframes: Vec<f64>,
        adopted: Vec<usize>,
    },
    /// コンテナ索引を利用できない場合の等時間グリッド。
    ///
    /// `duration_secs` は有限個のセルと末尾クランプを定義するために軸自身が所有する。
    TimeGrid {
        interval_secs: f64,
        duration_secs: f64,
    },
}

impl StripAxis {
    /// 軸上に存在するセル数を返す。
    pub(crate) fn cell_count(&self) -> usize {
        match self {
            Self::KeyframeIndex { adopted, .. } => adopted.len(),
            Self::TimeGrid {
                interval_secs,
                duration_secs,
            } => time_grid_cell_count(*interval_secs, *duration_secs),
        }
    }

    /// セル `index` が表す時刻を返す。
    pub(crate) fn cell(&self, index: usize) -> Option<f64> {
        match self {
            Self::KeyframeIndex { keyframes, adopted } => adopted
                .get(index)
                .and_then(|keyframe_index| keyframes.get(*keyframe_index))
                .copied(),
            Self::TimeGrid {
                interval_secs,
                duration_secs,
            } => {
                let count = time_grid_cell_count(*interval_secs, *duration_secs);
                if index >= count {
                    return None;
                }
                let time_secs = index as f64 * *interval_secs;
                (time_secs.is_finite() && time_secs < *duration_secs).then_some(time_secs)
            }
        }
    }

    /// 小数セル位置を、隣接する採用セル間で線形補間した時刻へ変換する。
    ///
    /// 軸外の位置は端の時刻へクランプするが、呼び出し側の `center_index` 自体は変更しない。
    /// そのため描画側は先頭・末尾を越えた分を空セルとして残せる。
    pub(crate) fn time_for_center_index(&self, center_index: f64) -> Option<f64> {
        if !center_index.is_finite() {
            return None;
        }

        let count = self.cell_count();
        let first = self.cell(0)?;
        if count == 1 || center_index <= 0.0 {
            return Some(first);
        }

        let last_index = count.checked_sub(1)?;
        let last = self.cell(last_index)?;
        if center_index >= last_index as f64 {
            return Some(last);
        }

        let lower_index = center_index.floor() as usize;
        let upper_index = lower_index.checked_add(1)?;
        let lower_time = self.cell(lower_index)?;
        let upper_time = self.cell(upper_index)?;
        let fraction = center_index - lower_index as f64;
        Some(lower_time + fraction * (upper_time - lower_time))
    }

    /// 時刻を採用セル間の小数位置へ変換する。
    ///
    /// 時刻は両端へクランプする。同一時刻のセルが連続する場合は最初の一致セルを返す。
    pub(crate) fn center_index_for_time(&self, time_secs: f64) -> Option<f64> {
        if !time_secs.is_finite() {
            return None;
        }

        let count = self.cell_count();
        let first = self.cell(0)?;
        if count == 1 || time_secs <= first {
            return Some(0.0);
        }

        let last_index = count.checked_sub(1)?;
        let last = self.cell(last_index)?;
        if time_secs >= last {
            return Some(last_index as f64);
        }

        // 最初の `cell(i) >= time_secs` を探す。軸は列挙・間引き時に時刻順になる。
        let mut low = 1usize;
        let mut high = last_index;
        while low < high {
            let middle = low + (high - low) / 2;
            if self.cell(middle)? < time_secs {
                low = middle + 1;
            } else {
                high = middle;
            }
        }

        let upper_index = low;
        let lower_index = upper_index.checked_sub(1)?;
        let lower_time = self.cell(lower_index)?;
        let upper_time = self.cell(upper_index)?;
        let gap = upper_time - lower_time;
        if gap <= 0.0 {
            return Some(lower_index as f64);
        }
        Some(lower_index as f64 + (time_secs - lower_time) / gap)
    }
}

fn time_grid_cell_count(interval_secs: f64, duration_secs: f64) -> usize {
    if !interval_secs.is_finite()
        || interval_secs <= 0.0
        || !duration_secs.is_finite()
        || duration_secs < 0.0
    {
        return 0;
    }

    let cells = (duration_secs / interval_secs).ceil();
    let mut count = if cells >= usize::MAX as f64 {
        usize::MAX
    } else {
        cells as usize
    };

    // 浮動小数点の丸めで比が整数をわずかに越えても、動画末尾と同時刻のセルは作らない。
    while count > 0 && (count - 1) as f64 * interval_secs >= duration_secs {
        count -= 1;
    }
    count
}

/// コンテナ索引を採用できるかの純粋な判定結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StripAxisDecision {
    KeyframeIndex,
    TimeGrid(TimeGridReason),
}

/// 等時間グリッドを選んだ理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TimeGridReason {
    TooFewEntries,
    InvalidCoverage,
    IncompleteCoverage,
}

/// 索引エントリ数と末尾到達時刻から、使用する軸を決める。
///
/// `covered_secs` は最後の有効なキーフレーム PTS。索引の最後は動画末尾そのものには
/// ならないため、末尾の未被覆区間を一律ゼロには要求しない。§14 の実測では GOP が
/// 0.50〜4.17 秒だったことから、平均間隔 3 個分を許容しつつ 5〜30 秒に制限する。
/// さらに全尺の 80% 以上を必須にし、先頭付近だけの不完全な索引が少数エントリによって
/// 偶然この許容へ入ることを防ぐ。
pub(crate) fn decide_strip_axis(
    entry_count: usize,
    covered_secs: f64,
    duration_secs: f64,
) -> StripAxisDecision {
    if entry_count < 2 {
        return StripAxisDecision::TimeGrid(TimeGridReason::TooFewEntries);
    }
    if !covered_secs.is_finite() || covered_secs < 0.0 || !duration_secs.is_finite() {
        return StripAxisDecision::TimeGrid(TimeGridReason::InvalidCoverage);
    }
    if duration_secs <= 0.0 {
        return StripAxisDecision::KeyframeIndex;
    }

    let mean_gap = covered_secs / (entry_count - 1) as f64;
    let allowed_tail = (mean_gap * 3.0).clamp(5.0, 30.0);
    let covered_enough = covered_secs >= duration_secs * 0.8;
    let tail_secs = (duration_secs - covered_secs).max(0.0);
    if covered_enough && tail_secs <= allowed_tail {
        StripAxisDecision::KeyframeIndex
    } else {
        StripAxisDecision::TimeGrid(TimeGridReason::IncompleteCoverage)
    }
}

/// コンテナ索引からキーフレーム PTS を秒単位で列挙する。
///
/// 開いた直後の結果が 0〜1 件なら、Matroska / ASF の遅延索引を読み込ませる捨てシークを
/// 1 回だけ行い、必ず数え直す。復号やパケット走査は行わない。
pub(crate) fn enumerate_index_keyframes(
    input: &mut ffmpeg::format::context::Input,
    stream_index: usize,
    time_base: ffmpeg::Rational,
) -> Option<Vec<f64>> {
    let cold = read_index_keyframes(input, stream_index, time_base);
    if cold.as_ref().is_some_and(|entries| entries.len() >= 2) {
        return cold;
    }

    // SAFETY: `input` を排他的に借用中で、返るポインタは直後の null 確認と捨てシークにだけ使う。
    let format_context = unsafe { input.as_mut_ptr() };
    if format_context.is_null() {
        return cold;
    }
    let duration = input.duration();
    let warmup_target = if duration > 0 { duration / 2 } else { 0 };

    // SAFETY: `format_context` は上で null でないことを確認した有効な Input の所有ポインタ。
    // stream_index=-1 の timestamp は AV_TIME_BASE 単位であり、Input::duration() も同じ単位。
    // 戻り値の成否にかかわらず索引を数え直し、シーク結果そのものは再生に利用しない。
    let _seek_result = unsafe {
        ffmpeg::ffi::av_seek_frame(
            format_context,
            -1,
            warmup_target,
            ffmpeg::ffi::AVSEEK_FLAG_BACKWARD as i32,
        )
    };

    read_index_keyframes(input, stream_index, time_base).or(cold)
}

fn read_index_keyframes(
    input: &mut ffmpeg::format::context::Input,
    stream_index: usize,
    time_base: ffmpeg::Rational,
) -> Option<Vec<f64>> {
    use ffmpeg::ffi::{AV_NOPTS_VALUE, avformat_index_get_entries_count, avformat_index_get_entry};

    let numerator = time_base.numerator();
    let denominator = time_base.denominator();
    if numerator <= 0 || denominator <= 0 {
        return None;
    }
    let seconds_per_tick = f64::from(numerator) / f64::from(denominator);

    // SAFETY: `format_context` は呼び出し中 `input` が排他的に保持する。stream_index は
    // nb_streams で境界確認してから streams 配列へ使う。AVStream / AVIndexEntry の参照は
    // `input` が生きているこのブロック内だけで使い、timestamp の値だけを Vec へコピーする。
    unsafe {
        let format_context = input.as_mut_ptr();
        if format_context.is_null() || stream_index >= (*format_context).nb_streams as usize {
            return None;
        }
        let streams = (*format_context).streams;
        if streams.is_null() {
            return None;
        }
        let stream = *streams.add(stream_index);
        if stream.is_null() {
            return None;
        }

        let count = avformat_index_get_entries_count(stream);
        if count <= 0 {
            return None;
        }

        let mut keyframes = Vec::new();
        for index in 0..count {
            let entry = avformat_index_get_entry(stream, index);
            if entry.is_null() || (*entry).flags() & 1 == 0 {
                continue;
            }
            let timestamp = (*entry).timestamp;
            if timestamp == AV_NOPTS_VALUE {
                continue;
            }
            let pts_secs = timestamp as f64 * seconds_per_tick;
            if pts_secs.is_finite() {
                keyframes.push(pts_secs);
            }
        }

        keyframes.sort_by(f64::total_cmp);
        (!keyframes.is_empty()).then_some(keyframes)
    }
}

/// キーフレーム列を最小間隔で貪欲に間引き、採用した元添字を返す。
///
/// 入力は時刻の昇順を前提とする。`min_gap_secs <= 0.0` は全件を採用する。
pub(crate) fn thin_keyframes(keyframes: &[f64], min_gap_secs: f64) -> Vec<usize> {
    if keyframes.is_empty() {
        return Vec::new();
    }
    if min_gap_secs <= 0.0 {
        return (0..keyframes.len()).collect();
    }

    let mut adopted = Vec::with_capacity(keyframes.len());
    adopted.push(0);
    let mut last_adopted = keyframes[0];
    for (index, &pts_secs) in keyframes.iter().enumerate().skip(1) {
        if pts_secs >= last_adopted + min_gap_secs {
            adopted.push(index);
            last_adopted = pts_secs;
        }
    }
    adopted
}

/// 1 個以上のセルを表す inclusive range。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CellRange(RangeInclusive<usize>);

impl CellRange {
    pub(crate) fn new(start: usize, end: usize) -> Option<Self> {
        (start <= end).then_some(Self(start..=end))
    }

    pub(crate) fn start(&self) -> usize {
        *self.0.start()
    }

    pub(crate) fn end(&self) -> usize {
        *self.0.end()
    }
}

/// 充填対象の窓と、直前の窓には含まれなかった範囲。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StripWindow {
    pub(crate) ready: Option<CellRange>,
    pub(crate) new_ranges: Vec<CellRange>,
}

/// 可視範囲の前後へ追加する先読みセル数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StripLookahead {
    pub(crate) before: usize,
    pub(crate) after: usize,
}

impl StripLookahead {
    pub(crate) const fn new(before: usize, after: usize) -> Self {
        Self { before, after }
    }
}

/// 中央位置から、可視セルと指定方向の先読みセルを含む窓を計算する。
///
/// 小数の中央位置から可視領域の両端を求め、1 画素でも交差するセルをすべて含める。
/// 実セル範囲外は空セルなので ready 窓には含めず、全体が軸外なら `ready` は
/// `None` になる。
pub(crate) fn compute_strip_window(
    center_index: f64,
    visible_cell_count: usize,
    lookahead: StripLookahead,
    cell_count: usize,
    previous: Option<&CellRange>,
) -> StripWindow {
    let ready = compute_ready_range(center_index, visible_cell_count, lookahead, cell_count);
    let new_ranges = match (&ready, previous) {
        (Some(current), Some(previous)) => subtract_range(current, previous),
        (Some(current), None) => vec![current.clone()],
        (None, _) => Vec::new(),
    };
    StripWindow { ready, new_ranges }
}

fn compute_ready_range(
    center_index: f64,
    visible_cell_count: usize,
    lookahead: StripLookahead,
    cell_count: usize,
) -> Option<CellRange> {
    if !center_index.is_finite() || visible_cell_count == 0 || cell_count == 0 {
        return None;
    }

    let half_span = visible_cell_count as f64 / 2.0;
    let visible_start = (center_index - half_span).floor();
    // 右端は排他的なので、境界に触れるだけの次セルは含めない。
    let visible_end = (center_index + half_span).ceil() - 1.0;
    let raw_start = visible_start - lookahead.before as f64;
    let raw_end = visible_end + lookahead.after as f64;
    let last_cell = cell_count - 1;
    if raw_end < 0.0 || raw_start > last_cell as f64 {
        return None;
    }

    let start = raw_start.max(0.0) as usize;
    let end = raw_end.min(last_cell as f64) as usize;
    CellRange::new(start, end)
}

fn subtract_range(current: &CellRange, previous: &CellRange) -> Vec<CellRange> {
    if current.end() < previous.start() || current.start() > previous.end() {
        return vec![current.clone()];
    }

    let mut ranges = Vec::with_capacity(2);
    if current.start() < previous.start()
        && let Some(end) = previous.start().checked_sub(1)
        && let Some(range) = CellRange::new(current.start(), end.min(current.end()))
    {
        ranges.push(range);
    }
    if current.end() > previous.end()
        && let Some(start) = previous.end().checked_add(1)
        && let Some(range) = CellRange::new(start.max(current.start()), current.end())
    {
        ranges.push(range);
    }
    ranges
}

const SEEK_ROW_GESTURE_THRESHOLD_POINTS: f32 = 24.0;

/// シーク行で開始した 1 ドラッグの決着状態。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SeekRowGesture {
    Undecided { origin: Pos2 },
    Scrub { last_target_secs: Option<f64> },
    OpenStrip { origin: Pos2 },
}

/// `SeekRowGesture::update` が呼び出し側へ返す現在の決着。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SeekRowDecision {
    Undecided,
    Scrub,
    OpenStrip,
}

impl SeekRowGesture {
    /// 押下位置から未決着のドラッグを開始する。
    pub(crate) fn new(origin: Pos2) -> Self {
        Self::Undecided { origin }
    }

    /// 現在位置を反映し、最初の有意な移動でドラッグ種別を決める。
    pub(crate) fn update(&mut self, pos: Pos2) -> SeekRowDecision {
        match *self {
            Self::Undecided { origin } => {
                let delta = pos - origin;
                if delta.x == 0.0 && delta.y == 0.0 {
                    return SeekRowDecision::Undecided;
                }

                if delta.y < 0.0 && delta.y.abs() > delta.x.abs() {
                    if -delta.y > SEEK_ROW_GESTURE_THRESHOLD_POINTS {
                        *self = Self::OpenStrip { origin };
                        SeekRowDecision::OpenStrip
                    } else {
                        SeekRowDecision::Undecided
                    }
                } else {
                    *self = Self::Scrub {
                        last_target_secs: None,
                    };
                    SeekRowDecision::Scrub
                }
            }
            Self::Scrub { .. } => SeekRowDecision::Scrub,
            Self::OpenStrip { .. } => SeekRowDecision::OpenStrip,
        }
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::pos2;

    use super::*;

    const EPSILON: f64 = 1.0e-9;

    fn assert_close(actual: Option<f64>, expected: f64) {
        let close = actual.is_some_and(|value| (value - expected).abs() < EPSILON);
        assert!(close);
    }

    fn keyframe_axis(keyframes: &[f64], adopted: &[usize]) -> StripAxis {
        StripAxis::KeyframeIndex {
            keyframes: keyframes.to_vec(),
            adopted: adopted.to_vec(),
        }
    }

    fn range(start: usize, end: usize) -> CellRange {
        assert!(start <= end);
        CellRange(start..=end)
    }

    #[test]
    fn keyframe_cells_handle_empty_and_single_element_axes() {
        let empty = keyframe_axis(&[], &[]);
        assert_eq!(empty.cell_count(), 0);
        assert_eq!(empty.cell(0), None);
        assert_eq!(empty.time_for_center_index(0.0), None);
        assert_eq!(empty.center_index_for_time(0.0), None);

        let single = keyframe_axis(&[3.5], &[0]);
        assert_eq!(single.cell_count(), 1);
        assert_eq!(single.cell(0), Some(3.5));
        assert_close(single.time_for_center_index(-10.0), 3.5);
        assert_close(single.time_for_center_index(10.0), 3.5);
        assert_close(single.center_index_for_time(-10.0), 0.0);
        assert_close(single.center_index_for_time(10.0), 0.0);
    }

    #[test]
    fn time_grid_stops_strictly_before_duration() {
        let axis = StripAxis::TimeGrid {
            interval_secs: 3.0,
            duration_secs: 10.0,
        };
        assert_eq!(axis.cell_count(), 4);
        assert_eq!(axis.cell(0), Some(0.0));
        assert_eq!(axis.cell(3), Some(9.0));
        assert_eq!(axis.cell(4), None);

        let exact = StripAxis::TimeGrid {
            interval_secs: 3.0,
            duration_secs: 9.0,
        };
        assert_eq!(exact.cell_count(), 3);
        assert_eq!(exact.cell(2), Some(6.0));
        assert_eq!(exact.cell(3), None);
    }

    #[test]
    fn center_index_mapping_interpolates_and_clamps_without_changing_window_position() {
        let axis = keyframe_axis(&[0.0, 2.0, 7.0, 10.0], &[0, 2, 3]);
        assert_close(axis.time_for_center_index(0.5), 3.5);
        assert_close(axis.time_for_center_index(1.5), 8.5);
        assert_close(axis.time_for_center_index(-1.25), 0.0);
        assert_close(axis.time_for_center_index(9.0), 10.0);

        assert_close(axis.center_index_for_time(3.5), 0.5);
        assert_close(axis.center_index_for_time(8.5), 1.5);
        assert_close(axis.center_index_for_time(-1.0), 0.0);
        assert_close(axis.center_index_for_time(99.0), 2.0);

        let outside =
            compute_strip_window(9.0, 3, StripLookahead::default(), axis.cell_count(), None);
        assert_eq!(outside.ready, None);
    }

    #[test]
    fn center_index_and_time_round_trip_on_variable_gop_axis() {
        let axis = keyframe_axis(&[0.0, 0.5, 2.1, 6.8, 7.0, 12.0], &[0, 2, 3, 5]);
        for center in [0.0, 0.25, 1.0, 1.75, 2.5, 3.0] {
            let time = axis.time_for_center_index(center);
            let round_trip = time.and_then(|value| axis.center_index_for_time(value));
            assert_close(round_trip, center);
        }
    }

    #[test]
    fn axis_decision_requires_entries_and_credible_coverage() {
        assert_eq!(
            decide_strip_axis(0, 0.0, 100.0),
            StripAxisDecision::TimeGrid(TimeGridReason::TooFewEntries)
        );
        assert_eq!(
            decide_strip_axis(1, 95.0, 100.0),
            StripAxisDecision::TimeGrid(TimeGridReason::TooFewEntries)
        );
        assert_eq!(
            decide_strip_axis(20, 98.0, 100.0),
            StripAxisDecision::KeyframeIndex
        );
        assert_eq!(
            decide_strip_axis(20, 40.0, 100.0),
            StripAxisDecision::TimeGrid(TimeGridReason::IncompleteCoverage)
        );
        assert_eq!(
            decide_strip_axis(20, f64::NAN, 100.0),
            StripAxisDecision::TimeGrid(TimeGridReason::InvalidCoverage)
        );
    }

    #[test]
    fn unknown_duration_keeps_a_non_sparse_index() {
        assert_eq!(
            decide_strip_axis(8, 14.0, 0.0),
            StripAxisDecision::KeyframeIndex
        );
    }

    #[test]
    fn thinning_handles_empty_single_and_zero_gap() {
        assert_eq!(thin_keyframes(&[], 2.0), Vec::<usize>::new());
        assert_eq!(thin_keyframes(&[4.0], 2.0), vec![0]);
        assert_eq!(thin_keyframes(&[0.0, 0.5, 1.0, 3.0], 0.0), vec![0, 1, 2, 3]);
    }

    #[test]
    fn thinning_larger_than_duration_keeps_only_first() {
        assert_eq!(thin_keyframes(&[0.0, 1.0, 3.0, 8.0], 20.0), vec![0]);
    }

    #[test]
    fn thinning_uses_minimum_gap_for_variable_gop() {
        let keyframes = [0.0, 0.4, 0.9, 2.0, 2.1, 4.2, 7.0, 7.5];
        assert_eq!(thin_keyframes(&keyframes, 2.0), vec![0, 3, 5, 6]);
    }

    #[test]
    fn window_computation_clamps_to_axis_and_reports_initial_range() {
        let window = compute_strip_window(0.25, 5, StripLookahead::new(0, 2), 20, None);
        assert_eq!(window.ready, Some(range(0, 4)));
        assert_eq!(window.new_ranges, vec![range(0, 4)]);

        let tail = compute_strip_window(19.0, 5, StripLookahead::new(2, 0), 20, None);
        assert_eq!(tail.ready, Some(range(14, 19)));
    }

    #[test]
    fn fractional_center_includes_every_partially_visible_cell() {
        let window = compute_strip_window(8.9, 5, StripLookahead::default(), 30, None);
        assert_eq!(window.ready, Some(range(6, 11)));
    }

    #[test]
    fn window_reuse_reports_only_new_ranges() {
        let forward = StripLookahead::new(0, 2);
        let first = compute_strip_window(8.0, 5, forward, 30, None);
        assert_eq!(first.ready, Some(range(5, 12)));

        let shifted = compute_strip_window(10.0, 5, forward, 30, first.ready.as_ref());
        assert_eq!(shifted.ready, Some(range(7, 14)));
        assert_eq!(shifted.new_ranges, vec![range(13, 14)]);

        let contained = compute_strip_window(
            10.0,
            3,
            StripLookahead::default(),
            30,
            shifted.ready.as_ref(),
        );
        assert_eq!(contained.ready, Some(range(8, 11)));
        assert!(contained.new_ranges.is_empty());

        let disjoint = compute_strip_window(
            24.0,
            3,
            StripLookahead::default(),
            30,
            contained.ready.as_ref(),
        );
        assert_eq!(disjoint.new_ranges, vec![range(22, 25)]);
    }

    #[test]
    fn small_upward_dominant_gesture_remains_undecided() {
        let mut gesture = SeekRowGesture::new(pos2(100.0, 100.0));
        assert_eq!(
            gesture.update(pos2(103.0, 90.0)),
            SeekRowDecision::Undecided
        );
        assert!(matches!(gesture, SeekRowGesture::Undecided { .. }));
    }

    #[test]
    fn upward_vertical_gesture_opens_strip_and_never_redecides() {
        let mut gesture = SeekRowGesture::new(pos2(100.0, 100.0));
        assert_eq!(
            gesture.update(pos2(105.0, 70.0)),
            SeekRowDecision::OpenStrip
        );
        assert_eq!(
            gesture.update(pos2(160.0, 100.0)),
            SeekRowDecision::OpenStrip
        );
        assert!(matches!(gesture, SeekRowGesture::OpenStrip { .. }));
    }

    #[test]
    fn horizontal_gesture_scrubs_and_never_redecides() {
        let mut gesture = SeekRowGesture::new(pos2(100.0, 100.0));
        assert_eq!(gesture.update(pos2(110.0, 98.0)), SeekRowDecision::Scrub);
        assert_eq!(gesture.update(pos2(100.0, 60.0)), SeekRowDecision::Scrub);
        assert!(matches!(gesture, SeekRowGesture::Scrub { .. }));
    }

    #[test]
    fn exactly_flat_movement_scrubs_immediately() {
        let mut gesture = SeekRowGesture::new(pos2(100.0, 100.0));
        assert_eq!(gesture.update(pos2(110.0, 100.0)), SeekRowDecision::Scrub);
    }

    #[test]
    fn pending_upward_gesture_can_turn_horizontal_and_scrub() {
        let mut gesture = SeekRowGesture::new(pos2(100.0, 100.0));
        assert_eq!(
            gesture.update(pos2(102.0, 90.0)),
            SeekRowDecision::Undecided
        );
        assert_eq!(gesture.update(pos2(110.0, 95.0)), SeekRowDecision::Scrub);
        assert_eq!(gesture.update(pos2(100.0, 60.0)), SeekRowDecision::Scrub);
    }

    #[test]
    fn equal_diagonal_gesture_is_scrub() {
        let mut gesture = SeekRowGesture::new(pos2(100.0, 100.0));
        assert_eq!(gesture.update(pos2(75.0, 75.0)), SeekRowDecision::Scrub);
    }

    #[test]
    fn downward_gesture_is_scrub() {
        let mut gesture = SeekRowGesture::new(pos2(100.0, 100.0));
        assert_eq!(gesture.update(pos2(100.0, 125.0)), SeekRowDecision::Scrub);
    }
}
