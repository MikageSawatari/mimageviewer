//! EffeTune Mixwright sample integration. The controller owns its dedicated
//! bridge, state capture queue, persistence, and audio publication slot.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::Value;

use crate::video::dsp::{DspBridge, GuiFailure, GuiOwnerPolicy, LatencyPolicy};

mod bundle_location;
pub mod composition;
pub(crate) mod gui_gate;
mod window;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GuiButtonAction {
    Show,
    Activate,
    Hide,
}

pub(crate) fn gui_button_action(visible: bool, in_front: bool) -> GuiButtonAction {
    if !visible {
        GuiButtonAction::Show
    } else if in_front {
        GuiButtonAction::Hide
    } else {
        GuiButtonAction::Activate
    }
}

const BUNDLE_NAME: &str = "EffeTune Mixwright.vst3";
const INITIAL_CAPTURE_WAIT: Duration = Duration::from_secs(6);
const EXIT_FENCE: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq)]
pub enum UnavailableReason {
    BundleMissing(String),
    BundlePreparationFailed {
        reason: String,
        rejected_generation: Option<String>,
    },
    CpuUnsupported,
    PlatformUnsupported,
}

impl UnavailableReason {
    pub(crate) fn user_reason(&self) -> &'static str {
        match self {
            Self::BundleMissing(_) => "必要なファイルが見つかりません",
            Self::BundlePreparationFailed { reason, .. }
                if reason.contains(bundle_location::PATH_TOO_LONG_MARKER) =>
            {
                "保存先のパスが長すぎます (APPDATA のパスを短くしてください)"
            }
            Self::BundlePreparationFailed { .. } => {
                "同梱ファイルを準備できません (詳しくはログを確認してください)"
            }
            Self::CpuUnsupported => "この CPU では動作しません (AVX2/FMA が必要です)",
            Self::PlatformUnsupported => "この OS では利用できません",
        }
    }

    fn preparation_failed(reason: impl Into<String>) -> Self {
        Self::BundlePreparationFailed {
            reason: reason.into(),
            rejected_generation: None,
        }
    }

    pub(crate) fn preparation_retryable(&self) -> bool {
        matches!(self, Self::BundlePreparationFailed { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadOrigin {
    Startup,
    UserButton,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EffetuneFailure {
    LoadFailed(String),
    RestoreFailed(String),
    ProcessFailed(String),
    LatencyExceeded { total_secs: f64 },
    HostLost(String),
    GuiFailed(String),
}

impl EffetuneFailure {
    pub(crate) fn user_reason(&self) -> &'static str {
        match self {
            Self::LoadFailed(_) => "読み込みに失敗しました",
            Self::RestoreFailed(_) => "保存した設定を復元できませんでした",
            Self::ProcessFailed(_) => "音声処理に失敗しました",
            Self::LatencyExceeded { .. } => "音声処理の遅延が上限を超えました",
            Self::HostLost(_) => "音声処理との接続が切れました",
            Self::GuiFailed(_) => "設定画面の操作に失敗しました",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureError {
    CallerDeadline,
    HostExited { watchdog: bool },
    Interrupted(String),
    State(String),
    Persistence(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CallerDeadline => write!(f, "caller deadline expired"),
            Self::HostExited { watchdog: true } => write!(f, "host state watchdog expired"),
            Self::HostExited { watchdog: false } => write!(f, "host exited"),
            Self::Interrupted(reason) | Self::State(reason) | Self::Persistence(reason) => {
                f.write_str(reason)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EffetuneRuntime {
    Unavailable(UnavailableReason),
    Idle,
    Loading {
        origin: LoadOrigin,
        open_gui_when_ready: Option<ShowPermit>,
    },
    Running {
        generation: u64,
    },
    Failed(EffetuneFailure),
}

impl EffetuneRuntime {
    fn can_start_load(&self) -> bool {
        matches!(self, Self::Idle)
            || matches!(self, Self::Unavailable(reason) if reason.preparation_retryable())
    }

    pub(crate) fn toolbar_available(&self) -> bool {
        self.can_start_load() || matches!(self, Self::Running { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectiveState {
    Effective,
    Inert,
    Unparseable(String),
}

impl EffectiveState {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        match Self::parse(bytes) {
            Ok(effective) => effective,
            Err(reason) => Self::Unparseable(reason),
        }
    }

    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let document: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if document.get("formatVersion").and_then(Value::as_u64) != Some(1) {
            return Err("unsupported formatVersion".into());
        }
        let master_bypass = document
            .get("masterBypass")
            .and_then(Value::as_bool)
            .ok_or("masterBypass missing")?;
        if master_bypass {
            return Ok(Self::Inert);
        }
        let pipeline_name = match document.get("currentPipeline").and_then(Value::as_str) {
            Some("A") => "pipelineA",
            Some("B") => "pipelineB",
            _ => return Err("invalid currentPipeline".into()),
        };
        let plugins = match document.get(pipeline_name) {
            Some(Value::Array(plugins)) => plugins.as_slice(),
            Some(Value::Null) if pipeline_name == "pipelineB" => &[],
            _ => return Err(format!("{pipeline_name} missing or invalid")),
        };
        let mut section_enabled = true;
        for plugin in plugins {
            let name = plugin
                .get("name")
                .and_then(Value::as_str)
                .ok_or("plugin name missing")?;
            let enabled = plugin
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or("plugin enabled missing")?;
            if name == "Section" || name == "SectionPlugin" {
                section_enabled = enabled;
            } else if enabled && section_enabled {
                return Ok(Self::Effective);
            }
        }
        Ok(Self::Inert)
    }
}

/// All local players observe this one slot. A pump reads it once per block.
#[derive(Default)]
pub struct EffetuneAudioSlot {
    current: Mutex<Option<(u64, Arc<DspBridge>)>>,
    failure_tx: Mutex<Option<mpsc::Sender<EffetuneFailure>>>,
    failed_generation: AtomicU64,
}

impl EffetuneAudioSlot {
    pub fn snapshot(&self) -> Option<(u64, Arc<DspBridge>)> {
        self.current
            .lock()
            .unwrap()
            .clone()
            .filter(|(generation, _)| *generation > self.failed_generation.load(Ordering::Acquire))
    }

    fn publish(&self, generation: u64, bridge: Arc<DspBridge>) {
        *self.current.lock().unwrap() = Some((generation, bridge));
    }

    fn clear(&self) {
        *self.current.lock().unwrap() = None;
    }

    pub fn report_failure_once(&self, generation: u64, failure: EffetuneFailure) {
        let mut previous = self.failed_generation.load(Ordering::Acquire);
        loop {
            if previous >= generation {
                return;
            }
            match self.failed_generation.compare_exchange_weak(
                previous,
                generation,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => previous = actual,
            }
        }
        if let Some(tx) = self.failure_tx.lock().unwrap().as_ref() {
            let _ = tx.send(failure);
        }
    }
}

#[derive(Clone)]
struct LatestCapture {
    generation: u64,
    bytes: Option<Vec<u8>>,
    effective: Option<EffectiveState>,
}

impl Default for LatestCapture {
    fn default() -> Self {
        Self {
            generation: 0,
            bytes: None,
            effective: None,
        }
    }
}

struct CaptureJob {
    generation: u64,
    bridge: Arc<DspBridge>,
    reply: mpsc::Sender<Result<Vec<u8>, CaptureError>>,
}

enum CaptureCommand {
    Capture(CaptureJob),
    Stop,
}

pub struct CaptureQueue {
    tx: mpsc::Sender<CaptureCommand>,
    next_generation: AtomicU64,
    accepting: AtomicBool,
    latest: Arc<Mutex<LatestCapture>>,
    publication: Arc<PublicationGate>,
}

pub struct ExitCaptureFence {
    bridge: Arc<DspBridge>,
    rx: mpsc::Receiver<Result<Vec<u8>, CaptureError>>,
    deadline: Instant,
    publication: Arc<PublicationGate>,
}

enum HostCommand {
    RegisterBridge(std::sync::Weak<DspBridge>),
    ReconcileVisibility,
    Disable(Arc<DspBridge>),
    Show {
        bridge: Arc<DspBridge>,
        permit: ShowPermit,
    },
    Gui {
        bridge: Arc<crate::video::dsp::bridge::Bridge>,
        value: Value,
    },
    Hide {
        bridge: Arc<DspBridge>,
        reply: mpsc::Sender<Result<(), String>>,
    },
}

#[derive(Default)]
struct PublicationGate {
    state: Mutex<PublicationState>,
}

#[derive(Default)]
struct PublicationState {
    exit_deadline: Option<Instant>,
    phase: PublicationPhase,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum PublicationPhase {
    #[default]
    Open,
    Committing,
    Committed,
    Expired,
}

impl PublicationGate {
    fn begin_exit(&self, deadline: Instant) {
        let mut state = self.state.lock().unwrap();
        state.exit_deadline = Some(deadline);
        if state.phase == PublicationPhase::Committed {
            state.phase = PublicationPhase::Open;
        }
    }

    fn commit_with(&self, replace: impl FnOnce() -> std::io::Result<()>) -> std::io::Result<()> {
        {
            let mut state = self.state.lock().unwrap();
            if state.phase == PublicationPhase::Committing {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "state replacement already in progress",
                ));
            }
            if state.phase == PublicationPhase::Expired
                || state
                    .exit_deadline
                    .is_some_and(|deadline| Instant::now() >= deadline)
            {
                state.phase = PublicationPhase::Expired;
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "state publication fence expired",
                ));
            }
            state.phase = PublicationPhase::Committing;
        }
        // Admission and expiry are ordered by the mutex; file I/O never holds it.
        let result = replace();
        let mut state = self.state.lock().unwrap();
        state.phase = if result.is_ok() {
            PublicationPhase::Committed
        } else if state
            .exit_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            PublicationPhase::Expired
        } else {
            PublicationPhase::Open
        };
        result
    }

    /// Returns whether a replacement was already admitted before expiry.
    fn expire(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        match state.phase {
            PublicationPhase::Committing | PublicationPhase::Committed => true,
            PublicationPhase::Open | PublicationPhase::Expired => {
                state.phase = PublicationPhase::Expired;
                false
            }
        }
    }
}

impl CaptureQueue {
    pub fn new(state_path: PathBuf) -> Self {
        Self::new_with_capture(state_path, capture_once)
    }

    fn new_with_capture(
        state_path: PathBuf,
        capture: impl Fn(&DspBridge) -> Result<Vec<u8>, CaptureError> + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<CaptureCommand>();
        let latest = Arc::new(Mutex::new(LatestCapture::default()));
        let worker_latest = Arc::clone(&latest);
        let publication = Arc::new(PublicationGate::default());
        let worker_publication = Arc::clone(&publication);
        std::thread::Builder::new()
            .name("effetune-state-capture".into())
            .spawn(move || {
                while let Ok(CaptureCommand::Capture(job)) = rx.recv() {
                    let started = Instant::now();
                    let result = capture(&job.bridge).and_then(|bytes| {
                        let effective = EffectiveState::from_bytes(&bytes);
                        if let EffectiveState::Unparseable(reason) = &effective {
                            return Err(CaptureError::State(format!(
                                "state classification failed: {reason}"
                            )));
                        }
                        publish_capture_state(
                            &worker_latest,
                            &state_path,
                            job.generation,
                            &bytes,
                            effective.clone(),
                            &worker_publication,
                        )?;
                        crate::logger::log(format!(
                            "[EffeTune] capture generation={} state={effective:?} bytes={} elapsed_ms={}",
                            job.generation,
                            bytes.len(),
                            started.elapsed().as_millis()
                        ));
                        Ok(bytes)
                    });
                    if let Err(error) = &result {
                        crate::logger::log(format!(
                            "[EffeTune] capture generation={} failed after {}ms: {error}",
                            job.generation,
                            started.elapsed().as_millis()
                        ));
                    }
                    let _ = job.reply.send(result);
                }
            })
            .expect("EffeTune state worker spawn");
        Self {
            tx,
            next_generation: AtomicU64::new(0),
            accepting: AtomicBool::new(true),
            latest,
            publication,
        }
    }

    pub fn request(
        &self,
        bridge: Arc<DspBridge>,
    ) -> Result<mpsc::Receiver<Result<Vec<u8>, CaptureError>>, CaptureError> {
        if !self.accepting.load(Ordering::Acquire) {
            return Err(CaptureError::Interrupted(
                "state capture is stopping".into(),
            ));
        }
        self.enqueue(bridge)
    }

    fn enqueue(
        &self,
        bridge: Arc<DspBridge>,
    ) -> Result<mpsc::Receiver<Result<Vec<u8>, CaptureError>>, CaptureError> {
        let generation = self.next_generation.fetch_add(1, Ordering::AcqRel) + 1;
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(CaptureCommand::Capture(CaptureJob {
                generation,
                bridge,
                reply,
            }))
            .map_err(|_| CaptureError::Interrupted("state capture worker disconnected".into()))?;
        Ok(rx)
    }

    pub fn latest_state(&self) -> Option<(u64, Vec<u8>, EffectiveState)> {
        let latest = self.latest.lock().unwrap();
        Some((
            latest.generation,
            latest.bytes.clone()?,
            latest.effective.clone()?,
        ))
    }

    pub fn latest_effective_state(&self) -> Option<EffectiveState> {
        self.latest.lock().unwrap().effective.clone()
    }

    pub fn begin_final_capture(&self, bridge: Arc<DspBridge>) -> Option<ExitCaptureFence> {
        self.accepting.store(false, Ordering::Release);
        let deadline = Instant::now() + EXIT_FENCE;
        self.publication.begin_exit(deadline);
        let rx = self.enqueue(Arc::clone(&bridge));
        let _ = self.tx.send(CaptureCommand::Stop);
        match rx {
            Ok(rx) => Some(ExitCaptureFence {
                bridge,
                rx,
                deadline,
                publication: Arc::clone(&self.publication),
            }),
            Err(error) => {
                crate::logger::log(format!("[EffeTune] exit capture enqueue: {error}"));
                None
            }
        }
    }

    pub fn wait_final_capture(fence: ExitCaptureFence) {
        let remaining = fence.deadline.saturating_duration_since(Instant::now());
        match fence.rx.recv_timeout(remaining) {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => crate::logger::log(format!("[EffeTune] exit capture: {error}")),
            Err(_) => {
                let admitted = fence.publication.expire();
                crate::logger::log(if admitted {
                    "[EffeTune] exit capture fence expired; admitted state replacement may finish"
                } else {
                    "[EffeTune] exit capture fence expired; previous state retained"
                });
                fence.bridge.terminate_host_now();
            }
        }
    }
}

fn capture_once(bridge: &DspBridge) -> Result<Vec<u8>, CaptureError> {
    let rx = bridge
        .query_first_state_concurrent()
        .map_err(|error| classify_capture_bridge_error(bridge, error))?;
    let encoded = rx.recv().map_err(|error| {
        CaptureError::Interrupted(format!("capture result channel closed: {error}"))
    })?;
    let encoded = encoded.map_err(|error| classify_capture_bridge_error(bridge, error))?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| CaptureError::State(format!("state base64 invalid: {error}")))
}

fn classify_capture_bridge_error(
    bridge: &DspBridge,
    error: crate::video::dsp::bridge::ConcurrentStateError,
) -> CaptureError {
    match error {
        crate::video::dsp::bridge::ConcurrentStateError::HostExited => {
            let code = bridge.host_exit_code_after(Duration::from_secs(2));
            CaptureError::HostExited {
                watchdog: code == Some(crate::video::dsp::bridge::STATE_WATCHDOG_EXIT_CODE),
            }
        }
        crate::video::dsp::bridge::ConcurrentStateError::Interrupted(reason) => {
            CaptureError::Interrupted(reason)
        }
        crate::video::dsp::bridge::ConcurrentStateError::HostResponse(reason) => {
            CaptureError::State(reason)
        }
    }
}

fn publish_capture_state(
    latest: &Mutex<LatestCapture>,
    path: &Path,
    generation: u64,
    bytes: &[u8],
    effective: EffectiveState,
    publication: &PublicationGate,
) -> Result<(), CaptureError> {
    // Only this serial worker writes the file, so the generation check and I/O need no lock.
    if generation <= latest.lock().unwrap().generation {
        return Ok(());
    }
    write_state_atomic(path, bytes, |temp, path| {
        publication.commit_with(|| fs::rename(temp, path))
    })
    .map_err(|error| CaptureError::Persistence(format!("state write failed: {error}")))?;
    let mut current = latest.lock().unwrap();
    current.generation = generation;
    current.bytes = Some(bytes.to_vec());
    current.effective = Some(effective);
    Ok(())
}

fn write_state_atomic(
    path: &Path,
    bytes: &[u8],
    commit: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("state path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".mixwright-state-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.flush()?;
        drop(file);
        commit(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub fn resolve_bundle_from_exe(
    exe: Result<PathBuf, std::io::Error>,
) -> Result<PathBuf, UnavailableReason> {
    let exe =
        exe.map_err(|error| UnavailableReason::BundleMissing(format!("current_exe: {error}")))?;
    let parent = exe.parent().ok_or_else(|| {
        UnavailableReason::BundleMissing("executable has no parent directory".into())
    })?;
    resolve_bundle_at(parent, None, None)
}

fn resolve_bundle_at(
    parent: &Path,
    generation: Option<&str>,
    preparation_error: Option<&str>,
) -> Result<PathBuf, UnavailableReason> {
    #[cfg(not(feature = "portable"))]
    if let Some(error) = preparation_error {
        return Err(UnavailableReason::preparation_failed(error));
    }
    let container = parent.join("effetune");
    #[cfg(not(feature = "portable"))]
    let root = {
        let result = if let Some(generation) = generation {
            bundle_location::encode_pointer(generation)
                .and_then(|_| bundle_location::checked_directory(&container))
                .and_then(|_| {
                    let root = container.join(generation);
                    bundle_location::checked_directory(&root)?;
                    Ok(root)
                })
        } else {
            match std::fs::symlink_metadata(container.join(bundle_location::POINTER_FILE)) {
                Ok(_) => bundle_location::read_generation(&container),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(container.clone()),
                Err(error) => Err(error),
            }
        };
        result.map_err(|error| UnavailableReason::preparation_failed(error.to_string()))?
    };
    #[cfg(feature = "portable")]
    let root = {
        let _ = (generation, preparation_error);
        container
    };
    #[cfg(not(feature = "portable"))]
    match std::fs::symlink_metadata(&root) {
        Ok(_) => bundle_location::checked_directory(&root)
            .map_err(|error| UnavailableReason::preparation_failed(error.to_string()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => {
            return Err(UnavailableReason::preparation_failed(error.to_string()));
        }
    }
    let bundle = root.join(BUNDLE_NAME);
    if !bundle.is_dir() {
        return Err(UnavailableReason::BundleMissing(
            bundle.display().to_string(),
        ));
    }
    #[cfg(not(feature = "portable"))]
    bundle_location::checked_directory(&bundle)
        .map_err(|error| UnavailableReason::preparation_failed(error.to_string()))?;
    Ok(bundle)
}

fn resolve_bundle() -> Result<PathBuf, UnavailableReason> {
    if !cfg!(windows) {
        return Err(UnavailableReason::PlatformUnsupported);
    }
    let exe = std::env::current_exe()
        .map_err(|error| UnavailableReason::BundleMissing(format!("current_exe: {error}")))?;
    let parent = exe
        .parent()
        .ok_or_else(|| UnavailableReason::BundleMissing("executable has no parent".into()))?;
    let generation = std::env::var(bundle_location::GENERATION_ENV).ok();
    let error = std::env::var(bundle_location::PREPARATION_ERROR_ENV).ok();
    let bundle =
        resolve_bundle_at(parent, generation.as_deref(), error.as_deref()).map_err(|mut why| {
            if let UnavailableReason::BundlePreparationFailed {
                rejected_generation,
                ..
            } = &mut why
            {
                *rejected_generation = std::env::var(bundle_location::REJECTED_GENERATION_ENV).ok();
            }
            why
        })?;
    check_bundle_cpu(bundle)
}

fn check_bundle_cpu(bundle: PathBuf) -> Result<PathBuf, UnavailableReason> {
    if !(std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma")) {
        return Err(UnavailableReason::CpuUnsupported);
    }
    Ok(bundle)
}

fn resolve_bundle_for_retry(rejected: Option<&str>) -> Result<PathBuf, UnavailableReason> {
    let exe = std::env::current_exe()
        .map_err(|error| UnavailableReason::preparation_failed(format!("current_exe: {error}")))?;
    let parent = exe
        .parent()
        .ok_or_else(|| UnavailableReason::preparation_failed("executable has no parent"))?;
    check_bundle_cpu(resolve_retry_at(parent, rejected)?)
}

fn resolve_retry_at(parent: &Path, rejected: Option<&str>) -> Result<PathBuf, UnavailableReason> {
    // Retry must consume a new published generation, never the tree whose
    // integrity the launcher rejected, and never the development fallback.
    let generation = bundle_location::read_pointer(&parent.join("effetune"))
        .map_err(|error| UnavailableReason::preparation_failed(error.to_string()))?;
    if rejected == Some(generation.as_str()) {
        return Err(UnavailableReason::preparation_failed(
            "the rejected EffeTune generation has not been repaired yet",
        ));
    }
    resolve_bundle_at(parent, Some(&generation), None)
}
fn remote_playback_snapshot(
    source: &Mutex<Option<crate::remote_ipc::session::SessionHandle>>,
) -> (bool, Option<u64>) {
    let handle = source.lock().unwrap().clone();
    handle.map_or((false, None), |handle| {
        let snapshot = handle.snapshot();
        (
            snapshot.phase.blocks_local_control(),
            Some(snapshot.acquisition_sequence),
        )
    })
}

fn remote_playback_active(
    source: &Mutex<Option<crate::remote_ipc::session::SessionHandle>>,
) -> bool {
    remote_playback_snapshot(source).0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShowPermit {
    minimize_sequence: u64,
    remote_acquisition: Option<u64>,
}

impl ShowPermit {
    fn allows(self, current: Self, remote: bool, minimized: bool) -> bool {
        self == current && !remote && !minimized
    }
}

fn show_permit(
    window: &window::MainWindowObserver,
    remote: &Mutex<Option<crate::remote_ipc::session::SessionHandle>>,
) -> ShowPermit {
    ShowPermit {
        minimize_sequence: window.minimize_sequence(),
        remote_acquisition: remote_playback_snapshot(remote).1,
    }
}

struct LoadDone {
    bridge: Arc<DspBridge>,
}

enum LoadCompletion {
    Loaded {
        bundle: PathBuf,
        result: Result<Option<LoadDone>, EffetuneFailure>,
    },
    Unavailable(UnavailableReason),
    Failed(EffetuneFailure),
}

fn complete_load(
    bundle: Result<PathBuf, UnavailableReason>,
    load: impl FnOnce(&Path) -> Result<Option<LoadDone>, EffetuneFailure>,
) -> LoadCompletion {
    match bundle {
        Ok(bundle) => {
            let result = load(&bundle);
            LoadCompletion::Loaded { bundle, result }
        }
        Err(reason) => LoadCompletion::Unavailable(reason),
    }
}

pub struct EffetuneController {
    main_window: Arc<window::MainWindowObserver>,
    remote_session: Arc<Mutex<Option<crate::remote_ipc::session::SessionHandle>>>,
    // Edge notification only. Worker presentation reads the canonical handle.
    remote_session_notified: bool,
    pub runtime: EffetuneRuntime,
    pub slot: Arc<EffetuneAudioSlot>,
    bridge: Option<Arc<DspBridge>>,
    bundle_path: Option<PathBuf>,
    captures: Arc<CaptureQueue>,
    pending_load: Option<mpsc::Receiver<LoadCompletion>>,
    next_audio_generation: u64,
    pending_capture: Option<mpsc::Receiver<Result<Vec<u8>, CaptureError>>>,
    failure_rx: mpsc::Receiver<EffetuneFailure>,
    gui_failure_tx: mpsc::Sender<GuiFailure>,
    gui_failure_rx: mpsc::Receiver<GuiFailure>,
    host_tx: mpsc::Sender<HostCommand>,
    pending_hide: Option<mpsc::Receiver<Result<(), String>>>,
    repaint_context: Arc<Mutex<Option<egui::Context>>>,
}

fn wake_ui(context: &Mutex<Option<egui::Context>>) {
    if let Some(ctx) = context.lock().unwrap().as_ref() {
        ctx.request_repaint();
    }
}

impl EffetuneController {
    pub fn new() -> Self {
        let bundle = resolve_bundle();
        if let Err(reason) = &bundle {
            crate::logger::log(format!("[EffeTune] unavailable: {reason:?}"));
        }
        let (failure_tx, failure_rx) = mpsc::channel();
        let (gui_failure_tx, gui_failure_rx) = mpsc::channel();
        let (host_tx, host_rx) = mpsc::channel::<HostCommand>();
        let repaint_context = Arc::new(Mutex::new(None));
        let remote_session = Arc::new(Mutex::new(None));
        let main_window = Arc::new(window::MainWindowObserver::new(host_tx.clone()));
        let host_main_window = Arc::downgrade(&main_window);
        let host_remote_session = Arc::downgrade(&remote_session);
        let host_repaint = Arc::clone(&repaint_context);
        let host_failure_tx = failure_tx.clone();
        std::thread::Builder::new()
            .name("effetune-host-control".into())
            .spawn(move || {
                let mut presentation_bridge = std::sync::Weak::<DspBridge>::new();
                while let Ok(command) = host_rx.recv() {
                    match command {
                        HostCommand::RegisterBridge(bridge) => presentation_bridge = bridge,
                        HostCommand::ReconcileVisibility => {
                            if let Some(bridge) = presentation_bridge.upgrade() { bridge.sync_main_window_visibility(); }
                        }
                        HostCommand::Disable(bridge) => { presentation_bridge = std::sync::Weak::new(); bridge.disable(); },
                        HostCommand::Show { bridge, permit } => {
                            let Some(host_main_window) = host_main_window.upgrade() else { continue; };
                            let Some(host_remote_session) = host_remote_session.upgrade() else { continue; };
                            let result = (|| {
                                if !permit.allows(show_permit(&host_main_window, &host_remote_session), remote_playback_active(&host_remote_session), bridge.main_window_is_minimized()) { return Ok(()); }
                                bridge.attach_slot_gui_hidden(0)?;
                                let remote = remote_playback_active(&host_remote_session);
                                bridge.set_slot_gui_remote_session_checked(0, remote)?;
                                // An editor attached while Remote acquired control remains hidden.
                                // It was never visible, so release must not open it later.
                                if permit.allows(show_permit(&host_main_window, &host_remote_session), remote, bridge.main_window_is_minimized()) {
                                    bridge.show_slot_gui_checked(0, permit.minimize_sequence, gui_gate::GuiGate::remote_token(permit.remote_acquisition))?;
                                }
                                Ok::<_, String>(())
                            })();
                            if let Err(error) = result {
                                let _ = host_failure_tx.send(EffetuneFailure::GuiFailed(error));
                            }
                            wake_ui(&host_repaint);
                        }
                        HostCommand::Gui { bridge, value } => {
                            let result = (|| {
                                if matches!(value["cmd"].as_str(), Some("sync_gui_main_visibility" | "activate_gui")) {
                                    bridge.send_value(&serde_json::json!({
                                        "cmd": "set_gui_remote_session",
                                        "slot_id": value["slot_id"],
                                        "active": u32::from(host_remote_session.upgrade().is_some_and(|source| remote_playback_active(&source))),
                                    }))?;
                                }
                                bridge.send_value(&value)
                            })();
                            if let Err(error) = result {
                                let _ = host_failure_tx.send(EffetuneFailure::GuiFailed(format!(
                                    "GUI command failed: {error}"
                                )));
                                wake_ui(&host_repaint);
                            }
                        }
                        HostCommand::Hide { bridge, reply } => {
                            let _ = reply.send(bridge.hide_slot_gui_checked(0));
                            wake_ui(&host_repaint);
                        }
                    }
                }
            })
            .expect("EffeTune host control worker spawn");
        let runtime = match &bundle {
            Ok(_) => EffetuneRuntime::Idle,
            Err(reason) => EffetuneRuntime::Unavailable(reason.clone()),
        };
        Self {
            main_window,
            remote_session,
            remote_session_notified: false,
            runtime,
            slot: Arc::new(EffetuneAudioSlot {
                current: Mutex::new(None),
                failure_tx: Mutex::new(Some(failure_tx)),
                failed_generation: AtomicU64::new(0),
            }),
            bridge: None,
            bundle_path: bundle.ok(),
            captures: Arc::new(CaptureQueue::new(
                crate::data_dir::get()
                    .join("effetune")
                    .join("mixwright-state.json"),
            )),
            pending_load: None,
            next_audio_generation: 0,
            pending_capture: None,
            failure_rx,
            gui_failure_tx,
            gui_failure_rx,
            host_tx,
            pending_hide: None,
            repaint_context,
        }
    }

    pub fn set_main_hwnd(&self, hwnd: u64) {
        self.main_window.install(hwnd);
    }

    pub(crate) fn set_remote_session_source(
        &self,
        handle: Option<crate::remote_ipc::session::SessionHandle>,
    ) {
        let mut source = self.remote_session.lock().unwrap();
        if let Some(previous) = source.take() {
            previous.set_gui_gate(None);
        }
        if let Some(handle) = &handle {
            if let Ok(gate) = self.main_window.gate() {
                handle.set_gui_gate(Some(gate));
            }
        }
        *source = handle;
    }

    pub fn set_remote_session(&mut self, active: bool) {
        if std::mem::replace(&mut self.remote_session_notified, active) == active {
            return;
        }
        if active
            && let EffetuneRuntime::Loading {
                open_gui_when_ready,
                ..
            } = &mut self.runtime
        {
            *open_gui_when_ready = None;
        }
        let _ = self.host_tx.send(HostCommand::ReconcileVisibility);
    }

    pub fn request_show_gui(&self) {
        self.request_show_gui_with_permit(show_permit(&self.main_window, &self.remote_session));
    }

    pub fn request_show_gui_with_permit(&self, permit: ShowPermit) {
        if remote_playback_active(&self.remote_session) {
            return;
        }
        if let Some(bridge) = self.bridge.as_ref() {
            let _ = self.host_tx.send(HostCommand::Show {
                bridge: Arc::clone(bridge),
                permit,
            });
        }
    }

    pub fn click_foreground(&self, pointer_click: bool) -> u64 {
        self.main_window.click_foreground(pointer_click)
    }

    pub fn set_repaint_context(&self, ctx: &egui::Context) {
        let mut context = self.repaint_context.lock().unwrap();
        if context.is_none() {
            *context = Some(ctx.clone());
        }
    }

    pub fn startup(&mut self, pos: Option<(i32, i32)>, size: Option<(u32, u32)>) {
        self.start_load(LoadOrigin::Startup, false, pos, size);
    }

    pub fn click_idle(&mut self, pos: Option<(i32, i32)>, size: Option<(u32, u32)>) {
        self.start_load(LoadOrigin::UserButton, true, pos, size);
    }

    fn start_load(
        &mut self,
        origin: LoadOrigin,
        open_gui_when_ready: bool,
        pos: Option<(i32, i32)>,
        size: Option<(u32, u32)>,
    ) {
        if !self.runtime.can_start_load() || self.pending_load.is_some() {
            return;
        }
        let retry = match &self.runtime {
            EffetuneRuntime::Idle => None,
            EffetuneRuntime::Unavailable(reason) if reason.preparation_retryable() => {
                Some(reason.clone())
            }
            _ => return,
        };
        if let Err(error) = self.main_window.gate() {
            self.fail(EffetuneFailure::GuiFailed(format!(
                "presentation gate: {error}"
            )));
            return;
        }
        let bundle = self.bundle_path.clone();
        self.runtime = EffetuneRuntime::Loading {
            origin,
            open_gui_when_ready: open_gui_when_ready
                .then(|| show_permit(&self.main_window, &self.remote_session)),
        };
        let captures = Arc::clone(&self.captures);
        let gui_failure_tx = self.gui_failure_tx.clone();
        let host_tx = self.host_tx.clone();
        let repaint_context = Arc::clone(&self.repaint_context);
        let path = crate::data_dir::get()
            .join("effetune")
            .join("mixwright-state.json");
        let (tx, rx) = mpsc::channel();
        self.pending_load = Some(rx);
        let spawn = std::thread::Builder::new()
            .name("effetune-load".into())
            .spawn(move || {
                let bundle = if let Some(original) = retry {
                    let UnavailableReason::BundlePreparationFailed {
                        ref rejected_generation,
                        ..
                    } = original
                    else {
                        unreachable!()
                    };
                    match resolve_bundle_for_retry(rejected_generation.as_deref()) {
                        Ok(path) => Ok(path),
                        Err(error)
                            if matches!(
                                error,
                                UnavailableReason::CpuUnsupported
                                    | UnavailableReason::PlatformUnsupported
                            ) =>
                        {
                            Err(error)
                        }
                        Err(error) => {
                            crate::logger::log(format!(
                                "[EffeTune] preparation retry unavailable: {error:?}"
                            ));
                            Err(original)
                        }
                    }
                } else {
                    bundle.ok_or_else(|| {
                        UnavailableReason::BundleMissing("resolved bundle path missing".into())
                    })
                };
                let result = complete_load(bundle, |bundle| {
                    load_worker(
                        bundle,
                        &path,
                        &captures,
                        origin,
                        pos,
                        size,
                        gui_failure_tx,
                        host_tx,
                        repaint_context.clone(),
                    )
                });
                let _ = tx.send(result);
                wake_ui(&repaint_context);
            });
        if let Err(error) = spawn {
            self.pending_load = None;
            self.fail(EffetuneFailure::LoadFailed(format!(
                "load worker spawn: {error}"
            )));
        }
    }

    pub fn startup_pending(&self) -> bool {
        matches!(
            self.runtime,
            EffetuneRuntime::Loading {
                origin: LoadOrigin::Startup,
                ..
            }
        )
    }

    pub fn has_pending_ui_work(&self) -> bool {
        self.pending_load.is_some() || self.pending_hide.is_some() || self.pending_capture.is_some()
    }

    #[cfg(test)]
    pub(crate) fn set_test_startup_completion(&mut self, result: Result<(), EffetuneFailure>) {
        let (tx, rx) = mpsc::channel();
        self.pending_load = Some(rx);
        tx.send(match result {
            Ok(()) => LoadCompletion::Loaded {
                bundle: self.bundle_path.clone().unwrap_or_default(),
                result: Ok(None),
            },
            Err(error) => LoadCompletion::Failed(error),
        })
        .unwrap();
    }

    pub fn poll(&mut self) -> bool {
        if let Ok(GuiFailure::Attach(detail)) = self.gui_failure_rx.try_recv() {
            self.fail(EffetuneFailure::GuiFailed(detail));
        }
        if let Ok(failure) = self.failure_rx.try_recv() {
            self.fail(failure);
        }
        if let Some(rx) = self.pending_hide.as_ref() {
            let completed = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("hide GUI worker disconnected".into()))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = completed {
                self.pending_hide = None;
                match result {
                    Ok(()) if matches!(self.runtime, EffetuneRuntime::Running { .. }) => {
                        self.capture_on_hide();
                    }
                    Ok(()) => {}
                    Err(error) => self.fail(EffetuneFailure::GuiFailed(error)),
                }
            }
        }
        if let Some(rx) = self.pending_load.as_ref() {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(LoadCompletion::Failed(
                    EffetuneFailure::LoadFailed("load worker disconnected".into()),
                )),
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                self.pending_load = None;
                match result {
                    LoadCompletion::Loaded { bundle, result } => {
                        self.bundle_path = Some(bundle);
                        match result {
                            Ok(Some(done)) => self.publish_running(done.bridge),
                            Ok(None) => self.runtime = EffetuneRuntime::Idle,
                            Err(error) => self.fail(error),
                        }
                    }
                    LoadCompletion::Unavailable(reason) => {
                        crate::logger::log(format!("[EffeTune] unavailable: {reason:?}"));
                        self.runtime = EffetuneRuntime::Unavailable(reason);
                    }
                    LoadCompletion::Failed(error) => self.fail(error),
                }
                return true;
            }
        }
        if let Some(rx) = self.pending_capture.as_ref() {
            match rx.try_recv() {
                Ok(Ok(_)) => self.pending_capture = None,
                Ok(Err(error)) => {
                    self.pending_capture = None;
                    if matches!(error, CaptureError::HostExited { .. }) {
                        self.fail(EffetuneFailure::HostLost(error.to_string()));
                    } else {
                        crate::logger::log(format!("[EffeTune] hide capture: {error}"));
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending_capture = None;
                    crate::logger::log("[EffeTune] capture worker disconnected");
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        false
    }

    pub fn capture_on_hide(&mut self) {
        if let Some(bridge) = self.bridge.as_ref() {
            match self.captures.request(Arc::clone(bridge)) {
                Ok(rx) => self.pending_capture = Some(rx),
                Err(error) => crate::logger::log(format!("[EffeTune] hide capture: {error}")),
            }
        }
    }

    pub fn request_hide_gui(&mut self) {
        if self.pending_hide.is_some() || !matches!(self.runtime, EffetuneRuntime::Running { .. }) {
            return;
        }
        let Some(bridge) = self.bridge.as_ref().cloned() else {
            return;
        };
        let (reply, rx) = mpsc::channel();
        if self
            .host_tx
            .send(HostCommand::Hide { bridge, reply })
            .is_err()
        {
            self.fail(EffetuneFailure::GuiFailed(
                "host control worker disconnected".into(),
            ));
            return;
        }
        self.pending_hide = Some(rx);
    }

    pub fn effective_state(&self) -> Option<EffectiveState> {
        self.captures.latest_effective_state()
    }

    pub fn bridge(&self) -> Option<&Arc<DspBridge>> {
        self.bridge.as_ref()
    }

    fn publish_running(&mut self, bridge: Arc<DspBridge>) {
        if let Ok(gate) = self.main_window.gate() {
            bridge.set_gui_gate(gate);
        }
        let _ = self
            .host_tx
            .send(HostCommand::RegisterBridge(Arc::downgrade(&bridge)));
        #[cfg(windows)]
        {
            let weak = Arc::downgrade(&bridge);
            let failure_tx = self.slot.failure_tx.lock().unwrap().as_ref().cloned();
            let repaint_context = Arc::clone(&self.repaint_context);
            let spawn = std::thread::Builder::new()
                .name("effetune-host-monitor".into())
                .spawn(move || {
                    while let Some(bridge) = weak.upgrade() {
                        if !bridge.is_enabled() {
                            break;
                        }
                        if let Some(code) = bridge.host_exit_code_after(Duration::from_secs(1)) {
                            if let Some(tx) = failure_tx.as_ref() {
                                let reason = if code
                                    == crate::video::dsp::bridge::STATE_WATCHDOG_EXIT_CODE
                                {
                                    "state watchdog expired".to_string()
                                } else {
                                    format!("host exited with code {code:#x}")
                                };
                                let _ = tx.send(EffetuneFailure::HostLost(reason));
                                wake_ui(&repaint_context);
                            }
                            break;
                        }
                    }
                });
            if let Err(error) = spawn {
                self.bridge = Some(bridge);
                self.fail(EffetuneFailure::LoadFailed(format!(
                    "host monitor worker spawn: {error}"
                )));
                return;
            }
        }
        self.next_audio_generation += 1;
        self.slot
            .publish(self.next_audio_generation, Arc::clone(&bridge));
        self.bridge = Some(bridge);
        self.runtime = EffetuneRuntime::Running {
            generation: self.next_audio_generation,
        };
    }

    pub fn bundle_path(&self) -> Option<&Path> {
        self.bundle_path.as_deref()
    }

    pub fn fail(&mut self, failure: EffetuneFailure) {
        if matches!(self.runtime, EffetuneRuntime::Failed(_)) {
            return;
        }
        crate::logger::log(format!("[EffeTune] failed: {failure:?}"));
        self.slot.clear();
        self.pending_load = None;
        self.pending_capture = None;
        self.pending_hide = None;
        if let Some(bridge) = self.bridge.take() {
            if let Err(error) = self.host_tx.send(HostCommand::Disable(bridge)) {
                if let HostCommand::Disable(bridge) = error.0 {
                    std::thread::spawn(move || bridge.disable());
                }
            }
        }
        self.runtime = EffetuneRuntime::Failed(failure);
    }

    pub fn begin_exit_capture(&self) -> Option<ExitCaptureFence> {
        self.bridge
            .as_ref()
            .and_then(|bridge| self.captures.begin_final_capture(Arc::clone(bridge)))
    }

    pub fn finish_for_exit(&self, fence: Option<ExitCaptureFence>) {
        if let Some(fence) = fence {
            CaptureQueue::wait_final_capture(fence);
        }
    }
}

pub(crate) fn new_bridge() -> Arc<DspBridge> {
    DspBridge::new_with_gui_chrome(
        GuiOwnerPolicy::Unowned,
        LatencyPolicy::ReportOnly,
        true,
        false,
    )
}

fn load_worker(
    bundle: &Path,
    path: &Path,
    captures: &CaptureQueue,
    origin: LoadOrigin,
    pos: Option<(i32, i32)>,
    size: Option<(u32, u32)>,
    gui_failure_tx: mpsc::Sender<GuiFailure>,
    host_tx: mpsc::Sender<HostCommand>,
    repaint_context: Arc<Mutex<Option<egui::Context>>>,
) -> Result<Option<LoadDone>, EffetuneFailure> {
    let saved = match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(EffetuneFailure::LoadFailed(format!("state read: {error}"))),
    };
    if origin == LoadOrigin::Startup && !startup_state_requires_load(saved.as_deref()) {
        return Ok(None);
    }
    if saved.as_ref().is_some_and(Vec::is_empty) {
        return Err(EffetuneFailure::RestoreFailed(
            "saved state is empty".into(),
        ));
    }
    let bridge = new_bridge();
    bridge.set_gui_failure_sink(gui_failure_tx.clone());
    let gui_repaint = Arc::clone(&repaint_context);
    bridge.set_gui_result_wake(Arc::new(move || wake_ui(&gui_repaint)));
    bridge.set_gui_command_dispatch(Arc::new(move |bridge, value| {
        if host_tx.send(HostCommand::Gui { bridge, value }).is_err() {
            let _ = gui_failure_tx.send(GuiFailure::Attach(
                "host control worker disconnected".into(),
            ));
            wake_ui(&repaint_context);
        }
    }));
    bridge.enable().map_err(EffetuneFailure::LoadFailed)?;
    let state = saved
        .as_ref()
        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes));
    let sample_rate = crate::video::audio::default_output_sample_rate().unwrap_or(48_000);
    let path_string = bundle.to_string_lossy();
    bridge
        .add_plugin(
            &path_string,
            sample_rate,
            480,
            false,
            true,
            state.as_deref(),
            pos,
            size,
        )
        .map_err(|error| {
            if state.is_some() && error.contains("restore_state") {
                EffetuneFailure::RestoreFailed(error)
            } else {
                EffetuneFailure::LoadFailed(error)
            }
        })?;
    bridge
        .try_reset_plugins_sync()
        .map_err(EffetuneFailure::ProcessFailed)?;
    let rx = captures
        .request(Arc::clone(&bridge))
        .map_err(|error| EffetuneFailure::LoadFailed(error.to_string()))?;
    match rx.recv_timeout(INITIAL_CAPTURE_WAIT) {
        Ok(Ok(_)) => {}
        Ok(Err(CaptureError::HostExited { watchdog })) => {
            return Err(EffetuneFailure::HostLost(format!(
                "initial capture: {}",
                CaptureError::HostExited { watchdog }
            )));
        }
        Ok(Err(error)) => {
            return Err(EffetuneFailure::LoadFailed(format!(
                "initial capture: {error}"
            )));
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            return Err(EffetuneFailure::LoadFailed(format!(
                "initial capture: {}",
                CaptureError::CallerDeadline
            )));
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            return Err(EffetuneFailure::LoadFailed(
                "initial capture worker disconnected".into(),
            ));
        }
    }
    Ok(Some(LoadDone { bridge }))
}

fn startup_state_requires_load(saved: Option<&[u8]>) -> bool {
    saved.is_some_and(|bytes| EffectiveState::from_bytes(bytes) != EffectiveState::Inert)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_button_decides_hidden_front_and_behind() {
        assert_eq!(gui_button_action(false, false), GuiButtonAction::Show);
        assert_eq!(gui_button_action(false, true), GuiButtonAction::Show);
        assert_eq!(gui_button_action(true, true), GuiButtonAction::Hide);
        assert_eq!(gui_button_action(true, false), GuiButtonAction::Activate);
    }

    #[test]
    fn pending_show_is_revoked_by_current_or_completed_suppression() {
        let permit = ShowPermit {
            minimize_sequence: 1,
            remote_acquisition: Some(2),
        };
        assert!(permit.allows(permit, false, false));
        assert!(!permit.allows(permit, true, false));
        assert!(!permit.allows(permit, false, true));
        // A complete hide/restore interval during attach still revokes this open.
        assert!(!permit.allows(
            ShowPermit {
                minimize_sequence: 2,
                ..permit
            },
            false,
            false
        ));
        assert!(!permit.allows(
            ShowPermit {
                remote_acquisition: Some(3),
                ..permit
            },
            false,
            false
        ));
        assert!(!permit.allows(
            ShowPermit {
                remote_acquisition: None,
                ..permit
            },
            false,
            false
        ));
    }

    #[test]
    fn remote_acquisition_cancels_pending_open_and_blocks_show() {
        let mut controller = EffetuneController::new();
        let handle = crate::remote_ipc::session::SessionHandle::new();
        controller.set_remote_session_source(Some(handle.clone()));
        handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
            client_id: "phone".into(),
            peer: mimageviewer_ipc::SessionPeerInfo {
                connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                device_name: None,
            },
        });
        let (tx, rx) = mpsc::channel();
        controller.host_tx = tx;
        controller.bridge = Some(DspBridge::new());
        controller.runtime = EffetuneRuntime::Loading {
            origin: LoadOrigin::UserButton,
            open_gui_when_ready: Some(show_permit(
                &controller.main_window,
                &controller.remote_session,
            )),
        };
        controller.set_remote_session(true);
        assert!(matches!(
            rx.try_recv(),
            Ok(HostCommand::ReconcileVisibility)
        ));
        assert!(matches!(
            controller.runtime,
            EffetuneRuntime::Loading {
                open_gui_when_ready: None,
                ..
            }
        ));
        controller.request_show_gui();
        assert!(rx.try_recv().is_err());
        let generation = handle.snapshot().generation;
        assert!(handle.abort_acquire_barrier(generation));
        controller.request_show_gui();
        assert!(rx.try_recv().is_err()); // Drain still holds playback.
        assert!(handle.complete_app_drain(generation));
        controller.set_remote_session(false);
        assert!(matches!(
            rx.try_recv(),
            Ok(HostCommand::ReconcileVisibility)
        ));
        // Release itself never requests a previously hidden editor to open.
        assert!(rx.try_recv().is_err());
        controller.request_show_gui();
        assert!(matches!(rx.try_recv(), Ok(HostCommand::Show { .. })));
    }

    #[test]
    fn worker_remote_reader_observes_acquisition_before_ui_notification() {
        let mut controller = EffetuneController::new();
        let handle = crate::remote_ipc::session::SessionHandle::new();
        controller.set_remote_session_source(Some(handle.clone()));
        assert!(!remote_playback_active(&controller.remote_session));
        handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
            client_id: "phone".into(),
            peer: mimageviewer_ipc::SessionPeerInfo {
                connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                device_name: None,
            },
        });
        assert!(!controller.remote_session_notified);
        assert!(remote_playback_active(&controller.remote_session));
        let gate = controller.main_window.gate().unwrap();
        assert_eq!(gate.remote() & 1, 1);
        let issued = gui_gate::GuiGate::remote_token(Some(0));
        let generation = handle.snapshot().generation;
        assert!(handle.abort_acquire_barrier(generation));
        assert!(handle.complete_app_drain(generation));
        // The GUI task sees the completed acquisition despite no UI notification.
        assert_ne!(gate.remote(), issued);
        assert_eq!(gate.remote() & 1, 0);
        controller.set_remote_session_source(None);
        assert_eq!(gate.remote(), 0);
        handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
            client_id: "old-detached-source".into(),
            peer: mimageviewer_ipc::SessionPeerInfo {
                connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                device_name: None,
            },
        });
        assert_eq!(gate.remote(), 0); // Old source cannot publish into a later binding.
        assert!(!remote_playback_active(&controller.remote_session));
        // A detached source cannot keep suppression latched on another session.
        controller.set_remote_session(false);
    }

    // Fixture shape: Frieve-A/effetune-mixwright v0.12.0,
    // src/bridge/state_codec.cpp, StateCodec::encode. The formatVersion=1
    // shape was observed with v0.11.1; appVersion is informational to mIV.
    // No v0.12.0 product/plugin launch is claimed by this fixture.
    fn fixture(a: &str, b: &str, current: &str, bypass: bool) -> Vec<u8> {
        format!(r#"{{"appVersion":"0.12.0","formatVersion":1,"pipelineA":{a},"pipelineB":{b},"currentPipeline":"{current}","masterBypass":{bypass},"oversampling":{{"factor":1,"phase":"linear","quality":"medium"}},"ui":{{"columns":1,"zoom":1}}}}"#).into_bytes()
    }

    #[test]
    fn effective_state_follows_selected_pipeline_and_section_gate() {
        let effect = r#"[{"name":"Gain","enabled":true}]"#;
        let disabled_section =
            r#"[{"name":"Section","enabled":false},{"name":"Gain","enabled":true}]"#;
        assert_eq!(
            EffectiveState::from_bytes(&fixture("[]", "[]", "A", false)),
            EffectiveState::Inert
        );
        assert_eq!(
            EffectiveState::from_bytes(&fixture("[]", "null", "A", false)),
            EffectiveState::Inert
        );
        assert_eq!(
            EffectiveState::from_bytes(&fixture(effect, "[]", "A", true)),
            EffectiveState::Inert
        );
        assert_eq!(
            EffectiveState::from_bytes(&fixture(effect, "[]", "A", false)),
            EffectiveState::Effective
        );
        assert_eq!(
            EffectiveState::from_bytes(&fixture(effect, "[]", "B", false)),
            EffectiveState::Inert
        );
        assert_eq!(
            EffectiveState::from_bytes(&fixture(disabled_section, "[]", "A", false)),
            EffectiveState::Inert
        );
        assert!(matches!(
            EffectiveState::from_bytes(b"{"),
            EffectiveState::Unparseable(_)
        ));
        assert!(matches!(
            EffectiveState::from_bytes(br#"{"formatVersion":2}"#),
            EffectiveState::Unparseable(_)
        ));
    }

    #[test]
    fn bundle_resolution_never_uses_working_directory_on_exe_error() {
        let error = resolve_bundle_from_exe(Err(std::io::Error::other("unavailable")));
        assert!(
            matches!(error, Err(UnavailableReason::BundleMissing(reason)) if reason.contains("current_exe"))
        );
    }

    #[test]
    fn bundle_resolution_requires_bundle_beside_the_executable() {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("mimageviewer-core.exe");
        assert!(matches!(
            resolve_bundle_from_exe(Ok(exe.clone())),
            Err(UnavailableReason::BundleMissing(_))
        ));
        let bundle = temp.path().join("effetune").join(BUNDLE_NAME);
        std::fs::create_dir_all(&bundle).unwrap();
        assert_eq!(resolve_bundle_from_exe(Ok(exe)).unwrap(), bundle);
    }

    #[test]
    #[cfg(not(feature = "portable"))]
    fn bundle_resolution_pins_the_verified_generation_and_surfaces_preparation_failure() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path();
        let container = parent.join("effetune");
        let first = format!("{}-Abc123", "a".repeat(bundle_location::FINGERPRINT_LENGTH));
        let second = format!("{}-Def456", "b".repeat(bundle_location::FINGERPRINT_LENGTH));
        for name in [&first, &second] {
            std::fs::create_dir_all(container.join(name).join(BUNDLE_NAME)).unwrap();
        }
        std::fs::write(
            container.join(bundle_location::POINTER_FILE),
            bundle_location::encode_pointer(&first).unwrap(),
        )
        .unwrap();
        let resolved = resolve_bundle_at(parent, Some(&first), None).unwrap();
        std::fs::write(
            container.join(bundle_location::POINTER_FILE),
            bundle_location::encode_pointer(&second).unwrap(),
        )
        .unwrap();
        assert_eq!(
            resolve_bundle_at(parent, Some(&first), None).unwrap(),
            resolved
        );
        assert_eq!(
            resolve_bundle_at(parent, None, None).unwrap(),
            container.join(&second).join(BUNDLE_NAME)
        );
        let failure =
            resolve_bundle_at(parent, Some(&first), Some("publish: access denied")).unwrap_err();
        assert!(
            matches!(failure, UnavailableReason::BundlePreparationFailed { ref reason, .. } if reason.contains("access denied"))
        );
        assert!(failure.user_reason().contains("準備できません"));
        // Corrupt pointers must not silently select the old legacy tree.
        std::fs::create_dir_all(container.join(BUNDLE_NAME)).unwrap();
        std::fs::write(
            container.join(bundle_location::POINTER_FILE),
            "effetune-v2\n../outside\n",
        )
        .unwrap();
        assert!(matches!(
            resolve_bundle_at(parent, None, None),
            Err(UnavailableReason::BundlePreparationFailed { .. })
        ));
        assert!(resolve_bundle_at(parent, Some("../outside"), None).is_err());
    }

    #[test]
    #[cfg(not(feature = "portable"))]
    fn preparation_retry_rejects_old_generation_and_consumes_a_later_publication() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path();
        let container = parent.join("effetune");
        std::fs::create_dir_all(container.join(BUNDLE_NAME)).unwrap();
        assert!(resolve_retry_at(parent, None).is_err()); // No legacy fallback.
        let first = format!("{}-Abc123", "a".repeat(bundle_location::FINGERPRINT_LENGTH));
        let second = format!("{}-Def456", "b".repeat(bundle_location::FINGERPRINT_LENGTH));
        for generation in [&first, &second] {
            std::fs::create_dir_all(container.join(generation).join(BUNDLE_NAME)).unwrap();
        }
        std::fs::write(
            container.join(bundle_location::POINTER_FILE),
            bundle_location::encode_pointer(&first).unwrap(),
        )
        .unwrap();
        assert!(resolve_retry_at(parent, Some(&first)).is_err());
        let original = UnavailableReason::BundlePreparationFailed {
            reason: "publisher timeout".into(),
            rejected_generation: Some(first.clone()),
        };
        assert!(original.preparation_retryable());
        assert!(EffetuneRuntime::Unavailable(original.clone()).can_start_load());
        assert!(!EffetuneRuntime::Unavailable(UnavailableReason::CpuUnsupported).can_start_load());
        let mut controller = EffetuneController::new();
        let (tx, rx) = mpsc::channel();
        controller.pending_load = Some(rx);
        controller.runtime = EffetuneRuntime::Loading {
            origin: LoadOrigin::UserButton,
            open_gui_when_ready: None,
        };
        tx.send(complete_load(Err(original.clone()), |_| {
            panic!("unavailable preparation must never invoke the host loader")
        }))
        .unwrap();
        assert!(controller.poll());
        assert_eq!(controller.runtime, EffetuneRuntime::Unavailable(original));
        assert!(controller.bundle_path().is_none());
        std::fs::write(
            container.join(bundle_location::POINTER_FILE),
            bundle_location::encode_pointer(&second).unwrap(),
        )
        .unwrap();
        let published = container.join(&second).join(BUNDLE_NAME);
        let (tx, rx) = mpsc::channel();
        controller.pending_load = Some(rx);
        controller.runtime = EffetuneRuntime::Loading {
            origin: LoadOrigin::Startup,
            open_gui_when_ready: None,
        };
        tx.send(complete_load(
            resolve_retry_at(parent, Some(&first)),
            |path| {
                assert_eq!(path, published);
                Ok(None) // An inert startup state, without constructing a VST host.
            },
        ))
        .unwrap();
        assert!(controller.poll());
        assert_eq!(controller.runtime, EffetuneRuntime::Idle);
        assert_eq!(controller.bundle_path(), Some(published.as_path()));
        assert!(!controller.startup_pending());
        let long = UnavailableReason::preparation_failed(format!(
            "{}: fixture",
            bundle_location::PATH_TOO_LONG_MARKER
        ));
        assert!(long.user_reason().contains("パスが長すぎます"));
    }

    #[test]
    fn startup_state_load_rule_and_controller_failure_transitions() {
        assert!(!startup_state_requires_load(None));
        assert!(!startup_state_requires_load(Some(&fixture(
            "[]", "[]", "A", false
        ))));
        assert!(startup_state_requires_load(Some(&fixture(
            r#"[{"name":"Gain","enabled":true}]"#,
            "[]",
            "A",
            false,
        ))));
        assert!(startup_state_requires_load(Some(b"broken state")));

        for failure in [
            EffetuneFailure::LoadFailed("load".into()),
            EffetuneFailure::RestoreFailed("restore".into()),
            EffetuneFailure::ProcessFailed("process".into()),
            EffetuneFailure::LatencyExceeded { total_secs: 2.1 },
            EffetuneFailure::HostLost("host".into()),
            EffetuneFailure::GuiFailed("attach".into()),
        ] {
            let mut controller = EffetuneController::new();
            controller.runtime = EffetuneRuntime::Loading {
                origin: LoadOrigin::Startup,
                open_gui_when_ready: None,
            };
            controller.publish_running(DspBridge::new());
            assert!(controller.slot.snapshot().is_some());
            controller.fail(failure.clone());
            assert_eq!(controller.runtime, EffetuneRuntime::Failed(failure));
            assert!(controller.slot.snapshot().is_none());
        }
    }

    #[test]
    fn state_writer_replaces_without_reusing_temp_name() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("mixwright-state.json");
        write_state_atomic(&state, b"first", |temp, path| fs::rename(temp, path)).unwrap();
        write_state_atomic(&state, b"second", |temp, path| fs::rename(temp, path)).unwrap();
        assert_eq!(fs::read(state).unwrap(), b"second");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn older_capture_cannot_replace_newer_state_or_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixwright-state.json");
        let latest = Mutex::new(LatestCapture::default());
        let publication = PublicationGate::default();
        publish_capture_state(
            &latest,
            &path,
            2,
            b"new",
            EffectiveState::Effective,
            &publication,
        )
        .unwrap();
        publish_capture_state(
            &latest,
            &path,
            1,
            b"old",
            EffectiveState::Inert,
            &publication,
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(latest.lock().unwrap().generation, 2);
        assert_eq!(
            latest.lock().unwrap().effective,
            Some(EffectiveState::Effective)
        );
        let directory_target = dir.path().join("directory-target");
        fs::create_dir(&directory_target).unwrap();
        let error = publish_capture_state(
            &latest,
            &directory_target,
            3,
            b"not written",
            EffectiveState::Inert,
            &publication,
        );
        assert!(error.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(latest.lock().unwrap().generation, 2);
    }

    #[test]
    fn expired_exit_fence_does_not_wait_for_a_stalled_capture() {
        let (_tx, rx) = mpsc::channel();
        let fence = ExitCaptureFence {
            bridge: DspBridge::new(),
            rx,
            deadline: Instant::now(),
            publication: Arc::new(PublicationGate::default()),
        };
        let start = Instant::now();
        CaptureQueue::wait_final_capture(fence);
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn pump_failure_is_reported_once_and_stops_that_generation_immediately() {
        let slot = EffetuneAudioSlot::default();
        let (tx, rx) = mpsc::channel();
        *slot.failure_tx.lock().unwrap() = Some(tx);
        slot.publish(7, DspBridge::new());
        assert!(slot.snapshot().is_some());
        slot.report_failure_once(7, EffetuneFailure::ProcessFailed("first".into()));
        slot.report_failure_once(7, EffetuneFailure::LatencyExceeded { total_secs: 2.5 });
        assert!(slot.snapshot().is_none());
        assert_eq!(
            rx.try_recv().unwrap(),
            EffetuneFailure::ProcessFailed("first".into())
        );
        assert!(rx.try_recv().is_err());
        slot.publish(8, DspBridge::new());
        assert!(slot.snapshot().is_some());
        slot.report_failure_once(7, EffetuneFailure::ProcessFailed("stale".into()));
        assert!(slot.snapshot().is_some());
    }

    #[test]
    fn gui_failure_uses_controller_transition_and_queues_host_cleanup() {
        let mut controller = EffetuneController::new();
        let (host_tx, host_rx) = mpsc::channel();
        controller.host_tx = host_tx;
        controller.publish_running(DspBridge::new());
        controller
            .gui_failure_tx
            .send(GuiFailure::Attach("fixture attach error".into()))
            .unwrap();
        controller.poll();
        assert_eq!(
            controller.runtime,
            EffetuneRuntime::Failed(EffetuneFailure::GuiFailed("fixture attach error".into()))
        );
        assert!(controller.slot.snapshot().is_none());
        assert!(matches!(
            host_rx.try_recv(),
            Ok(HostCommand::RegisterBridge(_))
        ));
        assert!(matches!(host_rx.try_recv(), Ok(HostCommand::Disable(_))));
    }

    #[test]
    fn asynchronous_hide_failure_is_applied_by_controller_poll() {
        let mut controller = EffetuneController::new();
        controller.publish_running(DspBridge::new());
        controller.request_hide_gui();
        assert!(matches!(
            controller.runtime,
            EffetuneRuntime::Running { .. }
        ));
        let result = controller
            .pending_hide
            .take()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(result.is_err());
        let (reply, rx) = mpsc::channel();
        reply.send(result).unwrap();
        controller.pending_hide = Some(rx);
        controller.poll();
        assert!(matches!(
            controller.runtime,
            EffetuneRuntime::Failed(EffetuneFailure::GuiFailed(_))
        ));
        assert!(controller.slot.snapshot().is_none());
    }

    #[test]
    fn exit_fence_rejects_a_capture_waiting_before_file_commit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixwright-state.json");
        fs::write(&path, b"previous").unwrap();
        let publication = Arc::new(PublicationGate::default());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer_gate = Arc::clone(&publication);
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            write_state_atomic(&writer_path, b"late", |temp, path| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                writer_gate.commit_with(|| fs::rename(temp, path))
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        publication.expire();
        release_tx.send(()).unwrap();
        assert_eq!(
            writer.join().unwrap().unwrap_err().kind(),
            std::io::ErrorKind::TimedOut
        );
        assert_eq!(fs::read(&path).unwrap(), b"previous");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn admitted_replacement_does_not_hold_exit_fence_past_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixwright-state.json");
        fs::write(&path, b"previous").unwrap();
        let gate = Arc::new(PublicationGate::default());
        let writer_gate = Arc::clone(&gate);
        let writer_path = path.clone();
        let (decided_tx, decided_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            write_state_atomic(&writer_path, b"committed", |temp, path| {
                writer_gate.commit_with(|| {
                    decided_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    fs::rename(temp, path)
                })
            })
        });
        decided_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let deadline = Instant::now() + Duration::from_millis(30);
        let exit_gate = Arc::clone(&gate);
        let (exit_done_tx, exit_done_rx) = mpsc::channel();
        let exit = std::thread::spawn(move || {
            let started = Instant::now();
            exit_gate.begin_exit(deadline);
            let (_reply_tx, reply_rx) = mpsc::channel();
            CaptureQueue::wait_final_capture(ExitCaptureFence {
                bridge: DspBridge::new(),
                rx: reply_rx,
                deadline,
                publication: exit_gate,
            });
            exit_done_tx.send(started.elapsed()).unwrap();
        });
        let exit_elapsed = exit_done_rx.recv_timeout(Duration::from_millis(200));
        assert_eq!(fs::read(&path).unwrap(), b"previous");
        release_tx.send(()).unwrap();
        writer.join().unwrap().unwrap();
        exit.join().unwrap();
        assert!(exit_elapsed.unwrap() < Duration::from_millis(200));
        assert_eq!(fs::read(&path).unwrap(), b"committed");
        assert_eq!(
            gate.commit_with(|| fs::write(&path, b"too late"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn close_cancels_queued_fake_capture_and_blocks_running_capture_publication() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixwright-state.json");
        fs::write(&path, b"previous").unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let calls = Arc::new(AtomicU64::new(0));
        let closing = Arc::new(AtomicBool::new(false));
        let worker_calls = Arc::clone(&calls);
        let worker_closing = Arc::clone(&closing);
        let queue = CaptureQueue::new_with_capture(path.clone(), move |_| {
            if worker_calls.fetch_add(1, Ordering::AcqRel) == 0 {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(fixture("[]", "[]", "A", false))
            } else if worker_closing.load(Ordering::Acquire) {
                Err(CaptureError::Interrupted("queued capture cancelled".into()))
            } else {
                unreachable!("queued job ran before the running capture completed")
            }
        });
        let bridge = DspBridge::new();
        let running = queue.request(Arc::clone(&bridge)).unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let queued = queue.request(Arc::clone(&bridge)).unwrap();
        let mut fence = queue.begin_final_capture(bridge).unwrap();
        fence.deadline = Instant::now();
        closing.store(true, Ordering::Release);
        CaptureQueue::wait_final_capture(fence);
        release_tx.send(()).unwrap();
        assert!(matches!(
            running.recv_timeout(Duration::from_secs(2)).unwrap(),
            Err(CaptureError::Persistence(_))
        ));
        assert_eq!(
            queued.recv_timeout(Duration::from_secs(2)).unwrap(),
            Err(CaptureError::Interrupted("queued capture cancelled".into()))
        );
        assert_eq!(fs::read(path).unwrap(), b"previous");
    }
}
