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
    /// `integration.update`: egui begin/end pass (incl. building AccessKit) + `App::ui`.
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

pub fn record_root_paint(stages: PaintStages) {
    *PAINT.lock().unwrap() = Some(stages);
}

pub fn take_root_paint() -> Option<PaintStages> {
    PAINT.lock().unwrap().take()
}

pub fn record_root_pass(pass: RootPass) {
    *ROOT.lock().unwrap() = Some(pass);
}

/// The previous completed ROOT pass (taken once).
pub fn take_root_pass() -> Option<RootPass> {
    ROOT.lock().unwrap().take()
}

pub fn mark_root_pass_begin(at: Instant) {
    *ROOT_BEGIN.lock().unwrap() = Some(at);
}

/// When eframe entered the ROOT pass that is running now.
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
