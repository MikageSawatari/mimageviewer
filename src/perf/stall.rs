//! Slow-only stall witnesses. Producers never acquire either logger mutex here.
//! A bounded queue is drained by the existing perf writer, without recursive event calls.
use serde_json::Value;
use std::cell::RefCell;
use std::io::Write;
use std::panic::Location;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

pub(crate) const SLOW_MS: f64 = 50.0;
const QUEUE_CAPACITY: usize = 128;
static DIAGNOSTICS: OnceLock<DiagnosticQueue> = OnceLock::new();

struct DiagnosticRecords {
    records: [Option<SlowIo>; QUEUE_CAPACITY],
    len: usize,
}

impl DiagnosticRecords {
    fn empty() -> Self {
        Self {
            records: std::array::from_fn(|_| None),
            len: 0,
        }
    }
}

/// The queue lock is never waited on. A preempted producer cannot stall the
/// perf writer: a drain makes one try_lock attempt and leaves records for later.
struct DiagnosticQueue {
    records: Mutex<DiagnosticRecords>,
    queued: AtomicBool,
    dropped: AtomicU64,
}

impl DiagnosticQueue {
    fn new() -> Self {
        Self {
            records: Mutex::new(DiagnosticRecords::empty()),
            queued: AtomicBool::new(false),
            dropped: AtomicU64::new(0),
        }
    }

    fn enqueue(&self, record: SlowIo) {
        if let Ok(mut batch) = self.records.try_lock()
            && batch.len < QUEUE_CAPACITY
        {
            let next = batch.len;
            batch.records[next] = Some(record);
            batch.len += 1;
            self.queued.store(true, Ordering::Release);
            return;
        }
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }

    fn take(&self) -> Option<DiagnosticRecords> {
        if !self.queued.load(Ordering::Acquire) {
            return None;
        }
        let mut batch = self.records.try_lock().ok()?;
        let records = std::mem::replace(&mut *batch, DiagnosticRecords::empty());
        self.queued.store(false, Ordering::Release);
        Some(records)
    }
}

pub(super) fn init() {
    DIAGNOSTICS.get_or_init(DiagnosticQueue::new);
}

/// All locations are static. The sequence protects a best-effort single-read snapshot;
/// it never spins or waits while a holder is publishing/retiring its identity.
pub(crate) struct Holder {
    version: AtomicU64,
    tid: AtomicU64,
    site: AtomicPtr<Location<'static>>,
}

impl Holder {
    pub(crate) const fn new() -> Self {
        Self {
            version: AtomicU64::new(0),
            tid: AtomicU64::new(0),
            site: AtomicPtr::new(std::ptr::null_mut()),
        }
    }

    fn snapshot(&self) -> Option<(u64, &'static Location<'static>)> {
        // SeqCst keeps the two payload loads inside the version observations.
        let version = self.version.load(Ordering::SeqCst);
        if version & 1 != 0 {
            return None;
        }
        let tid = self.tid.load(Ordering::SeqCst);
        let site = self.site.load(Ordering::SeqCst);
        if tid == 0 || site.is_null() || version != self.version.load(Ordering::SeqCst) {
            return None;
        }
        // SAFETY: publish accepts only a static Location::caller(), never freed.
        Some((tid, unsafe { &*site }))
    }

    fn publish(&self, tid: u64, site: &'static Location<'static>) {
        self.version.fetch_add(1, Ordering::SeqCst);
        self.site
            .store(std::ptr::from_ref(site).cast_mut(), Ordering::SeqCst);
        self.tid.store(tid, Ordering::SeqCst);
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    fn clear(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
        self.tid.store(0, Ordering::SeqCst);
        self.version.fetch_add(1, Ordering::SeqCst);
    }
}

pub(crate) struct IoProbe {
    logger: &'static str,
    operation: &'static str,
    site: &'static Location<'static>,
    tid: u64,
    started: Instant,
    acquired: Instant,
    holder: Option<(u64, &'static Location<'static>)>,
}

impl IoProbe {
    #[track_caller]
    pub(crate) fn start(
        logger: &'static str,
        operation: &'static str,
        holder: &Holder,
    ) -> Option<Self> {
        if !super::is_enabled() {
            return None;
        }
        let started = Instant::now();
        Some(Self {
            logger,
            operation,
            site: Location::caller(),
            tid: crate::logger::current_thread_id_num().unwrap_or(0),
            started,
            acquired: started,
            holder: holder.snapshot(),
        })
    }

    pub(crate) fn acquired(&mut self, holder: &Holder) {
        self.acquired = Instant::now();
        holder.publish(self.tid, self.site);
    }

    /// Call after releasing the measured guard; no file I/O or logger call occurs here.
    pub(crate) fn finish(self, ended: Instant, write_ms: f64, flush_ms: f64, auxiliary_ms: f64) {
        let Some(record) = self.slow_record(ended, write_ms, flush_ms, auxiliary_ms) else {
            return;
        };
        if let Some(queue) = DIAGNOSTICS.get() {
            queue.enqueue(record);
        }
    }

    fn slow_record(
        self,
        ended: Instant,
        write_ms: f64,
        flush_ms: f64,
        auxiliary_ms: f64,
    ) -> Option<SlowIo> {
        let wait_ms = millis(self.acquired.duration_since(self.started));
        let hold_ms = millis(ended.duration_since(self.acquired));
        if !slow_io(wait_ms, hold_ms) {
            return None;
        }
        Some(SlowIo {
            probe: self,
            ended,
            wait_ms,
            hold_ms,
            write_ms,
            flush_ms,
            auxiliary_ms,
        })
    }
}

fn slow_io(wait_ms: f64, hold_ms: f64) -> bool {
    wait_ms >= SLOW_MS || hold_ms >= SLOW_MS
}

struct SlowIo {
    probe: IoProbe,
    ended: Instant,
    wait_ms: f64,
    hold_ms: f64,
    write_ms: f64,
    flush_ms: f64,
    auxiliary_ms: f64,
}

fn site_tag(site: &Location<'_>) -> String {
    let file = site
        .file()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(site.file());
    format!("{file}:{}", site.line())
}

pub(crate) fn seconds(at: Instant) -> f64 {
    super::START
        .get()
        .map(|start| at.saturating_duration_since(*start).as_secs_f64())
        .unwrap_or(0.0)
}

impl SlowIo {
    fn json(&self) -> Value {
        let p = &self.probe;
        serde_json::json!({
            "cat": "log", "kind": "slow_io", "t": seconds(self.ended), "tid": p.tid,
            "logger": p.logger, "operation": p.operation, "call_site": site_tag(p.site),
            "start_t": seconds(p.started), "acquired_t": seconds(p.acquired), "end_t": seconds(self.ended),
            "wait_ms": self.wait_ms, "hold_ms": self.hold_ms,
            "write_ms": self.write_ms, "flush_ms": self.flush_ms,
            "auxiliary_ms": self.auxiliary_ms,
            "holder_tid": p.holder.map(|h| h.0), "holder_site": p.holder.map(|h| site_tag(h.1)),
            "holder_snapshot": "at_wait_start_best_effort"
        })
    }
}

/// Already under the perf file guard. Bounded work, raw write path, no event()/flush()
/// recursion. Its time is included in the caller's hold and auxiliary_ms witness.
pub(super) fn drain(writer: &mut impl Write) {
    let Some(queue) = DIAGNOSTICS.get() else {
        return;
    };
    drain_queue(writer, queue);
}

fn drain_queue(writer: &mut impl Write, queue: &DiagnosticQueue) {
    // take() releases the queue guard before serialization or any file write.
    if let Some(batch) = queue.take() {
        for record in batch.records.into_iter().take(batch.len).flatten() {
            let _ = writeln!(writer, "{}", record.json());
        }
    }
    let dropped = queue.dropped.swap(0, Ordering::Relaxed);
    if dropped != 0 {
        let _ = writeln!(
            writer,
            "{}",
            serde_json::json!({
                "t": seconds(Instant::now()), "cat": "log", "kind": "diagnostic_dropped", "count": dropped
            })
        );
    }
}

pub(crate) fn retire(holder: &Holder, probe: &Option<IoProbe>) -> Option<Instant> {
    probe.as_ref().map(|_| {
        holder.clear();
        Instant::now()
    })
}

pub(crate) fn timer(probe: &Option<IoProbe>) -> Option<Instant> {
    probe.as_ref().map(|_| Instant::now())
}

pub(crate) fn elapsed(start: Option<Instant>) -> f64 {
    start.map(|at| millis(at.elapsed())).unwrap_or(0.0)
}

fn millis(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Timing {
    pub(crate) ms: f64,
    pub(crate) cycles: u64,
}

pub(crate) struct Span {
    at: Instant,
}
impl Span {
    pub(crate) fn start() -> Option<Self> {
        Self::start_with(super::is_enabled(), Instant::now)
    }
    fn start_with(enabled: bool, now: impl FnOnce() -> Instant) -> Option<Self> {
        enabled.then(|| Self { at: now() })
    }
    pub(crate) fn finish(self) -> Timing {
        Timing {
            ms: millis(self.at.elapsed()),
            cycles: 0, // Wall-only worker candidate; never emitted as a cycle sample.
        }
    }
}

pub(crate) struct TotalCycles(u64);
impl TotalCycles {
    pub(crate) fn start() -> Option<Self> {
        Self::start_with(super::is_enabled(), crate::app::App::thread_cycles_now)
    }
    fn start_with(enabled: bool, read: impl FnOnce() -> u64) -> Option<Self> {
        enabled.then(|| Self(read()))
    }
    pub(crate) fn finish(self) -> u64 {
        self.finish_with(crate::app::App::thread_cycles_now)
    }
    fn finish_with(self, read: impl FnOnce() -> u64) -> u64 {
        read().saturating_sub(self.0)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum PollPart {
    Other,
    DetailsMeta,
    SearchDebounce,
    SearchEvents,
    PreparedAdoption,
    TagPrewarm,
    VideoPinFetch,
}
const PARTS: [&str; 7] = [
    "other",
    "details_meta",
    "search_debounce",
    "global_search_events",
    "prepared_adoption",
    "tag_prewarm",
    "video_pin_fetch",
];

struct PollTimings {
    started: Instant,
    started_cycles: u64,
    last: Instant,
    last_cycles: u64,
    current: PollPart,
    parts: [Timing; 7],
}
impl PollTimings {
    fn switch_at(&mut self, next: PollPart, now: Instant, cycles: u64) -> PollPart {
        let prev = self.current;
        let part = &mut self.parts[prev as usize];
        part.ms += millis(now.duration_since(self.last));
        part.cycles += cycles.saturating_sub(self.last_cycles);
        self.last = now;
        self.last_cycles = cycles;
        self.current = next;
        prev
    }
    fn switch(&mut self, next: PollPart) -> PollPart {
        self.switch_at(next, Instant::now(), poll_cycles_now())
    }
}
thread_local! { static POLLS: RefCell<Option<PollTimings>> = const { RefCell::new(None) }; }

fn poll_cycles_now() -> u64 {
    #[cfg(test)]
    TEST_POLL_READS.with(|count| {
        if let Some(reads) = count.get() {
            count.set(Some(reads + 1));
        }
    });
    crate::app::App::thread_cycles_now()
}
#[cfg(test)]
thread_local! { static TEST_POLL_READS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
pub(crate) fn count_poll_reads(work: impl FnOnce()) -> usize {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_POLL_READS.with(|c| c.set(None));
        }
    }
    TEST_POLL_READS.with(|c| {
        assert!(c.get().is_none());
        c.set(Some(0));
    });
    let _reset = Reset;
    work();
    TEST_POLL_READS.with(|c| c.get().unwrap())
}

pub(crate) struct OtherWorkerScope {
    n: u64,
}
impl OtherWorkerScope {
    /// Reuse the enclosing update recorder's boundary, with no new OS sample.
    pub(crate) fn start_at(n: u64, at: Instant, cycles: u64) -> Self {
        POLLS.with(|cell| {
            assert!(
                cell.borrow().is_none(),
                "nested OtherWorkerPolls instrumentation"
            );
            *cell.borrow_mut() = Some(PollTimings {
                started: at,
                started_cycles: cycles,
                last: at,
                last_cycles: cycles,
                current: PollPart::Other,
                parts: [Timing::default(); 7],
            });
        });
        Self { n }
    }
    pub(crate) fn finish_at(self, at: Instant, cycles: u64) {
        let Some(mut timings) = POLLS.with(|cell| cell.borrow_mut().take()) else {
            return;
        };
        timings.switch_at(PollPart::Other, at, cycles);
        let total_ms = millis(timings.last.duration_since(timings.started));
        if total_ms < SLOW_MS {
            return;
        }
        let mut extras = vec![
            ("n".to_owned(), Value::from(self.n)),
            ("total_ms".to_owned(), Value::from(total_ms)),
            (
                "total_cycles".to_owned(),
                Value::from(timings.last_cycles.saturating_sub(timings.started_cycles)),
            ),
            ("start_t".to_owned(), Value::from(seconds(timings.started))),
            ("end_t".to_owned(), Value::from(seconds(timings.last))),
        ];
        for (name, timing) in PARTS.iter().zip(timings.parts) {
            extras.push((format!("{name}_ms"), Value::from(timing.ms)));
            extras.push((format!("{name}_cycles"), Value::from(timing.cycles)));
        }
        let refs = extras
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect::<Vec<_>>();
        super::event("ui", "other_worker_polls_breakdown", None, 0, &refs);
    }
}
impl Drop for OtherWorkerScope {
    fn drop(&mut self) {
        // An unwound/abandoned envelope must retire TLS, without another sample.
        POLLS.with(|cell| {
            cell.borrow_mut().take();
        });
    }
}

pub(crate) struct PollSection {
    previous: PollPart,
}
impl PollSection {
    pub(crate) fn start(part: PollPart) -> Option<Self> {
        let enabled = super::is_enabled();
        #[cfg(test)]
        let enabled = enabled || TEST_POLL_READS.with(|c| c.get().is_some());
        if !enabled {
            return None;
        }
        Self::start_with(part, Instant::now, poll_cycles_now)
    }
    fn start_with(
        part: PollPart,
        now: impl FnOnce() -> Instant,
        cycles: impl FnOnce() -> u64,
    ) -> Option<Self> {
        POLLS.with(|cell| {
            let mut state = cell.borrow_mut();
            let timings = state.as_mut()?;
            // A nested search poll can already be inside this same section.
            if timings.current as usize == part as usize {
                return None;
            }
            Some(Self {
                previous: timings.switch_at(part, now(), cycles()),
            })
        })
    }
    pub(crate) fn ensure(slot: &mut Option<Self>, part: PollPart) {
        if slot.is_none() {
            *slot = Self::start(part);
        }
    }
}
impl Drop for PollSection {
    fn drop(&mut self) {
        POLLS.with(|cell| {
            if let Some(timings) = cell.borrow_mut().as_mut() {
                timings.switch(self.previous);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn threshold_is_wait_or_hold_inclusive() {
        assert!(!slow_io(49.99, 49.99));
        assert!(slow_io(50.0, 0.0));
        assert!(slow_io(0.0, 50.0));
    }
    #[test]
    fn emit_decision_preserves_measured_wait_hold_and_io_parts() {
        let at = Instant::now();
        let probe = |wait| IoProbe {
            logger: "perf",
            operation: "flush",
            site: Location::caller(),
            tid: 42,
            started: at,
            acquired: at + std::time::Duration::from_millis(wait),
            holder: None,
        };
        assert!(
            probe(1)
                .slow_record(at + std::time::Duration::from_millis(2), 0.0, 1.0, 0.0)
                .is_none()
        );
        let record = probe(800)
            .slow_record(at + std::time::Duration::from_millis(802), 0.5, 1.0, 0.5)
            .unwrap();
        assert_eq!(record.wait_ms, 800.0);
        assert_eq!(record.hold_ms, 2.0);
        let json = record.json();
        assert_eq!(json["logger"], "perf");
        assert_eq!(json["operation"], "flush");
        assert_eq!(json["auxiliary_ms"], 0.5);
        assert_eq!(json["holder_tid"], Value::Null);
    }
    fn test_record() -> SlowIo {
        let at = Instant::now();
        SlowIo {
            probe: IoProbe {
                logger: "normal",
                operation: "log",
                site: Location::caller(),
                tid: 7,
                started: at,
                acquired: at,
                holder: Some((9, Location::caller())),
            },
            ended: at,
            wait_ms: 800.0,
            hold_ms: 1.0,
            write_ms: 0.5,
            flush_ms: 0.5,
            auxiliary_ms: 0.0,
        }
    }
    #[test]
    fn queue_is_bounded_and_reports_actual_loss() {
        let queue = DiagnosticQueue::new();
        for _ in 0..QUEUE_CAPACITY {
            queue.enqueue(test_record());
        }
        queue.enqueue(test_record());
        assert_eq!(queue.dropped.load(Ordering::Relaxed), 1);
        let batch = queue.take().unwrap();
        assert_eq!(batch.len, QUEUE_CAPACITY);
        let json = batch.records[0].as_ref().unwrap().json();
        assert_eq!(json["tid"], 7);
        assert_eq!(json["holder_tid"], 9);
        assert_eq!(json["wait_ms"], 800.0);
        assert!(queue.take().is_none());
        queue.enqueue(test_record());
        assert_eq!(queue.take().unwrap().len, 1);
    }
    #[test]
    fn preempted_producer_cannot_hold_up_drain_or_another_producer() {
        let queue = std::sync::Arc::new(DiagnosticQueue::new());
        queue.enqueue(test_record());
        let held = queue.records.lock().unwrap();
        let other = std::sync::Arc::clone(&queue);
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut out = Vec::new();
            drain_queue(&mut out, &other);
            assert!(
                out.is_empty(),
                "drain contention must preserve the queued record"
            );
            other.enqueue(test_record());
            tx.send(()).unwrap();
        });
        // The producer's lock stays held until the drain has returned. A blocking
        // queue implementation fails the deadline instead of hanging this test.
        let returned = rx.recv_timeout(std::time::Duration::from_secs(2));
        drop(held);
        thread.join().unwrap();
        returned.expect("queue path waited for a preempted producer");
        assert_eq!(queue.dropped.load(Ordering::Relaxed), 1);
        assert_eq!(queue.take().unwrap().len, 1);
    }
    #[test]
    fn drain_releases_queue_guard_before_writing() {
        struct Writer<'a>(&'a DiagnosticQueue);
        impl Write for Writer<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                assert!(self.0.records.try_lock().is_ok());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let queue = DiagnosticQueue::new();
        queue.enqueue(test_record());
        drain_queue(&mut Writer(&queue), &queue);
        assert!(queue.take().is_none());
    }
    #[test]
    fn worker_candidates_use_wall_only_and_total_uses_two_cycle_reads() {
        let reads = std::cell::Cell::new(0);
        let read = || {
            let n = reads.get() + 1;
            reads.set(n);
            n * 100
        };
        assert!(TotalCycles::start_with(false, read).is_none());
        assert!(Span::start_with(false, || panic!("disabled clock read")).is_none());
        let total = TotalCycles::start_with(true, read).unwrap();
        for _ in 0..5 {
            let span = Span::start_with(true, Instant::now).unwrap();
            assert_eq!(span.finish().cycles, 0);
        }
        assert_eq!(total.finish_with(read), 100);
        assert_eq!(reads.get(), 2);
    }
    #[test]
    fn envelope_reuses_marks_and_abandon_retires_without_cycle_reads() {
        let at = Instant::now();
        let reads = count_poll_reads(|| {
            let scope = OtherWorkerScope::start_at(1, at, 1_000);
            scope.finish_at(at + std::time::Duration::from_millis(1), 1_100);
            assert!(POLLS.with(|c| c.borrow().is_none()));
            let scope = OtherWorkerScope::start_at(2, at, 1_000);
            drop(scope);
            assert!(POLLS.with(|c| c.borrow().is_none()));
        });
        assert_eq!(reads, 0);
    }
    #[test]
    fn same_section_and_absent_envelope_do_not_resample_cycles() {
        assert!(
            PollSection::start_with(PollPart::Other, || panic!("clock"), || panic!("cycles"))
                .is_none()
        );
        let scope = OtherWorkerScope::start_at(1, Instant::now(), 1_000);
        assert!(
            PollSection::start_with(PollPart::Other, || panic!("clock"), || panic!("cycles"))
                .is_none()
        );
        drop(scope);
    }
    #[test]
    fn holder_snapshot_retires_and_replaces_without_stale_identity() {
        let holder = Holder::new();
        assert!(holder.snapshot().is_none());
        holder.publish(7, Location::caller());
        assert_eq!(holder.snapshot().unwrap().0, 7);
        holder.clear();
        assert!(holder.snapshot().is_none());
        holder.publish(9, Location::caller());
        assert_eq!(holder.snapshot().unwrap().0, 9);
    }
    #[test]
    fn nested_poll_sections_are_disjoint() {
        let at = Instant::now();
        let mut p = PollTimings {
            started: at,
            started_cycles: 0,
            last: at,
            last_cycles: 0,
            current: PollPart::Other,
            parts: [Timing::default(); 7],
        };
        let mut mark =
            |part, ms, cycles| p.switch_at(part, at + std::time::Duration::from_millis(ms), cycles);
        mark(PollPart::SearchEvents, 1, 10);
        mark(PollPart::PreparedAdoption, 3, 30);
        mark(PollPart::TagPrewarm, 5, 50);
        mark(PollPart::PreparedAdoption, 805, 60);
        mark(PollPart::SearchEvents, 807, 80);
        mark(PollPart::Other, 809, 100);
        assert_eq!(p.parts[PollPart::TagPrewarm as usize].ms, 800.0);
        assert_eq!(p.parts[PollPart::TagPrewarm as usize].cycles, 10);
        assert_eq!(p.parts[PollPart::PreparedAdoption as usize].ms, 4.0);
        assert_eq!(p.parts.iter().map(|p| p.ms).sum::<f64>(), 809.0);
    }
}
