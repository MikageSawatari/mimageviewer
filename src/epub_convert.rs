//! Background-only EPUB conversion. S2b will own scheduling and call this API.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use serde::Deserialize;

use crate::epub_cache::{
    self, EpubCache, GenerationRow, PublishOutcome, SourceGuard, WriteDenyingSource,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Parse,
    Extract,
    Init,
    Print,
    Merge,
    Verify,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Success,
    Drm,
    Invalid,
    Webview2Missing,
    Webview2Overridden,
    Webview2Unsupported,
    RenderFailed,
    Timeout,
}

// Kept locally because the worker is a standalone package, not a core dependency.
// Exact serde field names mirror crates/epub-pdf-worker/src/protocol.rs.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WorkerEvent {
    Progress {
        phase: Phase,
        done: usize,
        total: usize,
    },
    Result {
        status: Status,
        exit_code: i32,
        page_count: usize,
        direction: String,
        layout: String,
        profile: String,
        blocked_requests: usize,
        message: String,
    },
}

#[derive(Clone, Debug)]
pub struct ConvertProgress {
    pub phase: Phase,
    pub done: usize,
    pub total: usize,
}

#[derive(Debug)]
pub enum EpubConvertError {
    Drm,
    Invalid,
    WebView2Missing,
    WebView2Overridden,
    WebView2Unsupported,
    RenderFailed,
    Timeout,
    SourceBusy,
    Cancelled,
    Protocol,
    InvalidPdf,
    Io(io::Error),
    Cache(epub_cache::CacheError),
}

impl From<io::Error> for EpubConvertError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<epub_cache::CacheError> for EpubConvertError {
    fn from(error: epub_cache::CacheError) -> Self {
        Self::Cache(error)
    }
}

pub type ConvertResult = Result<PublishOutcome, EpubConvertError>;

#[derive(Clone)]
pub struct CancelToken {
    inner: Arc<CancelInner>,
}

struct CancelInner {
    cancelled: AtomicBool,
    #[cfg(windows)]
    event: std::os::windows::io::OwnedHandle,
}

impl CancelToken {
    pub fn new() -> io::Result<Self> {
        #[cfg(windows)]
        let event = {
            use std::os::windows::io::FromRawHandle as _;
            use windows::Win32::System::Threading::CreateEventW;
            let handle = unsafe { CreateEventW(None, true, false, None) }
                .map_err(|error| io::Error::other(error.to_string()))?;
            unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle.0) }
        };
        Ok(Self {
            inner: Arc::new(CancelInner {
                cancelled: AtomicBool::new(false),
                #[cfg(windows)]
                event,
            }),
        })
    }

    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle as _;
            use windows::Win32::Foundation::HANDLE;
            use windows::Win32::System::Threading::SetEvent;
            let _ = unsafe { SetEvent(HANDLE(self.inner.event.as_raw_handle())) };
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
pub struct WorkerSpec {
    pub executable: PathBuf,
    pub input: PathBuf,
    pub output: PathBuf,
    pub work_dir: PathBuf,
    pub user_data_dir: PathBuf,
    pub timeout_secs: u32,
    pub environment: Vec<(OsString, OsString)>,
    #[cfg(test)]
    pub native_test_args: Option<Vec<OsString>>,
}

fn worker_environment() -> Vec<(OsString, OsString)> {
    filter_worker_environment(std::env::vars_os())
}

fn filter_worker_environment(
    pairs: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    pairs
        .into_iter()
        .filter(|(key, _)| {
            !key.to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("WEBVIEW2_")
        })
        .collect()
}

pub trait WorkerChild: Send {
    fn take_stdout(&mut self) -> io::Result<Box<dyn Read + Send>>;
    fn take_stderr(&mut self) -> io::Result<Box<dyn Read + Send>>;
    fn wait(&mut self, cancel: &CancelToken, timeout: Duration) -> Result<i32, EpubConvertError>;
}

pub trait WorkerSpawner: Send + Sync {
    fn spawn(&self, spec: &WorkerSpec) -> Result<Box<dyn WorkerChild>, EpubConvertError>;
}

pub struct NativeSpawner;

impl WorkerSpawner for NativeSpawner {
    fn spawn(&self, spec: &WorkerSpec) -> Result<Box<dyn WorkerChild>, EpubConvertError> {
        #[cfg(windows)]
        {
            windows_spawn(spec)
        }
        #[cfg(not(windows))]
        {
            let _ = spec;
            Err(EpubConvertError::Io(io::Error::new(
                io::ErrorKind::Unsupported,
                "Windows only",
            )))
        }
    }
}

pub fn worker_executable() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("MIV_EPUB_PDF_WORKER") {
        return Ok(PathBuf::from(path));
    }
    Ok(std::env::current_exe()?.with_file_name("mimageviewer-epub-pdf.exe"))
}

pub fn convert(
    gate: &epub_cache::AliveGuard,
    source: &Path,
    cancel: &CancelToken,
    progress: &mpsc::Sender<ConvertProgress>,
    timeout_secs: u32,
) -> ConvertResult {
    let data_dir = crate::data_dir::get();
    convert_at(
        ConvertContext {
            gate,
            data_dir: &data_dir,
            temp_root: &crate::materializer::epub_temp_root(),
        },
        source,
        cancel,
        progress,
        timeout_secs,
        &NativeSpawner,
    )
}

pub struct ConvertContext<'a> {
    pub gate: &'a epub_cache::AliveGuard,
    pub data_dir: &'a Path,
    pub temp_root: &'a Path,
}

/// Call from a worker thread only. `spawner` allows deterministic fake workers.
pub fn convert_at<S: WorkerSpawner>(
    context: ConvertContext<'_>,
    source: &Path,
    cancel: &CancelToken,
    progress: &mpsc::Sender<ConvertProgress>,
    timeout_secs: u32,
    spawner: &S,
) -> ConvertResult {
    if !context.gate.authorizes(context.data_dir) {
        return Err(EpubConvertError::Cache(
            epub_cache::CacheError::InvalidState("gate for a different data directory"),
        ));
    }
    let mut db = EpubCache::open_at(context.data_dir)?;
    if timeout_secs == 0 {
        return Err(EpubConvertError::Invalid);
    }
    let id = db.reserve_generation_id()?;
    if cancel.is_cancelled() {
        return Err(EpubConvertError::Cancelled);
    }
    let temp = TempFolder::create(context.temp_root)?;
    let source_copy = temp.path.join("source.epub");
    let (state, head_hash, full_hash) = copy_source(source, &source_copy, cancel)?;
    if cancel.is_cancelled() {
        return Err(EpubConvertError::Cancelled);
    }
    let final_path = epub_cache::generation_file(context.data_dir, source, id);
    fs::create_dir_all(final_path.parent().ok_or(EpubConvertError::Protocol)?)?;
    let part = final_path.with_extension("pdf.part");
    let _part_cleanup = PartCleanup(part.clone());
    let spec = WorkerSpec {
        executable: worker_executable()?,
        input: source_copy,
        output: part.clone(),
        work_dir: temp.path.join("work"),
        user_data_dir: temp.path.join("ud"),
        timeout_secs,
        environment: worker_environment(),
        #[cfg(test)]
        native_test_args: None,
    };
    let mut child = spawner.spawn(&spec)?;
    let stdout = child.take_stdout()?;
    let stderr = child.take_stderr()?;
    let progress_tx = progress.clone();
    let stdout_thread = std::thread::spawn(move || read_events(stdout, &progress_tx));
    let stderr_thread = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            match line {
                Ok(line) => crate::logger::log(format!("epub_worker: {line}")),
                Err(_) => break,
            }
        }
    });
    let waited = child.wait(cancel, Duration::from_secs(timeout_secs as u64 + 10));
    drop(child); // Job handle closes on every exit path.
    let parsed = stdout_thread
        .join()
        .map_err(|_| EpubConvertError::Protocol)?;
    let _ = stderr_thread.join();
    let code = waited?;
    if cancel.is_cancelled() {
        return Err(EpubConvertError::Cancelled);
    }
    let result = parsed?;
    let (pages, direction, profile) = validate_result(code, result)?;
    verify_converted_pdf_stage2a(&part, pages)?;
    if cancel.is_cancelled() {
        return Err(EpubConvertError::Cancelled);
    }
    promote_part(&part, &final_path)?;
    let candidate = GenerationRow {
        generation_id: id,
        src_path_key: epub_cache::src_key(source),
        src_path: source.to_owned(),
        src_state: state,
        src_sha256: full_hash,
        src_head_hash: head_hash,
        pdf_file: final_path.clone(),
        pdf_size: fs::metadata(&final_path)?.len(),
        page_count: pages as u32,
        direction,
        profile,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
    };
    // Open before BEGIN IMMEDIATE. The guard denies writes until publish commits.
    let guard = WriteDenyingSource::open(source).map_err(map_source_open)?;
    db.publish(&candidate, &guard).map_err(Into::into)
}

fn copy_source(
    source: &Path,
    destination: &Path,
    cancel: &CancelToken,
) -> Result<(epub_cache::SourceState, String, String), EpubConvertError> {
    let mut source = WriteDenyingSource::open(source).map_err(map_source_open)?;
    let state = source.state()?;
    let mut copy = File::create(destination)?;
    let mut head_prefix = Vec::with_capacity(64 * 1024);
    let mut reader = CopyReader {
        source: source.file(),
        output: &mut copy,
        head_prefix: &mut head_prefix,
        cancel,
    };
    let full = match crate::content_identity::stage2_full_hash(&mut reader) {
        Ok(hash) => hash,
        Err(_) if cancel.is_cancelled() => return Err(EpubConvertError::Cancelled),
        Err(error) => return Err(error.into()),
    };
    let head =
        crate::content_identity::stage1_head_hash(&mut Cursor::new(head_prefix), state.size)?;
    copy.flush()?;
    if cancel.is_cancelled() {
        return Err(EpubConvertError::Cancelled);
    }
    if fs::metadata(destination)?.len() != state.size || source.state()? != state {
        return Err(EpubConvertError::SourceBusy);
    }
    Ok((state, head, full))
}

struct CopyReader<'a> {
    source: &'a mut File,
    output: &'a mut File,
    head_prefix: &'a mut Vec<u8>,
    cancel: &'a CancelToken,
}
impl Read for CopyReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let read = self.source.read(buffer)?;
        self.output.write_all(&buffer[..read])?;
        let remaining = (64 * 1024usize).saturating_sub(self.head_prefix.len());
        self.head_prefix
            .extend_from_slice(&buffer[..read.min(remaining)]);
        Ok(read)
    }
}

fn map_source_open(error: io::Error) -> EpubConvertError {
    // Windows ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION.
    if matches!(error.raw_os_error(), Some(32 | 33)) {
        EpubConvertError::SourceBusy
    } else {
        EpubConvertError::Io(error)
    }
}

fn promote_part(part: &Path, final_path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        use windows::Win32::Storage::FileSystem::MoveFileW;
        use windows::core::PCWSTR;
        let from: Vec<u16> = part.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = final_path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        // MoveFileW fails when the destination exists; no replacement flag is possible.
        unsafe { MoveFileW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr())) }
            .map_err(|error| io::Error::other(error.to_string()))
    }
    #[cfg(not(windows))]
    {
        fs::hard_link(part, final_path)?;
        fs::remove_file(part)
    }
}

struct TempFolder {
    path: PathBuf,
}
impl TempFolder {
    fn create(root: &Path) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        fs::create_dir_all(root)?;
        crate::materializer::validate_real_directory(root, "EPUB temp root")
            .map_err(io::Error::other)?;
        for _ in 0..100 {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!("epub-{}-{n}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "EPUB temp name exhaustion",
        ))
    }
}
impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = crate::materializer::remove_tree_without_following_links(&self.path);
    }
}
struct PartCleanup(PathBuf);
impl Drop for PartCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn read_events(
    stdout: Box<dyn Read + Send>,
    progress: &mpsc::Sender<ConvertProgress>,
) -> Result<WorkerEvent, EpubConvertError> {
    let mut final_result = None;
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        let event: WorkerEvent =
            serde_json::from_str(&line).map_err(|_| EpubConvertError::Protocol)?;
        match event {
            WorkerEvent::Progress { phase, done, total } if final_result.is_none() => {
                let _ = progress.send(ConvertProgress { phase, done, total });
            }
            WorkerEvent::Result { .. } if final_result.is_none() => final_result = Some(event),
            _ => return Err(EpubConvertError::Protocol),
        }
    }
    final_result.ok_or(EpubConvertError::Protocol)
}

fn validate_result(
    code: i32,
    result: WorkerEvent,
) -> Result<(usize, String, String), EpubConvertError> {
    let WorkerEvent::Result {
        status,
        exit_code,
        page_count,
        direction,
        layout: _,
        profile,
        blocked_requests: _,
        message: _,
    } = result
    else {
        return Err(EpubConvertError::Protocol);
    };
    let expected = match status {
        Status::Success => 0,
        Status::Drm => 2,
        Status::Invalid => 3,
        Status::Webview2Missing => 4,
        Status::RenderFailed => 5,
        Status::Timeout => 6,
        Status::Webview2Overridden => 7,
        Status::Webview2Unsupported => 8,
    };
    if code != exit_code || code != expected {
        return Err(EpubConvertError::Protocol);
    }
    match status {
        Status::Success => Ok((page_count, direction, profile)),
        Status::Drm => Err(EpubConvertError::Drm),
        Status::Invalid => Err(EpubConvertError::Invalid),
        Status::Webview2Missing => Err(EpubConvertError::WebView2Missing),
        Status::Webview2Overridden => Err(EpubConvertError::WebView2Overridden),
        Status::Webview2Unsupported => Err(EpubConvertError::WebView2Unsupported),
        Status::RenderFailed => Err(EpubConvertError::RenderFailed),
        Status::Timeout => Err(EpubConvertError::Timeout),
    }
}

/// S2b replaces this minimal byte check with PDFium `verify_converted_pdf`.
fn verify_converted_pdf_stage2a(path: &Path, pages: usize) -> Result<(), EpubConvertError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(EpubConvertError::InvalidPdf);
        }
        Err(error) => return Err(error.into()),
    };
    if pages == 0 || pages > u32::MAX as usize || !metadata.is_file() || metadata.len() == 0 {
        return Err(EpubConvertError::InvalidPdf);
    }
    let mut file = File::open(path)?;
    let mut header = [0; 5];
    file.read_exact(&mut header).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            EpubConvertError::InvalidPdf
        } else {
            EpubConvertError::Io(error)
        }
    })?;
    if &header != b"%PDF-" {
        return Err(EpubConvertError::InvalidPdf);
    }
    Ok(())
}

#[cfg(windows)]
mod windows_process {
    use super::*;
    use std::ffi::OsStr;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use windows::Win32::Foundation::{
        HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
        GetExitCodeProcess, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOW,
        WaitForMultipleObjects,
    };
    use windows::core::{PCWSTR, PWSTR};

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }
    fn quote(value: &OsStr) -> String {
        let value = value.to_string_lossy();
        let mut out = String::from("\"");
        let mut slashes = 0;
        for c in value.chars() {
            if c == '\\' {
                slashes += 1;
            } else if c == '"' {
                out.push_str(&"\\".repeat(slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            } else {
                out.push_str(&"\\".repeat(slashes));
                slashes = 0;
                out.push(c);
            }
        }
        out.push_str(&"\\".repeat(slashes * 2));
        out.push('"');
        out
    }
    fn child_environment(environment: &[(OsString, OsString)]) -> Vec<u16> {
        let mut pairs: Vec<_> = environment.to_vec();
        pairs.sort_by_key(|(key, _)| key.to_string_lossy().to_ascii_uppercase());
        let mut block = Vec::new();
        for (key, value) in pairs {
            block.extend(key.encode_wide());
            block.push(b'=' as u16);
            block.extend(value.encode_wide());
            block.push(0);
        }
        if block.is_empty() {
            block.push(0);
        }
        block.push(0);
        block
    }
    fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
        use windows::Win32::Security::SECURITY_ATTRIBUTES;
        let attrs = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            bInheritHandle: true.into(),
            ..Default::default()
        };
        let mut read = HANDLE::default();
        let mut write = HANDLE::default();
        unsafe { CreatePipe(&mut read, &mut write, Some(&attrs), 0) }
            .map_err(|error| io::Error::other(error.to_string()))?;
        let read = unsafe { OwnedHandle::from_raw_handle(read.0) };
        let write = unsafe { OwnedHandle::from_raw_handle(write.0) };
        unsafe {
            SetHandleInformation(
                HANDLE(read.as_raw_handle()),
                HANDLE_FLAG_INHERIT.0,
                Default::default(),
            )
        }
        .map_err(|error| io::Error::other(error.to_string()))?;
        Ok((read, write))
    }
    pub(super) struct NativeChild {
        job: OwnedHandle,
        process: OwnedHandle,
        stdout: Option<OwnedHandle>,
        stderr: Option<OwnedHandle>,
    }
    impl Drop for NativeChild {
        fn drop(&mut self) {
            let _ = unsafe { TerminateJobObject(HANDLE(self.job.as_raw_handle()), 1) };
        }
    }
    impl WorkerChild for NativeChild {
        fn take_stdout(&mut self) -> io::Result<Box<dyn Read + Send>> {
            Ok(Box::new(File::from(
                self.stdout
                    .take()
                    .ok_or_else(|| io::Error::other("stdout taken"))?,
            )))
        }
        fn take_stderr(&mut self) -> io::Result<Box<dyn Read + Send>> {
            Ok(Box::new(File::from(
                self.stderr
                    .take()
                    .ok_or_else(|| io::Error::other("stderr taken"))?,
            )))
        }
        fn wait(
            &mut self,
            cancel: &CancelToken,
            timeout: Duration,
        ) -> Result<i32, EpubConvertError> {
            let handles = [
                HANDLE(self.process.as_raw_handle()),
                HANDLE(cancel.inner.event.as_raw_handle()),
            ];
            let waited = unsafe {
                WaitForMultipleObjects(
                    &handles,
                    false,
                    timeout.as_millis().min(u32::MAX as u128) as u32,
                )
            };
            if waited == WAIT_OBJECT_0 {
                let mut code = 0;
                unsafe { GetExitCodeProcess(handles[0], &mut code) }
                    .map_err(|error| EpubConvertError::Io(io::Error::other(error.to_string())))?;
                return Ok(code as i32);
            }
            let _ = unsafe { TerminateJobObject(HANDLE(self.job.as_raw_handle()), 1) };
            if waited.0 == WAIT_OBJECT_0.0 + 1 {
                Err(EpubConvertError::Cancelled)
            } else if waited == WAIT_TIMEOUT {
                Err(EpubConvertError::Timeout)
            } else {
                Err(EpubConvertError::Io(io::Error::other(
                    "WaitForMultipleObjects failed",
                )))
            }
        }
    }
    pub(super) fn spawn(spec: &WorkerSpec) -> Result<Box<dyn WorkerChild>, EpubConvertError> {
        let job_raw = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|error| io::Error::other(error.to_string()))?;
        let job = unsafe { OwnedHandle::from_raw_handle(job_raw.0) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            SetInformationJobObject(
                HANDLE(job.as_raw_handle()),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        }
        .map_err(|error| io::Error::other(error.to_string()))?;
        let (stdout_read, stdout_write) = pipe()?;
        let (stderr_read, stderr_write) = pipe()?;
        let stdin = File::open("NUL")?;
        unsafe {
            SetHandleInformation(
                HANDLE(stdin.as_raw_handle()),
                HANDLE_FLAG_INHERIT.0,
                HANDLE_FLAG_INHERIT,
            )
        }
        .map_err(|error| io::Error::other(error.to_string()))?;
        let timeout = spec.timeout_secs.to_string();
        let args: Vec<OsString> = vec![
            spec.executable.as_os_str().to_os_string(),
            OsString::from("convert"),
            spec.input.as_os_str().to_os_string(),
            spec.output.as_os_str().to_os_string(),
            OsString::from("--work-dir"),
            spec.work_dir.as_os_str().to_os_string(),
            OsString::from("--user-data-dir"),
            spec.user_data_dir.as_os_str().to_os_string(),
            OsString::from("--progress-json"),
            OsString::from("--timeout-secs"),
            OsString::from(timeout),
        ];
        #[cfg(test)]
        let args: Vec<OsString> = if let Some(test_args) = &spec.native_test_args {
            std::iter::once(args[0].clone())
                .chain(test_args.iter().cloned())
                .collect()
        } else {
            args
        };
        let mut cmd = wide(OsStr::new(
            &args.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" "),
        ));
        let exe = wide(spec.executable.as_os_str());
        let env = child_environment(&spec.environment);
        let startup = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            hStdInput: HANDLE(stdin.as_raw_handle()),
            hStdOutput: HANDLE(stdout_write.as_raw_handle()),
            hStdError: HANDLE(stderr_write.as_raw_handle()),
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        unsafe {
            CreateProcessW(
                PCWSTR(exe.as_ptr()),
                Some(PWSTR(cmd.as_mut_ptr())),
                None,
                None,
                true,
                CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
                Some(env.as_ptr().cast()),
                PCWSTR::null(),
                &startup,
                &mut pi,
            )
        }
        .map_err(|error| io::Error::other(error.to_string()))?;
        let process = unsafe { OwnedHandle::from_raw_handle(pi.hProcess.0) };
        let thread = unsafe { OwnedHandle::from_raw_handle(pi.hThread.0) };
        if let Err(error) = unsafe {
            AssignProcessToJobObject(HANDLE(job.as_raw_handle()), HANDLE(process.as_raw_handle()))
        } {
            use windows::Win32::System::Threading::TerminateProcess;
            let _ = unsafe { TerminateProcess(HANDLE(process.as_raw_handle()), 1) };
            return Err(EpubConvertError::Io(io::Error::other(error.to_string())));
        }
        if unsafe { ResumeThread(HANDLE(thread.as_raw_handle())) } == u32::MAX {
            return Err(EpubConvertError::Io(io::Error::other(
                "ResumeThread failed",
            )));
        }
        drop(thread);
        drop(stdout_write);
        drop(stderr_write);
        Ok(Box::new(NativeChild {
            job,
            process,
            stdout: Some(stdout_read),
            stderr: Some(stderr_read),
        }))
    }
}

#[cfg(windows)]
fn windows_spawn(spec: &WorkerSpec) -> Result<Box<dyn WorkerChild>, EpubConvertError> {
    windows_process::spawn(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::AtomicBool;

    #[derive(Clone, Copy)]
    enum FakeMode {
        Success,
        Failure(i32),
        NoResult,
        CrashAfterOutput,
        Cancel,
        Stale,
    }
    struct FakeSpawner {
        mode: FakeMode,
        source: PathBuf,
        killed: Arc<AtomicBool>,
    }
    struct FakeChild {
        stdout: Option<Vec<u8>>,
        code: i32,
        mode: FakeMode,
        killed: Arc<AtomicBool>,
    }
    impl WorkerSpawner for FakeSpawner {
        fn spawn(&self, spec: &WorkerSpec) -> Result<Box<dyn WorkerChild>, EpubConvertError> {
            assert_eq!(spec.input.file_name().unwrap(), "source.epub");
            assert!(spec.work_dir.starts_with(spec.input.parent().unwrap()));
            assert!(spec.user_data_dir.starts_with(spec.input.parent().unwrap()));
            assert!(spec.environment.iter().all(|(name, _)| {
                !name
                    .to_string_lossy()
                    .to_ascii_uppercase()
                    .starts_with("WEBVIEW2_")
            }));
            if !matches!(self.mode, FakeMode::Failure(_) | FakeMode::NoResult) {
                fs::write(&spec.output, b"%PDF-1.4\nfake").unwrap();
            }
            if matches!(self.mode, FakeMode::Stale) {
                fs::write(&self.source, b"changed source").unwrap();
            }
            let code = match self.mode {
                FakeMode::Failure(code) => code,
                FakeMode::CrashAfterOutput => 5,
                _ => 0,
            };
            let status = match code {
                0 => "success",
                2 => "drm",
                3 => "invalid",
                4 => "webview2_missing",
                5 => "render_failed",
                6 => "timeout",
                7 => "webview2_overridden",
                8 => "webview2_unsupported",
                _ => unreachable!(),
            };
            let stdout = if matches!(self.mode, FakeMode::NoResult | FakeMode::CrashAfterOutput) {
                Vec::new()
            } else {
                format!("{{\"event\":\"progress\",\"phase\":\"print\",\"done\":1,\"total\":0}}\n{{\"event\":\"result\",\"status\":\"{status}\",\"exit_code\":{code},\"page_count\":1,\"direction\":\"rtl\",\"layout\":\"reflow\",\"profile\":\"reflow-v1\",\"blocked_requests\":0,\"message\":\"\"}}\n").into_bytes()
            };
            Ok(Box::new(FakeChild {
                stdout: Some(stdout),
                code,
                mode: self.mode,
                killed: Arc::clone(&self.killed),
            }))
        }
    }
    impl WorkerChild for FakeChild {
        fn take_stdout(&mut self) -> io::Result<Box<dyn Read + Send>> {
            Ok(Box::new(Cursor::new(self.stdout.take().unwrap())))
        }
        fn take_stderr(&mut self) -> io::Result<Box<dyn Read + Send>> {
            Ok(Box::new(Cursor::new(Vec::<u8>::new())))
        }
        fn wait(
            &mut self,
            cancel: &CancelToken,
            _timeout: Duration,
        ) -> Result<i32, EpubConvertError> {
            if matches!(self.mode, FakeMode::Cancel) {
                cancel.cancel();
            }
            if cancel.is_cancelled() {
                self.killed.store(true, Ordering::Release);
                Err(EpubConvertError::Cancelled)
            } else {
                Ok(self.code)
            }
        }
    }

    fn run(mode: FakeMode) -> (ConvertResult, tempfile::TempDir, Arc<AtomicBool>, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("book.epub");
        fs::write(&source, b"source EPUB bytes").unwrap();
        let temp_root = tmp.path().join("temp");
        let killed = Arc::new(AtomicBool::new(false));
        let fake = FakeSpawner {
            mode,
            source: source.clone(),
            killed: Arc::clone(&killed),
        };
        let cancel = CancelToken::new().unwrap();
        let (tx, _rx) = mpsc::channel();
        let gate = epub_cache::startup_gate(tmp.path());
        let epub_cache::GateOutcome::Enabled { guard, .. } = gate else {
            panic!("test gate disabled");
        };
        let result = convert_at(
            ConvertContext {
                gate: &guard,
                data_dir: tmp.path(),
                temp_root: &temp_root,
            },
            &source,
            &cancel,
            &tx,
            3,
            &fake,
        );
        (result, tmp, killed, temp_root)
    }

    #[test]
    fn epub_convert_fake_success_publishes_and_cleans_temp() {
        let (result, tmp, _, temp_root) = run(FakeMode::Success);
        assert!(matches!(result, Ok(PublishOutcome::Published)));
        let db = EpubCache::open_at(tmp.path()).unwrap();
        let row = db
            .current_generation(&epub_cache::src_key(&tmp.path().join("book.epub")))
            .unwrap()
            .unwrap();
        assert!(row.pdf_file.exists());
        assert_eq!(row.page_count, 1);
        assert!(fs::read_dir(temp_root).unwrap().next().is_none());
    }

    #[test]
    fn epub_convert_fake_maps_every_worker_error() {
        for (code, kind) in [
            (2, "drm"),
            (3, "invalid"),
            (4, "missing"),
            (5, "render"),
            (6, "timeout"),
            (7, "overridden"),
            (8, "unsupported"),
        ] {
            let (result, _tmp, _, temp_root) = run(FakeMode::Failure(code));
            let matched = matches!(
                (&result, kind),
                (Err(EpubConvertError::Drm), "drm")
                    | (Err(EpubConvertError::Invalid), "invalid")
                    | (Err(EpubConvertError::WebView2Missing), "missing")
                    | (Err(EpubConvertError::RenderFailed), "render")
                    | (Err(EpubConvertError::Timeout), "timeout")
                    | (Err(EpubConvertError::WebView2Overridden), "overridden")
                    | (Err(EpubConvertError::WebView2Unsupported), "unsupported")
            );
            assert!(matched, "code {code}: {result:?}");
            assert!(fs::read_dir(temp_root).unwrap().next().is_none());
        }
    }

    #[test]
    fn epub_convert_fake_missing_result_or_crash_after_output_is_protocol_error() {
        for mode in [FakeMode::NoResult, FakeMode::CrashAfterOutput] {
            let (result, _, _, _) = run(mode);
            assert!(
                matches!(result, Err(EpubConvertError::Protocol)),
                "{result:?}"
            );
        }
    }

    #[test]
    fn epub_convert_rejects_result_exit_or_status_mismatch() {
        let result = WorkerEvent::Result {
            status: Status::Success,
            exit_code: 0,
            page_count: 1,
            direction: "rtl".into(),
            layout: "reflow".into(),
            profile: "reflow-v1".into(),
            blocked_requests: 0,
            message: String::new(),
        };
        assert!(matches!(
            validate_result(5, result),
            Err(EpubConvertError::Protocol)
        ));
        let result = WorkerEvent::Result {
            status: Status::Drm,
            exit_code: 0,
            page_count: 0,
            direction: "default".into(),
            layout: "unknown".into(),
            profile: "reflow-v1".into(),
            blocked_requests: 0,
            message: String::new(),
        };
        assert!(matches!(
            validate_result(0, result),
            Err(EpubConvertError::Protocol)
        ));
    }

    #[test]
    fn epub_convert_fake_cancel_kills_worker_and_cleans_part_and_temp() {
        let (result, tmp, killed, temp_root) = run(FakeMode::Cancel);
        assert!(matches!(result, Err(EpubConvertError::Cancelled)));
        assert!(killed.load(Ordering::Acquire));
        assert!(fs::read_dir(temp_root).unwrap().next().is_none());
        let db = EpubCache::open_at(tmp.path()).unwrap();
        assert!(db.list_current().unwrap().is_empty());
        assert!(!tmp.path().join("epub_cache").join("book.pdf.part").exists());
    }

    #[test]
    fn epub_convert_fake_stale_after_copy_is_retired() {
        let (result, tmp, _, _) = run(FakeMode::Stale);
        assert!(matches!(result, Ok(PublishOutcome::Stale)), "{result:?}");
        let db = EpubCache::open_at(tmp.path()).unwrap();
        assert!(
            db.current_generation(&epub_cache::src_key(&tmp.path().join("book.epub")))
                .unwrap()
                .is_none()
        );
        assert_eq!(db.conn_for_tests_retired_count(), 1);
    }

    #[test]
    fn epub_convert_filters_all_webview2_environment_names() {
        let filtered = filter_worker_environment([
            (
                OsString::from("WEBVIEW2_USER_DATA_FOLDER"),
                OsString::from("bad"),
            ),
            (OsString::from("WebView2_Extra"), OsString::from("bad")),
            (OsString::from("PATH"), OsString::from("safe")),
        ]);
        assert_eq!(
            filtered,
            vec![(OsString::from("PATH"), OsString::from("safe"))]
        );
    }

    #[cfg(windows)]
    struct ScriptSpawner {
        script: PathBuf,
        mode: &'static str,
        started: std::sync::Mutex<Option<mpsc::Sender<()>>>,
    }

    #[cfg(windows)]
    struct ScriptChild(std::process::Child);

    #[cfg(windows)]
    impl WorkerSpawner for ScriptSpawner {
        fn spawn(&self, spec: &WorkerSpec) -> Result<Box<dyn WorkerChild>, EpubConvertError> {
            use std::os::windows::process::CommandExt as _;
            let powershell = std::env::var_os("SystemRoot")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
                .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
            let mut command = std::process::Command::new(powershell);
            command
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ])
                .arg(&self.script)
                .arg(&spec.output)
                .arg(self.mode)
                .env_clear()
                .envs(spec.environment.clone())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            let child = command.spawn()?;
            if let Some(tx) = self.started.lock().unwrap().take() {
                let _ = tx.send(());
            }
            Ok(Box::new(ScriptChild(child)))
        }
    }

    #[cfg(windows)]
    impl WorkerChild for ScriptChild {
        fn take_stdout(&mut self) -> io::Result<Box<dyn Read + Send>> {
            Ok(Box::new(
                self.0
                    .stdout
                    .take()
                    .ok_or_else(|| io::Error::other("stdout taken"))?,
            ))
        }
        fn take_stderr(&mut self) -> io::Result<Box<dyn Read + Send>> {
            Ok(Box::new(
                self.0
                    .stderr
                    .take()
                    .ok_or_else(|| io::Error::other("stderr taken"))?,
            ))
        }
        fn wait(
            &mut self,
            cancel: &CancelToken,
            timeout: Duration,
        ) -> Result<i32, EpubConvertError> {
            use std::os::windows::io::AsRawHandle as _;
            use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
            use windows::Win32::System::Threading::WaitForMultipleObjects;
            let handles = [
                HANDLE(self.0.as_raw_handle()),
                HANDLE(cancel.inner.event.as_raw_handle()),
            ];
            let waited =
                unsafe { WaitForMultipleObjects(&handles, false, timeout.as_millis() as u32) };
            if waited == WAIT_OBJECT_0 {
                return Ok(self.0.wait()?.code().unwrap_or(-1));
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
            if waited.0 == WAIT_OBJECT_0.0 + 1 {
                Err(EpubConvertError::Cancelled)
            } else if waited == WAIT_TIMEOUT {
                Err(EpubConvertError::Timeout)
            } else {
                Err(EpubConvertError::Io(io::Error::other("fake wait failed")))
            }
        }
    }

    #[cfg(windows)]
    impl Drop for ScriptChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(windows)]
    fn script_spawner(
        root: &Path,
        mode: &'static str,
        started: Option<mpsc::Sender<()>>,
    ) -> ScriptSpawner {
        let script = root.join("fake-worker.ps1");
        fs::write(&script, r#"param([string]$outFile,[string]$mode)
if (Get-ChildItem Env:WEBVIEW2_* -ErrorAction SilentlyContinue) { exit 9 }
[System.IO.File]::WriteAllBytes($outFile, [System.Text.Encoding]::ASCII.GetBytes('%PDF-1.4 fake'))
Write-Output '{"event":"progress","phase":"print","done":1,"total":0}'
if ($mode -eq 'hang') { Start-Sleep -Seconds 30; exit 5 }
Write-Output '{"event":"result","status":"success","exit_code":0,"page_count":1,"direction":"rtl","layout":"reflow","profile":"reflow-v1","blocked_requests":0,"message":"env_clean"}'
exit 0
"#).unwrap();
        ScriptSpawner {
            script,
            mode,
            started: std::sync::Mutex::new(started),
        }
    }

    #[cfg(windows)]
    #[test]
    fn epub_convert_script_child_success_and_clean_environment() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("book.epub");
        fs::write(&src, b"epub bytes").unwrap();
        let spawner = script_spawner(tmp.path(), "success", None);
        let epub_cache::GateOutcome::Enabled { guard, .. } = epub_cache::startup_gate(tmp.path())
        else {
            panic!("gate disabled");
        };
        let (tx, rx) = mpsc::channel();
        let result = convert_at(
            ConvertContext {
                gate: &guard,
                data_dir: tmp.path(),
                temp_root: &tmp.path().join("temp"),
            },
            &src,
            &CancelToken::new().unwrap(),
            &tx,
            5,
            &spawner,
        );
        assert!(
            matches!(result, Ok(PublishOutcome::Published)),
            "{result:?}"
        );
        assert_eq!(rx.try_recv().unwrap().phase, Phase::Print);
    }

    #[cfg(windows)]
    #[test]
    fn epub_convert_script_child_hang_cancel_kills_and_cleans() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("book.epub");
        fs::write(&src, b"epub bytes").unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let spawner = script_spawner(tmp.path(), "hang", Some(started_tx));
        let epub_cache::GateOutcome::Enabled { guard, .. } = epub_cache::startup_gate(tmp.path())
        else {
            panic!("gate disabled");
        };
        let cancel = CancelToken::new().unwrap();
        let cancel_on_start = cancel.clone();
        let signal = std::thread::spawn(move || {
            started_rx.recv().unwrap();
            cancel_on_start.cancel();
        });
        let (tx, _) = mpsc::channel();
        let result = convert_at(
            ConvertContext {
                gate: &guard,
                data_dir: tmp.path(),
                temp_root: &tmp.path().join("temp"),
            },
            &src,
            &cancel,
            &tx,
            5,
            &spawner,
        );
        signal.join().unwrap();
        assert!(
            matches!(result, Err(EpubConvertError::Cancelled)),
            "{result:?}"
        );
        assert!(
            fs::read_dir(tmp.path().join("temp"))
                .unwrap()
                .next()
                .is_none()
        );
        let db = EpubCache::open_at(tmp.path()).unwrap();
        assert!(db.list_current().unwrap().is_empty());
    }

    #[cfg(windows)]
    struct NativeScriptSpawner {
        script: PathBuf,
        mode: &'static str,
    }

    #[cfg(windows)]
    impl WorkerSpawner for NativeScriptSpawner {
        fn spawn(&self, spec: &WorkerSpec) -> Result<Box<dyn WorkerChild>, EpubConvertError> {
            let mut spec = spec.clone();
            spec.executable = std::env::var_os("SystemRoot")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
                .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
            spec.native_test_args = Some(vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                self.script.as_os_str().to_os_string(),
                spec.output.as_os_str().to_os_string(),
                self.mode.into(),
            ]);
            NativeSpawner.spawn(&spec)
        }
    }

    #[cfg(windows)]
    #[test]
    fn epub_convert_native_spawner_fake_child_success() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("book.epub");
        fs::write(&src, b"epub bytes").unwrap();
        let script = script_spawner(tmp.path(), "success", None).script;
        let spawner = NativeScriptSpawner {
            script,
            mode: "success",
        };
        let epub_cache::GateOutcome::Enabled { guard, .. } = epub_cache::startup_gate(tmp.path())
        else {
            panic!("gate disabled");
        };
        let (tx, rx) = mpsc::channel();
        let result = convert_at(
            ConvertContext {
                gate: &guard,
                data_dir: tmp.path(),
                temp_root: &tmp.path().join("temp"),
            },
            &src,
            &CancelToken::new().unwrap(),
            &tx,
            5,
            &spawner,
        );
        assert!(
            matches!(result, Ok(PublishOutcome::Published)),
            "{result:?}"
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap().phase,
            Phase::Print
        );
    }

    #[cfg(windows)]
    #[test]
    fn epub_convert_native_spawner_hang_after_progress_cancel_kills_job() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_owned();
        let src = root.join("book.epub");
        fs::write(&src, b"epub bytes").unwrap();
        let script = script_spawner(&root, "hang", None).script;
        let spawner = NativeScriptSpawner {
            script,
            mode: "hang",
        };
        let epub_cache::GateOutcome::Enabled { guard, .. } = epub_cache::startup_gate(&root) else {
            panic!("gate disabled");
        };
        let cancel = CancelToken::new().unwrap();
        let running_cancel = cancel.clone();
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            convert_at(
                ConvertContext {
                    gate: &guard,
                    data_dir: &root,
                    temp_root: &root.join("temp"),
                },
                &src,
                &running_cancel,
                &tx,
                5,
                &spawner,
            )
        });
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap().phase,
            Phase::Print
        );
        cancel.cancel();
        let result = handle.join().unwrap();
        assert!(
            matches!(result, Err(EpubConvertError::Cancelled)),
            "{result:?}"
        );
        assert!(
            fs::read_dir(tmp.path().join("temp"))
                .unwrap()
                .next()
                .is_none()
        );
        let db = EpubCache::open_at(tmp.path()).unwrap();
        assert!(db.list_current().unwrap().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn epub_convert_source_busy_with_open_writer() {
        use std::os::windows::fs::OpenOptionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("book.epub");
        fs::write(&source, b"epub").unwrap();
        let _writer = fs::OpenOptions::new()
            .write(true)
            .share_mode(0x0000_0001)
            .open(&source)
            .unwrap();
        let result = copy_source(
            &source,
            &tmp.path().join("copy.epub"),
            &CancelToken::new().unwrap(),
        );
        assert!(matches!(result, Err(EpubConvertError::SourceBusy)));
    }
}
