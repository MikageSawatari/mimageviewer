//! Stage A CPU boundary for 3D photos. No viewer state or GPU resources live here.
//!
//! A pending owns its cancellation and reply. The process worker owns one waiting
//! slot; replacing it never creates an unbounded image queue. Stage B calls submit
//! with App's shared runtime owner and `local_ai_activity_lease()`.
#![allow(dead_code)] // Viewer integration follows in stage B.

use crate::LocalAiActivityLease;
use crate::ai::runtime::{AiRuntime, AiRuntimeInitOwner, AiRuntimeInitWait};
use crate::ai::{AiError, ModelKind};
use crate::app::ViewerContextId;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};

pub(crate) const INPUT_SIZE: u32 = 504;
const INPUT_PIXELS: usize = INPUT_SIZE as usize * INPUT_SIZE as usize;
type CancelToken = Arc<AtomicBool>; // Existing viewer/BiRefNet cancellation convention.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DepthSourceKey {
    pub(crate) texture_id: egui::TextureId,
}

pub(crate) struct DepthGuide {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct DepthMap {
    pub(crate) key: DepthSourceKey,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) values: Vec<half::f16>,
    pub(crate) median: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RequestStamp {
    context: ViewerContextId,
    request_id: u64,
    key: DepthSourceKey,
}

struct DepthReply {
    stamp: RequestStamp,
    result: Result<DepthMap, String>,
}

pub(crate) struct DepthPending {
    stamp: RequestStamp,
    cancel: CancelToken,
    rx: mpsc::Receiver<DepthReply>,
}

impl DepthPending {
    pub(crate) fn request_id(&self) -> u64 {
        self.stamp.request_id
    }

    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    /// Nonblocking adoption. A completed reply can still be cancelled before
    /// adoption; check the same token on both sides of that publication boundary.
    pub(crate) fn try_result(
        &self,
        context: ViewerContextId,
        key: DepthSourceKey,
    ) -> Option<Result<DepthMap, String>> {
        if self.cancel.load(Ordering::Acquire) {
            return None;
        }
        if context != self.stamp.context || key != self.stamp.key {
            self.cancel();
            return None;
        }
        match self.rx.try_recv() {
            Ok(reply) if reply.stamp == self.stamp && !self.cancel.load(Ordering::Acquire) => {
                Some(reply.result)
            }
            Ok(_) | Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("深度推論 worker が終了しました".to_owned()))
            }
        }
    }
}

impl Drop for DepthPending {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct DepthRequest {
    stamp: RequestStamp,
    viewport: egui::ViewportId,
    input_504: Vec<u8>,
    guide: DepthGuide,
    cancel: CancelToken,
    reply: mpsc::Sender<DepthReply>,
    egui: egui::Context,
    runtime: Arc<AiRuntimeInitOwner>,
    // Kept until actual completion even if pending/slot is cancelled. Remote must
    // not acquire the viewer while an uninterruptible ORT inference is running.
    _activity: LocalAiActivityLease,
    input_seq: u64,
}

struct ActiveRequest {
    stamp: RequestStamp,
    cancel: CancelToken,
}

#[derive(Default)]
struct QueueState {
    slot: Option<DepthRequest>,
    active: Option<ActiveRequest>,
    shutdown: bool,
}

struct Queue {
    state: Mutex<QueueState>,
    changed: Condvar,
}

pub(crate) struct DepthWorker {
    queue: Arc<Queue>,
}

type Estimate = dyn Fn(&DepthRequest) -> Result<DepthMap, String> + Send + 'static;

impl DepthWorker {
    pub(crate) fn shared() -> Result<&'static Self, String> {
        static WORKER: OnceLock<Result<DepthWorker, String>> = OnceLock::new();
        WORKER
            .get_or_init(|| Self::start(Box::new(estimate)))
            .as_ref()
            .map_err(Clone::clone)
    }

    fn start(estimate: Box<Estimate>) -> Result<Self, String> {
        let queue = Arc::new(Queue {
            state: Mutex::new(QueueState::default()),
            changed: Condvar::new(),
        });
        let worker_queue = Arc::clone(&queue);
        std::thread::Builder::new()
            .name("depth-estimation".to_owned())
            .spawn(move || run(worker_queue, estimate))
            .map_err(|error| format!("深度推論 worker を開始できません: {error}"))?;
        Ok(Self { queue })
    }

    /// Only captures owned memory and handles. File I/O, runtime waiting, input
    /// conversion, inference and filtering all happen on depth-estimation.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn submit(
        &self,
        context: ViewerContextId,
        viewport: egui::ViewportId,
        key: DepthSourceKey,
        input_504: Vec<u8>,
        guide: DepthGuide,
        runtime: Arc<AiRuntimeInitOwner>,
        activity: LocalAiActivityLease,
        egui: egui::Context,
        input_seq: u64,
    ) -> Result<DepthPending, String> {
        static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let (reply, rx) = mpsc::channel();
        let mut state = self.queue.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.shutdown {
            return Err("深度推論 worker は終了しています".to_owned());
        }
        let stamp = RequestStamp {
            context,
            request_id: NEXT_REQUEST.fetch_add(1, Ordering::Relaxed),
            key,
        };
        // Supersession is context scoped. An active sibling is allowed to finish;
        // the single global *waiting* slot is intentionally latest-wins.
        if let Some(active) = &state.active
            && active.stamp.context == context
        {
            active.cancel.store(true, Ordering::Release);
        }
        let old = state.slot.replace(DepthRequest {
            stamp,
            viewport,
            input_504,
            guide,
            cancel: Arc::clone(&cancel),
            reply,
            runtime,
            egui,
            _activity: activity,
            input_seq,
        });
        if let Some(old) = &old {
            old.cancel.store(true, Ordering::Release);
        }
        drop(state);
        drop(old); // Release displaced image buffers, reply and lease now.
        self.queue.changed.notify_one();
        Ok(DepthPending { stamp, cancel, rx })
    }
}

impl Drop for DepthWorker {
    fn drop(&mut self) {
        let mut state = self.queue.state.lock().unwrap_or_else(|e| e.into_inner());
        state.shutdown = true;
        if let Some(active) = &state.active {
            active.cancel.store(true, Ordering::Release);
        }
        let queued = state.slot.take();
        if let Some(queued) = &queued {
            queued.cancel.store(true, Ordering::Release);
        }
        drop(state);
        drop(queued);
        self.queue.changed.notify_all(); // No UI join/wait.
    }
}

fn run(queue: Arc<Queue>, estimate: Box<Estimate>) {
    loop {
        let request = {
            let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
            while state.slot.is_none() && !state.shutdown {
                state = queue.changed.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            if state.shutdown {
                return;
            }
            let request = state.slot.take().unwrap();
            state.active = Some(ActiveRequest {
                stamp: request.stamp,
                cancel: Arc::clone(&request.cancel),
            });
            request
        };
        if !request.cancel.load(Ordering::Acquire) {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| estimate(&request)))
                    .unwrap_or_else(|_| Err("深度推論 worker でエラーが発生しました".to_owned()));
            if !request.cancel.load(Ordering::Acquire) {
                if let Err(error) = &result {
                    crate::logger::log(format!(
                        "[Depth] request {} failed: {error}",
                        request.stamp.request_id
                    ));
                }
                let _ = request.reply.send(DepthReply {
                    stamp: request.stamp,
                    result,
                });
                request.egui.request_repaint_of(request.viewport);
            }
        }
        let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active = None;
        drop(state);
    }
}

fn estimate(request: &DepthRequest) -> Result<DepthMap, String> {
    validate_guide(&request.guide)?;
    let runtime = match request
        .runtime
        .wait_terminal_while(|| !request.cancel.load(Ordering::Acquire))
    {
        AiRuntimeInitWait::Ready(runtime) => runtime,
        AiRuntimeInitWait::Failed(error) => return Err(error.to_string()),
        AiRuntimeInitWait::Cancelled => return Err(AiError::Cancelled.to_string()),
    };
    let started = crate::perf::is_enabled().then(std::time::Instant::now);
    let disparity = infer_disparity(&runtime, None, &request.input_504, &request.cancel);
    emit_duration(request, "inference", started);
    let disparity = disparity?;
    if request.cancel.load(Ordering::Acquire) {
        return Err(AiError::Cancelled.to_string());
    }
    let started = crate::perf::is_enabled().then(std::time::Instant::now);
    let prepared = crate::depth_preprocess::prepare_depth(
        &disparity,
        INPUT_SIZE,
        INPUT_SIZE,
        &request.guide.rgba,
        request.guide.width,
        request.guide.height,
    );
    emit_duration(request, "preprocess", started);
    let prepared = prepared?;
    Ok(DepthMap {
        key: request.stamp.key,
        width: prepared.width,
        height: prepared.height,
        values: prepared.values,
        median: prepared.median,
    })
}

fn emit_duration(request: &DepthRequest, kind: &str, started: Option<std::time::Instant>) {
    if let Some(started) = started {
        crate::perf::event(
            "depth",
            kind,
            None,
            request.input_seq,
            &[
                ("request_id", serde_json::json!(request.stamp.request_id)),
                (
                    "context",
                    serde_json::json!(format!("{:?}", request.stamp.context)),
                ),
                (
                    "ms",
                    serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                ),
            ],
        );
    }
}

fn validate_guide(guide: &DepthGuide) -> Result<(), String> {
    if guide.width == 0
        || guide.height == 0
        || guide.width.max(guide.height) > 1536
        || guide.rgba.len() != guide.width as usize * guide.height as usize * 4
    {
        return Err("深度ガイド画像の寸法が不正です".to_owned());
    }
    Ok(())
}

fn image_input(rgba: &[u8]) -> Result<Vec<f32>, String> {
    if rgba.len() != INPUT_PIXELS * 4 {
        return Err("深度推論の入力は 504 x 504 RGBA8 が必要です".to_owned());
    }
    let mut input = vec![0.0; INPUT_PIXELS * 3];
    for (i, pixel) in rgba.chunks_exact(4).enumerate() {
        for channel in 0..3 {
            input[channel * INPUT_PIXELS + i] = (pixel[channel] as f32 / 255.0
                - [0.485, 0.456, 0.406][channel])
                / [0.229, 0.224, 0.225][channel];
        }
    }
    Ok(input)
}

fn depth_to_disparity(depth: &[f32], sky: &[f32]) -> Result<Vec<f32>, String> {
    if depth.len() != INPUT_PIXELS || sky.len() != INPUT_PIXELS {
        return Err("深度モデルの出力サイズが不正です".to_owned());
    }
    Ok(depth
        .iter()
        .zip(sky)
        .map(|(&depth, &sky)| {
            if sky > 0.5 || !depth.is_finite() || depth <= 0.0 {
                0.0
            } else {
                let disparity = depth.recip();
                if disparity.is_finite() {
                    disparity
                } else {
                    0.0
                }
            }
        })
        .collect())
}

/// Worker/headless tests only. No dimension override: the distributed graph is fixed.
fn infer_disparity(
    runtime: &AiRuntime,
    model_path: Option<&Path>,
    rgba: &[u8],
    cancel: &AtomicBool,
) -> Result<Vec<f32>, String> {
    let input = image_input(rgba)?;
    if cancel.load(Ordering::Acquire) {
        return Err(AiError::Cancelled.to_string());
    }
    if !runtime.is_loaded(ModelKind::DepthDa3MonoLarge) {
        // Verify an installed addon once on cold load, never hash 0.67 GB again
        // for each page. The existing runtime owns the session until reload.
        let resolved;
        let path = match model_path {
            Some(path) => path,
            None => {
                resolved = crate::ai::model_manager::ModelManager::new()
                    .model_path(ModelKind::DepthDa3MonoLarge)
                    .ok_or_else(|| {
                        AiError::ModelNotFound(ModelKind::DepthDa3MonoLarge).to_string()
                    })?;
                &resolved
            }
        };
        runtime
            .load_model(ModelKind::DepthDa3MonoLarge, path)
            .map_err(|e| e.to_string())?;
    }
    let image = ort::value::Tensor::from_array(([1usize, 1, 3, 504, 504], input))
        .map_err(|e| e.to_string())?;
    let extrinsics = ort::value::Tensor::from_array(([1usize, 1, 4, 4], vec![-1.0f32; 16]))
        .map_err(|e| e.to_string())?;
    let intrinsics = ort::value::Tensor::from_array(([1usize, 1, 3, 3], vec![-1.0f32; 9]))
        .map_err(|e| e.to_string())?;
    runtime.with_session(ModelKind::DepthDa3MonoLarge, |session| {
        if cancel.load(Ordering::Acquire) { return Err(AiError::Cancelled); }
        let outputs = session.run(ort::inputs!["image" => image, "extrinsics_in" => extrinsics, "intrinsics_in" => intrinsics])
            .map_err(|e| AiError::ort_status(&e, format!("DA3 inference: {e}")))?;
        let depth_value = outputs.get("depth").ok_or_else(|| AiError::Ort("DA3 depth output missing".to_owned()))?;
        let sky_value = outputs.get("sky").ok_or_else(|| AiError::Ort("DA3 sky output missing".to_owned()))?;
        let (depth_shape, depth) = depth_value.try_extract_tensor::<f32>().map_err(|e| AiError::ort_status(&e, format!("DA3 depth: {e}")))?;
        let (sky_shape, sky) = sky_value.try_extract_tensor::<f32>().map_err(|e| AiError::ort_status(&e, format!("DA3 sky: {e}")))?;
        if depth_shape.as_ref() != [1i64, 1, 504, 504] || sky_shape.as_ref() != [1i64, 1, 504, 504] {
            return Err(AiError::Ort("DA3 expected [1,1,504,504] outputs".to_owned()));
        }
        depth_to_disparity(depth, sky).map_err(AiError::ImageProcessing)
    }).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    const DEADLINE: Duration = Duration::from_secs(20);

    fn key(id: u64) -> DepthSourceKey {
        DepthSourceKey {
            texture_id: egui::TextureId::Managed(id),
        }
    }

    fn map(request: &DepthRequest) -> DepthMap {
        DepthMap {
            key: request.stamp.key,
            width: 1,
            height: 1,
            values: vec![half::f16::from_f32(0.5)],
            median: 0.5,
        }
    }

    struct Harness {
        worker: DepthWorker,
        started: mpsc::Receiver<RequestStamp>,
        finish: mpsc::Sender<()>,
        activity: Arc<AtomicUsize>,
    }

    impl Harness {
        fn new() -> Self {
            let (started_tx, started) = mpsc::channel();
            let (finish, finish_rx) = mpsc::channel();
            let worker = DepthWorker::start(Box::new(move |request| {
                started_tx.send(request.stamp).unwrap();
                finish_rx.recv_timeout(DEADLINE).unwrap();
                Ok(map(request))
            }))
            .unwrap();
            Self {
                worker,
                started,
                finish,
                activity: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn submit(&self, context: u64, texture: u64) -> DepthPending {
            self.worker
                .submit(
                    ViewerContextId::for_test(context),
                    egui::ViewportId::ROOT,
                    key(texture),
                    vec![],
                    DepthGuide {
                        width: 1,
                        height: 1,
                        rgba: vec![0; 4],
                    },
                    AiRuntimeInitOwner::new(),
                    LocalAiActivityLease::new(self.activity.clone()),
                    egui::Context::default(),
                    0,
                )
                .unwrap()
        }

        fn started(&self) -> RequestStamp {
            self.started.recv_timeout(DEADLINE).unwrap()
        }

        fn finish(&self) {
            self.finish.send(()).unwrap();
        }
    }

    #[test]
    fn depth_input_preserves_srgb_bytes_and_imagenet_chw() {
        let mut rgba = vec![0u8; INPUT_PIXELS * 4];
        rgba[..4].copy_from_slice(&[128, 64, 255, 0]);
        let input = image_input(&rgba).unwrap();
        assert!((input[0] - (128.0 / 255.0 - 0.485) / 0.229).abs() < 1e-6);
        assert!((input[INPUT_PIXELS] - (64.0 / 255.0 - 0.456) / 0.224).abs() < 1e-6);
        assert!((input[INPUT_PIXELS * 2] - (1.0 - 0.406) / 0.225).abs() < 1e-6);
        assert!(image_input(&rgba[..100]).is_err());
    }

    #[test]
    fn depth_sky_invalid_depth_and_inverse_are_finite() {
        let mut depth = vec![2.0; INPUT_PIXELS];
        let mut sky = vec![0.0; INPUT_PIXELS];
        depth[..8].copy_from_slice(&[
            0.0,
            -1.0,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            4.0,
            f32::MIN_POSITIVE / 100.0,
            2.0,
        ]);
        sky[5] = 0.51;
        sky[7] = 0.5; // Exact boundary remains foreground.
        let disparity = depth_to_disparity(&depth, &sky).unwrap();
        assert_eq!(&disparity[..7], &[0.0; 7]);
        assert_eq!(disparity[7], 0.5);
        assert_eq!(disparity[8], 0.5);
        assert!(depth_to_disparity(&[], &sky).is_err());
    }

    #[test]
    fn depth_model_is_distinct_and_not_an_upscale_or_trt_pack_model() {
        let kind = ModelKind::DepthDa3MonoLarge;
        assert_eq!(ModelKind::from_str(kind.as_str()), Some(kind));
        assert!(!ModelKind::upscale_models().contains(&kind));
        assert!(!ModelKind::denoise_models().contains(&kind));
        assert!(!crate::ai::trt_worker_lifecycle::model_uses_trt_worker(
            kind
        ));
    }

    #[test]
    fn depth_pending_drop_cancels_its_own_request() {
        let harness = Harness::new();
        let pending = harness.submit(1, 9);
        harness.started();
        let cancel = pending.cancel.clone();
        drop(pending);
        assert!(cancel.load(Ordering::Acquire));
        assert_eq!(harness.activity.load(Ordering::Acquire), 1);
        harness.finish();
    }

    #[test]
    fn depth_slot_replacement_cancels_and_releases_the_waiting_request() {
        let harness = Harness::new();
        let running = harness.submit(1, 1);
        harness.started();
        let waiting = harness.submit(2, 2);
        let newest = harness.submit(3, 3);
        assert!(!running.cancel.load(Ordering::Acquire));
        assert!(waiting.cancel.load(Ordering::Acquire));
        assert!(!newest.cancel.load(Ordering::Acquire));
        assert_eq!(harness.activity.load(Ordering::Acquire), 2);
        assert!(matches!(
            waiting.rx.recv_timeout(DEADLINE),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        harness.finish();
        assert_eq!(
            running.rx.recv_timeout(DEADLINE).unwrap().stamp,
            running.stamp
        );
        assert_eq!(harness.started(), newest.stamp);
        harness.finish();
        assert_eq!(
            newest.rx.recv_timeout(DEADLINE).unwrap().stamp,
            newest.stamp
        );
    }

    #[test]
    fn depth_new_request_discards_old_inference_even_for_the_same_texture() {
        let harness = Harness::new();
        let old = harness.submit(1, 7);
        harness.started();
        let new = harness.submit(1, 7);
        assert!(new.request_id() > old.request_id());
        assert!(old.cancel.load(Ordering::Acquire));
        harness.finish();
        assert_eq!(harness.started(), new.stamp);
        assert!(matches!(
            old.rx.recv_timeout(DEADLINE),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        harness.finish();
        let reply = new.rx.recv_timeout(DEADLINE).unwrap();
        assert_eq!(reply.stamp, new.stamp);
        assert_eq!(reply.result.unwrap().key, key(7));
    }

    #[test]
    fn depth_cancelled_inference_discards_result() {
        let harness = Harness::new();
        let pending = harness.submit(1, 1);
        harness.started();
        pending.cancel();
        harness.finish();
        assert!(matches!(
            pending.rx.recv_timeout(DEADLINE),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        assert!(
            pending
                .try_result(pending.stamp.context, pending.stamp.key)
                .is_none()
        );
    }

    #[test]
    fn depth_separate_contexts_have_private_receivers_and_cancellation() {
        let harness = Harness::new();
        let first = harness.submit(1, 7);
        harness.started();
        let second = harness.submit(2, 7);
        assert!(!first.cancel.load(Ordering::Acquire));
        harness.finish();
        assert_eq!(harness.started(), second.stamp);
        let first_reply = first.rx.recv_timeout(DEADLINE).unwrap();
        assert_eq!(first_reply.stamp, first.stamp);
        assert!(matches!(
            second.rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(first); // Cancelling a retired sibling cannot cancel the active one.
        assert!(!second.cancel.load(Ordering::Acquire));
        harness.finish();
        assert_eq!(
            second.rx.recv_timeout(DEADLINE).unwrap().stamp,
            second.stamp
        );
    }

    fn completed_pending() -> DepthPending {
        let stamp = RequestStamp {
            context: ViewerContextId::for_test(1),
            request_id: 42,
            key: key(7),
        };
        let (tx, rx) = mpsc::channel();
        tx.send(DepthReply {
            stamp,
            result: Ok(DepthMap {
                key: key(7),
                width: 1,
                height: 1,
                values: vec![half::f16::from_f32(0.5)],
                median: 0.5,
            }),
        })
        .unwrap();
        DepthPending {
            stamp,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn depth_adoption_rejects_stale_key_context_and_completed_cancellation() {
        let pending = completed_pending();
        assert!(pending.try_result(pending.stamp.context, key(8)).is_none());
        let pending = completed_pending();
        assert!(
            pending
                .try_result(ViewerContextId::for_test(2), key(7))
                .is_none()
        );
        let pending = completed_pending();
        pending.cancel();
        assert!(pending.try_result(pending.stamp.context, key(7)).is_none());
        let pending = completed_pending();
        assert_eq!(
            pending
                .try_result(pending.stamp.context, key(7))
                .unwrap()
                .unwrap()
                .median,
            0.5
        );
    }

    #[test]
    fn depth_adoption_rejects_a_reply_from_another_request() {
        let mut pending = completed_pending();
        pending.stamp.request_id += 1;
        assert!(pending.try_result(pending.stamp.context, key(7)).is_none());
    }

    #[test]
    fn depth_runtime_failure_is_delivered_without_model_io() {
        let worker = DepthWorker::start(Box::new(estimate)).unwrap();
        let pending = worker
            .submit(
                ViewerContextId::for_test(1),
                egui::ViewportId::ROOT,
                key(1),
                vec![0; INPUT_PIXELS * 4],
                DepthGuide {
                    width: 1,
                    height: 1,
                    rgba: vec![0; 4],
                },
                AiRuntimeInitOwner::failed_for_test("test runtime init failed"),
                LocalAiActivityLease::new(Arc::new(AtomicUsize::new(0))),
                egui::Context::default(),
                0,
            )
            .unwrap();
        assert!(
            pending
                .rx
                .recv_timeout(DEADLINE)
                .unwrap()
                .result
                .unwrap_err()
                .contains("test runtime init failed")
        );
    }

    #[test]
    fn depth_guide_contract_rejects_oversized_or_malformed_images() {
        assert!(
            validate_guide(&DepthGuide {
                width: 0,
                height: 1,
                rgba: vec![]
            })
            .is_err()
        );
        assert!(
            validate_guide(&DepthGuide {
                width: 1537,
                height: 1,
                rgba: vec![0; 1537 * 4]
            })
            .is_err()
        );
        assert!(
            validate_guide(&DepthGuide {
                width: 1,
                height: 1,
                rgba: vec![0; 3]
            })
            .is_err()
        );
    }

    /// Generate oracle files with tools/depth_model/generate_fixtures.py.
    /// Explicitly selected headless test; never launches the application or uses
    /// the real profile. nextest gives ORT its own process/backend lifetime.
    #[test]
    #[ignore = "requires MIV_DEPTH_MODEL_DIR and the locally built DA3 model/oracle"]
    fn depth_real_model_matches_python_static_stretch() {
        let _data = crate::data_dir::TestDataDirGuard::new();
        let root = std::path::PathBuf::from(
            std::env::var_os("MIV_DEPTH_MODEL_DIR").expect("MIV_DEPTH_MODEL_DIR"),
        );
        let rgba = std::fs::read(root.join("reference-input.rgba")).unwrap();
        let reference = std::fs::read(root.join("reference-disparity.f32")).unwrap();
        let reference: Vec<f32> = reference
            .chunks_exact(4)
            .map(|v| f32::from_le_bytes(v.try_into().unwrap()))
            .collect();
        let owner = AiRuntimeInitOwner::new();
        owner.start(|| {});
        let runtime = match owner.wait_terminal_while(|| true) {
            AiRuntimeInitWait::Ready(runtime) => runtime,
            _ => panic!("runtime initialization failed"),
        };
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("reference.json")).unwrap()).unwrap();
        let guide = std::fs::read(root.join("reference-guide.rgba")).unwrap();
        let (gw, gh) = (
            metadata["guide_width"].as_u64().unwrap() as u32,
            metadata["guide_height"].as_u64().unwrap() as u32,
        );
        let model_path = root.join("da3mono_large_fp16_504.onnx");
        let (actual, prepared) = std::thread::Builder::new()
            .name("depth-estimation".into())
            .spawn(move || {
                let actual =
                    infer_disparity(&runtime, Some(&model_path), &rgba, &AtomicBool::new(false))
                        .unwrap();
                let prepared = crate::depth_preprocess::prepare_depth(
                    &actual, INPUT_SIZE, INPUT_SIZE, &guide, gw, gh,
                )
                .unwrap();
                (actual, prepared)
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(actual.len(), INPUT_PIXELS);
        assert_eq!(reference.len(), INPUT_PIXELS);
        let mean = |v: &[f32]| v.iter().map(|&x| x as f64).sum::<f64>() / v.len() as f64;
        let (am, bm) = (mean(&actual), mean(&reference));
        let (mut covariance, mut avar, mut bvar) = (0.0, 0.0, 0.0);
        for (&a, &b) in actual.iter().zip(&reference) {
            let (a, b) = (a as f64 - am, b as f64 - bm);
            covariance += a * b;
            avar += a * a;
            bvar += b * b;
        }
        let correlation = covariance / (avar * bvar).sqrt();
        println!("DA3 504x504 Python stretch correlation: {correlation:.8}");
        assert!(correlation > 0.999, "correlation={correlation}");
        let reference_prepared = std::fs::read(root.join("reference-prepared.f32")).unwrap();
        let reference_prepared: Vec<f32> = reference_prepared
            .chunks_exact(4)
            .map(|v| f32::from_le_bytes(v.try_into().unwrap()))
            .collect();
        assert_eq!((prepared.width, prepared.height), (gw, gh));
        assert_eq!(prepared.values.len(), reference_prepared.len());
        let errors: Vec<f32> = prepared
            .values
            .iter()
            .zip(&reference_prepared)
            .map(|(a, &b)| (a.to_f32() - b).abs())
            .collect();
        let max_error = errors.iter().copied().fold(0.0_f32, f32::max);
        let mean_error = errors.iter().map(|&v| v as f64).sum::<f64>() / errors.len() as f64;
        let median_error = (prepared.median as f64 - metadata["median"].as_f64().unwrap()).abs();
        println!(
            "DA3 prepared {gw}x{gh} all_pixels={} max_error={max_error:.8} mean_error={mean_error:.8} median_error={median_error:.8}",
            errors.len()
        );
        // Compare every pixel to the float32 Python oracle. f16 quantization
        // alone permits <=1/4096 absolute error; leave 5.6e-5 for ORT/OpenCV
        // float32 accumulation differences. Median is measured before f16.
        assert!(max_error < 3e-4, "max_error={max_error}");
        assert!(mean_error < 1e-4, "mean_error={mean_error}");
        assert!(median_error < 2e-5, "median_error={median_error}");
    }
}
