//! 動画シークストリップの軸・窓・ジェスチャに関する純粋ロジック。
//!
//! 描画、サムネイル復号、設定、presenter との接続は後続の増分で扱う。本モジュールは
//! コンテナ索引を読む短い FFI 境界を除き、副作用を持たない値変換だけを所有する。

use std::ops::RangeInclusive;
use std::time::Duration;

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
        /// 尺とセル数上限から決まる下限。**軸が自分で作り直せるように持つ。**
        /// これが無いと、画像間隔を変えたときに格子は元の間隔しか知らず作り直せない。
        fallback_interval_secs: f64,
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
                ..
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
                ..
            } => {
                let count = time_grid_cell_count(*interval_secs, *duration_secs);
                if index >= count {
                    return None;
                }
                let time_secs = time_grid_cell_secs(index, *interval_secs, *duration_secs);
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

    /// 保持済みの全キーフレームはそのままに、採用リストだけを作り直す。
    ///
    /// 設定変更でサムネイルキャッシュを無効化しないための境界でもある。
    pub(crate) fn with_minimum_gap(&self, min_gap_secs: f64) -> Self {
        match self {
            Self::KeyframeIndex { keyframes, .. } => Self::KeyframeIndex {
                keyframes: keyframes.clone(),
                adopted: thin_keyframes(keyframes, min_gap_secs),
            },
            Self::TimeGrid {
                fallback_interval_secs,
                duration_secs,
                ..
            } => Self::TimeGrid {
                interval_secs: time_grid_interval_secs(*fallback_interval_secs, min_gap_secs),
                fallback_interval_secs: *fallback_interval_secs,
                duration_secs: *duration_secs,
            },
        }
    }
}

/// 格子軸のセル間隔。**利用者の「画像間隔」設定を必ず尊重する。**
///
/// `fallback_interval_secs` は尺とセル数上限から決まる下限、`min_gap_secs` は利用者が選んだ
/// 最小間隔。**どちらも下限なので大きい方を採る。** 以前は前者だけを使っていたため、索引が
/// 不完全で格子へ落ちた動画では設定が無視され、15 秒に設定しても 1 秒間隔のセルが並んでいた
/// (実素材 172.7 秒の mp4 で 173 セル、利用者報告 2026-08-26)。
pub(crate) fn time_grid_interval_secs(fallback_interval_secs: f64, min_gap_secs: f64) -> f64 {
    if min_gap_secs.is_finite() && min_gap_secs > 0.0 {
        fallback_interval_secs.max(min_gap_secs)
    } else {
        fallback_interval_secs
    }
}

/// 格子セルの時刻。**末尾セルだけは 1 間隔ぶん手前へ寄せる。**
///
/// 尺は間隔の整数倍とは限らないので、素直に `index * interval` を使うと最後のセルが
/// 尺の端数の位置に来る。172.7 秒 / 5 秒間隔なら最後は 170.0 秒で、そこへスクラブすると
/// **残り 2.7 秒しか無く、すぐ次の動画へ移ってしまう** (利用者報告 2026-08-27)。
/// 末尾セルを `duration - interval` に置けば、どのセルへ移っても最低 1 間隔ぶんは見られ、
/// セルの右端が動画の末尾と一致する。
///
/// 末尾以外は動かさない。`count = ceil(duration / interval)` なので
/// `(count - 1) * interval < duration`、つまり `(count - 2) * interval < duration - interval`
/// が常に成り立ち、**直前のセルと同時刻になることはない**。
fn time_grid_cell_secs(index: usize, interval_secs: f64, duration_secs: f64) -> f64 {
    let regular_secs = index as f64 * interval_secs;
    let last_cell_secs = (duration_secs - interval_secs).max(0.0);
    regular_secs.min(last_cell_secs)
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
    Unavailable(StripAxisUnavailableReason),
}

/// Material-level reasons why neither seek-strip axis is useful enough to expose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StripAxisUnavailableReason {
    KeyframesTooSparse,
}

impl StripAxisUnavailableReason {
    pub(crate) const fn user_notice(self) -> &'static str {
        match self {
            Self::KeyframesTooSparse => "キーフレームが少ないためストリップを表示できません",
        }
    }

    pub(crate) const fn tooltip(self) -> &'static str {
        match self {
            Self::KeyframesTooSparse => "キーフレームが少ない動画では使えません",
        }
    }
}

/// Per-video material preflight state shared by every strip entry point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SeekStripMaterialAvailability {
    #[default]
    Unknown,
    Available,
    Unavailable(StripAxisUnavailableReason),
}

impl SeekStripMaterialAvailability {
    pub(crate) const fn allows_open(self) -> bool {
        !matches!(self, Self::Unavailable(_))
    }

    pub(crate) const fn encode(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Available => 1,
            Self::Unavailable(StripAxisUnavailableReason::KeyframesTooSparse) => 2,
        }
    }

    pub(crate) const fn decode(value: u8) -> Self {
        match value {
            1 => Self::Available,
            2 => Self::Unavailable(StripAxisUnavailableReason::KeyframesTooSparse),
            _ => Self::Unknown,
        }
    }
}

/// A cell must never require more than one reasonably sized GOP to reconstruct.
///
/// The 2026-08-25 stage-1 rerun had 1,164 unique healthy keyframe-index files. Their raw
/// maximum GOP was p99/max 10.43 s (adopted p99 12.10 s, max 12.39 s); the only declined
/// file had a raw maximum GOP of 833.43 s and previously timed out while decoding. A 15 s
/// raw-GOP ceiling therefore leaves 44% headroom over the measured healthy raw maximum
/// while declining the material before any thumbnail or waveform decode starts. This is
/// intentionally based on raw index gaps, not the user-configurable adopted spacing.
pub(crate) const SEEK_STRIP_MAX_RAW_KEYFRAME_GAP_SECS: f64 = 15.0;

/// Persisted minimum-interval choices for the thumbnail strip, in seconds.
///
/// 分の段は 4 時間級の動画のためにある。1 分間隔だと 1 画面が 11 分ぶんにしかならず、
/// 全体を見渡せない (利用者要望 2026-08-26)。30 分まで広げると 4 時間が 8 セルに収まる。
pub(crate) const THUMBNAIL_RANGE_STEPS_SECS: &[f64] = &[
    0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0,
];

/// Persisted one-screen span choices for the waveform strip, in seconds.
pub(crate) const WAVEFORM_RANGE_STEPS_SECS: &[f64] = &[
    5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0, 10800.0,
];

/// 表示範囲に未着セルがあるとき、ワーカーへ頼み直すか。
///
/// **要求台帳ではなく配達で判断する。** ワーカーは新しい要求が来ると処理中の窓を捨てるので、
/// 「要求した」は「届く」を意味しない。速いドラッグでは 1 つの窓が 2 セルほどしか解決できず、
/// 残りは捨てられる。ドラッグが止まった時点で「要求済みだから頼まない」と判断すると、
/// 捨てられたセルが**セッション中ずっと空のまま**残る (2026-08-27 実機)。
///
/// 送りっぱなしにもしない。ワーカーが処理中の要求を毎フレーム上書きすると、そのたびに
/// 計画を立て直して前へ進まなくなる。手を離すまでは待つ。
pub(crate) fn should_request_strip_window(
    trigger_cells_missing: bool,
    last_sent_request_id: Option<u64>,
    last_finished_request_id: Option<u64>,
) -> bool {
    if !trigger_cells_missing {
        return false;
    }
    match (last_sent_request_id, last_finished_request_id) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(sent), Some(finished)) => finished >= sent,
    }
}
/// One wheel event changes the strip range by exactly one adjacent ladder step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeekStripRangeStep {
    Narrower,
    Wider,
}

impl SeekStripRangeStep {
    /// Positive wheel motion zooms in to a narrower range; negative motion zooms out.
    pub(crate) fn from_wheel_delta(wheel_delta: f32) -> Option<Self> {
        if !wheel_delta.is_finite() || wheel_delta == 0.0 {
            return None;
        }
        Some(if wheel_delta > 0.0 {
            Self::Narrower
        } else {
            Self::Wider
        })
    }
}

fn range_steps(mode: crate::settings::VideoSeekStripMode) -> &'static [f64] {
    match mode {
        crate::settings::VideoSeekStripMode::Thumbnails => THUMBNAIL_RANGE_STEPS_SECS,
        crate::settings::VideoSeekStripMode::Waveform => WAVEFORM_RANGE_STEPS_SECS,
    }
}

pub(crate) fn format_seek_strip_range_value(
    mode: crate::settings::VideoSeekStripMode,
    seconds: f64,
) -> String {
    if !seconds.is_finite() {
        return "--".to_owned();
    }
    let whole_epsilon = f64::EPSILON * seconds.abs().max(1.0) * 8.0;
    // 分表示は mode を問わない。サムネイル側も 2 分以上の段を持つようになったので、
    // 波形だけ分・サムネイルだけ秒にすると同じ値が画面によって違う書き方になる。
    let _ = mode;
    let (value, unit) = if seconds >= 60.0 && (seconds % 60.0).abs() <= whole_epsilon {
        (seconds / 60.0, " 分")
    } else {
        (seconds, " 秒")
    };
    let mut label = value.to_string();
    label.push_str(unit);
    label
}

/// Move a persisted strip range by one step.
///
/// Older versions allowed arbitrary values. An off-ladder value moves to the nearest ladder value
/// strictly in the requested direction. Values outside the ladder clamp to its corresponding end.
pub(crate) fn step_seek_strip_range(
    mode: crate::settings::VideoSeekStripMode,
    current_secs: f64,
    step: SeekStripRangeStep,
) -> Option<f64> {
    if !current_secs.is_finite() {
        return None;
    }
    let steps = range_steps(mode);
    match step {
        SeekStripRangeStep::Narrower => steps
            .iter()
            .rev()
            .copied()
            .find(|candidate| *candidate < current_secs)
            .or_else(|| steps.first().copied()),
        SeekStripRangeStep::Wider => steps
            .iter()
            .copied()
            .find(|candidate| *candidate > current_secs)
            .or_else(|| steps.last().copied()),
    }
}

/// 等時間グリッドを選んだ理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TimeGridReason {
    TooFewEntries,
    NonMonotonicTimestamps,
    InvalidCoverage,
    IncompleteCoverage,
}

/// Keyframe timestamps from the container index plus their enumeration-order health.
///
/// The keyframes field is sorted because axis math requires it, while monotonic and
/// inversion_count preserve whether sorting changed the original index order. Silently
/// sorting an index with backward jumps can make the axis disagree with packet decode order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EnumeratedIndexKeyframes {
    pub(crate) keyframes: Vec<f64>,
    pub(crate) monotonic: bool,
    pub(crate) inversion_count: usize,
}

fn normalize_index_keyframes(mut keyframes: Vec<f64>) -> EnumeratedIndexKeyframes {
    let inversion_count = keyframes
        .windows(2)
        .filter(|pair| pair[1] < pair[0])
        .count();
    keyframes.sort_by(f64::total_cmp);
    EnumeratedIndexKeyframes {
        keyframes,
        monotonic: inversion_count == 0,
        inversion_count,
    }
}

/// Includes enumeration order in the decision whether the container index is usable.
pub(crate) fn decide_enumerated_strip_axis(
    index: &EnumeratedIndexKeyframes,
    duration_secs: f64,
) -> StripAxisDecision {
    if !index.monotonic {
        StripAxisDecision::TimeGrid(TimeGridReason::NonMonotonicTimestamps)
    } else {
        decide_strip_axis(&index.keyframes, duration_secs)
    }
}

/// 索引エントリ数と末尾到達時刻から、使用する軸を決める。
///
/// `keyframes` は有効なキーフレーム seek timestamp の昇順列。MP4 などでは index entry が
/// presentation PTS ではなく DTS のことがある。索引の最後は動画末尾そのものには
/// ならない。索引 completeness の主判定は「最後のキーフレームが、素材内で観測した最大
/// GOP 1 個分以内に末尾へ到達していること」とする。これは短尺・長 GOP 素材を percentage
/// coverage で誤って拒否せず、途中で止まった partial index は大きな未索引 tail で拒否する。
pub(crate) fn decide_strip_axis(keyframes: &[f64], duration_secs: f64) -> StripAxisDecision {
    if keyframes.len() < 2 {
        return StripAxisDecision::TimeGrid(TimeGridReason::TooFewEntries);
    }
    let first_secs = keyframes[0];
    let covered_secs = *keyframes.last().unwrap_or(&f64::NAN);
    if !first_secs.is_finite()
        || !covered_secs.is_finite()
        || covered_secs < 0.0
        || !duration_secs.is_finite()
    {
        return StripAxisDecision::TimeGrid(TimeGridReason::InvalidCoverage);
    }
    if duration_secs <= 0.0 {
        return StripAxisDecision::KeyframeIndex;
    }

    let mut observed_max_gap = 0.0_f64;
    for pair in keyframes.windows(2) {
        let gap = pair[1] - pair[0];
        if !gap.is_finite() || gap < 0.0 {
            return StripAxisDecision::TimeGrid(TimeGridReason::InvalidCoverage);
        }
        if gap > 0.0 {
            observed_max_gap = observed_max_gap.max(gap);
        }
    }
    if observed_max_gap <= 0.0 || !observed_max_gap.is_finite() {
        return StripAxisDecision::TimeGrid(TimeGridReason::InvalidCoverage);
    }
    let tail_secs = (duration_secs - covered_secs).max(0.0);
    let comparison_scale = duration_secs
        .abs()
        .max(covered_secs.abs())
        .max(observed_max_gap)
        .max(1.0);
    let rounding_guard = f64::EPSILON * comparison_scale * 8.0;
    if tail_secs <= observed_max_gap + rounding_guard {
        if observed_max_gap > SEEK_STRIP_MAX_RAW_KEYFRAME_GAP_SECS {
            StripAxisDecision::Unavailable(StripAxisUnavailableReason::KeyframesTooSparse)
        } else {
            StripAxisDecision::KeyframeIndex
        }
    } else {
        StripAxisDecision::TimeGrid(TimeGridReason::IncompleteCoverage)
    }
}

/// コンテナ索引からキーフレーム seek timestamp を秒単位で列挙する。
///
/// `AVIndexEntry.timestamp` は presentation PTS とは限らない。thumbnail worker は要求窓の
/// key packet を復号するときに packet DTS と PTS を対応付け、frame PTS との照合はそちらで行う。
///
/// 開いた直後の結果が 0〜1 件なら、Matroska / ASF の遅延索引を読み込ませる捨てシークを
/// 1 回だけ行い、必ず数え直す。復号やパケット走査は行わない。
pub(crate) fn enumerate_index_keyframes(
    input: &mut ffmpeg::format::context::Input,
    stream_index: usize,
    time_base: ffmpeg::Rational,
) -> Option<EnumeratedIndexKeyframes> {
    let cold = read_index_keyframes(input, stream_index, time_base);
    if cold
        .as_ref()
        .is_some_and(|entries| entries.keyframes.len() >= 2)
    {
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
) -> Option<EnumeratedIndexKeyframes> {
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

        (!keyframes.is_empty()).then(|| normalize_index_keyframes(keyframes))
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
        let gap_secs = pts_secs - last_adopted;
        let comparison_scale = pts_secs
            .abs()
            .max(last_adopted.abs())
            .max(min_gap_secs.abs())
            .max(1.0);
        let rounding_guard = f64::EPSILON * comparison_scale * 8.0;
        if gap_secs.is_finite() && gap_secs >= min_gap_secs - rounding_guard {
            adopted.push(index);
            last_adopted = pts_secs;
        }
    }
    // **最後のキーフレームは間隔に関わらず必ず採る** (利用者判断 2026-08-26)。
    //
    // 間引きは前から順に採るので、末尾のキーフレームが直前の採用より近いと落ちる。すると
    // ストリップは動画の終わりまで届かず、右端が空く。実素材 `アカリがやってきたぞっ` は
    // 301 秒だが最終セルが 283.1 秒になり、末尾 18 秒にセルが無かった (最後のキーフレーム
    // 294.8 秒が直前から 11.7 秒しか離れておらず、15 秒に満たない)。
    //
    // 末尾だけ間隔が詰まるが、**間隔が詰まるよりサムネイルが見えない方が影響が大きい**という
    // 判断。終わりは利用者が探す目印なので、必ず 1 枚置く。
    if let Some(&last) = adopted.last()
        && last + 1 < keyframes.len()
    {
        adopted.push(keyframes.len() - 1);
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
    // D16: integer positions are cell *left edges*, so cell i occupies [i, i + 1).
    // This is the same convention used by drawing and pointer hit testing.
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

/// ストリップを開けるかを決めるための、描画や App state に依存しない入力。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SeekStripOpenContext {
    pub(crate) has_video: bool,
    pub(crate) duration_secs: f64,
    pub(crate) tile_mode_open: bool,
    pub(crate) audio_only: bool,
    pub(crate) hud_dimmed: bool,
}

pub(crate) const UNKNOWN_VIDEO_DURATION_NOTICE: &str = "この動画には長さの情報がないため、シークバーとシークストリップは使えません。動画の再生はできます。";

/// シーク位置を全体尺へ対応付けられる動画か。
pub(crate) fn video_position_controls_available(duration_secs: f64) -> bool {
    duration_secs.is_finite() && duration_secs > 0.0
}

pub(crate) fn video_position_controls_notice(
    has_video: bool,
    duration_secs: f64,
) -> Option<&'static str> {
    (has_video && !video_position_controls_available(duration_secs))
        .then_some(UNKNOWN_VIDEO_DURATION_NOTICE)
}

/// 現在の動画 surface でストリップを開いてよいか。
pub(crate) fn seek_strip_may_open(context: SeekStripOpenContext) -> bool {
    context.has_video
        && video_position_controls_available(context.duration_secs)
        && !context.tile_mode_open
        && !context.audio_only
        && !context.hud_dimmed
}

/// ストリップを閉じる lifecycle 境界。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeekStripCloseCause {
    Toggle,
    DownwardDrag,
    Escape,
    HudHidden,
    VideoChanged,
    FullscreenExit,
    TileModeOpened,
    Unavailable,
}

impl SeekStripCloseCause {
    /// The user asked for the strip to go away, as opposed to something else taking the screen
    /// or the material turning out to be unusable.
    pub(crate) const fn is_user_dismissal(self) -> bool {
        matches!(self, Self::Toggle | Self::DownwardDrag | Self::Escape)
    }

    /// Explicit close operations clear the persisted 3-state selection. Resource-only lifecycle
    /// boundaries keep it so the same strip can be restored for the next video session.
    ///
    /// While the strip is pinned, a close the user did not ask for must not clear it either:
    /// a video with no usable material, or a trip through the tile grid, would otherwise cancel
    /// the pin for every video after it.
    pub(crate) const fn clears_persisted_state(self, strip_locked: bool) -> bool {
        if self.is_user_dismissal() {
            return true;
        }
        !strip_locked
            && matches!(
                self,
                Self::HudHidden | Self::TileModeOpened | Self::Unavailable
            )
    }
}

/// ストリップとタイル一覧の排他 surface。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SeekStripSurface {
    #[default]
    Neither,
    Strip,
    Tile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SeekStripSurfaceIntent {
    OpenStrip { may_open: bool },
    CloseStrip(SeekStripCloseCause),
    ToggleTile,
}

/// 2 つの動画探索 surface の排他とストリップの close 条件を 1 箇所で決める。
pub(crate) fn decide_seek_strip_surface(
    current: SeekStripSurface,
    intent: SeekStripSurfaceIntent,
) -> SeekStripSurface {
    match intent {
        SeekStripSurfaceIntent::OpenStrip { may_open } => match current {
            SeekStripSurface::Neither if may_open => SeekStripSurface::Strip,
            other => other,
        },
        SeekStripSurfaceIntent::CloseStrip(_) => match current {
            SeekStripSurface::Strip => SeekStripSurface::Neither,
            other => other,
        },
        SeekStripSurfaceIntent::ToggleTile => match current {
            SeekStripSurface::Tile => SeekStripSurface::Neither,
            SeekStripSurface::Neither | SeekStripSurface::Strip => SeekStripSurface::Tile,
        },
    }
}

/// ドラッグ開始時の中心と水平方向の総移動量から、新しい中心位置を求める。
///
/// 軸外へはクランプしない。描画側がその分を空セルとして残し、seek 時だけ軸が端へ
/// クランプする。
pub(crate) fn center_index_after_drag(
    origin_center_index: f64,
    drag_delta_x: f32,
    cell_width: f32,
) -> Option<f64> {
    if !origin_center_index.is_finite()
        || !drag_delta_x.is_finite()
        || !cell_width.is_finite()
        || cell_width <= 0.0
    {
        return None;
    }
    Some(origin_center_index - f64::from(drag_delta_x / cell_width))
}

/// ストリップ上の x 座標を、セル左端を整数とする連続位置へ変換する。
pub(crate) fn center_index_at_pointer(
    center_index: f64,
    pointer_x: f32,
    marker_x: f32,
    cell_width: f32,
) -> Option<f64> {
    center_index_after_drag(center_index, marker_x - pointer_x, cell_width)
}

/// セル左端の連続位置をストリップ上の x 座標へ変換する。
///
/// `center_index_at_pointer` の逆変換であり、D16 の描画と hit test が同じ座標規約を
/// 共有するための正本。整数 `i` の戻り値はセル `i` の nominal left edge になる。
pub(crate) fn x_for_center_index(
    index_position: f64,
    center_index: f64,
    marker_x: f32,
    cell_width: f32,
) -> Option<f32> {
    if !index_position.is_finite()
        || !center_index.is_finite()
        || !marker_x.is_finite()
        || !cell_width.is_finite()
        || cell_width <= 0.0
    {
        return None;
    }
    let x = f64::from(marker_x) + (index_position - center_index) * f64::from(cell_width);
    x.is_finite().then_some(x as f32)
}

/// ポインタ直下にある実セルを D16 の左端基準で返す。
pub(crate) fn cell_index_at_pointer(
    center_index: f64,
    pointer_x: f32,
    marker_x: f32,
    cell_width: f32,
    cell_count: usize,
) -> Option<usize> {
    let index_position = center_index_at_pointer(center_index, pointer_x, marker_x, cell_width)?;
    let index = index_position.floor();
    (index >= 0.0 && index < cell_count as f64).then_some(index as usize)
}

/// Mode-specific coordinate carried across App/presenter boundaries.
///
/// Thumbnail mode is indexed by adopted cells, while waveform mode is
/// time-linear. Keeping the coordinate typed prevents a mode switch or delayed
/// presenter event from interpreting one axis as the other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SeekStripCenter {
    Thumbnails { center_index: f64 },
    Waveform { center_time_secs: f64 },
}

/// Immutable press-time snapshot for one seek-strip drag.
///
/// Pointer movement is always measured from this position. In particular, it must not be
/// reconstructed from egui's per-frame drag delta, or dropped/repeated frames make the strip
/// spring back toward the press-time centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SeekStripDragOrigin {
    pub(crate) center: SeekStripCenter,
    pub(crate) pointer: eframe::egui::Pos2,
}

impl SeekStripDragOrigin {
    pub(crate) fn new(center: SeekStripCenter, pointer: eframe::egui::Pos2) -> Self {
        Self { center, pointer }
    }
}

impl SeekStripCenter {
    pub(crate) fn mode(self) -> crate::settings::VideoSeekStripMode {
        match self {
            Self::Thumbnails { .. } => crate::settings::VideoSeekStripMode::Thumbnails,
            Self::Waveform { .. } => crate::settings::VideoSeekStripMode::Waveform,
        }
    }

    pub(crate) fn time_secs(self, axis: Option<&StripAxis>) -> Option<f64> {
        match self {
            Self::Thumbnails { center_index } => axis?.time_for_center_index(center_index),
            Self::Waveform { center_time_secs } => {
                center_time_secs.is_finite().then_some(center_time_secs)
            }
        }
    }
}

/// Switch axes without changing the time under the fixed centre marker.
pub(crate) fn switch_seek_strip_center(
    current: SeekStripCenter,
    target_mode: crate::settings::VideoSeekStripMode,
    thumbnail_axis: Option<&StripAxis>,
) -> Option<SeekStripCenter> {
    if current.mode() == target_mode {
        return Some(current);
    }
    let center_time_secs = current.time_secs(thumbnail_axis)?;
    match target_mode {
        crate::settings::VideoSeekStripMode::Thumbnails => Some(SeekStripCenter::Thumbnails {
            center_index: thumbnail_axis?.center_index_for_time(center_time_secs)?,
        }),
        crate::settings::VideoSeekStripMode::Waveform => {
            Some(SeekStripCenter::Waveform { center_time_secs })
        }
    }
}

/// Time-linear waveform drag: one full strip width always spans the supplied seconds.
pub(crate) fn waveform_center_after_drag(
    origin_center_secs: f64,
    drag_delta_x: f32,
    strip_width: f32,
    span_secs: f64,
) -> Option<f64> {
    if !origin_center_secs.is_finite()
        || !drag_delta_x.is_finite()
        || !strip_width.is_finite()
        || strip_width <= 0.0
        || !span_secs.is_finite()
        || span_secs <= 0.0
    {
        return None;
    }
    Some(origin_center_secs - f64::from(drag_delta_x / strip_width) * span_secs)
}

pub(crate) fn waveform_time_at_pointer(
    center_time_secs: f64,
    pointer_x: f32,
    marker_x: f32,
    strip_width: f32,
    span_secs: f64,
) -> Option<f64> {
    waveform_center_after_drag(
        center_time_secs,
        marker_x - pointer_x,
        strip_width,
        span_secs,
    )
}

/// Resolve the current centre from one immutable press snapshot and the current pointer.
///
/// This is deliberately a pure mapping so live movement and release cannot disagree about how a
/// drag accumulates.
pub(crate) fn seek_strip_center_at_drag_pointer(
    origin: SeekStripDragOrigin,
    pointer: eframe::egui::Pos2,
    thumbnail_cell_width: f32,
    strip_width: f32,
    waveform_span_secs: f64,
) -> Option<SeekStripCenter> {
    let total_delta_x = pointer.x - origin.pointer.x;
    match origin.center {
        SeekStripCenter::Thumbnails { center_index } => {
            center_index_after_drag(center_index, total_delta_x, thumbnail_cell_width)
                .map(|center_index| SeekStripCenter::Thumbnails { center_index })
        }
        SeekStripCenter::Waveform { center_time_secs } => waveform_center_after_drag(
            center_time_secs,
            total_delta_x,
            strip_width,
            waveform_span_secs,
        )
        .map(|center_time_secs| SeekStripCenter::Waveform { center_time_secs }),
    }
}

/// ストリップ上で始めたドラッグが、下のシーク行へ戻って close する動きか。
pub(crate) fn strip_drag_closes_downward(
    origin_pointer: eframe::egui::Pos2,
    pointer: eframe::egui::Pos2,
    seek_row_top: f32,
) -> bool {
    let drag_delta = pointer - origin_pointer;
    drag_delta.y > SEEK_ROW_GESTURE_THRESHOLD_POINTS
        && drag_delta.y > drag_delta.x.abs()
        && pointer.y >= seek_row_top
}

/// App がストリップの非同期結果を取り込むために次の repaint を予約すべき状態。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SeekStripRepaintContext {
    pub(crate) visible_cells_pending: bool,
    pub(crate) drag_active: bool,
}

/// 可視セルが未決着か、ストリップ自身のドラッグが続いている間だけ polling する。
pub(crate) fn seek_strip_needs_repaint(context: SeekStripRepaintContext) -> bool {
    context.visible_cells_pending || context.drag_active
}

/// Playback-follow cadence. This is fast enough to look continuous while avoiding an App-side
/// axis conversion and presenter payload change on every decoded frame.
pub(crate) const SEEK_STRIP_FOLLOW_INTERVAL: Duration = Duration::from_millis(100);
const SEEK_STRIP_FOLLOW_MIN_DELTA_SECS: f64 = 0.040;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SeekStripFollowContext {
    pub(crate) playing: bool,
    pub(crate) drag_active: bool,
    pub(crate) elapsed_since_recenter: Option<Duration>,
    pub(crate) current_time_secs: Option<f64>,
    pub(crate) playhead_time_secs: f64,
}

/// Return the playhead time that should become the new strip centre, or `None` when this frame is
/// intentionally rate-limited/detached. The caller performs the mode-specific time→axis mapping.
pub(crate) fn decide_follow_playhead_recenter(context: SeekStripFollowContext) -> Option<f64> {
    if !context.playing
        || context.drag_active
        || !context.playhead_time_secs.is_finite()
        || context
            .elapsed_since_recenter
            .is_some_and(|elapsed| elapsed < SEEK_STRIP_FOLLOW_INTERVAL)
    {
        return None;
    }
    let current_time_secs = context.current_time_secs?;
    if !current_time_secs.is_finite()
        || (context.playhead_time_secs - current_time_secs).abs() < SEEK_STRIP_FOLLOW_MIN_DELTA_SECS
    {
        return None;
    }
    Some(context.playhead_time_secs)
}

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

    fn evenly_spaced_keyframes(count: usize, covered_secs: f64) -> Vec<f64> {
        if count < 2 {
            return vec![covered_secs; count];
        }
        (0..count)
            .map(|index| covered_secs * index as f64 / (count - 1) as f64)
            .collect()
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
            fallback_interval_secs: 3.0,
            duration_secs: 10.0,
        };
        assert_eq!(axis.cell_count(), 4);
        assert_eq!(axis.cell(0), Some(0.0));
        // 末尾セルは 9.0 ではなく 10.0 - 3.0。9.0 だと残り 1 秒しか無く、そこへ移ると
        // すぐ次の動画へ移ってしまう (利用者報告 2026-08-27)。
        assert_eq!(axis.cell(3), Some(7.0));
        assert_eq!(axis.cell(4), None);

        let exact = StripAxis::TimeGrid {
            interval_secs: 3.0,
            fallback_interval_secs: 3.0,
            duration_secs: 9.0,
        };
        assert_eq!(exact.cell_count(), 3);
        // 尺が間隔の整数倍なら、末尾セルはもともと 1 間隔ぶん手前にある。動かさない。
        assert_eq!(exact.cell(2), Some(6.0));
        assert_eq!(exact.cell(3), None);
    }

    /// 実機で報告された形。172.7 秒 / 5 秒間隔で、末尾セルが 170.0 秒に来ていた。
    #[test]
    fn time_grid_last_cell_ends_at_the_material_end() {
        let duration_secs = 172.733_243_f64;
        let interval_secs = 5.0_f64;
        let axis = StripAxis::TimeGrid {
            interval_secs,
            fallback_interval_secs: interval_secs,
            duration_secs,
        };
        let last_index = axis.cell_count() - 1;
        let last = axis.cell(last_index).unwrap();
        // 末尾セルの右端が動画の末尾と一致する。
        assert!(
            (last + interval_secs - duration_secs).abs() < 1.0e-9,
            "last={last}"
        );
        // 直前のセルは通常どおりで、同時刻にはならない。
        let previous = axis.cell(last_index - 1).unwrap();
        assert!((previous - (last_index - 1) as f64 * interval_secs).abs() < 1.0e-9);
        assert!(previous < last, "previous={previous} last={last}");
        // どのセルからも最低 1 間隔ぶん再生できる。
        for index in 0..=last_index {
            let secs = axis.cell(index).unwrap();
            assert!(
                secs + interval_secs <= duration_secs + 1.0e-9,
                "cell {index} at {secs} leaves less than one interval"
            );
        }
    }

    /// 尺が 1 間隔に満たない素材。末尾セルを手前へ寄せる余地が無いので 0 のまま。
    #[test]
    fn time_grid_shorter_than_one_interval_keeps_a_single_cell_at_zero() {
        let axis = StripAxis::TimeGrid {
            interval_secs: 15.0,
            fallback_interval_secs: 15.0,
            duration_secs: 4.0,
        };
        assert_eq!(axis.cell_count(), 1);
        assert_eq!(axis.cell(0), Some(0.0));
        assert_eq!(axis.cell(1), None);
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
            decide_strip_axis(&[], 100.0),
            StripAxisDecision::TimeGrid(TimeGridReason::TooFewEntries)
        );
        assert_eq!(
            decide_strip_axis(&[95.0], 100.0),
            StripAxisDecision::TimeGrid(TimeGridReason::TooFewEntries)
        );
        assert_eq!(
            decide_strip_axis(&evenly_spaced_keyframes(20, 98.0), 100.0),
            StripAxisDecision::KeyframeIndex
        );
        assert_eq!(
            decide_strip_axis(&evenly_spaced_keyframes(20, 40.0), 100.0),
            StripAxisDecision::TimeGrid(TimeGridReason::IncompleteCoverage)
        );
        assert_eq!(
            decide_strip_axis(&[0.0, f64::NAN], 100.0),
            StripAxisDecision::TimeGrid(TimeGridReason::InvalidCoverage)
        );
    }

    #[test]
    fn enumerated_axis_rejects_backward_index_timestamps_before_sort_can_hide_them() {
        let index = normalize_index_keyframes(vec![0.0, 10.0, 5.0, 15.0]);
        assert_eq!(index.keyframes, vec![0.0, 5.0, 10.0, 15.0]);
        assert!(!index.monotonic);
        assert_eq!(index.inversion_count, 1);
        assert_eq!(
            decide_enumerated_strip_axis(&index, 16.0),
            StripAxisDecision::TimeGrid(TimeGridReason::NonMonotonicTimestamps)
        );
    }

    #[test]
    fn unknown_duration_keeps_a_non_sparse_index() {
        assert_eq!(
            decide_strip_axis(&evenly_spaced_keyframes(8, 14.0), 0.0),
            StripAxisDecision::KeyframeIndex
        );
    }

    #[test]
    fn axis_decision_allows_one_observed_variable_gop_at_the_unindexed_tail() {
        let mut keyframes: Vec<f64> = (0..=40).map(|index| index as f64 * 0.5).collect();
        keyframes.extend((0..=10).map(|index| 29.5 + index as f64 * 2.0));

        assert_eq!(
            decide_strip_axis(&keyframes, 57.5),
            StripAxisDecision::KeyframeIndex,
            "the 8s tail is shorter than the observed 9.5s GOP"
        );
        assert_eq!(
            decide_strip_axis(&keyframes, 60.0),
            StripAxisDecision::TimeGrid(TimeGridReason::IncompleteCoverage),
            "a tail longer than every observed GOP remains incomplete"
        );

        let adopted = thin_keyframes(&keyframes, 2.0);
        assert!(
            adopted.windows(2).any(|pair| pair[1] - pair[0] >= 4),
            "the regression axis must include adopted cells several raw entries apart"
        );
    }

    #[test]
    fn axis_decision_accepts_complete_short_long_gop_index_below_eighty_percent() {
        let keyframes = [0.0, 8.34, 16.68, 23.4];

        assert_eq!(
            decide_strip_axis(&keyframes, 30.1),
            StripAxisDecision::KeyframeIndex,
            "the 6.7s tail is within the observed 8.34s GOP"
        );
    }

    #[test]
    fn axis_decision_declines_sparse_material_before_axis_fallback() {
        assert_eq!(
            decide_strip_axis(&[0.0, 15.0, 30.0], 44.0),
            StripAxisDecision::KeyframeIndex
        );
        assert_eq!(
            decide_strip_axis(&[0.0, 15.001, 30.002], 44.0),
            StripAxisDecision::Unavailable(StripAxisUnavailableReason::KeyframesTooSparse)
        );
    }

    #[test]
    fn axis_decision_rejects_index_that_stops_one_third_of_the_way_in() {
        let keyframes = [0.0, 8.34, 16.68, 23.4];

        assert_eq!(
            decide_strip_axis(&keyframes, 70.2),
            StripAxisDecision::TimeGrid(TimeGridReason::IncompleteCoverage),
            "the 46.8s unindexed tail is much longer than every observed GOP"
        );
    }

    #[test]
    fn thinning_handles_empty_single_and_zero_gap() {
        assert_eq!(thin_keyframes(&[], 2.0), Vec::<usize>::new());
        assert_eq!(thin_keyframes(&[4.0], 2.0), vec![0]);
        assert_eq!(thin_keyframes(&[0.0, 0.5, 1.0, 3.0], 0.0), vec![0, 1, 2, 3]);
    }

    #[test]
    fn thinning_larger_than_duration_keeps_the_first_and_the_last() {
        // 間隔が尺より長くても、先頭と**末尾**は置く (2026-08-26 の判断。以前は先頭だけだった)。
        // 8 秒の素材に 20 秒を指定した場合、始まりと終わりの 2 枚になる。
        assert_eq!(thin_keyframes(&[0.0, 1.0, 3.0, 8.0], 20.0), vec![0, 3]);
    }

    #[test]
    fn thinning_uses_minimum_gap_for_variable_gop() {
        let keyframes = [0.0, 0.4, 0.9, 2.0, 2.1, 4.2, 7.0, 7.5];
        // 末尾の 7.5 は直前の 7.0 から 0.5 秒しか離れていないが、**終わりは必ず置く**ので
        // 採用する。末尾だけ間隔が詰まるのは承知のうえの判断 (2026-08-26)。
        assert_eq!(thin_keyframes(&keyframes, 2.0), vec![0, 3, 5, 6, 7]);
    }

    #[test]
    fn thinning_treats_rounding_noise_at_the_exact_threshold_as_equal() {
        let epsilon = f64::EPSILON;
        let keyframes = [0.0, 2.0 - epsilon, 4.0 - epsilon * 2.0];
        assert_eq!(thin_keyframes(&keyframes, 2.0), vec![0, 1, 2]);

        let genuinely_short = [0.0, 2.0 - 1.0e-8, 4.0];
        assert_eq!(thin_keyframes(&genuinely_short, 2.0), vec![0, 2]);
    }

    /// 「要求した」を「届く」として扱うと、ワーカーが supersede で捨てたセルへ戻る道が無くなる。
    /// 2026-08-27 実機: 速いドラッグで末尾 5 セルが空のまま固定され、Shift+S で開き直すまで
    /// 埋まらなかった。判定は配達で行い、ワーカーが手を離すまでは待つ。
    #[test]
    fn strip_window_request_follows_delivery_not_the_request_ledger() {
        // 届いているなら頼まない。
        assert!(!should_request_strip_window(false, None, None));
        assert!(!should_request_strip_window(false, Some(7), Some(7)));

        // 一度も頼んでいないなら頼む。
        assert!(should_request_strip_window(true, None, None));

        // 頼んだが、ワーカーはまだ 1 件も終えていない。毎フレーム上書きすると計画が
        // 立て直され続けて前へ進まないので待つ。
        assert!(!should_request_strip_window(true, Some(3), None));
        assert!(!should_request_strip_window(true, Some(3), Some(2)));

        // ワーカーが手を離したのに未着が残っている = supersede で捨てられた。頼み直す。
        assert!(should_request_strip_window(true, Some(3), Some(3)));
        assert!(should_request_strip_window(true, Some(3), Some(9)));
    }

    #[test]
    fn range_step_ladders_match_the_persisted_choices() {
        assert_eq!(
            THUMBNAIL_RANGE_STEPS_SECS,
            &[
                0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0,
                1800.0,
            ]
        );
        assert_eq!(
            WAVEFORM_RANGE_STEPS_SECS,
            &[
                5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0,
                10800.0,
            ]
        );
        assert_eq!(
            crate::settings::VIDEO_SEEK_STRIP_MIN_INTERVAL_DEFAULT_SECS,
            15.0
        );
        assert_eq!(
            crate::settings::VIDEO_SEEK_STRIP_WAVEFORM_SPAN_DEFAULT_SECS,
            180.0
        );
    }

    #[test]
    fn wheel_direction_and_ladder_ends_are_stable() {
        assert_eq!(
            SeekStripRangeStep::from_wheel_delta(120.0),
            Some(SeekStripRangeStep::Narrower)
        );
        assert_eq!(
            SeekStripRangeStep::from_wheel_delta(-120.0),
            Some(SeekStripRangeStep::Wider)
        );
        assert_eq!(SeekStripRangeStep::from_wheel_delta(0.0), None);

        assert_eq!(
            step_seek_strip_range(
                crate::settings::VideoSeekStripMode::Thumbnails,
                0.1,
                SeekStripRangeStep::Narrower,
            ),
            Some(0.1)
        );
        assert_eq!(
            step_seek_strip_range(
                crate::settings::VideoSeekStripMode::Thumbnails,
                60.0,
                SeekStripRangeStep::Wider,
            ),
            Some(120.0)
        );
        assert_eq!(
            step_seek_strip_range(
                crate::settings::VideoSeekStripMode::Thumbnails,
                1800.0,
                SeekStripRangeStep::Wider,
            ),
            Some(1800.0)
        );
        assert_eq!(
            step_seek_strip_range(
                crate::settings::VideoSeekStripMode::Waveform,
                5.0,
                SeekStripRangeStep::Narrower,
            ),
            Some(5.0)
        );
        assert_eq!(
            step_seek_strip_range(
                crate::settings::VideoSeekStripMode::Waveform,
                10_800.0,
                SeekStripRangeStep::Wider,
            ),
            Some(10_800.0)
        );
    }

    #[test]
    fn old_off_ladder_range_moves_to_the_adjacent_step_in_wheel_direction() {
        assert_eq!(
            step_seek_strip_range(
                crate::settings::VideoSeekStripMode::Thumbnails,
                7.0,
                SeekStripRangeStep::Narrower,
            ),
            Some(5.0)
        );
        assert_eq!(
            step_seek_strip_range(
                crate::settings::VideoSeekStripMode::Thumbnails,
                7.0,
                SeekStripRangeStep::Wider,
            ),
            Some(10.0)
        );
        assert_eq!(
            step_seek_strip_range(
                crate::settings::VideoSeekStripMode::Waveform,
                45.0,
                SeekStripRangeStep::Narrower,
            ),
            Some(30.0)
        );
        assert_eq!(
            step_seek_strip_range(
                crate::settings::VideoSeekStripMode::Waveform,
                45.0,
                SeekStripRangeStep::Wider,
            ),
            Some(60.0)
        );
    }

    #[test]
    fn compact_range_labels_use_seconds_then_whole_minutes() {
        assert_eq!(
            format_seek_strip_range_value(crate::settings::VideoSeekStripMode::Thumbnails, 0.1,),
            "0.1 秒"
        );
        assert_eq!(
            format_seek_strip_range_value(crate::settings::VideoSeekStripMode::Thumbnails, 15.0,),
            "15 秒"
        );
        assert_eq!(
            format_seek_strip_range_value(crate::settings::VideoSeekStripMode::Waveform, 180.0,),
            "3 分"
        );
        assert_eq!(
            format_seek_strip_range_value(crate::settings::VideoSeekStripMode::Waveform, 10_800.0,),
            "180 分"
        );
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
    fn follow_playhead_is_rate_limited_and_drag_detaches_it() {
        let base = SeekStripFollowContext {
            playing: true,
            drag_active: false,
            elapsed_since_recenter: Some(SEEK_STRIP_FOLLOW_INTERVAL),
            current_time_secs: Some(12.0),
            playhead_time_secs: 12.5,
        };
        assert_eq!(decide_follow_playhead_recenter(base), Some(12.5));
        assert_eq!(
            decide_follow_playhead_recenter(SeekStripFollowContext {
                elapsed_since_recenter: Some(SEEK_STRIP_FOLLOW_INTERVAL - Duration::from_millis(1),),
                ..base
            }),
            None
        );
        assert_eq!(
            decide_follow_playhead_recenter(SeekStripFollowContext {
                drag_active: true,
                ..base
            }),
            None
        );
        assert_eq!(
            decide_follow_playhead_recenter(SeekStripFollowContext {
                playing: false,
                ..base
            }),
            None
        );
    }

    #[test]
    fn follow_playhead_skips_sub_display_motion_but_reacts_after_threshold() {
        let context = |playhead_time_secs| SeekStripFollowContext {
            playing: true,
            drag_active: false,
            elapsed_since_recenter: None,
            current_time_secs: Some(20.0),
            playhead_time_secs,
        };
        assert_eq!(decide_follow_playhead_recenter(context(20.039)), None);
        assert_eq!(
            decide_follow_playhead_recenter(context(20.041)),
            Some(20.041)
        );
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

    #[test]
    fn open_policy_requires_a_ready_exclusive_video_surface() {
        let ready = SeekStripOpenContext {
            has_video: true,
            duration_secs: 120.0,
            tile_mode_open: false,
            audio_only: false,
            hud_dimmed: false,
        };
        assert!(seek_strip_may_open(ready));
        assert!(!seek_strip_may_open(SeekStripOpenContext {
            tile_mode_open: true,
            ..ready
        }));
        assert!(!seek_strip_may_open(SeekStripOpenContext {
            audio_only: true,
            ..ready
        }));
        assert!(!seek_strip_may_open(SeekStripOpenContext {
            hud_dimmed: true,
            ..ready
        }));
        assert!(!seek_strip_may_open(SeekStripOpenContext {
            duration_secs: 0.0,
            ..ready
        }));
        assert!(!seek_strip_may_open(SeekStripOpenContext {
            duration_secs: f64::INFINITY,
            ..ready
        }));
        assert_eq!(
            video_position_controls_notice(true, 0.0),
            Some(UNKNOWN_VIDEO_DURATION_NOTICE)
        );
        assert_eq!(video_position_controls_notice(true, 120.0), None);
        assert_eq!(video_position_controls_notice(false, 0.0), None);
    }

    #[test]
    fn strip_and_tile_surface_transitions_are_mutually_exclusive() {
        let opened = decide_seek_strip_surface(
            SeekStripSurface::Neither,
            SeekStripSurfaceIntent::OpenStrip { may_open: true },
        );
        assert_eq!(opened, SeekStripSurface::Strip);
        assert_eq!(
            decide_seek_strip_surface(opened, SeekStripSurfaceIntent::ToggleTile),
            SeekStripSurface::Tile
        );
        assert_eq!(
            decide_seek_strip_surface(
                SeekStripSurface::Tile,
                SeekStripSurfaceIntent::OpenStrip { may_open: true },
            ),
            SeekStripSurface::Tile
        );
    }

    #[test]
    fn every_declared_close_cause_closes_an_open_strip() {
        let causes = [
            SeekStripCloseCause::Toggle,
            SeekStripCloseCause::DownwardDrag,
            SeekStripCloseCause::Escape,
            SeekStripCloseCause::HudHidden,
            SeekStripCloseCause::VideoChanged,
            SeekStripCloseCause::FullscreenExit,
            SeekStripCloseCause::TileModeOpened,
            SeekStripCloseCause::Unavailable,
        ];
        for cause in causes {
            assert_eq!(
                decide_seek_strip_surface(
                    SeekStripSurface::Strip,
                    SeekStripSurfaceIntent::CloseStrip(cause),
                ),
                SeekStripSurface::Neither
            );
        }
    }

    #[test]
    fn drag_mapping_is_continuous_and_leaves_axis_overscroll_unclamped() {
        assert_close(center_index_after_drag(10.5, 75.0, 150.0), 10.0);
        assert_close(center_index_after_drag(0.0, 300.0, 150.0), -2.0);
        assert_close(center_index_at_pointer(4.25, 400.0, 250.0, 150.0), 5.25);
        assert!(strip_drag_closes_downward(
            eframe::egui::pos2(300.0, 472.0),
            eframe::egui::pos2(308.0, 500.0),
            490.0
        ));
        assert!(!strip_drag_closes_downward(
            eframe::egui::pos2(300.0, 472.0),
            eframe::egui::pos2(330.0, 500.0),
            490.0
        ));
    }

    #[test]
    fn left_aligned_cell_x_and_pointer_hit_test_round_trip() {
        let center_index = 4.25;
        let marker_x = 250.0;
        let cell_width = 150.0;
        let cell_count = 12;

        for index in [0, 4, 5, 11] {
            let left_x = x_for_center_index(index as f64, center_index, marker_x, cell_width)
                .expect("finite cell left edge");
            assert_close(
                center_index_at_pointer(center_index, left_x, marker_x, cell_width),
                index as f64,
            );
            assert_eq!(
                cell_index_at_pointer(
                    center_index,
                    left_x + cell_width * 0.75,
                    marker_x,
                    cell_width,
                    cell_count,
                ),
                Some(index),
            );
        }

        let boundary_x =
            x_for_center_index(5.0, center_index, marker_x, cell_width).expect("boundary");
        assert_eq!(
            cell_index_at_pointer(center_index, boundary_x, marker_x, cell_width, cell_count,),
            Some(5),
            "the exact boundary belongs to the cell that starts there",
        );
    }

    #[test]
    fn one_drag_accumulates_from_press_and_advances_monotonically() {
        let origin = SeekStripDragOrigin::new(
            SeekStripCenter::Thumbnails { center_index: 10.0 },
            eframe::egui::pos2(600.0, 100.0),
        );
        let centers = [590.0, 570.0, 535.0, 500.0].map(|pointer_x| {
            let SeekStripCenter::Thumbnails { center_index } = seek_strip_center_at_drag_pointer(
                origin,
                eframe::egui::pos2(pointer_x, 100.0),
                100.0,
                1_000.0,
                60.0,
            )
            .expect("finite drag") else {
                panic!("thumbnail origin must stay on the thumbnail axis");
            };
            center_index
        });
        for (actual, expected) in centers.into_iter().zip([10.1, 10.3, 10.65, 11.0]) {
            assert!((actual - expected).abs() < 1.0e-6);
        }
        assert!(centers.windows(2).all(|pair| pair[0] < pair[1]));

        let release = seek_strip_center_at_drag_pointer(
            origin,
            eframe::egui::pos2(500.0, 100.0),
            100.0,
            1_000.0,
            60.0,
        );
        assert_eq!(
            release,
            Some(SeekStripCenter::Thumbnails { center_index: 11.0 })
        );

        let waveform_origin = SeekStripDragOrigin::new(
            SeekStripCenter::Waveform {
                center_time_secs: 90.0,
            },
            eframe::egui::pos2(600.0, 100.0),
        );
        let waveform_centers = [590.0, 570.0, 535.0, 500.0].map(|pointer_x| {
            let SeekStripCenter::Waveform { center_time_secs } = seek_strip_center_at_drag_pointer(
                waveform_origin,
                eframe::egui::pos2(pointer_x, 100.0),
                100.0,
                1_000.0,
                60.0,
            )
            .expect("finite drag") else {
                panic!("waveform origin must stay on the waveform axis");
            };
            center_time_secs
        });
        assert!(waveform_centers.windows(2).all(|pair| pair[0] < pair[1]));
        assert!((waveform_centers[3] - 96.0).abs() < 1.0e-6);
    }

    #[test]
    fn waveform_drag_is_time_linear_for_the_named_span() {
        assert_close(waveform_center_after_drag(90.0, 300.0, 1_200.0, 60.0), 75.0);
        assert_close(
            waveform_time_at_pointer(90.0, 900.0, 600.0, 1_200.0, 60.0),
            105.0,
        );
    }

    #[test]
    fn switching_modes_preserves_the_time_under_the_center_marker() {
        let axis = keyframe_axis(&[0.0, 5.0, 20.0, 50.0], &[0, 1, 2, 3]);
        let thumbnails = SeekStripCenter::Thumbnails { center_index: 1.5 };
        let waveform = switch_seek_strip_center(
            thumbnails,
            crate::settings::VideoSeekStripMode::Waveform,
            Some(&axis),
        )
        .unwrap();
        assert_close(waveform.time_secs(None), 12.5);

        let round_trip = switch_seek_strip_center(
            waveform,
            crate::settings::VideoSeekStripMode::Thumbnails,
            Some(&axis),
        )
        .unwrap();
        assert_close(round_trip.time_secs(Some(&axis)), 12.5);
    }

    #[test]
    fn repaint_polling_stops_when_visible_cells_are_settled_and_drag_is_idle() {
        assert!(!seek_strip_needs_repaint(SeekStripRepaintContext::default()));
        assert!(seek_strip_needs_repaint(SeekStripRepaintContext {
            visible_cells_pending: true,
            drag_active: false,
        }));
        assert!(seek_strip_needs_repaint(SeekStripRepaintContext {
            visible_cells_pending: false,
            drag_active: true,
        }));
    }

    #[test]
    fn rebuilding_minimum_gap_preserves_full_index_and_changes_only_adoption() {
        let axis = keyframe_axis(&[0.0, 1.0, 2.1, 4.5], &[0, 1, 2, 3]);
        let rebuilt = axis.with_minimum_gap(2.0);
        assert_eq!(
            rebuilt,
            StripAxis::KeyframeIndex {
                keyframes: vec![0.0, 1.0, 2.1, 4.5],
                adopted: vec![0, 2, 3],
            }
        );
    }
    /// 実機報告 2026-08-26: ストリップが動画の終わりまで届かない動画があった。数値は
    /// `アカリがやってきたぞっ` のもの。最後のキーフレーム 294.795 秒は直前の採用 283.083 秒
    /// から 11.7 秒しか離れておらず、15 秒の間引きで落ちて末尾 18 秒が空いていた。
    #[test]
    fn the_last_keyframe_is_always_adopted() {
        let keyframes = vec![0.0, 15.5, 31.0, 283.083, 294.795];
        let adopted = thin_keyframes(&keyframes, 15.0);
        assert_eq!(
            adopted.last(),
            Some(&(keyframes.len() - 1)),
            "間隔に満たなくても最後のキーフレームは採る"
        );
        assert_eq!(adopted, vec![0, 1, 2, 3, 4]);

        // 既に最後を採っているなら二重に入れない。
        let already = thin_keyframes(&[0.0, 20.0, 40.0], 15.0);
        assert_eq!(already, vec![0, 1, 2]);

        // 1 枚しかない場合も壊れない。
        assert_eq!(thin_keyframes(&[7.0], 15.0), vec![0]);
    }

    /// 実機報告 2026-08-26: 画像間隔を変えてもストリップの一覧が変わらない動画があった。
    /// 索引が不完全で格子軸へ落ちた場合、`with_minimum_gap` が格子には何もしていなかった。
    /// 数値は報告された mp4 のもの (172.7 秒、尺由来の下限は 1 秒)。
    #[test]
    fn changing_the_interval_rebuilds_the_time_grid_too() {
        let grid = StripAxis::TimeGrid {
            interval_secs: 1.0,
            fallback_interval_secs: 1.0,
            duration_secs: 172.733,
        };
        assert_eq!(grid.cell_count(), 173);

        let coarser = grid.with_minimum_gap(15.0);
        assert_eq!(coarser.cell_count(), 12, "15 秒にしたら 12 セルになる");
        assert_eq!(coarser.cell(1), Some(15.0));

        // 元へ戻せる: 尺由来の下限は軸が保持しているので、細かい側へも作り直せる。
        let back = coarser.with_minimum_gap(1.0);
        assert_eq!(back.cell_count(), 173);

        // 上限保護の下限は越えない。
        let capped = StripAxis::TimeGrid {
            interval_secs: 30.0,
            fallback_interval_secs: 30.0,
            duration_secs: 3600.0,
        };
        assert_eq!(capped.with_minimum_gap(0.5).cell_count(), 120);
    }
}
