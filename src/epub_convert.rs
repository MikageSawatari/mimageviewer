//! Background-only EPUB conversion. S2b will own scheduling and call this API.
//! A `ConvertedPdfVerifier` is mandatory: S2b must inject PDFium's
//! `pdf_loader::verify_converted_pdf` before wiring this runner to the app.
//! The S2a byte-level placeholder is available only to tests.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::epub_cache::{
    self, EpubCache, GenerationRow, PublishOutcome, ReservedOutput, SourceGuard, WriteDenyingSource,
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
        #[serde(default)]
        pages: Option<usize>,
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
    pub pages: Option<usize>,
}

#[derive(Debug)]
pub enum EpubConvertError {
    Drm,
    Invalid,
    WebView2Missing,
    WebView2Unsupported,
    RenderFailed,
    Timeout,
    SourceBusy,
    SourceChanged,
    ExistingPdf,
    Unavailable(String),
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

#[derive(Debug)]
pub struct SavedPdf {
    pub path: PathBuf,
    pub reused_cache: bool,
    pub user_data_errors: Vec<String>,
}

/// A finished PDF in the parent-owned work directory. The verifier accepts this
/// token so logical EPUB paths cannot bypass generation resolution.
pub struct SiblingOutput {
    part_path: PathBuf,
}

impl SiblingOutput {
    pub fn part_path(&self) -> &Path {
        &self.part_path
    }
}

/// Verifies the finished worker output before it is promoted or published.
pub trait ConvertedPdfVerifier: Send + Sync {
    fn verify(
        &self,
        output: &ReservedOutput,
        pages: usize,
        cancel: &CancelToken,
    ) -> Result<(), EpubConvertError>;

    fn verify_sibling(
        &self,
        output: &SiblingOutput,
        pages: usize,
        cancel: &CancelToken,
    ) -> Result<(), EpubConvertError>;
}

/// Production verifier. `verify` blocks on the PDF pool; run it only on a worker thread.
pub struct PdfiumConvertedPdfVerifier;

impl ConvertedPdfVerifier for PdfiumConvertedPdfVerifier {
    fn verify(
        &self,
        output: &ReservedOutput,
        pages: usize,
        cancel: &CancelToken,
    ) -> Result<(), EpubConvertError> {
        crate::pdf_loader::verify_converted_pdf_with_cancel(output, pages, Some(cancel.pool_flag()))
    }

    fn verify_sibling(
        &self,
        output: &SiblingOutput,
        pages: usize,
        cancel: &CancelToken,
    ) -> Result<(), EpubConvertError> {
        crate::pdf_loader::verify_sibling_pdf_with_cancel(output, pages, Some(cancel.pool_flag()))
    }
}

#[derive(Clone)]
pub struct CancelToken {
    inner: Arc<CancelInner>,
}

struct CancelInner {
    cancelled: Arc<AtomicBool>,
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
                cancelled: Arc::new(AtomicBool::new(false)),
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

    pub(crate) fn pool_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.inner.cancelled)
    }
}

#[derive(Clone)]
pub struct WorkerSpec {
    pub operation: WorkerOperation,
    pub executable: PathBuf,
    pub input: PathBuf,
    pub output: PathBuf,
    /// Original EPUB file stem; the copied worker input is always `source.epub`.
    pub source_stem: Option<OsString>,
    pub work_dir: PathBuf,
    pub user_data_dir: PathBuf,
    pub timeout_secs: u32,
    pub environment: Vec<(OsString, OsString)>,
    #[cfg(test)]
    pub native_test_args: Option<Vec<OsString>>,
}

#[derive(Clone, Copy)]
pub enum WorkerOperation {
    Inspect,
    Convert,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpubInspectSummary {
    pub layout: String,
    pub direction: String,
    pub spine_count: usize,
    pub sibling_pdf_exists: bool,
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

/// Inspect is a separate, cancellable worker process and must run off the UI thread.
pub fn inspect(
    source: &Path,
    cancel: &CancelToken,
    timeout_secs: u32,
) -> Result<EpubInspectSummary, EpubConvertError> {
    inspect_at(source, cancel, timeout_secs, &NativeSpawner)
}

pub fn inspect_at<S: WorkerSpawner>(
    source: &Path,
    cancel: &CancelToken,
    timeout_secs: u32,
    spawner: &S,
) -> Result<EpubInspectSummary, EpubConvertError> {
    let spec = WorkerSpec {
        operation: WorkerOperation::Inspect,
        executable: worker_executable()?,
        input: source.to_owned(),
        output: PathBuf::new(),
        source_stem: None,
        work_dir: PathBuf::new(),
        user_data_dir: PathBuf::new(),
        timeout_secs,
        environment: worker_environment(),
        #[cfg(test)]
        native_test_args: None,
    };
    let mut child = spawner.spawn(&spec)?;
    let stdout = child.take_stdout()?;
    let stderr = child.take_stderr()?;
    let output_thread = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        BufReader::new(stdout)
            .take(32 * 1024 * 1024)
            .read_to_end(&mut bytes)?;
        Ok::<_, io::Error>(bytes)
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = BufReader::new(stderr)
            .take(64 * 1024)
            .read_to_end(&mut bytes);
    });
    let waited = child.wait(cancel, Duration::from_secs(timeout_secs as u64));
    drop(child);
    let output = output_thread
        .join()
        .map_err(|_| EpubConvertError::Protocol)??;
    let _ = stderr_thread.join();
    let code = waited?;
    if cancel.is_cancelled() {
        return Err(EpubConvertError::Cancelled);
    }
    match code {
        2 => return Err(EpubConvertError::Drm),
        3 => return Err(EpubConvertError::Invalid),
        0 => {}
        _ => return Err(EpubConvertError::RenderFailed),
    }
    #[derive(Deserialize)]
    struct Inspected {
        rendition: InspectedRendition,
        direction: String,
        spine: Vec<InspectedSpine>,
        drm: String,
    }
    #[derive(Deserialize)]
    struct InspectedRendition {
        layout: Option<String>,
    }
    #[derive(Deserialize)]
    struct InspectedSpine {
        rendition: InspectedRendition,
    }
    let inspected: Inspected =
        serde_json::from_slice(&output).map_err(|_| EpubConvertError::Protocol)?;
    if inspected.drm != "none" {
        return Err(EpubConvertError::Drm);
    }
    let package_fixed = inspected.rendition.layout.as_deref() == Some("pre-paginated");
    let fixed_count = inspected
        .spine
        .iter()
        .filter(|item| {
            item.rendition.layout.as_deref() == Some("pre-paginated")
                || (package_fixed && item.rendition.layout.is_none())
        })
        .count();
    let layout = if fixed_count == 0 {
        "reflow"
    } else if fixed_count == inspected.spine.len() {
        "fixed"
    } else {
        "mixed"
    };
    Ok(EpubInspectSummary {
        layout: layout.to_owned(),
        direction: inspected.direction,
        spine_count: inspected.spine.len(),
        sibling_pdf_exists: source.with_extension("pdf").exists(),
    })
}

pub fn convert(
    gate: &epub_cache::AliveGuard,
    source: &Path,
    cancel: &CancelToken,
    progress: &mpsc::Sender<ConvertProgress>,
    timeout_secs: u32,
    verifier: &dyn ConvertedPdfVerifier,
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
        verifier,
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
    verifier: &dyn ConvertedPdfVerifier,
) -> ConvertResult {
    if !context.gate.authorizes(context.data_dir) {
        return Err(EpubConvertError::Cache(
            epub_cache::CacheError::InvalidState("gate for a different data directory"),
        ));
    }
    let _book_read = crate::pdf_loader::acquire_epub_book_lease(source);
    let mut db = EpubCache::open_at(context.data_dir)?;
    if timeout_secs == 0 {
        return Err(EpubConvertError::Invalid);
    }
    let reserved = db.reserve_output(source)?;
    if cancel.is_cancelled() {
        return Err(EpubConvertError::Cancelled);
    }
    let temp = TempFolder::create(context.temp_root)?;
    let source_copy = temp.path.join("source.epub");
    let (state, head_hash, full_hash) = copy_source(source, &source_copy, cancel)?;
    if cancel.is_cancelled() {
        return Err(EpubConvertError::Cancelled);
    }
    let final_path = reserved.final_path();
    fs::create_dir_all(final_path.parent().ok_or(EpubConvertError::Protocol)?)?;
    let part = reserved.part_path();
    let _part_cleanup = PartCleanup(part.to_owned());
    let (pages, direction, profile) = run_worker_to_part(
        source_copy,
        part,
        source.file_stem().map(OsStr::to_os_string),
        &temp,
        cancel,
        progress,
        timeout_secs,
        spawner,
    )?;
    verifier.verify(&reserved, pages, cancel)?;
    if cancel.is_cancelled() {
        return Err(EpubConvertError::Cancelled);
    }
    promote_part(part, final_path)?;
    let candidate = GenerationRow {
        generation_id: reserved.generation_id(),
        src_path_key: epub_cache::src_key(source),
        src_path: source.to_owned(),
        src_state: state,
        src_sha256: full_hash,
        src_head_hash: head_hash,
        pdf_file: final_path.to_owned(),
        pdf_size: fs::metadata(final_path)?.len(),
        page_count: pages as u32,
        direction,
        profile,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
        output_version: epub_cache::CONVERTER_OUTPUT_VERSION,
    };
    // Open before BEGIN IMMEDIATE. The guard denies writes until publish commits.
    let guard = WriteDenyingSource::open(source).map_err(map_source_open)?;
    db.publish(&candidate, &guard).map_err(Into::into)
}

fn run_worker_to_part<S: WorkerSpawner>(
    source_copy: PathBuf,
    part: &Path,
    source_stem: Option<OsString>,
    temp: &TempFolder,
    cancel: &CancelToken,
    progress: &mpsc::Sender<ConvertProgress>,
    timeout_secs: u32,
    spawner: &S,
) -> Result<(usize, String, String), EpubConvertError> {
    let spec = WorkerSpec {
        operation: WorkerOperation::Convert,
        executable: worker_executable()?,
        input: source_copy,
        output: part.to_owned(),
        source_stem,
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
    validate_result(code, result)
}

/// Explicit sibling-PDF save. Only the caller's worker thread may invoke this;
/// it performs filesystem, PDF-pool and SQLite operations.
pub fn save_sibling(
    gate: &epub_cache::AliveGuard,
    source: &Path,
    cancel: &CancelToken,
    progress: &mpsc::Sender<ConvertProgress>,
    timeout_secs: u32,
    verifier: &dyn ConvertedPdfVerifier,
) -> Result<SavedPdf, EpubConvertError> {
    let data_dir = crate::data_dir::get();
    save_sibling_at(
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
        verifier,
    )
}

pub fn save_sibling_at<S: WorkerSpawner>(
    context: ConvertContext<'_>,
    source: &Path,
    cancel: &CancelToken,
    progress: &mpsc::Sender<ConvertProgress>,
    timeout_secs: u32,
    spawner: &S,
    verifier: &dyn ConvertedPdfVerifier,
) -> Result<SavedPdf, EpubConvertError> {
    if !context.gate.authorizes(context.data_dir) {
        return Err(
            epub_cache::CacheError::InvalidState("gate for a different data directory").into(),
        );
    }
    if timeout_secs == 0
        || !source
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
    {
        return Err(EpubConvertError::Invalid);
    }
    let destination = source.with_extension("pdf");
    if destination.exists() {
        return Err(EpubConvertError::ExistingPdf);
    }
    if cancel.is_cancelled() {
        return Err(EpubConvertError::Cancelled);
    }
    let _book_read = crate::pdf_loader::acquire_epub_book_lease(source);
    let mut cache = EpubCache::open_at(context.data_dir)?;
    let temp = TempFolder::create(&context.data_dir.join("epub_sibling_work"))?;
    let output = SiblingOutput {
        part_path: temp.path.join("finished.pdf"),
    };
    let source_guard = WriteDenyingSource::open(source).map_err(map_source_open)?;
    let source_state = source_guard.state()?;
    let cached = cache
        .current_generation(&epub_cache::src_key(source))?
        .filter(|row| {
            row.src_state == source_state
                && row.page_count > 0
                && row.output_version == epub_cache::CONVERTER_OUTPUT_VERSION
        })
        .filter(|row| cache.validate_generation_pdf(row).is_ok())
        .filter(|row| {
            fs::metadata(&row.pdf_file)
                .is_ok_and(|meta| meta.is_file() && meta.len() == row.pdf_size)
        });
    let (pages, reused_cache, expected_state, guard) = if let Some(row) = cached {
        copy_cached_pdf(&row.pdf_file, output.part_path(), cancel)?;
        (row.page_count as usize, true, source_state, source_guard)
    } else {
        drop(source_guard);
        let source_copy = temp.path.join("source.epub");
        let (state, _, _) = copy_source(source, &source_copy, cancel)?;
        let (pages, _, _) = run_worker_to_part(
            source_copy,
            output.part_path(),
            source.file_stem().map(OsStr::to_os_string),
            &temp,
            cancel,
            progress,
            timeout_secs,
            spawner,
        )?;
        let guard = WriteDenyingSource::open(source).map_err(map_source_open)?;
        (pages, false, state, guard)
    };
    // Hold the finished work file write-denied before PDFium verifies its path. The
    // copy below reads from this same handle, so verification and publication see
    // one immutable byte sequence.
    let mut verified_work_file = open_work_pdf_for_verification(output.part_path())?;
    verifier.verify_sibling(&output, pages, cancel)?;
    if cancel.is_cancelled() {
        return Err(EpubConvertError::Cancelled);
    }
    if guard.state()? != expected_state {
        return Err(EpubConvertError::SourceChanged);
    }
    let mut reserved = cache.reserve_sibling_output(&destination)?;
    let publish = (|| {
        let mut destination_file = match reserved.create_file() {
            Ok(file) => file,
            Err(error) => {
                if let Err(cleanup_error) = cache.abandon_sibling_reservation(&reserved) {
                    crate::logger::log(format!(
                        "epub sibling reservation removal failed: {cleanup_error:?}"
                    ));
                }
                return Err(EpubConvertError::Io(error));
            }
        };
        if let Err(error) = cache.mark_sibling_created(&mut reserved, &destination_file) {
            if let Err(cleanup_error) =
                epub_cache::discard_unrecorded_sibling_file(&destination_file, reserved.temp_path())
            {
                crate::logger::log(format!(
                    "epub sibling unrecorded temp removal failed: {cleanup_error:?}"
                ));
            }
            if let Err(cleanup_error) = cache.abandon_sibling_reservation(&reserved) {
                crate::logger::log(format!(
                    "epub sibling reservation removal failed: {cleanup_error:?}"
                ));
            }
            return Err(error.into());
        }
        let result = (|| {
            copy_verified_pdf_to_sibling_file(
                &mut verified_work_file,
                &mut destination_file,
                cancel,
            )?;
            if cancel.is_cancelled() {
                return Err(EpubConvertError::Cancelled);
            }
            if guard.state()? != expected_state {
                return Err(EpubConvertError::SourceChanged);
            }
            epub_cache::publish_sibling_file(&destination_file, reserved.temp_path(), &destination)
                .map_err(|error| {
                    if destination.exists() {
                        EpubConvertError::ExistingPdf
                    } else {
                        EpubConvertError::Io(error)
                    }
                })
        })();
        if result.is_err()
            && let Err(error) =
                epub_cache::discard_unrecorded_sibling_file(&destination_file, reserved.temp_path())
        {
            crate::logger::log(format!("epub sibling temp removal failed: {error:?}"));
        }
        drop(destination_file);
        result
    })();
    if reserved.was_created()
        && let Err(error) = cache.finish_sibling_output(&reserved)
    {
        crate::logger::log(format!(
            "epub sibling cleanup deferred to next startup: {error:?}"
        ));
    }
    publish?;
    drop(guard);
    let mappings = [
        crate::rename_key_migration::StoreCopyPathMapping::exact(source, &destination),
        crate::rename_key_migration::StoreCopyPathMapping::virtual_prefix(source, &destination),
    ];
    let copied = crate::rename_key_migration::copy_restore_stores_without_identity_at(
        context.data_dir,
        &mappings,
    );
    Ok(SavedPdf {
        path: destination,
        reused_cache,
        user_data_errors: copied.errors,
    })
}

fn copy_cached_pdf(
    source: &Path,
    destination: &Path,
    cancel: &CancelToken,
) -> Result<(), EpubConvertError> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    copy_pdf_to_open_file(source, &mut output, cancel)
}

fn copy_pdf_to_open_file(
    source: &Path,
    output: &mut File,
    cancel: &CancelToken,
) -> Result<(), EpubConvertError> {
    let mut input = File::open(source)?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err(EpubConvertError::Cancelled);
        }
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
    }
    output.flush()?;
    Ok(())
}

fn open_work_pdf_for_verification(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        options.share_mode(0x0000_0001); // allow readers, deny writes and rename/delete
    }
    options.open(path)
}

/// The verified work PDF and the published bytes are checked through the retained temp handle.
fn copy_verified_pdf_to_sibling_file(
    input: &mut File,
    output: &mut File,
    cancel: &CancelToken,
) -> Result<(), EpubConvertError> {
    input.seek(SeekFrom::Start(0))?;
    let mut work_hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err(EpubConvertError::Cancelled);
        }
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        work_hash.update(&buffer[..read]);
        output.write_all(&buffer[..read])?;
    }
    output.flush()?;
    output.sync_all()?;
    output.seek(SeekFrom::Start(0))?;
    let mut destination_hash = Sha256::new();
    loop {
        if cancel.is_cancelled() {
            return Err(EpubConvertError::Cancelled);
        }
        let read = output.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        destination_hash.update(&buffer[..read]);
    }
    #[cfg(test)]
    if FAIL_NEXT_SIBLING_READBACK_HASH.with(|flag| flag.replace(false)) {
        destination_hash.update(b"injected mismatch");
    }
    if work_hash.finalize() != destination_hash.finalize() {
        return Err(EpubConvertError::InvalidPdf);
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static FAIL_NEXT_SIBLING_READBACK_HASH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
            WorkerEvent::Progress {
                phase,
                done,
                total,
                pages,
            } if final_result.is_none() => {
                let _ = progress.send(ConvertProgress {
                    phase,
                    done,
                    total,
                    pages,
                });
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
        Status::Webview2Unsupported => Err(EpubConvertError::WebView2Unsupported),
        Status::RenderFailed => Err(EpubConvertError::RenderFailed),
        Status::Timeout => Err(EpubConvertError::Timeout),
    }
}

/// Test-only placeholder. Production callers must supply PDFium verification.
#[cfg(test)]
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
        let mut args: Vec<OsString> = match spec.operation {
            WorkerOperation::Inspect => vec![
                spec.executable.as_os_str().to_os_string(),
                OsString::from("inspect"),
                spec.input.as_os_str().to_os_string(),
            ],
            WorkerOperation::Convert => vec![
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
            ],
        };
        if matches!(spec.operation, WorkerOperation::Convert) {
            if let Some(stem) = &spec.source_stem {
                args.push(OsString::from("--source-stem"));
                args.push(stem.clone());
            }
        }
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

    #[test]
    fn epub_convert_progress_parser_accepts_spine_pages_and_legacy_shape() {
        for (progress_json, expected_pages) in [
            (
                r#"{"event":"progress","phase":"print","done":2,"total":5,"pages":18}"#,
                Some(18),
            ),
            (
                r#"{"event":"progress","phase":"print","done":18,"total":0}"#,
                None,
            ),
        ] {
            let stream = format!(
                "{progress_json}\n{{\"event\":\"result\",\"status\":\"success\",\"exit_code\":0,\"page_count\":18,\"direction\":\"ltr\",\"layout\":\"reflow\",\"profile\":\"reflow-v1\",\"blocked_requests\":0,\"message\":\"\"}}\n"
            );
            let (tx, rx) = mpsc::channel();
            assert!(matches!(
                read_events(Box::new(Cursor::new(stream.into_bytes())), &tx).unwrap(),
                WorkerEvent::Result {
                    status: Status::Success,
                    ..
                }
            ));
            let progress = rx.recv().unwrap();
            assert_eq!(progress.phase, Phase::Print);
            assert_eq!(progress.pages, expected_pages);
            assert_eq!(progress.done, if expected_pages.is_some() { 2 } else { 18 });
            assert_eq!(progress.total, if expected_pages.is_some() { 5 } else { 0 });
        }
    }

    struct TestPdfVerifier;
    impl ConvertedPdfVerifier for TestPdfVerifier {
        fn verify(
            &self,
            output: &ReservedOutput,
            pages: usize,
            _cancel: &CancelToken,
        ) -> Result<(), EpubConvertError> {
            verify_converted_pdf_stage2a(output.part_path(), pages)
        }

        fn verify_sibling(
            &self,
            output: &SiblingOutput,
            pages: usize,
            _cancel: &CancelToken,
        ) -> Result<(), EpubConvertError> {
            verify_converted_pdf_stage2a(output.part_path(), pages)
        }
    }

    fn reserved_part(root: &Path) -> PathBuf {
        let conn = rusqlite::Connection::open(root.join("epub_cache.db")).unwrap();
        let file: String = conn
            .query_row(
                "SELECT pdf_file FROM generation_ids ORDER BY generation_id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        PathBuf::from(file).with_extension("pdf.part")
    }

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
            assert_eq!(spec.source_stem.as_deref(), self.source.file_stem());
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
            if matches!(
                self.mode,
                FakeMode::Cancel | FakeMode::CrashAfterOutput | FakeMode::Failure(6)
            ) {
                fs::write(
                    format!("{}.tmp-worker", spec.output.display()),
                    b"worker residue",
                )
                .unwrap();
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
                7 => "render_failed", // retired code: deliberately mismatches the status contract
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

    struct InspectSpawner {
        code: i32,
        stdout: Vec<u8>,
    }

    impl WorkerSpawner for InspectSpawner {
        fn spawn(&self, spec: &WorkerSpec) -> Result<Box<dyn WorkerChild>, EpubConvertError> {
            assert!(matches!(spec.operation, WorkerOperation::Inspect));
            assert!(
                spec.input
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
            );
            Ok(Box::new(FakeChild {
                stdout: Some(self.stdout.clone()),
                code: self.code,
                mode: FakeMode::Success,
                killed: Arc::new(AtomicBool::new(false)),
            }))
        }
    }

    #[test]
    fn inspect_fake_worker_reports_summary_and_rejects_drm() {
        let spawner = InspectSpawner {
            code: 0,
            stdout: br#"{"rendition":{"layout":"pre-paginated"},"direction":"rtl","spine":[{"rendition":{"layout":null}},{"rendition":{"layout":null}}],"drm":"none"}"#.to_vec(),
        };
        let cancel = CancelToken::new().unwrap();
        assert_eq!(
            inspect_at(Path::new("book.epub"), &cancel, 5, &spawner).unwrap(),
            EpubInspectSummary {
                layout: "fixed".into(),
                direction: "rtl".into(),
                spine_count: 2,
                sibling_pdf_exists: false,
            }
        );
        let drm = InspectSpawner {
            code: 2,
            stdout: Vec::new(),
        };
        assert!(matches!(
            inspect_at(Path::new("book.epub"), &cancel, 5, &drm),
            Err(EpubConvertError::Drm)
        ));
    }

    #[test]
    fn inspect_worker_result_marks_existing_sibling_pdf() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("book.epub");
        fs::write(&source, b"epub").unwrap();
        fs::write(source.with_extension("pdf"), b"existing").unwrap();
        let spawner = InspectSpawner {
            code: 0,
            stdout: br#"{"rendition":{"layout":"pre-paginated"},"direction":"ltr","spine":[{"rendition":{"layout":null}}],"drm":"none"}"#.to_vec(),
        };
        let summary = inspect_at(&source, &CancelToken::new().unwrap(), 5, &spawner).unwrap();
        assert!(summary.sibling_pdf_exists);
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
            &TestPdfVerifier,
        );
        (result, tmp, killed, temp_root)
    }

    struct NoSpawner;
    impl WorkerSpawner for NoSpawner {
        fn spawn(&self, _: &WorkerSpec) -> Result<Box<dyn WorkerChild>, EpubConvertError> {
            panic!("a current cache generation must be copied without launching the worker")
        }
    }

    fn save_with<S: WorkerSpawner>(
        root: &Path,
        source: &Path,
        spawner: &S,
        verifier: &dyn ConvertedPdfVerifier,
        cancel: &CancelToken,
    ) -> Result<SavedPdf, EpubConvertError> {
        let gate = epub_cache::startup_gate(root);
        let epub_cache::GateOutcome::Enabled { guard, .. } = gate else {
            panic!("test gate disabled")
        };
        let (tx, _rx) = mpsc::channel();
        save_sibling_at(
            ConvertContext {
                gate: &guard,
                data_dir: root,
                temp_root: &root.join("temp"),
            },
            source,
            cancel,
            &tx,
            3,
            spawner,
            verifier,
        )
    }

    #[test]
    fn sibling_save_reuses_current_generation_without_worker() {
        let (converted, tmp, _, _) = run(FakeMode::Success);
        assert!(converted.is_ok());
        let source = tmp.path().join("book.epub");
        let before = EpubCache::open_at(tmp.path())
            .unwrap()
            .list_current()
            .unwrap()
            .len();
        let saved = save_with(
            tmp.path(),
            &source,
            &NoSpawner,
            &TestPdfVerifier,
            &CancelToken::new().unwrap(),
        )
        .unwrap();
        assert!(saved.reused_cache);
        assert!(saved.path.exists());
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt as _;
            assert_eq!(
                fs::metadata(&saved.path).unwrap().file_attributes() & 0x2,
                0
            );
        }
        assert_eq!(
            EpubCache::open_at(tmp.path())
                .unwrap()
                .list_current()
                .unwrap()
                .len(),
            before
        );
    }

    #[test]
    fn sibling_save_reconverts_old_output_version_but_keeps_old_generation_viewable() {
        let (converted, tmp, _, _) = run(FakeMode::Success);
        assert!(converted.is_ok());
        let source = tmp.path().join("book.epub");
        let conn = rusqlite::Connection::open(tmp.path().join("epub_cache.db")).unwrap();
        conn.execute("UPDATE generations SET output_version=0", [])
            .unwrap();
        drop(conn);
        let old = EpubCache::open_at(tmp.path())
            .unwrap()
            .current_generation(&epub_cache::src_key(&source))
            .unwrap()
            .unwrap();
        assert_eq!(old.output_version, 0);
        assert!(old.pdf_file.exists());
        let fake = FakeSpawner {
            mode: FakeMode::Success,
            source: source.clone(),
            killed: Arc::new(AtomicBool::new(false)),
        };
        let saved = save_with(
            tmp.path(),
            &source,
            &fake,
            &TestPdfVerifier,
            &CancelToken::new().unwrap(),
        )
        .unwrap();
        assert!(!saved.reused_cache);
        assert!(saved.path.exists());
        assert_eq!(
            EpubCache::open_at(tmp.path())
                .unwrap()
                .current_generation(&epub_cache::src_key(&source))
                .unwrap()
                .unwrap()
                .generation_id,
            old.generation_id
        );
    }

    #[test]
    fn sibling_save_without_generation_converts_without_cache_row_and_cleans_part() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("book.epub");
        fs::write(&source, b"source EPUB bytes").unwrap();
        let fake = FakeSpawner {
            mode: FakeMode::Success,
            source: source.clone(),
            killed: Arc::new(AtomicBool::new(false)),
        };
        let saved = save_with(
            tmp.path(),
            &source,
            &fake,
            &TestPdfVerifier,
            &CancelToken::new().unwrap(),
        )
        .unwrap();
        assert!(!saved.reused_cache);
        assert_eq!(fs::read(&saved.path).unwrap(), b"%PDF-1.4\nfake");
        assert!(
            EpubCache::open_at(tmp.path())
                .unwrap()
                .list_current()
                .unwrap()
                .is_empty()
        );
        let conn = rusqlite::Connection::open(tmp.path().join("epub_cache.db")).unwrap();
        let outstanding: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM outstanding_sibling_outputs",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(outstanding, 0);
        assert_eq!(
            fs::read_dir(tmp.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".part"))
                .count(),
            0
        );
    }
    #[test]
    fn sibling_save_passes_original_stem_to_worker_after_copying_source() {
        struct WorkSpawner(FakeSpawner, PathBuf);
        impl WorkerSpawner for WorkSpawner {
            fn spawn(&self, spec: &WorkerSpec) -> Result<Box<dyn WorkerChild>, EpubConvertError> {
                assert!(spec.output.starts_with(self.1.join("epub_sibling_work")));
                self.0.spawn(spec)
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("MyNovel.epub");
        fs::write(&source, b"source EPUB bytes").unwrap();
        let fake = WorkSpawner(
            FakeSpawner {
                mode: FakeMode::Success,
                source: source.clone(),
                killed: Arc::new(AtomicBool::new(false)),
            },
            tmp.path().to_owned(),
        );
        let saved = save_with(
            tmp.path(),
            &source,
            &fake,
            &TestPdfVerifier,
            &CancelToken::new().unwrap(),
        )
        .unwrap();
        assert_eq!(saved.path, source.with_extension("pdf"));
        assert!(!saved.reused_cache);
    }

    #[test]
    fn sibling_save_stale_generation_runs_worker() {
        let (converted, tmp, _, _) = run(FakeMode::Success);
        assert!(converted.is_ok());
        let source = tmp.path().join("book.epub");
        fs::write(&source, b"new EPUB bytes").unwrap();
        let fake = FakeSpawner {
            mode: FakeMode::Success,
            source: source.clone(),
            killed: Arc::new(AtomicBool::new(false)),
        };
        let saved = save_with(
            tmp.path(),
            &source,
            &fake,
            &TestPdfVerifier,
            &CancelToken::new().unwrap(),
        )
        .unwrap();
        assert!(!saved.reused_cache);
        assert_eq!(
            EpubCache::open_at(tmp.path())
                .unwrap()
                .list_current()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn sibling_save_does_not_reuse_generation_path_outside_cache() {
        let (converted, tmp, _, _) = run(FakeMode::Success);
        assert!(converted.is_ok());
        let source = tmp.path().join("book.epub");
        let outside = tmp.path().join("outside.pdf");
        fs::write(&outside, b"%PDF-1.4\nfake").unwrap();
        let db = rusqlite::Connection::open(tmp.path().join("epub_cache.db")).unwrap();
        db.execute(
            "UPDATE generations SET pdf_file = ?1 WHERE src_path_key = ?2",
            rusqlite::params![outside.to_string_lossy(), epub_cache::src_key(&source)],
        )
        .unwrap();
        drop(db);
        let fake = FakeSpawner {
            mode: FakeMode::Success,
            source: source.clone(),
            killed: Arc::new(AtomicBool::new(false)),
        };
        let saved = save_with(
            tmp.path(),
            &source,
            &fake,
            &TestPdfVerifier,
            &CancelToken::new().unwrap(),
        )
        .unwrap();
        assert!(!saved.reused_cache);
    }

    struct PublishRaceVerifier {
        destination: PathBuf,
    }

    #[cfg(windows)]
    struct WorkFileLockVerifier;
    #[cfg(windows)]
    impl ConvertedPdfVerifier for WorkFileLockVerifier {
        fn verify(
            &self,
            _: &ReservedOutput,
            _: usize,
            _: &CancelToken,
        ) -> Result<(), EpubConvertError> {
            unreachable!()
        }
        fn verify_sibling(
            &self,
            output: &SiblingOutput,
            pages: usize,
            cancel: &CancelToken,
        ) -> Result<(), EpubConvertError> {
            assert!(
                OpenOptions::new()
                    .write(true)
                    .open(output.part_path())
                    .is_err()
            );
            TestPdfVerifier.verify_sibling(output, pages, cancel)
        }
    }

    #[cfg(windows)]
    #[test]
    fn sibling_save_holds_verified_work_file_write_denied_through_publish() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("book.epub");
        fs::write(&source, b"source EPUB bytes").unwrap();
        let fake = FakeSpawner {
            mode: FakeMode::Success,
            source: source.clone(),
            killed: Arc::new(AtomicBool::new(false)),
        };
        let saved = save_with(
            tmp.path(),
            &source,
            &fake,
            &WorkFileLockVerifier,
            &CancelToken::new().unwrap(),
        )
        .unwrap();
        assert_eq!(fs::read(saved.path).unwrap(), b"%PDF-1.4\nfake");
    }
    impl ConvertedPdfVerifier for PublishRaceVerifier {
        fn verify(
            &self,
            _: &ReservedOutput,
            _: usize,
            _: &CancelToken,
        ) -> Result<(), EpubConvertError> {
            unreachable!()
        }
        fn verify_sibling(
            &self,
            output: &SiblingOutput,
            pages: usize,
            cancel: &CancelToken,
        ) -> Result<(), EpubConvertError> {
            TestPdfVerifier.verify_sibling(output, pages, cancel)?;
            fs::write(&self.destination, b"another writer's PDF").unwrap();
            Ok(())
        }
    }

    #[test]
    fn sibling_save_publish_does_not_clobber_pdf_created_after_check() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("book.epub");
        fs::write(&source, b"source EPUB bytes").unwrap();
        let fake = FakeSpawner {
            mode: FakeMode::Success,
            source: source.clone(),
            killed: Arc::new(AtomicBool::new(false)),
        };
        let result = save_with(
            tmp.path(),
            &source,
            &fake,
            &PublishRaceVerifier {
                destination: source.with_extension("pdf"),
            },
            &CancelToken::new().unwrap(),
        );
        assert!(
            matches!(result, Err(EpubConvertError::ExistingPdf)),
            "{result:?}"
        );
        assert_eq!(
            fs::read(source.with_extension("pdf")).unwrap(),
            b"another writer's PDF"
        );
        assert_eq!(
            fs::read_dir(tmp.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".part"))
                .count(),
            0
        );
    }

    #[test]
    fn sibling_save_readback_hash_mismatch_removes_temp_without_publishing() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("book.epub");
        fs::write(&source, b"source EPUB bytes").unwrap();
        let fake = FakeSpawner {
            mode: FakeMode::Success,
            source: source.clone(),
            killed: Arc::new(AtomicBool::new(false)),
        };
        FAIL_NEXT_SIBLING_READBACK_HASH.with(|flag| flag.set(true));
        let result = save_with(
            tmp.path(),
            &source,
            &fake,
            &TestPdfVerifier,
            &CancelToken::new().unwrap(),
        );
        assert!(
            matches!(result, Err(EpubConvertError::InvalidPdf)),
            "{result:?}"
        );
        assert!(!source.with_extension("pdf").exists());
        assert_eq!(
            fs::read_dir(tmp.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".miv-part-"))
                .count(),
            0
        );
    }

    #[test]
    fn sibling_save_cancel_removes_part_and_does_not_publish() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("book.epub");
        fs::write(&source, b"source EPUB bytes").unwrap();
        let fake = FakeSpawner {
            mode: FakeMode::Cancel,
            source: source.clone(),
            killed: Arc::new(AtomicBool::new(false)),
        };
        let result = save_with(
            tmp.path(),
            &source,
            &fake,
            &TestPdfVerifier,
            &CancelToken::new().unwrap(),
        );
        assert!(
            matches!(result, Err(EpubConvertError::Cancelled)),
            "{result:?}"
        );
        assert!(!source.with_extension("pdf").exists());
        assert_eq!(
            fs::read_dir(tmp.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".part"))
                .count(),
            0
        );
    }

    #[test]
    fn sibling_save_cancel_timeout_and_worker_kill_leave_no_destination_temps() {
        for mode in [
            FakeMode::Cancel,
            FakeMode::Failure(6),
            FakeMode::CrashAfterOutput,
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let source = tmp.path().join("book.epub");
            fs::write(&source, b"source EPUB bytes").unwrap();
            let fake = FakeSpawner {
                mode,
                source: source.clone(),
                killed: Arc::new(AtomicBool::new(false)),
            };
            assert!(
                save_with(
                    tmp.path(),
                    &source,
                    &fake,
                    &TestPdfVerifier,
                    &CancelToken::new().unwrap()
                )
                .is_err()
            );
            assert!(!source.with_extension("pdf").exists());
            let leftovers: Vec<_> = fs::read_dir(tmp.path())
                .unwrap()
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| {
                    name.contains(".miv-part-")
                        || name.contains(".tmp-worker")
                        || name.ends_with(".part")
                })
                .collect();
            assert!(leftovers.is_empty(), "{leftovers:?}");
        }
    }

    #[test]
    fn sibling_save_worker_failure_removes_part_and_does_not_publish() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("book.epub");
        fs::write(&source, b"source EPUB bytes").unwrap();
        let fake = FakeSpawner {
            mode: FakeMode::Failure(5),
            source: source.clone(),
            killed: Arc::new(AtomicBool::new(false)),
        };
        let result = save_with(
            tmp.path(),
            &source,
            &fake,
            &TestPdfVerifier,
            &CancelToken::new().unwrap(),
        );
        assert!(
            matches!(result, Err(EpubConvertError::RenderFailed)),
            "{result:?}"
        );
        assert!(!source.with_extension("pdf").exists());
        assert_eq!(
            fs::read_dir(tmp.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".part"))
                .count(),
            0
        );
    }

    struct PermissionDeniedVerifier;
    impl ConvertedPdfVerifier for PermissionDeniedVerifier {
        fn verify(
            &self,
            _: &ReservedOutput,
            _: usize,
            _: &CancelToken,
        ) -> Result<(), EpubConvertError> {
            unreachable!()
        }

        fn verify_sibling(
            &self,
            _: &SiblingOutput,
            _: usize,
            _: &CancelToken,
        ) -> Result<(), EpubConvertError> {
            Err(EpubConvertError::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "read-only destination",
            )))
        }
    }

    #[test]
    fn sibling_save_permission_failure_removes_part_and_does_not_publish() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("book.epub");
        fs::write(&source, b"source EPUB bytes").unwrap();
        let fake = FakeSpawner {
            mode: FakeMode::Success,
            source: source.clone(),
            killed: Arc::new(AtomicBool::new(false)),
        };
        let result = save_with(
            tmp.path(),
            &source,
            &fake,
            &PermissionDeniedVerifier,
            &CancelToken::new().unwrap(),
        );
        assert!(
            matches!(result, Err(EpubConvertError::Io(error)) if error.kind() == io::ErrorKind::PermissionDenied)
        );
        assert!(!source.with_extension("pdf").exists());
        assert_eq!(
            fs::read_dir(tmp.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".part"))
                .count(),
            0
        );
    }

    #[test]
    fn sibling_save_copies_existing_file_copy_data_but_not_identity_bookmarks_or_collection() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("book.epub");
        let destination = source.with_extension("pdf");
        fs::write(&source, b"source EPUB bytes").unwrap();
        let epub_key = crate::adjustment_db::normalize_path(&source);
        let pdf_key = crate::adjustment_db::normalize_path(&destination);
        let epub_page = format!("{epub_key}::page_1");
        let pdf_page = format!("{pdf_key}::page_1");
        let rating = crate::rating_db::RatingDb::open_at(tmp.path().join("rating.db")).unwrap();
        rating.set(&epub_key, 4).unwrap();
        drop(rating);
        let adjust =
            crate::adjustment_db::AdjustmentDb::open_at(&tmp.path().join("adjustment.db")).unwrap();
        let params = crate::adjustment::AdjustParams {
            brightness: 19.0,
            ..Default::default()
        };
        adjust.set_page_params(&epub_page, &params).unwrap();
        drop(adjust);
        let identity = rusqlite::Connection::open(tmp.path().join("content_identity.db")).unwrap();
        identity.execute_batch("CREATE TABLE edit_origin (file_key TEXT PRIMARY KEY, size INTEGER NOT NULL, head_hash TEXT NOT NULL, full_hash TEXT, hashed_mtime INTEGER NOT NULL, kind TEXT NOT NULL, last_edit_at INTEGER NOT NULL, has_restorable_content INTEGER NOT NULL)").unwrap();
        identity
            .execute(
                "INSERT INTO edit_origin VALUES (?1, 10, 'head', 'full', 20, 'epub', 30, 1)",
                [&epub_key],
            )
            .unwrap();
        drop(identity);
        let bookmarks_path = tmp.path().join("book_bookmarks.db");
        crate::book_bookmarks::ensure_schema_at(&bookmarks_path).unwrap();
        let bookmarks = rusqlite::Connection::open(&bookmarks_path).unwrap();
        bookmarks.execute("INSERT INTO book_bookmarks (container_key, container_path, container_kind, page_kind, page_value, page_key, created_at_ms) VALUES (?1, ?2, 'pdf', 'index', '1', 'page_1', 1)", rusqlite::params![crate::book_bookmarks::container_key(&source), source.to_string_lossy()]).unwrap();
        drop(bookmarks);
        let collection = rusqlite::Connection::open(tmp.path().join("collection.db")).unwrap();
        collection
            .execute_batch("CREATE TABLE collection_entries (source_path TEXT NOT NULL)")
            .unwrap();
        collection
            .execute(
                "INSERT INTO collection_entries VALUES (?1)",
                [source.to_string_lossy().as_ref()],
            )
            .unwrap();
        drop(collection);

        let fake = FakeSpawner {
            mode: FakeMode::Success,
            source: source.clone(),
            killed: Arc::new(AtomicBool::new(false)),
        };
        let saved = save_with(
            tmp.path(),
            &source,
            &fake,
            &TestPdfVerifier,
            &CancelToken::new().unwrap(),
        )
        .unwrap();
        assert!(
            saved.user_data_errors.is_empty(),
            "{:?}",
            saved.user_data_errors
        );
        let rating = crate::rating_db::RatingDb::open_at(tmp.path().join("rating.db")).unwrap();
        assert_eq!(rating.get(&epub_key), 4);
        assert_eq!(rating.get(&pdf_key), 4);
        let adjust =
            crate::adjustment_db::AdjustmentDb::open_at(&tmp.path().join("adjustment.db")).unwrap();
        assert_eq!(adjust.get_page_params(&epub_page).unwrap().brightness, 19.0);
        assert_eq!(adjust.get_page_params(&pdf_page).unwrap().brightness, 19.0);
        let identity = rusqlite::Connection::open(tmp.path().join("content_identity.db")).unwrap();
        let count: i64 = identity
            .query_row(
                "SELECT COUNT(*) FROM edit_origin WHERE file_key = ?1",
                [&pdf_key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        let count: i64 = identity
            .query_row(
                "SELECT COUNT(*) FROM edit_origin WHERE file_key = ?1",
                [&epub_key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let bookmarks = rusqlite::Connection::open(bookmarks_path).unwrap();
        let bookmark_paths: Vec<String> = bookmarks
            .prepare("SELECT container_path FROM book_bookmarks")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(bookmark_paths, vec![source.to_string_lossy().to_string()]);
        let collection = rusqlite::Connection::open(tmp.path().join("collection.db")).unwrap();
        let collection_paths: Vec<String> = collection
            .prepare("SELECT source_path FROM collection_entries")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(collection_paths, vec![source.to_string_lossy().to_string()]);
    }

    #[test]
    fn sibling_save_post_publish_bookkeeping_error_preserves_success_and_copies_data() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("book.epub");
        let destination = source.with_extension("pdf");
        fs::write(&source, b"source EPUB bytes").unwrap();
        let epub_key = crate::adjustment_db::normalize_path(&source);
        let pdf_key = crate::adjustment_db::normalize_path(&destination);
        crate::rating_db::RatingDb::open_at(tmp.path().join("rating.db"))
            .unwrap()
            .set(&epub_key, 4)
            .unwrap();
        let adjust =
            crate::adjustment_db::AdjustmentDb::open_at(&tmp.path().join("adjustment.db")).unwrap();
        adjust
            .set_page_params(
                &format!("{epub_key}::page_1"),
                &crate::adjustment::AdjustParams {
                    brightness: 19.0,
                    ..Default::default()
                },
            )
            .unwrap();
        drop(adjust);
        let fake = FakeSpawner {
            mode: FakeMode::Success,
            source: source.clone(),
            killed: Arc::new(AtomicBool::new(false)),
        };
        crate::epub_cache::fail_next_sibling_finish_for_test();
        let saved = save_with(
            tmp.path(),
            &source,
            &fake,
            &TestPdfVerifier,
            &CancelToken::new().unwrap(),
        )
        .unwrap();
        assert_eq!(saved.path, destination);
        assert!(
            saved.user_data_errors.is_empty(),
            "{:?}",
            saved.user_data_errors
        );
        assert!(saved.path.exists());
        let mut app = crate::app::setup_app_for_test();
        app.epub_convert = Some(
            crate::ui_dialogs::epub_convert::EpubConvertState::saved_for_test(
                source.clone(),
                crate::app::OpenRequestOwner::Navigation,
                saved,
                app.top_level_grid_view.generation(),
                app.smart_folder_transition_sequence,
            ),
        );
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| app.show_epub_convert_dialog(ctx));
        assert_eq!(app.pdf_enumerate_pending.as_ref().unwrap().0, destination);
        assert_eq!(
            crate::rating_db::RatingDb::open_at(tmp.path().join("rating.db"))
                .unwrap()
                .get(&pdf_key),
            4
        );
        assert_eq!(
            crate::adjustment_db::AdjustmentDb::open_at(&tmp.path().join("adjustment.db"))
                .unwrap()
                .get_page_params(&format!("{pdf_key}::page_1"))
                .unwrap()
                .brightness,
            19.0
        );
        let db = crate::epub_cache::EpubCache::open_at(tmp.path()).unwrap();
        assert_eq!(db.outstanding_sibling_count_for_test(), 1);
        drop(db);
        assert!(matches!(
            crate::epub_cache::startup_gate(tmp.path()),
            crate::epub_cache::GateOutcome::Enabled { .. }
        ));
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let db = crate::epub_cache::EpubCache::open_at(tmp.path()).unwrap();
            if db.outstanding_sibling_count_for_test() == 0 {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
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
                    | (Err(EpubConvertError::WebView2Unsupported), "unsupported")
            );
            assert!(matched, "code {code}: {result:?}");
            assert!(fs::read_dir(temp_root).unwrap().next().is_none());
        }
    }

    #[test]
    fn epub_convert_fake_exit_seven_is_unexpected() {
        let (result, _tmp, _, temp_root) = run(FakeMode::Failure(7));
        assert!(
            matches!(result, Err(EpubConvertError::Protocol)),
            "{result:?}"
        );
        assert!(fs::read_dir(temp_root).unwrap().next().is_none());
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
        assert!(!reserved_part(tmp.path()).exists());
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
            &TestPdfVerifier,
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
            &TestPdfVerifier,
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
        assert!(!reserved_part(tmp.path()).exists());
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
            &TestPdfVerifier,
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
                &TestPdfVerifier,
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
        assert!(!reserved_part(tmp.path()).exists());
    }

    #[cfg(windows)]
    #[test]
    fn epub_convert_native_spawner_timeout_cleans_part_and_temp() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("book.epub");
        fs::write(&src, b"epub bytes").unwrap();
        let script = script_spawner(tmp.path(), "hang", None).script;
        let spawner = NativeScriptSpawner {
            script,
            mode: "hang",
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
            1,
            &spawner,
            &TestPdfVerifier,
        );
        assert!(
            matches!(result, Err(EpubConvertError::Timeout)),
            "{result:?}"
        );
        assert_eq!(rx.try_recv().unwrap().phase, Phase::Print);
        assert!(!reserved_part(tmp.path()).exists());
        assert!(
            fs::read_dir(tmp.path().join("temp"))
                .unwrap()
                .next()
                .is_none()
        );
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
