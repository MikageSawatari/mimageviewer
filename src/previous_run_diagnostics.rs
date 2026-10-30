//! Previous GUI-run evidence. Only the admitted main owner may request a clean
//! terminal; dropping an App, owner, or unwinding never manufactures one.
//! All file operations and report formatting belong to the dedicated writer.
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub mod checkpoint;
mod clock;
pub(crate) mod device_loss;
pub(crate) mod fatal;
use crate::gpu_info::diagnostic_sampler::{AdapterProbe, DiagnosticSampler, MemorySample};
use checkpoint::{Checkpoint, CheckpointPublisher, Publication, Schedule, Stamp};

const SCHEMA: u32 = 1;
const MAX_RECORD_BYTES: u64 = 16 * 1024;
const MAX_REFERENCE_BYTES: u64 = 1024;
const RUNNING: u8 = 0;
const RETURNED_OK: u8 = 1;
const RETURNED_ERROR: u8 = 2;
const ABANDONED: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuiRunOutcome {
    ReturnedOk,
    ReturnedError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviousRunReport {
    body: String,
    summary_end: usize,
    details_start: usize,
}
impl PreviousRunReport {
    pub(crate) fn from_sections(summary: String, details: String) -> Self {
        let summary_end = summary.len();
        let mut body = summary;
        body.push_str("\n\n詳細（報告用）\n");
        let details_start = body.len();
        body.push_str(&details);
        Self {
            body,
            summary_end,
            details_start,
        }
    }
    pub(crate) fn summary(&self) -> &str {
        &self.body[..self.summary_end]
    }
    pub(crate) fn details(&self) -> &str {
        &self.body[self.details_start..]
    }
    /// Copies both sections, including details while their UI is collapsed.
    pub fn body(&self) -> &str {
        &self.body
    }
}

struct Shared {
    state: AtomicU8,
    writer: OnceLock<std::thread::Thread>,
    report: OnceLock<PreviousRunReport>,
    report_taken: AtomicBool,
    initialized: AtomicBool,
    repaint: OnceLock<egui::Context>,
    completion: AtomicU8,
    checkpoint: Publication,
    device_loss: device_loss::Registry,
    #[cfg(test)]
    ready: (std::sync::Mutex<bool>, std::sync::Condvar),
    #[cfg(test)]
    after_panic_sink: OnceLock<Arc<std::sync::Barrier>>,
}
impl Shared {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(RUNNING),
            writer: OnceLock::new(),
            report: OnceLock::new(),
            report_taken: AtomicBool::new(false),
            initialized: AtomicBool::new(false),
            repaint: OnceLock::new(),
            completion: AtomicU8::new(0),
            checkpoint: Publication::new(),
            device_loss: device_loss::Registry::new(),
            #[cfg(test)]
            ready: (std::sync::Mutex::new(false), std::sync::Condvar::new()),
            #[cfg(test)]
            after_panic_sink: OnceLock::new(),
        }
    }
    fn wake(&self) {
        if let Some(writer) = self.writer.get() {
            writer.unpark();
        }
    }
    fn initialized(&self) {
        if self.initialized.swap(true, Ordering::AcqRel) {
            return;
        }
        #[cfg(test)]
        {
            *self.ready.0.lock().unwrap() = true;
            self.ready.1.notify_all();
        }
        if let Some(context) = self.repaint.get() {
            context.request_repaint();
        }
    }
}

#[derive(Clone)]
pub struct PreviousRunReportHandle(Arc<Shared>);
impl PreviousRunReportHandle {
    /// Register once, without file I/O or waiting for initialization. Either
    /// side of the registration/completion race requests the necessary repaint.
    pub fn set_repaint_context(&self, context: &egui::Context) {
        let _ = self.0.repaint.set(context.clone());
        if self.0.initialized.load(Ordering::Acquire) {
            context.request_repaint();
        }
    }

    /// Call only when a normal root can actually present the notification.
    pub fn take_report(&self) -> Option<PreviousRunReport> {
        if !self.0.initialized.load(Ordering::Acquire) {
            return None;
        }
        let report = self.0.report.get()?;
        if self.0.report_taken.swap(true, Ordering::AcqRel) {
            return None;
        }
        Some(report.clone())
    }
}

enum Owner {
    Inert,
    Admitted {
        shared: Arc<Shared>,
        completed: mpsc::Receiver<bool>,
    },
}

pub struct GuiRunDiagnostics {
    owner: Owner,
}
impl Default for GuiRunDiagnostics {
    fn default() -> Self {
        Self::new()
    }
}
impl GuiRunDiagnostics {
    /// Inert construction is safe before parsing modes or single-instance
    /// admission: it reads, creates, rotates, and consumes no evidence.
    pub fn new() -> Self {
        Self {
            owner: Owner::Inert,
        }
    }

    /// Caller has resolved data_dir and holds first-instance admission in main.
    /// Excluded modes and activation-only processes must never call this.
    pub fn admit(&mut self, resolved_data_dir: &Path) {
        let Some(identity) = miv_startup::process_identity() else {
            return;
        };
        if identity.role != miv_startup::Role::Core {
            return;
        }
        let data_dir = resolved_data_dir.to_path_buf();
        self.admit_resolving_with_setup(
            move || resolve_directory_with(&data_dir, miv_startup::gui_diagnostics_directory),
            RunIdentity::from(identity),
            Some(resolved_data_dir.join("logs")),
        );
    }

    #[cfg(test)]
    fn admit_at(&mut self, directory: PathBuf, identity: RunIdentity) {
        self.admit_resolving(move || Ok(directory), identity);
    }

    fn admit_resolving(
        &mut self,
        resolve: impl FnOnce() -> io::Result<PathBuf> + Send + 'static,
        identity: RunIdentity,
    ) {
        self.admit_resolving_with_setup(resolve, identity, None);
    }

    fn admit_resolving_with_setup(
        &mut self,
        resolve: impl FnOnce() -> io::Result<PathBuf> + Send + 'static,
        identity: RunIdentity,
        setup_logs: Option<PathBuf>,
    ) {
        if !matches!(self.owner, Owner::Inert) {
            return;
        }
        let shared = Arc::new(Shared::new());
        let (completed_tx, completed_rx) = mpsc::channel();
        self.owner = Owner::Admitted {
            shared: shared.clone(),
            completed: completed_rx,
        };
        let worker_shared = shared.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("gui-run-diagnostics".into())
            .spawn(move || {
                let _ = worker_shared.writer.set(std::thread::current());
                let result = resolve().and_then(|directory| {
                    let mut sampler = DiagnosticSampler::new();
                    writer_sampling_with_setup(
                        &directory,
                        identity,
                        &worker_shared,
                        |adapter| {
                            sampler.sample_timed(adapter, || worker_shared.checkpoint.stamp())
                        },
                        setup_logs.as_deref(),
                    )
                });
                if let Err(error) = &result {
                    log::warn!("GUI run diagnostics unavailable: {error}");
                }
                worker_shared.initialized();
                worker_shared
                    .completion
                    .store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
                let _ = completed_tx.send(result.is_ok());
            })
        {
            log::warn!("GUI run diagnostics worker unavailable: {error}");
            shared.completion.store(2, Ordering::Release);
            shared.initialized();
        }
    }

    pub fn report_handle(&self) -> Option<PreviousRunReportHandle> {
        match &self.owner {
            Owner::Inert => None,
            Owner::Admitted { shared, .. } => Some(PreviousRunReportHandle(shared.clone())),
        }
    }

    pub fn publisher(&self) -> Option<CheckpointPublisher> {
        match &self.owner {
            Owner::Inert => None,
            Owner::Admitted { shared, .. } => Some(CheckpointPublisher::new(shared.clone())),
        }
    }

    /// Main's last explicit boundary, using the deadline already spent by the
    /// other log writers. No join, retries, fresh budget, or UI-frame wait.
    pub fn finish_until(&mut self, outcome: GuiRunOutcome, deadline: Instant) -> bool {
        let Owner::Admitted { shared, completed } = &self.owner else {
            return true;
        };
        let terminal = match outcome {
            GuiRunOutcome::ReturnedOk => RETURNED_OK,
            GuiRunOutcome::ReturnedError => RETURNED_ERROR,
        };
        let _ =
            shared
                .state
                .compare_exchange(RUNNING, terminal, Ordering::AcqRel, Ordering::Acquire);
        shared.wake();
        match shared.completion.load(Ordering::Acquire) {
            1 => return true,
            2 => return false,
            _ => {}
        }
        completed
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(false)
    }
}

fn resolve_directory_with(
    data_dir: &Path,
    select: impl FnOnce(&Path) -> Option<PathBuf>,
) -> io::Result<PathBuf> {
    // Match the single-instance namespace, including relative/absolute spelling,
    // Windows case, verbatim prefixes, and existing junction aliases. This may
    // canonicalize the filesystem and must only execute on the writer thread.
    let key = crate::single_instance::resolved_data_dir_key(data_dir);
    select(Path::new(&key)).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "GUI diagnostic directory unavailable",
        )
    })
}
impl Drop for GuiRunDiagnostics {
    fn drop(&mut self) {
        if let Owner::Admitted { shared, .. } = &self.owner {
            // An abandoned run stays Running on disk. A prior explicit terminal
            // remains absorbing even if its I/O outlives main's deadline.
            let _ = shared.state.compare_exchange(
                RUNNING,
                ABANDONED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            shared.wake();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunIdentity {
    role: RunRole,
    run: u64,
    core_pid: u32,
    core_started_utc_ms: u64,
    core_started_ticks: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum RunRole {
    Core,
}
impl From<miv_startup::ProcessIdentity> for RunIdentity {
    fn from(value: miv_startup::ProcessIdentity) -> Self {
        Self {
            role: RunRole::Core,
            run: value.run,
            core_pid: value.pid,
            core_started_utc_ms: value.started_utc_ms,
            core_started_ticks: value.started_ticks,
        }
    }
}
impl RunIdentity {
    fn process_identity(self) -> miv_startup::ProcessIdentity {
        miv_startup::ProcessIdentity {
            role: miv_startup::Role::Core,
            run: self.run,
            pid: self.core_pid,
            started_utc_ms: self.core_started_utc_ms,
            started_ticks: self.core_started_ticks,
        }
    }
    fn name(self) -> String {
        format!(
            "{:016x}-core-{}-{}-{:016x}",
            self.run, self.core_pid, self.core_started_utc_ms, self.core_started_ticks
        )
    }
    fn valid(self) -> bool {
        self.core_pid != 0 && self.core_started_utc_ms != 0 && self.core_started_ticks != 0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum RunState {
    Running,
    ReturnedOk,
    ReturnedError,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u32,
    identity: RunIdentity,
    version: [u16; 3],
    state: RunState,
    last_recorded_utc_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkpoint: Option<Checkpoint>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    schema: u32,
    current: RunIdentity,
    previous: Option<RunIdentity>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
fn version() -> [u16; 3] {
    [
        env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap_or(0),
        env!("CARGO_PKG_VERSION_MINOR").parse().unwrap_or(0),
        env!("CARGO_PKG_VERSION_PATCH").parse().unwrap_or(0),
    ]
}
fn bounded_read<T: serde::de::DeserializeOwned>(path: &Path, max: u64) -> io::Result<T> {
    let file = File::open(path)?;
    if file.metadata()?.len() > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "diagnostic record exceeds size limit",
        ));
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "diagnostic record exceeds size limit",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid diagnostic record"))
}
fn replace<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "diagnostic record exceeds size limit",
        ));
    }
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "diagnostic parent missing")
    })?)?;
    file.write_all(&bytes)?;
    file.flush()?;
    file.persist(path).map(|_| ()).map_err(|error| error.error)
}
fn open_lease(directory: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("alive.lock"))?;
    if !file.try_lock_exclusive()? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "GUI diagnostic namespace is alive",
        ));
    }
    Ok(file)
}

fn read_reference(directory: &Path) -> Option<Reference> {
    let value: Reference =
        bounded_read(&directory.join("current.json"), MAX_REFERENCE_BYTES).ok()?;
    (value.schema == SCHEMA
        && value.current.valid()
        && value.previous.is_none_or(RunIdentity::valid))
    .then_some(value)
}
fn previous_report(directory: &Path, reference: &Reference) -> Option<PreviousRunReport> {
    previous_report_with_native(directory, reference, None)
}
fn previous_report_with_native(
    directory: &Path,
    reference: &Reference,
    native_logs: Option<&Path>,
) -> Option<PreviousRunReport> {
    let record: Record = bounded_read(
        &directory.join(reference.current.name()).join("state.json"),
        MAX_RECORD_BYTES,
    )
    .ok()?;
    if record.schema != SCHEMA
        || record.identity != reference.current
        || record.state != RunState::Running
        || record.last_recorded_utc_ms < record.identity.core_started_utc_ms
        || record.last_recorded_utc_ms > now_ms()
        || record
            .checkpoint
            .as_ref()
            .is_some_and(|checkpoint| !checkpoint.valid())
    {
        return None;
    }
    let private = directory.join(record.identity.name());
    let panic = fatal::read_observations(&private, record.identity).unwrap_or_else(|error| {
        log::warn!("panic evidence unavailable: {error}");
        Vec::new()
    });
    let devices =
        device_loss::read_observations(&private, record.identity).unwrap_or_else(|error| {
            log::warn!("device evidence unavailable: {error}");
            Vec::new()
        });
    let native = native_logs
        .map(|logs| {
            crate::native_exception_log::read_correlated(logs, record.identity.process_identity())
                .unwrap_or_else(|error| {
                    log::warn!("native evidence unavailable: {error}");
                    Vec::new()
                })
        })
        .unwrap_or_default();
    Some(format_report_with_observations(
        &record, &panic, &devices, &native,
    ))
}
fn diagnostic_optional<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "不明".to_owned(), |value| value.to_string())
}
fn diagnostic_dimensions(value: Option<checkpoint::Dimensions>) -> String {
    value.map_or_else(
        || "不明".to_owned(),
        |value| format!("{}x{}", value.width, value.height),
    )
}

fn diagnostic_yes_no(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "はい",
        Some(false) => "いいえ",
        None => "不明",
    }
}
fn diagnostic_model(value: Option<checkpoint::DiagnosticModel>) -> &'static str {
    use crate::ai::ModelKind as K;
    use checkpoint::DiagnosticModel as M;
    match value {
        None | Some(M::Unknown) => "不明",
        Some(M::UpscaleRealEsrganX4Plus) => K::UpscaleRealEsrganX4Plus.display_label(),
        Some(M::UpscaleRealEsrganAnime6B) => K::UpscaleRealEsrganAnime6B.display_label(),
        Some(M::UpscaleRealEsrGeneralV3) => K::UpscaleRealEsrGeneralV3.display_label(),
        Some(M::UpscaleRealCugan4x) => K::UpscaleRealCugan4x.display_label(),
        Some(M::UpscaleNmkdSiax4x) => K::UpscaleNmkdSiax4x.display_label(),
        Some(M::DenoiseRealplksr) => K::DenoiseRealplksr.display_label(),
        Some(M::InpaintMiGan) => K::InpaintMiGan.display_label(),
        Some(M::SubjectMatte) => K::SubjectMatte.display_label(),
    }
}
fn diagnostic_backend(value: checkpoint::DiagnosticBackend) -> &'static str {
    use checkpoint::DiagnosticBackend as B;
    match value {
        B::Unknown => "不明",
        B::DirectMl => "DirectML",
        B::TensorRt => "TensorRT",
        B::Cpu => "CPU",
    }
}
fn diagnostic_job_stage(value: checkpoint::JobStage) -> &'static str {
    use checkpoint::JobStage as S;
    match value {
        S::Queued => "実行待ち",
        S::ModelLoad => "モデル読込",
        S::ScaleDetect => "拡大率の確認",
        S::TileRun => "分割画像の推論",
        S::Blend => "分割画像の合成",
        S::ResultWaiting => "結果待ち",
        S::Adopting => "結果の反映",
        S::FinalEffectPending => "後段画像処理待ち",
        S::FinalEffect => "後段画像処理",
        S::GpuRegistration => "GPUへの画像登録",
        S::Completed => "完了",
        S::Discarded => "破棄",
        S::Failed => "失敗",
    }
}
fn diagnostic_gpu_status(
    value: crate::gpu_info::diagnostic_sampler::GpuSampleStatus,
) -> &'static str {
    use crate::gpu_info::diagnostic_sampler::GpuSampleStatus as S;
    match value {
        S::Available => "取得済み",
        S::NoRenderAdapter => "描画用GPUが未登録",
        S::UnsupportedBackend => "この描画方式は未対応",
        S::LuidUnavailable => "GPU識別番号を取得できません",
        S::AdapterMismatch => "GPUの照合が不一致",
        S::Unavailable => "取得できません",
    }
}
fn diagnostic_adapter(
    value: Option<crate::gpu_info::diagnostic_sampler::AdapterIdentity>,
) -> String {
    use crate::gpu_info::diagnostic_sampler::DiagnosticBackend as B;
    let Some(value) = value else {
        return "不明".into();
    };
    let backend = match value.backend {
        B::Noop => "描画なし",
        B::Vulkan => "Vulkan",
        B::Metal => "Metal",
        B::Dx12 => "DirectX 12",
        B::Gl => "OpenGL",
        B::BrowserWebGpu => "WebGPU",
    };
    let identity = value
        .luid
        .map_or_else(|| "不明".into(), |[low, high]| format!("{low}:{high}"));
    format!(
        "メーカー番号 {} / 機種番号 {} / 描画方式 {backend} / 機器識別番号 {identity}",
        value.vendor_id, value.device_id
    )
}
fn diagnostic_segment(value: Option<crate::gpu_info::diagnostic_sampler::SegmentMemory>) -> String {
    value.map_or_else(
        || "不明".into(),
        |value| {
            format!(
                "使用量 {} / 予算 {} バイト",
                value.usage_bytes, value.budget_bytes
            )
        },
    )
}
fn diagnostic_ai_gpu_correlation(
    value: crate::gpu_info::diagnostic_sampler::AiAdapterCorrelation,
) -> &'static str {
    match value {
        crate::gpu_info::diagnostic_sampler::AiAdapterCorrelation::Unknown => "不明",
    }
}

fn elapsed_seconds(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

fn report_summary(
    record: &Record,
    panic: &[fatal::PanicObservation],
    devices: &[device_loss::DeviceObservation],
    native: &[crate::native_exception_log::NativeObservation],
    date: &impl Fn(u64) -> String,
) -> String {
    let mut observations = Vec::new();
    if panic
        .iter()
        .any(|event| event.kind == fatal::PanicKind::GraphicsOutOfMemory)
    {
        observations.push("終了前に、描画用のGPUメモリの確保に失敗していました。".to_owned());
    }
    if !devices.is_empty() {
        observations.push("終了前に、画面表示の機能（GPU）が使えなくなっていました。".to_owned());
    }
    if observations.is_empty() && !panic.is_empty() {
        observations.push("終了前に、プログラム内部のエラーが起きていました。".to_owned());
    }
    if observations.is_empty() && !native.is_empty() {
        observations.push(
            "終了前に、プログラムの例外が記録されていました（処理が続いた場合も含みます）。"
                .to_owned(),
        );
    }
    if let Some(cp) = &record.checkpoint {
        // A retained/cancelled background job must not be described as the
        // current page. All jobs remain available in the report's details.
        if let Some(display) = &cp.display
            && let Some(job) = cp.jobs.iter().find(|job| {
                job.active
                    && !job.cancel_requested
                    && job.metadata.context_id == Some(display.value.context_id)
                    && job.metadata.items_generation == Some(display.value.items_generation)
                    && job.metadata.page.is_some()
                    && job.metadata.page == display.value.page
                    && matches!(
                        job.update.stage,
                        checkpoint::JobStage::ModelLoad
                            | checkpoint::JobStage::ScaleDetect
                            | checkpoint::JobStage::TileRun
                            | checkpoint::JobStage::Blend
                    )
            })
        {
            observations.push(format!(
                "最後の記録ではAI処理の実行中でした（ページ {}/{}、{}）。",
                diagnostic_optional(job.metadata.page),
                diagnostic_optional(job.metadata.total),
                diagnostic_model(
                    job.update
                        .model
                        .or(job.metadata.upscale_model)
                        .or(job.metadata.denoise_model)
                )
            ));
        }
        // This is the rendering adapter, never labelled as AI VRAM. UMA and
        // failed queries keep their explicit qualification in the details.
        if cp.gpu.value.status == crate::gpu_info::diagnostic_sampler::GpuSampleStatus::Available
            && cp.gpu.value.uma == Some(false)
            && let Some(memory) = cp.gpu.value.local
        {
            observations.push(format!(
                "描画用GPUメモリ 使用 {} / 予算 {} MiB（{}）。",
                memory.usage_bytes / (1024 * 1024),
                memory.budget_bytes / (1024 * 1024),
                date(cp.gpu.stamp.utc_ms)
            ));
        }
    }
    let mut summary = "前回の正常終了を確認できませんでした。".to_owned();
    if observations.is_empty() {
        summary
            .push_str("\n原因は記録されていません。強制終了・電源断などでも、この表示になります。");
    } else {
        for observation in observations.iter().take(2) {
            summary.push('\n');
            summary.push_str(observation);
        }
        summary.push_str("\n記録だけでは、終了の原因は断定できません。");
    }
    summary
}

fn format_report(record: &Record) -> PreviousRunReport {
    format_report_with_observations(record, &[], &[], &[])
}
fn format_report_with_observations(
    record: &Record,
    panic: &[fatal::PanicObservation],
    devices: &[device_loss::DeviceObservation],
    native: &[crate::native_exception_log::NativeObservation],
) -> PreviousRunReport {
    format_report_at(record, panic, devices, native, &|ms| {
        crate::diagnostic_time::LocalDateTime::from_utc_ms(ms)
    })
}
fn format_report_at(
    record: &Record,
    panic: &[fatal::PanicObservation],
    devices: &[device_loss::DeviceObservation],
    native: &[crate::native_exception_log::NativeObservation],
    local: &impl Fn(u64) -> Option<crate::diagnostic_time::LocalDateTime>,
) -> PreviousRunReport {
    // The typed schema has no arbitrary diagnostic strings. Formatting is
    // shared by the visible body and the explicit copy action.
    let [major, minor, patch] = record.version;
    let date = |ms| crate::diagnostic_time::display(local(ms));
    let mut body = format!(
        "最終記録時刻: {} / 終了との間隔不明\nmIV {major}.{minor}.{patch} / 記録ID {:016x}-core-{}-{:016x}\n起動時刻: {}\n強制終了・OS終了・電源断・記録失敗とクラッシュは区別できません。\n最終記録以降に状態が変わった可能性があります。",
        date(record.last_recorded_utc_ms),
        record.identity.run,
        record.identity.core_pid,
        record.identity.core_started_ticks,
        date(record.identity.core_started_utc_ms)
    );
    append_observations(&mut body, panic, devices, native, &date);
    if let Some(checkpoint) = &record.checkpoint {
        use std::fmt::Write as _;
        let _ = write!(
            body,
            "\n定期記録: {} / 記録開始後 {} 秒 / 終了との間隔不明\n活動数 {} / 処理数 {} / 省略した処理数 {} / 受渡しできなかった更新数 {}",
            date(checkpoint.stamp.utc_ms),
            elapsed_seconds(checkpoint.stamp.run_elapsed_ms),
            checkpoint.active_leases,
            checkpoint.active_jobs,
            checkpoint.omitted_jobs,
            checkpoint.dropped_updates
        );
        body.push_str(if checkpoint.active_leases == 0 {
            " / 活動終了（待機中）"
        } else {
            " / 活動中"
        });
        let _ = write!(
            body,
            "\n実行待ち {} / 保持入力容量（バイト） {} / 実行中 {} / 結果待ち {} / 後段合成 {} / GPU登録要求 {}\n入力画像の共有分も含むため、追加メモリ量とは異なります。GPUへの登録要求はGPU処理の完了を意味しません。",
            checkpoint.queued_jobs,
            diagnostic_optional(checkpoint.queued_source_bytes),
            checkpoint.running_jobs,
            checkpoint.result_waiting_jobs,
            checkpoint.final_effect_jobs,
            checkpoint.gpu_registration_jobs
        );
        body.push_str(
            "\n処理方式は選択された方式です。AIライブラリ内部の実際の割当先は未確認です。",
        );
        if let Some(display) = &checkpoint.display {
            let _ = write!(
                body,
                "\n表示: 画面番号 {} / 更新番号 {} / ページ {}/{} / {} / 記録開始後 {} 秒 / 終了との間隔不明",
                display.value.context_id,
                display.value.items_generation,
                diagnostic_optional(display.value.page),
                diagnostic_optional(display.value.total),
                date(display.stamp.utc_ms),
                elapsed_seconds(display.stamp.run_elapsed_ms)
            );
        }
        for job in &checkpoint.jobs {
            let job_label = if job.metadata.job_id == 0 {
                "処理番号 不明".into()
            } else {
                format!("処理番号 {}", job.metadata.job_id)
            };
            let backend = job.update.backend.unwrap_or(job.metadata.backend);
            let _ = write!(
                body,
                "\n{job_label}: 画面番号 {} / 更新番号 {} / ページ {}/{} / 段階 {} / 処理方式 {} / 拡大モデル {} / ノイズ除去モデル {} / 使用モデル {} / 入力画像の寸法 {} / 保持入力容量（バイト） {} / 出力画像の寸法 {} / 分割画像の寸法 {} {}/{} / {} / 記録開始後 {} 秒 / 終了との間隔不明\n取消の確認: {} / {} / 記録開始後 {} 秒",
                diagnostic_optional(job.metadata.context_id),
                diagnostic_optional(job.metadata.items_generation),
                diagnostic_optional(job.metadata.page),
                diagnostic_optional(job.metadata.total),
                diagnostic_job_stage(job.update.stage),
                diagnostic_backend(backend),
                diagnostic_model(job.metadata.upscale_model),
                diagnostic_model(job.metadata.denoise_model),
                diagnostic_model(job.update.model),
                diagnostic_dimensions(job.metadata.source),
                diagnostic_optional(job.metadata.source_bytes),
                diagnostic_dimensions(job.update.output),
                diagnostic_dimensions(job.update.tile),
                diagnostic_optional(job.update.tile_index),
                diagnostic_optional(job.update.tile_total),
                date(job.stamp.utc_ms),
                elapsed_seconds(job.stamp.run_elapsed_ms),
                diagnostic_yes_no(Some(job.cancel_requested)),
                date(job.cancel_stamp.utc_ms),
                elapsed_seconds(job.cancel_stamp.run_elapsed_ms)
            );
        }
        let cpu = &checkpoint.cpu_private_bytes;
        let cpu_value = cpu
            .value
            .map_or_else(|| "不明".into(), |value| value.to_string());
        let _ = write!(
            body,
            "\nアプリ専用メモリ確保量（バイト）: {cpu_value} / {} / 記録開始後 {} 秒 / 終了との間隔不明",
            date(cpu.stamp.utc_ms),
            elapsed_seconds(cpu.stamp.run_elapsed_ms)
        );
        let gpu = &checkpoint.gpu;
        let _ = write!(
            body,
            "\n描画用GPUの照会: {} / GPU識別情報 {} / GPU内の区分番号 {} / メモリ共有型か {} / GPU側メモリ区分 {} / システム側メモリ区分 {} / {} / 記録開始後 {} 秒 / 終了との間隔不明\nAI処理GPUとの一致状況: {}（一致未確認）\nメモリ区分は専用メモリの有無を示しません。GPU使用量・予算だけでは終了原因は判断できません。",
            diagnostic_gpu_status(gpu.value.status),
            diagnostic_adapter(gpu.value.adapter),
            diagnostic_optional(gpu.value.node_index),
            diagnostic_yes_no(gpu.value.uma),
            diagnostic_segment(gpu.value.local),
            diagnostic_segment(gpu.value.nonlocal),
            date(gpu.stamp.utc_ms),
            elapsed_seconds(gpu.stamp.run_elapsed_ms),
            diagnostic_ai_gpu_correlation(checkpoint.ai_adapter_correlation)
        );
    }
    if let (Some(checkpoint), Some(observation)) = (&record.checkpoint, panic.last()) {
        append_panic_interval(&mut body, "定期記録", checkpoint.stamp, observation.stamp);
        if let Some(display) = &checkpoint.display {
            append_panic_interval(&mut body, "表示状態", display.stamp, observation.stamp);
        }
        append_panic_interval(
            &mut body,
            "アプリのメモリ記録",
            checkpoint.cpu_private_bytes.stamp,
            observation.stamp,
        );
        append_panic_interval(
            &mut body,
            "GPUメモリ記録",
            checkpoint.gpu.stamp,
            observation.stamp,
        );
        for job in &checkpoint.jobs {
            append_panic_interval(
                &mut body,
                &format!("処理番号 {}", job.metadata.job_id),
                job.stamp,
                observation.stamp,
            );
        }
    }
    PreviousRunReport::from_sections(report_summary(record, panic, devices, native, &date), body)
}

fn append_observations(
    body: &mut String,
    panic: &[fatal::PanicObservation],
    devices: &[device_loss::DeviceObservation],
    native: &[crate::native_exception_log::NativeObservation],
    date: &impl Fn(u64) -> String,
) {
    use std::fmt::Write as _;
    for event in panic {
        let classification = match event.kind {
            fatal::PanicKind::Rust => "プログラム内部のエラー（詳細不明）",
            fatal::PanicKind::GraphicsOutOfMemory => {
                "描画用GPUメモリの確保失敗 / プログラム内部のエラー"
            }
            fatal::PanicKind::GraphicsValidation => "描画処理の検証エラー / プログラム内部のエラー",
            fatal::PanicKind::GraphicsInternal => "描画処理の内部エラー / プログラム内部のエラー",
        };
        let _ = write!(
            body,
            "\n終了前に観測された情報: {classification}\n内部エラーの観測時刻: {} / 記録開始後 {} 秒 / 実行単位番号 {}",
            date(event.stamp.utc_ms),
            elapsed_seconds(event.stamp.run_elapsed_ms),
            event.thread
        );
    }
    for event in devices {
        let _ = write!(
            body,
            "\n終了前に観測された情報: 描画機能の利用不能（番号 {}） / 理由 不明\n保存処理の確認時刻: {} / 記録開始後 {} 秒（異常の発生時刻・終了時刻ではありません）",
            event.device_id,
            date(event.stamp.utc_ms),
            elapsed_seconds(event.stamp.run_elapsed_ms)
        );
    }
    // Keep presentation bounded even after a run hits the native recorder cap.
    // The local native log retains every event; copy and display share this tail.
    let omitted = native.len().saturating_sub(8);
    if omitted != 0 {
        let _ = write!(body, "\n以前の例外観測: {omitted}件を省略");
    }
    for event in native.iter().skip(omitted) {
        let classification = match event.code {
            0xC0000005 => "アクセス違反",
            0xC00000FD => "処理の呼出し領域の超過",
            0x80000003 => "停止点への到達",
            0xC000001D => "無効な命令",
            0xC0000094 => "整数のゼロ除算",
            0xC0000095 => "整数の表現範囲超過",
            0xC0000374 => "動作中メモリ領域の破損",
            _ => "不明",
        };
        let _ = write!(
            body,
            "\n終了前に観測された例外（処理が続いた場合も含む）: {classification} / 例外番号 0x{:08X} / {}。終了原因かは不明です。",
            event.code,
            date(event.utc_ms)
        );
    }
    if !panic.is_empty() || !devices.is_empty() || !native.is_empty() {
        body.push_str("\n観測された情報だけでは終了原因を確定できません。背景処理や捕捉された内部エラー、処理が続いた例外でも記録されます。");
    }
}
fn append_panic_interval(body: &mut String, label: &str, sample: Stamp, panic: Stamp) {
    use std::fmt::Write as _;
    if sample.utc_ms <= panic.utc_ms && sample.run_elapsed_ms <= panic.run_elapsed_ms {
        let _ = write!(
            body,
            "\n{label}採取→同じ実行の最後の内部エラー観測: {} 秒前（終了との間隔は不明）",
            elapsed_seconds(panic.run_elapsed_ms - sample.run_elapsed_ms)
        );
    } else {
        let _ = write!(
            body,
            "\n{label}採取と同じ実行の最後の内部エラー観測との間隔: 不明"
        );
    }
}

fn writer(directory: &Path, identity: RunIdentity, shared: &Shared) -> io::Result<()> {
    let mut sampler = DiagnosticSampler::new();
    writer_sampling(directory, identity, shared, |adapter| {
        sampler.sample_timed(adapter, || shared.checkpoint.stamp())
    })
}

fn capture_checkpoint(
    shared: &Shared,
    sample: &mut impl FnMut(Option<&AdapterProbe>) -> (MemorySample, Stamp, Stamp),
) -> Checkpoint {
    let snapshot = shared.checkpoint.snapshot();
    // The mailbox guard has been released before any platform call.
    let (memory, cpu_stamp, gpu_stamp) = sample(snapshot.adapter.as_ref());
    snapshot.complete(&shared.checkpoint, memory, cpu_stamp, gpu_stamp)
}

fn writer_sampling(
    directory: &Path,
    identity: RunIdentity,
    shared: &Shared,
    sample: impl FnMut(Option<&AdapterProbe>) -> (MemorySample, Stamp, Stamp),
) -> io::Result<()> {
    writer_sampling_with_setup(directory, identity, shared, sample, None)
}
fn writer_sampling_with_setup(
    directory: &Path,
    identity: RunIdentity,
    shared: &Shared,
    mut sample: impl FnMut(Option<&AdapterProbe>) -> (MemorySample, Stamp, Stamp),
    setup_logs: Option<&Path>,
) -> io::Result<()> {
    std::fs::create_dir_all(directory)?;
    // Held across initialization and every outstanding I/O, including after a
    // main deadline. A newer run never competes with this writer's reference
    // publication or retention. Failure is logged once; no wait/retry.
    let _alive = open_lease(directory)?;
    let previous = read_reference(directory);
    if let Some(report) = previous
        .as_ref()
        .and_then(|reference| previous_report_with_native(directory, reference, setup_logs))
    {
        let _ = shared.report.set(report);
    }
    let run_directory = directory.join(identity.name());
    std::fs::create_dir(&run_directory)?;
    let path = run_directory.join("state.json");
    let initial_boundary = shared.checkpoint.boundaries();
    let initial_active = shared.checkpoint.active();
    let mut record = Record {
        schema: SCHEMA,
        identity,
        version: version(),
        state: RunState::Running,
        last_recorded_utc_ms: now_ms(),
        checkpoint: Some(capture_checkpoint(shared, &mut sample)),
    };
    record.last_recorded_utc_ms = now_ms();
    replace(&path, &record)?;
    // Initial publication must also happen when main requested a terminal
    // during initialization: otherwise an old Running reference survives a
    // successful short run. The alive lease excludes every newer writer until
    // this publication and the final private-state replacement have completed.
    // Initial reference publication and bounded retention form one startup
    // transaction even for a short run. Terminal processing never republishes
    // a reference or performs generation cleanup.
    let reference = Reference {
        schema: SCHEMA,
        current: identity,
        previous: previous.map(|r| r.current),
    };
    replace(&directory.join("current.json"), &reference)?;
    // Device evidence obeys the same reachability boundary as the fatal sink.
    let mut device_writer = match device_loss::Writer::open(&run_directory) {
        Ok(writer) => Some(writer),
        Err(error) => {
            log::warn!("device evidence unavailable: {error}");
            None
        }
    };
    observe_devices(shared, identity, &mut device_writer);
    // A published fatal sink must never write an unreachable run. Both its
    // Running record and current reference are saved before publishing it.
    // Previous native evidence was secured above, before this rotation/open.
    if let Some(logs) = setup_logs {
        if let Err(error) = fatal::prepare(&run_directory, identity, shared.checkpoint.origin()) {
            log::warn!("panic evidence unavailable: {error}");
        }
        #[cfg(test)]
        if let Some(gate) = shared.after_panic_sink.get() {
            gate.wait();
            gate.wait();
        }
        crate::native_exception_log::report_install_result(
            crate::native_exception_log::install_in(logs, Some(identity.process_identity())),
        );
    }
    retain_two(directory, &reference)?;
    shared.initialized();
    let mut schedule = Schedule::default();
    schedule.saved(
        shared.checkpoint.elapsed_ms(),
        initial_active,
        initial_boundary,
    );
    loop {
        match shared.state.load(Ordering::Acquire) {
            RUNNING => {
                let new_device_event = observe_devices(shared, identity, &mut device_writer);
                let now = shared.checkpoint.elapsed_ms();
                let active = shared.checkpoint.active();
                let boundary = shared.checkpoint.boundaries();
                if new_device_event || schedule.due(now, active, boundary) {
                    record.checkpoint = Some(capture_checkpoint(shared, &mut sample));
                    record.last_recorded_utc_ms = now_ms();
                    replace(&path, &record)?;
                    // Save only the revision captured before sampling/I/O.
                    // Changes during either operation require another pass.
                    schedule.saved(shared.checkpoint.elapsed_ms(), active, boundary);
                    continue;
                }
                match schedule.wait(now, active) {
                    Some(wait) => std::thread::park_timeout(wait),
                    None => std::thread::park(),
                }
            }
            ABANDONED => return Ok(()),
            terminal => {
                observe_devices(shared, identity, &mut device_writer);
                record.state = if terminal == RETURNED_OK {
                    RunState::ReturnedOk
                } else {
                    RunState::ReturnedError
                };
                record.checkpoint = Some(capture_checkpoint(shared, &mut sample));
                record.last_recorded_utc_ms = now_ms();
                return replace(&path, &record);
            }
        }
    }
}

fn observe_devices(
    shared: &Shared,
    identity: RunIdentity,
    writer: &mut Option<device_loss::Writer>,
) -> bool {
    let observations = shared.device_loss.collect(shared.checkpoint.stamp());
    let changed = !observations.is_empty();
    for observation in observations {
        if let Some(writer) = writer {
            if let Err(error) = writer.append(identity, observation) {
                log::warn!("device evidence write failed: {error}");
            }
        }
    }
    changed
}

fn retain_two(directory: &Path, reference: &Reference) -> io::Result<()> {
    // Bounded enumeration; never recurse through unknown/reparse directories.
    // Only a schema-valid record determines a directory that we own.
    for entry in std::fs::read_dir(directory)?.take(64) {
        let entry = entry?;
        if !plain_directory(&entry.path())? {
            continue;
        }
        let path = entry.path();
        let Ok(record) = bounded_read::<Record>(&path.join("state.json"), MAX_RECORD_BYTES) else {
            continue;
        };
        if record.schema != SCHEMA
            || !record.identity.valid()
            || entry.file_name() != record.identity.name().as_str()
            || record.identity == reference.current
            || Some(record.identity) == reference.previous
        {
            continue;
        }
        for name in ["panic-events.log", device_loss::FILE_NAME] {
            let owned = path.join(name);
            match std::fs::symlink_metadata(&owned) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    std::fs::remove_file(owned)?;
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        std::fs::remove_file(path.join("state.json"))?;
        // No recursive deletion: unrecognized contents are never removed.
        let _ = std::fs::remove_dir(path);
    }
    Ok(())
}

fn plain_directory(path: &Path) -> io::Result<bool> {
    let metadata = std::fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Ok(false);
        }
    }
    Ok(metadata.is_dir() && !metadata.file_type().is_symlink())
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;
    static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

    /// In-memory publisher only; no worker, namespace, files or native queries.
    pub(crate) fn publisher() -> CheckpointPublisher {
        CheckpointPublisher::new(Arc::new(Shared::new()))
    }

    pub(crate) fn checkpoint_report_body() -> String {
        checkpoint_report().body
    }
    pub(crate) fn checkpoint_report() -> PreviousRunReport {
        format_report_at(&checkpoint_record(), &[], &[], &[], &fixture_local)
    }
    fn fixture_local(ms: u64) -> Option<crate::diagnostic_time::LocalDateTime> {
        crate::diagnostic_time::LocalDateTime::with_offset(ms, 540)
    }

    pub(crate) fn stage_c_report_body() -> String {
        stage_c_report().body
    }
    pub(crate) fn stage_c_report() -> PreviousRunReport {
        let record = checkpoint_record();
        let stamp = Stamp {
            utc_ms: record.last_recorded_utc_ms + 700,
            run_elapsed_ms: 12_700,
        };
        format_report_at(
            &record,
            &[fatal::PanicObservation {
                stamp,
                thread: 17,
                kind: fatal::PanicKind::GraphicsOutOfMemory,
            }],
            &[device_loss::DeviceObservation {
                device_id: 1,
                stamp,
                reason: device_loss::DeviceReason::Unknown,
            }],
            &[crate::native_exception_log::NativeObservation {
                utc_ms: stamp.utc_ms,
                code: 0xC0000005,
            }],
            &fixture_local,
        )
    }

    pub(super) fn checkpoint_record() -> Record {
        use crate::gpu_info::diagnostic_sampler::{
            AdapterIdentity, DiagnosticBackend as GpuBackend, GpuMemorySample, GpuSampleStatus,
            SegmentMemory,
        };
        let shared = Arc::new(Shared::new());
        let publisher = CheckpointPublisher::new(shared.clone());
        publisher.publish_display(checkpoint::DisplaySnapshot {
            context_id: 3,
            items_generation: 17,
            page: Some(12),
            total: Some(40),
        });
        let job = publisher.begin_job(
            checkpoint::JobMetadata {
                context_id: Some(3),
                items_generation: Some(17),
                job_id: 655,
                page: Some(12),
                total: Some(40),
                upscale_model: Some(checkpoint::DiagnosticModel::UpscaleRealEsrGeneralV3),
                source: Some(checkpoint::Dimensions {
                    width: 2400,
                    height: 3600,
                }),
                source_bytes: Some(34_560_000),
                ..Default::default()
            },
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
        job.update(checkpoint::JobUpdate {
            stage: checkpoint::JobStage::TileRun,
            backend: Some(checkpoint::DiagnosticBackend::DirectMl),
            model: Some(checkpoint::DiagnosticModel::UpscaleRealEsrGeneralV3),
            output: Some(checkpoint::Dimensions {
                width: 9600,
                height: 14400,
            }),
            tile: Some(checkpoint::Dimensions {
                width: 256,
                height: 256,
            }),
            tile_index: Some(37),
            tile_total: Some(150),
        });
        let stamp = Stamp {
            utc_ms: 1_793_340_000_000,
            run_elapsed_ms: 12_000,
        };
        let mut cp = capture_checkpoint(&shared, &mut |_| {
            (
                MemorySample {
                    cpu_private_bytes: Some(1_200_000_000),
                    rendering: GpuMemorySample {
                        status: GpuSampleStatus::Available,
                        adapter: Some(AdapterIdentity {
                            vendor_id: 4318,
                            device_id: 9860,
                            backend: GpuBackend::Dx12,
                            luid: Some([10, 0]),
                        }),
                        local: Some(SegmentMemory {
                            usage_bytes: 2_000_000_000,
                            budget_bytes: 4_000_000_000,
                        }),
                        nonlocal: Some(SegmentMemory {
                            usage_bytes: 100_000_000,
                            budget_bytes: 8_000_000_000,
                        }),
                        node_index: Some(0),
                        uma: Some(false),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                stamp,
                stamp,
            )
        });
        cp.stamp = stamp;
        cp.display.as_mut().unwrap().stamp = stamp;
        cp.cpu_private_bytes.stamp = stamp;
        cp.gpu.stamp = stamp;
        for job in &mut cp.jobs {
            job.stamp = stamp;
            job.cancel_stamp = stamp;
        }
        Record {
            schema: SCHEMA,
            identity: RunIdentity {
                role: RunRole::Core,
                run: 655,
                core_pid: 1234,
                core_started_utc_ms: stamp.utc_ms - 12_000,
                core_started_ticks: 1,
            },
            version: [4, 10, 0],
            state: RunState::Running,
            last_recorded_utc_ms: stamp.utc_ms,
            checkpoint: Some(cp),
        }
    }

    fn panic_identity(pid: u32) -> RunIdentity {
        RunIdentity {
            role: RunRole::Core,
            run: 1,
            core_pid: pid,
            core_started_utc_ms: 1,
            core_started_ticks: 1,
        }
    }
    pub(crate) fn prepare_panic_sink(directory: &Path) {
        fatal::prepare(
            directory,
            panic_identity(std::process::id()),
            clock::RunClock::new(),
        )
        .unwrap();
    }
    pub(crate) fn panic_kinds(directory: &Path, pid: u32) -> Vec<fatal::PanicKind> {
        fatal::read_observations(directory, panic_identity(pid))
            .unwrap()
            .into_iter()
            .map(|event| event.kind)
            .collect()
    }

    pub(super) fn identity() -> RunIdentity {
        RunIdentity {
            role: RunRole::Core,
            run: NEXT_RUN.fetch_add(1, Ordering::Relaxed),
            core_pid: std::process::id(),
            core_started_utc_ms: now_ms(),
            core_started_ticks: NEXT_RUN.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Isolated fixture only: no process-global startup identity or normal logs.
    pub(crate) fn admit(owner: &mut GuiRunDiagnostics, directory: &Path) {
        owner.admit_at(directory.to_path_buf(), identity());
        wait_ready(owner);
    }

    pub(super) fn wait_ready(owner: &GuiRunDiagnostics) {
        let Owner::Admitted { shared, .. } = &owner.owner else {
            panic!("fixture admission failed")
        };
        let ready = shared.ready.0.lock().unwrap();
        let (_ready, timeout) = shared
            .ready
            .1
            .wait_timeout_while(ready, Duration::from_secs(20), |ready| !*ready)
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "diagnostic fixture did not initialize"
        );
        assert_ne!(
            shared.completion.load(Ordering::Acquire),
            2,
            "diagnostic fixture initialization failed"
        );
    }

    pub(crate) fn terminal(directory: &Path) -> Option<GuiRunOutcome> {
        match record(directory)?.state {
            RunState::ReturnedOk => Some(GuiRunOutcome::ReturnedOk),
            RunState::ReturnedError => Some(GuiRunOutcome::ReturnedError),
            RunState::Running => None,
        }
    }

    pub(crate) fn is_running(directory: &Path) -> bool {
        record(directory).is_some_and(|record| record.state == RunState::Running)
    }

    /// A ready immutable report for headless presentation/hidden-root tests.
    pub(crate) fn report_handle() -> PreviousRunReportHandle {
        let shared = Arc::new(Shared::new());
        let _ = shared.report.set(format_report(&Record {
            schema: SCHEMA,
            identity: identity(),
            version: [4, 10, 0],
            state: RunState::Running,
            last_recorded_utc_ms: now_ms(),
            checkpoint: None,
        }));
        shared.initialized();
        PreviousRunReportHandle(shared)
    }

    pub(super) fn record(directory: &Path) -> Option<Record> {
        let reference = read_reference(directory)?;
        bounded_read(
            &directory.join(reference.current.name()).join("state.json"),
            MAX_RECORD_BYTES,
        )
        .ok()
    }

    pub(super) fn wait_abandoned(mut owner: GuiRunDiagnostics) {
        let Owner::Admitted { completed, .. } = &mut owner.owner else {
            panic!("missing fixture")
        };
        let completed = std::mem::replace(completed, mpsc::channel().1);
        drop(owner);
        assert!(completed.recv_timeout(Duration::from_secs(20)).unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn finish(owner: &mut GuiRunDiagnostics, outcome: GuiRunOutcome) {
        assert!(owner.finish_until(outcome, Instant::now() + Duration::from_secs(20)));
    }

    #[test]
    fn inert_owner_does_not_create_or_consume_any_evidence() {
        let mut owner = GuiRunDiagnostics::new();
        assert!(owner.report_handle().is_none());
        assert!(owner.finish_until(GuiRunOutcome::ReturnedOk, Instant::now()));
        assert!(owner.report_handle().is_none());
    }

    #[test]
    fn equivalent_data_dir_spellings_share_previous_run_and_resolve_on_worker() {
        use std::hash::{Hash, Hasher};
        let directory = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let data_dir = directory.path().join("FixtureData");
        std::fs::create_dir(&data_dir).unwrap();
        let absolute = std::fs::canonicalize(&data_dir).unwrap();
        // Use std::path::absolute (without a verbatim prefix) for strip_prefix.
        let plain_absolute = std::path::absolute(&data_dir).unwrap();
        let relative = plain_absolute
            .strip_prefix(std::env::current_dir().unwrap())
            .unwrap()
            .to_path_buf();
        let mut spellings = vec![absolute.clone(), absolute.join("."), relative];
        #[cfg(windows)]
        spellings.push(PathBuf::from(absolute.to_string_lossy().to_uppercase()));
        let caller = std::thread::current().id();
        for (index, spelling) in spellings.drain(..).enumerate() {
            let logs = directory.path().join("logs");
            let mut owner = GuiRunDiagnostics::new();
            owner.admit_resolving(
                move || {
                    assert_ne!(
                        std::thread::current().id(),
                        caller,
                        "directory canonicalization must stay off the caller/UI thread"
                    );
                    resolve_directory_with(&spelling, |key| {
                        let mut hash = std::collections::hash_map::DefaultHasher::new();
                        key.hash(&mut hash);
                        Some(logs.join(format!("{:016x}", hash.finish())))
                    })
                },
                test_support::identity(),
            );
            test_support::wait_ready(&owner);
            assert_eq!(
                owner.report_handle().unwrap().take_report().is_some(),
                index != 0
            );
            test_support::wait_abandoned(owner);
        }
    }

    #[test]
    fn normal_and_controlled_error_are_terminal_and_do_not_warn_next_time() {
        for outcome in [GuiRunOutcome::ReturnedOk, GuiRunOutcome::ReturnedError] {
            let directory = tempfile::tempdir().unwrap();
            let mut owner = GuiRunDiagnostics::new();
            test_support::admit(&mut owner, directory.path());
            assert!(owner.report_handle().unwrap().take_report().is_none());
            finish(&mut owner, outcome);
            assert_eq!(test_support::terminal(directory.path()), Some(outcome));
            let mut next = GuiRunDiagnostics::new();
            test_support::admit(&mut next, directory.path());
            assert!(next.report_handle().unwrap().take_report().is_none());
            finish(&mut next, GuiRunOutcome::ReturnedOk);
        }
    }

    #[test]
    fn drop_preserves_running_and_report_is_consumed_once_across_handles() {
        let directory = tempfile::tempdir().unwrap();
        let mut owner = GuiRunDiagnostics::new();
        test_support::admit(&mut owner, directory.path());
        test_support::wait_abandoned(owner);
        assert_eq!(
            test_support::record(directory.path()).unwrap().state,
            RunState::Running
        );
        let mut next = GuiRunDiagnostics::new();
        test_support::admit(&mut next, directory.path());
        let handle = next.report_handle().unwrap();
        let other = handle.clone();
        let report = handle.take_report().unwrap();
        assert!(
            report
                .body()
                .contains("前回の正常終了を確認できませんでした")
        );
        assert!(other.take_report().is_none());
        finish(&mut next, GuiRunOutcome::ReturnedOk);
    }

    #[test]
    fn terminal_is_absorbing_and_no_late_running_replaces_it() {
        let directory = tempfile::tempdir().unwrap();
        let mut owner = GuiRunDiagnostics::new();
        test_support::admit(&mut owner, directory.path());
        finish(&mut owner, GuiRunOutcome::ReturnedError);
        finish(&mut owner, GuiRunOutcome::ReturnedOk);
        assert_eq!(
            test_support::terminal(directory.path()),
            Some(GuiRunOutcome::ReturnedError)
        );
    }

    #[test]
    fn live_namespace_is_neither_consumed_nor_modified() {
        let directory = tempfile::tempdir().unwrap();
        let mut owner = GuiRunDiagnostics::new();
        test_support::admit(&mut owner, directory.path());
        let before = std::fs::read(directory.path().join("current.json")).unwrap();
        let shared = Shared::new();
        let error = writer(directory.path(), test_support::identity(), &shared).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(shared.report.get().is_none());
        assert_eq!(
            before,
            std::fs::read(directory.path().join("current.json")).unwrap()
        );
        finish(&mut owner, GuiRunOutcome::ReturnedOk);
    }

    #[test]
    fn namespace_retains_exactly_current_and_previous_owned_run() {
        let directory = tempfile::tempdir().unwrap();
        for _ in 0..4 {
            let mut owner = GuiRunDiagnostics::new();
            test_support::admit(&mut owner, directory.path());
            finish(&mut owner, GuiRunOutcome::ReturnedOk);
        }
        let reference = read_reference(directory.path()).unwrap();
        let run_directories: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(Result::unwrap)
            .filter(|e| e.file_type().unwrap().is_dir())
            .collect();
        assert_eq!(run_directories.len(), 2);
        for entry in run_directories {
            assert!(
                entry.file_name() == reference.current.name().as_str()
                    || entry.file_name() == reference.previous.unwrap().name().as_str()
            );
        }
    }

    #[test]
    fn unknown_corrupt_oversized_and_identity_mismatched_records_are_not_reports() {
        let directory = tempfile::tempdir().unwrap();
        let identity = test_support::identity();
        std::fs::create_dir(directory.path().join(identity.name())).unwrap();
        let reference = Reference {
            schema: SCHEMA,
            current: identity,
            previous: None,
        };
        replace(&directory.path().join("current.json"), &reference).unwrap();
        let path = directory.path().join(identity.name()).join("state.json");
        for bytes in [
            b"not JSON".to_vec(),
            vec![b' '; MAX_RECORD_BYTES as usize + 1],
            serde_json::to_vec(&serde_json::json!({"schema":99,"identity":identity,"version":[1,2,3],"state":"Running","last_recorded_utc_ms":now_ms()})).unwrap(),
            serde_json::to_vec(&serde_json::json!({"schema":1,"identity":test_support::identity(),"version":[1,2,3],"state":"Running","last_recorded_utc_ms":now_ms()})).unwrap(),
            serde_json::to_vec(&serde_json::json!({"schema":1,"identity":identity,"version":[1,2,3],"state":"Running","last_recorded_utc_ms":now_ms(),"path":"C:/private/secret.png"})).unwrap(),
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert!(previous_report(directory.path(), &reference).is_none());
        }
        std::fs::write(directory.path().join("current.json"), b"{\"schema\":99}").unwrap();
        assert!(read_reference(directory.path()).is_none());
    }

    #[test]
    fn same_pid_and_run_with_a_different_start_is_not_the_same_identity() {
        let directory = tempfile::tempdir().unwrap();
        let identity = test_support::identity();
        let other = RunIdentity {
            core_started_ticks: identity.core_started_ticks + 1,
            ..identity
        };
        std::fs::create_dir(directory.path().join(identity.name())).unwrap();
        let record = Record {
            schema: SCHEMA,
            identity: other,
            version: [1, 2, 3],
            state: RunState::Running,
            last_recorded_utc_ms: now_ms(),
            checkpoint: None,
        };
        replace(
            &directory.path().join(identity.name()).join("state.json"),
            &record,
        )
        .unwrap();
        assert!(
            previous_report(
                directory.path(),
                &Reference {
                    schema: SCHEMA,
                    current: identity,
                    previous: None
                }
            )
            .is_none()
        );
    }

    #[test]
    fn impossible_record_time_is_not_adopted_and_old_time_has_no_freshness_claim() {
        let directory = tempfile::tempdir().unwrap();
        let identity = RunIdentity {
            core_started_utc_ms: 1000,
            ..test_support::identity()
        };
        std::fs::create_dir(directory.path().join(identity.name())).unwrap();
        let reference = Reference {
            schema: SCHEMA,
            current: identity,
            previous: None,
        };
        let path = directory.path().join(identity.name()).join("state.json");
        for time in [999, u64::MAX] {
            replace(
                &path,
                &Record {
                    schema: SCHEMA,
                    identity,
                    version: [1, 2, 3],
                    state: RunState::Running,
                    last_recorded_utc_ms: time,
                    checkpoint: None,
                },
            )
            .unwrap();
            assert!(previous_report(directory.path(), &reference).is_none());
        }
        replace(
            &path,
            &Record {
                schema: SCHEMA,
                identity,
                version: [1, 2, 3],
                state: RunState::Running,
                last_recorded_utc_ms: 1000,
                checkpoint: None,
            },
        )
        .unwrap();
        let report = previous_report(directory.path(), &reference).unwrap();
        assert!(report.body().contains("終了との間隔不明"));
        assert!(!report.body().contains("秒前"));
    }

    #[test]
    fn namespace_initialization_failure_does_not_retry_or_change_user_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("not-a-directory");
        std::fs::write(&path, b"preserve user data").unwrap();
        let shared = Shared::new();
        assert!(writer(&path, test_support::identity(), &shared).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"preserve user data");
    }

    #[test]
    fn deadline_does_not_join_or_release_a_still_writing_old_run() {
        // Block a writer at a real outstanding-I/O boundary, then finish main
        // with no budget. The next writer cannot acquire this old run's lease.
        let directory = tempfile::tempdir().unwrap();
        let identity = test_support::identity();
        let shared = Arc::new(Shared::new());
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (completed_tx, completed_rx) = mpsc::channel();
        let worker_shared = shared.clone();
        let root = directory.path().to_path_buf();
        std::thread::spawn(move || {
            let lease = open_lease(&root).unwrap();
            let run_directory = root.join(identity.name());
            std::fs::create_dir(&run_directory).unwrap();
            replace(
                &run_directory.join("state.json"),
                &Record {
                    schema: SCHEMA,
                    identity,
                    version: [1, 2, 3],
                    state: RunState::Running,
                    last_recorded_utc_ms: now_ms(),
                    checkpoint: None,
                },
            )
            .unwrap();
            replace(
                &root.join("current.json"),
                &Reference {
                    schema: SCHEMA,
                    current: identity,
                    previous: None,
                },
            )
            .unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            assert_eq!(worker_shared.state.load(Ordering::Acquire), RETURNED_OK);
            // Simulated completion writes only this run's private state.
            replace(
                &run_directory.join("state.json"),
                &Record {
                    schema: SCHEMA,
                    identity,
                    version: [1, 2, 3],
                    state: RunState::ReturnedOk,
                    last_recorded_utc_ms: now_ms(),
                    checkpoint: None,
                },
            )
            .unwrap();
            drop(lease);
            worker_shared.completion.store(1, Ordering::Release);
            completed_tx.send(true).unwrap();
        });
        ready_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        let mut owner = GuiRunDiagnostics {
            owner: Owner::Admitted {
                shared,
                completed: completed_rx,
            },
        };
        assert!(!owner.finish_until(GuiRunOutcome::ReturnedOk, Instant::now()));
        let before = std::fs::read(directory.path().join("current.json")).unwrap();
        let error = writer(directory.path(), test_support::identity(), &Shared::new()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(
            before,
            std::fs::read(directory.path().join("current.json")).unwrap()
        );
        release_tx.send(()).unwrap();
        finish(&mut owner, GuiRunOutcome::ReturnedOk);
        let mut next = GuiRunDiagnostics::new();
        test_support::admit(&mut next, directory.path());
        let next_reference = std::fs::read(directory.path().join("current.json")).unwrap();
        drop(owner);
        assert_eq!(
            next_reference,
            std::fs::read(directory.path().join("current.json")).unwrap()
        );
        finish(&mut next, GuiRunOutcome::ReturnedOk);
    }

    #[test]
    fn terminal_requested_before_initialization_is_saved_last_and_published() {
        let directory = tempfile::tempdir().unwrap();
        let identity = test_support::identity();
        let shared = Shared::new();
        shared.state.store(RETURNED_ERROR, Ordering::Release);
        writer(directory.path(), identity, &shared).unwrap();
        let record: Record = bounded_read(
            &directory.path().join(identity.name()).join("state.json"),
            MAX_RECORD_BYTES,
        )
        .unwrap();
        assert_eq!(record.state, RunState::ReturnedError);
        assert_eq!(
            test_support::terminal(directory.path()),
            Some(GuiRunOutcome::ReturnedError)
        );
    }

    #[test]
    fn consecutive_terminal_requests_during_initialization_still_retain_two_runs() {
        let directory = tempfile::tempdir().unwrap();
        for _ in 0..5 {
            let shared = Shared::new();
            shared.state.store(RETURNED_ERROR, Ordering::Release);
            writer(directory.path(), test_support::identity(), &shared).unwrap();
            assert_eq!(
                test_support::terminal(directory.path()),
                Some(GuiRunOutcome::ReturnedError)
            );
        }
        let run_count = std::fs::read_dir(directory.path())
            .unwrap()
            .map(Result::unwrap)
            .filter(|entry| entry.file_type().unwrap().is_dir())
            .count();
        assert_eq!(run_count, 2);
    }

    #[test]
    fn finish_during_gated_initialization_replaces_old_running_reference() {
        let directory = tempfile::tempdir().unwrap();
        let mut previous = GuiRunDiagnostics::new();
        test_support::admit(&mut previous, directory.path());
        test_support::wait_abandoned(previous);
        let old = read_reference(directory.path()).unwrap().current;
        let identity = test_support::identity();
        let shared = Arc::new(Shared::new());
        let worker_shared = shared.clone();
        let root = directory.path().to_path_buf();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (completed_tx, completed_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = worker_shared.writer.set(std::thread::current());
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let result = writer(&root, identity, &worker_shared);
            worker_shared
                .completion
                .store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
            completed_tx.send(result.is_ok()).unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        let mut owner = GuiRunDiagnostics {
            owner: Owner::Admitted {
                shared,
                completed: completed_rx,
            },
        };
        assert!(!owner.finish_until(GuiRunOutcome::ReturnedOk, Instant::now()));
        assert_eq!(read_reference(directory.path()).unwrap().current, old);
        release_tx.send(()).unwrap();
        finish(&mut owner, GuiRunOutcome::ReturnedOk);
        assert_eq!(read_reference(directory.path()).unwrap().current, identity);
        assert_eq!(
            test_support::terminal(directory.path()),
            Some(GuiRunOutcome::ReturnedOk)
        );
        let mut next = GuiRunDiagnostics::new();
        test_support::admit(&mut next, directory.path());
        assert!(next.report_handle().unwrap().take_report().is_none());
        finish(&mut next, GuiRunOutcome::ReturnedOk);
    }

    #[test]
    fn untrusted_details_cannot_enter_the_allowlisted_report() {
        let identity = test_support::identity();
        let record = Record {
            schema: SCHEMA,
            identity,
            version: [4, 10, 0],
            state: RunState::Running,
            last_recorded_utc_ms: now_ms(),
            checkpoint: None,
        };
        let report = format_report(&record);
        for forbidden in [
            "settings.db",
            "C:/",
            "\\Users\\",
            "panic.log",
            "native-exception.log",
            "backtrace",
            "secret.png",
            "search query",
        ] {
            assert!(!report.body().contains(forbidden));
        }
        assert!(report.body().len() < MAX_RECORD_BYTES as usize);
        assert!(serde_json::to_vec(&record).unwrap().len() < 1024);
    }

    #[test]
    fn stage_b_initialization_boundaries_and_late_terminal_are_absorbing() {
        let directory = tempfile::tempdir().unwrap();
        let identity = test_support::identity();
        let shared = Arc::new(Shared::new());
        let publisher = CheckpointPublisher::new(shared.clone());
        let mut lease = Some(publisher.begin_job(
            checkpoint::JobMetadata::default(),
            Arc::new(AtomicBool::new(false)),
        ));
        let mut samples = 0;
        writer_sampling(directory.path(), identity, &shared, |_| {
            samples += 1;
            match samples {
                1 => {
                    // Activity ends while the initial checkpoint is sampled.
                    lease.as_ref().unwrap().update(checkpoint::JobUpdate {
                        stage: checkpoint::JobStage::Completed,
                        ..Default::default()
                    });
                    drop(lease.take());
                }
                2 => {
                    assert_eq!(
                        shared.state.compare_exchange(
                            RUNNING,
                            RETURNED_OK,
                            Ordering::AcqRel,
                            Ordering::Acquire
                        ),
                        Ok(RUNNING)
                    );
                    publisher.publish_display(checkpoint::DisplaySnapshot {
                        context_id: 999,
                        ..Default::default()
                    });
                }
                3 => {}
                _ => panic!("unexpected periodic wake while idle"),
            }
            (
                MemorySample::default(),
                shared.checkpoint.stamp(),
                shared.checkpoint.stamp(),
            )
        })
        .unwrap();
        assert_eq!(samples, 3);
        let record = test_support::record(directory.path()).unwrap();
        assert_eq!(record.state, RunState::ReturnedOk);
        let checkpoint = record.checkpoint.unwrap();
        assert_eq!(checkpoint.active_leases, 0);
        assert_eq!(
            checkpoint.jobs[0].update.stage,
            checkpoint::JobStage::Completed
        );
        assert!(checkpoint.display.is_none());
        // A late producer can never re-open the finished run.
        publisher.publish_display(checkpoint::DisplaySnapshot::default());
        assert_eq!(
            test_support::record(directory.path()).unwrap().state,
            RunState::ReturnedOk
        );
    }

    #[test]
    fn stage_b_report_keeps_old_source_times_and_only_typed_fields() {
        let shared = Arc::new(Shared::new());
        let publisher = CheckpointPublisher::new(shared.clone());
        let lease = publisher.begin_job(
            checkpoint::JobMetadata {
                job_id: 0,
                backend: checkpoint::DiagnosticBackend::Unknown,
                ..Default::default()
            },
            Arc::new(AtomicBool::new(false)),
        );
        lease.update(checkpoint::JobUpdate {
            stage: checkpoint::JobStage::GpuRegistration,
            backend: Some(checkpoint::DiagnosticBackend::DirectMl),
            ..Default::default()
        });
        let mut captured = capture_checkpoint(&shared, &mut |_| {
            (
                MemorySample::default(),
                Stamp {
                    utc_ms: 100,
                    run_elapsed_ms: 10,
                },
                Stamp {
                    utc_ms: 90,
                    run_elapsed_ms: 20,
                },
            )
        });
        captured.stamp = Stamp {
            utc_ms: 900,
            run_elapsed_ms: 800,
        };
        let record = Record {
            schema: SCHEMA,
            identity: test_support::identity(),
            version: [4, 10, 0],
            state: RunState::Running,
            last_recorded_utc_ms: now_ms(),
            checkpoint: Some(captured),
        };
        let report = format_report_at(&record, &[], &[], &[], &|ms| {
            crate::diagnostic_time::LocalDateTime::with_offset(ms, 540)
        });
        assert!(report.body().contains(
            "アプリ専用メモリ確保量（バイト）: 不明 / 1970-01-01 09:00:00 (UTC+09:00) / 記録開始後 0.010 秒"
        ));
        assert!(
            report.body().contains(
                "1970-01-01 09:00:00 (UTC+09:00) / 記録開始後 0.020 秒 / 終了との間隔不明"
            )
        );
        assert!(report.body().contains("処理番号 不明"));
        assert!(report.body().contains("DirectML"));
        assert!(!report.body().contains("秒前"));
        assert!(!report.body().contains("終了原因: OOM"));
        let mut json = serde_json::to_value(&record).unwrap();
        json["checkpoint"]["jobs"][0]["metadata"]["path"] =
            serde_json::json!("C:/private/secret.png");
        assert!(serde_json::from_value::<Record>(json).is_err());
        drop(lease);
    }

    #[test]
    fn stage_b_maximum_numeric_record_and_copy_body_remain_bounded() {
        let shared = Arc::new(Shared::new());
        let publisher = CheckpointPublisher::new(shared.clone());
        let dims = checkpoint::Dimensions {
            width: u32::MAX,
            height: u32::MAX,
        };
        publisher.publish_display(checkpoint::DisplaySnapshot {
            context_id: u64::MAX,
            items_generation: u64::MAX,
            page: Some(u32::MAX),
            total: Some(u32::MAX),
        });
        let leases: Vec<_> = (1..=6)
            .map(|id| {
                let lease = publisher.begin_job(
                    checkpoint::JobMetadata {
                        context_id: Some(u64::MAX),
                        items_generation: Some(u64::MAX),
                        job_id: id,
                        page: Some(u32::MAX),
                        total: Some(u32::MAX),
                        upscale_model: Some(checkpoint::DiagnosticModel::UpscaleRealEsrganAnime6B),
                        denoise_model: Some(checkpoint::DiagnosticModel::DenoiseRealplksr),
                        backend: checkpoint::DiagnosticBackend::DirectMl,
                        source: Some(dims),
                        source_bytes: Some(u64::MAX / 6),
                    },
                    Arc::new(AtomicBool::new(true)),
                );
                lease.update(checkpoint::JobUpdate {
                    stage: checkpoint::JobStage::TileRun,
                    backend: Some(checkpoint::DiagnosticBackend::DirectMl),
                    model: Some(checkpoint::DiagnosticModel::UpscaleRealEsrganAnime6B),
                    output: Some(dims),
                    tile: Some(dims),
                    tile_index: Some(u32::MAX),
                    tile_total: Some(u32::MAX),
                });
                lease
            })
            .collect();
        let record = Record {
            schema: SCHEMA,
            identity: test_support::identity(),
            version: [u16::MAX; 3],
            state: RunState::Running,
            last_recorded_utc_ms: now_ms(),
            checkpoint: Some(capture_checkpoint(&shared, &mut |_| {
                (
                    MemorySample::default(),
                    shared.checkpoint.stamp(),
                    shared.checkpoint.stamp(),
                )
            })),
        };
        assert!(serde_json::to_vec(&record).unwrap().len() < MAX_RECORD_BYTES as usize);
        assert!(format_report(&record).body().len() < MAX_RECORD_BYTES as usize);
        drop(leases);
    }

    #[test]
    fn stage_b_reads_stage_a_records_without_a_checkpoint() {
        let identity = test_support::identity();
        let json = serde_json::json!({
            "schema":SCHEMA, "identity": identity, "version":[4,10,0],
            "state":"Running", "last_recorded_utc_ms":now_ms()
        });
        let record: Record = serde_json::from_value(json).unwrap();
        assert!(record.checkpoint.is_none());
        assert!(format_report(&record).body().contains("終了との間隔不明"));
    }

    #[test]
    fn diagnostic_device_latch_is_saved_by_normal_maintenance_and_clean_suppresses_notice() {
        let directory = tempfile::tempdir().unwrap();
        let mut owner = GuiRunDiagnostics::new();
        test_support::admit(&mut owner, directory.path());
        let publisher = owner.publisher().unwrap();
        let latch = publisher.register_device_loss_source().unwrap();
        let identity = test_support::record(directory.path()).unwrap().identity;
        let private = directory.path().join(identity.name());
        latch.observe(wgpu::DeviceLostReason::Unknown);
        // Only normal maintenance/initialization can consume and save the latch.
        publisher.publish_display(checkpoint::DisplaySnapshot::default());
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if !device_loss::read_observations(&private, identity)
                .unwrap()
                .is_empty()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "normal maintenance must wake idle writer"
            );
            std::thread::yield_now();
        }
        assert_eq!(
            test_support::record(directory.path()).unwrap().state,
            RunState::Running
        );
        finish(&mut owner, GuiRunOutcome::ReturnedOk);
        let reference = read_reference(directory.path()).unwrap();
        assert!(previous_report(directory.path(), &reference).is_none());
        assert_eq!(
            device_loss::read_observations(&private, identity)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn diagnostic_device_failed_save_is_not_retried_and_does_not_replace_panic_evidence() {
        let shared = Shared::new();
        let latch = shared.device_loss.register().unwrap();
        latch.observe(wgpu::DeviceLostReason::Unknown);
        let identity = test_support::identity();
        // A failed open has no retry or recovery path; consume once.
        assert!(observe_devices(&shared, identity, &mut None));
        assert!(!observe_devices(&shared, identity, &mut None));
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("panic-events.log"),
            b"prior panic evidence",
        )
        .unwrap();
        let mut writer = Some(device_loss::Writer::open(directory.path()).unwrap());
        assert!(!observe_devices(&shared, identity, &mut writer));
        let second = shared.device_loss.register().unwrap();
        second.observe(wgpu::DeviceLostReason::Unknown);
        assert!(observe_devices(&shared, identity, &mut writer));
        assert_eq!(
            std::fs::read(directory.path().join("panic-events.log")).unwrap(),
            b"prior panic evidence"
        );
        assert_eq!(
            device_loss::read_observations(directory.path(), identity).unwrap()[0].device_id,
            2
        );
    }

    #[test]
    fn diagnostic_observation_report_is_bounded_and_intervals_are_only_same_run_panic() {
        let report = test_support::stage_c_report_body();
        assert!(report.contains("描画用GPUメモリの確保失敗"));
        assert!(report.contains("保存処理の確認時刻"));
        assert!(report.contains("処理が続いた場合も含む"));
        assert!(report.contains("終了原因かは不明"));
        assert!(report.contains("0.700 秒前（終了との間隔は不明）"));
        assert!(!report.contains("address="));
        assert!(!report.contains("target="));
        let sample = Stamp {
            utc_ms: 100,
            run_elapsed_ms: 10,
        };
        let mut reversed = String::new();
        append_panic_interval(
            &mut reversed,
            "sample",
            sample,
            Stamp {
                utc_ms: 99,
                run_elapsed_ms: 20,
            },
        );
        assert!(reversed.ends_with("間隔: 不明"));
        let mut native = String::new();
        append_observations(
            &mut native,
            &[],
            &[],
            &vec![
                crate::native_exception_log::NativeObservation {
                    utc_ms: 100,
                    code: 0xC0000005
                };
                256
            ],
            &|ms| {
                crate::diagnostic_time::display(crate::diagnostic_time::LocalDateTime::with_offset(
                    ms, 540,
                ))
            },
        );
        assert_eq!(native.matches("処理が続いた場合も含む）:").count(), 8);
        assert!(native.contains("248件を省略"));
        assert!(native.len() < MAX_RECORD_BYTES as usize);
    }

    #[test]
    fn diagnostic_summary_is_short_and_details_keep_all_allowlisted_evidence() {
        let report = test_support::stage_c_report();
        assert_eq!(report.summary().lines().count(), 4);
        assert!(
            report
                .summary()
                .contains("描画用のGPUメモリの確保に失敗していました")
        );
        assert!(
            report
                .summary()
                .contains("画面表示の機能（GPU）が使えなくなっていました")
        );
        for internal in [
            "記録ID",
            "処理番号",
            "活動数",
            "保持入力容量",
            "UTC 179",
            "ミリ秒",
            "直前",
            "原因です",
        ] {
            assert!(!report.summary().contains(internal), "summary: {internal}");
        }
        assert!(report.details().contains("例外番号 0xC0000005"));
        assert!(report.details().contains("処理番号 655"));
        assert!(report.details().contains("(UTC+09:00)"));
        assert!(!report.body().contains("1793340000000"));
        assert!(!report.body().contains("ミリ秒"));
        assert_eq!(
            report.body(),
            format!(
                "{}\n\n詳細（報告用）\n{}",
                report.summary(),
                report.details()
            )
        );

        let report = test_support::checkpoint_report();
        assert_eq!(report.summary().lines().count(), 4);
        assert!(
            report
                .summary()
                .contains("AI処理の実行中でした（ページ 12/40、高速汎用）")
        );
        assert!(
            report
                .summary()
                .contains("描画用GPUメモリ 使用 1907 / 予算 3814 MiB")
        );
    }

    #[test]
    fn diagnostic_summary_never_attributes_background_or_cancelled_jobs_to_current_page() {
        let mut record = test_support::checkpoint_record();
        let render = |record: &Record| {
            format_report_at(record, &[], &[], &[], &|ms| {
                crate::diagnostic_time::LocalDateTime::with_offset(ms, 540)
            })
        };
        record.checkpoint.as_mut().unwrap().jobs[0].cancel_requested = true;
        assert!(!render(&record).summary().contains("AI処理の実行中"));
        record.checkpoint.as_mut().unwrap().jobs[0].cancel_requested = false;
        record.checkpoint.as_mut().unwrap().jobs[0]
            .metadata
            .context_id = Some(4);
        assert!(!render(&record).summary().contains("AI処理の実行中"));
        record.checkpoint.as_mut().unwrap().jobs[0]
            .metadata
            .context_id = Some(3);
        record.checkpoint.as_mut().unwrap().jobs[0].metadata.page = Some(11);
        assert!(!render(&record).summary().contains("AI処理の実行中"));
        record.checkpoint = None;
        let report = render(&record);
        assert_eq!(report.summary().lines().count(), 2);
        assert!(
            report
                .summary()
                .contains("原因は記録されていません。強制終了・電源断")
        );
    }

    #[test]
    fn diagnostic_reports_use_public_labels_and_preserve_numeric_evidence() {
        for report in [
            test_support::checkpoint_report_body(),
            test_support::stage_c_report_body(),
        ] {
            for internal in [
                "context ",
                "世代",
                "job ",
                "source ",
                "unknown",
                "checkpoint",
                "lease ",
                "queue ",
                "backend",
                "runtime",
                "worker",
                "thread",
                "panic",
                "first-chance",
                "upscale ",
                "denoise ",
                "model ",
                "adapter ",
                "AdapterIdentity",
                "SegmentMemory",
                "vendor_id",
                "usage_bytes",
                "budget_bytes",
            ] {
                assert!(!report.contains(internal), "internal term: {internal}");
            }
            for evidence in [
                "画面番号 3 / 更新番号 17 / ページ 12/40",
                "処理番号 655",
                "入力画像の寸法 2400x3600",
                "1200000000",
                "2000000000",
                "4000000000",
                "メーカー番号 4318",
                "機種番号 9860",
                "機器識別番号 10:0",
            ] {
                assert!(report.contains(evidence), "missing evidence: {evidence}");
            }
        }
        let report = test_support::stage_c_report_body();
        assert!(report.contains("例外番号 0xC0000005"));
        assert!(report.contains("0.700 秒前"));
    }

    #[test]
    fn diagnostic_previous_run_report_combines_only_correlated_native_and_device_observations() {
        let directory = tempfile::tempdir().unwrap();
        let logs = tempfile::tempdir().unwrap();
        let mut owner = GuiRunDiagnostics::new();
        test_support::admit(&mut owner, directory.path());
        let record = test_support::record(directory.path()).unwrap();
        let private = directory.path().join(record.identity.name());
        let stamp = Stamp {
            utc_ms: now_ms(),
            run_elapsed_ms: 20,
        };
        device_loss::Writer::open(&private)
            .unwrap()
            .append(
                record.identity,
                device_loss::DeviceObservation {
                    device_id: 1,
                    stamp,
                    reason: device_loss::DeviceReason::Unknown,
                },
            )
            .unwrap();
        test_support::wait_abandoned(owner);
        let id = record.identity;
        let fields = format!(
            "schema=1 run={} pid={} start_utc_ms={} start_ticks={} role=core",
            id.run, id.core_pid, id.core_started_utc_ms, id.core_started_ticks
        );
        let event = format!(
            "[SystemTime {{ intervals: {} }}] NATIVE EXCEPTION chance=first code=0xC0000005 flags=0x0 address=0x1234 thread=4 version=4.10.0 exe_base=0x0 ntdll_base=0x0 {fields}\n",
            (stamp.utc_ms + 11_644_473_600_000) * 10_000
        );
        let path = logs.path().join("native-exception.log");
        std::fs::write(&path, &event).unwrap(); // A line identity alone is insufficient.
        let reference = read_reference(directory.path()).unwrap();
        let uncorrelated =
            previous_report_with_native(directory.path(), &reference, Some(logs.path())).unwrap();
        assert!(uncorrelated.body().contains("描画機能の利用不能（番号 1）"));
        assert!(!uncorrelated.body().contains("処理が続いた場合も含む）："));
        assert!(!uncorrelated.body().contains("アクセス違反"));
        std::fs::write(&path, format!("NATIVE RUN {fields}\n{event}")).unwrap();
        let correlated =
            previous_report_with_native(directory.path(), &reference, Some(logs.path())).unwrap();
        assert!(correlated.body().contains("アクセス違反"));
        assert!(correlated.body().contains("終了原因かは不明"));
        assert!(!correlated.body().contains("0x1234"));
        let mut clean = record;
        clean.state = RunState::ReturnedError;
        replace(&private.join("state.json"), &clean).unwrap();
        assert!(
            previous_report_with_native(directory.path(), &reference, Some(logs.path())).is_none()
        );
    }

    /// Runs only as a single libtest child. Freeze the production initializer
    /// immediately after the sink publication, before ready/retention/native.
    #[test]
    fn diagnostic_startup_panic_child() {
        let Some(directory) = std::env::var_os("MIV_GUI_STARTUP_PANIC_DIR") else {
            return;
        };
        let directory = PathBuf::from(directory);
        let _data_lock = crate::data_dir::test_override_lock();
        crate::data_dir::set_test_override(Some(directory.join("fixture-data")));
        crate::install_panic_log_hook();
        let identity = test_support::identity();
        let shared = Arc::new(Shared::new());
        let gate = Arc::new(std::sync::Barrier::new(2));
        assert!(shared.after_panic_sink.set(gate.clone()).is_ok());
        let worker_shared = shared.clone();
        let worker_directory = directory.clone();
        std::thread::spawn(move || {
            writer_sampling_with_setup(
                &worker_directory,
                identity,
                &worker_shared,
                |_| {
                    let stamp = worker_shared.checkpoint.stamp();
                    (MemorySample::default(), stamp, stamp)
                },
                Some(&worker_directory.join("fixture-data/logs")),
            )
            .unwrap();
        });
        gate.wait();
        assert!(!shared.initialized.load(Ordering::Acquire));
        assert_eq!(read_reference(&directory).unwrap().current, identity);
        assert_eq!(
            test_support::record(&directory).unwrap().state,
            RunState::Running
        );
        // The initializer cannot advance past the second gate. Libtest catches
        // this panic and exits the isolated process without joining that worker.
        panic!("injected GUI panic while production diagnostic initialization is gated");
    }

    #[test]
    fn diagnostic_startup_panic_is_reachable_next_run_before_initialization_finishes() {
        use std::process::{Command, Stdio};
        for has_previous in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let old = if has_previous {
                let mut owner = GuiRunDiagnostics::new();
                test_support::admit(&mut owner, directory.path());
                let old = read_reference(directory.path()).unwrap().current;
                test_support::wait_abandoned(owner);
                Some(old)
            } else {
                None
            };
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "previous_run_diagnostics::tests::diagnostic_startup_panic_child",
                    "--nocapture",
                ])
                .env("MIV_GUI_STARTUP_PANIC_DIR", directory.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let pid = child.id();
            let deadline = Instant::now() + Duration::from_secs(30);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("gated startup panic child timed out");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert!(!status.success());
            let reference =
                read_reference(directory.path()).expect("new run must already be published");
            assert_eq!(reference.previous, old);
            assert_eq!(
                reference.current.core_pid, pid,
                "must reach the crashing run, not the old reference"
            );
            let private = directory.path().join(reference.current.name());
            let observations = fatal::read_observations(&private, reference.current).unwrap();
            assert_eq!(observations.len(), 1);
            assert_eq!(observations[0].kind, fatal::PanicKind::Rust);
            let mut next = GuiRunDiagnostics::new();
            test_support::admit(&mut next, directory.path());
            let report = next.report_handle().unwrap().take_report().unwrap();
            assert!(report.body().contains("プログラム内部のエラー"));
            assert!(report.body().contains(&format!(
                "記録ID {:016x}-core-{}-{:016x}",
                reference.current.run,
                reference.current.core_pid,
                reference.current.core_started_ticks
            )));
            finish(&mut next, GuiRunOutcome::ReturnedOk);
        }
    }

    #[test]
    fn diagnostic_child() {
        let Some(directory) = std::env::var_os("MIV_GUI_DIAGNOSTIC_CHILD_DIR") else {
            return;
        };
        let mode = std::env::var("MIV_GUI_DIAGNOSTIC_CHILD_MODE").unwrap();
        let mut owner = GuiRunDiagnostics::new();
        test_support::admit(&mut owner, Path::new(&directory));
        let _data_lock = crate::data_dir::test_override_lock();
        crate::data_dir::set_test_override(Some(directory.clone().into()));
        let record = test_support::record(Path::new(&directory)).unwrap();
        let Owner::Admitted { shared, .. } = &owner.owner else {
            unreachable!()
        };
        fatal::prepare(
            &Path::new(&directory).join(record.identity.name()),
            record.identity,
            shared.checkpoint.origin(),
        )
        .unwrap();
        crate::install_panic_log_hook();
        println!("GUI_DIAGNOSTIC_READY");
        std::io::stdout().flush().unwrap();
        match mode.as_str() {
            "ok" => finish(&mut owner, GuiRunOutcome::ReturnedOk),
            "error" => finish(&mut owner, GuiRunOutcome::ReturnedError),
            "early-exit" => {
                finish(&mut owner, GuiRunOutcome::ReturnedError);
                std::process::exit(7);
            }
            "panic" => panic!("fixture main panic"),
            "worker-panic-ok" => {
                assert!(
                    std::thread::spawn(|| panic!("fixture worker panic"))
                        .join()
                        .is_err()
                );
                finish(&mut owner, GuiRunOutcome::ReturnedOk);
            }
            "kill" => loop {
                std::thread::park();
            },
            _ => panic!("unknown child fixture"),
        }
    }

    #[test]
    fn bounded_child_process_return_error_early_exit_panic_worker_panic_and_kill() {
        use std::io::BufRead;
        use std::process::{Command, Stdio};
        for (mode, terminal) in [
            ("ok", Some(GuiRunOutcome::ReturnedOk)),
            ("error", Some(GuiRunOutcome::ReturnedError)),
            ("early-exit", Some(GuiRunOutcome::ReturnedError)),
            ("panic", None),
            ("worker-panic-ok", Some(GuiRunOutcome::ReturnedOk)),
            ("kill", None),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "previous_run_diagnostics::tests::diagnostic_child",
                    "--nocapture",
                ])
                .env("MIV_GUI_DIAGNOSTIC_CHILD_DIR", directory.path())
                .env("MIV_GUI_DIAGNOSTIC_CHILD_MODE", mode)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let stdout = child.stdout.take().unwrap();
            let (ready_tx, ready_rx) = mpsc::channel();
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(stdout)
                    .lines()
                    .map_while(Result::ok)
                {
                    if line.contains("GUI_DIAGNOSTIC_READY") {
                        let _ = ready_tx.send(());
                    }
                }
            });
            if ready_rx.recv_timeout(Duration::from_secs(20)).is_err() {
                let _ = child.kill();
                let _ = child.wait();
                panic!("child did not initialize for {mode}");
            }
            if mode == "kill" {
                child.kill().unwrap();
            }
            let wait_deadline = Instant::now() + Duration::from_secs(20);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if Instant::now() >= wait_deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("child did not exit for {mode}");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert_eq!(
                status.success(),
                matches!(mode, "ok" | "error" | "worker-panic-ok"),
                "{mode}"
            );
            assert_eq!(test_support::terminal(directory.path()), terminal, "{mode}");
            let record = test_support::record(directory.path()).unwrap();
            let evidence = fatal::read_observations(
                &directory.path().join(record.identity.name()),
                record.identity,
            )
            .unwrap();
            assert_eq!(
                evidence.len(),
                usize::from(matches!(mode, "panic" | "worker-panic-ok")),
                "{mode}"
            );
            let mut next = GuiRunDiagnostics::new();
            test_support::admit(&mut next, directory.path());
            assert_eq!(
                next.report_handle().unwrap().take_report().is_some(),
                terminal.is_none(),
                "{mode}"
            );
            finish(&mut next, GuiRunOutcome::ReturnedOk);
        }
    }
}
