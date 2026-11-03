//! One caption job's worker thread. The App alone owns the queue and decides
//! when to send `Finish`; this worker never interprets an empty queue as EOF.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::vlm_worker::{
    self, CaptionOutcome, KillHandle, LoadRequest, PROMPT_VERSION, VlmError, VlmWorkerClient,
};
use crate::ai_caption_db::{
    CaptionDb, CaptionResult, CaptionStatus, FileSignature, SaveOutcome, SaveRequest,
};
use crate::app::book_cover::{
    BookCover, BookCoverEnv, BookCoverFailure, BookKind, ResolvedBookCover, resolve_book_cover,
};
use crate::canonical_image_loader::{
    AnimationPolicy, CanonicalDecodeOptions, CanonicalImageDecode, CanonicalImageSource,
    RawDecodeRuntime, RawStage, decode_canonical_image,
};
use crate::rotation_db::{Rotation, RotationDb};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptionTarget {
    Image(PathBuf),
    Book { path: PathBuf, kind: BookKind },
}

impl CaptionTarget {
    pub fn path(&self) -> &Path {
        match self {
            Self::Image(path) | Self::Book { path, .. } => path,
        }
    }
}

pub struct CaptionWorkerConfig {
    /// None resolves/extracts the bundled worker on this job's thread.
    pub worker_executable: Option<PathBuf>,
    pub load: LoadRequest,
    pub model_id: String,
    pub caption_db_path: PathBuf,
    pub rotation_db_path: PathBuf,
    pub raw: crate::raw::RawDecodeContext,
    pub book_cover_env: BookCoverEnv,
}

#[derive(Debug)]
pub enum CaptionWorkerCommand {
    Item(CaptionTarget),
    Finish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionItemOutcome {
    Created,
    AlreadyCreated,
    Missing,
    Unconverted,
    PasswordRequired,
    VideoCover,
    Failed,
    /// A failed row was committed; unlike a retained old ok row, it needs reflection.
    FailedWritten,
    Discarded,
    Cancelled,
}

pub enum CaptionWorkerEvent {
    Started(KillHandle),
    ItemDone(CaptionItemOutcome),
    Finished(Result<(), String>),
}

/// No thread joins occur on drop. Publication of the kill handle is protected
/// together with the exit flag check, including exit before Started is polled.
pub struct CaptionWorker {
    pub commands: Sender<CaptionWorkerCommand>,
    pub events: Receiver<CaptionWorkerEvent>,
    lifetime: Arc<WorkerLifetime>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct WorkerLifetime {
    exit: AtomicBool,
    kill: Mutex<Option<KillHandle>>,
}

impl WorkerLifetime {
    fn publish(&self, kill: KillHandle) -> bool {
        let mut slot = self.kill.lock().unwrap();
        if self.exit.load(Ordering::Acquire) {
            drop(slot);
            let _ = kill.kill();
            return false;
        }
        *slot = Some(kill);
        true
    }

    fn terminate(&self) {
        self.exit.store(true, Ordering::Release);
        // Never keep this brief publication lock across a process API call.
        let kill = self.kill.lock().unwrap().clone();
        if let Some(kill) = kill {
            let _ = kill.kill();
        }
    }
}

impl CaptionWorker {
    /// Application exit, unlike the user's graceful cancel, kills immediately.
    pub fn stop_for_exit(&self) {
        self.cancel.store(true, Ordering::Release);
        self.lifetime.terminate();
        let _ = self.commands.send(CaptionWorkerCommand::Finish);
    }

    #[cfg(test)]
    pub(crate) fn test_channels(
        cancel: Arc<AtomicBool>,
    ) -> (
        Self,
        Receiver<CaptionWorkerCommand>,
        Sender<CaptionWorkerEvent>,
    ) {
        let (commands, command_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        (
            Self {
                commands,
                events,
                lifetime: Arc::new(WorkerLifetime::default()),
                cancel,
            },
            command_rx,
            event_tx,
        )
    }
}

impl Drop for CaptionWorker {
    fn drop(&mut self) {
        self.stop_for_exit();
    }
}

pub fn spawn_caption_worker(
    config: CaptionWorkerConfig,
    cancel: Arc<AtomicBool>,
    repaint: egui::Context,
) -> io::Result<CaptionWorker> {
    let (commands, command_rx) = mpsc::channel();
    let (event_tx, events) = mpsc::channel();
    let lifetime = Arc::new(WorkerLifetime::default());
    let worker_lifetime = Arc::clone(&lifetime);
    let worker_cancel = Arc::clone(&cancel);
    std::thread::Builder::new()
        .name("caption-job".into())
        .spawn(move || {
            let result = run_worker(
                &config,
                &worker_cancel,
                &worker_lifetime,
                command_rx,
                &event_tx,
                &repaint,
            );
            if let Err(error) = &result {
                crate::logger::log(format!("caption_job: {error}"));
            }
            send_event(&event_tx, &repaint, CaptionWorkerEvent::Finished(result));
        })?;
    Ok(CaptionWorker {
        commands,
        events,
        lifetime,
        cancel,
    })
}

fn send_event(
    events: &Sender<CaptionWorkerEvent>,
    repaint: &egui::Context,
    event: CaptionWorkerEvent,
) -> bool {
    let sent = events.send(event).is_ok();
    repaint.request_repaint();
    sent
}

fn stopped(cancel: &AtomicBool, lifetime: &WorkerLifetime) -> bool {
    cancel.load(Ordering::Acquire) || lifetime.exit.load(Ordering::Acquire)
}

fn run_worker(
    config: &CaptionWorkerConfig,
    cancel: &Arc<AtomicBool>,
    lifetime: &WorkerLifetime,
    commands: Receiver<CaptionWorkerCommand>,
    events: &Sender<CaptionWorkerEvent>,
    repaint: &egui::Context,
) -> Result<(), String> {
    if stopped(cancel, lifetime) {
        return Ok(());
    }
    let executable = match &config.worker_executable {
        Some(path) => path.clone(),
        None => vlm_worker::worker_executable().map_err(|error| error.to_string())?,
    };
    if stopped(cancel, lifetime) {
        return Ok(());
    }
    let mut client = VlmWorkerClient::start_at(&executable).map_err(|error| error.to_string())?;
    let kill = client.kill_handle();
    if !lifetime.publish(kill.clone()) {
        return Ok(());
    }
    if !send_event(events, repaint, CaptionWorkerEvent::Started(kill)) {
        return Ok(());
    }
    // Keep the client in this scope so every early return shuts it down too.
    let result = run_started_worker(
        config,
        cancel,
        lifetime,
        commands,
        events,
        repaint,
        &mut client,
    );
    client.shutdown();
    result
}

fn run_started_worker(
    config: &CaptionWorkerConfig,
    cancel: &Arc<AtomicBool>,
    lifetime: &WorkerLifetime,
    commands: Receiver<CaptionWorkerCommand>,
    events: &Sender<CaptionWorkerEvent>,
    repaint: &egui::Context,
    client: &mut VlmWorkerClient,
) -> Result<(), String> {
    if stopped(cancel, lifetime) {
        return Ok(());
    }
    client.hello().map_err(|error| error.to_string())?;
    if stopped(cancel, lifetime) {
        return Ok(());
    }
    let started = Instant::now();
    let loaded = client.load(&config.load);
    stage_event("model_load", started, loaded.is_ok());
    loaded.map_err(|error| error.to_string())?;
    if stopped(cancel, lifetime) {
        return Ok(());
    }
    let db = CaptionDb::open_at(&config.caption_db_path).map_err(|error| error.to_string())?;
    let rotation = match std::fs::metadata(&config.rotation_db_path) {
        Ok(_) => Some(
            RotationDb::open_readonly_at(&config.rotation_db_path)
                .map_err(|error| error.to_string())?,
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    let mut request_id = 0;
    let mut discarded = 0;
    while let Ok(command) = commands.recv() {
        match command {
            CaptionWorkerCommand::Finish => break,
            CaptionWorkerCommand::Item(target) => {
                request_id += 1;
                let outcome = if stopped(cancel, lifetime) {
                    CaptionItemOutcome::Cancelled
                } else {
                    process_target(
                        config,
                        &db,
                        rotation.as_ref(),
                        cancel,
                        &target,
                        request_id,
                        &mut |id, width, height, rgb| client.caption(id, width, height, rgb),
                    )?
                };
                if outcome == CaptionItemOutcome::Discarded {
                    discarded += 1;
                }
                if !send_event(events, repaint, CaptionWorkerEvent::ItemDone(outcome)) {
                    break;
                }
            }
        }
    }
    if discarded != 0 {
        crate::logger::log(format!("caption_job: discarded={discarded}"));
    }
    Ok(())
}

fn process_target(
    config: &CaptionWorkerConfig,
    db: &CaptionDb,
    rotation: Option<&RotationDb>,
    cancel: &Arc<AtomicBool>,
    target: &CaptionTarget,
    request_id: u64,
    infer: &mut impl FnMut(u64, u32, u32, Vec<u8>) -> Result<CaptionOutcome, VlmError>,
) -> Result<CaptionItemOutcome, String> {
    if cancel.load(Ordering::Acquire) {
        return Ok(CaptionItemOutcome::Cancelled);
    }
    match target {
        CaptionTarget::Image(path) => {
            process_item(config, db, rotation, cancel, path, request_id, infer)
        }
        CaptionTarget::Book { path, kind } => {
            let started = Instant::now();
            let cover = resolve_book_cover(path, kind.clone(), &config.book_cover_env);
            stage_event(
                "cover_resolve",
                started,
                matches!(&cover, BookCover::Readable(_)),
            );
            process_book_cover(
                config,
                db,
                cancel,
                path,
                cover,
                request_id,
                infer,
                &mut |cover| decode_book_input(config, rotation, cover, cancel),
            )
        }
    }
}

/// Resolution and source-read failures never create a failed caption row.
fn process_book_cover(
    config: &CaptionWorkerConfig,
    db: &CaptionDb,
    cancel: &Arc<AtomicBool>,
    path: &Path,
    cover: BookCover,
    request_id: u64,
    infer: &mut impl FnMut(u64, u32, u32, Vec<u8>) -> Result<CaptionOutcome, VlmError>,
    read: &mut impl FnMut(
        &ResolvedBookCover,
    ) -> Result<Result<image::RgbImage, CaptionItemOutcome>, String>,
) -> Result<CaptionItemOutcome, String> {
    if cancel.load(Ordering::Acquire) {
        return Ok(CaptionItemOutcome::Cancelled);
    }
    let cover = match cover {
        BookCover::Readable(cover) => cover,
        BookCover::VideoFrame { .. } => return Ok(CaptionItemOutcome::VideoCover),
        BookCover::Unreadable(reason) => {
            return Ok(match reason {
                BookCoverFailure::Unconverted => CaptionItemOutcome::Unconverted,
                BookCoverFailure::PasswordRequired => CaptionItemOutcome::PasswordRequired,
                BookCoverFailure::Missing => CaptionItemOutcome::Missing,
                BookCoverFailure::ReadFailed => CaptionItemOutcome::Failed,
            });
        }
    };
    let existing = db
        .get(&path.to_string_lossy())
        .map_err(|error| error.to_string())?;
    if existing.is_some_and(|row| {
        row.status == CaptionStatus::Ok
            && row.cover_ref == cover.cover_ref
            && row.signature == cover.signature
            && row.model_id == config.model_id
            && row.prompt_version == PROMPT_VERSION
    }) {
        return Ok(CaptionItemOutcome::AlreadyCreated);
    }
    let started = Instant::now();
    let decoded = read(&cover);
    stage_event("image_load", started, matches!(&decoded, Ok(Ok(_))));
    if cancel.load(Ordering::Acquire) {
        return Ok(CaptionItemOutcome::Cancelled);
    }
    let rgb = match decoded? {
        Ok(rgb) => rgb,
        Err(outcome) => return Ok(outcome),
    };
    let started = Instant::now();
    let inferred = infer(request_id, rgb.width(), rgb.height(), rgb.into_raw());
    stage_event("infer", started, inferred.is_ok());
    if cancel.load(Ordering::Acquire) {
        return Ok(CaptionItemOutcome::Cancelled);
    }
    let result = match inferred.map_err(|error| error.to_string())? {
        CaptionOutcome::Ok(result) => CaptionResult::Ok {
            caption: result.caption,
            text: result.text,
            keywords: deduplicate_keywords(result.keywords),
        },
        CaptionOutcome::ImageFailure { message } => {
            crate::logger::log(format!("caption_job: book inference: {message}"));
            CaptionResult::Failed
        }
    };
    save_request(
        db,
        cancel,
        SaveRequest {
            item_path: path.to_owned(),
            signature_path: cover.signature_path,
            signature: cover.signature,
            cover_ref: cover.cover_ref,
            model_id: config.model_id.clone(),
            prompt_version: PROMPT_VERSION,
            lang: "ja".into(),
            result,
        },
    )
}

fn decode_book_input(
    config: &CaptionWorkerConfig,
    rotation: Option<&RotationDb>,
    cover: &ResolvedBookCover,
    cancel: &Arc<AtomicBool>,
) -> Result<Result<image::RgbImage, CaptionItemOutcome>, String> {
    use crate::pdf_loader::{CancelWaitPolicy, JobPriority, PdfRenderValidity};
    use crate::thumb_loader::ReadLocation;
    let decoded = match &cover.location {
        ReadLocation::File(path) => {
            return decode_input(config, rotation, path, cancel).map(|result| {
                result.map_err(|message| {
                    crate::logger::log(format!("caption_job: book image decode: {message}"));
                    CaptionItemOutcome::Failed
                })
            });
        }
        ReadLocation::ArchiveEntry { archive, entry } => {
            let source = match cover.preloaded_bytes.as_deref() {
                Some(bytes) => CanonicalImageSource::File {
                    path: Path::new(entry),
                    verified_bytes: Some(bytes),
                },
                None => CanonicalImageSource::ArchiveEntry {
                    archive_path: archive,
                    entry_name: entry,
                },
            };
            decode_canonical_image(source, decode_options(config, cancel))
                .map_err(|error| format!("{error:?}"))
                .and_then(|result| match result {
                    CanonicalImageDecode::Static(image) => Ok(image.image),
                    _ => Err("first-frame decode did not return a static image".into()),
                })
        }
        ReadLocation::PdfPage { pdf, page } => {
            let rendered = match cover.epub_read.as_ref() {
                Some(read) => crate::pdf_loader::render_page_with_read_target(
                    pdf,
                    read,
                    *page,
                    1024,
                    cover.pdf_password.as_deref(),
                    Some(cancel.clone()),
                    JobPriority::Normal,
                    PdfRenderValidity::Unscoped,
                    CancelWaitPolicy::AbortOnCancel,
                ),
                None => crate::pdf_loader::render_page(
                    pdf,
                    *page,
                    1024,
                    cover.pdf_password.as_deref(),
                    Some(cancel.clone()),
                    JobPriority::Normal,
                    PdfRenderValidity::Unscoped,
                    CancelWaitPolicy::AbortOnCancel,
                ),
            };
            match rendered {
                Ok(result) => Ok(result.image),
                Err(error) => {
                    crate::logger::log(format!("caption_job: book PDF render: {error}"));
                    return Ok(Err(pdf_failure_outcome(&error)));
                }
            }
        }
        ReadLocation::ChildCached | ReadLocation::Unreadable { .. } => {
            return Ok(Err(CaptionItemOutcome::Failed));
        }
    };
    Ok(decoded.map(|image| resize_rgb(&image)).map_err(|message| {
        crate::logger::log(format!("caption_job: book cover decode: {message}"));
        CaptionItemOutcome::Failed
    }))
}

fn pdf_failure_outcome(error: &io::Error) -> CaptionItemOutcome {
    if crate::pdf_loader::is_password_required_error(error) {
        CaptionItemOutcome::PasswordRequired
    } else {
        CaptionItemOutcome::Failed
    }
}

fn decode_options<'a>(
    config: &'a CaptionWorkerConfig,
    cancel: &'a Arc<AtomicBool>,
) -> CanonicalDecodeOptions<'a> {
    CanonicalDecodeOptions::fullscreen_cancellable(
        AnimationPolicy::FirstFrameOnly,
        cancel,
        RawStage::Full,
    )
    .with_avif_workload(crate::avif_decoder::Workload::Background)
    .with_raw_runtime(RawDecodeRuntime {
        executor: &config.raw.executor,
        brightness: config.raw.brightness,
        priority: crate::raw::RawPriority::Background,
        on_submitted: None,
    })
}

fn resize_rgb(image: &image::DynamicImage) -> image::RgbImage {
    crate::fast_resize::resize_dynamic_fit(image, 1024, 1024, crate::fast_resize::Quality::Lanczos3)
        .into_rgb8()
}

fn process_item(
    config: &CaptionWorkerConfig,
    db: &CaptionDb,
    rotation: Option<&RotationDb>,
    cancel: &Arc<AtomicBool>,
    path: &Path,
    request_id: u64,
    infer: &mut impl FnMut(u64, u32, u32, Vec<u8>) -> Result<CaptionOutcome, VlmError>,
) -> Result<CaptionItemOutcome, String> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(CaptionItemOutcome::Missing);
        }
        Err(error) => {
            crate::logger::log(format!("caption_job: input metadata: {error}"));
            return Ok(CaptionItemOutcome::Failed);
        }
    };
    let Some(signature) = FileSignature::from_metadata(&metadata) else {
        crate::logger::log("caption_job: unavailable input signature");
        return Ok(CaptionItemOutcome::Failed);
    };
    let existing = db
        .get(&path.to_string_lossy())
        .map_err(|error| error.to_string())?;
    if existing.is_some_and(|row| {
        row.status == CaptionStatus::Ok
            && row.signature == signature
            && row.model_id == config.model_id
            && row.prompt_version == PROMPT_VERSION
    }) {
        return Ok(CaptionItemOutcome::AlreadyCreated);
    }
    let started = Instant::now();
    let decoded = decode_input(config, rotation, path, cancel);
    stage_event("image_load", started, decoded.is_ok());
    if cancel.load(Ordering::Acquire) {
        return Ok(CaptionItemOutcome::Cancelled);
    }
    let result = match decoded? {
        Ok(rgb) => {
            let started = Instant::now();
            let inferred = infer(request_id, rgb.width(), rgb.height(), rgb.into_raw());
            stage_event("infer", started, inferred.is_ok());
            // User cancellation never persists the current result, even failure.
            if cancel.load(Ordering::Acquire) {
                return Ok(CaptionItemOutcome::Cancelled);
            }
            match inferred.map_err(|error| error.to_string())? {
                CaptionOutcome::Ok(result) => CaptionResult::Ok {
                    caption: result.caption,
                    text: result.text,
                    keywords: deduplicate_keywords(result.keywords),
                },
                CaptionOutcome::ImageFailure { message } => {
                    crate::logger::log(format!("caption_job: image inference: {message}"));
                    CaptionResult::Failed
                }
            }
        }
        Err(message) => {
            crate::logger::log(format!("caption_job: image decode: {message}"));
            CaptionResult::Failed
        }
    };
    save_result(config, db, cancel, path, signature, result)
}

/// Outer errors are fatal DB failures; inner errors belong to just this image.
fn decode_input(
    config: &CaptionWorkerConfig,
    rotation: Option<&RotationDb>,
    path: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<Result<image::RgbImage, String>, String> {
    let angle = match rotation {
        Some(db) => {
            // RotationDb's existing physical-path key contract.
            let key = path.to_string_lossy().to_lowercase().replace('\\', "/");
            db.get_key_checked(&key)
                .map_err(|error| error.to_string())?
                .unwrap_or(Rotation::None)
        }
        None => Rotation::None,
    };
    let decoded = decode_canonical_image(
        CanonicalImageSource::File {
            path,
            verified_bytes: None,
        },
        decode_options(config, cancel),
    );
    let image = match decoded {
        Ok(CanonicalImageDecode::Static(image)) => image.image,
        Ok(_) => {
            return Ok(Err(
                "first-frame decode did not return a static image".into()
            ));
        }
        Err(error) => return Ok(Err(format!("{error:?}"))),
    };
    let image = crate::capture::rotate_dynamic_image(image, angle);
    Ok(Ok(resize_rgb(&image)))
}

fn deduplicate_keywords(keywords: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    keywords
        .into_iter()
        .filter_map(|word| {
            let word = word.trim().to_owned();
            seen.insert(word.clone()).then_some(word)
        })
        .collect()
}

fn save_result(
    config: &CaptionWorkerConfig,
    db: &CaptionDb,
    cancel: &Arc<AtomicBool>,
    path: &Path,
    signature: FileSignature,
    result: CaptionResult,
) -> Result<CaptionItemOutcome, String> {
    if cancel.load(Ordering::Acquire) {
        return Ok(CaptionItemOutcome::Cancelled);
    }
    let request = SaveRequest {
        item_path: path.to_owned(),
        signature_path: path.to_owned(),
        signature,
        cover_ref: String::new(),
        model_id: config.model_id.clone(),
        prompt_version: PROMPT_VERSION,
        lang: "ja".into(),
        result,
    };
    save_request(db, cancel, request)
}

fn save_request(
    db: &CaptionDb,
    cancel: &Arc<AtomicBool>,
    request: SaveRequest,
) -> Result<CaptionItemOutcome, String> {
    let failed = matches!(request.result, CaptionResult::Failed);
    let started = Instant::now();
    let saved = db.save_unless_cancelled(&request, cancel);
    stage_event("save", started, saved.is_ok());
    match saved.map_err(|error| error.to_string())? {
        None => Ok(CaptionItemOutcome::Cancelled),
        Some(SaveOutcome::Discarded) => Ok(CaptionItemOutcome::Discarded),
        Some(SaveOutcome::Written) => Ok(if failed {
            CaptionItemOutcome::FailedWritten
        } else {
            CaptionItemOutcome::Created
        }),
        Some(SaveOutcome::KeptExistingOk) => Ok(CaptionItemOutcome::Failed),
    }
}

fn stage_event(stage: &str, started: Instant, success: bool) {
    if !crate::perf::is_enabled() {
        return;
    }
    crate::perf::event(
        "caption",
        stage,
        None,
        0,
        &[
            (
                "elapsed_ms",
                serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
            ),
            ("success", serde_json::json!(success)),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;
    use std::time::Duration;

    fn config(root: &Path) -> CaptionWorkerConfig {
        CaptionWorkerConfig {
            worker_executable: None,
            book_cover_env: BookCoverEnv::default(),
            load: LoadRequest {
                model_path: "synthetic model.gguf".into(),
                mmproj_path: "synthetic mmproj.gguf".into(),
                device_luid: "00000000:00000001".into(),
                cpu_threads: 2,
            },
            model_id: "synthetic-model".into(),
            caption_db_path: root.join("ai_caption.db"),
            rotation_db_path: root.join("rotation.db"),
            raw: crate::raw::RawDecodeContext::new(
                Arc::new(crate::raw::RawDevelopExecutor::new(1).unwrap()),
                crate::raw::RawBrightness::default(),
            ),
        }
    }

    fn image(root: &Path) -> PathBuf {
        let path = root.join("synthetic.png");
        image::RgbImage::from_raw(2, 1, vec![13, 10, 0, 26, 255, 128])
            .unwrap()
            .save(&path)
            .unwrap();
        path
    }

    fn ok_result() -> CaptionOutcome {
        CaptionOutcome::Ok(vlm_worker::CaptionResult {
            content: String::new(),
            finish_reason: "stop".into(),
            caption: "synthetic caption".into(),
            text: String::new(),
            keywords: vec![" 海 ".into(), "海".into(), "海辺".into()],
            validation: vlm_worker::Validation {
                encoding_valid: true,
                ended_on_eog: true,
                json_complete: true,
                field_types: true,
                limits: true,
                token_limit: true,
                ok: true,
            },
            generated_tokens: 7,
            wall_s: 0.01,
        })
    }

    fn fake_worker() -> &'static Path {
        use sha2::{Digest as _, Sha256};

        static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
        FIXTURE.get_or_init(|| {
            let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
            let source = manifest.join("tests/support/vlm_fake_worker.rs");
            let hash = Sha256::digest(include_bytes!("../../tests/support/vlm_fake_worker.rs"));
            // Reuse the transport tests' content-addressed standalone fake.
            let root = manifest.join("target/vlm-fake-worker");
            let directory = root.join(format!("{hash:x}"));
            let executable = directory.join("vlm-fake-worker.exe");
            if executable.is_file() {
                return executable;
            }
            std::fs::create_dir_all(&root).unwrap();
            let staging = tempfile::Builder::new()
                .prefix(".build-")
                .tempdir_in(&root)
                .unwrap();
            let output = std::process::Command::new("rustc")
                .env("TEMP", staging.path())
                .env("TMP", staging.path())
                .args(["--edition=2024", "--crate-name=vlm_fake_worker"])
                .arg(source)
                .arg("-o")
                .arg(staging.path().join("vlm-fake-worker.exe"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            if let Err(error) = std::fs::rename(staging.path(), &directory) {
                assert!(executable.is_file(), "fake worker publish: {error}");
            }
            executable
        })
    }

    fn next(worker: &CaptionWorker) -> CaptionWorkerEvent {
        worker
            .events
            .recv_timeout(Duration::from_secs(20))
            .expect("worker completion")
    }

    #[test]
    fn keyword_deduplication_trims_exact_matches_and_preserves_order_and_spelling() {
        let words = [
            " 海 ",
            "海辺",
            "海",
            "Ａ",
            "A",
            " ｶﾀｶﾅ",
            "カタカナ",
            "A",
            "",
            " ",
        ];
        assert_eq!(
            deduplicate_keywords(words.into_iter().map(str::to_owned).collect()),
            ["海", "海辺", "Ａ", "A", "ｶﾀｶﾅ", "カタカナ", ""]
        );
    }

    #[test]
    fn fake_job_saves_rotated_rgb_then_skips_and_waits_for_app_finish() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = config(temp.path());
        config.worker_executable = Some(fake_worker().to_owned());
        let path = image(temp.path());
        let rotations = RotationDb::open_at(&config.rotation_db_path).unwrap();
        rotations.set(&path, Rotation::Cw90).unwrap();
        let worker = spawn_caption_worker(
            config,
            Arc::new(AtomicBool::new(false)),
            egui::Context::default(),
        )
        .unwrap();
        worker
            .commands
            .send(CaptionWorkerCommand::Item(CaptionTarget::Image(
                path.clone(),
            )))
            .unwrap();
        assert!(matches!(next(&worker), CaptionWorkerEvent::Started(_)));
        assert!(matches!(
            next(&worker),
            CaptionWorkerEvent::ItemDone(CaptionItemOutcome::Created)
        ));
        let db = CaptionDb::open_at(temp.path().join("ai_caption.db")).unwrap();
        let row = db.get(&path.to_string_lossy()).unwrap().unwrap();
        assert_eq!(row.caption, "0d0a001aff80");
        assert_eq!(row.keywords, ["binary"]);
        worker
            .commands
            .send(CaptionWorkerCommand::Item(CaptionTarget::Image(path)))
            .unwrap();
        assert!(matches!(
            next(&worker),
            CaptionWorkerEvent::ItemDone(CaptionItemOutcome::AlreadyCreated)
        ));
        assert!(matches!(
            worker.events.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        worker.commands.send(CaptionWorkerCommand::Finish).unwrap();
        assert!(matches!(
            next(&worker),
            CaptionWorkerEvent::Finished(Ok(()))
        ));
    }

    #[test]
    fn worker_preprocessing_rotates_and_never_upscales_or_exceeds_1024() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let cancel = Arc::new(AtomicBool::new(false));
        let path = image(temp.path());
        let rotation = RotationDb::open_at(&config.rotation_db_path).unwrap();
        rotation.set(&path, Rotation::Cw90).unwrap();
        let rgb = decode_input(&config, Some(&rotation), &path, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(rgb.dimensions(), (1, 2));
        assert_eq!(rgb.into_raw(), [13, 10, 0, 26, 255, 128]);
        image::RgbImage::new(2048, 1024).save(&path).unwrap();
        let rgb = decode_input(&config, Some(&rotation), &path, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(rgb.dimensions(), (512, 1024));
    }

    #[test]
    fn failed_items_retry_and_never_replace_existing_ok() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = config(temp.path());
        let db = CaptionDb::open_at(&config.caption_db_path).unwrap();
        let path = image(temp.path());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut failed = |_, _, _, _| {
            Ok(CaptionOutcome::ImageFailure {
                message: "synthetic failure".into(),
            })
        };
        assert_eq!(
            process_item(&config, &db, None, &cancel, &path, 1, &mut failed).unwrap(),
            CaptionItemOutcome::FailedWritten
        );
        assert_eq!(
            db.get(&path.to_string_lossy()).unwrap().unwrap().status,
            CaptionStatus::Failed
        );
        assert_eq!(
            process_item(&config, &db, None, &cancel, &path, 2, &mut |_, _, _, _| Ok(
                ok_result()
            ))
            .unwrap(),
            CaptionItemOutcome::Created
        );
        let row = db.get(&path.to_string_lossy()).unwrap().unwrap();
        assert_eq!(row.keywords, ["海", "海辺"]);
        config.model_id = "other model".into();
        assert_eq!(
            process_item(&config, &db, None, &cancel, &path, 3, &mut failed).unwrap(),
            CaptionItemOutcome::Failed
        );
        assert_eq!(db.get(&path.to_string_lossy()).unwrap().unwrap(), row);
    }

    #[test]
    fn cancellation_after_inference_and_source_change_discard_without_writing() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let db = CaptionDb::open_at(&config.caption_db_path).unwrap();
        let path = image(temp.path());
        let cancel = Arc::new(AtomicBool::new(false));
        let result = process_item(&config, &db, None, &cancel, &path, 1, &mut |_, _, _, _| {
            cancel.store(true, Ordering::Release);
            Ok(ok_result())
        })
        .unwrap();
        assert_eq!(result, CaptionItemOutcome::Cancelled);
        assert!(db.get(&path.to_string_lossy()).unwrap().is_none());
        cancel.store(false, Ordering::Release);
        let result = process_item(&config, &db, None, &cancel, &path, 2, &mut |_, _, _, _| {
            std::fs::write(&path, b"changed source").unwrap();
            Ok(ok_result())
        })
        .unwrap();
        assert_eq!(result, CaptionItemOutcome::Discarded);
        assert!(db.get(&path.to_string_lossy()).unwrap().is_none());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            process_item(
                &config,
                &db,
                None,
                &cancel,
                &path,
                3,
                &mut |_, _, _, _| panic!("missing input inferred")
            )
            .unwrap(),
            CaptionItemOutcome::Missing
        );
    }

    #[test]
    fn fatal_inference_and_database_failure_stop_the_item_pipeline() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let db = CaptionDb::open_at(&config.caption_db_path).unwrap();
        let path = image(temp.path());
        let cancel = Arc::new(AtomicBool::new(false));
        let result = process_item(&config, &db, None, &cancel, &path, 1, &mut |_, _, _, _| {
            Err(VlmError::Fatal("synthetic fatal".into()))
        });
        assert!(result.unwrap_err().contains("synthetic fatal"));
        assert!(db.get(&path.to_string_lossy()).unwrap().is_none());
        let result = process_item(&config, &db, None, &cancel, &path, 2, &mut |_, _, _, _| {
            rusqlite::Connection::open(&config.caption_db_path)
                .unwrap()
                .execute_batch("DROP TABLE captions")
                .unwrap();
            Ok(ok_result())
        });
        assert!(result.is_err());
    }

    #[test]
    fn undecodable_input_saves_failed_and_old_prompt_does_not_skip() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let db = CaptionDb::open_at(&config.caption_db_path).unwrap();
        let path = image(temp.path());
        let cancel = Arc::new(AtomicBool::new(false));
        let signature = FileSignature::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
        db.save(&SaveRequest {
            item_path: path.clone(),
            signature_path: path.clone(),
            signature,
            cover_ref: String::new(),
            model_id: config.model_id.clone(),
            prompt_version: PROMPT_VERSION - 1,
            lang: "ja".into(),
            result: CaptionResult::Ok {
                caption: "old prompt".into(),
                text: String::new(),
                keywords: Vec::new(),
            },
        })
        .unwrap();
        assert_eq!(
            process_item(&config, &db, None, &cancel, &path, 1, &mut |_, _, _, _| Ok(
                ok_result()
            ))
            .unwrap(),
            CaptionItemOutcome::Created
        );
        db.delete(&path.to_string_lossy()).unwrap();
        std::fs::write(&path, b"synthetic invalid PNG").unwrap();
        assert_eq!(
            process_item(
                &config,
                &db,
                None,
                &cancel,
                &path,
                2,
                &mut |_, _, _, _| panic!("undecodable input inferred")
            )
            .unwrap(),
            CaptionItemOutcome::FailedWritten
        );
        assert_eq!(
            db.get(&path.to_string_lossy()).unwrap().unwrap().status,
            CaptionStatus::Failed
        );
    }

    #[test]
    fn startup_cancel_prevents_resolution_and_spawn_and_missing_worker_reports_failure() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = config(temp.path());
        config.worker_executable = Some(temp.path().join("does-not-exist.exe"));
        let worker = spawn_caption_worker(
            config,
            Arc::new(AtomicBool::new(true)),
            egui::Context::default(),
        )
        .unwrap();
        assert!(matches!(
            next(&worker),
            CaptionWorkerEvent::Finished(Ok(()))
        ));
        let mut config = self::config(temp.path());
        config.worker_executable = Some(temp.path().join("does-not-exist.exe"));
        let worker = spawn_caption_worker(
            config,
            Arc::new(AtomicBool::new(false)),
            egui::Context::default(),
        )
        .unwrap();
        assert!(matches!(
            next(&worker),
            CaptionWorkerEvent::Finished(Err(_))
        ));
    }

    #[test]
    fn exit_before_or_after_kill_publication_terminates_the_same_worker() {
        for exit_first in [true, false] {
            let client = VlmWorkerClient::start_at(fake_worker()).unwrap();
            let kill = client.kill_handle();
            let lifetime = WorkerLifetime::default();
            if exit_first {
                lifetime.terminate();
                assert!(!lifetime.publish(kill.clone()));
            } else {
                assert!(lifetime.publish(kill.clone()));
                lifetime.terminate();
            }
            assert!(kill.is_killed());
            assert!(!client.is_usable());
        }
    }
    fn book_cover(source: &Path, cover_ref: &str) -> BookCover {
        BookCover::Readable(ResolvedBookCover {
            location: crate::thumb_loader::ReadLocation::File(source.to_owned()),
            signature_path: source.to_owned(),
            signature: FileSignature::from_metadata(&std::fs::metadata(source).unwrap()).unwrap(),
            cover_ref: cover_ref.into(),
            epub_read: None,
            pdf_password: None,
            preloaded_bytes: None,
        })
    }

    #[test]
    fn book_job_saves_folder_key_cover_signature_and_recreates_changed_cover() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let db = CaptionDb::open_at(&config.caption_db_path).unwrap();
        let source = image(temp.path());
        let folder = temp.path();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut calls = 0;
        let mut infer = |_, _, _, _| {
            calls += 1;
            Ok(ok_result())
        };
        let mut read = |cover: &ResolvedBookCover| decode_book_input(&config, None, cover, &cancel);
        assert_eq!(
            process_book_cover(
                &config,
                &db,
                &cancel,
                folder,
                book_cover(&source, "synthetic.png"),
                1,
                &mut infer,
                &mut read
            )
            .unwrap(),
            CaptionItemOutcome::Created
        );
        let row = db.get(&folder.to_string_lossy()).unwrap().unwrap();
        assert_eq!(row.cover_ref, "synthetic.png");
        assert_eq!(
            row.signature,
            FileSignature::from_metadata(&std::fs::metadata(&source).unwrap()).unwrap()
        );
        assert_eq!(row.model_id, config.model_id);
        assert_eq!(row.prompt_version, PROMPT_VERSION);
        assert!(db.get(&source.to_string_lossy()).unwrap().is_none());
        assert_eq!(
            process_book_cover(
                &config,
                &db,
                &cancel,
                folder,
                book_cover(&source, "synthetic.png"),
                2,
                &mut infer,
                &mut |_| panic!("unchanged book decoded")
            )
            .unwrap(),
            CaptionItemOutcome::AlreadyCreated
        );
        assert_eq!(
            process_book_cover(
                &config,
                &db,
                &cancel,
                folder,
                book_cover(&source, "different.png"),
                3,
                &mut infer,
                &mut read
            )
            .unwrap(),
            CaptionItemOutcome::Created
        );
        assert_eq!(calls, 2);
        assert_eq!(
            db.get(&folder.to_string_lossy())
                .unwrap()
                .unwrap()
                .cover_ref,
            "different.png"
        );
    }

    #[test]
    fn book_job_uses_original_container_signature_not_converted_read_path() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let db = CaptionDb::open_at(&config.caption_db_path).unwrap();
        let original = temp.path().join("original.7z");
        std::fs::write(&original, b"original archive").unwrap();
        let source = image(temp.path());
        let mut cover = match book_cover(&source, "#entry=normalized/001.png") {
            BookCover::Readable(cover) => cover,
            _ => unreachable!(),
        };
        cover.signature_path = original.clone();
        cover.signature =
            FileSignature::from_metadata(&std::fs::metadata(&original).unwrap()).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let result = process_book_cover(
            &config,
            &db,
            &cancel,
            &original,
            BookCover::Readable(cover),
            1,
            &mut |_, _, _, _| Ok(ok_result()),
            &mut |cover| decode_book_input(&config, None, cover, &cancel),
        )
        .unwrap();
        assert_eq!(result, CaptionItemOutcome::Created);
        let row = db.get(&original.to_string_lossy()).unwrap().unwrap();
        assert_eq!(row.cover_ref, "#entry=normalized/001.png");
        assert_eq!(row.signature.size, 16);
    }

    #[test]
    fn book_job_unreadable_unconverted_password_and_video_never_read_infer_or_save() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let db = CaptionDb::open_at(&config.caption_db_path).unwrap();
        let source = image(temp.path());
        let cancel = Arc::new(AtomicBool::new(false));
        let cases = [
            (
                BookCover::Unreadable(BookCoverFailure::Unconverted),
                CaptionItemOutcome::Unconverted,
            ),
            (
                BookCover::Unreadable(BookCoverFailure::PasswordRequired),
                CaptionItemOutcome::PasswordRequired,
            ),
            (
                BookCover::Unreadable(BookCoverFailure::Missing),
                CaptionItemOutcome::Missing,
            ),
            (
                BookCover::Unreadable(BookCoverFailure::ReadFailed),
                CaptionItemOutcome::Failed,
            ),
            (
                BookCover::VideoFrame {
                    cover_ref: "movie.mp4#frame=10".into(),
                    signature_path: source.clone(),
                    signature: FileSignature::from_metadata(&std::fs::metadata(&source).unwrap())
                        .unwrap(),
                },
                CaptionItemOutcome::VideoCover,
            ),
        ];
        for (cover, expected) in cases {
            assert_eq!(
                process_book_cover(
                    &config,
                    &db,
                    &cancel,
                    temp.path(),
                    cover,
                    1,
                    &mut |_, _, _, _| panic!("unreadable book inferred"),
                    &mut |_| panic!("unreadable book decoded")
                )
                .unwrap(),
                expected
            );
            assert!(db.get(&temp.path().to_string_lossy()).unwrap().is_none());
        }
    }

    #[test]
    fn book_job_decode_failures_do_not_save_and_pdf_password_errors_are_classified() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let db = CaptionDb::open_at(&config.caption_db_path).unwrap();
        let source = image(temp.path());
        let cancel = Arc::new(AtomicBool::new(false));
        for message in [
            "Password required",
            "wrong password",
            "page not found",
            "PDF render failed",
        ] {
            let expected = if message.contains("assword") {
                CaptionItemOutcome::PasswordRequired
            } else {
                CaptionItemOutcome::Failed
            };
            let result = process_book_cover(
                &config,
                &db,
                &cancel,
                temp.path(),
                book_cover(&source, "book.pdf#page=3"),
                1,
                &mut |_, _, _, _| panic!("failed PDF inferred"),
                &mut |_| Ok(Err(pdf_failure_outcome(&io::Error::other(message)))),
            )
            .unwrap();
            assert_eq!(result, expected);
            assert!(db.get(&temp.path().to_string_lossy()).unwrap().is_none());
        }
        std::fs::write(&source, b"invalid PNG").unwrap();
        assert_eq!(
            process_book_cover(
                &config,
                &db,
                &cancel,
                temp.path(),
                book_cover(&source, "synthetic.png"),
                2,
                &mut |_, _, _, _| panic!("bad cover inferred"),
                &mut |cover| decode_book_input(&config, None, cover, &cancel)
            )
            .unwrap(),
            CaptionItemOutcome::Failed
        );
        assert!(db.get(&temp.path().to_string_lossy()).unwrap().is_none());
    }

    #[test]
    fn book_job_cancel_after_read_and_inference_and_source_change_prevents_save() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let db = CaptionDb::open_at(&config.caption_db_path).unwrap();
        let source = image(temp.path());
        let cancel = Arc::new(AtomicBool::new(false));
        assert_eq!(
            process_book_cover(
                &config,
                &db,
                &cancel,
                temp.path(),
                book_cover(&source, "synthetic.png"),
                1,
                &mut |_, _, _, _| panic!("cancelled read inferred"),
                &mut |_| {
                    cancel.store(true, Ordering::Release);
                    Ok(Ok(image::RgbImage::new(1, 1)))
                }
            )
            .unwrap(),
            CaptionItemOutcome::Cancelled
        );
        cancel.store(false, Ordering::Release);
        assert_eq!(
            process_book_cover(
                &config,
                &db,
                &cancel,
                temp.path(),
                book_cover(&source, "synthetic.png"),
                2,
                &mut |_, _, _, _| {
                    cancel.store(true, Ordering::Release);
                    Ok(ok_result())
                },
                &mut |cover| decode_book_input(&config, None, cover, &cancel)
            )
            .unwrap(),
            CaptionItemOutcome::Cancelled
        );
        cancel.store(false, Ordering::Release);
        assert_eq!(
            process_book_cover(
                &config,
                &db,
                &cancel,
                temp.path(),
                book_cover(&source, "synthetic.png"),
                3,
                &mut |_, _, _, _| {
                    std::fs::write(&source, b"changed").unwrap();
                    Ok(ok_result())
                },
                &mut |cover| decode_book_input(&config, None, cover, &cancel)
            )
            .unwrap(),
            CaptionItemOutcome::Discarded
        );
        assert!(db.get(&temp.path().to_string_lossy()).unwrap().is_none());
    }

    #[test]
    fn book_job_preloaded_archive_image_uses_entry_extension_and_ignores_rotation() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let source = image(temp.path());
        let rotation = RotationDb::open_at(&config.rotation_db_path).unwrap();
        rotation.set(&source, Rotation::Cw90).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut cover = match book_cover(&source, "#entry=cover.png") {
            BookCover::Readable(cover) => cover,
            _ => unreachable!(),
        };
        assert_eq!(
            decode_book_input(&config, Some(&rotation), &cover, &cancel)
                .unwrap()
                .unwrap()
                .dimensions(),
            (1, 2)
        );
        cover.location = crate::thumb_loader::ReadLocation::ArchiveEntry {
            archive: temp.path().join("missing.zip"),
            entry: "cover.png".into(),
        };
        cover.preloaded_bytes = Some(std::fs::read(&source).unwrap());
        let rgb = decode_book_input(&config, Some(&rotation), &cover, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(rgb.dimensions(), (2, 1));
        assert_eq!(rgb.into_raw(), [13, 10, 0, 26, 255, 128]);
    }

    #[test]
    fn fake_book_job_resolves_folder_cover_and_saves_book_key() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = config(temp.path());
        config.worker_executable = Some(fake_worker().to_owned());
        let source = image(temp.path());
        let worker = spawn_caption_worker(
            config,
            Arc::new(AtomicBool::new(false)),
            egui::Context::default(),
        )
        .unwrap();
        worker
            .commands
            .send(CaptionWorkerCommand::Item(CaptionTarget::Book {
                path: temp.path().to_owned(),
                kind: BookKind::Folder,
            }))
            .unwrap();
        assert!(matches!(next(&worker), CaptionWorkerEvent::Started(_)));
        assert!(matches!(
            next(&worker),
            CaptionWorkerEvent::ItemDone(CaptionItemOutcome::Created)
        ));
        let db = CaptionDb::open_at(temp.path().join("ai_caption.db")).unwrap();
        let row = db.get(&temp.path().to_string_lossy()).unwrap().unwrap();
        assert_eq!(row.cover_ref, "synthetic.png");
        assert_eq!(row.caption, "0d0a001aff80");
        assert!(db.get(&source.to_string_lossy()).unwrap().is_none());
        worker.commands.send(CaptionWorkerCommand::Finish).unwrap();
        assert!(matches!(
            next(&worker),
            CaptionWorkerEvent::Finished(Ok(()))
        ));
    }
}
