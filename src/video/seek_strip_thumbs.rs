//! 動画シークストリップ用のキーフレームサムネイル抽出ワーカー。
//!
//! UI や presenter から独立した長寿命ワーカーが、動画 1 本につき補助デコーダを 1 個だけ
//! 所有する。窓要求は最新勝ちだが、既に公開した画像は要求 PTS ごとに保持し、新しい窓でも
//! 再利用する。SQLite/WebP と FFmpeg はすべてワーカースレッド内で扱う。補助デコーダは
//! メインプレイヤーの `GpuVideoDevice` / D3D ロックを共有せず、`LIVE_VIDEO_DECODE_THREADS`
//! にも含めない。

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};
use ffmpeg_the_third as ffmpeg;

use super::seek_strip::{
    CellRange, StripAxis, StripAxisDecision, StripLookahead, compute_strip_window,
    decide_strip_axis, enumerate_index_keyframes, thin_keyframes,
};
use super::tile_thumb_cache::TileThumbCache;

/// ストリップ専用の永続キャッシュ行幅。
///
/// タイルモードの固定 640px 行と主キーが衝突しない。縦動画も同じ品質で得られるよう、
/// 抽出時の外接矩形は 320x320 とする。
pub(crate) const STRIP_THUMB_EXTRACT_WIDTH: u32 = 320;
const STRIP_THUMB_EXTRACT_HEIGHT: u32 = 320;
/// Absorbs only rounding when FFmpeg index and packet timestamps are converted to seconds.
const FRAME_PTS_MATCH_EPSILON_SECS: f64 = 0.005;

/// セルが要求した時刻から作る、窓変更をまたいで安定したプロセス内キー。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct StripCellId(u64);

impl StripCellId {
    fn from_secs(secs: f64) -> Option<Self> {
        if !secs.is_finite() {
            return None;
        }
        let normalized = if secs == 0.0 { 0.0 } else { secs };
        Some(Self(normalized.to_bits()))
    }

    fn timestamp_ms(self) -> Option<i64> {
        timestamp_ms(self.target_secs())
    }

    pub(crate) fn target_secs(self) -> f64 {
        f64::from_bits(self.0)
    }
}

fn timestamp_ms(secs: f64) -> Option<i64> {
    if !secs.is_finite() {
        return None;
    }
    let millis = secs * 1000.0;
    (millis.is_finite() && millis >= i64::MIN as f64 && millis <= i64::MAX as f64)
        .then(|| millis.round() as i64)
}

/// 1 セル分の抽出済み画像。
#[derive(Clone, Debug)]
pub(crate) struct StripThumbnail {
    pub(crate) target_secs: f64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Arc<Vec<u8>>,
}

/// セル単位で UI へ返す、最終的な抽出失敗。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StripThumbnailFailure {
    InvalidCellTime,
    DecoderUnavailable(String),
    SeekFailed(String),
    DemuxFailed(String),
    DecodeFailed(String),
    ConvertFailed(String),
    NoFrame,
}

impl StripThumbnailFailure {
    fn should_retry_in_software(&self) -> bool {
        matches!(self, Self::DecodeFailed(_) | Self::ConvertFailed(_))
    }
}

/// セルは画像または型付き失敗のどちらかで決着する。
#[derive(Clone, Debug)]
pub(crate) enum StripThumbnailOutcome {
    Ready(StripThumbnail),
    Failed,
}

/// UI へ渡す前に確定する、1 セルの表示状態。
///
/// `None` はまだ worker が決着させていない pending であり、失敗と同一視しない。
#[derive(Clone, Copy, Debug)]
pub(crate) enum StripThumbnailCellState<'a> {
    Pending,
    Ready(&'a StripThumbnail),
    Failed,
}

pub(crate) fn decide_strip_thumbnail_cell_state<'a>(
    outcome: Option<&'a StripThumbnailOutcome>,
    latest_request_failure: Option<&StripThumbnailFailure>,
) -> StripThumbnailCellState<'a> {
    match outcome {
        Some(StripThumbnailOutcome::Ready(thumbnail)) => StripThumbnailCellState::Ready(thumbnail),
        Some(StripThumbnailOutcome::Failed) => StripThumbnailCellState::Failed,
        None if latest_request_failure.is_some() => StripThumbnailCellState::Failed,
        None => StripThumbnailCellState::Pending,
    }
}

/// ワーカースレッド全体の状態。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StripThumbnailWorkerStatus {
    Running,
    DecoderUnavailable(String),
    Cancelled,
    ThreadSpawnFailed(String),
}

/// 補助 decoder が実際に選んだ decode 経路。
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StripThumbnailDecodePath {
    HardwareD3d11va,
    Software,
}

/// 実素材 probe と障害解析のために snapshot へ載せる decode 経路の履歴。
#[cfg(test)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StripThumbnailDecodeDiagnostics {
    pub(crate) initial_path: Option<StripThumbnailDecodePath>,
    pub(crate) current_path: Option<StripThumbnailDecodePath>,
    pub(crate) software_retry_failure: Option<StripThumbnailFailure>,
}

/// セル列を描くか、strip 全体の terminal notice へ置き換えるか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StripThumbnailDisplayScope {
    Cells,
    StripUnavailable,
}

pub(crate) fn decide_strip_thumbnail_display_scope(
    axis_resolved: bool,
    status: &StripThumbnailWorkerStatus,
) -> StripThumbnailDisplayScope {
    if axis_resolved && matches!(status, StripThumbnailWorkerStatus::DecoderUnavailable(_)) {
        StripThumbnailDisplayScope::StripUnavailable
    } else {
        StripThumbnailDisplayScope::Cells
    }
}

/// ワーカースレッド上で解決するストリップ軸。
#[derive(Clone, Debug)]
pub(crate) enum StripAxisResolution {
    Resolving,
    Ready(Arc<StripAxis>),
    Failed(String),
}

/// UI がロックを保持せず参照できる現在スナップショット。
#[derive(Clone, Debug)]
pub(crate) struct StripThumbnailSnapshot {
    pub(crate) axis: StripAxisResolution,
    pub(crate) cells: BTreeMap<StripCellId, StripThumbnailOutcome>,
    pub(crate) status: StripThumbnailWorkerStatus,
    pub(crate) latest_request_failures: Vec<(usize, StripThumbnailFailure)>,
    #[cfg(test)]
    pub(crate) decode_diagnostics: StripThumbnailDecodeDiagnostics,
}

impl StripThumbnailSnapshot {
    pub(crate) fn outcome_for_secs(&self, target_secs: f64) -> Option<&StripThumbnailOutcome> {
        let id = StripCellId::from_secs(target_secs)?;
        self.cells.get(&id)
    }

    pub(crate) fn latest_failure_for_index(&self, index: usize) -> Option<&StripThumbnailFailure> {
        self.latest_request_failures
            .iter()
            .find_map(|(failed_index, failure)| (*failed_index == index).then_some(failure))
    }

    pub(crate) fn display_scope(&self) -> StripThumbnailDisplayScope {
        decide_strip_thumbnail_display_scope(
            matches!(&self.axis, StripAxisResolution::Ready(_)),
            &self.status,
        )
    }
}

#[cfg(test)]
fn record_decoder_path(state: &Mutex<SharedState>, decoder: &SeekStripDecoder) {
    let path = if decoder.hw_decode_active() {
        StripThumbnailDecodePath::HardwareD3d11va
    } else {
        StripThumbnailDecodePath::Software
    };
    let mut shared = lock_recover(state);
    shared.decode_diagnostics.initial_path.get_or_insert(path);
    shared.decode_diagnostics.current_path = Some(path);
}

#[cfg(not(test))]
fn record_decoder_path(_state: &Mutex<SharedState>, _decoder: &SeekStripDecoder) {}

#[cfg(test)]
fn record_software_retry(state: &Mutex<SharedState>, failure: &StripThumbnailFailure) {
    lock_recover(state)
        .decode_diagnostics
        .software_retry_failure = Some(failure.clone());
}

#[cfg(not(test))]
fn record_software_retry(_state: &Mutex<SharedState>, _failure: &StripThumbnailFailure) {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StripWindowSpec {
    pub(crate) center_index: f64,
    pub(crate) visible_count: usize,
    pub(crate) lookahead: StripLookahead,
}

impl StripWindowSpec {
    pub(crate) const fn new(
        center_index: f64,
        visible_count: usize,
        lookahead: StripLookahead,
    ) -> Self {
        Self {
            center_index,
            visible_count,
            lookahead,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlannedStripCell {
    pub(crate) index: usize,
    pub(crate) id: StripCellId,
    pub(crate) target_secs: f64,
    pub(crate) timestamp_ms: i64,
    pub(crate) visible: bool,
    pub(crate) exact_keyframe: bool,
    /// Maximum timestamp-domain distance before and after this cell.
    ///
    /// An indexed cell stops at the adjacent raw index entry in each direction, so a
    /// DTS-to-PTS mapping cannot cross into the neighboring indexed scene. A TimeGrid
    /// cell uses one grid interval in both directions, avoiding arbitrarily stale frames.
    pub(crate) frame_match_tolerance: FrameMatchTolerance,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameMatchTolerance {
    before_secs: f64,
    after_secs: f64,
}

impl FrameMatchTolerance {
    fn symmetric(secs: f64) -> Self {
        Self {
            before_secs: secs,
            after_secs: secs,
        }
    }

    fn permits_shift(self, shift_secs: f64) -> bool {
        let tolerance = if shift_secs < 0.0 {
            self.before_secs
        } else {
            self.after_secs
        };
        shift_secs.is_finite()
            && tolerance.is_finite()
            && tolerance >= 0.0
            && shift_secs.abs() <= tolerance + FRAME_PTS_MATCH_EPSILON_SECS
    }
}

fn frame_match_tolerance(axis: &StripAxis, index: usize) -> Option<FrameMatchTolerance> {
    match axis {
        StripAxis::KeyframeIndex { keyframes, adopted } => {
            let raw_index = *adopted.get(index)?;
            let target = *keyframes.get(raw_index)?;
            let previous_gap = keyframes[..raw_index]
                .iter()
                .rev()
                .map(|previous| target - *previous)
                .find(|gap| gap.is_finite() && *gap > 0.0);
            let next_gap = keyframes[raw_index.saturating_add(1)..]
                .iter()
                .map(|next| *next - target)
                .find(|gap| gap.is_finite() && *gap > 0.0);
            let fallback_gap = previous_gap
                .or(next_gap)
                .unwrap_or(FRAME_PTS_MATCH_EPSILON_SECS);
            Some(FrameMatchTolerance {
                before_secs: previous_gap
                    .unwrap_or(fallback_gap)
                    .max(FRAME_PTS_MATCH_EPSILON_SECS),
                after_secs: next_gap
                    .unwrap_or(fallback_gap)
                    .max(FRAME_PTS_MATCH_EPSILON_SECS),
            })
        }
        StripAxis::TimeGrid { interval_secs, .. } => {
            Some(FrameMatchTolerance::symmetric(*interval_secs))
        }
    }
}

fn cell_presentation_target_secs(
    cell: &PlannedStripCell,
    indexed_presentation_targets: &BTreeMap<StripCellId, f64>,
) -> f64 {
    indexed_presentation_targets
        .get(&cell.id)
        .copied()
        .unwrap_or(cell.target_secs)
}

fn indexed_packet_presentation_target_secs(
    cell: &PlannedStripCell,
    packet_dts_secs: f64,
    packet_pts_secs: f64,
) -> Option<f64> {
    if !cell.exact_keyframe
        || !packet_dts_secs.is_finite()
        || !packet_pts_secs.is_finite()
        || packet_pts_secs < 0.0
        || (packet_dts_secs - cell.target_secs).abs() > FRAME_PTS_MATCH_EPSILON_SECS
        || !cell
            .frame_match_tolerance
            .permits_shift(packet_pts_secs - cell.target_secs)
    {
        return None;
    }
    Some(packet_pts_secs)
}

fn is_preceding_frame_within_tolerance(
    target_secs: f64,
    frame_pts_secs: f64,
    tolerance: FrameMatchTolerance,
) -> bool {
    target_secs.is_finite()
        && frame_pts_secs.is_finite()
        && tolerance.before_secs.is_finite()
        && tolerance.before_secs >= 0.0
        && frame_pts_secs <= target_secs + FRAME_PTS_MATCH_EPSILON_SECS
        && target_secs - frame_pts_secs <= tolerance.before_secs + FRAME_PTS_MATCH_EPSILON_SECS
}

fn is_following_frame_within_tolerance(
    target_secs: f64,
    frame_pts_secs: f64,
    tolerance: FrameMatchTolerance,
) -> bool {
    target_secs.is_finite()
        && frame_pts_secs.is_finite()
        && tolerance.after_secs.is_finite()
        && tolerance.after_secs >= 0.0
        && frame_pts_secs >= target_secs - FRAME_PTS_MATCH_EPSILON_SECS
        && frame_pts_secs - target_secs <= tolerance.after_secs + FRAME_PTS_MATCH_EPSILON_SECS
}

#[derive(Debug, PartialEq, Eq)]
struct CellsBeforeFrameMatches {
    preceding_indices: Vec<usize>,
    following_indices: Vec<usize>,
    failed_indices: Vec<usize>,
    next_cursor: usize,
}

fn match_cells_before_frame(
    cells: &[PlannedStripCell],
    cursor: usize,
    previous_pts_secs: Option<f64>,
    current_pts_secs: f64,
    indexed_presentation_targets: &BTreeMap<StripCellId, f64>,
) -> CellsBeforeFrameMatches {
    let mut next_cursor = cursor;
    let mut preceding_indices = Vec::new();
    let mut following_indices = Vec::new();
    let mut failed_indices = Vec::new();
    while let Some(cell) = cells.get(next_cursor) {
        let target_secs = cell_presentation_target_secs(cell, indexed_presentation_targets);
        if target_secs >= current_pts_secs - FRAME_PTS_MATCH_EPSILON_SECS {
            break;
        }
        if previous_pts_secs.is_some_and(|previous_pts_secs| {
            is_preceding_frame_within_tolerance(
                target_secs,
                previous_pts_secs,
                cell.frame_match_tolerance,
            )
        }) {
            preceding_indices.push(next_cursor);
        } else if is_following_frame_within_tolerance(
            target_secs,
            current_pts_secs,
            cell.frame_match_tolerance,
        ) {
            following_indices.push(next_cursor);
        } else {
            failed_indices.push(next_cursor);
        }
        next_cursor += 1;
    }
    CellsBeforeFrameMatches {
        preceding_indices,
        following_indices,
        failed_indices,
        next_cursor,
    }
}

/// 1 窓について UI 状態だけから決められる作業。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StripWindowWork {
    pub(crate) visible_range: Option<CellRange>,
    pub(crate) visible_ids: Vec<StripCellId>,
    /// DB を照会するセル。中心から外側の順で、既決着セルを含まない。
    pub(crate) cache_lookup: Vec<PlannedStripCell>,
    /// 外部データが不正で安定したセル ID を作れなかった添字。
    pub(crate) invalid_cells: Vec<usize>,
}

/// 窓と既決着セルから、DB 照会対象を中心優先で作る純関数。
pub(crate) fn plan_strip_window_work(
    axis: &StripAxis,
    spec: StripWindowSpec,
    settled: &BTreeSet<StripCellId>,
) -> StripWindowWork {
    let visible_range = compute_strip_window(
        spec.center_index,
        spec.visible_count,
        StripLookahead::default(),
        axis.cell_count(),
        None,
    )
    .ready;
    let requested_range = compute_strip_window(
        spec.center_index,
        spec.visible_count,
        spec.lookahead,
        axis.cell_count(),
        None,
    )
    .ready;

    let mut visible_ids = Vec::new();
    let mut cache_lookup = Vec::new();
    let mut invalid_cells = Vec::new();
    let mut seen_ids = BTreeSet::new();
    let exact_keyframe = matches!(axis, StripAxis::KeyframeIndex { .. });

    if let Some(range) = requested_range.as_ref() {
        for index in range.start()..=range.end() {
            let visible = visible_range.as_ref().is_some_and(|visible_range| {
                index >= visible_range.start() && index <= visible_range.end()
            });
            let Some(target_secs) = axis.cell(index) else {
                invalid_cells.push(index);
                continue;
            };
            let Some(id) = StripCellId::from_secs(target_secs) else {
                invalid_cells.push(index);
                continue;
            };
            let Some(timestamp_ms) = id.timestamp_ms() else {
                invalid_cells.push(index);
                continue;
            };
            let Some(frame_match_tolerance) = frame_match_tolerance(axis, index) else {
                invalid_cells.push(index);
                continue;
            };
            if visible && !visible_ids.contains(&id) {
                visible_ids.push(id);
            }
            if settled.contains(&id) || !seen_ids.insert(id) {
                continue;
            }
            cache_lookup.push(PlannedStripCell {
                index,
                id,
                target_secs,
                timestamp_ms,
                visible,
                exact_keyframe,
                frame_match_tolerance,
            });
        }
    }

    cache_lookup.sort_by(|left, right| {
        let left_distance = (left.index as f64 - spec.center_index).abs();
        let right_distance = (right.index as f64 - spec.center_index).abs();
        left_distance
            .total_cmp(&right_distance)
            .then_with(|| left.index.cmp(&right.index))
    });

    StripWindowWork {
        visible_range,
        visible_ids,
        cache_lookup,
        invalid_cells,
    }
}

/// バッチ照会結果から、実際に復号する miss だけを中心優先順のまま残す純関数。
pub(crate) fn plan_strip_decode_cells(
    cache_lookup: &[PlannedStripCell],
    cache_hits: &BTreeSet<StripCellId>,
) -> Vec<PlannedStripCell> {
    cache_lookup
        .iter()
        .filter(|cell| !cache_hits.contains(&cell.id))
        .cloned()
        .collect()
}

/// 1 回の backward seek と前方復号で処理する連続セル列。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StripDecodeRun {
    /// PTS 昇順。必ず添字も連続する。
    pub(crate) cells: Vec<PlannedStripCell>,
}

/// セルを可視優先・中心優先の連続 run にまとめる純関数。
///
/// 最も中心に近い可視 miss は単独 run にして最初に出す。残りは最大連続範囲へまとめ、
/// 可視 run をすべて終えてから lookahead run を処理する。可視範囲は中心・右・左の最大
/// 3 seek、lookahead も左右ごとに 1 seek へまとめ、セルごとの seek にはしない。
pub(crate) fn group_strip_decode_runs(
    cells: &[PlannedStripCell],
    center_index: f64,
) -> Vec<StripDecodeRun> {
    let mut visible: Vec<_> = cells.iter().filter(|cell| cell.visible).cloned().collect();
    let lookahead: Vec<_> = cells.iter().filter(|cell| !cell.visible).cloned().collect();
    let mut runs = Vec::new();

    if let Some((pivot_pos, _)) = visible.iter().enumerate().min_by(|(_, left), (_, right)| {
        let left_distance = (left.index as f64 - center_index).abs();
        let right_distance = (right.index as f64 - center_index).abs();
        left_distance
            .total_cmp(&right_distance)
            .then_with(|| left.index.cmp(&right.index))
    }) {
        let pivot = visible.remove(pivot_pos);
        runs.push(StripDecodeRun { cells: vec![pivot] });
    }

    runs.extend(group_one_priority_band(visible, center_index));
    runs.extend(group_one_priority_band(lookahead, center_index));
    runs
}

fn group_one_priority_band(
    mut cells: Vec<PlannedStripCell>,
    center_index: f64,
) -> Vec<StripDecodeRun> {
    cells.sort_by_key(|cell| cell.index);
    let mut grouped: Vec<Vec<PlannedStripCell>> = Vec::new();
    for cell in cells {
        let extends_last = grouped
            .last()
            .and_then(|run| run.last())
            .is_some_and(|previous| previous.index.checked_add(1) == Some(cell.index));
        if extends_last {
            if let Some(run) = grouped.last_mut() {
                run.push(cell);
            }
        } else {
            grouped.push(vec![cell]);
        }
    }

    grouped.sort_by(|left, right| {
        let left_distance = run_distance(left, center_index);
        let right_distance = run_distance(right, center_index);
        let left_is_right = left
            .first()
            .is_some_and(|cell| cell.index as f64 >= center_index);
        let right_is_right = right
            .first()
            .is_some_and(|cell| cell.index as f64 >= center_index);
        left_distance
            .total_cmp(&right_distance)
            // 同距離なら前方復号の公開順が自然な右側を先にする。
            .then_with(|| right_is_right.cmp(&left_is_right))
    });

    grouped
        .into_iter()
        .map(|cells| StripDecodeRun { cells })
        .collect()
}

fn run_distance(run: &[PlannedStripCell], center_index: f64) -> f64 {
    run.iter()
        .map(|cell| (cell.index as f64 - center_index).abs())
        .min_by(f64::total_cmp)
        .unwrap_or(f64::INFINITY)
}

/// supersede 時に保持済み結果と未処理 work を分ける純粋な判断結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StripSupersedeDecision {
    pub(crate) kept: BTreeSet<StripCellId>,
    pub(crate) discarded: Vec<StripCellId>,
}

pub(crate) fn decide_strip_supersede(
    settled: &BTreeSet<StripCellId>,
    active_cells: &[PlannedStripCell],
) -> StripSupersedeDecision {
    let discarded = active_cells
        .iter()
        .filter(|cell| !settled.contains(&cell.id))
        .map(|cell| cell.id)
        .collect();
    StripSupersedeDecision {
        kept: settled.clone(),
        discarded,
    }
}

#[derive(Clone)]
struct WindowRequest {
    id: u64,
    axis: Arc<StripAxis>,
    spec: StripWindowSpec,
    requested_at: Instant,
}

#[derive(Default)]
struct LatestWindowRequest {
    pending: Option<WindowRequest>,
}

impl LatestWindowRequest {
    fn replace(&mut self, request: WindowRequest) -> Option<WindowRequest> {
        self.pending.replace(request)
    }

    fn take(&mut self) -> Option<WindowRequest> {
        self.pending.take()
    }

    fn supersedes(&self, request_id: u64) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.id > request_id)
    }
}

struct SharedState {
    axis: StripAxisResolution,
    cells: BTreeMap<StripCellId, StripThumbnailOutcome>,
    status: StripThumbnailWorkerStatus,
    latest_request_failures: Vec<(usize, StripThumbnailFailure)>,
    #[cfg(test)]
    decode_diagnostics: StripThumbnailDecodeDiagnostics,
    fill_wait_emitted_request_id: Option<u64>,
}

impl SharedState {
    fn new() -> Self {
        Self {
            axis: StripAxisResolution::Resolving,
            cells: BTreeMap::new(),
            status: StripThumbnailWorkerStatus::Running,
            latest_request_failures: Vec::new(),
            #[cfg(test)]
            decode_diagnostics: StripThumbnailDecodeDiagnostics::default(),
            fill_wait_emitted_request_id: None,
        }
    }

    fn snapshot(&self) -> StripThumbnailSnapshot {
        StripThumbnailSnapshot {
            axis: self.axis.clone(),
            cells: self.cells.clone(),
            status: self.status.clone(),
            latest_request_failures: self.latest_request_failures.clone(),
            #[cfg(test)]
            decode_diagnostics: self.decode_diagnostics.clone(),
        }
    }

    fn settled_ids(&self) -> BTreeSet<StripCellId> {
        self.cells.keys().copied().collect()
    }
}

fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 停止済みワーカーへの要求を呼び出し側へ返す。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StripThumbnailRequestError {
    WorkerStopped,
}

/// 動画 1 本につき 1 個作る、latest-wins のストリップ抽出ワーカー。
pub(crate) struct SeekStripThumbnailWorker {
    requests: Arc<Mutex<LatestWindowRequest>>,
    wake_tx: Sender<()>,
    state: Arc<Mutex<SharedState>>,
    cancel: Arc<AtomicBool>,
    next_request_id: AtomicU64,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct SeekStripWorkerConfig {
    path: PathBuf,
    hw_decode: bool,
    cache: Option<Arc<TileThumbCache>>,
    duration_secs: f64,
    min_gap_secs: f64,
    fallback_interval_secs: f64,
}

impl SeekStripThumbnailWorker {
    /// ワーカーを起動する。DB と動画は生成したスレッド内でのみ触る。
    pub(crate) fn spawn(
        path: PathBuf,
        hw_decode: bool,
        cache: Option<Arc<TileThumbCache>>,
        duration_secs: f64,
        min_gap_secs: f64,
        fallback_interval_secs: f64,
    ) -> Self {
        let (wake_tx, wake_rx) = bounded::<()>(1);
        let requests = Arc::new(Mutex::new(LatestWindowRequest::default()));
        let state = Arc::new(Mutex::new(SharedState::new()));
        let cancel = Arc::new(AtomicBool::new(false));

        let worker_requests = Arc::clone(&requests);
        let worker_state = Arc::clone(&state);
        let worker_cancel = Arc::clone(&cancel);
        let config = SeekStripWorkerConfig {
            path,
            hw_decode,
            cache,
            duration_secs,
            min_gap_secs,
            fallback_interval_secs,
        };
        let thread_result = std::thread::Builder::new()
            .name("video-seek-strip-thumbs".into())
            .spawn(move || {
                run_worker(
                    config,
                    wake_rx,
                    worker_requests,
                    worker_state,
                    worker_cancel,
                );
            });
        let thread = match thread_result {
            Ok(handle) => Some(handle),
            Err(error) => {
                let mut shared = lock_recover(&state);
                shared.status = StripThumbnailWorkerStatus::ThreadSpawnFailed(error.to_string());
                shared.axis = StripAxisResolution::Failed(error.to_string());
                None
            }
        };

        Self {
            requests,
            wake_tx,
            state,
            cancel,
            next_request_id: AtomicU64::new(1),
            thread,
        }
    }

    /// `(axis, center_index, visible_count, lookahead)` の最新窓を要求する。
    pub(crate) fn request(
        &self,
        axis: Arc<StripAxis>,
        center_index: f64,
        visible_count: usize,
        lookahead: StripLookahead,
    ) -> Result<u64, StripThumbnailRequestError> {
        if self.cancel.load(Ordering::Acquire)
            || matches!(
                lock_recover(&self.state).status,
                StripThumbnailWorkerStatus::Cancelled
                    | StripThumbnailWorkerStatus::ThreadSpawnFailed(_)
            )
        {
            return Err(StripThumbnailRequestError::WorkerStopped);
        }

        let request_id = self.next_request_id.fetch_add(1, Ordering::AcqRel);
        let request = WindowRequest {
            id: request_id,
            axis,
            spec: StripWindowSpec::new(center_index, visible_count, lookahead),
            requested_at: Instant::now(),
        };
        lock_recover(&self.requests).replace(request);
        let _ = self.wake_tx.try_send(());
        Ok(request_id)
    }

    /// 現在までの画像・型付き失敗を shallow clone して返す。
    pub(crate) fn snapshot(&self) -> StripThumbnailSnapshot {
        lock_recover(&self.state).snapshot()
    }

    /// close / 動画切替 / fullscreen 終了で呼ぶ停止境界。
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        lock_recover(&self.state).status = StripThumbnailWorkerStatus::Cancelled;
        let _ = self.wake_tx.try_send(());
    }
}

impl Drop for SeekStripThumbnailWorker {
    fn drop(&mut self) {
        // seek / readback / SQLite 中の join は UI を止めるため、cancel だけ立てて detach する。
        self.cancel();
        let _ = self.thread.take();
    }
}

#[derive(Clone)]
struct DeferredCacheEntry {
    target_secs: f64,
    width: u32,
    height: u32,
    rgba: Arc<Vec<u8>>,
}

/// 永続キャッシュは次回も planner が要求するセル時刻で引く。
///
/// 復号フレームの実 PTS は表示結果に保持するが、edit list や DTS ベース索引では
/// セル時刻と一致しないため DB キーには使わない。
fn queue_deferred_cache_write(
    pending: &mut BTreeMap<i64, DeferredCacheEntry>,
    decoded: &DecodedCell,
) {
    pending
        .entry(decoded.cell.timestamp_ms)
        .or_insert(DeferredCacheEntry {
            target_secs: decoded.cell.target_secs,
            width: decoded.thumbnail.width,
            height: decoded.thumbnail.height,
            rgba: Arc::clone(&decoded.thumbnail.rgba),
        });
}

#[derive(Default)]
struct DecodeMetrics {
    seek: Duration,
    transform: Duration,
    total: Duration,
    decoded_frames: usize,
    published_cells: usize,
    runs: usize,
    discarded_on_supersede: usize,
}

impl DecodeMetrics {
    fn merge(&mut self, other: RunDecodeMetrics) {
        self.seek += other.seek;
        self.transform += other.transform;
        self.total += other.total;
        self.decoded_frames += other.decoded_frames;
        self.published_cells += other.published_cells;
        self.runs += 1;
    }

    fn decode_ms(&self) -> f64 {
        self.total
            .saturating_sub(self.seek)
            .saturating_sub(self.transform)
            .as_secs_f64()
            * 1000.0
    }
}

struct WorkerRuntime {
    decoder: Option<SeekStripDecoder>,
    hw_decode_failed: bool,
    decoder_unavailable: Option<String>,
    open_event_emitted: bool,
    deferred_cache_writes: BTreeMap<i64, DeferredCacheEntry>,
}

impl WorkerRuntime {
    fn new() -> Self {
        Self {
            decoder: None,
            hw_decode_failed: false,
            decoder_unavailable: None,
            open_event_emitted: false,
            deferred_cache_writes: BTreeMap::new(),
        }
    }
}

fn run_worker(
    config: SeekStripWorkerConfig,
    wake_rx: Receiver<()>,
    requests: Arc<Mutex<LatestWindowRequest>>,
    state: Arc<Mutex<SharedState>>,
    cancel: Arc<AtomicBool>,
) {
    let SeekStripWorkerConfig {
        path,
        hw_decode,
        cache,
        duration_secs,
        min_gap_secs,
        fallback_interval_secs,
    } = config;
    let mut runtime = WorkerRuntime::new();
    let video_mtime = std::fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);

    let axis = resolve_strip_axis(&path, duration_secs, min_gap_secs, fallback_interval_secs);
    if cancel.load(Ordering::Acquire) {
        lock_recover(&state).status = StripThumbnailWorkerStatus::Cancelled;
        return;
    }
    match axis {
        Ok(axis) => lock_recover(&state).axis = StripAxisResolution::Ready(Arc::new(axis)),
        Err(error) => {
            let mut shared = lock_recover(&state);
            shared.status = StripThumbnailWorkerStatus::DecoderUnavailable(error.clone());
            shared.axis = StripAxisResolution::Failed(error);
            return;
        }
    }

    while !cancel.load(Ordering::Acquire) {
        let request = lock_recover(&requests).take();
        if let Some(request) = request {
            process_window_request(
                &path,
                hw_decode,
                cache.as_ref(),
                video_mtime,
                &requests,
                &state,
                &cancel,
                &mut runtime,
                request,
            );
            continue;
        }

        flush_deferred_cache_writes(
            &path,
            cache.as_ref(),
            video_mtime,
            &requests,
            &cancel,
            &mut runtime.deferred_cache_writes,
        );
        if lock_recover(&requests).pending.is_some() {
            continue;
        }
        match wake_rx.recv() {
            Ok(()) => {}
            Err(_) => break,
        }
    }

    if cancel.load(Ordering::Acquire) {
        lock_recover(&state).status = StripThumbnailWorkerStatus::Cancelled;
    }
}

fn fallback_strip_axis(
    duration_secs: f64,
    fallback_interval_secs: f64,
) -> Result<StripAxis, String> {
    if !duration_secs.is_finite() {
        return Err("video duration is not finite".to_string());
    }
    if duration_secs <= 0.0 {
        return Err("video duration is not positive".to_string());
    }
    if !fallback_interval_secs.is_finite() {
        return Err("fallback strip interval is invalid".to_string());
    }
    if fallback_interval_secs <= 0.0 {
        return Err("fallback strip interval is not positive".to_string());
    }
    Ok(StripAxis::TimeGrid {
        interval_secs: fallback_interval_secs,
        duration_secs,
    })
}

fn resolve_strip_axis(
    path: &Path,
    duration_secs: f64,
    min_gap_secs: f64,
    fallback_interval_secs: f64,
) -> Result<StripAxis, String> {
    ffmpeg::init().map_err(|error| error.to_string())?;
    let mut input = match ffmpeg::format::input(path) {
        Ok(input) => input,
        Err(_) => return fallback_strip_axis(duration_secs, fallback_interval_secs),
    };
    let (stream_index, time_base) = {
        let Some(stream) = input.streams().best(ffmpeg::media::Type::Video) else {
            return fallback_strip_axis(duration_secs, fallback_interval_secs);
        };
        (stream.index(), stream.time_base())
    };
    let Some(keyframes) = enumerate_index_keyframes(&mut input, stream_index, time_base) else {
        return fallback_strip_axis(duration_secs, fallback_interval_secs);
    };
    let covered_secs = keyframes.last().copied().unwrap_or_default();
    match decide_strip_axis(keyframes.len(), covered_secs, duration_secs) {
        StripAxisDecision::KeyframeIndex => {
            let adopted = thin_keyframes(&keyframes, min_gap_secs);
            if adopted.is_empty() {
                fallback_strip_axis(duration_secs, fallback_interval_secs)
            } else {
                Ok(StripAxis::KeyframeIndex { keyframes, adopted })
            }
        }
        StripAxisDecision::TimeGrid(_) => {
            fallback_strip_axis(duration_secs, fallback_interval_secs)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn process_window_request(
    path: &Path,
    hw_decode: bool,
    cache: Option<&Arc<TileThumbCache>>,
    video_mtime: i64,
    requests: &Mutex<LatestWindowRequest>,
    state: &Mutex<SharedState>,
    cancel: &AtomicBool,
    runtime: &mut WorkerRuntime,
    request: WindowRequest,
) {
    {
        let mut shared = lock_recover(state);
        shared.latest_request_failures.clear();
        shared.fill_wait_emitted_request_id = None;
    }

    let settled = lock_recover(state).settled_ids();
    let work = plan_strip_window_work(&request.axis, request.spec, &settled);
    for &index in &work.invalid_cells {
        lock_recover(state)
            .latest_request_failures
            .push((index, StripThumbnailFailure::InvalidCellTime));
    }

    if maybe_emit_fill_wait(path, &request, &work, requests, state) {
        emit_open_once(path, &request.axis, 0, "memory", 0.0, runtime);
        return;
    }

    let cache_hits = load_cached_cells(
        path,
        cache,
        video_mtime,
        requests,
        state,
        cancel,
        &request,
        &work,
        &work.cache_lookup,
    );
    let _ = maybe_emit_fill_wait(path, &request, &work, requests, state);

    if cancelled_or_superseded(cancel, requests, request.id) {
        emit_open_once(
            path,
            &request.axis,
            cache_hits.len(),
            "cache_only",
            0.0,
            runtime,
        );
        return;
    }

    let mut decode_cells = plan_strip_decode_cells(&work.cache_lookup, &cache_hits);
    decode_cells.retain(|cell| !lock_recover(state).cells.contains_key(&cell.id));
    if decode_cells.is_empty() {
        emit_open_once(
            path,
            &request.axis,
            cache_hits.len(),
            "cache_only",
            0.0,
            runtime,
        );
        return;
    }

    let open_t0 = Instant::now();
    if runtime.decoder.is_none() && runtime.decoder_unavailable.is_none() {
        let use_hw = hw_decode && !runtime.hw_decode_failed;
        match SeekStripDecoder::open(path, use_hw) {
            Ok(decoder) => {
                record_decoder_path(state, &decoder);
                runtime.decoder = Some(decoder);
            }
            Err(error) => {
                runtime.decoder_unavailable = Some(error.clone());
                lock_recover(state).status =
                    StripThumbnailWorkerStatus::DecoderUnavailable(error.clone());
            }
        }
    }
    let open_ms = open_t0.elapsed().as_secs_f64() * 1000.0;
    let open_path = runtime
        .decoder
        .as_ref()
        .map(SeekStripDecoder::decode_path)
        .unwrap_or("unavailable");
    emit_open_once(
        path,
        &request.axis,
        cache_hits.len(),
        open_path,
        open_ms,
        runtime,
    );

    if let Some(error) = runtime.decoder_unavailable.clone() {
        for cell in &decode_cells {
            publish_failure(
                state,
                cell,
                StripThumbnailFailure::DecoderUnavailable(error.clone()),
            );
        }
        return;
    }

    let mut metrics = DecodeMetrics::default();
    let mut superseded = false;
    let mut retry_with_software = true;
    while retry_with_software && !cancel.load(Ordering::Acquire) {
        retry_with_software = false;
        decode_cells.retain(|cell| !lock_recover(state).cells.contains_key(&cell.id));
        let runs = group_strip_decode_runs(&decode_cells, request.spec.center_index);
        for run in runs {
            if cancelled_or_superseded(cancel, requests, request.id) {
                let decision =
                    decide_strip_supersede(&lock_recover(state).settled_ids(), &run.cells);
                metrics.discarded_on_supersede += decision.discarded.len();
                superseded = !cancel.load(Ordering::Acquire);
                break;
            }

            let decoder_was_hw = runtime
                .decoder
                .as_ref()
                .is_some_and(SeekStripDecoder::hw_decode_active);
            let result = {
                let Some(decoder) = runtime.decoder.as_mut() else {
                    break;
                };
                let pending_writes = &mut runtime.deferred_cache_writes;
                let mut publish = |decoded: DecodedCell| {
                    publish_ready(state, &decoded.cell, decoded.thumbnail.clone());
                    queue_deferred_cache_write(pending_writes, &decoded);
                    let _ = maybe_emit_fill_wait(path, &request, &work, requests, state);
                };
                decoder.decode_run(&run, request.id, requests, cancel, &mut publish)
            };
            for (cell, failure) in &result.cell_failures {
                publish_failure(state, cell, failure.clone());
            }
            metrics.merge(result.metrics);
            match result.stop {
                RunStop::Complete => {}
                RunStop::Cancelled => break,
                RunStop::Superseded => {
                    let decision =
                        decide_strip_supersede(&lock_recover(state).settled_ids(), &run.cells);
                    metrics.discarded_on_supersede += decision.discarded.len();
                    superseded = true;
                    break;
                }
                RunStop::Failed(failure)
                    if decoder_was_hw && failure.should_retry_in_software() =>
                {
                    runtime.hw_decode_failed = true;
                    runtime.decoder = None;
                    record_software_retry(state, &failure);
                    match SeekStripDecoder::open(path, false) {
                        Ok(decoder) => {
                            record_decoder_path(state, &decoder);
                            runtime.decoder = Some(decoder);
                            retry_with_software = true;
                        }
                        Err(error) => {
                            runtime.decoder_unavailable = Some(error.clone());
                            lock_recover(state).status =
                                StripThumbnailWorkerStatus::DecoderUnavailable(error.clone());
                            for cell in &decode_cells {
                                publish_failure(
                                    state,
                                    cell,
                                    StripThumbnailFailure::DecoderUnavailable(error.clone()),
                                );
                            }
                        }
                    }
                    crate::logger::log(format!(
                        "video-seek-strip-thumbs HW failure; retrying in software: {:?}",
                        failure
                    ));
                    break;
                }
                RunStop::Failed(failure) => {
                    for cell in &run.cells {
                        publish_failure(state, cell, failure.clone());
                    }
                }
            }
        }
    }

    emit_decode(path, &request, runtime, &metrics, superseded);
}

#[allow(clippy::too_many_arguments)]
fn load_cached_cells(
    path: &Path,
    cache: Option<&Arc<TileThumbCache>>,
    video_mtime: i64,
    requests: &Mutex<LatestWindowRequest>,
    state: &Mutex<SharedState>,
    cancel: &AtomicBool,
    request: &WindowRequest,
    work: &StripWindowWork,
    cells: &[PlannedStripCell],
) -> BTreeSet<StripCellId> {
    let Some(cache) = cache else {
        return BTreeSet::new();
    };
    let timestamps: Vec<i64> = cells.iter().map(|cell| cell.timestamp_ms).collect();
    let rows = cache.lookup_webp_batch(path, &timestamps, video_mtime, STRIP_THUMB_EXTRACT_WIDTH);
    let mut hits = BTreeSet::new();
    for (cell, row) in cells.iter().zip(rows) {
        if cancelled_or_superseded(cancel, requests, request.id) {
            break;
        }
        let Some(webp) = row else {
            continue;
        };
        let Some((width, height, rgba)) = crate::catalog::decode_thumb_to_rgba(&webp) else {
            crate::logger::log(format!(
                "video-seek-strip-thumbs corrupt cache row: path={} timestamp_ms={}",
                path.display(),
                cell.timestamp_ms
            ));
            continue;
        };
        let thumbnail = StripThumbnail {
            target_secs: cell.target_secs,
            width,
            height,
            rgba: Arc::new(rgba),
        };
        publish_ready(state, cell, thumbnail);
        hits.insert(cell.id);
        let _ = maybe_emit_fill_wait(path, request, work, requests, state);
    }
    hits
}

fn publish_ready(state: &Mutex<SharedState>, cell: &PlannedStripCell, thumbnail: StripThumbnail) {
    lock_recover(state)
        .cells
        .insert(cell.id, StripThumbnailOutcome::Ready(thumbnail));
}

fn publish_failure(
    state: &Mutex<SharedState>,
    cell: &PlannedStripCell,
    failure: StripThumbnailFailure,
) {
    let mut shared = lock_recover(state);
    if shared.cells.contains_key(&cell.id) {
        return;
    }
    shared.cells.insert(cell.id, StripThumbnailOutcome::Failed);
    shared.latest_request_failures.push((cell.index, failure));
}

fn all_visible_ready(work: &StripWindowWork, state: &Mutex<SharedState>) -> bool {
    let invalid_visible = work.invalid_cells.iter().any(|index| {
        work.visible_range
            .as_ref()
            .is_some_and(|range| *index >= range.start() && *index <= range.end())
    });
    if work.visible_ids.is_empty() || invalid_visible {
        return false;
    }
    let shared = lock_recover(state);
    work.visible_ids
        .iter()
        .all(|id| matches!(shared.cells.get(id), Some(StripThumbnailOutcome::Ready(_))))
}

fn maybe_emit_fill_wait(
    path: &Path,
    request: &WindowRequest,
    work: &StripWindowWork,
    requests: &Mutex<LatestWindowRequest>,
    state: &Mutex<SharedState>,
) -> bool {
    if lock_recover(requests).supersedes(request.id) {
        return false;
    }
    if !all_visible_ready(work, state) {
        return false;
    }
    {
        let mut shared = lock_recover(state);
        if shared.fill_wait_emitted_request_id == Some(request.id) {
            return true;
        }
        shared.fill_wait_emitted_request_id = Some(request.id);
    }
    if crate::perf::is_enabled() {
        let key = path.display().to_string();
        crate::perf::event(
            "video_strip",
            "fill_wait",
            Some(&key),
            0,
            &[
                ("request_id", serde_json::Value::from(request.id)),
                (
                    "ms",
                    serde_json::Value::from(request.requested_at.elapsed().as_secs_f64() * 1000.0),
                ),
                (
                    "visible_cells",
                    serde_json::Value::from(work.visible_ids.len() as u64),
                ),
            ],
        );
    }
    true
}

fn emit_open_once(
    path: &Path,
    axis: &StripAxis,
    cache_hits: usize,
    decode_path: &str,
    open_ms: f64,
    runtime: &mut WorkerRuntime,
) {
    if runtime.open_event_emitted {
        return;
    }
    runtime.open_event_emitted = true;
    if crate::perf::is_enabled() {
        let key = path.display().to_string();
        let (axis_kind, keyframe_count) = match axis {
            StripAxis::KeyframeIndex { keyframes, .. } => ("keyframe_index", keyframes.len()),
            StripAxis::TimeGrid { .. } => ("time_grid", 0),
        };
        crate::perf::event(
            "video_strip",
            "open",
            Some(&key),
            0,
            &[
                ("axis_kind", serde_json::Value::from(axis_kind)),
                (
                    "keyframe_count",
                    serde_json::Value::from(keyframe_count as u64),
                ),
                ("cache_hits", serde_json::Value::from(cache_hits as u64)),
                ("open_ms", serde_json::Value::from(open_ms)),
                ("decode_path", serde_json::Value::from(decode_path)),
            ],
        );
    }
}

fn emit_decode(
    path: &Path,
    request: &WindowRequest,
    runtime: &WorkerRuntime,
    metrics: &DecodeMetrics,
    superseded: bool,
) {
    if crate::perf::is_enabled() {
        let key = path.display().to_string();
        let decode_path = runtime
            .decoder
            .as_ref()
            .map(SeekStripDecoder::decode_path)
            .unwrap_or("unavailable");
        crate::perf::event(
            "video_strip",
            "decode",
            Some(&key),
            0,
            &[
                ("request_id", serde_json::Value::from(request.id)),
                (
                    "seek_ms",
                    serde_json::Value::from(metrics.seek.as_secs_f64() * 1000.0),
                ),
                ("decode_ms", serde_json::Value::from(metrics.decode_ms())),
                (
                    "transform_ms",
                    serde_json::Value::from(metrics.transform.as_secs_f64() * 1000.0),
                ),
                (
                    "decoded_frames",
                    serde_json::Value::from(metrics.decoded_frames as u64),
                ),
                (
                    "published_cells",
                    serde_json::Value::from(metrics.published_cells as u64),
                ),
                ("runs", serde_json::Value::from(metrics.runs as u64)),
                ("superseded", serde_json::Value::from(superseded)),
                (
                    "discarded_cells",
                    serde_json::Value::from(metrics.discarded_on_supersede as u64),
                ),
                ("decode_path", serde_json::Value::from(decode_path)),
            ],
        );
    }
}

fn cancelled_or_superseded(
    cancel: &AtomicBool,
    requests: &Mutex<LatestWindowRequest>,
    request_id: u64,
) -> bool {
    cancel.load(Ordering::Acquire) || lock_recover(requests).supersedes(request_id)
}

fn flush_deferred_cache_writes(
    path: &Path,
    cache: Option<&Arc<TileThumbCache>>,
    video_mtime: i64,
    requests: &Mutex<LatestWindowRequest>,
    cancel: &AtomicBool,
    pending: &mut BTreeMap<i64, DeferredCacheEntry>,
) {
    let Some(cache) = cache else {
        pending.clear();
        return;
    };
    let keys: Vec<i64> = pending.keys().copied().collect();
    for timestamp_ms in keys {
        if cancel.load(Ordering::Acquire) || lock_recover(requests).pending.is_some() {
            break;
        }
        let Some(entry) = pending.remove(&timestamp_ms) else {
            continue;
        };
        let expected_len = (entry.width as usize)
            .checked_mul(entry.height as usize)
            .and_then(|pixels| pixels.checked_mul(4));
        if expected_len != Some(entry.rgba.len()) {
            crate::logger::log(format!(
                "video-seek-strip-thumbs refusing invalid RGBA cache payload: target={}",
                entry.target_secs
            ));
            continue;
        }
        let encoder = webp::Encoder::from_rgba(entry.rgba.as_slice(), entry.width, entry.height);
        let webp = encoder.encode(70.0);
        if let Err(error) = cache.store_webp(
            path,
            STRIP_THUMB_EXTRACT_WIDTH,
            timestamp_ms,
            video_mtime,
            entry.height,
            webp.as_ref(),
        ) {
            crate::logger::log(format!(
                "video-seek-strip-thumbs cache store failed: {error}"
            ));
        }
    }
}

struct SeekStripDecoder {
    input: ffmpeg::format::context::Input,
    stream_idx: usize,
    tb_num: f64,
    tb_den: f64,
    geometry: DecoderGeometry,
    scaler: Option<ffmpeg::software::scaling::Context>,
    scaler_src_fmt: Option<ffmpeg::format::Pixel>,
    decoder: crate::video::decoder::AuxVideoDecoder,
}

#[derive(Clone, Copy)]
struct DecoderGeometry {
    src_w: u32,
    src_h: u32,
    scaled_w: u32,
    scaled_h: u32,
    dst_w: u32,
    dst_h: u32,
    orientation: crate::video::display_metadata::VideoOrientation,
}

impl SeekStripDecoder {
    fn open(path: &Path, hw_preferred: bool) -> Result<Self, String> {
        use ffmpeg::media::Type as MediaType;

        ffmpeg::init().map_err(|error| format!("ffmpeg init failed: {error}"))?;
        let input =
            ffmpeg::format::input(path).map_err(|error| format!("open input failed: {error}"))?;
        let video_stream = input
            .streams()
            .best(MediaType::Video)
            .ok_or_else(|| "video stream not found".to_string())?;
        let stream_idx = video_stream.index();
        let time_base = video_stream.time_base();
        if time_base.numerator() <= 0 || time_base.denominator() <= 0 {
            return Err("invalid video time base".to_string());
        }
        let tb_num = f64::from(time_base.numerator());
        let tb_den = f64::from(time_base.denominator());
        let orientation = crate::video::display_metadata::orientation_from_stream(&video_stream);
        let params_ref = video_stream.parameters();
        let sar = params_ref.sample_aspect_ratio();
        let (sar_num, sar_den) =
            crate::video::decoder::normalize_sar(sar.numerator(), sar.denominator());
        let params = crate::video::decoder::clone_codec_parameters(&params_ref)?;
        let codec_id = params.id();
        let mut decoder = crate::video::decoder::open_aux_video_decoder_with_fallback(
            &params,
            codec_id,
            hw_preferred,
            "video-seek-strip-thumbs",
        )?;
        decoder.decoder_mut().skip_frame(ffmpeg::Discard::NonKey);
        let src_w = decoder.width();
        let src_h = decoder.height();
        if src_w == 0 || src_h == 0 {
            return Err(format!("invalid decoded dimensions {src_w}x{src_h}"));
        }
        let (dst_w, dst_h) = crate::video::display_metadata::fit_display_within(
            src_w,
            src_h,
            sar_num,
            sar_den,
            orientation,
            STRIP_THUMB_EXTRACT_WIDTH,
            STRIP_THUMB_EXTRACT_HEIGHT,
        );
        let (scaled_w, scaled_h) = if orientation.swaps_axes() {
            (dst_h, dst_w)
        } else {
            (dst_w, dst_h)
        };
        crate::logger::log(format!(
            "video-seek-strip-thumbs decoder ready: codec={} decoder={} decode_path={} d3d11va_supported={} d3d11va_config={} src_size={}x{} scale_size={}x{} display_size={}x{} orientation={orientation:?}",
            codec_id.name(),
            decoder.decoder_name(),
            if decoder.hw_decode_active() {
                "hw_d3d11va"
            } else {
                "sw"
            },
            decoder.d3d11va_supported(),
            decoder.d3d11va_config(),
            src_w,
            src_h,
            scaled_w,
            scaled_h,
            dst_w,
            dst_h,
        ));

        Ok(Self {
            input,
            stream_idx,
            tb_num,
            tb_den,
            geometry: DecoderGeometry {
                src_w,
                src_h,
                scaled_w,
                scaled_h,
                dst_w,
                dst_h,
                orientation,
            },
            scaler: None,
            scaler_src_fmt: None,
            decoder,
        })
    }

    fn hw_decode_active(&self) -> bool {
        self.decoder.hw_decode_active()
    }

    fn decode_path(&self) -> &'static str {
        if self.hw_decode_active() {
            "hw_d3d11va"
        } else {
            "sw"
        }
    }

    fn decode_run(
        &mut self,
        run: &StripDecodeRun,
        request_id: u64,
        requests: &Mutex<LatestWindowRequest>,
        cancel: &AtomicBool,
        publish: &mut impl FnMut(DecodedCell),
    ) -> RunDecodeResult {
        use ffmpeg::util::frame::video::Video;

        let run_t0 = Instant::now();
        let Some(first) = run.cells.first() else {
            return RunDecodeResult::complete();
        };
        let Some(seek_pts) = secs_to_av_time_base(first.target_secs) else {
            return RunDecodeResult::failed(StripThumbnailFailure::SeekFailed(
                "target is outside AV_TIME_BASE range".to_string(),
            ));
        };
        let seek_t0 = Instant::now();
        let seek_result = unsafe {
            ffmpeg::ffi::av_seek_frame(
                self.input.as_mut_ptr(),
                -1,
                seek_pts,
                ffmpeg::ffi::AVSEEK_FLAG_BACKWARD as i32,
            )
        };
        self.decoder.decoder_mut().flush();
        let seek_elapsed = seek_t0.elapsed();
        if seek_result < 0 {
            return RunDecodeResult {
                stop: RunStop::Failed(StripThumbnailFailure::SeekFailed(format!(
                    "av_seek_frame returned {seek_result}"
                ))),
                metrics: RunDecodeMetrics {
                    seek: seek_elapsed,
                    total: run_t0.elapsed(),
                    ..RunDecodeMetrics::default()
                },
                cell_failures: Vec::new(),
            };
        }

        let geometry = self.geometry;
        let stream_idx = self.stream_idx;
        let tb_num = self.tb_num;
        let tb_den = self.tb_den;
        let hw_active = self.hw_decode_active();
        let input = &mut self.input;
        let decoder = &mut self.decoder;
        let scaler = &mut self.scaler;
        let scaler_src_fmt = &mut self.scaler_src_fmt;
        let mut cursor = 0usize;
        let mut last_frame: Option<(Video, f64)> = None;
        let mut first_demux_error: Option<String> = None;
        let mut first_decode_error: Option<String> = None;
        let mut decoded_frames = 0usize;
        let mut published_cells = 0usize;
        let mut transform_elapsed = Duration::ZERO;
        let indexed_presentation_targets = RefCell::new(BTreeMap::new());
        let mut cell_failures = Vec::new();

        let mut publish_frame_for =
            |frame: &Video, cells: &[PlannedStripCell]| -> Result<(), StripThumbnailFailure> {
                if cells.is_empty() {
                    return Ok(());
                }
                let transform_t0 = Instant::now();
                let image = convert_keyframe(frame, geometry, scaler, scaler_src_fmt)
                    .map_err(StripThumbnailFailure::ConvertFailed)?;
                transform_elapsed += transform_t0.elapsed();
                for cell in cells {
                    let decoded = DecodedCell {
                        cell: cell.clone(),
                        thumbnail: StripThumbnail {
                            target_secs: cell.target_secs,
                            width: image.width,
                            height: image.height,
                            rgba: Arc::clone(&image.rgba),
                        },
                    };
                    publish(decoded);
                    published_cells += 1;
                }
                Ok(())
            };

        let mut accept_frame = |frame: Video| -> Result<bool, StripThumbnailFailure> {
            decoded_frames += 1;
            let Some(current_cell) = run.cells.get(cursor) else {
                return Ok(true);
            };
            let Some(pts) = crate::video::decoder::video_frame_timestamp(&frame) else {
                crate::logger::log(format!(
                    "video-seek-strip-thumbs frame PTS missing; using requested time: {}",
                    current_cell.target_secs
                ));
                publish_frame_for(&frame, &run.cells[cursor..cursor + 1])?;
                cursor += 1;
                last_frame = None;
                return Ok(cursor >= run.cells.len());
            };
            let pts_secs = pts as f64 * tb_num / tb_den;
            if !pts_secs.is_finite() || pts_secs < 0.0 {
                return Err(StripThumbnailFailure::DecodeFailed(
                    "decoded keyframe has invalid PTS".to_string(),
                ));
            }

            // TimeGrid の先頭は backward seek で着地した直前キーフレームそのもの。
            // 次のキーフレームを待たず公開し、中心セルの first-frame latency を守る。
            if decoded_frames == 1
                && !current_cell.exact_keyframe
                && pts_secs <= current_cell.target_secs + FRAME_PTS_MATCH_EPSILON_SECS
            {
                if is_preceding_frame_within_tolerance(
                    current_cell.target_secs,
                    pts_secs,
                    current_cell.frame_match_tolerance,
                ) {
                    publish_frame_for(&frame, &run.cells[cursor..cursor + 1])?;
                    cursor += 1;
                    last_frame = Some((frame, pts_secs));
                    return Ok(cursor >= run.cells.len());
                }
                // The landed frame is too old for this TimeGrid cell. Keep decoding so a
                // bounded following frame can satisfy it instead.
                last_frame = Some((frame, pts_secs));
                return Ok(false);
            }

            let matches = match_cells_before_frame(
                &run.cells,
                cursor,
                last_frame.as_ref().map(|(_, pts_secs)| *pts_secs),
                pts_secs,
                &indexed_presentation_targets.borrow(),
            );
            let preceding_cells: Vec<_> = matches
                .preceding_indices
                .iter()
                .filter_map(|index| run.cells.get(*index).cloned())
                .collect();
            if let Some((previous, _)) = last_frame.as_ref() {
                publish_frame_for(previous, &preceding_cells)?;
            }
            let following_cells: Vec<_> = matches
                .following_indices
                .iter()
                .filter_map(|index| run.cells.get(*index).cloned())
                .collect();
            publish_frame_for(&frame, &following_cells)?;
            for index in matches.failed_indices {
                if let Some(cell) = run.cells.get(index) {
                    cell_failures.push((cell.clone(), StripThumbnailFailure::NoFrame));
                }
            }
            cursor = matches.next_cursor;

            let exact_start = cursor;
            while let Some(cell) = run.cells.get(cursor) {
                let target_secs =
                    cell_presentation_target_secs(cell, &indexed_presentation_targets.borrow());
                if (target_secs - pts_secs).abs() > FRAME_PTS_MATCH_EPSILON_SECS {
                    break;
                }
                cursor += 1;
            }
            publish_frame_for(&frame, &run.cells[exact_start..cursor])?;
            last_frame = Some((frame, pts_secs));
            Ok(cursor >= run.cells.len())
        };

        let mut stop = RunStop::Complete;
        let mut run_completed = false;
        'packets: for item in input.packets() {
            if cancel.load(Ordering::Acquire) {
                stop = RunStop::Cancelled;
                break;
            }
            if lock_recover(requests).supersedes(request_id) {
                stop = RunStop::Superseded;
                break;
            }
            let (stream, packet) = match item {
                Ok(value) => value,
                Err(error) => {
                    first_demux_error.get_or_insert_with(|| error.to_string());
                    break;
                }
            };
            if stream.index() != stream_idx {
                continue;
            }
            if packet.is_key()
                && let Some(packet_dts) = packet.dts().or_else(|| packet.pts())
                && let Some(packet_pts) = packet.pts().or_else(|| packet.dts())
            {
                let packet_dts_secs = packet_dts as f64 * tb_num / tb_den;
                let packet_pts_secs = packet_pts as f64 * tb_num / tb_den;
                let mut targets = indexed_presentation_targets.borrow_mut();
                for cell in run.cells.iter().filter(|cell| cell.exact_keyframe) {
                    if targets.contains_key(&cell.id) {
                        continue;
                    }
                    if let Some(target_secs) = indexed_packet_presentation_target_secs(
                        cell,
                        packet_dts_secs,
                        packet_pts_secs,
                    ) {
                        targets.insert(cell.id, target_secs);
                    }
                }
            }
            if let Err(error) = decoder.decoder_mut().send_packet(&packet) {
                if hw_active {
                    stop = RunStop::Failed(StripThumbnailFailure::DecodeFailed(format!(
                        "HW send_packet failed: {error}"
                    )));
                    break;
                }
                first_decode_error.get_or_insert_with(|| error.to_string());
                continue;
            }
            loop {
                let mut frame = Video::empty();
                if decoder.decoder_mut().receive_frame(&mut frame).is_err() {
                    break;
                }
                match accept_frame(frame) {
                    Ok(true) => {
                        run_completed = true;
                        break 'packets;
                    }
                    Ok(false) => {}
                    Err(failure) => {
                        stop = RunStop::Failed(failure);
                        break 'packets;
                    }
                }
            }
        }

        if matches!(stop, RunStop::Complete) && !run_completed {
            if let Err(error) = decoder.decoder_mut().send_eof() {
                first_decode_error.get_or_insert_with(|| error.to_string());
            } else {
                loop {
                    let mut frame = Video::empty();
                    if decoder.decoder_mut().receive_frame(&mut frame).is_err() {
                        break;
                    }
                    match accept_frame(frame) {
                        Ok(true) => break,
                        Ok(false) => {}
                        Err(failure) => {
                            stop = RunStop::Failed(failure);
                            break;
                        }
                    }
                }
            }
        }

        drop(accept_frame);
        if matches!(stop, RunStop::Complete) && cursor < run.cells.len() {
            if let Some((frame, pts_secs)) = last_frame.as_ref() {
                let mut ready_cells = Vec::new();
                while let Some(cell) = run.cells.get(cursor) {
                    let target_secs =
                        cell_presentation_target_secs(cell, &indexed_presentation_targets.borrow());
                    if is_preceding_frame_within_tolerance(
                        target_secs,
                        *pts_secs,
                        cell.frame_match_tolerance,
                    ) {
                        ready_cells.push(cell.clone());
                    } else {
                        cell_failures.push((cell.clone(), StripThumbnailFailure::NoFrame));
                    }
                    cursor += 1;
                }
                if let Err(failure) = publish_frame_for(frame, &ready_cells) {
                    stop = RunStop::Failed(failure);
                }
            }
        }

        if matches!(stop, RunStop::Complete) && cursor < run.cells.len() {
            stop = if let Some(error) = first_demux_error {
                RunStop::Failed(StripThumbnailFailure::DemuxFailed(error))
            } else if let Some(error) = first_decode_error {
                RunStop::Failed(StripThumbnailFailure::DecodeFailed(error))
            } else {
                RunStop::Failed(StripThumbnailFailure::NoFrame)
            };
        }

        drop(publish_frame_for);
        RunDecodeResult {
            stop,
            metrics: RunDecodeMetrics {
                seek: seek_elapsed,
                transform: transform_elapsed,
                total: run_t0.elapsed(),
                decoded_frames,
                published_cells,
            },
            cell_failures,
        }
    }
}

fn secs_to_av_time_base(secs: f64) -> Option<i64> {
    if !secs.is_finite() {
        return None;
    }
    let value = secs * ffmpeg::ffi::AV_TIME_BASE as f64;
    (value.is_finite() && value >= i64::MIN as f64 && value <= i64::MAX as f64)
        .then(|| value.round() as i64)
}

#[derive(Default)]
struct RunDecodeMetrics {
    seek: Duration,
    transform: Duration,
    total: Duration,
    decoded_frames: usize,
    published_cells: usize,
}

struct RunDecodeResult {
    stop: RunStop,
    metrics: RunDecodeMetrics,
    cell_failures: Vec<(PlannedStripCell, StripThumbnailFailure)>,
}

impl RunDecodeResult {
    fn complete() -> Self {
        Self {
            stop: RunStop::Complete,
            metrics: RunDecodeMetrics::default(),
            cell_failures: Vec::new(),
        }
    }

    fn failed(failure: StripThumbnailFailure) -> Self {
        Self {
            stop: RunStop::Failed(failure),
            metrics: RunDecodeMetrics::default(),
            cell_failures: Vec::new(),
        }
    }
}

enum RunStop {
    Complete,
    Superseded,
    Cancelled,
    Failed(StripThumbnailFailure),
}

struct DecodedImage {
    width: u32,
    height: u32,
    rgba: Arc<Vec<u8>>,
}

struct DecodedCell {
    cell: PlannedStripCell,
    thumbnail: StripThumbnail,
}

fn convert_keyframe(
    frame: &ffmpeg::util::frame::video::Video,
    geometry: DecoderGeometry,
    scaler: &mut Option<ffmpeg::software::scaling::Context>,
    scaler_src_fmt: &mut Option<ffmpeg::format::Pixel>,
) -> Result<DecodedImage, String> {
    use ffmpeg::format::Pixel;
    use ffmpeg::software::scaling::{Context as ScaleContext, Flags as ScaleFlags};
    use ffmpeg::util::frame::video::Video;

    let mut sw_holder: Option<Video> = None;
    let frame_for_scaler =
        crate::video::swscale_helpers::prepare_frame_for_swscale(frame, &mut sw_holder)
            .map_err(|error| error.to_string())?;
    let current_format = frame_for_scaler.format();
    if scaler.is_none() || *scaler_src_fmt != Some(current_format) {
        *scaler = Some(
            ScaleContext::get(
                current_format,
                geometry.src_w,
                geometry.src_h,
                Pixel::RGBA,
                geometry.scaled_w,
                geometry.scaled_h,
                ScaleFlags::BILINEAR,
            )
            .map_err(|error| format!("sws_scale init failed: {error}"))?,
        );
        *scaler_src_fmt = Some(current_format);
    }
    let scaler = scaler
        .as_mut()
        .ok_or_else(|| "scaler was not initialized".to_string())?;
    let mut rgba = Video::empty();
    scaler
        .run(frame_for_scaler, &mut rgba)
        .map_err(|error| format!("sws_scale failed: {error}"))?;

    let row_bytes = (geometry.scaled_w as usize)
        .checked_mul(4)
        .ok_or_else(|| "RGBA row size overflow".to_string())?;
    let height = geometry.scaled_h as usize;
    let output_len = row_bytes
        .checked_mul(height)
        .ok_or_else(|| "RGBA output size overflow".to_string())?;
    let stride = rgba.stride(0);
    if stride < row_bytes {
        return Err(format!(
            "RGBA stride {stride} is smaller than row size {row_bytes}"
        ));
    }
    let plane = rgba.data(0);
    let mut packed = Vec::with_capacity(output_len);
    for row in 0..height {
        let start = row
            .checked_mul(stride)
            .ok_or_else(|| "RGBA plane offset overflow".to_string())?;
        let end = start
            .checked_add(row_bytes)
            .ok_or_else(|| "RGBA plane row overflow".to_string())?;
        let bytes = plane
            .get(start..end)
            .ok_or_else(|| "RGBA plane is shorter than its stride metadata".to_string())?;
        packed.extend_from_slice(bytes);
    }

    let (width, height, oriented) = crate::video::display_metadata::orient_rgba(
        geometry.scaled_w,
        geometry.scaled_h,
        &packed,
        geometry.orientation,
    )?;
    if (width, height) != (geometry.dst_w, geometry.dst_h) {
        return Err(format!(
            "oriented dimensions {width}x{height} do not match expected {}x{}",
            geometry.dst_w, geometry.dst_h
        ));
    }
    Ok(DecodedImage {
        width,
        height,
        rgba: Arc::new(oriented),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis(count: usize) -> StripAxis {
        StripAxis::KeyframeIndex {
            keyframes: (0..count).map(|index| index as f64).collect(),
            adopted: (0..count).collect(),
        }
    }

    fn plan(
        axis: &StripAxis,
        spec: StripWindowSpec,
        settled: &BTreeSet<StripCellId>,
    ) -> StripWindowWork {
        plan_strip_window_work(axis, spec, settled)
    }

    fn id(secs: f64) -> StripCellId {
        StripCellId::from_secs(secs).expect("test timestamp must be valid")
    }

    fn thumbnail(target_secs: f64) -> StripThumbnail {
        StripThumbnail {
            target_secs,
            width: 1,
            height: 1,
            rgba: Arc::new(vec![0, 0, 0, 255]),
        }
    }

    #[test]
    fn cell_display_state_distinguishes_pending_ready_and_failed() {
        assert!(matches!(
            decide_strip_thumbnail_cell_state(None, None),
            StripThumbnailCellState::Pending
        ));

        let ready = StripThumbnailOutcome::Ready(thumbnail(3.0));
        assert!(matches!(
            decide_strip_thumbnail_cell_state(Some(&ready), None),
            StripThumbnailCellState::Ready(thumbnail) if thumbnail.target_secs == 3.0
        ));

        let failed = StripThumbnailOutcome::Failed;
        assert!(matches!(
            decide_strip_thumbnail_cell_state(Some(&failed), None),
            StripThumbnailCellState::Failed
        ));
        assert!(matches!(
            decide_strip_thumbnail_cell_state(None, Some(&StripThumbnailFailure::InvalidCellTime)),
            StripThumbnailCellState::Failed
        ));
    }

    #[test]
    fn only_decoder_unavailable_after_axis_resolution_replaces_the_cell_row() {
        assert_eq!(
            decide_strip_thumbnail_display_scope(
                true,
                &StripThumbnailWorkerStatus::DecoderUnavailable("open failed".into()),
            ),
            StripThumbnailDisplayScope::StripUnavailable
        );
        assert_eq!(
            decide_strip_thumbnail_display_scope(
                false,
                &StripThumbnailWorkerStatus::DecoderUnavailable("axis failed".into()),
            ),
            StripThumbnailDisplayScope::Cells
        );
        for status in [
            StripThumbnailWorkerStatus::Running,
            StripThumbnailWorkerStatus::ThreadSpawnFailed("spawn failed".into()),
            StripThumbnailWorkerStatus::Cancelled,
        ] {
            assert_eq!(
                decide_strip_thumbnail_display_scope(true, &status),
                StripThumbnailDisplayScope::Cells
            );
        }

        for resolved_axis in [
            Arc::new(axis(3)),
            Arc::new(StripAxis::TimeGrid {
                interval_secs: 2.0,
                duration_secs: 30.0,
            }),
        ] {
            let snapshot = StripThumbnailSnapshot {
                axis: StripAxisResolution::Ready(resolved_axis),
                cells: BTreeMap::new(),
                status: StripThumbnailWorkerStatus::DecoderUnavailable("open failed".into()),
                latest_request_failures: Vec::new(),
                decode_diagnostics: StripThumbnailDecodeDiagnostics::default(),
            };
            assert_eq!(
                snapshot.display_scope(),
                StripThumbnailDisplayScope::StripUnavailable
            );
        }
    }

    #[test]
    fn settled_failure_keeps_state_and_current_request_reason_in_one_place_each() {
        let axis = axis(3);
        let work = plan(
            &axis,
            StripWindowSpec::new(1.0, 3, StripLookahead::default()),
            &BTreeSet::new(),
        );
        let cell = work
            .cache_lookup
            .iter()
            .find(|cell| cell.index == 1)
            .expect("center cell must be planned");
        let state = Mutex::new(SharedState::new());

        publish_failure(&state, cell, StripThumbnailFailure::NoFrame);

        let snapshot = lock_recover(&state).snapshot();
        assert!(matches!(
            snapshot.outcome_for_secs(cell.target_secs),
            Some(StripThumbnailOutcome::Failed)
        ));
        assert_eq!(
            snapshot.latest_failure_for_index(cell.index),
            Some(&StripThumbnailFailure::NoFrame)
        );
    }

    #[test]
    #[ignore = "manual real-media strip-thumbnail worker diagnosis"]
    fn probe_app_thumbnail_worker_window_from_env() {
        let path = std::env::var_os("MIV_STRIP_THUMB_PROBE_PATH")
            .map(PathBuf::from)
            .expect("set MIV_STRIP_THUMB_PROBE_PATH to a real video");
        ffmpeg::init().expect("FFmpeg must initialize");
        let input = ffmpeg::format::input(&path).expect("probe video must open");
        let duration_secs = input.duration() as f64 / ffmpeg::ffi::AV_TIME_BASE as f64;
        assert!(
            duration_secs.is_finite() && duration_secs > 0.0,
            "probe video must have a positive duration"
        );
        let center_time_secs = std::env::var("MIV_STRIP_THUMB_PROBE_CENTER_SECS")
            .ok()
            .map(|value| value.parse().expect("probe center must be seconds"))
            .unwrap_or(duration_secs * 0.5);
        let visible_count = std::env::var("MIV_STRIP_THUMB_PROBE_VISIBLE_COUNT")
            .ok()
            .map(|value| value.parse().expect("visible count must be an integer"))
            .unwrap_or(9usize);
        let hw_decode = std::env::var("MIV_STRIP_THUMB_PROBE_HW")
            .ok()
            .map(|value| value != "0")
            .unwrap_or(true);
        let lookahead = StripLookahead::new(visible_count / 2, visible_count - visible_count / 2);
        let worker = SeekStripThumbnailWorker::spawn(
            path.clone(),
            hw_decode,
            None,
            duration_secs,
            2.0,
            10.0,
        );

        let started = Instant::now();
        let axis = loop {
            match worker.snapshot().axis {
                StripAxisResolution::Ready(axis) => break axis,
                StripAxisResolution::Failed(error) => panic!("axis resolution failed: {error}"),
                StripAxisResolution::Resolving => {}
            }
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "axis resolution timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        let center_index = axis
            .center_index_for_time(center_time_secs)
            .expect("probe center must map onto the strip axis");
        let spec = StripWindowSpec::new(center_index, visible_count, lookahead);
        let work = plan_strip_window_work(&axis, spec, &BTreeSet::new());
        worker
            .request(Arc::clone(&axis), center_index, visible_count, lookahead)
            .expect("worker request must be accepted");

        let snapshot = loop {
            let snapshot = worker.snapshot();
            let all_settled = work
                .cache_lookup
                .iter()
                .all(|cell| snapshot.cells.contains_key(&cell.id));
            if all_settled
                || matches!(
                    snapshot.status,
                    StripThumbnailWorkerStatus::DecoderUnavailable(_)
                        | StripThumbnailWorkerStatus::Cancelled
                        | StripThumbnailWorkerStatus::ThreadSpawnFailed(_)
                )
            {
                break snapshot;
            }
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "thumbnail window timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        };

        println!(
            "path={} duration_secs={duration_secs:.3} center_time_secs={center_time_secs:.3} center_index={center_index:.3} axis_cells={} first_axis_cell={:?} invalid_cells={:?} status={:?}",
            path.display(),
            axis.cell_count(),
            axis.cell(0),
            work.invalid_cells,
            snapshot.status,
        );
        println!(
            "decode initial={:?} current={:?} software_retry_failure={:?}",
            snapshot.decode_diagnostics.initial_path,
            snapshot.decode_diagnostics.current_path,
            snapshot.decode_diagnostics.software_retry_failure,
        );
        let mut cells = work.cache_lookup.clone();
        cells.sort_by_key(|cell| cell.index);
        for cell in cells {
            let outcome = match snapshot.cells.get(&cell.id) {
                Some(StripThumbnailOutcome::Ready(_)) => "ready".to_string(),
                Some(StripThumbnailOutcome::Failed) => {
                    format!("failed {:?}", snapshot.latest_failure_for_index(cell.index))
                }
                None => "pending".to_string(),
            };
            println!(
                "cell index={} target_secs={:.6} visible={} outcome={outcome}",
                cell.index, cell.target_secs, cell.visible,
            );
        }
    }

    #[test]
    fn window_plan_looks_up_only_missing_cells_center_outward() {
        let axis = axis(20);
        let held = BTreeSet::from([id(5.0), id(7.0)]);
        let work = plan(
            &axis,
            StripWindowSpec::new(5.0, 5, StripLookahead::new(1, 2)),
            &held,
        );
        let indices: Vec<usize> = work.cache_lookup.iter().map(|cell| cell.index).collect();
        assert_eq!(indices, vec![4, 6, 3, 2, 8, 1, 9]);
        assert!(work.cache_lookup.iter().all(|cell| cell.index != 5));
        assert!(work.cache_lookup.iter().all(|cell| cell.index != 7));
    }

    #[test]
    fn indexed_packet_maps_dts_cell_to_b_frame_presentation_time_within_local_tolerance() {
        let axis = StripAxis::KeyframeIndex {
            keyframes: vec![10.0, 12.0, 20.0],
            adopted: vec![0, 1, 2],
        };
        let work = plan(
            &axis,
            StripWindowSpec::new(1.0, 1, StripLookahead::default()),
            &BTreeSet::new(),
        );
        let cell = work
            .cache_lookup
            .iter()
            .find(|cell| cell.index == 1)
            .expect("middle indexed cell must be planned");
        assert_eq!(
            cell.frame_match_tolerance,
            FrameMatchTolerance {
                before_secs: 2.0,
                after_secs: 8.0,
            }
        );
        assert_eq!(
            indexed_packet_presentation_target_secs(cell, 12.0, 12.1),
            Some(12.1)
        );
        assert_eq!(
            indexed_packet_presentation_target_secs(cell, 12.0, 20.006),
            None,
            "a presentation timestamp beyond the following raw entry must be rejected"
        );
        assert_eq!(
            indexed_packet_presentation_target_secs(cell, 12.006, 12.1),
            None,
            "an unrelated key packet must not be associated with the cell"
        );

        let tail = plan(
            &axis,
            StripWindowSpec::new(2.0, 1, StripLookahead::default()),
            &BTreeSet::new(),
        );
        let tail_cell = tail
            .cache_lookup
            .iter()
            .find(|cell| cell.index == 2)
            .expect("tail indexed cell must be planned");
        assert_eq!(
            indexed_packet_presentation_target_secs(tail_cell, 20.0, 20.1),
            Some(20.1),
            "an endpoint uses its only adjacent raw gap in both directions"
        );
    }

    #[test]
    fn leading_negative_dts_cell_is_planned_and_maps_to_its_first_presentation_pts() {
        let first_dts_secs = -1001.0 / 24_000.0;
        let axis = StripAxis::KeyframeIndex {
            keyframes: vec![first_dts_secs, 7.632625],
            adopted: vec![0, 1],
        };
        let work = plan(
            &axis,
            StripWindowSpec::new(0.0, 1, StripLookahead::default()),
            &BTreeSet::new(),
        );
        assert!(work.invalid_cells.is_empty());
        let cell = work
            .cache_lookup
            .iter()
            .find(|cell| cell.index == 0)
            .expect("signed leading index cell must be planned");
        assert_eq!(cell.target_secs, first_dts_secs);
        assert_eq!(cell.id.target_secs(), first_dts_secs);
        assert_eq!(cell.timestamp_ms, -42);
        assert_eq!(secs_to_av_time_base(first_dts_secs), Some(-41_708));
        assert_eq!(
            indexed_packet_presentation_target_secs(cell, first_dts_secs, 0.0),
            Some(0.0),
            "the first B-frame key packet must map its signed DTS cell to presentation time"
        );
    }

    #[test]
    fn first_cell_without_a_preceding_frame_falls_forward_within_its_local_gap() {
        let axis = StripAxis::KeyframeIndex {
            keyframes: vec![0.0, 2.0, 4.0],
            adopted: vec![0, 1, 2],
        };
        let work = plan(
            &axis,
            StripWindowSpec::new(0.0, 1, StripLookahead::default()),
            &BTreeSet::new(),
        );
        let cell = work
            .cache_lookup
            .iter()
            .find(|cell| cell.index == 0)
            .expect("cell 0 must be planned");
        let cells = vec![cell.clone()];

        let following = match_cells_before_frame(&cells, 0, None, 0.125, &BTreeMap::new());
        assert_eq!(following.preceding_indices, Vec::<usize>::new());
        assert_eq!(following.following_indices, vec![0]);
        assert!(following.failed_indices.is_empty());
        assert_eq!(following.next_cursor, 1);

        let unrelated = match_cells_before_frame(&cells, 0, None, 2.006, &BTreeMap::new());
        assert!(unrelated.preceding_indices.is_empty());
        assert!(unrelated.following_indices.is_empty());
        assert_eq!(unrelated.failed_indices, vec![0]);
    }

    #[test]
    fn frame_matching_prefers_bounded_preceding_and_does_not_strand_a_later_cell() {
        let tolerance = FrameMatchTolerance::symmetric(1.0);
        assert!(is_preceding_frame_within_tolerance(10.0, 9.0, tolerance));
        assert!(!is_preceding_frame_within_tolerance(10.0, 8.994, tolerance));
        assert!(!is_preceding_frame_within_tolerance(
            10.0, 10.006, tolerance
        ));
        assert!(is_following_frame_within_tolerance(10.0, 11.0, tolerance));
        assert!(!is_following_frame_within_tolerance(
            10.0, 11.006, tolerance
        ));

        let axis = StripAxis::TimeGrid {
            interval_secs: 2.0,
            duration_secs: 8.0,
        };
        let work = plan(
            &axis,
            StripWindowSpec::new(1.0, 3, StripLookahead::default()),
            &BTreeSet::new(),
        );
        let mut cells = work.cache_lookup;
        cells.sort_by_key(|cell| cell.index);
        cells[0].frame_match_tolerance = FrameMatchTolerance::symmetric(1.0);
        let matches = match_cells_before_frame(&cells, 0, Some(-3.0), 2.0, &BTreeMap::new());
        assert!(matches.preceding_indices.is_empty());
        assert!(matches.following_indices.is_empty());
        assert_eq!(matches.failed_indices, vec![0]);
        assert_eq!(
            matches.next_cursor, 1,
            "the next cell remains available for the current 2s frame"
        );

        let preceding = match_cells_before_frame(&cells, 0, Some(-0.5), 0.5, &BTreeMap::new());
        assert_eq!(preceding.preceding_indices, vec![0]);
        assert!(preceding.following_indices.is_empty());
    }

    #[test]
    fn last_cell_uses_its_previous_raw_gap_when_there_is_no_following_keyframe() {
        let axis = StripAxis::KeyframeIndex {
            keyframes: vec![10.0, 12.0, 20.0],
            adopted: vec![0, 1, 2],
        };
        let work = plan(
            &axis,
            StripWindowSpec::new(2.0, 1, StripLookahead::default()),
            &BTreeSet::new(),
        );
        let tail = work
            .cache_lookup
            .iter()
            .find(|cell| cell.index == 2)
            .expect("last cell must be planned");
        assert_eq!(
            tail.frame_match_tolerance,
            FrameMatchTolerance {
                before_secs: 8.0,
                after_secs: 8.0,
            }
        );
        assert!(is_preceding_frame_within_tolerance(
            20.0,
            12.0,
            tail.frame_match_tolerance
        ));
        assert!(!is_preceding_frame_within_tolerance(
            20.0,
            11.994,
            tail.frame_match_tolerance
        ));
        assert!(is_following_frame_within_tolerance(
            20.0,
            28.0,
            tail.frame_match_tolerance
        ));
        assert!(!is_following_frame_within_tolerance(
            20.0,
            28.006,
            tail.frame_match_tolerance
        ));
    }

    #[test]
    fn cache_hits_are_removed_without_changing_decode_priority() {
        let axis = axis(12);
        let work = plan(
            &axis,
            StripWindowSpec::new(5.0, 5, StripLookahead::new(0, 2)),
            &BTreeSet::new(),
        );
        let decode =
            plan_strip_decode_cells(&work.cache_lookup, &BTreeSet::from([id(5.0), id(4.0)]));
        let indices: Vec<usize> = decode.iter().map(|cell| cell.index).collect();
        assert_eq!(indices, vec![6, 3, 7, 2, 8, 9]);
    }

    #[test]
    fn decoded_window_round_trips_through_cache_with_requested_timestamp_keys() {
        let _data_dir = crate::data_dir::TestDataDirGuard::new();
        let cache = Arc::new(TileThumbCache::open().expect("temporary cache must open"));
        let video_path = Path::new("c:/edit-list.mp4");
        let video_mtime = 42;
        let axis = axis(12);
        let spec = StripWindowSpec::new(5.0, 5, StripLookahead::new(1, 2));
        let first = plan(&axis, spec, &BTreeSet::new());
        let mut pending = BTreeMap::new();

        for cell in &first.cache_lookup {
            let decoded = DecodedCell {
                cell: cell.clone(),
                thumbnail: StripThumbnail {
                    target_secs: cell.target_secs,
                    width: 1,
                    height: 1,
                    rgba: Arc::new(vec![0, 0, 0, 255]),
                },
            };
            queue_deferred_cache_write(&mut pending, &decoded);
        }

        flush_deferred_cache_writes(
            video_path,
            Some(&cache),
            video_mtime,
            &Mutex::new(LatestWindowRequest::default()),
            &AtomicBool::new(false),
            &mut pending,
        );
        assert!(pending.is_empty());

        let second = plan(&axis, spec, &BTreeSet::new());
        let timestamps: Vec<i64> = second
            .cache_lookup
            .iter()
            .map(|cell| cell.timestamp_ms)
            .collect();
        let rows = cache.lookup_webp_batch(
            video_path,
            &timestamps,
            video_mtime,
            STRIP_THUMB_EXTRACT_WIDTH,
        );
        assert!(rows.iter().all(Option::is_some));

        let cache_hits: BTreeSet<StripCellId> = second
            .cache_lookup
            .iter()
            .zip(rows)
            .filter_map(|(cell, row)| row.map(|_| cell.id))
            .collect();
        assert!(plan_strip_decode_cells(&second.cache_lookup, &cache_hits).is_empty());
    }

    #[test]
    fn decode_runs_publish_center_then_visible_then_lookahead() {
        let axis = axis(14);
        let work = plan(
            &axis,
            StripWindowSpec::new(6.0, 5, StripLookahead::new(2, 2)),
            &BTreeSet::new(),
        );
        let runs = group_strip_decode_runs(&work.cache_lookup, 6.0);
        let shapes: Vec<Vec<usize>> = runs
            .iter()
            .map(|run| run.cells.iter().map(|cell| cell.index).collect())
            .collect();
        assert_eq!(
            shapes,
            vec![vec![6], vec![7, 8], vec![3, 4, 5], vec![9, 10], vec![1, 2],]
        );
    }

    #[test]
    fn cache_holes_form_maximal_contiguous_runs_not_per_cell_seeks() {
        let axis = axis(14);
        let work = plan(
            &axis,
            StripWindowSpec::new(6.0, 7, StripLookahead::default()),
            &BTreeSet::new(),
        );
        let decode =
            plan_strip_decode_cells(&work.cache_lookup, &BTreeSet::from([id(4.0), id(8.0)]));
        let runs = group_strip_decode_runs(&decode, 6.0);
        let shapes: Vec<Vec<usize>> = runs
            .iter()
            .map(|run| run.cells.iter().map(|cell| cell.index).collect())
            .collect();
        assert_eq!(shapes, vec![vec![6], vec![7], vec![5], vec![9], vec![2, 3]]);
    }

    #[test]
    fn supersede_keeps_settled_cells_and_discards_only_unfinished_work() {
        let axis = axis(20);
        let first = plan(
            &axis,
            StripWindowSpec::new(5.0, 5, StripLookahead::new(0, 2)),
            &BTreeSet::new(),
        );
        let kept = BTreeSet::from([id(5.0), id(6.0)]);
        let decision = decide_strip_supersede(&kept, &first.cache_lookup);
        assert_eq!(decision.kept, kept);
        assert!(!decision.discarded.contains(&id(5.0)));
        assert!(!decision.discarded.contains(&id(6.0)));

        let next = plan(
            &axis,
            StripWindowSpec::new(12.0, 5, StripLookahead::new(2, 0)),
            &decision.kept,
        );
        assert_eq!(decision.kept.len(), 2);
        assert!(
            next.cache_lookup
                .iter()
                .all(|cell| !decision.kept.contains(&cell.id))
        );
    }

    #[test]
    fn invalid_external_axis_cell_is_reported_by_the_plan() {
        let axis = StripAxis::KeyframeIndex {
            keyframes: vec![0.0, f64::NAN, 2.0],
            adopted: vec![0, 1, 2],
        };
        let work = plan(
            &axis,
            StripWindowSpec::new(1.0, 3, StripLookahead::default()),
            &BTreeSet::new(),
        );
        assert_eq!(work.invalid_cells, vec![1]);
        assert_eq!(
            work.cache_lookup
                .iter()
                .map(|cell| cell.index)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );
    }

    #[test]
    fn latest_pending_request_replaces_the_previous_one() {
        let axis = Arc::new(axis(4));
        let mut pending = LatestWindowRequest::default();
        let make = |id| WindowRequest {
            id,
            axis: Arc::clone(&axis),
            spec: StripWindowSpec::new(id as f64, 3, StripLookahead::default()),
            requested_at: Instant::now(),
        };
        assert!(pending.replace(make(1)).is_none());
        assert_eq!(pending.replace(make(2)).map(|request| request.id), Some(1));
        assert_eq!(pending.replace(make(3)).map(|request| request.id), Some(2));
        assert_eq!(pending.take().map(|request| request.id), Some(3));
    }
}
