use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::job::VideoUpscaleOptions;
use super::manifest::{load_json, now_unix_ms, save_json_atomic};

pub const QUEUE_SCHEMA_VERSION: u32 = 1;
pub const QUEUE_FILE_NAME: &str = "video_upscale_tasks.json";
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskQueue {
    pub schema: u32,
    pub paused: bool,
    #[serde(default = "default_parallel_segments")]
    pub parallel_segments: u8,
    #[serde(default)]
    pub tasks: Vec<VideoUpscaleTask>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoUpscaleTask {
    pub task_id: Uuid,
    pub source_path: PathBuf,
    pub manifest_path: PathBuf,
    #[serde(default)]
    pub options: VideoUpscaleOptions,
    pub state: TaskState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<FailureReason>,
    pub created_unix_ms: u64,
    pub updated_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Planning,
    Running,
    Paused,
    Canceling,
    Failed,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureReason {
    SchemaMismatch,
    StaleSource,
    AudioMux,
    NoSpace,
    PlanDrift,
    Io,
}

pub struct QueueLock {
    #[cfg(windows)]
    mutex: windows::Win32::Foundation::HANDLE,
    #[cfg(not(windows))]
    path: PathBuf,
    #[cfg(not(windows))]
    _file: std::fs::File,
}

impl TaskQueue {
    pub fn new() -> Self {
        Self {
            schema: QUEUE_SCHEMA_VERSION,
            paused: false,
            parallel_segments: default_parallel_segments(),
            tasks: Vec::new(),
        }
    }

    pub fn queue_path(data_dir: &Path) -> PathBuf {
        data_dir.join(QUEUE_FILE_NAME)
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        load_json(path)
    }

    pub fn load_or_backup_broken(path: &Path) -> (Self, Option<(PathBuf, String)>) {
        match Self::load(path) {
            Ok(queue) => (queue, None),
            Err(err) => {
                let message = err.to_string();
                let backup = broken_queue_backup_path(path, now_unix_ms());
                if path.exists() {
                    let _ = fs::rename(path, &backup);
                }
                (Self::new(), Some((backup, message)))
            }
        }
    }

    pub fn save_atomic(&self, path: &Path) -> io::Result<()> {
        save_json_atomic(path, self)
    }

    pub fn push_task(
        &mut self,
        source_path: PathBuf,
        manifest_path: PathBuf,
        options: VideoUpscaleOptions,
    ) -> Uuid {
        let now = now_unix_ms();
        let task_id = Uuid::new_v4();
        self.tasks.push(VideoUpscaleTask {
            task_id,
            source_path,
            manifest_path,
            options,
            state: TaskState::Queued,
            failure_reason: None,
            created_unix_ms: now,
            updated_unix_ms: now,
        });
        task_id
    }

    pub fn mark_state(&mut self, task_id: Uuid, state: TaskState) -> bool {
        let Some(task) = self.tasks.iter_mut().find(|task| task.task_id == task_id) else {
            return false;
        };
        task.state = state;
        if state != TaskState::Failed {
            task.failure_reason = None;
        }
        task.updated_unix_ms = now_unix_ms();
        true
    }

    pub fn mark_done(&mut self, task_id: Uuid) -> bool {
        self.mark_state(task_id, TaskState::Done)
    }

    pub fn mark_failed(&mut self, task_id: Uuid, reason: FailureReason) -> bool {
        let Some(task) = self.tasks.iter_mut().find(|task| task.task_id == task_id) else {
            return false;
        };
        task.state = TaskState::Failed;
        task.failure_reason = Some(reason);
        task.updated_unix_ms = now_unix_ms();
        true
    }

    pub fn remove_task(&mut self, task_id: Uuid) -> bool {
        let before = self.tasks.len();
        self.tasks.retain(|task| task.task_id != task_id);
        before != self.tasks.len()
    }

    pub fn move_task_up(&mut self, task_id: Uuid) -> bool {
        let Some(index) = self.tasks.iter().position(|task| task.task_id == task_id) else {
            return false;
        };
        if self.tasks[index].state != TaskState::Queued {
            return false;
        }
        let Some(prev_index) = self.tasks[..index]
            .iter()
            .rposition(|task| task.state == TaskState::Queued)
        else {
            return false;
        };
        self.tasks.swap(prev_index, index);
        let now = now_unix_ms();
        self.tasks[prev_index].updated_unix_ms = now;
        self.tasks[index].updated_unix_ms = now;
        true
    }

    pub fn move_task_down(&mut self, task_id: Uuid) -> bool {
        let Some(index) = self.tasks.iter().position(|task| task.task_id == task_id) else {
            return false;
        };
        if self.tasks[index].state != TaskState::Queued {
            return false;
        }
        let Some(relative_next) = self.tasks[index + 1..]
            .iter()
            .position(|task| task.state == TaskState::Queued)
        else {
            return false;
        };
        let next_index = index + 1 + relative_next;
        self.tasks.swap(index, next_index);
        let now = now_unix_ms();
        self.tasks[index].updated_unix_ms = now;
        self.tasks[next_index].updated_unix_ms = now;
        true
    }

    pub fn set_parallel_segments(&mut self, value: u8) -> bool {
        let value = value.clamp(1, 5);
        if self.parallel_segments == value {
            return false;
        }
        self.parallel_segments = value;
        true
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new()
    }
}

fn default_parallel_segments() -> u8 {
    1
}

impl QueueLock {
    pub fn acquire(data_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(data_dir)?;

        #[cfg(windows)]
        {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
            use windows::Win32::System::Threading::CreateMutexW;
            use windows::core::PCWSTR;

            let mut hasher = DefaultHasher::new();
            data_dir.to_string_lossy().to_lowercase().hash(&mut hasher);
            let mutex_name = format!(
                "Global\\mimageviewer_video_upscale_queue_{:016x}",
                hasher.finish()
            );
            let wide: Vec<u16> = mutex_name.encode_utf16().chain([0]).collect();
            let mutex = unsafe { CreateMutexW(None, false, PCWSTR(wide.as_ptr())) }
                .map_err(|e| io::Error::other(e.to_string()))?;
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                unsafe {
                    let _ = CloseHandle(mutex);
                }
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "video upscale queue is locked by another process",
                ));
            }
            Ok(Self { mutex })
        }

        #[cfg(not(windows))]
        {
            use std::fs::OpenOptions;

            let path = data_dir.join("video_upscale_tasks.lock");
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            Ok(Self { path, _file: file })
        }
    }

    #[cfg(not(windows))]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for QueueLock {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            use windows::Win32::Foundation::CloseHandle;
            if !self.mutex.is_invalid() {
                let _ = CloseHandle(self.mutex);
            }
        }

        #[cfg(not(windows))]
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn broken_queue_backup_path(path: &Path, timestamp_ms: u64) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(QUEUE_FILE_NAME);
    path.with_file_name(format!("{file_name}.broken-{timestamp_ms}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_roundtrips_with_failure_reason() {
        let task_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();
        let queue = TaskQueue {
            schema: QUEUE_SCHEMA_VERSION,
            paused: true,
            parallel_segments: 3,
            tasks: vec![VideoUpscaleTask {
                task_id,
                source_path: PathBuf::from(r"E:\videos\movie.mp4"),
                manifest_path: PathBuf::from(r"E:\videos\movie.miv.work\job.miv-upscale.json"),
                options: VideoUpscaleOptions::default(),
                state: TaskState::Failed,
                failure_reason: Some(FailureReason::NoSpace),
                created_unix_ms: 10,
                updated_unix_ms: 20,
            }],
        };

        let json = serde_json::to_string_pretty(&queue).unwrap();
        assert!(json.contains("\"state\": \"failed\""));
        assert!(json.contains("\"failure_reason\": \"no_space\""));

        let parsed: TaskQueue = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, queue);
    }

    #[test]
    fn queue_parallel_segments_defaults_and_clamps() {
        let legacy = r#"{"schema":1,"paused":false,"tasks":[]}"#;
        let parsed: TaskQueue = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.parallel_segments, 1);

        let mut queue = TaskQueue::new();
        assert_eq!(queue.parallel_segments, 1);
        assert!(!queue.set_parallel_segments(1));
        assert!(queue.set_parallel_segments(4));
        assert_eq!(queue.parallel_segments, 4);
        assert!(queue.set_parallel_segments(9));
        assert_eq!(queue.parallel_segments, 5);
        assert!(queue.set_parallel_segments(0));
        assert_eq!(queue.parallel_segments, 1);
    }

    #[test]
    fn queue_save_load_uses_empty_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = TaskQueue::queue_path(dir.path());

        let missing = TaskQueue::load(&path).unwrap();
        assert!(missing.tasks.is_empty());

        let mut queue = TaskQueue::new();
        queue.push_task(
            PathBuf::from("movie.mp4"),
            PathBuf::from("movie.miv.work/job.json"),
            VideoUpscaleOptions::default(),
        );
        queue.save_atomic(&path).unwrap();

        let loaded = TaskQueue::load(&path).unwrap();
        assert_eq!(loaded.tasks.len(), 1);
        assert_eq!(loaded.tasks[0].state, TaskState::Queued);
    }

    #[test]
    fn load_or_backup_broken_preserves_corrupt_queue() {
        let dir = tempfile::tempdir().unwrap();
        let path = TaskQueue::queue_path(dir.path());
        fs::write(&path, "{not json").unwrap();

        let (queue, backup) = TaskQueue::load_or_backup_broken(&path);

        assert!(queue.tasks.is_empty());
        let (backup_path, message) = backup.expect("broken queue is reported");
        assert!(!message.is_empty());
        assert!(!path.exists());
        assert!(backup_path.exists());
        assert_eq!(fs::read_to_string(backup_path).unwrap(), "{not json");
    }

    #[test]
    fn queue_lock_is_exclusive_and_removed_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let first = QueueLock::acquire(dir.path()).unwrap();
        assert!(QueueLock::acquire(dir.path()).is_err());

        drop(first);
        assert!(QueueLock::acquire(dir.path()).is_ok());
    }

    #[test]
    fn mark_failed_updates_task_state() {
        let mut queue = TaskQueue::new();
        let task_id = queue.push_task(
            PathBuf::from("movie.mp4"),
            PathBuf::from("manifest.json"),
            VideoUpscaleOptions::default(),
        );

        assert!(queue.mark_failed(task_id, FailureReason::PlanDrift));

        assert_eq!(queue.tasks[0].state, TaskState::Failed);
        assert_eq!(
            queue.tasks[0].failure_reason,
            Some(FailureReason::PlanDrift)
        );
    }

    #[test]
    fn mark_state_and_done_clear_failure_reason() {
        let mut queue = TaskQueue::new();
        let task_id = queue.push_task(
            PathBuf::from("movie.mp4"),
            PathBuf::from("manifest.json"),
            VideoUpscaleOptions::default(),
        );
        assert!(queue.mark_failed(task_id, FailureReason::Io));

        assert!(queue.mark_state(task_id, TaskState::Running));
        assert_eq!(queue.tasks[0].state, TaskState::Running);
        assert_eq!(queue.tasks[0].failure_reason, None);

        assert!(queue.mark_done(task_id));
        assert_eq!(queue.tasks[0].state, TaskState::Done);
    }

    #[test]
    fn queued_tasks_can_move_across_other_states() {
        let mut queue = TaskQueue::new();
        let running = queue.push_task(
            PathBuf::from("running.mp4"),
            PathBuf::from("running.json"),
            VideoUpscaleOptions::default(),
        );
        let first = queue.push_task(
            PathBuf::from("first.mp4"),
            PathBuf::from("first.json"),
            VideoUpscaleOptions::default(),
        );
        let failed = queue.push_task(
            PathBuf::from("failed.mp4"),
            PathBuf::from("failed.json"),
            VideoUpscaleOptions::default(),
        );
        let second = queue.push_task(
            PathBuf::from("second.mp4"),
            PathBuf::from("second.json"),
            VideoUpscaleOptions::default(),
        );
        queue.mark_state(running, TaskState::Running);
        queue.mark_failed(failed, FailureReason::Io);

        assert!(queue.move_task_up(second));
        assert_eq!(queue.tasks[1].task_id, second);
        assert_eq!(queue.tasks[3].task_id, first);

        assert!(queue.move_task_down(second));
        assert_eq!(queue.tasks[1].task_id, first);
        assert_eq!(queue.tasks[3].task_id, second);
        assert!(!queue.move_task_up(running));
    }
}
