//! Single file owner for the normal log and optional perf JSONL.
//! Producers never acquire a file lock or wait for delivery. Completion barriers
//! are deliberately separate from the fire-and-forget facade flush operation.
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

const RECORD_LIMIT: usize = 262_144;
const PAYLOAD_LIMIT: usize = 128 * 1024 * 1024;
const RECORD_BYTES: usize = 1024 * 1024;
const BATCH: usize = 256;
const PERF_FLUSH: Duration = Duration::from_millis(250);
const BARRIER_WAIT: Duration = Duration::from_secs(2);
static WRITER: OnceLock<Arc<Service>> = OnceLock::new();
static REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static HOLDER: crate::perf::stall::Holder = crate::perf::stall::Holder::new();

#[derive(Clone, Copy)]
pub(crate) enum Sink {
    Normal = 0,
    Perf = 1,
}
impl Sink {
    fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Perf => "perf",
        }
    }
}

pub(crate) struct NormalConfig {
    pub path: PathBuf,
    pub truncate: bool,
}
pub(crate) struct PerfConfig {
    pub path: PathBuf,
    pub rotate: bool,
    pub start: Instant,
    pub session: Value,
}

#[derive(Serialize, Debug, Clone)]
pub(crate) struct Checkpoint {
    pub schema: u32,
    pub session_id: String,
    pub pid: u32,
    pub process_start_unix_ms: u64,
    pub request_id: String,
    pub purpose: String,
    pub coverage_t: f64,
    pub ticket_cutoff: u64,
    pub settled_watermark: u64,
    pub written_watermark: u64,
    pub normal_status: &'static str,
    pub perf_status: &'static str,
    pub normal_dropped_total: u64,
    pub perf_dropped_total: u64,
    pub diagnostic_dropped_total: u64,
    pub dropped_by_reason:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, u64>>,
}
impl Checkpoint {
    pub fn evidence_incomplete(&self) -> bool {
        self.normal_dropped_total != 0
            || self.perf_dropped_total != 0
            || self.diagnostic_dropped_total != 0
    }
}
pub(crate) struct SnapshotFile {
    pub file: File,
    pub len: u64,
    pub path: PathBuf,
}
pub(crate) struct ExportSnapshot {
    pub normal: SnapshotFile,
    pub perf: Option<SnapshotFile>,
    pub checkpoint: Checkpoint,
}
struct Delivered {
    normal: Option<SnapshotFile>,
    perf: Option<SnapshotFile>,
    checkpoint: Checkpoint,
}

#[derive(Default)]
struct Drops {
    count: AtomicU64,
    first: AtomicU64,
    last: AtomicU64,
}
impl Drops {
    fn note(&self, t: f64) {
        let stamp = (t.max(0.0) * 1_000_000.0) as u64 + 1;
        // The range is cumulative, like count; no reset races can lose a drop.
        let _ = self
            .first
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                Some(if old == 0 { stamp } else { old.min(stamp) })
            });
        self.last.fetch_max(stamp, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Release);
    }
}
const REASONS: [&str; 4] = [
    "records_full",
    "payload_full",
    "oversize",
    "journal_contended",
];
struct Record {
    sink: Sink,
    line: String,
    enqueued_at: Instant,
    tid: u64,
    site: &'static std::panic::Location<'static>,
}
struct Control {
    request_id: String,
    purpose: String,
    coverage_at: Instant,
    cutoff: u64,
    deadline: Instant,
    response: mpsc::Sender<Result<Delivered, String>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Lifecycle {
    Running,
    Closing,
    Stopped,
    Failed,
}
enum ErrorNotice {
    Clear,
    Pending(String),
    Reported,
}

struct Service {
    normal: OnceLock<NormalConfig>,
    perf: OnceLock<PerfConfig>,
    queue: Mutex<VecDeque<Record>>,
    records: AtomicUsize,
    bytes: AtomicUsize,
    record_limit: usize,
    payload_limit: usize,
    drops: [[Drops; 4]; 2],
    epoch: AtomicU64,
    active: [AtomicUsize; 2],
    phase: AtomicU8,
    control_busy: AtomicBool,
    control: Mutex<Option<Control>>,
    flush: AtomicBool,
    thread: OnceLock<std::thread::Thread>,
    completed: (Mutex<()>, Condvar),
    error: Mutex<ErrorNotice>,
    created: u64,
    session_id: String,
    clock: Instant,
    #[cfg(test)]
    hook: Mutex<Option<Arc<test_support::Latch>>>,
    #[cfg(test)]
    fixture_gate: Mutex<()>,
}
impl Service {
    fn new(record_limit: usize, payload_limit: usize) -> Arc<Self> {
        let created = crate::perf::process_start_unix_ms().unwrap_or(0);
        Arc::new(Self {
            normal: OnceLock::new(),
            perf: OnceLock::new(),
            queue: Mutex::new(VecDeque::with_capacity(record_limit)),
            records: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
            record_limit,
            payload_limit,
            drops: std::array::from_fn(|_| std::array::from_fn(|_| Drops::default())),
            epoch: AtomicU64::new(1),
            active: std::array::from_fn(|_| AtomicUsize::new(0)),
            phase: AtomicU8::new(Lifecycle::Running as u8),
            control_busy: AtomicBool::new(false),
            control: Mutex::new(None),
            flush: AtomicBool::new(false),
            thread: OnceLock::new(),
            completed: (Mutex::new(()), Condvar::new()),
            error: Mutex::new(ErrorNotice::Clear),
            created,
            session_id: format!("{}-{created}", std::process::id()),
            clock: Instant::now(),
            #[cfg(test)]
            hook: Mutex::new(None),
            #[cfg(test)]
            fixture_gate: Mutex::new(()),
        })
    }
    fn start(self: &Arc<Self>) {
        let owner = self.clone();
        match std::thread::Builder::new()
            .name("log-writer".into())
            .spawn(move || Worker::new(owner).run())
        {
            Ok(handle) => {
                let _ = self.thread.set(handle.thread().clone()); /* no producer joins */
            }
            Err(error) => {
                self.fail(format!("log writer: {error}"));
                self.complete(false);
            }
        }
    }
    fn clock(&self) -> f64 {
        self.perf.get().map_or_else(
            || self.clock.elapsed().as_secs_f64(),
            |p| p.start.elapsed().as_secs_f64(),
        )
    }
    fn wake(&self) {
        if let Some(thread) = self.thread.get() {
            thread.unpark();
        }
    }
    fn is_running(&self) -> bool {
        self.phase.load(Ordering::SeqCst) == Lifecycle::Running as u8
    }
    fn fail(&self, error: String) {
        let mut status = self.error.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*status, ErrorNotice::Clear) {
            *status = ErrorNotice::Pending(error);
        }
    }
    fn complete(&self, success: bool) {
        let _completion = self.completed.0.lock().unwrap_or_else(|e| e.into_inner());
        self.phase.store(
            if success {
                Lifecycle::Stopped
            } else {
                Lifecycle::Failed
            } as u8,
            Ordering::SeqCst,
        );
        self.completed.1.notify_all();
    }
    fn drop_record(&self, sink: Sink, reason: usize) {
        self.drops[sink as usize][reason].note(self.clock());
        self.wake();
    }
    // Cohort registration is bounded by live producers, rather than keeping an
    // unbounded ledger of tickets while one producer is preempted. No retry.
    fn publication(self: &Arc<Self>, sink: Sink) -> Option<Publication> {
        if match sink {
            Sink::Normal => self.normal.get().is_none(),
            Sink::Perf => self.perf.get().is_none(),
        } {
            return None;
        }
        if !self.is_running() {
            return None;
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        self.active[epoch as usize % 2].fetch_add(1, Ordering::SeqCst);
        #[cfg(test)]
        self.pause("publication-registered");
        if self.epoch.load(Ordering::SeqCst) != epoch || !self.is_running() {
            #[cfg(test)]
            self.pause("registration-drop");
            // A barrier may observe zero immediately after this decrement.
            // Publish the rejected record's counter before settling its cohort.
            self.drop_record(sink, 3);
            self.active[epoch as usize % 2].fetch_sub(1, Ordering::SeqCst);
            self.wake();
            return None;
        }
        Some(Publication {
            service: self.clone(),
            epoch,
            sink,
        })
    }
    fn enqueue(
        &self,
        sink: Sink,
        line: String,
        _t: f64,
        tid: u64,
        site: &'static std::panic::Location<'static>,
    ) -> bool {
        let size = line.len();
        if size > RECORD_BYTES {
            self.drop_record(sink, 2);
            return false;
        }
        if self.records.fetch_add(1, Ordering::AcqRel) >= self.record_limit {
            self.records.fetch_sub(1, Ordering::AcqRel);
            self.drop_record(sink, 0);
            return false;
        }
        if self
            .bytes
            .fetch_add(size, Ordering::AcqRel)
            .saturating_add(size)
            > self.payload_limit
        {
            self.bytes.fetch_sub(size, Ordering::AcqRel);
            self.records.fetch_sub(1, Ordering::AcqRel);
            self.drop_record(sink, 1);
            return false;
        }
        if let Ok(mut queue) = self.queue.try_lock() {
            // Closing is sealed only after all registered publications settle.
            queue.push_back(Record {
                sink,
                line,
                enqueued_at: Instant::now(),
                tid,
                site,
            });
            drop(queue);
            self.wake();
            true
        } else {
            self.bytes.fetch_sub(size, Ordering::AcqRel);
            self.records.fetch_sub(1, Ordering::AcqRel);
            self.drop_record(sink, 3);
            false
        }
    }
    fn barrier(self: &Arc<Self>, request_id: String, purpose: &str) -> Result<Delivered, String> {
        if !self.is_running()
            || self
                .control_busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Err("log delivery busy or closing".into());
        }
        let coverage_at = Instant::now(); // before the cutoff, not after it
        let cutoff = self.epoch.fetch_add(1, Ordering::SeqCst);
        let (response, receiver) = mpsc::channel();
        let deadline = Instant::now() + BARRIER_WAIT;
        *self.control.lock().unwrap_or_else(|e| e.into_inner()) = Some(Control {
            request_id,
            purpose: purpose.into(),
            coverage_at,
            cutoff,
            deadline,
            response,
        });
        self.wake();
        receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| "log delivery deadline exceeded".to_owned())?
    }
    fn snapshot(self: &Arc<Self>) -> Result<ExportSnapshot, String> {
        let id = format!("zip-{}", REQUEST_ID.fetch_add(1, Ordering::Relaxed));
        let Some(mark) = self.publication(Sink::Normal) else {
            return Err("export marker rejected".into());
        };
        let t = self.clock();
        let tid = crate::logger::current_thread_id_num().unwrap_or(0);
        let normal = mark.submit(
            format!("[{t:>8.3}s][t{tid:>3}] diagnostics: export_requested request_id={id}\n"),
            t,
            tid,
            std::panic::Location::caller(),
        );
        let perf = if self.perf.get().is_some() {
            let Some(mark) = self.publication(Sink::Perf) else {
                return Err("export marker rejected".into());
            };
            mark.submit(format!("{}\n", json!({"t":t,"tid":tid,"cat":"diagnostics","kind":"export_requested","request_id":id})), t, tid, std::panic::Location::caller())
        } else {
            true
        };
        if !normal || !perf {
            return Err("export marker dropped".into());
        }
        let delivered = self.barrier(id, "diagnostics")?;
        Ok(ExportSnapshot {
            normal: delivered.normal.ok_or("normal snapshot unavailable")?,
            perf: delivered.perf,
            checkpoint: delivered.checkpoint,
        })
    }
    fn shutdown(&self, deadline: Instant) -> bool {
        let _ = self.phase.compare_exchange(
            Lifecycle::Running as u8,
            Lifecycle::Closing as u8,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        self.wake();
        let done = self.completed.0.lock().unwrap_or_else(|e| e.into_inner());
        let (_done, _) = self
            .completed
            .1
            .wait_timeout_while(
                done,
                deadline.saturating_duration_since(Instant::now()),
                |_| self.phase.load(Ordering::SeqCst) < Lifecycle::Stopped as u8,
            )
            .unwrap_or_else(|e| e.into_inner());
        self.phase.load(Ordering::SeqCst) == Lifecycle::Stopped as u8
    }
    #[cfg(test)]
    fn pause(&self, stage: &'static str) {
        let hook = self.hook.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook.wait(stage);
        }
    }
}
pub(crate) struct Publication {
    service: Arc<Service>,
    epoch: u64,
    sink: Sink,
}
impl Publication {
    pub fn submit(
        self,
        line: String,
        _t: f64,
        tid: u64,
        site: &'static std::panic::Location<'static>,
    ) -> bool {
        #[cfg(test)]
        let _fixture = self.service.fixture_gate.lock().unwrap();
        self.service
            .enqueue(self.sink, line, self.service.clock(), tid, site)
    }
}
impl Drop for Publication {
    fn drop(&mut self) {
        self.service.active[self.epoch as usize % 2].fetch_sub(1, Ordering::SeqCst);
        self.service.wake();
    }
}
fn service() -> &'static Arc<Service> {
    WRITER.get_or_init(|| {
        let service = Service::new(RECORD_LIMIT, PAYLOAD_LIMIT);
        service.start();
        service
    })
}
pub(crate) fn configure_normal(config: NormalConfig) {
    let s = service();
    let _ = s.normal.set(config);
    s.wake();
}
pub(crate) fn configure_perf(mut config: PerfConfig) -> bool {
    let s = service();
    if let Some(object) = config.session.as_object_mut() {
        object.insert("log_delivery_schema".into(), 1.into());
        object.insert("session_id".into(), s.session_id.clone().into());
    }
    if s.perf.set(config).is_err() {
        return false;
    }
    s.wake();
    #[cfg(windows)]
    control_pipe::start(s.clone());
    true
}
pub(crate) fn publication(sink: Sink) -> Option<Publication> {
    WRITER.get()?.publication(sink)
}
pub(crate) fn request_flush() {
    if let Some(s) = WRITER.get() {
        s.flush.store(true, Ordering::Release);
        s.wake();
    }
}
pub(crate) fn diagnostics_snapshot() -> Result<ExportSnapshot, String> {
    WRITER.get().ok_or("log writer not initialized")?.snapshot()
}
pub(crate) fn shutdown_until(deadline: Instant) -> bool {
    WRITER.get().is_none_or(|s| s.shutdown(deadline))
}
pub(crate) fn take_error() -> Option<String> {
    let mut notice = WRITER.get()?.error.try_lock().ok()?;
    if !matches!(*notice, ErrorNotice::Pending(_)) {
        return None;
    }
    match std::mem::replace(&mut *notice, ErrorNotice::Reported) {
        ErrorNotice::Pending(message) => Some(message),
        _ => None,
    }
}

enum FileSink<T> {
    Disabled,
    Ready(T),
    Failed,
}
impl<T> FileSink<T> {
    fn status(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Ready(_) => "ready",
            Self::Failed => "failed",
        }
    }
}
struct PerfFile {
    path: PathBuf,
    file: BufWriter<File>,
}
struct Worker {
    service: Arc<Service>,
    normal: FileSink<crate::logger::LogFile>,
    perf: FileSink<PerfFile>,
    reported: [[u64; 4]; 2],
    perf_dirty: bool,
    last_flush: Instant,
    pending: Option<Control>,
}
impl Worker {
    fn new(service: Arc<Service>) -> Self {
        Self {
            service,
            normal: FileSink::Disabled,
            perf: FileSink::Disabled,
            reported: [[0; 4]; 2],
            perf_dirty: false,
            last_flush: Instant::now(),
            pending: None,
        }
    }
    fn initialize(&mut self) {
        if matches!(self.normal, FileSink::Disabled)
            && let Some(config) = self.service.normal.get()
        {
            let started = Instant::now();
            #[cfg(test)]
            self.service.pause("open");
            match crate::logger::LogFile::open(&config.path, config.truncate) {
                Ok(file) => self.normal = FileSink::Ready(file),
                Err(error) => {
                    self.normal = FileSink::Failed;
                    self.service.fail(format!("normal log: {error}"));
                }
            }
            self.open_completed(Sink::Normal, started, Instant::now());
        }
        if matches!(self.perf, FileSink::Disabled)
            && let Some(config) = self.service.perf.get()
        {
            let started = Instant::now();
            #[cfg(test)]
            self.service.pause("open");
            let result = (|| -> std::io::Result<PerfFile> {
                if let Some(parent) = config.path.parent() {
                    std::fs::create_dir_all(parent)?;
                    if config.rotate {
                        crate::perf::rotate_logs(parent)?;
                    }
                }
                let mut options = OpenOptions::new();
                options.create(true).write(true).truncate(true);
                crate::logger::allow_live_log_reading(&mut options);
                let mut file = BufWriter::with_capacity(64 * 1024, options.open(&config.path)?);
                writeln!(file, "{}", config.session)?;
                file.flush()?;
                Ok(PerfFile {
                    path: config.path.clone(),
                    file,
                })
            })();
            match result {
                Ok(file) => self.perf = FileSink::Ready(file),
                Err(error) => {
                    self.perf = FileSink::Failed;
                    self.service.fail(format!("perf log: {error}"));
                }
            }
            self.open_completed(Sink::Perf, started, Instant::now());
        }
    }
    fn open_completed(&mut self, sink: Sink, started: Instant, ended: Instant) {
        let hold_ms = ended.saturating_duration_since(started).as_secs_f64() * 1000.0;
        if hold_ms < crate::perf::stall::SLOW_MS {
            return;
        }
        // Slow-only, captured on the file owner. Normal initialization can
        // precede perf enablement; its witness still goes to the normal sink.
        let origin = self
            .service
            .perf
            .get()
            .map_or(self.service.clock, |p| p.start);
        let start_t = started.saturating_duration_since(origin).as_secs_f64();
        let end_t = ended.saturating_duration_since(origin).as_secs_f64();
        let tid = crate::logger::current_thread_id_num().unwrap_or(0);
        self.raw(Sink::Normal, &format!("[{end_t:>8.3}s][t{tid:>3}] [async-log] open sink={} start_t={start_t:.6} end_t={end_t:.6} elapsed_ms={hold_ms:.3} io_owner=writer\n", sink.name()));
        self.flush(false);
        self.raw(Sink::Perf, &format!("{}\n", json!({"t":end_t,"tid":tid,"cat":"log","kind":"slow_io","io_owner":"writer","logger":sink.name(),"operation":"open","start_t":start_t,"acquired_t":start_t,"end_t":end_t,"wait_ms":0.0,"hold_ms":hold_ms,"write_ms":0.0,"flush_ms":0.0,"auxiliary_ms":hold_ms,"holder_tid":null,"holder_site":null,"holder_snapshot":"not_applicable_writer_owner","call_site":format!("{}:{}",file!(),line!())})));
    }
    fn raw(&mut self, sink: Sink, line: &str) {
        #[cfg(test)]
        self.service.pause("write");
        #[cfg(test)]
        if matches!(sink, Sink::Normal)
            && matches!(&self.normal, FileSink::Ready(file) if file.needs_rotation(line.len()))
        {
            self.service.pause("rotation");
        }
        let mut probe = crate::perf::stall::IoProbe::start(sink.name(), "writer", &HOLDER);
        if let Some(p) = probe.as_mut() {
            p.acquired(&HOLDER);
        }
        let at = crate::perf::stall::timer(&probe);
        let result = match sink {
            Sink::Normal => match &mut self.normal {
                FileSink::Ready(f) => f.write(line.as_bytes()),
                _ => Ok(()),
            },
            Sink::Perf => match &mut self.perf {
                FileSink::Ready(f) => {
                    self.perf_dirty = true;
                    f.file.write_all(line.as_bytes())
                }
                _ => Ok(()),
            },
        };
        let write_ms = crate::perf::stall::elapsed(at);
        if let Some(end) = crate::perf::stall::retire(&HOLDER, &probe)
            && let Some(p) = probe
        {
            p.finish(end, write_ms, 0.0, 0.0);
        }
        if let Err(error) = result {
            self.failed(sink, error);
        }
    }
    fn failed(&mut self, sink: Sink, error: std::io::Error) {
        match sink {
            Sink::Normal => self.normal = FileSink::Failed,
            Sink::Perf => {
                if let FileSink::Ready(file) = std::mem::replace(&mut self.perf, FileSink::Failed) {
                    // BufWriter::drop would retry the failed write. Discard its
                    // unwritten buffer explicitly; this sink stays unavailable.
                    let (file, _) = file.file.into_parts();
                    drop(file);
                }
            }
        }
        self.service.fail(format!("{} log: {error}", sink.name()));
    }
    fn flush(&mut self, perf: bool) {
        #[cfg(test)]
        self.service.pause("flush");
        let mut probe = crate::perf::stall::IoProbe::start(
            if perf { "perf" } else { "normal" },
            "writer_flush",
            &HOLDER,
        );
        if let Some(p) = probe.as_mut() {
            p.acquired(&HOLDER);
        }
        let at = crate::perf::stall::timer(&probe);
        let result = if perf {
            match &mut self.perf {
                FileSink::Ready(f) => {
                    crate::perf::stall::drain(&mut f.file).and_then(|()| f.file.flush())
                }
                _ => Ok(()),
            }
        } else {
            match &mut self.normal {
                FileSink::Ready(f) => f.flush(),
                _ => Ok(()),
            }
        };
        let flush_ms = crate::perf::stall::elapsed(at);
        if let Some(end) = crate::perf::stall::retire(&HOLDER, &probe)
            && let Some(p) = probe
        {
            p.finish(end, 0.0, flush_ms, 0.0);
        }
        if let Err(error) = result {
            self.failed(if perf { Sink::Perf } else { Sink::Normal }, error);
        }
        if perf {
            self.perf_dirty = false;
            self.last_flush = Instant::now();
        }
    }
    fn batch(&mut self) -> bool {
        let records: Vec<_> = {
            #[cfg(test)]
            let _fixture = self.service.fixture_gate.lock().unwrap();
            let mut queue = self.service.queue.lock().unwrap_or_else(|e| e.into_inner());
            let count = queue.len().min(BATCH);
            queue.drain(..count).collect()
        };
        if records.is_empty() {
            return false;
        }
        // A late facade configuration can arrive between the loop's initial
        // check and dequeue. Open it before delivering its first queued record.
        self.initialize();
        let mut normal_written = false;
        let mut delayed: [Option<(usize, f64, f64, u64, &'static std::panic::Location<'static>)>;
            2] = [None, None];
        for record in records {
            if matches!(record.sink, Sink::Normal) {
                normal_written = true;
            }
            let delay = record.enqueued_at.elapsed().as_secs_f64();
            if delay >= 0.050 {
                let slot = &mut delayed[record.sink as usize];
                let previous = slot.map_or(0, |d| d.0);
                if slot.is_none_or(|d| delay > d.1) {
                    *slot = Some((
                        previous + 1,
                        delay,
                        record
                            .enqueued_at
                            .saturating_duration_since(
                                self.service
                                    .perf
                                    .get()
                                    .map_or(self.service.clock, |p| p.start),
                            )
                            .as_secs_f64(),
                        record.tid,
                        record.site,
                    ));
                } else if let Some(d) = slot {
                    d.0 += 1;
                }
            }
            let len = record.line.len();
            self.raw(record.sink, &record.line);
            self.service.bytes.fetch_sub(len, Ordering::AcqRel);
            self.service.records.fetch_sub(1, Ordering::AcqRel);
        }
        for (sink, delayed) in delayed.into_iter().enumerate() {
            if let Some((count, delay, producer_t, tid, site)) = delayed {
                self.raw(Sink::Perf, &format!("{}\n", json!({"t":self.service.clock(),"cat":"log","kind":"queue_delay","count":count,"ms":delay*1000.0,"producer_t":producer_t,"producer_tid":tid,"producer_file":site.file(),"producer_line":site.line(),"sink":if sink == 0 {"normal"} else {"perf"},"io_owner":"writer"})));
            }
        }
        if normal_written {
            self.flush(false);
        } // intentionally no 250ms normal delay
        true
    }
    fn drop_summaries(&mut self) {
        for sink in [Sink::Normal, Sink::Perf] {
            for (reason, name) in REASONS.iter().enumerate() {
                let d = &self.service.drops[sink as usize][reason];
                let total = d.count.load(Ordering::Acquire);
                let count = total - self.reported[sink as usize][reason];
                if count == 0 {
                    continue;
                }
                let first_t =
                    d.first.load(Ordering::Relaxed).saturating_sub(1) as f64 / 1_000_000.0;
                let last_t = d.last.load(Ordering::Relaxed).saturating_sub(1) as f64 / 1_000_000.0;
                self.reported[sink as usize][reason] = total;
                let t = self.service.clock();
                let tid = crate::logger::current_thread_id_num().unwrap_or(0);
                self.raw(Sink::Normal, &format!("[{t:>8.3}s][t{tid:>3}] [async-log] dropped count={count} total={total} sink={} reason={name} first_t={first_t:.6} last_t={last_t:.6}\n", sink.name()));
                self.raw(Sink::Perf, &format!("{}\n", json!({"t":t,"tid":tid,"cat":"log","kind":"records_dropped","count":count,"total":total,"sink":sink.name(),"reason":name,"first_t":first_t,"last_t":last_t})));
                self.flush(false);
            }
        }
    }
    fn deliver(&mut self, control: &Control) -> Result<Delivered, String> {
        self.drop_summaries();
        self.flush(false);
        self.flush(true);
        // Read each counter once. Producers continue after the cutoff, so an
        // independently sampled total and reason map could disagree.
        let dropped_by_reason: std::collections::BTreeMap<
            String,
            std::collections::BTreeMap<String, u64>,
        > = [Sink::Normal, Sink::Perf]
            .into_iter()
            .map(|sink| {
                (
                    sink.name().into(),
                    REASONS
                        .iter()
                        .enumerate()
                        .map(|(reason, name)| {
                            (
                                (*name).into(),
                                self.service.drops[sink as usize][reason]
                                    .count
                                    .load(Ordering::Acquire),
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        let normal_dropped_total = dropped_by_reason["normal"].values().sum();
        let perf_dropped_total = dropped_by_reason["perf"].values().sum();
        let checkpoint = Checkpoint {
            schema: 1,
            session_id: self.service.session_id.clone(),
            pid: std::process::id(),
            process_start_unix_ms: self.service.created,
            request_id: control.request_id.clone(),
            purpose: control.purpose.clone(),
            coverage_t: control
                .coverage_at
                .saturating_duration_since(
                    self.service
                        .perf
                        .get()
                        .map_or(self.service.clock, |p| p.start),
                )
                .as_secs_f64(),
            ticket_cutoff: control.cutoff,
            settled_watermark: control.cutoff,
            written_watermark: control.cutoff,
            normal_status: self.normal.status(),
            perf_status: self.perf.status(),
            normal_dropped_total,
            perf_dropped_total,
            diagnostic_dropped_total: crate::perf::stall::dropped_total(),
            dropped_by_reason,
        };
        let mut event = serde_json::to_value(&checkpoint).map_err(|e| e.to_string())?;
        let map = event.as_object_mut().unwrap();
        map.insert("cat".into(), "log".into());
        map.insert("kind".into(), "drain_checkpoint".into());
        map.insert("t".into(), self.service.clock().into());
        // The checkpoint is the final proof row. Do not recursively append the
        // act of saving this metadata after its own completion boundary.
        if let FileSink::Ready(file) = &mut self.perf {
            if let Err(error) = writeln!(file.file, "{event}").and_then(|()| file.file.flush()) {
                self.failed(Sink::Perf, error);
            }
            self.perf_dirty = false;
            self.last_flush = Instant::now();
        }
        if self.normal.status() == "failed" || self.perf.status() == "failed" {
            return Err("log sink failed".into());
        }
        let normal = match &self.normal {
            FileSink::Ready(f) => Some(snapshot(&f.path)?),
            _ => None,
        };
        let perf = match &self.perf {
            FileSink::Ready(f) => Some(snapshot(&f.path)?),
            _ => None,
        };
        Ok(Delivered {
            normal,
            perf,
            checkpoint,
        })
    }
    fn run(mut self) {
        // Register before any IO; a wake issued during initialization is retained.
        let _ = self.service.thread.set(std::thread::current());
        loop {
            self.initialize();
            if self.pending.is_none() {
                self.pending = self
                    .service
                    .control
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take();
            }
            #[cfg(test)]
            if self.pending.is_some() {
                self.service.pause("control");
            }
            let had_batch = self.batch();
            if had_batch {
                self.drop_summaries();
                #[cfg(test)]
                self.service.pause("resumed-batch");
            }
            if !self.service.is_running() {
                if let Some(control) = self.pending.take() {
                    let _ = control.response.send(Err("log writer closing".into()));
                    self.service.control_busy.store(false, Ordering::Release);
                }
                if self
                    .service
                    .active
                    .iter()
                    .all(|a| a.load(Ordering::SeqCst) == 0)
                    && self.service.records.load(Ordering::Acquire) == 0
                {
                    let (response, _) = mpsc::channel();
                    let control = Control {
                        request_id: format!(
                            "shutdown-{}",
                            REQUEST_ID.fetch_add(1, Ordering::Relaxed)
                        ),
                        purpose: "shutdown".into(),
                        coverage_at: Instant::now(),
                        cutoff: self.service.epoch.load(Ordering::SeqCst),
                        deadline: Instant::now(),
                        response,
                    };
                    let ok = self.deliver(&control).is_ok();
                    self.normal = FileSink::Disabled;
                    self.perf = FileSink::Disabled;
                    self.service.complete(ok);
                    return;
                }
            } else if let Some(control) = self.pending.as_ref() {
                // Do not recycle a cohort slot until all old publications settled,
                // even when the requesting worker already timed out.
                if self.service.active[control.cutoff as usize % 2].load(Ordering::SeqCst) == 0 {
                    let control = self.pending.take().unwrap();
                    let result = if Instant::now() >= control.deadline {
                        Err("log delivery deadline exceeded".into())
                    } else {
                        // Snapshot bounded accepted tail; later producers cannot
                        // keep this barrier waiting for an ever-empty journal.
                        let tail = self
                            .service
                            .queue
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .len();
                        for _ in 0..tail.div_ceil(BATCH) {
                            self.batch();
                        }
                        self.deliver(&control)
                    };
                    let _ = control.response.send(result);
                    self.service.control_busy.store(false, Ordering::Release);
                }
            }
            if self.service.flush.swap(false, Ordering::AcqRel) {
                let tail = self
                    .service
                    .queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .len();
                for _ in 0..tail.div_ceil(BATCH) {
                    self.batch();
                }
                self.drop_summaries();
                self.flush(false);
                self.flush(true);
            }
            if self.perf_dirty && self.last_flush.elapsed() >= PERF_FLUSH {
                self.drop_summaries();
                self.flush(true);
            }
            if !had_batch {
                self.drop_summaries();
                std::thread::park_timeout(if self.perf_dirty {
                    PERF_FLUSH
                        .saturating_sub(self.last_flush.elapsed())
                        .max(Duration::from_millis(1))
                } else {
                    PERF_FLUSH
                });
            }
        }
    }
}
fn snapshot(path: &Path) -> Result<SnapshotFile, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    crate::logger::allow_live_log_reading(&mut options);
    let file = options.open(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    Ok(SnapshotFile {
        file,
        len,
        path: path.into(),
    })
}

#[cfg(windows)]
mod control_pipe;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    #[derive(Clone)]
    pub struct TestWriter {
        pub(super) service: Arc<Service>,
    }
    pub fn install_global(writer: &TestWriter) {
        assert!(WRITER.set(writer.service.clone()).is_ok());
        crate::logger::install_test_clock(writer.service.clock);
        crate::perf::install_test_clock(writer.service.perf.get().unwrap().start);
    }
    pub fn writer(root: &Path, record_limit: usize, payload_limit: usize) -> TestWriter {
        let service = Service::new(record_limit, payload_limit);
        service
            .normal
            .set(NormalConfig {
                path: root.join("mimageviewer.log"),
                truncate: true,
            })
            .ok();
        service.perf.set(PerfConfig { path: root.join("perf_events.jsonl"), rotate: false, start: service.clock, session: json!({"t":0,"cat":"session","kind":"start","pid":std::process::id(),"process_start_unix_ms":service.created,"log_delivery_schema":1,"session_id":service.session_id}) }).ok();
        service.start();
        service.barrier("test-ready".into(), "test").unwrap();
        TestWriter { service }
    }
    impl TestWriter {
        #[track_caller]
        pub fn submit_normal(&self, text: &str) -> bool {
            let Some(p) = self.service.publication(Sink::Normal) else {
                return false;
            };
            p.submit(
                format!("{text}\n"),
                self.service.clock(),
                crate::logger::current_thread_id_num().unwrap_or(0),
                std::panic::Location::caller(),
            )
        }
        #[track_caller]
        pub fn submit_perf(&self, mut event: Value) -> bool {
            let Some(p) = self.service.publication(Sink::Perf) else {
                return false;
            };
            let t = self.service.clock();
            let tid = crate::logger::current_thread_id_num().unwrap_or(0);
            event["t"] = t.into();
            event["tid"] = tid.into();
            p.submit(format!("{event}\n"), t, tid, std::panic::Location::caller())
        }
        pub fn snapshot(&self) -> Result<ExportSnapshot, String> {
            self.service.snapshot()
        }
        pub fn shutdown(&self) -> bool {
            self.service.shutdown(Instant::now() + BARRIER_WAIT)
        }
        pub fn force_drop(&self) {
            let _ = self.submit_normal(&"x".repeat(RECORD_BYTES + 1));
        }
        pub fn pause_write(&self) -> Pause {
            self.pause("write")
        }
        pub fn pause(&self, stage: &'static str) -> Pause {
            pause_service(&self.service, stage)
        }
    }
    pub(super) fn pause_service(service: &Arc<Service>, stage: &'static str) -> Pause {
        let latch = Arc::new(Latch {
            stage,
            state: Mutex::new((false, false)),
            changed: Condvar::new(),
        });
        *service.hook.lock().unwrap() = Some(latch.clone());
        Pause { latch }
    }
    pub struct Latch {
        stage: &'static str,
        state: Mutex<(bool, bool)>,
        changed: Condvar,
    }
    impl Latch {
        pub(super) fn wait(&self, stage: &str) {
            if stage != self.stage {
                return;
            }
            let mut state = self.state.lock().unwrap();
            if state.1 {
                return;
            }
            state.0 = true;
            self.changed.notify_all();
            drop(self.changed.wait_while(state, |s| !s.1).unwrap());
        }
    }
    pub struct Pause {
        latch: Arc<Latch>,
    }
    impl Pause {
        pub fn entered(&self) {
            let state = self.latch.state.lock().unwrap();
            let (state, _) = self
                .latch
                .changed
                .wait_timeout_while(state, BARRIER_WAIT, |s| !s.0)
                .unwrap();
            assert!(state.0, "writer did not reach latch");
        }
    }
    impl Drop for Pause {
        fn drop(&mut self) {
            self.latch.state.lock().unwrap().1 = true;
            self.latch.changed.notify_all();
        }
    }
}
