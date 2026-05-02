use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobManifest {
    pub schema: u32,
    pub task_id: Uuid,
    pub source: ManifestSource,
    pub output: ManifestOutput,
    pub options: ManifestOptions,
    pub progress: ManifestProgress,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<SegmentPlan>,
    #[serde(default)]
    pub segments: Vec<SegmentEntry>,
    pub updated_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestSource {
    pub file_name: String,
    pub size: u64,
    pub mtime_unix_ms: u64,
    pub head_tail_sha256: String,
    pub time_base: TimeBase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestOutput {
    pub final_path: PathBuf,
    pub sidecar_path: PathBuf,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestOptions {
    pub scale: u32,
    pub model: String,
    pub quality_level: u8,
    pub container: String,
    pub video_codec: String,
    pub encoder: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestProgress {
    pub estimated_frames: u64,
    pub completed_frames: u64,
    pub next_output_frame_index: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentPlan {
    pub strategy: SegmentPlanStrategy,
    pub state: SegmentPlanState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_progress_pts: Option<i64>,
    #[serde(default)]
    pub segments: Vec<PlannedSegment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentPlanStrategy {
    SourceKeyframeSnap,
    TimeBased,
    FrameBased,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentPlanState {
    Planning,
    Complete,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedSegment {
    pub index: u32,
    pub target_start_frame: u64,
    pub target_end_frame_exclusive: u64,
    pub target_start_pts: i64,
    pub target_end_pts: i64,
    #[serde(default)]
    pub seek_start_frame: u64,
    pub seek_start_pts: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentEntry {
    pub index: u32,
    pub path: PathBuf,
    pub state: SegmentState,
    pub output_frame_start: u64,
    pub output_frame_count: u64,
    pub output_total_pts_ticks: i64,
    pub output_time_base: TimeBase,
    pub source_start_pts: i64,
    pub source_last_pts: i64,
    pub size: u64,
    pub mtime_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_started_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentState {
    Pending,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "[i64; 2]", into = "[i64; 2]")]
pub struct TimeBase {
    pub num: i32,
    pub den: i32,
}

impl TimeBase {
    pub fn new(num: i32, den: i32) -> Self {
        Self { num, den }
    }
}

impl From<[i64; 2]> for TimeBase {
    fn from(value: [i64; 2]) -> Self {
        Self {
            num: value[0] as i32,
            den: value[1] as i32,
        }
    }
}

impl From<TimeBase> for [i64; 2] {
    fn from(value: TimeBase) -> Self {
        [value.num as i64, value.den as i64]
    }
}

impl JobManifest {
    pub fn new(
        task_id: Uuid,
        source: ManifestSource,
        output: ManifestOutput,
        options: ManifestOptions,
        estimated_frames: u64,
    ) -> Self {
        Self {
            schema: MANIFEST_SCHEMA_VERSION,
            task_id,
            source,
            output,
            options,
            progress: ManifestProgress {
                estimated_frames,
                completed_frames: 0,
                next_output_frame_index: 0,
            },
            plan: None,
            segments: Vec::new(),
            updated_unix_ms: now_unix_ms(),
        }
    }

    pub fn is_supported_schema(&self) -> bool {
        self.schema == MANIFEST_SCHEMA_VERSION
    }

    pub fn plan_is_complete(&self) -> bool {
        self.plan.as_ref().is_some_and(|plan| {
            plan.state == SegmentPlanState::Complete && !plan.segments.is_empty()
        })
    }

    pub fn reset_stale_running_segments(
        &mut self,
        is_worker_alive: impl Fn(u32, u64) -> bool,
    ) -> usize {
        let mut reset = 0;
        for segment in &mut self.segments {
            if segment.state == SegmentState::Running {
                let alive = match (segment.worker_pid, segment.worker_started_unix_ms) {
                    (Some(pid), Some(started_ms)) => is_worker_alive(pid, started_ms),
                    _ => false,
                };
                if !alive {
                    segment.state = SegmentState::Pending;
                    segment.worker_id = None;
                    segment.worker_pid = None;
                    segment.worker_started_unix_ms = None;
                    reset += 1;
                }
            }
        }
        if reset > 0 {
            self.updated_unix_ms = now_unix_ms();
        }
        reset
    }

    pub fn save_atomic(&mut self, path: &Path) -> io::Result<()> {
        self.updated_unix_ms = now_unix_ms();
        save_json_atomic(path, self)
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        load_json(path)
    }
}

pub fn save_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| format!("{ext}."))
            .unwrap_or_default()
    ));
    let json = serde_json::to_string_pretty(value).map_err(io_other)?;
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path).or_else(|err| {
        let _ = fs::remove_file(&tmp);
        Err(err)
    })
}

pub fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<T> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(io_invalid_data)
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn io_invalid_data(err: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

fn io_other(err: serde_json::Error) -> io::Error {
    io::Error::other(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> JobManifest {
        let task_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();
        let mut manifest = JobManifest::new(
            task_id,
            ManifestSource {
                file_name: "movie.mp4".to_owned(),
                size: 123,
                mtime_unix_ms: 456,
                head_tail_sha256: "abcd".to_owned(),
                time_base: TimeBase::new(1, 24_000),
            },
            ManifestOutput {
                final_path: PathBuf::from("movie.miv.mkv"),
                sidecar_path: PathBuf::from("movie.miv.json"),
                width: 2560,
                height: 1440,
            },
            ManifestOptions {
                scale: 4,
                model: "realesrgan_anime6b".to_owned(),
                quality_level: 3,
                container: "mkv".to_owned(),
                video_codec: "av1".to_owned(),
                encoder: "libsvtav1".to_owned(),
            },
            240,
        );
        manifest.plan = Some(SegmentPlan {
            strategy: SegmentPlanStrategy::SourceKeyframeSnap,
            state: SegmentPlanState::Complete,
            scan_progress_pts: Some(120120),
            segments: vec![PlannedSegment {
                index: 0,
                target_start_frame: 0,
                target_end_frame_exclusive: 120,
                target_start_pts: 0,
                target_end_pts: 120120,
                seek_start_frame: 0,
                seek_start_pts: 0,
            }],
        });
        manifest.segments.push(SegmentEntry {
            index: 0,
            path: PathBuf::from("segments/000000.mkv"),
            state: SegmentState::Done,
            output_frame_start: 0,
            output_frame_count: 120,
            output_total_pts_ticks: 120120,
            output_time_base: TimeBase::new(1, 24_000),
            source_start_pts: 0,
            source_last_pts: 119119,
            size: 123456,
            mtime_unix_ms: 789,
            worker_id: None,
            worker_pid: None,
            worker_started_unix_ms: None,
        });
        manifest
    }

    #[test]
    fn time_base_serializes_as_json_array() {
        let json = serde_json::to_string(&TimeBase::new(1, 24_000)).unwrap();
        assert_eq!(json, "[1,24000]");

        let parsed: TimeBase = serde_json::from_str("[1001,24000]").unwrap();
        assert_eq!(parsed.num, 1001);
        assert_eq!(parsed.den, 24_000);
    }

    #[test]
    fn manifest_roundtrips_with_plan_and_segment_metadata() {
        let manifest = sample_manifest();
        let json = serde_json::to_string_pretty(&manifest).unwrap();

        assert!(json.contains("\"strategy\": \"source_keyframe_snap\""));
        assert!(json.contains("\"output_time_base\": ["));

        let parsed: JobManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, manifest);
        assert!(parsed.plan_is_complete());
    }

    #[test]
    fn atomic_save_and_load_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("movie.miv.work")
            .join("job.miv-upscale.json");
        let mut manifest = sample_manifest();

        manifest.save_atomic(&path).unwrap();
        let loaded = JobManifest::load(&path).unwrap();

        assert_eq!(loaded.task_id, manifest.task_id);
        assert_eq!(loaded.source.time_base, TimeBase::new(1, 24_000));
    }

    #[test]
    fn stale_running_segments_reset_to_pending() {
        let mut manifest = sample_manifest();
        manifest.segments[0].state = SegmentState::Running;
        manifest.segments[0].worker_id = Some("w1".to_owned());
        manifest.segments[0].worker_pid = Some(1234);
        manifest.segments[0].worker_started_unix_ms = Some(999);

        let reset = manifest
            .reset_stale_running_segments(|pid, started_ms| pid == 5678 && started_ms == 999);

        assert_eq!(reset, 1);
        assert_eq!(manifest.segments[0].state, SegmentState::Pending);
        assert!(manifest.segments[0].worker_id.is_none());
    }

    #[test]
    fn running_segment_keeps_alive_only_when_pid_and_start_time_match() {
        let mut manifest = sample_manifest();
        manifest.segments[0].state = SegmentState::Running;
        manifest.segments[0].worker_pid = Some(1234);
        manifest.segments[0].worker_started_unix_ms = Some(999);

        let reset = manifest
            .reset_stale_running_segments(|pid, started_ms| pid == 1234 && started_ms == 999);

        assert_eq!(reset, 0);
        assert_eq!(manifest.segments[0].state, SegmentState::Running);
    }

    #[test]
    fn unsupported_schema_is_reported() {
        let mut manifest = sample_manifest();
        manifest.schema = 99;

        assert!(!manifest.is_supported_schema());
    }
}
