//! Batch verification facade for the production seek-strip thumbnail worker.
//!
//! This module intentionally contains no thumbnail decoder or axis implementation. It drives
//! SeekStripThumbnailWorker, uses the resolved StripAxis, and turns worker snapshots into
//! stable text/JSON-friendly reports.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ffmpeg_the_third as ffmpeg;
use serde::Serialize;

use super::seek_strip::{StripAxis, StripLookahead, compute_strip_window};
use super::seek_strip_thumbs::{
    SeekStripThumbnailWorker, StripAxisDiagnostics, StripAxisResolution, StripAxisResolutionReason,
    StripThumbnailDecodeDiagnostics, StripThumbnailDecodePath, StripThumbnailFailure,
    StripThumbnailOutcome, StripThumbnailWorkerStatus,
};

const FALLBACK_MAX_CELLS: usize = 240;

#[derive(Clone, Debug, Serialize)]
pub struct BatchOptions {
    pub visible_count: usize,
    pub hardware_decode: bool,
    pub minimum_gap_secs: f64,
    pub axis_timeout_secs: u64,
    pub window_timeout_secs: u64,
}

impl Default for BatchOptions {
    fn default() -> Self {
        Self {
            visible_count: 11,
            hardware_decode: true,
            minimum_gap_secs: crate::settings::VIDEO_SEEK_STRIP_MIN_INTERVAL_DEFAULT_SECS,
            axis_timeout_secs: 15,
            window_timeout_secs: 30,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DiscoveryIssue {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiscoveryReport {
    pub files: Vec<PathBuf>,
    pub issues: Vec<DiscoveryIssue>,
    pub limit_reached: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpacingStats {
    pub minimum_secs: f64,
    pub p50_secs: f64,
    pub p90_secs: f64,
    pub maximum_secs: f64,
    pub mean_secs: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct AxisReport {
    pub kind: String,
    pub reason_code: String,
    pub reason: String,
    pub cell_count: usize,
    pub interval_secs: Option<f64>,
    pub keyframe_count: usize,
    pub index_first_secs: Option<f64>,
    pub index_last_secs: Option<f64>,
    pub index_coverage_percent: Option<f64>,
    pub index_timestamps_monotonic: Option<bool>,
    pub index_inversion_count: usize,
    pub adopted_count: Option<usize>,
    pub adopted_spacing: Option<SpacingStats>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CellState {
    Ready,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct CellReport {
    pub index: usize,
    pub time_secs: f64,
    pub state: CellState,
    pub failure: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WindowReport {
    pub name: String,
    pub requested_start_index: usize,
    pub actual_start_index: usize,
    pub actual_end_index: usize,
    pub ready_count: usize,
    pub failed_count: usize,
    pub elapsed_ms: f64,
    pub cells: Vec<CellReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DecodeReport {
    pub initial_path: Option<String>,
    pub final_path: Option<String>,
    pub software_retry_failure: Option<String>,
    pub full_frame_fallback_trigger: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileReport {
    pub path: String,
    pub duration_secs: Option<f64>,
    pub elapsed_ms: f64,
    pub axis: Option<AxisReport>,
    pub decode: Option<DecodeReport>,
    pub windows: Vec<WindowReport>,
    pub skipped_reason: Option<String>,
    pub file_error: Option<String>,
}

impl FileReport {
    pub fn failed_cell_count(&self) -> usize {
        self.windows.iter().map(|window| window.failed_count).sum()
    }

    pub fn passed(&self) -> bool {
        self.skipped_reason.is_none() && self.file_error.is_none() && self.failed_cell_count() == 0
    }

    pub fn skipped(&self) -> bool {
        self.skipped_reason.is_some()
    }

    pub fn verification_failed(&self) -> bool {
        self.file_error.is_some() || self.failed_cell_count() > 0
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BatchReport {
    pub schema_version: u32,
    pub roots: Vec<String>,
    pub options: BatchOptions,
    pub discovery_issues: Vec<DiscoveryIssue>,
    pub limit_reached: bool,
    pub files: Vec<FileReport>,
    pub passed_files: usize,
    pub failed_files: usize,
    pub skipped_files: usize,
    pub failed_cells: usize,
    pub elapsed_ms: f64,
}

impl BatchReport {
    pub fn from_files(
        roots: &[PathBuf],
        options: BatchOptions,
        discovery: DiscoveryReport,
        files: Vec<FileReport>,
        elapsed: Duration,
    ) -> Self {
        let passed_files = files.iter().filter(|file| file.passed()).count();
        let failed_files = files
            .iter()
            .filter(|file| file.verification_failed())
            .count();
        let skipped_files = files.iter().filter(|file| file.skipped()).count();
        let failed_cells = files.iter().map(FileReport::failed_cell_count).sum();
        Self {
            schema_version: 1,
            roots: roots.iter().map(|path| path_string(path)).collect(),
            options,
            discovery_issues: discovery.issues,
            limit_reached: discovery.limit_reached,
            files,
            passed_files,
            failed_files,
            skipped_files,
            failed_cells,
            elapsed_ms: elapsed.as_secs_f64() * 1000.0,
        }
    }

    pub fn has_failures(&self) -> bool {
        self.failed_files > 0 || !self.discovery_issues.is_empty()
    }
}

pub fn discover_video_files(roots: &[PathBuf], limit: Option<usize>) -> DiscoveryReport {
    let mut files = Vec::new();
    let mut issues = Vec::new();
    let mut seen = HashSet::new();
    let mut limit_reached = false;

    for root in roots {
        if limit.is_some_and(|limit| files.len() >= limit) {
            limit_reached = true;
            break;
        }
        let metadata = match std::fs::metadata(root) {
            Ok(metadata) => metadata,
            Err(error) => {
                issues.push(DiscoveryIssue {
                    path: path_string(root),
                    reason: error.to_string(),
                });
                continue;
            }
        };
        if metadata.is_file() {
            push_video_file(root, &mut files, &mut seen, limit, &mut limit_reached);
            continue;
        }
        if !metadata.is_dir() {
            issues.push(DiscoveryIssue {
                path: path_string(root),
                reason: "not a regular file or directory".to_string(),
            });
            continue;
        }

        let mut directories = vec![root.clone()];
        while let Some(directory) = directories.pop() {
            if limit.is_some_and(|limit| files.len() >= limit) {
                limit_reached = true;
                break;
            }
            let read_dir = match std::fs::read_dir(&directory) {
                Ok(read_dir) => read_dir,
                Err(error) => {
                    issues.push(DiscoveryIssue {
                        path: path_string(&directory),
                        reason: error.to_string(),
                    });
                    continue;
                }
            };
            let mut entries = Vec::new();
            for entry in read_dir {
                match entry {
                    Ok(entry) => match entry.file_type() {
                        Ok(file_type) => entries.push((entry.path(), file_type)),
                        Err(error) => issues.push(DiscoveryIssue {
                            path: path_string(&entry.path()),
                            reason: error.to_string(),
                        }),
                    },
                    Err(error) => issues.push(DiscoveryIssue {
                        path: path_string(&directory),
                        reason: error.to_string(),
                    }),
                }
            }
            entries.sort_by(|left, right| {
                path_string(&left.0)
                    .to_lowercase()
                    .cmp(&path_string(&right.0).to_lowercase())
            });
            let mut child_directories = Vec::new();
            for (path, file_type) in entries {
                if file_type.is_file() {
                    push_video_file(&path, &mut files, &mut seen, limit, &mut limit_reached);
                    if limit_reached {
                        break;
                    }
                } else if file_type.is_dir() {
                    child_directories.push(path);
                }
            }
            for child in child_directories.into_iter().rev() {
                directories.push(child);
            }
        }
    }

    DiscoveryReport {
        files,
        issues,
        limit_reached,
    }
}

fn push_video_file(
    path: &Path,
    files: &mut Vec<PathBuf>,
    seen: &mut HashSet<String>,
    limit: Option<usize>,
    limit_reached: &mut bool,
) {
    if !is_supported_video(path) {
        return;
    }
    if limit.is_some_and(|limit| files.len() >= limit) {
        *limit_reached = true;
        return;
    }
    let identity = path_string(path).to_lowercase();
    if seen.insert(identity) {
        files.push(path.to_path_buf());
    }
}

fn is_supported_video(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension.as_str())
}

pub fn verify_file(path: &Path, options: &BatchOptions) -> FileReport {
    let started = Instant::now();
    let path_text = path_string(path);
    let duration_secs = match probe_duration(path) {
        Ok(duration_secs) => duration_secs,
        Err(reason) => {
            return FileReport {
                path: path_text,
                duration_secs: None,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                axis: None,
                decode: None,
                windows: Vec::new(),
                skipped_reason: Some(reason),
                file_error: None,
            };
        }
    };

    let fallback_interval_secs =
        crate::ui_video_tile::pick_interval(duration_secs, FALLBACK_MAX_CELLS);
    let worker = SeekStripThumbnailWorker::spawn(
        path.to_path_buf(),
        options.hardware_decode,
        None,
        duration_secs,
        options.minimum_gap_secs,
        fallback_interval_secs,
    );

    let axis_started = Instant::now();
    let (axis, diagnostics) = loop {
        let snapshot = worker.snapshot();
        match snapshot.axis {
            StripAxisResolution::Ready(axis) => {
                let Some(diagnostics) = snapshot.axis_diagnostics else {
                    worker.cancel();
                    return file_error_report(
                        path_text,
                        duration_secs,
                        started,
                        "axis resolved without diagnostics".to_string(),
                    );
                };
                break (axis, diagnostics);
            }
            StripAxisResolution::Failed(error) => {
                worker.cancel();
                return file_error_report(path_text, duration_secs, started, error);
            }
            StripAxisResolution::Resolving => {}
        }
        if axis_started.elapsed() >= Duration::from_secs(options.axis_timeout_secs) {
            worker.cancel();
            return file_error_report(
                path_text,
                duration_secs,
                started,
                format!(
                    "axis resolution timed out after {}s",
                    options.axis_timeout_secs
                ),
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    let axis_report = build_axis_report(&axis, &diagnostics, duration_secs);
    let mut remembered_failures = BTreeMap::new();
    let mut windows = Vec::new();
    for (name, requested_start_index) in window_positions(&axis, options.visible_count) {
        windows.push(run_window(
            &worker,
            &axis,
            name,
            requested_start_index,
            options,
            &mut remembered_failures,
        ));
    }
    let decode = Some(build_decode_report(&worker.snapshot().decode_diagnostics));
    worker.cancel();

    FileReport {
        path: path_text,
        duration_secs: Some(duration_secs),
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        axis: Some(axis_report),
        decode,
        windows,
        skipped_reason: None,
        file_error: None,
    }
}

fn probe_duration(path: &Path) -> Result<f64, String> {
    ffmpeg::init().map_err(|error| format!("FFmpeg initialization failed: {error}"))?;
    let input =
        ffmpeg::format::input(path).map_err(|error| format!("cannot open input: {error}"))?;
    if input.streams().best(ffmpeg::media::Type::Video).is_none() {
        return Err("no video stream".to_string());
    }
    let duration_secs = input.duration() as f64 / ffmpeg::ffi::AV_TIME_BASE as f64;
    if !duration_secs.is_finite() || duration_secs <= 0.0 {
        return Err("duration is unavailable or non-positive".to_string());
    }
    Ok(duration_secs)
}

fn file_error_report(
    path: String,
    duration_secs: f64,
    started: Instant,
    error: String,
) -> FileReport {
    FileReport {
        path,
        duration_secs: Some(duration_secs),
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        axis: None,
        decode: None,
        windows: Vec::new(),
        skipped_reason: None,
        file_error: Some(error),
    }
}

fn build_axis_report(
    axis: &StripAxis,
    diagnostics: &StripAxisDiagnostics,
    duration_secs: f64,
) -> AxisReport {
    let (kind, interval_secs, adopted_count, adopted_spacing) = match axis {
        StripAxis::KeyframeIndex { keyframes, adopted } => {
            let adopted_times: Vec<f64> = adopted
                .iter()
                .filter_map(|index| keyframes.get(*index).copied())
                .collect();
            (
                "keyframe_index".to_string(),
                None,
                Some(adopted.len()),
                spacing_stats(&adopted_times),
            )
        }
        StripAxis::TimeGrid { interval_secs, .. } => {
            ("time_grid".to_string(), Some(*interval_secs), None, None)
        }
    };
    let (reason_code, reason) = axis_reason(diagnostics);
    AxisReport {
        kind,
        reason_code,
        reason,
        cell_count: axis.cell_count(),
        interval_secs,
        keyframe_count: diagnostics.keyframe_count,
        index_first_secs: diagnostics.first_keyframe_secs,
        index_last_secs: diagnostics.last_keyframe_secs,
        index_coverage_percent: diagnostics
            .last_keyframe_secs
            .filter(|_| duration_secs > 0.0)
            .map(|last| last / duration_secs * 100.0),
        index_timestamps_monotonic: diagnostics.index_timestamps_monotonic,
        index_inversion_count: diagnostics.index_inversion_count,
        adopted_count,
        adopted_spacing,
    }
}

fn build_decode_report(diagnostics: &StripThumbnailDecodeDiagnostics) -> DecodeReport {
    DecodeReport {
        initial_path: diagnostics.initial_path.map(decode_path_name),
        final_path: diagnostics.current_path.map(decode_path_name),
        software_retry_failure: diagnostics
            .software_retry_failure
            .as_ref()
            .map(failure_text),
        full_frame_fallback_trigger: diagnostics
            .full_frame_retry_failure
            .as_ref()
            .map(failure_text),
    }
}

fn decode_path_name(path: StripThumbnailDecodePath) -> String {
    match path {
        StripThumbnailDecodePath::HardwareD3d11va => "hardware_d3d11va",
        StripThumbnailDecodePath::Software => "software",
    }
    .to_string()
}

fn axis_reason(diagnostics: &StripAxisDiagnostics) -> (String, String) {
    let (code, text) = match diagnostics.reason {
        StripAxisResolutionReason::UsableKeyframeIndex => (
            "usable_keyframe_index",
            "index timestamps are monotonic and coverage is usable".to_string(),
        ),
        StripAxisResolutionReason::InputOpenFailed => (
            "input_open_failed",
            "axis resolver could not open the input; using time grid".to_string(),
        ),
        StripAxisResolutionReason::VideoStreamMissing => (
            "video_stream_missing",
            "axis resolver found no video stream; using time grid".to_string(),
        ),
        StripAxisResolutionReason::IndexUnavailable => (
            "index_unavailable",
            "container exposes no usable keyframe index; using time grid".to_string(),
        ),
        StripAxisResolutionReason::TooFewIndexEntries => (
            "too_few_index_entries",
            "container exposes fewer than two keyframe index entries; using time grid".to_string(),
        ),
        StripAxisResolutionReason::NonMonotonicIndexTimestamps => (
            "non_monotonic_index_timestamps",
            format!(
                "index timestamps contain {} backward jump(s); using time grid",
                diagnostics.index_inversion_count
            ),
        ),
        StripAxisResolutionReason::InvalidIndexCoverage => (
            "invalid_index_coverage",
            "keyframe index timestamps or spacing are invalid; using time grid".to_string(),
        ),
        StripAxisResolutionReason::IncompleteIndexCoverage => (
            "incomplete_index_coverage",
            "keyframe index does not cover enough of the duration; using time grid".to_string(),
        ),
        StripAxisResolutionReason::NoAdoptedKeyframes => (
            "no_adopted_keyframes",
            "minimum-gap adoption produced no cells; using time grid".to_string(),
        ),
    };
    (code.to_string(), text)
}

fn spacing_stats(times: &[f64]) -> Option<SpacingStats> {
    let mut gaps: Vec<f64> = times
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .filter(|gap| gap.is_finite() && *gap >= 0.0)
        .collect();
    if gaps.is_empty() {
        return None;
    }
    gaps.sort_by(f64::total_cmp);
    let mean_secs = gaps.iter().sum::<f64>() / gaps.len() as f64;
    Some(SpacingStats {
        minimum_secs: gaps[0],
        p50_secs: percentile(&gaps, 0.50),
        p90_secs: percentile(&gaps, 0.90),
        maximum_secs: *gaps.last().unwrap_or(&gaps[0]),
        mean_secs,
    })
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn window_positions(axis: &StripAxis, visible_count: usize) -> Vec<(String, usize)> {
    let span = visible_count.max(1).min(axis.cell_count());
    let last_start = axis.cell_count().saturating_sub(span);
    [
        ("index_0", 0.0),
        ("25_percent", 0.25),
        ("50_percent", 0.50),
        ("75_percent", 0.75),
        ("last_full_window", 1.0),
    ]
    .into_iter()
    .map(|(name, fraction)| {
        (
            name.to_string(),
            (last_start as f64 * fraction).round() as usize,
        )
    })
    .collect()
}

fn run_window(
    worker: &SeekStripThumbnailWorker,
    axis: &Arc<StripAxis>,
    name: String,
    requested_start_index: usize,
    options: &BatchOptions,
    remembered_failures: &mut BTreeMap<usize, String>,
) -> WindowReport {
    let started = Instant::now();
    let visible_count = options.visible_count.max(1).min(axis.cell_count());
    let center_index = requested_start_index as f64 + visible_count as f64 / 2.0;
    let range = compute_strip_window(
        center_index,
        visible_count,
        StripLookahead::default(),
        axis.cell_count(),
        None,
    )
    .ready
    .expect("a resolved non-empty axis must produce a visible range");
    let request_result = worker.request(
        Arc::clone(axis),
        center_index,
        visible_count,
        StripLookahead::default(),
    );

    let mut timed_out = false;
    let snapshot = if request_result.is_err() {
        worker.snapshot()
    } else {
        loop {
            let snapshot = worker.snapshot();
            for (index, failure) in &snapshot.latest_request_failures {
                remembered_failures.insert(*index, failure_text(failure));
            }
            let settled = (range.start()..=range.end()).all(|index| {
                axis.cell(index)
                    .and_then(|time| snapshot.outcome_for_secs(time))
                    .is_some()
            });
            let terminal = !matches!(snapshot.status, StripThumbnailWorkerStatus::Running);
            if settled || terminal {
                break snapshot;
            }
            if started.elapsed() >= Duration::from_secs(options.window_timeout_secs) {
                timed_out = true;
                break snapshot;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    for (index, failure) in &snapshot.latest_request_failures {
        remembered_failures.insert(*index, failure_text(failure));
    }
    let terminal_failure = worker_status_failure(&snapshot.status);
    let request_failure = request_result
        .err()
        .map(|error| format!("worker request failed: {error:?}"));
    let timeout_failure =
        timed_out.then(|| format!("window timed out after {}s", options.window_timeout_secs));

    let mut cells = Vec::new();
    for index in range.start()..=range.end() {
        let time_secs = axis.cell(index).unwrap_or(f64::NAN);
        match snapshot.outcome_for_secs(time_secs) {
            Some(StripThumbnailOutcome::Ready(_)) => cells.push(CellReport {
                index,
                time_secs,
                state: CellState::Ready,
                failure: None,
            }),
            Some(StripThumbnailOutcome::Failed) => cells.push(CellReport {
                index,
                time_secs,
                state: CellState::Failed,
                failure: Some(
                    remembered_failures
                        .get(&index)
                        .cloned()
                        .unwrap_or_else(|| "worker reported failure without a reason".to_string()),
                ),
            }),
            None => cells.push(CellReport {
                index,
                time_secs,
                state: CellState::Failed,
                failure: request_failure
                    .clone()
                    .or_else(|| timeout_failure.clone())
                    .or_else(|| terminal_failure.clone())
                    .or_else(|| Some("worker stopped before settling the cell".to_string())),
            }),
        }
    }
    let ready_count = cells
        .iter()
        .filter(|cell| cell.state == CellState::Ready)
        .count();
    let failed_count = cells.len() - ready_count;
    WindowReport {
        name,
        requested_start_index,
        actual_start_index: range.start(),
        actual_end_index: range.end(),
        ready_count,
        failed_count,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        cells,
    }
}

fn worker_status_failure(status: &StripThumbnailWorkerStatus) -> Option<String> {
    match status {
        StripThumbnailWorkerStatus::Running => None,
        StripThumbnailWorkerStatus::DecoderUnavailable(error) => {
            Some(format!("decoder unavailable: {error}"))
        }
        StripThumbnailWorkerStatus::Cancelled => Some("worker cancelled".to_string()),
        StripThumbnailWorkerStatus::ThreadSpawnFailed(error) => {
            Some(format!("worker thread spawn failed: {error}"))
        }
    }
}

fn failure_text(failure: &StripThumbnailFailure) -> String {
    match failure {
        StripThumbnailFailure::InvalidCellTime => "invalid cell time".to_string(),
        StripThumbnailFailure::DecoderUnavailable(error) => {
            format!("decoder unavailable: {error}")
        }
        StripThumbnailFailure::SeekFailed(error) => format!("seek failed: {error}"),
        StripThumbnailFailure::DemuxFailed(error) => format!("demux failed: {error}"),
        StripThumbnailFailure::DecodeFailed(error) => format!("decode failed: {error}"),
        StripThumbnailFailure::ConvertFailed(error) => format!("conversion failed: {error}"),
        StripThumbnailFailure::NoFrame => "no matching frame".to_string(),
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacing_stats_reports_named_quantiles() {
        let stats = spacing_stats(&[0.0, 2.0, 5.0, 9.0]).unwrap();
        assert_eq!(stats.minimum_secs, 2.0);
        assert_eq!(stats.p50_secs, 3.0);
        assert_eq!(stats.p90_secs, 4.0);
        assert_eq!(stats.maximum_secs, 4.0);
        assert!((stats.mean_secs - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn window_positions_cover_first_quarters_and_last_full_window() {
        let axis = StripAxis::TimeGrid {
            interval_secs: 1.0,
            duration_secs: 100.0,
        };
        let starts: Vec<_> = window_positions(&axis, 11)
            .into_iter()
            .map(|(_, start)| start)
            .collect();
        assert_eq!(starts, vec![0, 22, 45, 67, 89]);
    }
}
