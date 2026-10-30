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

#[derive(Clone, Debug)]
pub struct PreviousRunReport {
    body: String,
}
impl PreviousRunReport {
    /// The exact same allowlisted text is displayed and explicitly copied.
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
fn diagnostic_optional<T: std::fmt::Debug>(value: Option<T>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |value| format!("{value:?}"))
}
fn diagnostic_dimensions(value: Option<checkpoint::Dimensions>) -> String {
    value.map_or_else(
        || "unknown".to_owned(),
        |value| format!("{}x{}", value.width, value.height),
    )
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
    // The typed schema has no arbitrary diagnostic strings. Formatting is
    // shared by the visible body and the explicit copy action.
    let [major, minor, patch] = record.version;
    let introduction = if panic.is_empty() && devices.is_empty() && native.is_empty() {
        "前回の正常終了を確認できませんでした。原因は記録されていません。"
    } else {
        "前回の正常終了を確認できませんでした。終了前に観測された情報があります。"
    };
    let mut report = PreviousRunReport {
        body: format!(
            "{introduction}\n最終記録時刻: Unix {} ms (UTC) / 終了との間隔不明\nmIV {major}.{minor}.{patch} / 記録ID {}\n強制終了・OS終了・電源断・記録失敗とクラッシュは区別できません。\n最終記録以降に状態が変わった可能性があります。",
            record.last_recorded_utc_ms,
            record.identity.name()
        ),
    };
    append_observations(&mut report.body, panic, devices, native);
    if let Some(checkpoint) = &record.checkpoint {
        use std::fmt::Write as _;
        let _ = write!(
            report.body,
            "\ncheckpoint: UTC {} ms / run {} ms / 終了との間隔不明\n活動 lease {} / job {} / 省略 {} / 更新欠落 {}",
            checkpoint.stamp.utc_ms,
            checkpoint.stamp.run_elapsed_ms,
            checkpoint.active_leases,
            checkpoint.active_jobs,
            checkpoint.omitted_jobs,
            checkpoint.dropped_updates
        );
        report.body.push_str(if checkpoint.active_leases == 0 {
            " / 活動終了 (idle)"
        } else {
            " / 活動中"
        });
        let _ = write!(
            report.body,
            "\nqueue {} / 所有入力bytes {} / 実行 {} / 結果待ち {} / 後段合成 {} / GPU登録要求 {}\n所有入力はArc共有を含み、追加RAM量ではありません。GPU登録要求はGPU完了を意味しません。",
            checkpoint.queued_jobs,
            diagnostic_optional(checkpoint.queued_source_bytes),
            checkpoint.running_jobs,
            checkpoint.result_waiting_jobs,
            checkpoint.final_effect_jobs,
            checkpoint.gpu_registration_jobs
        );
        report
            .body
            .push_str("\nbackendはruntime/推論経路の選択値です。ORT内部のEP配置は未確認です。");
        if let Some(display) = &checkpoint.display {
            let _ = write!(
                report.body,
                "\n表示: context {} / 世代 {} / 項目位置 {}/{} / UTC {} ms / run {} ms / 終了との間隔不明",
                display.value.context_id,
                display.value.items_generation,
                diagnostic_optional(display.value.page),
                diagnostic_optional(display.value.total),
                display.stamp.utc_ms,
                display.stamp.run_elapsed_ms
            );
        }
        for job in &checkpoint.jobs {
            let job_label = if job.metadata.job_id == 0 {
                "job不明".into()
            } else {
                format!("job {}", job.metadata.job_id)
            };
            let backend = job.update.backend.unwrap_or(job.metadata.backend);
            let _ = write!(
                report.body,
                "\n{job_label}: context {} / 世代 {} / 項目位置 {}/{} / {:?} / {:?} / upscale {} / denoise {} / 実行model {} / source {} / 所有入力bytes {} / output {} / tile {} {}/{} / UTC {} ms / run {} ms / 終了との間隔不明\n取消観測: {} / UTC {} ms / run {} ms",
                diagnostic_optional(job.metadata.context_id),
                diagnostic_optional(job.metadata.items_generation),
                diagnostic_optional(job.metadata.page),
                diagnostic_optional(job.metadata.total),
                job.update.stage,
                backend,
                diagnostic_optional(job.metadata.upscale_model),
                diagnostic_optional(job.metadata.denoise_model),
                diagnostic_optional(job.update.model),
                diagnostic_dimensions(job.metadata.source),
                diagnostic_optional(job.metadata.source_bytes),
                diagnostic_dimensions(job.update.output),
                diagnostic_dimensions(job.update.tile),
                diagnostic_optional(job.update.tile_index),
                diagnostic_optional(job.update.tile_total),
                job.stamp.utc_ms,
                job.stamp.run_elapsed_ms,
                job.cancel_requested,
                job.cancel_stamp.utc_ms,
                job.cancel_stamp.run_elapsed_ms
            );
        }
        let cpu = &checkpoint.cpu_private_bytes;
        let cpu_value = cpu
            .value
            .map_or_else(|| "unknown".into(), |value| value.to_string());
        let _ = write!(
            report.body,
            "\nCPU private bytes: {cpu_value} / UTC {} ms / run {} ms / 終了との間隔不明",
            cpu.stamp.utc_ms, cpu.stamp.run_elapsed_ms
        );
        let gpu = &checkpoint.gpu;
        let _ = write!(
            report.body,
            "\n描画 adapter照会: {:?} / adapter {} / node {} / UMA {} / local {} / nonlocal {} / UTC {} ms / run {} ms / 終了との間隔不明\nAI adapter相関: {:?} (AI adapterとの一致未確認)\nGPU使用量・予算だけでは終了原因は判断できません。",
            gpu.value.status,
            diagnostic_optional(gpu.value.adapter),
            diagnostic_optional(gpu.value.node_index),
            diagnostic_optional(gpu.value.uma),
            diagnostic_optional(gpu.value.local),
            diagnostic_optional(gpu.value.nonlocal),
            gpu.stamp.utc_ms,
            gpu.stamp.run_elapsed_ms,
            checkpoint.ai_adapter_correlation
        );
    }
    if let (Some(checkpoint), Some(observation)) = (&record.checkpoint, panic.last()) {
        append_panic_interval(
            &mut report.body,
            "checkpoint",
            checkpoint.stamp,
            observation.stamp,
        );
        if let Some(display) = &checkpoint.display {
            append_panic_interval(
                &mut report.body,
                "表示状態",
                display.stamp,
                observation.stamp,
            );
        }
        append_panic_interval(
            &mut report.body,
            "CPU sample",
            checkpoint.cpu_private_bytes.stamp,
            observation.stamp,
        );
        append_panic_interval(
            &mut report.body,
            "GPU sample",
            checkpoint.gpu.stamp,
            observation.stamp,
        );
        for job in &checkpoint.jobs {
            append_panic_interval(
                &mut report.body,
                &format!("job {}", job.metadata.job_id),
                job.stamp,
                observation.stamp,
            );
        }
    }
    report
}

fn append_observations(
    body: &mut String,
    panic: &[fatal::PanicObservation],
    devices: &[device_loss::DeviceObservation],
    native: &[crate::native_exception_log::NativeObservation],
) {
    use std::fmt::Write as _;
    for event in panic {
        let classification = match event.kind {
            fatal::PanicKind::Rust => "Rust panic（詳細は分類できませんでした）",
            fatal::PanicKind::GraphicsOutOfMemory => "描画用GPUメモリの確保失敗 / Rust panic",
            fatal::PanicKind::GraphicsValidation => "描画APIの検証エラー / Rust panic",
            fatal::PanicKind::GraphicsInternal => "描画APIの内部エラー / Rust panic",
        };
        let _ = write!(
            body,
            "\n終了前に観測された情報: {classification}\npanic観測時刻: UTC {} ms / run {} ms / thread {}",
            event.stamp.utc_ms, event.stamp.run_elapsed_ms, event.thread
        );
    }
    for event in devices {
        let _ = write!(
            body,
            "\n終了前に観測された情報: 描画device {} の喪失 / 理由 unknown\n保存workerの観測時刻: UTC {} ms / run {} ms（callback発生時刻・終了時刻ではありません）",
            event.device_id, event.stamp.utc_ms, event.stamp.run_elapsed_ms
        );
    }
    // Keep presentation bounded even after a run hits the native recorder cap.
    // The local native log retains every event; copy and display share this tail.
    let omitted = native.len().saturating_sub(8);
    if omitted != 0 {
        let _ = write!(body, "\n以前のnative例外観測: {omitted}件を省略");
    }
    for event in native.iter().skip(omitted) {
        let classification = match event.code {
            0xC0000005 => "access violation",
            0xC00000FD => "stack overflow",
            0x80000003 => "breakpoint",
            0xC000001D => "illegal instruction",
            0xC0000094 => "integer divide by zero",
            0xC0000095 => "integer overflow",
            0xC0000374 => "heap corruption",
            _ => "unknown",
        };
        let _ = write!(
            body,
            "\n終了前に観測された例外（first-chance）: {classification} / code 0x{:08X} / UTC {} ms。終了原因かは不明です。",
            event.code, event.utc_ms
        );
    }
    if !panic.is_empty() || !devices.is_empty() || !native.is_empty() {
        body.push_str("\n観測された情報だけでは終了原因を確定できません。背景thread・捕捉されたpanicやfirst-chanceでも記録されます。");
    }
}
fn append_panic_interval(body: &mut String, label: &str, sample: Stamp, panic: Stamp) {
    use std::fmt::Write as _;
    if sample.utc_ms <= panic.utc_ms && sample.run_elapsed_ms <= panic.run_elapsed_ms {
        let _ = write!(
            body,
            "\n{label}採取→同一runの最後のpanic観測: {} ms前（終了との間隔は不明）",
            panic.run_elapsed_ms - sample.run_elapsed_ms
        );
    } else {
        let _ = write!(
            body,
            "\n{label}採取と同一runの最後のpanic観測との間隔: 不明"
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
    // Previous immutable report is secured above, before native rotation/open.
    // Global process-lifetime sinks are prepared only for real GUI admission;
    // ordinary unit fixtures retain local/isolated recorders.
    if let Some(logs) = setup_logs {
        if let Err(error) = fatal::prepare(&run_directory, identity, shared.checkpoint.origin()) {
            log::warn!("panic evidence unavailable: {error}");
        }
        crate::native_exception_log::report_install_result(
            crate::native_exception_log::install_in(logs, Some(identity.process_identity())),
        );
    }
    let mut device_writer = match device_loss::Writer::open(&run_directory) {
        Ok(writer) => Some(writer),
        Err(error) => {
            log::warn!("device evidence unavailable: {error}");
            None
        }
    };
    observe_devices(shared, identity, &mut device_writer);
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
        format_report(&checkpoint_record()).body
    }

    pub(crate) fn stage_c_report_body() -> String {
        let record = checkpoint_record();
        let stamp = Stamp {
            utc_ms: record.last_recorded_utc_ms + 700,
            run_elapsed_ms: 12_700,
        };
        format_report_with_observations(
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
        )
        .body
    }

    fn checkpoint_record() -> Record {
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
                page: Some(11),
                total: Some(40),
                upscale_model: Some(checkpoint::DiagnosticModel::UpscaleRealEsrGeneralV3),
                source: Some(checkpoint::Dimensions {
                    width: 2400,
                    height: 3600,
                }),
                source_bytes: Some(34_560_000),
                ..Default::default()
            },
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
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
            Instant::now(),
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
        let report = format_report(&record);
        assert!(
            report
                .body()
                .contains("CPU private bytes: unknown / UTC 100 ms / run 10 ms")
        );
        assert!(
            report
                .body()
                .contains("UTC 90 ms / run 20 ms / 終了との間隔不明")
        );
        assert!(report.body().contains("job不明"));
        assert!(report.body().contains("DirectMl"));
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
        assert!(report.contains("保存workerの観測時刻"));
        assert!(report.contains("first-chance"));
        assert!(report.contains("終了原因かは不明"));
        assert!(report.contains("700 ms前（終了との間隔は不明）"));
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
        );
        assert_eq!(native.matches("first-chance）:").count(), 8);
        assert!(native.contains("248件を省略"));
        assert!(native.len() < MAX_RECORD_BYTES as usize);
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
        assert!(uncorrelated.body().contains("描画device 1 の喪失"));
        assert!(!uncorrelated.body().contains("first-chance）："));
        assert!(!uncorrelated.body().contains("access violation"));
        std::fs::write(&path, format!("NATIVE RUN {fields}\n{event}")).unwrap();
        let correlated =
            previous_report_with_native(directory.path(), &reference, Some(logs.path())).unwrap();
        assert!(correlated.body().contains("access violation"));
        assert!(correlated.body().contains("終了原因かは不明"));
        assert!(!correlated.body().contains("0x1234"));
        let mut clean = record;
        clean.state = RunState::ReturnedError;
        replace(&private.join("state.json"), &clean).unwrap();
        assert!(
            previous_report_with_native(directory.path(), &reference, Some(logs.path())).is_none()
        );
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
