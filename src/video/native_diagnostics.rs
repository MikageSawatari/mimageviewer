//! Owner-local, bounded native-window/render diagnostics. No scheduling or I/O lives here.
//!
//! Scalar collection is allocation-free. Only existing owner boundaries serialize records
//! through perf's nonblocking journal. An absent recorder does not evaluate sample closures.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde_json::{Map, Value, json};

const COUNTER_CAPACITY: usize = 128;
const SNAPSHOT_CAPACITY: usize = 32;
const FIELD_CAPACITY: usize = 32;
const DETAIL_CAPACITY: usize = 8;
const INTERVAL_US: u64 = 1_000_000;
static NEXT_OUTPUT_ID: AtomicU64 = AtomicU64::new(1);

/// Correlation only; zero means diagnostics were disabled when the output was created.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DiagnosticOutputId(u64);

impl DiagnosticOutputId {
    pub(crate) fn allocate() -> Self {
        if !crate::perf::is_enabled() {
            return Self(0);
        }
        Self(NEXT_OUTPUT_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub(crate) fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Scalar {
    U64(u64),
    I64(i64),
    F64(f64),
    Bool(bool),
    Text(&'static str),
    Wide([u16; 32]),
    Rect([i32; 4]),
}

impl Scalar {
    fn json(self) -> Value {
        match self {
            Self::U64(v) => v.into(),
            Self::I64(v) => v.into(),
            Self::F64(v) => v.into(),
            Self::Bool(v) => v.into(),
            Self::Text(v) => v.into(),
            Self::Wide(v) => {
                let len = v.iter().position(|&unit| unit == 0).unwrap_or(v.len());
                String::from_utf16_lossy(&v[..len]).into()
            }
            Self::Rect(v) => json!(v),
        }
    }
}

/// Fixed scalar sample. Field names and classifications are static; never capture user text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DiagnosticSample {
    fields: [Option<(&'static str, Scalar)>; FIELD_CAPACITY],
    len: usize,
    omitted_fields: u64,
}

impl DiagnosticSample {
    pub(crate) fn new() -> Self {
        Self {
            fields: [None; FIELD_CAPACITY],
            len: 0,
            omitted_fields: 0,
        }
    }

    fn push(mut self, name: &'static str, value: Scalar) -> Self {
        if self.len < FIELD_CAPACITY {
            self.fields[self.len] = Some((name, value));
            self.len += 1;
        } else {
            self.omitted_fields += 1;
        }
        self
    }

    pub(crate) fn u64(self, name: &'static str, value: u64) -> Self {
        self.push(name, Scalar::U64(value))
    }
    pub(crate) fn i64(self, name: &'static str, value: i64) -> Self {
        self.push(name, Scalar::I64(value))
    }
    pub(crate) fn f64(self, name: &'static str, value: f64) -> Self {
        self.push(name, Scalar::F64(value))
    }
    pub(crate) fn bool(self, name: &'static str, value: bool) -> Self {
        self.push(name, Scalar::Bool(value))
    }
    pub(crate) fn text(self, name: &'static str, value: &'static str) -> Self {
        self.push(name, Scalar::Text(value))
    }
    pub(crate) fn wide(self, name: &'static str, value: [u16; 32]) -> Self {
        self.push(name, Scalar::Wide(value))
    }
    pub(crate) fn rect(self, name: &'static str, value: [i32; 4]) -> Self {
        self.push(name, Scalar::Rect(value))
    }

    fn json(self) -> Value {
        let mut map = Map::new();
        for (name, value) in self.fields[..self.len].iter().flatten() {
            map.insert((*name).into(), value.json());
        }
        if self.omitted_fields != 0 {
            map.insert("omitted_fields".into(), self.omitted_fields.into());
        }
        Value::Object(map)
    }
}

impl Default for DiagnosticSample {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) trait DiagnosticClock {
    fn enabled(&self) -> bool;
    fn now_us(&self) -> u64;
    fn at_us(&self, now: Instant) -> u64;
}

pub(crate) struct PerfClock {
    origin: Option<Instant>,
}

impl DiagnosticClock for PerfClock {
    fn enabled(&self) -> bool {
        crate::perf::is_enabled()
    }
    fn now_us(&self) -> u64 {
        self.at_us(Instant::now())
    }
    fn at_us(&self, now: Instant) -> u64 {
        self.origin.map_or(0, |start| {
            now.saturating_duration_since(start)
                .as_micros()
                .min(u128::from(u64::MAX)) as u64
        })
    }
}

pub(crate) trait DiagnosticSink {
    fn emit(&self, kind: &'static str, record: Value);
}

pub(crate) struct PerfSink;

impl DiagnosticSink for PerfSink {
    fn emit(&self, kind: &'static str, record: Value) {
        if !crate::perf::is_enabled() {
            return;
        }
        let extras: Vec<_> = record
            .as_object()
            .expect("diagnostic object")
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone()))
            .collect();
        crate::perf::event("native_diag", kind, None, 0, &extras);
    }
}

#[derive(Clone, Copy)]
struct Observation {
    observed_us: u64,
    revision: u64,
    sample: DiagnosticSample,
}

impl Observation {
    fn json(self) -> Value {
        json!({"observed_us": self.observed_us, "revision": self.revision, "sample": self.sample.json()})
    }
}

#[derive(Clone, Copy)]
struct Snapshot {
    kind: &'static str,
    latest: Observation,
    first: Option<Observation>,
    last: Option<Observation>,
    total: u64,
    omitted: u64,
}

#[derive(Clone, Copy)]
struct Detail {
    kind: &'static str,
    observation: Observation,
}

struct State {
    output_id: DiagnosticOutputId,
    window_epoch: u64,
    target_role: &'static str,
    owner: &'static str,
    interval_start_us: u64,
    revision: u64,
    // Fixed-length heap tables avoid building the (wide monitor sample-bearing)
    // snapshot inventory on the small native owner/test thread stack.
    counters: Box<[Option<(&'static str, u64)>]>,
    snapshots: Box<[Option<Snapshot>]>,
    details: Box<[Option<Detail>]>,
    detail_count: usize,
    counter_slot_overflow: u64,
    snapshot_slot_overflow: u64,
    finished: bool,
}

impl State {
    fn count(&mut self, name: &'static str, amount: u64) {
        for slot in self.counters.iter_mut() {
            match slot {
                Some((key, total)) if *key == name => {
                    *total = total.saturating_add(amount);
                    return;
                }
                None => {
                    *slot = Some((name, amount));
                    return;
                }
                _ => {}
            }
        }
        self.counter_slot_overflow = self.counter_slot_overflow.saturating_add(amount);
    }

    fn observe(
        &mut self,
        kind: &'static str,
        sample: DiagnosticSample,
        always: bool,
        clock: &impl DiagnosticClock,
    ) {
        let index = self
            .snapshots
            .iter()
            .position(|slot| slot.as_ref().is_none_or(|s| s.kind == kind));
        let Some(index) = index else {
            self.snapshot_slot_overflow = self.snapshot_slot_overflow.saturating_add(1);
            return;
        };
        if !always
            && self.snapshots[index]
                .as_ref()
                .is_some_and(|s| s.latest.sample == sample)
        {
            return;
        }
        self.revision = self.revision.saturating_add(1);
        let observation = Observation {
            observed_us: clock.now_us(),
            revision: self.revision,
            sample,
        };
        let snapshot = self.snapshots[index].get_or_insert(Snapshot {
            kind,
            latest: observation,
            first: None,
            last: None,
            total: 0,
            omitted: 0,
        });
        snapshot.latest = observation;
        snapshot.first.get_or_insert(observation);
        snapshot.last = Some(observation);
        snapshot.total += 1;
        if self.detail_count < DETAIL_CAPACITY {
            self.details[self.detail_count] = Some(Detail { kind, observation });
            self.detail_count += 1;
        } else {
            snapshot.omitted += 1;
        }
    }

    fn correlation(&self) -> Value {
        json!({"output_id":self.output_id.get(), "window_epoch":self.window_epoch,
            "target_role":self.target_role, "owner":self.owner})
    }

    fn flush(&mut self, end_us: u64, terminal: Option<&'static str>, sink: &impl DiagnosticSink) {
        for detail in self.details[..self.detail_count].iter().flatten() {
            let mut record = self.correlation();
            let map = record.as_object_mut().unwrap();
            map.insert("change_kind".into(), detail.kind.into());
            map.insert("observation".into(), detail.observation.json());
            sink.emit("change", record);
        }
        let mut record = self.correlation();
        let map = record.as_object_mut().unwrap();
        map.insert("interval_start_us".into(), self.interval_start_us.into());
        map.insert("interval_end_us".into(), end_us.into());
        map.insert(
            "elapsed_us".into(),
            end_us.saturating_sub(self.interval_start_us).into(),
        );
        map.insert("terminal".into(), terminal.map_or(Value::Null, Into::into));
        map.insert("revision".into(), self.revision.into());
        let counters: Map<_, _> = self
            .counters
            .iter()
            .flatten()
            .map(|(name, n)| ((*name).into(), (*n).into()))
            .collect();
        map.insert("counters".into(), Value::Object(counters));
        let changes: Vec<_> = self
            .snapshots
            .iter()
            .flatten()
            .map(|s| {
                json!({"kind":s.kind, "total":s.total, "omitted":s.omitted,
                "first":s.first.map(Observation::json), "last":s.last.map(Observation::json),
                "latest":s.latest.json()})
            })
            .collect();
        map.insert("changes".into(), changes.into());
        map.insert("details_emitted".into(), self.detail_count.into());
        map.insert(
            "counter_slot_overflow".into(),
            self.counter_slot_overflow.into(),
        );
        map.insert(
            "snapshot_slot_overflow".into(),
            self.snapshot_slot_overflow.into(),
        );
        sink.emit("summary", record);
        self.interval_start_us = end_us;
        for (_, n) in self.counters.iter_mut().flatten() {
            *n = 0;
        }
        for s in self.snapshots.iter_mut().flatten() {
            s.first = None;
            s.last = None;
            s.total = 0;
            s.omitted = 0;
        }
        self.details.fill(None);
        self.detail_count = 0;
        self.counter_slot_overflow = 0;
        self.snapshot_slot_overflow = 0;
        self.finished = terminal.is_some();
    }
}

/// Lives on one existing pump or render owner. `RefCell` permits read-only native
/// wrappers to report results without introducing a shared diagnostic mutex.
pub(crate) struct NativeDiagnostics<C: DiagnosticClock = PerfClock, S: DiagnosticSink = PerfSink> {
    state: Option<Box<RefCell<State>>>,
    clock: C,
    sink: S,
}

impl NativeDiagnostics {
    pub(crate) fn new(
        output_id: DiagnosticOutputId,
        window_epoch: u64,
        target_role: &'static str,
        owner: &'static str,
    ) -> Self {
        let enabled = crate::perf::is_enabled() && output_id.get() != 0;
        let clock = PerfClock {
            origin: if enabled {
                crate::perf::program_start()
            } else {
                None
            },
        };
        Self::with_backend(
            output_id,
            window_epoch,
            target_role,
            owner,
            clock,
            PerfSink,
            enabled,
        )
    }
}

impl<C: DiagnosticClock, S: DiagnosticSink> NativeDiagnostics<C, S> {
    fn with_backend(
        output_id: DiagnosticOutputId,
        window_epoch: u64,
        target_role: &'static str,
        owner: &'static str,
        clock: C,
        sink: S,
        enabled: bool,
    ) -> Self {
        let state = if enabled {
            Some(Box::new(RefCell::new(State {
                output_id,
                window_epoch,
                target_role,
                owner,
                interval_start_us: clock.now_us(),
                revision: 0,
                counters: vec![None; COUNTER_CAPACITY].into_boxed_slice(),
                snapshots: vec![None; SNAPSHOT_CAPACITY].into_boxed_slice(),
                details: vec![None; DETAIL_CAPACITY].into_boxed_slice(),
                detail_count: 0,
                counter_slot_overflow: 0,
                snapshot_slot_overflow: 0,
                finished: false,
            })))
        } else {
            None
        };
        Self { state, clock, sink }
    }

    #[inline]
    pub(crate) fn is_enabled(&self) -> bool {
        self.state.is_some() && self.clock.enabled()
    }

    /// Correlates legacy perf events without duplicating window identity state.
    pub(crate) fn correlation_key(&self) -> (u64, u64) {
        self.state.as_ref().map_or((0, 0), |state| {
            let state = state.borrow();
            (state.output_id.get(), state.window_epoch)
        })
    }

    #[inline]
    pub(crate) fn count(&self, name: &'static str, amount: u64) {
        if !self.is_enabled() {
            return;
        }
        let mut state = self.state.as_ref().unwrap().borrow_mut();
        if !state.finished {
            state.count(name, amount);
        }
    }

    pub(crate) fn snapshot(&self, kind: &'static str, sample: impl FnOnce() -> DiagnosticSample) {
        self.observe(kind, sample, false);
    }

    pub(crate) fn event(&self, kind: &'static str, sample: impl FnOnce() -> DiagnosticSample) {
        self.observe(kind, sample, true);
    }

    fn observe(&self, kind: &'static str, sample: impl FnOnce() -> DiagnosticSample, always: bool) {
        if !self.is_enabled() {
            return;
        }
        let mut state = self.state.as_ref().unwrap().borrow_mut();
        if !state.finished {
            state.observe(kind, sample(), always, &self.clock);
        }
    }

    /// Uses a timestamp already taken by the existing owner loop.
    pub(crate) fn boundary_at(&self, now: Instant) {
        if !self.is_enabled() {
            return;
        }
        self.flush_if_due(self.clock.at_us(now));
    }

    pub(crate) fn boundary(&self) {
        if !self.is_enabled() {
            return;
        }
        self.flush_if_due(self.clock.now_us());
    }

    fn flush_if_due(&self, now_us: u64) {
        let mut state = self.state.as_ref().unwrap().borrow_mut();
        if !state.finished && now_us.saturating_sub(state.interval_start_us) >= INTERVAL_US {
            state.flush(now_us, None, &self.sink);
        }
    }

    /// Flushes a final fractional interval once. Drop is a safety net, not a wake or wait.
    pub(crate) fn finish(&self, reason: &'static str) {
        if !self.is_enabled() {
            return;
        }
        let mut state = self.state.as_ref().unwrap().borrow_mut();
        if !state.finished {
            state.flush(self.clock.now_us(), Some(reason), &self.sink);
        }
    }
}

impl<C: DiagnosticClock, S: DiagnosticSink> Drop for NativeDiagnostics<C, S> {
    fn drop(&mut self) {
        self.finish("drop");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    #[derive(Clone, Default)]
    struct Clock {
        now: Rc<Cell<u64>>,
        reads: Rc<Cell<u64>>,
        enabled: Rc<Cell<bool>>,
    }
    impl DiagnosticClock for Clock {
        fn enabled(&self) -> bool {
            self.enabled.get()
        }
        fn now_us(&self) -> u64 {
            self.reads.set(self.reads.get() + 1);
            self.now.get()
        }
        fn at_us(&self, _: Instant) -> u64 {
            self.now_us()
        }
    }
    #[derive(Clone, Default)]
    struct Sink(Rc<RefCell<Vec<(&'static str, Value)>>>);
    impl DiagnosticSink for Sink {
        fn emit(&self, kind: &'static str, record: Value) {
            self.0.borrow_mut().push((kind, record));
        }
    }
    fn fixture(id: u64, clock: &Clock, sink: &Sink) -> NativeDiagnostics<Clock, Sink> {
        NativeDiagnostics::with_backend(
            DiagnosticOutputId(id),
            7,
            "hud",
            "pump",
            clock.clone(),
            sink.clone(),
            clock.enabled(),
        )
    }
    fn enabled_clock() -> Clock {
        let c = Clock::default();
        c.enabled.set(true);
        c
    }

    #[test]
    fn off_bypasses_sample_query_format_clock_counters_and_sink() {
        let clock = Clock::default();
        let sink = Sink::default();
        let d = fixture(1, &clock, &sink);
        d.snapshot("query", || panic!("query/format must not run"));
        d.event("event", || panic!("event sample must not run"));
        d.count("attempt", 1);
        d.boundary();
        d.boundary_at(Instant::now());
        d.finish("close");
        drop(d);
        assert_eq!(clock.reads.get(), 0);
        assert!(sink.0.borrow().is_empty());
    }

    #[test]
    fn boundaries_use_observation_time_coalesce_changes_and_flush_terminal_once() {
        let clock = enabled_clock();
        let sink = Sink::default();
        let d = fixture(1, &clock, &sink);
        for n in 0..12 {
            clock.now.set(n * 10);
            d.snapshot("geometry", || DiagnosticSample::new().u64("x", n));
        }
        let reads = clock.reads.get();
        d.snapshot("geometry", || DiagnosticSample::new().u64("x", 11));
        assert_eq!(
            reads,
            clock.reads.get(),
            "unchanged snapshots do not read clocks"
        );
        d.count("requests", 12);
        d.count("coalesced", 11);
        d.count("consumed", 1);
        clock.now.set(999_999);
        d.boundary();
        assert!(sink.0.borrow().is_empty());
        clock.now.set(1_000_000);
        d.boundary();
        let rows = sink.0.borrow();
        assert_eq!(rows.len(), 9);
        let summary = &rows[8].1;
        assert_eq!(summary["changes"][0]["total"], 12);
        assert_eq!(summary["changes"][0]["omitted"], 4);
        assert_eq!(summary["changes"][0]["first"]["observed_us"], 0);
        assert_eq!(summary["changes"][0]["last"]["observed_us"], 110);
        assert_eq!(summary["counters"]["requests"], 12);
        assert_eq!(summary["counters"]["coalesced"], 11);
        drop(rows);
        clock.now.set(5_500_000);
        d.boundary();
        assert_eq!(sink.0.borrow().last().unwrap().1["elapsed_us"], 4_500_000);
        clock.now.set(5_600_000);
        d.count("consumed", 1);
        d.finish("abort");
        d.finish("close");
        drop(d);
        let rows = sink.0.borrow();
        assert_eq!(rows.len(), 11);
        assert_eq!(rows[10].1["elapsed_us"], 100_000);
        assert_eq!(rows[10].1["terminal"], "abort");
        assert_eq!(rows[10].1["changes"][0]["latest"]["sample"]["x"], 11);
    }

    #[test]
    fn outputs_same_epoch_reused_hwnd_and_shared_device_do_not_mix() {
        let clock = enabled_clock();
        let sink = Sink::default();
        let a = fixture(41, &clock, &sink);
        let b = fixture(42, &clock, &sink);
        a.snapshot("target", || {
            DiagnosticSample::new()
                .u64("hwnd", 123)
                .u64("device_generation", 2)
                .u64("source_epoch", 1)
        });
        clock.now.set(20);
        b.snapshot("target", || {
            DiagnosticSample::new()
                .u64("hwnd", 123)
                .u64("device_generation", 2)
                .u64("source_epoch", 8)
        });
        clock.now.set(30);
        a.snapshot("target", || {
            DiagnosticSample::new()
                .u64("hwnd", 123)
                .u64("device_generation", 2)
                .u64("source_epoch", 2)
        });
        a.finish("retire");
        b.finish("staging_abort");
        let rows = sink.0.borrow();
        let summaries: Vec<_> = rows
            .iter()
            .filter(|(k, _)| *k == "summary")
            .map(|(_, r)| r)
            .collect();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0]["output_id"], 41);
        assert_eq!(summaries[1]["output_id"], 42);
        assert_eq!(summaries[0]["changes"][0]["total"], 2);
        assert_eq!(summaries[1]["changes"][0]["total"], 1);
        assert_eq!(summaries[1]["changes"][0]["first"]["observed_us"], 20);
        assert_eq!(summaries[0]["changes"][0]["last"]["revision"], 2);
    }

    #[test]
    fn fixed_capacities_report_overflow_and_terminal_obeys_detail_limit() {
        let clock = enabled_clock();
        let sink = Sink::default();
        let d = fixture(1, &clock, &sink);
        // Event repeats count even with equal samples, as Win32 attempts need to.
        for _ in 0..50 {
            d.event("attempt", DiagnosticSample::new);
        }
        let mut sample = DiagnosticSample::new();
        for _ in 0..FIELD_CAPACITY + 1 {
            sample = sample.u64("field", 1);
        }
        d.snapshot("field_overflow", || sample);
        // Directly exercise the bounded table-full path without generating dynamic names.
        let mut state = d.state.as_ref().unwrap().borrow_mut();
        state.counters.fill(Some(("occupied", 1)));
        state.count("new", 3);
        let occupied = state.snapshots[0];
        state.snapshots.fill(occupied);
        state.observe("new", DiagnosticSample::new(), true, &clock);
        drop(state);
        d.finish("fault");
        let rows = sink.0.borrow();
        assert_eq!(rows.len(), 9);
        let summary = &rows[8].1;
        assert_eq!(summary["counter_slot_overflow"], 3);
        assert_eq!(summary["snapshot_slot_overflow"], 1);
        assert_eq!(summary["changes"][0]["omitted"], 42);
        assert_eq!(sample.json()["omitted_fields"], 1);
    }
}
