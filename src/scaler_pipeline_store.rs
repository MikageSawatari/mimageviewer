//! Immutable scaler pipelines shared by viewer contexts on one GPU device.
//!
//! Each cell alone owns unstarted, building, ready, and failed creation states.
//! Contexts retain the returned Arc in their ordinary prepare/encode slot.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Once, OnceLock},
    time::Instant,
};

use crate::{
    adjustment::PostFilter,
    gpu_anime4k::{Anime4kResampler, STILL_IMAGE_ANIME4K_VARIANT},
    gpu_lanczos::NisPipelineSet,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScalerKind {
    Nis,
    Anime4k,
}

impl ScalerKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Nis => "nis",
            Self::Anime4k => "anime4k",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PipelineSource {
    Prebuilt,
    Waited,
    Inline,
}

impl PipelineSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Prebuilt => "prebuilt",
            Self::Waited => "waited",
            Self::Inline => "inline",
        }
    }
}

pub(crate) struct Acquisition<T> {
    pub value: Option<Arc<T>>,
    pub source: PipelineSource,
    pub elapsed_ms: f64,
    pub wait_ms: f64,
}

struct PipelineCell<T> {
    value: OnceLock<Option<Arc<T>>>,
    requested: Once,
}

impl<T> PipelineCell<T> {
    fn new() -> Self {
        Self {
            value: OnceLock::new(),
            requested: Once::new(),
        }
    }

    fn acquire(&self, build: impl FnOnce() -> Option<T>) -> Acquisition<T> {
        self.acquire_observed(build, || {})
    }

    // The observer lets CPU tests rendezvous after the initial unready probe,
    // while the worker's initializer is still held. It owns no creation state.
    fn acquire_observed(
        &self,
        build: impl FnOnce() -> Option<T>,
        after_miss: impl FnOnce(),
    ) -> Acquisition<T> {
        let started = Instant::now();
        if let Some(value) = self.value.get() {
            return Acquisition {
                value: value.clone(),
                source: PipelineSource::Prebuilt,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                wait_ms: 0.0,
            };
        }
        after_miss();
        let mut initialized_here = false;
        let value = self.value.get_or_init(|| {
            initialized_here = true;
            build().map(Arc::new)
        });
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        Acquisition {
            value: value.clone(),
            source: if initialized_here {
                PipelineSource::Inline
            } else {
                PipelineSource::Waited
            },
            elapsed_ms,
            wait_ms: if initialized_here { 0.0 } else { elapsed_ms },
        }
    }

    fn request(&self, spawn: impl FnOnce()) {
        self.requested.call_once(spawn);
    }
}

pub(crate) struct ScalerPipelineStore {
    device: wgpu::Device,
    nis: PipelineCell<NisPipelineSet>,
    anime4k: PipelineCell<Anime4kResampler>,
    startup: Once,
}

impl ScalerPipelineStore {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            device: device.clone(),
            nis: PipelineCell::new(),
            anime4k: PipelineCell::new(),
            startup: Once::new(),
        }
    }

    pub(crate) fn get_nis(&self, device: &wgpu::Device) -> Acquisition<NisPipelineSet> {
        let started = Instant::now();
        if !self.matches_device(device) {
            return device_mismatch(started);
        }
        let mut acquired = self
            .nis
            .acquire(|| build_with_scopes(device, ScalerKind::Nis, || NisPipelineSet::new(device)));
        acquired.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        acquired
    }

    pub(crate) fn get_anime4k(&self, device: &wgpu::Device) -> Acquisition<Anime4kResampler> {
        let started = Instant::now();
        if !self.matches_device(device) {
            return device_mismatch(started);
        }
        let mut acquired = self.anime4k.acquire(|| {
            build_with_scopes(device, ScalerKind::Anime4k, || {
                Anime4kResampler::new(device, STILL_IMAGE_ANIME4K_VARIANT)
            })
        });
        acquired.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        acquired
    }

    fn matches_device(&self, device: &wgpu::Device) -> bool {
        if self.device == *device {
            return true;
        }
        crate::logger::log("scaler pipeline store: different GPU device; using context creation");
        false
    }

    pub(crate) fn request(self: &Arc<Self>, kind: ScalerKind, device: &wgpu::Device) {
        if !self.matches_device(device) {
            return;
        }
        let spawn = || {
            let store = self.clone();
            let device = device.clone();
            if let Err(error) = std::thread::Builder::new()
                .name(format!("miv-scaler-prebuild-{}", kind.as_str()))
                .spawn(move || {
                    let started = Instant::now();
                    record_prebuild(kind, "begin", 0.0, None);
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        crate::similar_index::lower_current_thread_priority();
                        match kind {
                            ScalerKind::Nis => worker_result(store.get_nis(&device)),
                            ScalerKind::Anime4k => worker_result(store.get_anime4k(&device)),
                        }
                    }));
                    let result = match result {
                        Ok(result) => result,
                        Err(_) => {
                            crate::logger::log(&format!(
                                "scaler pipeline prebuild {}: worker panicked",
                                kind.as_str()
                            ));
                            "failed"
                        }
                    };
                    record_prebuild(
                        kind,
                        "end",
                        started.elapsed().as_secs_f64() * 1000.0,
                        Some(result),
                    );
                })
            {
                crate::logger::log(&format!(
                    "scaler pipeline prebuild {}: could not start worker: {error}",
                    kind.as_str()
                ));
            }
        };
        match kind {
            ScalerKind::Nis => self.nis.request(spawn),
            ScalerKind::Anime4k => self.anime4k.request(spawn),
        }
    }

    pub(crate) fn request_startup(
        self: &Arc<Self>,
        normal_presented: bool,
        device: &wgpu::Device,
        filters: impl IntoIterator<Item = PostFilter>,
    ) {
        request_startup_once(&self.startup, normal_presented, filters, |kind| {
            self.request(kind, device);
        });
    }

    #[cfg(test)]
    pub(crate) fn fail_for_test(&self, kind: ScalerKind) {
        let inserted = match kind {
            ScalerKind::Nis => self.nis.value.set(None).is_ok(),
            ScalerKind::Anime4k => self.anime4k.value.set(None).is_ok(),
        };
        assert!(inserted, "failure injection requires an uninitialized cell");
    }

    #[cfg(test)]
    pub(crate) fn wait_ready_for_test(&self, kind: ScalerKind) {
        let ready = match kind {
            ScalerKind::Nis => self.nis.value.wait().is_some(),
            ScalerKind::Anime4k => self.anime4k.value.wait().is_some(),
        };
        assert!(ready, "background scaler construction failed");
    }
}

fn device_mismatch<T>(started: Instant) -> Acquisition<T> {
    Acquisition {
        value: None,
        source: PipelineSource::Inline,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        wait_ms: 0.0,
    }
}

fn worker_result<T>(acquisition: Acquisition<T>) -> &'static str {
    if acquisition.value.is_none() {
        "failed"
    } else if acquisition.source == PipelineSource::Inline {
        "ready"
    } else {
        "already"
    }
}

fn build_with_scopes<T>(
    device: &wgpu::Device,
    kind: ScalerKind,
    build: impl FnOnce() -> T,
) -> Option<T> {
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let value = build();
    match finish_scoped_build(value, [oom, internal, validation], |scope| {
        pollster::block_on(scope.pop())
    }) {
        Ok(value) => Some(value),
        Err(errors) => {
            crate::logger::log(&format!(
                "scaler pipeline creation {} failed: {errors:?}",
                kind.as_str()
            ));
            None
        }
    }
}

fn finish_scoped_build<T, S, E>(
    value: T,
    scopes: [S; 3],
    pop: impl FnMut(S) -> Option<E>,
) -> Result<T, [Option<E>; 3]> {
    // Array::map eagerly pops every scope before any error is inspected.
    let errors = scopes.map(pop);
    if errors.iter().any(Option::is_some) {
        Err(errors)
    } else {
        Ok(value)
    }
}

pub(crate) fn startup_kinds(filters: impl IntoIterator<Item = PostFilter>) -> Vec<ScalerKind> {
    let mut kinds = Vec::with_capacity(2);
    for filter in filters {
        let kind = match filter {
            PostFilter::UpscaleSharp => ScalerKind::Nis,
            PostFilter::UpscaleAnime => ScalerKind::Anime4k,
            _ => continue,
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds
}

fn request_startup_once(
    startup: &Once,
    normal_presented: bool,
    filters: impl IntoIterator<Item = PostFilter>,
    mut request: impl FnMut(ScalerKind),
) {
    if normal_presented {
        startup.call_once(|| {
            for kind in startup_kinds(filters) {
                request(kind);
            }
        });
    }
}

fn record_prebuild(kind: ScalerKind, phase: &'static str, ms: f64, result: Option<&'static str>) {
    if !crate::perf::is_enabled() {
        return;
    }
    let mut extras = vec![
        ("scaler_kind", kind.as_str().into()),
        ("trigger", "startup".into()),
        ("phase", phase.into()),
        ("ms", ms.into()),
    ];
    if let Some(result) = result {
        extras.push(("result", result.into()));
    }
    crate::perf::event("gpu", "scaler_prebuild", None, 0, &extras);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Barrier,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };

    #[test]
    fn ui_creation_is_retained_and_later_worker_does_not_build() {
        let cell = PipelineCell::new();
        let ui = cell.acquire(|| Some(17));
        assert_eq!(ui.source, PipelineSource::Inline);
        assert_eq!(ui.wait_ms, 0.0);
        cell.request(|| {
            let worker = cell.acquire(|| panic!("UI already initialized the cell"));
            assert_eq!(worker.source, PipelineSource::Prebuilt);
            assert!(Arc::ptr_eq(
                ui.value.as_ref().unwrap(),
                worker.value.as_ref().unwrap()
            ));
            assert_eq!(worker_result(worker), "already");
        });
    }

    #[test]
    fn worker_completion_is_shared_by_two_contexts() {
        let cell = Arc::new(PipelineCell::new());
        let mut worker = None;
        cell.request(|| {
            let worker_cell = cell.clone();
            worker = Some(std::thread::spawn(move || worker_cell.acquire(|| Some(23))));
        });
        let built = worker.unwrap().join().unwrap();
        let main = cell.acquire(|| panic!("already built"));
        let detached = cell.acquire(|| panic!("already built"));
        assert_eq!(main.source, PipelineSource::Prebuilt);
        assert_eq!(detached.source, PipelineSource::Prebuilt);
        assert!(Arc::ptr_eq(
            built.value.as_ref().unwrap(),
            main.value.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            main.value.as_ref().unwrap(),
            detached.value.as_ref().unwrap()
        ));
        drop(main);
        assert_eq!(**detached.value.as_ref().unwrap(), 23);
        assert!(Arc::ptr_eq(
            cell.value.get().unwrap().as_ref().unwrap(),
            detached.value.as_ref().unwrap()
        ));
    }

    #[test]
    fn in_flight_acquisition_joins_worker_and_returns_same_arc() {
        let cell = Arc::new(PipelineCell::new());
        let building = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let worker = {
            let cell = cell.clone();
            let building = building.clone();
            let release = release.clone();
            std::thread::spawn(move || {
                cell.acquire(|| {
                    building.wait();
                    release.wait();
                    Some(29)
                })
            })
        };
        building.wait();
        let (miss_tx, miss_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let ui = {
            let cell = cell.clone();
            std::thread::spawn(move || {
                let acquired = cell.acquire_observed(
                    || panic!("worker owns this initializer"),
                    || miss_tx.send(()).unwrap(),
                );
                done_tx.send(()).unwrap();
                acquired
            })
        };
        // This notification follows the actual cell.get() miss, while the
        // worker is held inside its initializer. No scheduler sleeps are used.
        miss_rx.recv().unwrap();
        assert!(done_rx.try_recv().is_err());
        release.wait();
        let built = worker.join().unwrap();
        let acquired = ui.join().unwrap();
        assert_eq!(acquired.source, PipelineSource::Waited);
        assert_eq!(acquired.wait_ms, acquired.elapsed_ms);
        assert!(Arc::ptr_eq(
            built.value.as_ref().unwrap(),
            acquired.value.as_ref().unwrap()
        ));
    }

    #[test]
    fn failed_shared_creation_keeps_one_context_prepare_encode_slot() {
        let cell = PipelineCell::<usize>::new();
        let failed = cell.acquire(|| None);
        assert!(failed.value.is_none());
        assert_eq!(failed.source, PipelineSource::Inline);
        let mut slot = None;
        let prepare = slot
            .get_or_insert_with(|| {
                cell.acquire(|| panic!("failure is final in shared store"))
                    .value
                    .unwrap_or_else(|| Arc::new(31))
            })
            .clone();
        let encode = slot.as_ref().unwrap();
        assert!(Arc::ptr_eq(&prepare, encode));
        assert_eq!(**encode, 31);
        assert!(cell.value.get().unwrap().is_none());
    }

    #[test]
    fn panic_leaves_cell_uninitialized_for_ui_creation() {
        let cell = Arc::new(PipelineCell::<usize>::new());
        let mut worker = None;
        cell.request(|| {
            let worker_cell = cell.clone();
            worker = Some(std::thread::spawn(move || {
                catch_unwind(AssertUnwindSafe(|| {
                    worker_cell.acquire(|| panic!("fake builder panic"))
                }))
                .is_err()
            }));
        });
        assert!(worker.unwrap().join().unwrap());
        assert!(cell.value.get().is_none());
        cell.request(|| panic!("panicked worker is not started again"));
        let ui = cell.acquire(|| Some(37));
        assert_eq!(ui.source, PipelineSource::Inline);
        assert_eq!(**ui.value.as_ref().unwrap(), 37);
    }

    #[test]
    fn repeated_requests_start_only_one_worker() {
        let cell = PipelineCell::<usize>::new();
        let starts = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..16 {
                let cell = &cell;
                let starts = &starts;
                scope.spawn(move || {
                    cell.request(|| {
                        starts.fetch_add(1, Ordering::SeqCst);
                    })
                });
            }
        });
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn startup_waits_for_present_and_consumes_even_empty_selection() {
        let startup = Once::new();
        let mut requests = Vec::new();
        request_startup_once(&startup, false, [PostFilter::UpscaleSharp], |kind| {
            requests.push(kind)
        });
        assert!(!startup.is_completed());
        request_startup_once(&startup, true, [PostFilter::None], |kind| {
            requests.push(kind)
        });
        assert!(startup.is_completed());
        request_startup_once(&startup, true, [PostFilter::UpscaleAnime], |kind| {
            requests.push(kind)
        });
        assert!(requests.is_empty());
    }

    #[test]
    fn startup_selection_covers_global_slots_and_favorites_without_duplicates() {
        let global = [PostFilter::None];
        let slots = [PostFilter::UpscaleSharp, PostFilter::UpscaleSharp];
        let favorites = [PostFilter::UpscaleAnime, PostFilter::UpscalePixelArt];
        assert_eq!(
            startup_kinds(global.into_iter().chain(slots).chain(favorites)),
            vec![ScalerKind::Nis, ScalerKind::Anime4k]
        );
        assert_eq!(
            startup_kinds([PostFilter::UpscaleAnime]),
            vec![ScalerKind::Anime4k]
        );
        assert!(
            startup_kinds([
                PostFilter::None,
                PostFilter::Nearest,
                PostFilter::CrtSimple,
                PostFilter::UpscalePixelArt
            ])
            .is_empty()
        );
    }

    #[test]
    fn internal_error_becomes_shared_none_and_all_scopes_are_popped() {
        let cell = PipelineCell::<usize>::new();
        let mut popped = Vec::new();
        let acquired = cell.acquire(|| {
            finish_scoped_build(41, ["oom", "internal", "validation"], |scope| {
                popped.push(scope);
                (scope == "internal").then_some("fake internal pipeline error")
            })
            .ok()
        });
        assert!(acquired.value.is_none());
        assert_eq!(popped, ["oom", "internal", "validation"]);
        assert_eq!(worker_result(acquired), "failed");
    }

    #[test]
    fn first_scope_error_does_not_short_circuit_remaining_pops() {
        let mut popped = Vec::new();
        let result = finish_scoped_build(43, ["oom", "internal", "validation"], |scope| {
            popped.push(scope);
            Some(scope)
        });
        assert_eq!(popped, ["oom", "internal", "validation"]);
        assert_eq!(
            result,
            Err([Some("oom"), Some("internal"), Some("validation")])
        );
    }
}
