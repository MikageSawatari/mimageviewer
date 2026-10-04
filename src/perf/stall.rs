//! Slow-only stall witnesses. Producers never acquire either logger mutex here.
//! A bounded queue is drained by the existing perf writer, without recursive event calls.
use serde_json::Value;
use std::cell::RefCell;
use std::io::Write;
use std::panic::Location;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::time::Instant;

pub(crate) const SLOW_MS: f64 = 50.0;
const QUEUE_CAPACITY: usize = 128;
static DIAGNOSTICS: OnceLock<(
    crossbeam_channel::Sender<SlowIo>,
    crossbeam_channel::Receiver<SlowIo>,
)> = OnceLock::new();
static DROPPED: AtomicU64 = AtomicU64::new(0);

pub(super) fn init() {
    DIAGNOSTICS.get_or_init(|| crossbeam_channel::bounded(QUEUE_CAPACITY));
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
        if let Some((tx, _)) = DIAGNOSTICS.get() {
            enqueue(tx, record, &DROPPED);
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

fn enqueue(tx: &crossbeam_channel::Sender<SlowIo>, record: SlowIo, dropped: &AtomicU64) {
    if tx.try_send(record).is_err() {
        dropped.fetch_add(1, Ordering::Relaxed);
    }
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
    let Some((_, rx)) = DIAGNOSTICS.get() else {
        return;
    };
    for _ in 0..rx.len().min(QUEUE_CAPACITY) {
        let Ok(record) = rx.try_recv() else {
            break;
        };
        let _ = writeln!(writer, "{}", record.json());
    }
    let dropped = DROPPED.swap(0, Ordering::Relaxed);
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
    cycles: u64,
}
impl Span {
    pub(crate) fn start() -> Option<Self> {
        super::is_enabled().then(|| Self {
            at: Instant::now(),
            cycles: crate::app::App::thread_cycles_now(),
        })
    }
    pub(crate) fn finish(self) -> Timing {
        Timing {
            ms: millis(self.at.elapsed()),
            cycles: crate::app::App::thread_cycles_now().saturating_sub(self.cycles),
        }
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
        self.switch_at(next, Instant::now(), crate::app::App::thread_cycles_now())
    }
}
thread_local! { static POLLS: RefCell<Option<PollTimings>> = const { RefCell::new(None) }; }

pub(crate) struct OtherWorkerScope {
    n: u64,
}
impl OtherWorkerScope {
    pub(crate) fn start(n: u64) -> Option<Self> {
        if !super::is_enabled() {
            return None;
        }
        let at = Instant::now();
        let cycles = crate::app::App::thread_cycles_now();
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
        Some(Self { n })
    }
}
impl Drop for OtherWorkerScope {
    fn drop(&mut self) {
        let Some(mut timings) = POLLS.with(|cell| cell.borrow_mut().take()) else {
            return;
        };
        timings.switch(PollPart::Other);
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

pub(crate) struct PollSection {
    previous: PollPart,
}
impl PollSection {
    pub(crate) fn start(part: PollPart) -> Option<Self> {
        if !super::is_enabled() {
            return None;
        }
        POLLS.with(|cell| {
            cell.borrow_mut().as_mut().map(|timings| Self {
                previous: timings.switch(part),
            })
        })
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
    #[test]
    fn queue_is_bounded_and_reports_loss_without_blocking() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let dropped = AtomicU64::new(0);
        let at = Instant::now();
        let record = || SlowIo {
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
        };
        enqueue(&tx, record(), &dropped);
        enqueue(&tx, record(), &dropped);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        let json = rx.try_recv().unwrap().json();
        assert_eq!(json["tid"], 7);
        assert_eq!(json["holder_tid"], 9);
        assert_eq!(json["wait_ms"], 800.0);
        assert_eq!(json["write_ms"], 0.5);
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
