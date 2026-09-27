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

pub mod composition;

const BUNDLE_NAME: &str = "EffeTune Mixwright.vst3";
const INITIAL_CAPTURE_WAIT: Duration = Duration::from_secs(6);
const EXIT_FENCE: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq)]
pub enum UnavailableReason {
    BundleMissing(String),
    CpuUnsupported,
    PlatformUnsupported,
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
        open_gui_when_ready: bool,
    },
    Running {
        generation: u64,
    },
    Failed(EffetuneFailure),
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

#[derive(Clone)]
pub struct RemoteCaptureSource {
    queue: Arc<CaptureQueue>,
    bridge: Arc<DspBridge>,
}

impl RemoteCaptureSource {
    pub fn request(&self) -> Result<mpsc::Receiver<Result<Vec<u8>, CaptureError>>, CaptureError> {
        self.queue.request(Arc::clone(&self.bridge))
    }

    #[cfg(test)]
    pub(crate) fn with_fake_capture(
        state_path: PathBuf,
        capture: impl Fn(&DspBridge) -> Result<Vec<u8>, CaptureError> + Send + 'static,
    ) -> Self {
        Self {
            queue: Arc::new(CaptureQueue::new_with_capture(state_path, capture)),
            bridge: DspBridge::new(),
        }
    }
}

#[derive(Default)]
struct PublicationGate {
    exit_deadline: Mutex<Option<Instant>>,
    cancelled: AtomicBool,
}

impl PublicationGate {
    fn begin_exit(&self, deadline: Instant) {
        *self.exit_deadline.lock().unwrap() = Some(deadline);
    }

    fn may_commit(&self) -> bool {
        !self.cancelled.load(Ordering::Acquire)
            && self
                .exit_deadline
                .lock()
                .unwrap()
                .is_none_or(|deadline| Instant::now() < deadline)
    }

    fn expire(&self) {
        self.cancelled.store(true, Ordering::Release);
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
                fence.publication.expire();
                crate::logger::log(
                    "[EffeTune] exit capture fence expired; previous state retained",
                );
                fence.bridge.terminate_host_now();
            }
        }
    }
}

fn capture_once(bridge: &DspBridge) -> Result<Vec<u8>, CaptureError> {
    let rx = bridge
        .query_first_state_concurrent()
        .map_err(|error| classify_capture_bridge_error(bridge, error, false))?;
    let encoded = rx.recv().map_err(|error| {
        classify_capture_bridge_error(
            bridge,
            format!("capture result channel closed: {error}"),
            true,
        )
    })?;
    let encoded = encoded.map_err(|error| match error {
        crate::video::dsp::bridge::ConcurrentStateError::Interrupted(reason) => {
            classify_capture_bridge_error(bridge, reason, true)
        }
        crate::video::dsp::bridge::ConcurrentStateError::HostResponse(reason) => {
            classify_capture_bridge_error(bridge, reason, false)
        }
    })?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| CaptureError::State(format!("state base64 invalid: {error}")))
}

fn classify_capture_bridge_error(
    bridge: &DspBridge,
    error: String,
    interrupted: bool,
) -> CaptureError {
    if !bridge.host_alive() {
        CaptureError::HostExited {
            watchdog: bridge.host_watchdog_expired(),
        }
    } else if interrupted {
        CaptureError::Interrupted(error)
    } else {
        CaptureError::State(error)
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
    write_state_atomic(path, bytes, || publication.may_commit())
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
    may_commit: impl Fn() -> bool,
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
        if !may_commit() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "state publication fence expired",
            ));
        }
        fs::rename(&temp, path)
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
    let bundle = parent.join("effetune").join(BUNDLE_NAME);
    if !bundle.is_dir() {
        return Err(UnavailableReason::BundleMissing(
            bundle.display().to_string(),
        ));
    }
    Ok(bundle)
}

fn resolve_bundle() -> Result<PathBuf, UnavailableReason> {
    if !cfg!(windows) {
        return Err(UnavailableReason::PlatformUnsupported);
    }
    let bundle = resolve_bundle_from_exe(std::env::current_exe())?;
    if !(std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma")) {
        return Err(UnavailableReason::CpuUnsupported);
    }
    Ok(bundle)
}

struct LoadDone {
    bridge: Arc<DspBridge>,
}

pub struct EffetuneController {
    pub runtime: EffetuneRuntime,
    pub slot: Arc<EffetuneAudioSlot>,
    bridge: Option<Arc<DspBridge>>,
    bundle_path: Option<PathBuf>,
    captures: Arc<CaptureQueue>,
    pending_load: Option<mpsc::Receiver<Result<Option<LoadDone>, EffetuneFailure>>>,
    next_audio_generation: u64,
    pending_capture: Option<mpsc::Receiver<Result<Vec<u8>, CaptureError>>>,
    failure_rx: mpsc::Receiver<EffetuneFailure>,
    gui_failure_tx: mpsc::Sender<GuiFailure>,
    gui_failure_rx: mpsc::Receiver<GuiFailure>,
    cleanup_tx: mpsc::Sender<Arc<DspBridge>>,
    gui_rect_tx: mpsc::Sender<(u64, Option<(i32, i32)>, Option<(u32, u32)>)>,
}

impl EffetuneController {
    pub fn new() -> Self {
        let bundle = resolve_bundle();
        let (failure_tx, failure_rx) = mpsc::channel();
        let (gui_failure_tx, gui_failure_rx) = mpsc::channel();
        let (cleanup_tx, cleanup_rx) = mpsc::channel::<Arc<DspBridge>>();
        std::thread::Builder::new()
            .name("effetune-host-cleanup".into())
            .spawn(move || {
                while let Ok(bridge) = cleanup_rx.recv() {
                    bridge.disable();
                }
            })
            .expect("EffeTune cleanup worker spawn");
        let (gui_rect_tx, gui_rect_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("effetune-gui-rect-save".into())
            .spawn(move || {
                while let Ok((generation, pos, size)) = gui_rect_rx.recv() {
                    crate::settings::Settings::persist_effetune_gui_rect(generation, pos, size);
                }
            })
            .expect("EffeTune GUI rectangle worker spawn");
        let runtime = match &bundle {
            Ok(_) => EffetuneRuntime::Idle,
            Err(reason) => EffetuneRuntime::Unavailable(reason.clone()),
        };
        Self {
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
            cleanup_tx,
            gui_rect_tx,
        }
    }

    pub fn startup(&mut self, pos: Option<(i32, i32)>, size: Option<(u32, u32)>) {
        self.start_load(LoadOrigin::Startup, false, pos, size);
    }

    pub fn click_idle(&mut self, pos: Option<(i32, i32)>, size: Option<(u32, u32)>) {
        if self.runtime == EffetuneRuntime::Idle {
            self.start_load(LoadOrigin::UserButton, true, pos, size);
        }
    }

    fn start_load(
        &mut self,
        origin: LoadOrigin,
        open_gui_when_ready: bool,
        pos: Option<(i32, i32)>,
        size: Option<(u32, u32)>,
    ) {
        if !matches!(self.runtime, EffetuneRuntime::Idle) {
            return;
        }
        let Some(bundle) = self.bundle_path.clone() else {
            return;
        };
        self.runtime = EffetuneRuntime::Loading {
            origin,
            open_gui_when_ready,
        };
        let captures = Arc::clone(&self.captures);
        let gui_failure_tx = self.gui_failure_tx.clone();
        let path = crate::data_dir::get()
            .join("effetune")
            .join("mixwright-state.json");
        let (tx, rx) = mpsc::channel();
        self.pending_load = Some(rx);
        let spawn = std::thread::Builder::new()
            .name("effetune-load".into())
            .spawn(move || {
                let result =
                    load_worker(&bundle, &path, &captures, origin, pos, size, gui_failure_tx);
                let _ = tx.send(result);
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

    #[cfg(test)]
    pub(crate) fn set_test_startup_completion(&mut self, result: Result<(), EffetuneFailure>) {
        let (tx, rx) = mpsc::channel();
        self.pending_load = Some(rx);
        tx.send(result.map(|()| None)).unwrap();
    }

    pub fn poll(&mut self) -> bool {
        if let Ok(GuiFailure::Attach(detail)) = self.gui_failure_rx.try_recv() {
            self.fail(EffetuneFailure::GuiFailed(detail));
        }
        if let Ok(failure) = self.failure_rx.try_recv() {
            self.fail(failure);
        }
        #[cfg(windows)]
        if self
            .bridge
            .as_ref()
            .is_some_and(|bridge| !bridge.host_alive())
        {
            self.fail(EffetuneFailure::HostLost("EffeTune host exited".into()));
        }
        if let Some(rx) = self.pending_load.as_ref() {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(EffetuneFailure::LoadFailed(
                    "load worker disconnected".into(),
                ))),
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                self.pending_load = None;
                match result {
                    Ok(Some(done)) => {
                        self.publish_running(done.bridge);
                    }
                    Ok(None) => self.runtime = EffetuneRuntime::Idle,
                    Err(error) => self.fail(error),
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

    pub fn remote_capture_source(&self) -> Result<RemoteCaptureSource, String> {
        let bridge = self.bridge.as_ref().ok_or("EffeTune is not running")?;
        Ok(RemoteCaptureSource {
            queue: Arc::clone(&self.captures),
            bridge: Arc::clone(bridge),
        })
    }

    pub fn effective_state(&self) -> Option<EffectiveState> {
        self.captures.latest_effective_state()
    }

    pub fn bridge(&self) -> Option<&Arc<DspBridge>> {
        self.bridge.as_ref()
    }

    fn publish_running(&mut self, bridge: Arc<DspBridge>) {
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

    pub fn save_gui_rect_async(&self, pos: Option<(i32, i32)>, size: Option<(u32, u32)>) {
        let generation = crate::settings::Settings::next_effetune_gui_rect_generation();
        if self.gui_rect_tx.send((generation, pos, size)).is_err() {
            crate::logger::log("[EffeTune] GUI rectangle writer disconnected");
        }
    }

    pub fn fail(&mut self, failure: EffetuneFailure) {
        if matches!(self.runtime, EffetuneRuntime::Failed(_)) {
            return;
        }
        crate::logger::log(format!("[EffeTune] failed: {failure:?}"));
        self.slot.clear();
        self.pending_capture = None;
        if let Some(bridge) = self.bridge.take() {
            if let Err(error) = self.cleanup_tx.send(bridge) {
                std::thread::spawn(move || error.0.disable());
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

fn load_worker(
    bundle: &Path,
    path: &Path,
    captures: &CaptureQueue,
    origin: LoadOrigin,
    pos: Option<(i32, i32)>,
    size: Option<(u32, u32)>,
    gui_failure_tx: mpsc::Sender<GuiFailure>,
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
    let bridge = DspBridge::new_with_gui_chrome(
        GuiOwnerPolicy::FixedMain,
        LatencyPolicy::ReportOnly,
        true,
        false,
    );
    bridge.set_gui_failure_sink(gui_failure_tx);
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

    // Fixture shape: Frieve-A/effetune-mixwright v0.11.1,
    // src/bridge/state_codec.cpp, StateCodec::encode. Confirmed against bytes
    // returned by the bundled v0.11.1 plug-in through the real host handler.
    fn fixture(a: &str, b: &str, current: &str, bypass: bool) -> Vec<u8> {
        format!(r#"{{"appVersion":"0.11.1","formatVersion":1,"pipelineA":{a},"pipelineB":{b},"currentPipeline":"{current}","masterBypass":{bypass},"oversampling":{{"factor":1,"phase":"linear","quality":"medium"}},"ui":{{"columns":1,"zoom":1}}}}"#).into_bytes()
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
                open_gui_when_ready: false,
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
        write_state_atomic(&state, b"first", || true).unwrap();
        write_state_atomic(&state, b"second", || true).unwrap();
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
        let (cleanup_tx, cleanup_rx) = mpsc::channel();
        controller.cleanup_tx = cleanup_tx;
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
        assert!(cleanup_rx.try_recv().is_ok());
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
            write_state_atomic(&writer_path, b"late", || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                writer_gate.may_commit()
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
    fn remote_caller_deadline_does_not_end_running_local_capture() {
        let dir = tempfile::tempdir().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let queue = Arc::new(CaptureQueue::new_with_capture(
            dir.path().join("mixwright-state.json"),
            move |_| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(fixture("[]", "[]", "A", false))
            },
        ));
        let mut controller = EffetuneController::new();
        controller.captures = queue;
        controller.publish_running(DspBridge::new());
        let capture = controller
            .remote_capture_source()
            .unwrap()
            .request()
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            capture.recv_timeout(Duration::from_millis(10)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(matches!(
            controller.runtime,
            EffetuneRuntime::Running { .. }
        ));
        assert!(controller.slot.snapshot().is_some());
        release_tx.send(()).unwrap();
        assert!(
            capture
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .is_ok()
        );
        assert!(matches!(
            controller.runtime,
            EffetuneRuntime::Running { .. }
        ));
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
