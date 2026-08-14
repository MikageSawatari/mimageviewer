//! mIV diagnostic: a ledger of egui texture deltas, kept in memory and dumped on failure.
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
//!   wrong. Events go into a fixed-size ring buffer; the ring is dumped only when a delta is
//!   found to be out of bounds.
//! - The out-of-bounds write is skipped instead of executed, so the diagnostic build does not
//!   die at the very moment it has something to tell us.

use std::collections::VecDeque;
use std::sync::Mutex;

/// How many events to keep. A frame produces at most a handful of font-atlas deltas, so this
/// covers several seconds of history - far more than the ~5 resync generations we need to see.
const CAPACITY: usize = 512;

/// Where a delta was observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    /// `Context::run` returned it (eframe's main render path).
    ProducedMain,
    /// `Context::run` returned it for an immediate viewport.
    ProducedImmediate,
    /// Applied by `Painter::paint_and_update_textures`.
    AppliedPaint,
    /// Applied by `Painter::apply_textures_delta` (a frame that will not be painted).
    AppliedNoPaint,
    /// Freed by the renderer.
    Freed,
}

impl Site {
    fn as_str(self) -> &'static str {
        match self {
            Self::ProducedMain => "produced/main",
            Self::ProducedImmediate => "produced/immediate",
            Self::AppliedPaint => "applied/paint",
            Self::AppliedNoPaint => "applied/no-paint",
            Self::Freed => "freed",
        }
    }
}

#[derive(Clone, Debug)]
struct Event {
    seq: u64,
    site: Site,
    id: epaint::TextureId,
    /// `None` for a full (reallocating) delta.
    pos: Option<[usize; 2]>,
    /// Size of the image carried by the delta.
    size: [usize; 2],
    /// What the renderer held for this id before applying, when known.
    renderer_size_before: Option<[u32; 2]>,
    viewport: Option<u64>,
}

impl Event {
    fn format(&self) -> String {
        let kind = match self.pos {
            Some(pos) => format!(
                "partial pos=[{}, {}] region=[{}..{}, {}..{}]",
                pos[0],
                pos[1],
                pos[0],
                pos[0] + self.size[0],
                pos[1],
                pos[1] + self.size[1],
            ),
            None => format!("FULL size=[{}, {}]", self.size[0], self.size[1]),
        };
        let before = match self.renderer_size_before {
            Some([w, h]) => format!(" renderer_before=[{w}, {h}]"),
            None => String::new(),
        };
        let viewport = match self.viewport {
            Some(v) => format!(" viewport={v:#x}"),
            None => String::new(),
        };
        format!(
            "#{:<5} {:<18} id={:?} {kind}{before}{viewport}",
            self.seq,
            self.site.as_str(),
            self.id,
        )
    }
}

struct Ledger {
    events: VecDeque<Event>,
    next_seq: u64,
    /// Set once we have reported an overflow, so a persistent desync does not spam the log.
    reported: bool,
}

static LEDGER: Mutex<Ledger> = Mutex::new(Ledger {
    events: VecDeque::new(),
    next_seq: 0,
    reported: false,
});

/// Where dumps go. The host application installs this; without it the ledger still records but
/// has nowhere to report, so nothing is lost by leaving it unset in tests.
static SINK: Mutex<Option<fn(String)>> = Mutex::new(None);

/// Install the log sink. mIV points this at its file logger.
pub fn set_sink(sink: fn(String)) {
    if let Ok(mut guard) = SINK.lock() {
        *guard = Some(sink);
    }
}

fn emit(line: String) {
    let sink = SINK.lock().ok().and_then(|guard| *guard);
    if let Some(sink) = sink {
        sink(line);
    }
}

/// Is this an id worth tracking? Only the font atlas has shown this failure, and tracking every
/// image texture would drown the ring buffer in thumbnail uploads.
fn tracked(id: epaint::TextureId) -> bool {
    id == epaint::TextureId::default()
}

fn record(
    site: Site,
    id: epaint::TextureId,
    pos: Option<[usize; 2]>,
    size: [usize; 2],
    renderer_size_before: Option<[u32; 2]>,
    viewport: Option<u64>,
) {
    if !tracked(id) {
        return;
    }
    let Ok(mut ledger) = LEDGER.lock() else {
        return;
    };
    let seq = ledger.next_seq;
    ledger.next_seq += 1;
    if ledger.events.len() == CAPACITY {
        ledger.events.pop_front();
    }
    ledger.events.push_back(Event {
        seq,
        site,
        id,
        pos,
        size,
        renderer_size_before,
        viewport,
    });
}

/// Record a delta as egui handed it to the backend.
pub fn record_produced(
    site: Site,
    textures_delta: &epaint::textures::TexturesDelta,
    viewport: Option<u64>,
) {
    for (id, delta) in &textures_delta.set {
        record(
            site,
            *id,
            delta.pos,
            [delta.image.width(), delta.image.height()],
            None,
            viewport,
        );
    }
    for id in &textures_delta.free {
        record(Site::Freed, *id, None, [0, 0], None, viewport);
    }
}

/// Record a delta as the renderer applied it, along with the size the renderer held beforehand.
pub fn record_applied(
    site: Site,
    id: epaint::TextureId,
    delta: &epaint::ImageDelta,
    renderer_size_before: Option<[u32; 2]>,
) {
    record(
        site,
        id,
        delta.pos,
        [delta.image.width(), delta.image.height()],
        renderer_size_before,
        None,
    );
}

/// A partial delta was found to reach outside the texture the renderer holds. This is the
/// failure we are hunting: report the whole ring buffer so the drop can be located, then let the
/// caller skip the write.
pub fn report_overflow(
    id: epaint::TextureId,
    pos: [usize; 2],
    delta_size: [usize; 2],
    texture_size: [u32; 2],
) {
    let Ok(mut ledger) = LEDGER.lock() else {
        return;
    };
    if ledger.reported {
        return;
    }
    ledger.reported = true;

    let mut out = String::new();
    out.push_str(&format!(
        "[atlas-diag] OUT OF BOUNDS: id={id:?} partial pos=[{}, {}] size=[{}, {}] \
         would write x {}..{} y {}..{} into a texture of [{}, {}]\n",
        pos[0],
        pos[1],
        delta_size[0],
        delta_size[1],
        pos[0],
        pos[0] + delta_size[0],
        pos[1],
        pos[1] + delta_size[1],
        texture_size[0],
        texture_size[1],
    ));
    out.push_str(&format!(
        "[atlas-diag] history for the font atlas ({} events, newest last):\n",
        ledger.events.len()
    ));
    for event in &ledger.events {
        out.push_str("[atlas-diag]   ");
        out.push_str(&event.format());
        out.push('\n');
    }
    out.push_str(
        "[atlas-diag] read it as: every 'produced' line must be followed by a matching \
         'applied' line. A FULL produced with no FULL applied is a dropped delta (the bug is in \
         the backend). A partial reaching past the last applied FULL with no FULL produced in \
         between means egui never emitted the growth (the bug is upstream).",
    );
    drop(ledger);
    emit(out);
}
