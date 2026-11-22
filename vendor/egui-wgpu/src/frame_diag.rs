//! mIV: per-frame ROOT paint diagnostics (§1.827 measurement).
//!
//! Off unless the app enables it (only while its perf log is on). eframe records
//! the time it spends after `App::ui` returns, and the painter records its GPU
//! stages, for the ROOT viewport only. The app takes the record at its next
//! frame and logs it; nothing here changes what is drawn.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// GPU-side stages of one ROOT paint, in milliseconds.
#[derive(Clone, Copy, Debug, Default)]
pub struct PaintStages {
    pub textures_ms: f64,
    pub buffers_ms: f64,
    pub acquire_ms: f64,
    pub encode_ms: f64,
    pub submit_ms: f64,
    pub release_ms: f64,
    pub present_ms: f64,
    pub meshes: usize,
    pub vertices: usize,
    pub indices: usize,
    pub delta_set: usize,
    pub delta_free: usize,
    pub delta_bytes: usize,
    pub presented: bool,
}

/// eframe's ROOT pass after the app returned, in milliseconds.
#[derive(Clone, Copy, Debug, Default)]
pub struct RootPass {
    /// `integration.update` as a whole: every egui pass of this operation (incl. retries
    /// and building AccessKit) with the app's logic and UI inside. Not separable here.
    pub run_ui_ms: f64,
    pub finalize_ms: f64,
    /// Platform output, including handing the AccessKit update to the OS adapter.
    pub platform_ms: f64,
    pub tessellate_ms: f64,
    pub paint_ms: f64,
    pub viewport_out_ms: f64,
    pub accesskit_nodes: Option<usize>,
    pub shapes: usize,
    pub clipped_primitives: usize,
    pub paint: Option<PaintStages>,
    pub end: Option<Instant>,
}

static PAINT: Mutex<Option<PaintStages>> = Mutex::new(None);
static ROOT: Mutex<Option<RootPass>> = Mutex::new(None);
static ROOT_BEGIN: Mutex<Option<Instant>> = Mutex::new(None);
/// True only between `begin_root_pass` and `finish_root_pass` of one eframe ROOT operation.
static ROOT_ACTIVE: AtomicBool = AtomicBool::new(false);

/// One eframe ROOT operation being recorded. Every exit closes it: `finish` publishes
/// the record; dropping it unfinished (a logic-only pass, an early return) discards it.
#[must_use]
pub struct RootPassScope {
    finished: bool,
}

/// Start one ROOT operation's record; drops any paint record not owned by it.
pub fn begin_root_pass(at: Instant) -> RootPassScope {
    *PAINT.lock().unwrap() = None;
    *ROOT_BEGIN.lock().unwrap() = Some(at);
    ROOT_ACTIVE.store(true, Ordering::Relaxed);
    RootPassScope { finished: false }
}

impl RootPassScope {
    /// Close the operation: attach its own paint record (None when it did not paint).
    pub fn finish(mut self, mut pass: RootPass) {
        self.finished = true;
        ROOT_ACTIVE.store(false, Ordering::Relaxed);
        pass.paint = PAINT.lock().unwrap().take();
        *ROOT.lock().unwrap() = Some(pass);
    }
}

impl Drop for RootPassScope {
    fn drop(&mut self) {
        if !self.finished {
            ROOT_ACTIVE.store(false, Ordering::Relaxed);
            *PAINT.lock().unwrap() = None;
        }
    }
}

/// Whether the painter may record its stages (inside a recorded ROOT operation).
pub fn root_pass_active() -> bool {
    ROOT_ACTIVE.load(Ordering::Relaxed)
}

fn record_root_paint(stages: PaintStages) {
    if root_pass_active() {
        *PAINT.lock().unwrap() = Some(stages);
    }
}


/// The previous completed ROOT pass (taken once).
pub fn take_root_pass() -> Option<RootPass> {
    ROOT.lock().unwrap().take()
}

/// When eframe entered the ROOT operation that is running now. The gap from the
/// previous operation's end is the time between ROOT operations (event wait, other
/// viewports, logic-only passes), not purely the event-loop wait.
pub fn root_pass_begin() -> Option<Instant> {
    *ROOT_BEGIN.lock().unwrap()
}

/// Stage clock used inside the painter.
pub struct PaintClock {
    last: Instant,
    pub stages: PaintStages,
}

impl PaintClock {
    pub fn start(
        clipped_primitives: &[epaint::ClippedPrimitive],
        textures_delta: &epaint::textures::TexturesDelta,
    ) -> Self {
        let mut stages = PaintStages::default();
        for primitive in clipped_primitives {
            if let epaint::Primitive::Mesh(mesh) = &primitive.primitive {
                stages.meshes += 1;
                stages.vertices += mesh.vertices.len();
                stages.indices += mesh.indices.len();
            }
        }
        stages.delta_set = textures_delta.set.iter().map(|(_, deltas)| deltas.len()).sum();
        stages.delta_free = textures_delta.free.len();
        stages.delta_bytes = textures_delta
            .set
            .iter()
            .flat_map(|(_, deltas)| deltas.iter())
            .map(|delta| {
                delta.image.bytes_per_pixel() * delta.image.width() * delta.image.height()
            })
            .sum();
        Self {
            last: Instant::now(),
            stages,
        }
    }

    pub fn lap(&mut self) -> f64 {
        let now = Instant::now();
        let ms = now.duration_since(self.last).as_secs_f64() * 1000.0;
        self.last = now;
        ms
    }
}

impl Drop for PaintClock {
    fn drop(&mut self) {
        record_root_paint(self.stages);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unfinished_root_scope_closes_and_discards_its_paint() {
        let scope = begin_root_pass(Instant::now());
        assert!(root_pass_active());
        record_root_paint(PaintStages {
            meshes: 1,
            ..Default::default()
        });
        drop(scope); // logic-only pass or early return
        assert!(!root_pass_active());
        // Outside a scope the painter does not record, and nothing leaks into the next one.
        record_root_paint(PaintStages::default());
        let scope = begin_root_pass(Instant::now());
        scope.finish(RootPass::default());
        let pass = take_root_pass().unwrap();
        assert!(pass.paint.is_none());
        assert!(!root_pass_active());
    }
}
