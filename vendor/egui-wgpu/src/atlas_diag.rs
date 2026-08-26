//! mIV diagnostic: a ledger of egui font-atlas texture deltas, kept in memory and dumped on failure.
//!
//! Background: a wgpu validation panic
//!   "Copy of Y 45..126 would end up overrunning the bounds of the Destination texture of Y size 32"
//! has survived three fix attempts. egui hands each delta over exactly once, so a delta that is
//! produced but never applied leaves the renderer permanently behind what egui believes is on the
//! GPU. Reading the code has ruled out every path we can see; this records what actually happens.
//!
//! Design constraints:
//! - The user has to reproduce by hand, so one reproduction must be enough. We keep the full
//!   recent history rather than sampling.
//! - Steady-state cost must be near zero, so nothing is written to disk until something goes
//!   wrong. Events go into a fixed-size ring buffer.
//! - An out-of-bounds write is skipped rather than executed, so the diagnostic build does not die
//!   at the very moment it has something to tell us.
//! - The dump happens at the end of the batch, not at the offending delta. A batch can be
//!   `[stale partial, repairing full]`, and dumping at the partial would hide the repair.
//! - **Two threads write here.** mIV's native video presenter owns a second egui context and
//!   renderer and drives them from its own render thread. Batches are therefore identified, and
//!   the "an overflow is waiting to be reported" flag lives in thread-local storage, so one
//!   thread's flush can never consume another thread's pending report.
//!
//! Reading a dump:
//! - Every `produced` batch must be followed by an `applied` batch carrying the same deltas.
//!   A produced FULL with no applied FULL means the backend dropped it.
//! - `egui_installed` is what egui's own texture manager believes is on the GPU. If it says 128
//!   while the renderer says 32, the delta that closed the gap never arrived.
//! - `before`/`after` on an applied delta bracket the actual `update_texture` call, so an entry
//!   whose `after` did not change is an attempt that did nothing.
//! - Events interleave across threads; use `batch=` and `renderer=` to regroup them, not adjacency.
//! - A nested immediate viewport's run takes deltas out of the *shared* texture manager, so a
//!   growth caused by the parent can legitimately appear in the child's produced batch.
//!
//! Note on flush timing: a batch is flushed once its `set` list has been applied, which is before
//! the caller's `free` list runs. A free of the atlas arriving in the same output would land in
//! the ledger but not in that dump. egui never frees the font atlas, so this is a documented
//! limit rather than a gap worth restructuring three call sites for.

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// How many events to keep. A frame produces at most a handful of font-atlas deltas, so this
/// covers several seconds of history - far more than the ~5 resync generations we need to see.
const CAPACITY: usize = 768;

/// How many failures to report before going quiet. A desync that never heals would otherwise
/// write a dump every frame.
const MAX_DUMPS: u32 = 3;

/// Which `Context::run` produced a batch, or which path applied one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    /// `Context::run` for the root or a deferred viewport (eframe's main render path).
    ProducedMain,
    /// `Context::run` for an immediate viewport, nested inside the parent's pass.
    ProducedImmediate,
    /// `Context::run` for the native video presenter's own context.
    ProducedPresenter,
    /// Applied by `Painter::paint_and_update_textures`.
    AppliedPaint,
    /// Applied by `Painter::apply_textures_delta` - a frame that will not be painted.
    AppliedNoPaint,
    /// Applied by the native video presenter, on its own renderer and thread.
    AppliedPresenter,
}

impl Site {
    fn as_str(self) -> &'static str {
        match self {
            Self::ProducedMain => "produced/main",
            Self::ProducedImmediate => "produced/immediate",
            Self::ProducedPresenter => "produced/presenter",
            Self::AppliedPaint => "applied/paint",
            Self::AppliedNoPaint => "applied/no-paint",
            Self::AppliedPresenter => "applied/presenter",
        }
    }

    /// Which egui context this site belongs to. The root, deferred and immediate viewports all
    /// share eframe's context and therefore its atlas; the presenter owns a separate one.
    fn context_group(self) -> usize {
        match self {
            Self::ProducedMain
            | Self::ProducedImmediate
            | Self::AppliedPaint
            | Self::AppliedNoPaint => 0,
            Self::ProducedPresenter | Self::AppliedPresenter => 1,
        }
    }
}

/// Why a batch was thrown away without being applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropReason {
    /// The painter had no render state, so it had no renderer to hand the deltas to.
    RenderStateAbsent,
}

impl DropReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::RenderStateAbsent => "render state absent",
        }
    }
}

#[derive(Clone, Debug)]
enum Event {
    /// Header for a batch egui handed to the backend.
    ProducedBatch {
        site: Site,
        viewport: Option<u64>,
        atlas_entries: usize,
        egui_installed: Option<[usize; 2]>,
    },
    /// Header for a batch the backend applied.
    AppliedBatch {
        batch: u64,
        site: Site,
        renderer: u64,
        atlas_entries: usize,
    },
    /// A batch that reached an apply site but was discarded there.
    DroppedBatch {
        site: Site,
        reason: DropReason,
        atlas_entries: usize,
    },
    /// One atlas delta inside a produced batch.
    Produced {
        pos: Option<[usize; 2]>,
        size: [usize; 2],
    },
    /// One atlas delta the backend put through `update_texture`, bracketing the call.
    Applied {
        batch: Option<u64>,
        renderer: u64,
        pos: Option<[usize; 2]>,
        size: [usize; 2],
        before: Option<[u32; 2]>,
        after: Option<[u32; 2]>,
    },
    /// `Renderer::free_texture` actually removed the atlas.
    Freed { renderer: u64, size: [u32; 2] },
    /// A partial delta reached outside its texture; the write was skipped.
    Overflow {
        batch: Option<u64>,
        renderer: u64,
        pos: [usize; 2],
        size: [usize; 2],
        texture: [u32; 2],
    },
    /// egui's texture manager reported a different atlas size than last time, on a run that
    /// carried no atlas delta of its own. This is what an upstream failure looks like.
    InstalledSizeChanged {
        site: Site,
        from: Option<[usize; 2]>,
        to: Option<[usize; 2]>,
    },
}

impl Event {
    fn format(&self) -> String {
        fn region(pos: Option<[usize; 2]>, size: [usize; 2]) -> String {
            match pos {
                Some(pos) => format!(
                    "partial x {}..{} y {}..{}",
                    pos[0],
                    pos[0] + size[0],
                    pos[1],
                    pos[1] + size[1]
                ),
                None => format!("FULL [{}, {}]", size[0], size[1]),
            }
        }
        fn dim(size: Option<[u32; 2]>) -> String {
            match size {
                Some([w, h]) => format!("[{w}, {h}]"),
                None => "absent".to_owned(),
            }
        }
        fn cpu(size: Option<[usize; 2]>) -> String {
            match size {
                Some([w, h]) => format!("[{w}, {h}]"),
                None => "absent".to_owned(),
            }
        }
        match self {
            Self::ProducedBatch {
                site,
                viewport,
                atlas_entries,
                egui_installed,
            } => format!(
                "{} viewport={} atlas_entries={atlas_entries} egui_installed={}",
                site.as_str(),
                viewport.map_or_else(|| "-".to_owned(), |v| format!("{v:#x}")),
                cpu(*egui_installed),
            ),
            Self::AppliedBatch {
                batch,
                site,
                renderer,
                atlas_entries,
            } => format!(
                "{} batch={batch} renderer=#{renderer} atlas_entries={atlas_entries}",
                site.as_str()
            ),
            Self::DroppedBatch {
                site,
                reason,
                atlas_entries,
            } => format!(
                "*** DROPPED {} atlas_entries={atlas_entries} reason={}",
                site.as_str(),
                reason.as_str()
            ),
            Self::Produced { pos, size } => format!("  produced {}", region(*pos, *size)),
            Self::Applied {
                batch,
                renderer,
                pos,
                size,
                before,
                after,
            } => format!(
                "  applied  batch={} renderer=#{renderer} {} before={} after={}",
                batch.map_or_else(|| "none".to_owned(), |b| b.to_string()),
                region(*pos, *size),
                dim(*before),
                dim(*after)
            ),
            Self::Freed { renderer, size } => format!(
                "  FREED atlas on renderer=#{renderer} (was [{}, {}])",
                size[0], size[1]
            ),
            Self::Overflow {
                batch,
                renderer,
                pos,
                size,
                texture,
            } => format!(
                "  *** OVERFLOW batch={} renderer=#{renderer} {} does not fit [{}, {}] - write skipped",
                batch.map_or_else(|| "-".to_owned(), |b| b.to_string()),
                region(Some(*pos), *size),
                texture[0],
                texture[1]
            ),
            Self::InstalledSizeChanged { site, from, to } => format!(
                "{} egui_installed changed {} -> {} with no atlas delta in this batch",
                site.as_str(),
                cpu(*from),
                cpu(*to)
            ),
        }
    }
}

/// How many independent egui contexts write here: eframe's (shared by the root, deferred and
/// immediate viewports) and the native video presenter's.
const CONTEXT_GROUPS: usize = 2;

struct Ledger {
    events: VecDeque<(u64, Event)>,
    next_seq: u64,
    /// Dumps spent, per renderer. A global quota would let a presenter failure use up every slot
    /// before the eframe reproduction we are actually chasing ever gets to report.
    dumps: Vec<(u64, u32)>,
    /// Last size egui's texture manager reported, **per context**, so a change can be recorded
    /// even on a run that carries no atlas delta.
    ///
    /// Keeping one shared value would be actively misleading: the two contexts own separate
    /// atlases, so alternating between them would print a size change on every run and fabricate
    /// exactly the "egui grew without emitting a delta" evidence this ledger exists to find.
    /// The outer `Option` distinguishes "never observed" from "observed as absent", and the first
    /// observation of a context is a baseline rather than a change.
    last_installed: [Option<Option<[usize; 2]>>; CONTEXT_GROUPS],
}

static LEDGER: Mutex<Ledger> = Mutex::new(Ledger {
    events: VecDeque::new(),
    next_seq: 0,
    dumps: Vec::new(),
    last_installed: [None; CONTEXT_GROUPS],
});

/// Where dumps go. The host application installs this; without it the ledger still records but
/// has nowhere to report, so nothing is lost by leaving it unset in tests.
static SINK: Mutex<Option<fn(String)>> = Mutex::new(None);

/// Hands out a stable small number per `Renderer`, so a dump says which renderer it means.
static NEXT_RENDERER_ID: AtomicU64 = AtomicU64::new(0);

/// Identifies one application batch, so its entries can be regrouped after interleaving.
static NEXT_BATCH_ID: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The batch this thread is currently applying, as `(batch id, renderer id)`. A batch is
    /// begun, filled and flushed on one stack, so thread-local storage is its natural owner.
    /// Cleared when the [`BatchScope`] drops, so an application outside any batch reads as absent
    /// rather than silently claiming batch zero.
    static CURRENT_BATCH: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
    /// An overflow happened in this thread's batch and has not been reported yet, as
    /// `(batch id, renderer id)`. Keeping it thread-local is what stops the presenter thread from
    /// consuming eframe's pending report (and vice versa), which would drop the very dump we are
    /// after.
    static OVERFLOW_PENDING: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
}

/// Holds a batch open for the duration of its application. Dropping it clears the thread's
/// current-batch slot.
pub struct BatchScope {
    tracked: bool,
}

impl BatchScope {
    /// Does this batch carry font-atlas deltas? Callers use it to decide whether to [`flush`].
    pub fn tracked(&self) -> bool {
        self.tracked
    }
}

impl Drop for BatchScope {
    fn drop(&mut self) {
        CURRENT_BATCH.with(|current| current.set(None));
    }
}

/// mIV: emit a free-form diagnostic line through the same sink, for probes that live in
/// the vendored backend but have no other way back to the file logger.
pub fn log_line(line: String) {
    emit(line);
}

/// Install the log sink. mIV points this at its file logger.
pub fn set_sink(sink: fn(String)) {
    if let Ok(mut guard) = SINK.lock() {
        *guard = Some(sink);
    }
}

/// Allocate a diagnostic id for a newly created renderer.
pub fn next_renderer_id() -> u64 {
    NEXT_RENDERER_ID.fetch_add(1, Ordering::Relaxed)
}

/// The only id this ledger tracks. Following every image texture would drown the ring buffer in
/// thumbnail uploads, and only the font atlas has ever shown this failure.
fn tracked(id: epaint::TextureId) -> bool {
    id == epaint::TextureId::default()
}

fn atlas_entries(textures_delta: &epaint::textures::TexturesDelta) -> usize {
    textures_delta
        .set
        .iter()
        .filter(|(id, _)| tracked(*id))
        .count()
}

fn push(event: Event) {
    let Ok(mut ledger) = LEDGER.lock() else {
        return;
    };
    push_locked(&mut ledger, event);
}

fn push_locked(ledger: &mut Ledger, event: Event) {
    let seq = ledger.next_seq;
    ledger.next_seq += 1;
    if ledger.events.len() == CAPACITY {
        ledger.events.pop_front();
    }
    ledger.events.push_back((seq, event));
}

/// Record a batch as egui handed it to the backend, before anything can drop it.
///
/// `egui_installed` should be egui's own idea of the atlas size after this run
/// (`ctx.tex_manager().read().meta(TextureId::default())`), which is the value the backend is
/// obliged to have installed once it has applied this batch. A change in that value is recorded
/// even when the batch carries no atlas delta, because "egui grew the atlas but emitted nothing"
/// is one of the outcomes this ledger has to be able to show.
pub fn record_produced(
    site: Site,
    textures_delta: &epaint::textures::TexturesDelta,
    viewport: Option<u64>,
    egui_installed: Option<[usize; 2]>,
) {
    let entries = atlas_entries(textures_delta);
    let frees_atlas = textures_delta.free.iter().any(|id| tracked(*id));

    let Ok(mut ledger) = LEDGER.lock() else {
        return;
    };
    // Per context: the presenter owns a different atlas, and comparing the two against one
    // another would report a change on every alternation.
    let group = site.context_group();
    let seen_before = ledger.last_installed[group];
    ledger.last_installed[group] = Some(egui_installed);
    // The first observation of a context is a baseline, not a change.
    let changed = seen_before.is_some_and(|previous| previous != egui_installed);
    let previous = seen_before.flatten();

    if entries == 0 && !frees_atlas {
        if changed {
            push_locked(
                &mut ledger,
                Event::InstalledSizeChanged {
                    site,
                    from: previous,
                    to: egui_installed,
                },
            );
        }
        return;
    }

    push_locked(
        &mut ledger,
        Event::ProducedBatch {
            site,
            viewport,
            atlas_entries: entries,
            egui_installed,
        },
    );
    for (id, delta) in &textures_delta.set {
        if tracked(*id) {
            push_locked(
                &mut ledger,
                Event::Produced {
                    pos: delta.pos,
                    size: [delta.image.width(), delta.image.height()],
                },
            );
        }
    }
}

/// Open an application batch. Hold the returned scope for as long as the batch is being applied;
/// dropping it clears the thread's current-batch slot.
#[must_use]
pub fn begin_applied_batch(
    site: Site,
    renderer: u64,
    textures_delta: &epaint::textures::TexturesDelta,
) -> BatchScope {
    let batch = NEXT_BATCH_ID.fetch_add(1, Ordering::Relaxed);
    CURRENT_BATCH.with(|current| current.set(Some((batch, renderer))));
    let entries = atlas_entries(textures_delta);
    if entries == 0 {
        return BatchScope { tracked: false };
    }
    push(Event::AppliedBatch {
        batch,
        site,
        renderer,
        atlas_entries: entries,
    });
    BatchScope { tracked: true }
}

/// Record a batch that reached an apply site and was discarded there instead.
pub fn record_dropped_batch(
    site: Site,
    reason: DropReason,
    textures_delta: &epaint::textures::TexturesDelta,
) {
    let entries = atlas_entries(textures_delta);
    if entries == 0 {
        return;
    }
    push(Event::DroppedBatch {
        site,
        reason,
        atlas_entries: entries,
    });
}

/// Record one delta the backend put through `update_texture`, bracketing the call. `after` is read
/// once the call has returned, so a skipped write shows as an unchanged size rather than as a
/// successful application.
pub fn record_applied(
    id: epaint::TextureId,
    renderer: u64,
    delta: &epaint::ImageDelta,
    before: Option<[u32; 2]>,
    after: Option<[u32; 2]>,
) {
    if !tracked(id) {
        return;
    }
    let batch = CURRENT_BATCH.with(Cell::get).map(|(batch, _)| batch);
    push(Event::Applied {
        batch,
        renderer,
        pos: delta.pos,
        size: [delta.image.width(), delta.image.height()],
        before,
        after,
    });
}

/// Record that `Renderer::free_texture` actually removed the atlas. Only call this once the
/// removal has happened and returned something, so the ledger cannot claim a free that did not
/// occur - that would falsely look like proof the GPU texture was reset.
pub fn record_freed(id: epaint::TextureId, renderer: u64, size: [u32; 2]) {
    if !tracked(id) {
        return;
    }
    push(Event::Freed { renderer, size });
}

/// A partial delta was found to reach outside the texture the renderer holds - the failure we are
/// hunting. Record it and arm this thread's flush; the caller skips the write and carries on with
/// the rest of the batch, so a repairing full delta later in the same batch still gets its chance.
pub fn report_overflow(
    id: epaint::TextureId,
    renderer: u64,
    pos: [usize; 2],
    size: [usize; 2],
    texture: [u32; 2],
) {
    if !tracked(id) {
        // The ledger only holds font-atlas history, so a dump would tell us nothing about this
        // texture - and arming the flag here would let an unrelated texture spend a dump slot and
        // mislabel the result as a font-atlas report. Say it happened and stop.
        emit(format!(
            "[atlas-diag] overflow on an untracked texture {id:?} on renderer #{renderer}: \
             partial at [{}, {}] sized [{}, {}] does not fit [{}, {}] - write skipped. This \
             ledger does not track that texture, so no history follows.",
            pos[0], pos[1], size[0], size[1], texture[0], texture[1],
        ));
        return;
    }
    let current = CURRENT_BATCH.with(Cell::get);
    push(Event::Overflow {
        batch: current.map(|(batch, _)| batch),
        renderer,
        pos,
        size,
        texture,
    });
    OVERFLOW_PENDING.with(|pending| {
        pending.set(Some((
            current.map_or(u64::MAX, |(batch, _)| batch),
            renderer,
        )))
    });
}

/// Dump the ring buffer if an overflow happened in this thread's batch. Call this once the batch's
/// `set` list has been applied, so the dump includes anything that came after the offending delta.
pub fn flush(context: &str) {
    let Some((batch, renderer)) = OVERFLOW_PENDING.with(Cell::get) else {
        return;
    };
    OVERFLOW_PENDING.with(|pending| pending.set(None));

    let dump = {
        let Ok(mut ledger) = LEDGER.lock() else {
            return;
        };
        // Quota per renderer: a global one would let repeated presenter failures use up every
        // slot before the eframe reproduction we are chasing ever gets to report.
        let spent = match ledger.dumps.iter_mut().find(|(id, _)| *id == renderer) {
            Some((_, spent)) => spent,
            None => {
                ledger.dumps.push((renderer, 0));
                &mut ledger
                    .dumps
                    .last_mut()
                    .expect("just pushed an entry for this renderer")
                    .1
            }
        };
        if *spent >= MAX_DUMPS {
            return;
        }
        *spent += 1;
        let remaining = MAX_DUMPS - *spent;

        let mut out = format!(
            "[atlas-diag] font atlas delta ledger ({} events, oldest first) after {context}; \
             overflow was in batch={batch} on renderer=#{renderer}; {remaining} further dumps \
             will be reported for this renderer. Events from the main and presenter threads \
             interleave - regroup by batch= and renderer=.\n",
            ledger.events.len()
        );
        for (seq, event) in &ledger.events {
            out.push_str(&format!("[atlas-diag] #{seq:<6} {}\n", event.format()));
        }
        out.push_str(
            "[atlas-diag] how to read: a produced FULL with no matching applied FULL means the \
             backend dropped it - a DROPPED line names the path if it took one of the known ones. \
             If instead egui_installed grows with no FULL ever produced, egui never emitted the \
             growth and the problem is upstream of the renderer.",
        );
        out
    };
    emit(dump);
}

fn emit(line: String) {
    let sink = SINK.lock().ok().and_then(|guard| *guard);
    if let Some(sink) = sink {
        sink(line);
    }
}
