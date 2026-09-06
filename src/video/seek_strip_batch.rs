//! Batch verification facade for the production seek-strip thumbnail worker.
//!
//! This module intentionally contains no thumbnail decoder or axis implementation. It drives
//! SeekStripThumbnailWorker, uses the resolved StripAxis, and turns worker snapshots into
//! stable text/JSON-friendly reports.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ffmpeg_the_third as ffmpeg;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::seek_strip::{StripAxis, StripLookahead, compute_strip_window};
use super::seek_strip_thumbs::{
    STRIP_THUMB_CELL_TIMEOUT_SECS, SeekStripThumbnailWorker, StripAxisDiagnostics,
    StripAxisResolution, StripAxisResolutionReason, StripThumbnailDecodeDiagnostics,
    StripThumbnailDecodePath, StripThumbnailFailure, StripThumbnailOutcome,
    StripThumbnailRequestTrigger, StripThumbnailWorkerStatus,
};

const FALLBACK_MAX_CELLS: usize = 240;
const DUPLICATE_SAMPLE_BYTES: usize = 8 * 1024;
const FLAT_MAX_LUMINANCE_VARIANCE: f64 = 1.0;
const FLAT_MAX_CHANNEL_RANGE: u8 = 4;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct SampledFileFingerprint {
    size_bytes: u64,
    sample_sha256: [u8; 32],
}

/// Finds byte-identical candidates without turning a multi-terabyte sweep into a full read.
///
/// Each large file contributes only an 8 KiB head and 8 KiB tail sample, with file size kept in
/// the key. This is deliberately content-based (not codec/GOP shape) so distinct streams still
/// exercise the decoder. As requested for the sweep tool, a matching sample is treated as an
/// identical duplicate; this is a fast sampled identity check, not a cryptographic whole-file
/// proof.
#[derive(Default)]
pub struct DuplicateDetector {
    seen: HashMap<SampledFileFingerprint, String>,
}

impl DuplicateDetector {
    pub fn check(&mut self, path: &Path) -> std::io::Result<Option<String>> {
        let fingerprint = sampled_file_fingerprint(path)?;
        if let Some(matched_path) = self.seen.get(&fingerprint) {
            return Ok(Some(matched_path.clone()));
        }
        self.seen.insert(fingerprint, path_string(path));
        Ok(None)
    }
}

fn sampled_file_fingerprint(path: &Path) -> std::io::Result<SampledFileFingerprint> {
    let mut file = std::fs::File::open(path)?;
    let size_bytes = file.metadata()?.len();
    let mut hasher = Sha256::new();
    let mut sample = vec![0_u8; DUPLICATE_SAMPLE_BYTES];
    if size_bytes <= (DUPLICATE_SAMPLE_BYTES * 2) as u64 {
        sample.clear();
        file.read_to_end(&mut sample)?;
        hasher.update(&sample);
    } else {
        file.read_exact(&mut sample)?;
        hasher.update(&sample);
        file.seek(std::io::SeekFrom::End(-(DUPLICATE_SAMPLE_BYTES as i64)))?;
        file.read_exact(&mut sample)?;
        hasher.update(&sample);
    }
    Ok(SampledFileFingerprint {
        size_bytes,
        sample_sha256: hasher.finalize().into(),
    })
}

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
            // The production worker may make one bounded recovery pass after a cell timeout.
            window_timeout_secs: STRIP_THUMB_CELL_TIMEOUT_SECS * 2 + 5,
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
    pub maximum_keyframe_gap_secs: Option<f64>,
    pub adopted_count: Option<usize>,
    pub adopted_spacing: Option<SpacingStats>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CellState {
    Ready,
    Flat,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct CellPixelStats {
    /// Mean BT.709 luminance in the 0..=255 sample domain.
    pub mean_luminance: f64,
    /// Population variance of BT.709 luminance in squared sample values.
    pub luminance_variance: f64,
    /// Largest of the red, green, and blue channel ranges.
    pub maximum_channel_range: u8,
}

impl CellPixelStats {
    fn effectively_flat(&self) -> bool {
        self.luminance_variance <= FLAT_MAX_LUMINANCE_VARIANCE
            && self.maximum_channel_range <= FLAT_MAX_CHANNEL_RANGE
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct FlatCellCriterion {
    pub luminance: String,
    pub maximum_luminance_variance: f64,
    pub maximum_channel_range: u8,
    pub interpretation: String,
}

impl Default for FlatCellCriterion {
    fn default() -> Self {
        Self {
            luminance: "BT.709: 0.2126 R + 0.7152 G + 0.0722 B, samples 0..255".to_string(),
            maximum_luminance_variance: FLAT_MAX_LUMINANCE_VARIANCE,
            maximum_channel_range: FLAT_MAX_CHANNEL_RANGE,
            interpretation: "reported for review, not asserted as a defect; real fades, black frames, and flat title cards can match".to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CellReport {
    pub index: usize,
    pub time_secs: f64,
    pub state: CellState,
    pub pixels: Option<CellPixelStats>,
    pub failure: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WindowReport {
    pub name: String,
    pub requested_center_index: f64,
    pub actual_start_index: usize,
    pub actual_end_index: usize,
    pub ready_count: usize,
    pub flat_count: usize,
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
    pub unavailable_reason: Option<String>,
    pub duplicate_of: Option<String>,
    pub file_error: Option<String>,
}

impl FileReport {
    pub fn failed_cell_count(&self) -> usize {
        self.windows.iter().map(|window| window.failed_count).sum()
    }

    pub fn flat_cell_count(&self) -> usize {
        self.windows.iter().map(|window| window.flat_count).sum()
    }

    pub fn passed(&self) -> bool {
        self.skipped_reason.is_none()
            && self.unavailable_reason.is_none()
            && self.duplicate_of.is_none()
            && self.file_error.is_none()
            && self.flat_cell_count() == 0
            && self.failed_cell_count() == 0
    }

    pub fn skipped(&self) -> bool {
        self.skipped_reason.is_some()
    }

    pub fn unavailable(&self) -> bool {
        self.unavailable_reason.is_some()
    }

    pub fn duplicate(&self) -> bool {
        self.duplicate_of.is_some()
    }

    pub fn verification_failed(&self) -> bool {
        self.file_error.is_some() || self.failed_cell_count() > 0
    }

    pub fn flagged_for_review(&self) -> bool {
        self.flat_cell_count() > 0
    }

    pub fn duplicate_of(path: &Path, matched_path: String) -> Self {
        Self {
            path: path_string(path),
            duration_secs: None,
            elapsed_ms: 0.0,
            axis: None,
            decode: None,
            windows: Vec::new(),
            skipped_reason: None,
            unavailable_reason: None,
            duplicate_of: Some(matched_path),
            file_error: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BatchReport {
    pub schema_version: u32,
    pub roots: Vec<String>,
    pub options: BatchOptions,
    pub flat_cell_criterion: FlatCellCriterion,
    pub discovery_issues: Vec<DiscoveryIssue>,
    pub limit_reached: bool,
    pub files: Vec<FileReport>,
    pub passed_files: usize,
    pub failed_files: usize,
    pub skipped_files: usize,
    pub unavailable_files: usize,
    pub duplicate_files: usize,
    pub flat_files: usize,
    pub flat_cells: usize,
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
        let unavailable_files = files.iter().filter(|file| file.unavailable()).count();
        let duplicate_files = files.iter().filter(|file| file.duplicate()).count();
        let flat_files = files
            .iter()
            .filter(|file| file.flagged_for_review())
            .count();
        let flat_cells = files.iter().map(FileReport::flat_cell_count).sum();
        let failed_cells = files.iter().map(FileReport::failed_cell_count).sum();
        Self {
            schema_version: 3,
            roots: roots.iter().map(|path| path_string(path)).collect(),
            options,
            flat_cell_criterion: FlatCellCriterion::default(),
            discovery_issues: discovery.issues,
            limit_reached: discovery.limit_reached,
            files,
            passed_files,
            failed_files,
            skipped_files,
            unavailable_files,
            duplicate_files,
            flat_files,
            flat_cells,
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
                unavailable_reason: None,
                duplicate_of: None,
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
            StripAxisResolution::Unavailable(reason) => {
                let Some(diagnostics) = snapshot.axis_diagnostics else {
                    worker.cancel();
                    return file_error_report(
                        path_text,
                        duration_secs,
                        started,
                        "axis declined material without diagnostics".to_string(),
                    );
                };
                let axis = build_unavailable_axis_report(&diagnostics, duration_secs);
                let decode = Some(build_decode_report(&snapshot.decode_diagnostics));
                worker.cancel();
                return FileReport {
                    path: path_text,
                    duration_secs: Some(duration_secs),
                    elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                    axis: Some(axis),
                    decode,
                    windows: Vec::new(),
                    skipped_reason: None,
                    unavailable_reason: Some(reason.user_notice().to_string()),
                    duplicate_of: None,
                    file_error: None,
                };
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
    let mut windows = Vec::new();
    for (name, requested_center_index) in window_positions(&axis) {
        windows.push(run_window(
            &worker,
            &axis,
            name,
            requested_center_index,
            options,
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
        unavailable_reason: None,
        duplicate_of: None,
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
        unavailable_reason: None,
        duplicate_of: None,
        file_error: Some(error),
    }
}

fn build_axis_report(
    axis: &StripAxis,
    diagnostics: &StripAxisDiagnostics,
    duration_secs: f64,
) -> AxisReport {
    let (kind, interval_secs, adopted_count, adopted_spacing) = match axis {
        StripAxis::KeyframeIndex {
            keyframes, adopted, ..
        } => {
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
        maximum_keyframe_gap_secs: diagnostics.maximum_keyframe_gap_secs,
        adopted_count,
        adopted_spacing,
    }
}

fn build_unavailable_axis_report(
    diagnostics: &StripAxisDiagnostics,
    duration_secs: f64,
) -> AxisReport {
    let (reason_code, reason) = axis_reason(diagnostics);
    AxisReport {
        kind: "unavailable".to_string(),
        reason_code,
        reason,
        cell_count: 0,
        interval_secs: None,
        keyframe_count: diagnostics.keyframe_count,
        index_first_secs: diagnostics.first_keyframe_secs,
        index_last_secs: diagnostics.last_keyframe_secs,
        index_coverage_percent: diagnostics
            .last_keyframe_secs
            .filter(|_| duration_secs > 0.0)
            .map(|last| last / duration_secs * 100.0),
        index_timestamps_monotonic: diagnostics.index_timestamps_monotonic,
        index_inversion_count: diagnostics.index_inversion_count,
        maximum_keyframe_gap_secs: diagnostics.maximum_keyframe_gap_secs,
        adopted_count: None,
        adopted_spacing: None,
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
        StripAxisResolutionReason::SparseKeyframes => (
            "sparse_keyframes",
            format!(
                "raw keyframe gap {:.2}s exceeds the {:.2}s strip limit",
                diagnostics.maximum_keyframe_gap_secs.unwrap_or(f64::NAN),
                super::seek_strip::SEEK_STRIP_MAX_RAW_KEYFRAME_GAP_SECS,
            ),
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

fn window_positions(axis: &StripAxis) -> Vec<(String, f64)> {
    let last_index = axis.cell_count().saturating_sub(1) as f64;
    [
        ("index_0", 0.0),
        ("25_percent", 0.25),
        ("50_percent", 0.50),
        ("75_percent", 0.75),
        ("final_cell", 1.0),
    ]
    .into_iter()
    .map(|(name, fraction)| (name.to_string(), last_index * fraction))
    .collect()
}

fn verification_window_range(
    axis: &StripAxis,
    center_index: f64,
    visible_count: usize,
) -> std::ops::RangeInclusive<usize> {
    let range = compute_strip_window(
        center_index,
        visible_count.max(1).min(axis.cell_count()),
        StripLookahead::default(),
        axis.cell_count(),
        None,
    )
    .ready
    .expect("a resolved non-empty axis must produce a visible range");
    range.start()..=range.end()
}

fn run_window(
    worker: &SeekStripThumbnailWorker,
    axis: &Arc<StripAxis>,
    name: String,
    requested_center_index: f64,
    options: &BatchOptions,
) -> WindowReport {
    let started = Instant::now();
    let visible_count = options.visible_count.max(1).min(axis.cell_count());
    let center_index = requested_center_index;
    let range = verification_window_range(axis, center_index, visible_count);
    let actual_start_index = *range.start();
    let actual_end_index = *range.end();
    let request_result = worker.request(
        Arc::clone(axis),
        center_index,
        visible_count,
        StripLookahead::default(),
        StripThumbnailRequestTrigger::StripRedisplayed,
    );

    let mut timed_out = false;
    let snapshot = if request_result.is_err() {
        worker.snapshot()
    } else {
        loop {
            let snapshot = worker.snapshot();
            let settled = (actual_start_index..=actual_end_index).all(|index| {
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

    let terminal_failure = worker_status_failure(&snapshot.status);
    let request_failure = request_result
        .err()
        .map(|error| format!("worker request failed: {error:?}"));
    let timeout_failure = timed_out.then(|| window_timeout_text(options.window_timeout_secs));

    let mut cells = Vec::new();
    for index in actual_start_index..=actual_end_index {
        let time_secs = axis.cell(index).unwrap_or(f64::NAN);
        match snapshot.outcome_for_secs(time_secs) {
            Some(StripThumbnailOutcome::Ready(thumbnail)) => match measure_cell_pixels(thumbnail) {
                Ok(pixels) => cells.push(CellReport {
                    index,
                    time_secs,
                    state: if pixels.effectively_flat() {
                        CellState::Flat
                    } else {
                        CellState::Ready
                    },
                    pixels: Some(pixels),
                    failure: None,
                }),
                Err(failure) => cells.push(CellReport {
                    index,
                    time_secs,
                    state: CellState::Failed,
                    pixels: None,
                    failure: Some(format!("pixel measurement failed: {failure}")),
                }),
            },
            Some(StripThumbnailOutcome::Failed(failure)) => cells.push(CellReport {
                index,
                time_secs,
                state: CellState::Failed,
                pixels: None,
                failure: Some(failure_text(failure)),
            }),
            None => cells.push(CellReport {
                index,
                time_secs,
                state: CellState::Failed,
                pixels: None,
                failure: request_failure
                    .clone()
                    .or_else(|| timeout_failure.clone())
                    .or_else(|| terminal_failure.clone())
                    .or_else(|| snapshot.latest_failure_for_index(index).map(failure_text))
                    .or_else(|| Some("worker stopped before settling the cell".to_string())),
            }),
        }
    }
    let ready_count = cells
        .iter()
        .filter(|cell| cell.state == CellState::Ready)
        .count();
    let flat_count = cells
        .iter()
        .filter(|cell| cell.state == CellState::Flat)
        .count();
    let failed_count = cells
        .iter()
        .filter(|cell| cell.state == CellState::Failed)
        .count();
    WindowReport {
        name,
        requested_center_index,
        actual_start_index,
        actual_end_index,
        ready_count,
        flat_count,
        failed_count,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        cells,
    }
}

fn measure_cell_pixels(
    thumbnail: &super::seek_strip_thumbs::StripThumbnail,
) -> Result<CellPixelStats, String> {
    let pixel_count = (thumbnail.width as usize)
        .checked_mul(thumbnail.height as usize)
        .ok_or_else(|| "pixel count overflow".to_string())?;
    let expected_len = pixel_count
        .checked_mul(4)
        .ok_or_else(|| "RGBA byte count overflow".to_string())?;
    if pixel_count == 0 || thumbnail.rgba.len() != expected_len {
        return Err(format!(
            "expected {} RGBA bytes for {}x{}, got {}",
            expected_len,
            thumbnail.width,
            thumbnail.height,
            thumbnail.rgba.len()
        ));
    }

    let mut luminance_sum = 0.0;
    let mut luminance_squared_sum = 0.0;
    let mut minimum = [u8::MAX; 3];
    let mut maximum = [u8::MIN; 3];
    for pixel in thumbnail.rgba.chunks_exact(4) {
        let luminance = 0.2126 * f64::from(pixel[0])
            + 0.7152 * f64::from(pixel[1])
            + 0.0722 * f64::from(pixel[2]);
        luminance_sum += luminance;
        luminance_squared_sum += luminance * luminance;
        for channel in 0..3 {
            minimum[channel] = minimum[channel].min(pixel[channel]);
            maximum[channel] = maximum[channel].max(pixel[channel]);
        }
    }
    let sample_count = pixel_count as f64;
    let mean_luminance = luminance_sum / sample_count;
    let luminance_variance =
        (luminance_squared_sum / sample_count - mean_luminance * mean_luminance).max(0.0);
    let maximum_channel_range = (0..3)
        .map(|channel| maximum[channel] - minimum[channel])
        .max()
        .unwrap_or_default();
    Ok(CellPixelStats {
        mean_luminance,
        luminance_variance,
        maximum_channel_range,
    })
}

fn worker_status_failure(status: &StripThumbnailWorkerStatus) -> Option<String> {
    match status {
        StripThumbnailWorkerStatus::Running => None,
        StripThumbnailWorkerStatus::MaterialUnavailable(reason) => {
            Some(reason.user_notice().to_string())
        }
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
        StripThumbnailFailure::NoFrame(reason) => {
            let last = reason
                .last_frame_pts_ms
                .map(|ms| format!("{:.3}", ms as f64 / 1000.0))
                .unwrap_or_else(|| "none".to_string());
            format!(
                "no matching frame ({}, target={:.3}, last_frame={last}, tol=-{:.3}/+{:.3})",
                if reason.at_end_of_stream {
                    "end of stream"
                } else {
                    "mid stream"
                },
                reason.target_ms as f64 / 1000.0,
                reason.tolerance_before_ms as f64 / 1000.0,
                reason.tolerance_after_ms as f64 / 1000.0,
            )
        }
    }
}

fn window_timeout_text(timeout_secs: u64) -> String {
    format!("thumbnail window timed out after {timeout_secs}s")
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
    fn window_timeout_text_keeps_the_harness_reason() {
        assert_eq!(
            window_timeout_text(30),
            "thumbnail window timed out after 30s"
        );
    }

    #[test]
    fn window_positions_anchor_the_true_first_and_final_cells() {
        let axis = StripAxis::TimeGrid {
            interval_secs: 1.0,
            fallback_interval_secs: 1.0,
            duration_secs: 100.0,
        };
        let centers: Vec<_> = window_positions(&axis)
            .into_iter()
            .map(|(_, center)| center)
            .collect();
        assert_eq!(centers, vec![0.0, 24.75, 49.5, 74.25, 99.0]);

        let first = verification_window_range(&axis, centers[0], 11);
        let final_cells = verification_window_range(&axis, centers[4], 11);
        assert_eq!(first.start(), &0);
        assert_eq!(final_cells.end(), &(axis.cell_count() - 1));
        assert!(final_cells.contains(&(axis.cell_count() - 4)));
    }

    #[test]
    fn pixel_measurement_flags_only_effectively_flat_single_colour_cells() {
        let flat = super::super::seek_strip_thumbs::StripThumbnail {
            target_secs: 0.0,
            width: 2,
            height: 2,
            rgba: Arc::new(vec![
                16, 32, 48, 255, 16, 32, 48, 255, 16, 32, 48, 255, 16, 32, 48, 255,
            ]),
        };
        let flat_stats = measure_cell_pixels(&flat).unwrap();
        assert!(flat_stats.effectively_flat());
        assert_eq!(flat_stats.maximum_channel_range, 0);

        let detailed = super::super::seek_strip_thumbs::StripThumbnail {
            rgba: Arc::new(vec![0, 0, 0, 255, 8, 0, 0, 255, 0, 8, 0, 255, 0, 0, 8, 255]),
            ..flat
        };
        assert!(!measure_cell_pixels(&detailed).unwrap().effectively_flat());
    }

    #[test]
    fn duplicate_detector_reports_the_first_matching_path() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.mp4");
        let duplicate = directory.path().join("duplicate.mp4");
        let bytes = vec![0x5a; DUPLICATE_SAMPLE_BYTES * 3];
        std::fs::write(&first, &bytes).unwrap();
        std::fs::write(&duplicate, &bytes).unwrap();

        let mut detector = DuplicateDetector::default();
        assert_eq!(detector.check(&first).unwrap(), None);
        assert_eq!(
            detector.check(&duplicate).unwrap(),
            Some(path_string(&first))
        );
    }

    #[test]
    fn duplicate_detector_keeps_same_size_files_with_different_samples() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.mp4");
        let distinct = directory.path().join("distinct.mp4");
        let first_bytes = vec![0x11; DUPLICATE_SAMPLE_BYTES * 3];
        let mut distinct_bytes = first_bytes.clone();
        distinct_bytes[0] = 0x22;
        std::fs::write(&first, first_bytes).unwrap();
        std::fs::write(&distinct, distinct_bytes).unwrap();

        let mut detector = DuplicateDetector::default();
        assert_eq!(detector.check(&first).unwrap(), None);
        assert_eq!(detector.check(&distinct).unwrap(), None);
    }
}
