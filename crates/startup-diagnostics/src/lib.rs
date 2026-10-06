//! Bounded, best-effort startup evidence. Publishers never wait for disk, the
//! writer, or the watchdog. These observations do not own application state.
mod clock;
mod output;
mod watch;

pub use output::is_unc;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
pub use watch::{WatchHandle, WatchSlot, first_dispatch_watch, watch_handle};

pub const HANDOFF_ENV: &str = "MIV_STARTUP_TRACE_V1";
const CAPACITY: usize = 1024;
const TEXT: usize = 256;
const TIME_MASK: u64 = (1 << 48) - 1;
static OWNER: OnceLock<Arc<Owner>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Launcher,
    Core,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Lane {
    Launcher,
    Core,
    Indexer,
    Navigation,
    Metadata,
}
impl Lane {
    fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0 => Self::Launcher,
            1 => Self::Core,
            2 => Self::Indexer,
            3 => Self::Navigation,
            4 => Self::Metadata,
            _ => return None,
        })
    }
}
macro_rules! stages {
    ($($name:ident),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u16)]
        pub enum Stage { $($name),+ }
        impl Stage {
            fn from_id(id:u16)->Option<Self> { $(if id==Self::$name as u16 {return Some(Self::$name);})+ None }
        }
    }
}
stages! {
    Entry, Args, DataDir, RuntimePin, WorkerMode, Help, Instance, Ipc, Logger,
    EffeTunePin, EffeTunePrepare, EpubGate, EpubLock, AiModel, SusieWorker, Icon,
    Settings, SettingsFamily, SettingsOpen, SettingsPragma, SettingsIntegrity,
    SettingsSchema, SettingsJsonMigration, SettingsRecovery, SettingsKeymap,
    CollectionActor, RawExecutor, RemoteReader, RemoteService, RunNative,
    WindowCreate, WgpuInstance, SurfaceCreate, AdapterEnumerate, AdapterSelect,
    DeviceRequest, SurfaceCapabilities, PipelineCreate, SurfaceConfigure,
    Creator, Fonts, Theme, AppCreate, AppDbOpen, AppDbRead, AppDbMigrate,
    D3d11Device, D3d11VideoInterface, D3d11Fence, D3d11SharedHandle,
    FavoriteAdjustment, ViewState, NameIndexerSpawn, CreatorExit, FirstUpdate,
    TextureDelivery, Buffers, SurfaceAcquire, Encode, Submit, Present,
    NormalUiDraw, IndexerInit, IndexerResultSend, IndexerAdopt,
    IndexerMetaOpen, IndexerRebuildRead, IndexerOldIndexWipe, FtsOpen,
    FtsSchemaRecreate, FtsReader, IndexerInventoryReset, IndexerRebuildComplete,
    FtsWriter, IndexerDispatcher, IndexerMetadataSpawn, MetadataCleanup,
    MetadataReconciliation, MetadataSupervisorSpawn, MetadataOrchestrationIdle,
    MetadataWatchRegistration, InitialTargetResolve, InitialTargetScan,
    InitialTargetEnumerate, InitialTargetAdopt, EnvironmentQuery,
    RuntimePath, RuntimeParentCreate, RuntimeCanonicalize, RuntimeLease,
    RuntimeExtractionLock, RuntimeVersionDir, AssetVerify, AssetHash, AssetWrite,
    AssetRename, EffeTuneInventory, EffeTuneReuse, EffeTunePublishLock,
    EffeTuneExtract, EffeTuneHash, CoreSpawn, RuntimeHandoff
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Outcome {
    Ok,
    Error,
    Skipped,
    Cancelled,
    SuspendedNotWatched,
}
impl Outcome {
    fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0 => Self::Ok,
            1 => Self::Error,
            2 => Self::Skipped,
            3 => Self::Cancelled,
            4 => Self::SuspendedNotWatched,
            _ => return None,
        })
    }
}
#[derive(Clone, Copy)]
struct Event {
    source_pid: u32,
    source_tid: u32,
    source_role: Role,
    qpc_ticks: u64,
    kind: u8,
    lane: Lane,
    stage: Stage,
    outcome: Outcome,
    id: u64,
    parent: u64,
    at: u64,
    elapsed: u64,
    correlation: u64,
    text: [u8; TEXT],
    len: u16,
    name: [u8; 64],
    name_len: u8,
}
impl Event {
    fn new(kind: u8, lane: Lane, stage: Stage, id: u64, at: u64) -> Self {
        Self {
            source_pid: std::process::id(),
            source_tid: clock::thread_id(),
            source_role: owner().map_or(
                if lane == Lane::Launcher {
                    Role::Launcher
                } else {
                    Role::Core
                },
                |o| o.role,
            ),
            qpc_ticks: clock::ticks(),
            kind,
            lane,
            stage,
            outcome: Outcome::Ok,
            id,
            parent: 0,
            at,
            elapsed: 0,
            correlation: 0,
            text: [0; TEXT],
            len: 0,
            name: [0; 64],
            name_len: 0,
        }
    }
    fn text(mut self, text: &str) -> Self {
        let mut n = text.len().min(TEXT);
        while !text.is_char_boundary(n) {
            n -= 1;
        }
        self.text[..n].copy_from_slice(&text.as_bytes()[..n]);
        self.len = n as u16;
        self
    }
    fn message(&self) -> &str {
        std::str::from_utf8(&self.text[..self.len as usize]).unwrap_or("")
    }
    fn named(mut self, name: &str) -> Self {
        let mut n = name.len().min(64);
        while !name.is_char_boundary(n) {
            n -= 1;
        }
        self.name[..n].copy_from_slice(&name.as_bytes()[..n]);
        self.name_len = n as u8;
        self
    }
    fn name(&self) -> &str {
        std::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }
}
struct Journal {
    history: VecDeque<Event>,
    pending: VecDeque<Event>,
}
#[repr(u8)]
enum ProcessState {
    Running,
    PublishingTerminal,
    Stopped,
}
struct Owner {
    role: Role,
    run: u64,
    origin: u64,
    frequency: u64,
    version: String,
    args: Vec<String>,
    sink: Option<PathBuf>,
    journal: Mutex<Journal>,
    dropped: AtomicU64,
    next_id: AtomicU64,
    current: [AtomicU64; 5],
    unavailable: AtomicBool,
    notice_taken: AtomicBool,
    shutdown: AtomicU8,
    first_present: AtomicBool,
    normal_present: AtomicBool,
    watches: [watch::WatchState; 5],
    first_dispatch_taken: AtomicBool,
    writer_wake: OnceLock<std::thread::Thread>,
}
impl Owner {
    fn now(&self) -> u64 {
        clock::relative_us(clock::ticks(), self.origin, self.frequency)
    }
    fn publish(&self, event: Event) {
        // A single attempt. Neither queue saturation nor writer contention may
        // turn an observation into a wait on the startup thread.
        if let Ok(mut j) = self.journal.try_lock() {
            if j.history.len() == CAPACITY {
                j.history.pop_front();
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            j.history.push_back(event);
            if j.pending.len() < CAPACITY {
                j.pending.push_back(event);
            } else {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(writer) = self.writer_wake.get() {
            writer.unpark();
        }
    }
    fn fail(&self) {
        self.unavailable.store(true, Ordering::Release);
    }
    fn stopped(&self) -> bool {
        self.shutdown.load(Ordering::Acquire) == ProcessState::Stopped as u8
    }
    fn take_notice(&self) -> bool {
        self.unavailable.load(Ordering::Acquire) && !self.notice_taken.swap(true, Ordering::AcqRel)
    }
}
fn owner() -> Option<&'static Arc<Owner>> {
    OWNER.get()
}

/// Capture entry before even collecting command-line arguments.
pub fn init_process(role: Role, version: &str, portable: bool) {
    let entry = clock::ticks();
    let args = std::env::args_os()
        .map(|s| s.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    initialize(role, version, &args, portable, entry);
}
pub fn init(role: Role, version: &str, args: &[String], portable: bool) {
    initialize(role, version, args, portable, clock::ticks());
}
fn initialize(role: Role, version: &str, args: &[String], portable: bool, entry: u64) {
    if OWNER.get().is_some() {
        return;
    }
    let frequency = clock::frequency();
    let inherited = if role == Role::Core {
        std::env::var(HANDOFF_ENV)
            .ok()
            .and_then(|s| decode_snapshot(&s, entry, frequency))
    } else {
        None
    };
    let (run, origin, events, lost) = inherited.unwrap_or_else(|| {
        (
            entry ^ ((std::process::id() as u64) << 32),
            entry,
            Vec::new(),
            0,
        )
    });
    let o = Arc::new(Owner {
        role,
        run,
        origin,
        frequency,
        version: version.chars().take(128).collect(),
        args: bounded_args(args),
        sink: output::select_sink(args, portable),
        journal: Mutex::new(Journal {
            history: VecDeque::with_capacity(CAPACITY),
            pending: VecDeque::with_capacity(CAPACITY),
        }),
        dropped: AtomicU64::new(lost),
        next_id: AtomicU64::new(((std::process::id() as u64) << 32) | 1),
        current: std::array::from_fn(|_| AtomicU64::new(0)),
        unavailable: AtomicBool::new(false),
        notice_taken: AtomicBool::new(false),
        shutdown: AtomicU8::new(ProcessState::Running as u8),
        first_present: AtomicBool::new(false),
        normal_present: AtomicBool::new(false),
        watches: std::array::from_fn(|_| watch::WatchState::new()),
        first_dispatch_taken: AtomicBool::new(false),
        writer_wake: OnceLock::new(),
    });
    if OWNER.set(o.clone()).is_err() {
        return;
    }
    for mut e in events {
        e.kind |= 0x80;
        o.publish(e);
    }
    let lane = if role == Role::Launcher {
        Lane::Launcher
    } else {
        Lane::Core
    };
    let mut entry_event = Event::new(
        2,
        lane,
        Stage::Entry,
        0,
        clock::relative_us(entry, origin, frequency),
    )
    .named("process.entry");
    entry_event.qpc_ticks = entry;
    o.publish(entry_event);
    if role != Role::Core || !non_gui_args(args) {
        watch::initialize(&o, clock::relative_us(entry, origin, frequency));
    }
    if output::spawn(o.clone()).is_err() {
        o.fail();
    }
    if watch::spawn(o.clone()).is_err() {
        o.fail();
    }
}
fn bounded_args(args: &[String]) -> Vec<String> {
    let mut left = 4096;
    args.iter()
        .take(64)
        .map(|s| {
            let n = s.len().min(left);
            let mut n = n;
            while !s.is_char_boundary(n) {
                n -= 1;
            }
            left -= n;
            s[..n].to_owned()
        })
        .collect()
}
fn non_gui_args(args: &[String]) -> bool {
    args.iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| {
            matches!(
                arg.as_str(),
                "--help"
                    | "-h"
                    | "--version"
                    | "-V"
                    | "--pdf-worker"
                    | "--tensorrt-build"
                    | "--tensorrt-infer-worker"
                    | "--trt-smoke-test"
            )
        })
}
#[derive(Clone, Copy, Default)]
struct StackEntry {
    owner: u64,
    packed: u64,
    id: u64,
}
thread_local! {static STACK:RefCell<[[StackEntry;32];5]>=const {RefCell::new([[StackEntry{owner:0,packed:0,id:0};32];5])};}
thread_local! {static OBSERVERS:RefCell<[Option<Span>;32]>=const {RefCell::new([const {None};32])};}

/// Paired callback bridge for renderer crates that intentionally do not depend
/// on this crate. Slots and nesting are publisher-thread local and bounded.
pub fn observer_begin(lane: Lane, stage: Stage) {
    let watch = match lane {
        Lane::Core => {
            watch_handle(WatchSlot::CoreStartup).or_else(|| watch_handle(WatchSlot::NormalPresent))
        }
        Lane::Indexer => watch_handle(WatchSlot::IndexerInit),
        Lane::Launcher => watch_handle(WatchSlot::Launcher),
        _ => None,
    };
    let s = span(lane, stage).watched_optional(watch);
    OBSERVERS.with(|slots| {
        let mut slots = slots.borrow_mut();
        if let Some(slot) = slots.iter_mut().find(|s| s.is_none()) {
            *slot = Some(s);
        } else {
            if let Some(o) = owner() {
                o.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    });
}
pub fn observer_end(lane: Lane, stage: Stage, outcome: Outcome) {
    let s = OBSERVERS.with(|slots| {
        let mut slots = slots.borrow_mut();
        let index = slots.iter().rposition(|s| {
            s.as_ref()
                .is_some_and(|s| s.lane == lane && s.stage == stage)
        });
        index.and_then(|i| slots[i].take())
    });
    if let Some(s) = s {
        s.finish(outcome);
    }
}

/// No terminal event is invented by Drop: a panic/abandoned operation remains
/// an unmatched begin. Call `finish` only after the real boundary returned.
pub struct Span {
    o: Option<Arc<Owner>>,
    lane: Lane,
    stage: Stage,
    id: u64,
    start: u64,
    packed: u64,
    parent: StackEntry,
    depth: usize,
    detached: bool,
    watch: Option<WatchHandle>,
    watch_parent: u64,
    watch_packed: u64,
    thread_owner: std::marker::PhantomData<std::rc::Rc<()>>,
}
pub fn span(lane: Lane, stage: Stage) -> Span {
    let watch = match lane {
        Lane::Launcher => watch_handle(WatchSlot::Launcher),
        Lane::Core => watch_handle(WatchSlot::CoreStartup),
        Lane::Indexer => watch_handle(WatchSlot::IndexerInit),
        _ => None,
    };
    new_span(lane, stage, 0, false).watched_optional(watch)
}
pub fn event_span(lane: Lane, stage: Stage, correlation: u64) -> Span {
    new_span(lane, stage, correlation, true)
}
fn new_span(lane: Lane, stage: Stage, correlation: u64, detached: bool) -> Span {
    new_span_for(owner().cloned(), lane, stage, correlation, detached)
}
fn new_span_for(
    o: Option<Arc<Owner>>,
    lane: Lane,
    stage: Stage,
    correlation: u64,
    detached: bool,
) -> Span {
    let mut s = Span {
        o,
        lane,
        stage,
        id: 0,
        start: 0,
        packed: 0,
        parent: StackEntry::default(),
        depth: 32,
        detached,
        watch: None,
        watch_parent: 0,
        watch_packed: 0,
        thread_owner: std::marker::PhantomData,
    };
    if let Some(o) = &s.o {
        s.start = o.now();
        s.id = o.next_id.fetch_add(1, Ordering::Relaxed);
        s.packed = pack(stage, s.start);
        if !detached {
            STACK.with(|stack| {
                let mut stack = stack.borrow_mut();
                let row = &mut stack[lane as usize];
                if let Some(depth) = row.iter().position(|e| e.id == 0) {
                    s.depth = depth;
                    if depth > 0 {
                        s.parent = row[depth - 1];
                    }
                    row[depth] = StackEntry {
                        owner: o.run,
                        packed: s.packed,
                        id: s.id,
                    };
                }
            });
            o.current[lane as usize].store(s.packed, Ordering::Release);
        }
        let mut e = Event::new(0, lane, stage, s.id, s.start);
        e.parent = s.parent.id;
        e.correlation = correlation;
        o.publish(e);
    }
    s
}
impl Span {
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn detail(self, text: &str) -> Self {
        if let Some(o) = &self.o {
            o.publish(Event::new(3, self.lane, self.stage, self.id, o.now()).text(text));
        }
        self
    }
    pub fn watched(mut self, handle: WatchHandle) -> Self {
        if self.watch == Some(handle) {
            return self;
        }
        if watch::allowed(self.lane, self.stage, handle.slot) {
            if let Some((child, parent)) = handle.child(self.packed) {
                if let Some(previous) = self.watch {
                    previous.restore_child(self.watch_packed, self.watch_parent);
                }
                self.watch_packed = child;
                self.watch_parent = parent;
                self.watch = Some(handle);
            }
        }
        self
    }
    pub fn watched_optional(self, handle: Option<WatchHandle>) -> Self {
        match handle {
            Some(h) => self.watched(h),
            None => self,
        }
    }
    pub fn finish(self, outcome: Outcome) {
        if let Some(o) = &self.o {
            let now = o.now();
            let mut e = Event::new(1, self.lane, self.stage, self.id, now);
            e.outcome = outcome;
            e.elapsed = now.saturating_sub(self.start);
            e.parent = self.parent.id;
            o.publish(e);
        }
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        let Some(o) = &self.o else {
            return;
        };
        if !self.detached {
            let parent = if self.parent.owner == o.run {
                self.parent.packed
            } else {
                0
            };
            let _ = o.current[self.lane as usize].compare_exchange(
                self.packed,
                parent,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
            if self.depth < 32 {
                STACK.with(|stack| {
                    let mut stack = stack.borrow_mut();
                    let e = &mut stack[self.lane as usize][self.depth];
                    if e.id == self.id {
                        *e = StackEntry::default();
                    }
                });
            }
        }
        if let Some(handle) = self.watch {
            handle.restore_child(self.watch_packed, self.watch_parent);
        }
    }
}
fn pack(stage: Stage, at: u64) -> u64 {
    ((stage as u64 + 1) << 48) | (at & TIME_MASK)
}
fn unpack(value: u64) -> Option<(Stage, u64)> {
    if value == 0 {
        None
    } else {
        Stage::from_id(((value >> 48) - 1) as u16).map(|s| (s, value & TIME_MASK))
    }
}
#[derive(Clone, Debug)]
pub struct LaneSnapshot {
    pub stage: Option<Stage>,
    pub elapsed: Duration,
    pub total: Duration,
}
pub fn lane_snapshot(lane: Lane) -> LaneSnapshot {
    let Some(o) = owner() else {
        return LaneSnapshot {
            stage: None,
            elapsed: Duration::ZERO,
            total: Duration::ZERO,
        };
    };
    lane_snapshot_for(o, lane, o.now())
}
fn lane_snapshot_for(o: &Owner, lane: Lane, now: u64) -> LaneSnapshot {
    let current = unpack(o.current[lane as usize].load(Ordering::Acquire));
    LaneSnapshot {
        stage: current.map(|s| s.0),
        elapsed: Duration::from_micros(current.map_or(0, |s| now.saturating_sub(s.1))),
        total: Duration::from_micros(now),
    }
}
pub fn milestone(name: &'static str) {
    record_detail(name, "");
}
pub fn record_detail(name: &'static str, value: &str) {
    if let Some(o) = owner() {
        let mut e = Event::new(
            2,
            if o.role == Role::Launcher {
                Lane::Launcher
            } else {
                Lane::Core
            },
            Stage::Entry,
            0,
            o.now(),
        )
        .text(value)
        .named(name);
        e.id = hash_name(name);
        o.publish(e);
    }
}
fn hash_name(name: &str) -> u64 {
    name.bytes().fold(14695981039346656037, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(1099511628211)
    })
}
pub fn take_unavailable_notice() -> bool {
    owner().is_some_and(|o| o.take_notice())
}
pub fn normal_presented() -> bool {
    owner().is_some_and(|o| o.normal_present.load(Ordering::Acquire))
}
pub fn mark_present(normal: bool) {
    if let Some(o) = owner() {
        mark_present_for(o, normal);
    }
}
fn mark_present_for(o: &Owner, normal: bool) {
    if !o.first_present.swap(true, Ordering::AcqRel) {
        o.publish(
            Event::new(2, Lane::Core, Stage::Present, 0, o.now()).named("first_present.returned"),
        );
        watch::retire(o, WatchSlot::CoreStartup, Outcome::Ok);
    }
    if normal && !o.normal_present.swap(true, Ordering::AcqRel) {
        o.publish(Event::new(2, Lane::Core, Stage::Present, 0, o.now()).named("normal_ui.ready"));
        watch::retire(o, WatchSlot::NormalPresent, Outcome::Ok);
    }
}
pub fn terminal(outcome: Outcome) {
    if let Some(o) = owner() {
        terminal_for(o, outcome);
    }
}
fn terminal_for(o: &Owner, outcome: Outcome) {
    // A single terminal publisher owns this transition. Stop the writer only
    // after the winner has queued the actual outcome and final watch events.
    if o.shutdown
        .compare_exchange(
            ProcessState::Running as u8,
            ProcessState::PublishingTerminal as u8,
            Ordering::AcqRel,
            Ordering::Relaxed,
        )
        .is_err()
    {
        return;
    }
    for slot in WatchSlot::ALL {
        watch::retire(o, slot, outcome);
    }
    let mut e = Event::new(
        2,
        if o.role == Role::Launcher {
            Lane::Launcher
        } else {
            Lane::Core
        },
        Stage::Entry,
        0,
        o.now(),
    )
    .named("process.terminal");
    e.outcome = outcome;
    o.publish(e);
    o.shutdown
        .store(ProcessState::Stopped as u8, Ordering::Release);
    if let Some(writer) = o.writer_wake.get() {
        writer.unpark();
    }
}
pub fn shutdown() {
    terminal(Outcome::Cancelled);
}
pub fn record_data_dir(path: &Path, selection: &str, runtime: Option<&Path>) {
    output::environment(
        path.to_owned(),
        selection.to_owned(),
        runtime.map(Path::to_owned),
    );
}

/// One nonblocking journal acquisition; no writer drain or path is inherited.
pub fn inherited_snapshot() -> Option<String> {
    owner().map(|o| encode_snapshot(o))
}
fn encode_snapshot(o: &Owner) -> String {
    let mut out = format!(
        "1:{:x}:{:x}:{:x}:{:x}:",
        o.run,
        o.origin,
        o.frequency,
        o.dropped.load(Ordering::Relaxed)
    );
    if let Ok(j) = o.journal.try_lock() {
        // 72 bytes/event encoded as hex; 170 entries plus the header stay
        // below 24 KiB even with maximum-width validated numeric values.
        let take = j.history.len().min(170);
        let lost = j.history.len() - take;
        if lost > 0 {
            out = format!(
                "1:{:x}:{:x}:{:x}:{:x}:",
                o.run,
                o.origin,
                o.frequency,
                o.dropped.load(Ordering::Relaxed) + lost as u64
            );
        }
        for e in j.history.iter().skip(j.history.len() - take) {
            let mut bytes = [0u8; 72];
            bytes[0] = e.kind;
            bytes[1] = e.lane as u8;
            bytes[2..4].copy_from_slice(&(e.stage as u16).to_le_bytes());
            bytes[4] = e.outcome as u8;
            bytes[8..16].copy_from_slice(&e.id.to_le_bytes());
            bytes[16..24].copy_from_slice(&e.parent.to_le_bytes());
            bytes[24..32].copy_from_slice(&e.at.to_le_bytes());
            bytes[32..40].copy_from_slice(&e.elapsed.to_le_bytes());
            bytes[40..48].copy_from_slice(&e.correlation.to_le_bytes());
            bytes[48..52].copy_from_slice(&e.source_pid.to_le_bytes());
            bytes[52..56].copy_from_slice(&e.source_tid.to_le_bytes());
            bytes[56] = if e.source_role == Role::Launcher {
                0
            } else {
                1
            };
            bytes[64..72].copy_from_slice(&e.qpc_ticks.to_le_bytes());
            use std::fmt::Write;
            for b in bytes {
                let _ = write!(out, "{b:02x}");
            }
        }
    } else {
        out = format!(
            "1:{:x}:{:x}:{:x}:{:x}:",
            o.run,
            o.origin,
            o.frequency,
            o.dropped.load(Ordering::Relaxed) + 1
        );
    }
    out
}
fn decode_snapshot(s: &str, now: u64, frequency: u64) -> Option<(u64, u64, Vec<Event>, u64)> {
    if s.len() > 24 * 1024 || !s.is_ascii() {
        return None;
    }
    let mut parts = s.split(':');
    if parts.next()? != "1" {
        return None;
    }
    let run = u64::from_str_radix(parts.next()?, 16).ok()?;
    let origin = u64::from_str_radix(parts.next()?, 16).ok()?;
    let freq = u64::from_str_radix(parts.next()?, 16).ok()?;
    let dropped = u64::from_str_radix(parts.next()?, 16).ok()?;
    let body = parts.next()?;
    if parts.next().is_some()
        || freq == 0
        || freq != frequency
        || origin > now
        || clock::relative_us(now, origin, freq) > 24 * 60 * 60 * 1_000_000
        || body.len() % 144 != 0
        || body.len() / 144 > 256
    {
        return None;
    }
    let mut events = Vec::with_capacity(body.len() / 144);
    for chunk in body.as_bytes().chunks_exact(144) {
        let mut b = [0u8; 72];
        for (i, pair) in chunk.chunks_exact(2).enumerate() {
            b[i] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        }
        if b[0] > 4 || b[5..8] != [0, 0, 0] || b[57..64] != [0; 7] {
            return None;
        }
        let read = |start: usize| u64::from_le_bytes(b[start..start + 8].try_into().unwrap());
        let at = read(24);
        if at > clock::relative_us(now, origin, freq) || read(32) > at {
            return None;
        }
        let mut e = Event::new(
            b[0],
            Lane::from_id(b[1])?,
            Stage::from_id(u16::from_le_bytes([b[2], b[3]]))?,
            read(8),
            at,
        );
        e.outcome = Outcome::from_id(b[4])?;
        e.parent = read(16);
        e.elapsed = read(32);
        e.correlation = read(40);
        e.source_pid = u32::from_le_bytes(b[48..52].try_into().unwrap());
        e.source_tid = u32::from_le_bytes(b[52..56].try_into().unwrap());
        e.source_role = match b[56] {
            0 => Role::Launcher,
            1 => Role::Core,
            _ => return None,
        };
        e.qpc_ticks = read(64);
        if e.source_pid == 0 || e.source_tid == 0 || e.qpc_ticks < origin || e.qpc_ticks > now {
            return None;
        }
        events.push(e);
    }
    Some((run, origin, events, dropped))
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(crate) fn test_owner(role: Role) -> Owner {
        Owner {
            role,
            run: 1,
            origin: clock::ticks(),
            frequency: clock::frequency(),
            version: "test".into(),
            args: Vec::new(),
            sink: None,
            journal: Mutex::new(Journal {
                history: VecDeque::with_capacity(CAPACITY),
                pending: VecDeque::with_capacity(CAPACITY),
            }),
            dropped: AtomicU64::new(0),
            next_id: AtomicU64::new(1),
            current: std::array::from_fn(|_| AtomicU64::new(0)),
            unavailable: AtomicBool::new(false),
            notice_taken: AtomicBool::new(false),
            shutdown: AtomicU8::new(ProcessState::Running as u8),
            first_present: AtomicBool::new(false),
            normal_present: AtomicBool::new(false),
            watches: std::array::from_fn(|_| watch::WatchState::new()),
            first_dispatch_taken: AtomicBool::new(false),
            writer_wake: OnceLock::new(),
        }
    }
    #[test]
    fn nested_parent_is_restored_and_duration_is_not_double_counted() {
        let o = Arc::new(test_owner(Role::Core));
        let parent = new_span_for(Some(o.clone()), Lane::Core, Stage::Settings, 0, false);
        let parent_packed = parent.packed;
        let child = new_span_for(Some(o.clone()), Lane::Core, Stage::SettingsOpen, 0, false);
        assert_eq!(
            unpack(o.current[1].load(Ordering::Acquire)).unwrap().0,
            Stage::SettingsOpen
        );
        child.finish(Outcome::Error);
        assert_eq!(o.current[1].load(Ordering::Acquire), parent_packed);
        parent.finish(Outcome::Ok);
        assert_eq!(o.current[1].load(Ordering::Acquire), 0);
        let j = o.journal.lock().unwrap();
        assert_eq!(j.history.len(), 4);
        assert_eq!(j.history[2].outcome, Outcome::Error);
        assert_eq!(j.history[2].parent, j.history[0].id);
    }
    #[test]
    fn abandoned_span_has_no_false_success_and_parallel_registration_has_no_lane_effect() {
        let o = Arc::new(test_owner(Role::Core));
        let s = new_span_for(
            Some(o.clone()),
            Lane::Metadata,
            Stage::MetadataWatchRegistration,
            42,
            true,
        );
        drop(s);
        assert_eq!(o.current[4].load(Ordering::Acquire), 0);
        let j = o.journal.lock().unwrap();
        assert_eq!(j.history.len(), 1);
        assert_eq!(j.history[0].correlation, 42);
    }
    #[test]
    fn journal_overflow_and_contention_are_lossy_not_blocking() {
        let o = test_owner(Role::Core);
        for id in 0..2048 {
            o.publish(Event::new(0, Lane::Core, Stage::Settings, id, 0));
        }
        assert_eq!(o.journal.lock().unwrap().history.len(), CAPACITY);
        assert!(o.dropped.load(Ordering::Relaxed) > 0);
        let guard = o.journal.lock().unwrap();
        let old = o.dropped.load(Ordering::Relaxed);
        o.publish(Event::new(0, Lane::Core, Stage::Settings, 5, 0));
        assert_eq!(o.dropped.load(Ordering::Relaxed), old + 1);
        drop(guard);
    }
    #[test]
    fn bounded_handoff_validates_schema_clock_counts_and_numeric_events() {
        let o = test_owner(Role::Launcher);
        for id in 0..1200 {
            o.publish(Event::new(0, Lane::Launcher, Stage::AssetVerify, id, 0));
        }
        let encoded = encode_snapshot(&o);
        assert!(encoded.len() < 24 * 1024);
        let decoded = decode_snapshot(&encoded, clock::ticks(), o.frequency).unwrap();
        assert_eq!(decoded.0, o.run);
        assert_eq!(decoded.1, o.origin);
        assert_eq!(decoded.2.len(), 170);
        assert!(decoded.3 > 0);
        assert!(decode_snapshot(&encoded, clock::ticks(), o.frequency + 1).is_none());
        assert!(
            decode_snapshot(
                &encoded.replacen("1:", "2:", 1),
                clock::ticks(),
                o.frequency
            )
            .is_none()
        );
        assert!(decode_snapshot(&"x".repeat(24 * 1024 + 1), clock::ticks(), o.frequency).is_none());
        assert!(decode_snapshot("1:1:0:0:0:", clock::ticks(), o.frequency).is_none());
        assert!(decode_snapshot(&(encoded.clone() + "00"), clock::ticks(), o.frequency).is_none());
        let _guard = o.journal.lock().unwrap();
        let partial = encode_snapshot(&o);
        assert!(
            decode_snapshot(&partial, clock::ticks(), o.frequency)
                .unwrap()
                .2
                .is_empty()
        );
    }
    #[test]
    fn lanes_remain_independent_and_same_stage_reentry_has_new_identity() {
        let o = Arc::new(test_owner(Role::Core));
        let a = new_span_for(Some(o.clone()), Lane::Core, Stage::Settings, 0, false);
        let core = o.current[1].load(Ordering::Acquire);
        let b = new_span_for(Some(o.clone()), Lane::Indexer, Stage::IndexerInit, 0, false);
        b.finish(Outcome::Skipped);
        assert_eq!(o.current[1].load(Ordering::Acquire), core);
        let id = a.id;
        a.finish(Outcome::Ok);
        let a = new_span_for(Some(o.clone()), Lane::Core, Stage::Settings, 0, false);
        assert_ne!(a.id, id);
        a.finish(Outcome::Cancelled);
    }
    #[test]
    fn non_gui_modes_do_not_watch_gui_readiness_but_positional_flags_are_paths() {
        for flag in [
            "--help",
            "-V",
            "--pdf-worker",
            "--tensorrt-build",
            "--tensorrt-infer-worker",
        ] {
            assert!(non_gui_args(&["app".into(), flag.into()]));
            assert!(!non_gui_args(&["app".into(), "--".into(), flag.into()]));
        }
    }
    #[test]
    fn first_present_retires_core_once_without_starting_normal_or_retiring_indexer() {
        let o = test_owner(Role::Core);
        watch::initialize(&o, 0);
        mark_present_for(&o, false);
        mark_present_for(&o, false);
        assert!(!watch::is_live(&o, WatchSlot::CoreStartup));
        assert!(watch::is_live(&o, WatchSlot::NormalPresent));
        assert!(watch::is_live(&o, WatchSlot::IndexerInit));
        assert!(watch::is_live(&o, WatchSlot::InitialDispatch));
        assert!(!o.normal_present.load(Ordering::Acquire));
        mark_present_for(&o, true);
        mark_present_for(&o, true);
        assert!(!watch::is_live(&o, WatchSlot::NormalPresent));
        assert!(watch::is_live(&o, WatchSlot::IndexerInit));
        let j = o.journal.lock().unwrap();
        assert_eq!(
            j.history
                .iter()
                .filter(|e| e.name() == "first_present.returned")
                .count(),
            1
        );
        assert_eq!(
            j.history
                .iter()
                .filter(|e| e.name() == "normal_ui.ready")
                .count(),
            1
        );
    }
    #[test]
    fn snapshot_preserves_original_publisher_identity_and_raw_qpc() {
        let o = test_owner(Role::Launcher);
        let mut e = Event::new(0, Lane::Launcher, Stage::AssetVerify, 77, 0);
        assert_eq!(e.source_pid, std::process::id());
        assert_eq!(e.source_tid, clock::thread_id());
        assert_eq!(e.source_role, Role::Launcher);
        e.source_pid = 1234;
        e.source_tid = 5678;
        e.correlation = u64::MAX;
        let ticks = e.qpc_ticks;
        o.publish(e);
        let encoded = encode_snapshot(&o);
        let mut decoded = decode_snapshot(&encoded, clock::ticks(), o.frequency)
            .unwrap()
            .2;
        let mut inherited = decoded.pop().unwrap();
        assert_eq!(inherited.source_pid, 1234);
        assert_eq!(inherited.source_tid, 5678);
        assert_eq!(inherited.source_role, Role::Launcher);
        assert_eq!(inherited.qpc_ticks, ticks);
        assert_eq!(inherited.correlation, u64::MAX);
        inherited.kind |= 0x80;
        let json = output::event_json(&inherited);
        assert_eq!(json["pid"], 1234);
        assert_eq!(json["tid"], 5678);
        assert_eq!(json["role"], "Launcher");
        assert_eq!(json["qpc_ticks"], ticks);
        assert_eq!(json["inherited"], true);
    }
    #[test]
    fn lane_total_uses_shared_run_origin_and_diagnostic_notice_is_once_only() {
        let o = test_owner(Role::Core);
        o.current[Lane::Indexer as usize]
            .store(pack(Stage::IndexerInit, 35_000_000), Ordering::Release);
        let snapshot = lane_snapshot_for(&o, Lane::Indexer, 40_000_000);
        assert_eq!(snapshot.elapsed, Duration::from_secs(5));
        assert_eq!(snapshot.total, Duration::from_secs(40));
        assert!(!o.take_notice());
        o.fail();
        assert!(o.take_notice());
        assert!(!o.take_notice());
        o.fail();
        assert!(!o.take_notice());
    }
    #[test]
    fn process_terminal_keeps_first_actual_outcome_and_retires_watches_once() {
        let o = test_owner(Role::Core);
        watch::initialize(&o, 0);
        terminal_for(&o, Outcome::Error);
        terminal_for(&o, Outcome::Ok);
        terminal_for(&o, Outcome::Cancelled);
        assert!(o.stopped());
        assert!(
            WatchSlot::ALL
                .into_iter()
                .all(|slot| !watch::is_live(&o, slot))
        );
        let j = o.journal.lock().unwrap();
        let terminals = j
            .history
            .iter()
            .filter(|e| e.name() == "process.terminal")
            .collect::<Vec<_>>();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].outcome, Outcome::Error);
    }
}
