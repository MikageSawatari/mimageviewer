#[cfg(debug_assertions)]
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock, mpsc};
use std::time::{Duration, Instant};

use mimageviewer_ipc::{
    ContainerEntry, ContainerEntryKind, ContainerKind, ContainerOpenMode, ContainerPayload,
    ContainerRequest, ContainerResponse, FolderListEntry, FolderListPayload, FolderListRequest,
    FolderListResponse, MediaError, MediaErrorCode, PageGroup, PagePayload, PagePriority,
    PageRequest, PageResponse, RemoteAddress, RemoteAiProgressPhase, RemoteAiStartRequest,
    RemoteAiTerminalCode, RemoteBookBookmarkList, RemoteBookBookmarkRow, RemoteBookBookmarkTarget,
    RemoteEntryKind, RemotePageDisplaySlot, RemotePageRenderContext, RemoteReadingDirection,
    RemoteSpreadMode, RemoteSubresource, RemoteWriteError, RemoteWriteErrorCode,
    RemoteWriteRequest, RemoteWriteResponse, RemoteWriteResult, ThumbnailError, ThumbnailErrorCode,
    ThumbnailResponse,
};

use super::path_guard::{
    ResolveError, ResolvedPath, page_identity_from_resolved, resolve_existing,
};
use super::thumbnail::WorkerContext;

const CONTAINER_ENTRY_LIMIT: usize = 100_000;
// A page group contributes at most one `pages` address and one `anchor` address
// per listed entry. 32 bytes per entry also covers object/array syntax and commas.
const PAGE_GROUP_JSON_OVERHEAD_PER_ENTRY: usize = 32;
const REMOTE_COMPOSITE_CACHE_ENTRIES: usize = 8;
const REMOTE_COMPOSITE_CACHE_BYTES: usize = 128 * 1024 * 1024;
const REMOTE_AUTO_TRIM_CACHE_ENTRIES: usize = 64;
const REMOTE_LUT_CACHE_ENTRIES: usize = 16;
const MAX_PAGE_RENDER_PX: u32 = crate::pdf_loader::PDF_RENDER_MAX_LONG_PX;
const PAGE_JPEG_QUALITY: i32 = 85;
/// Bump only when the native remote AI pipeline changes pixel semantics.
const REMOTE_AI_PIPELINE_SCHEMA: u32 = 1;

struct ContainerEntryBudget {
    estimated_bytes: usize,
    maximum_bytes: usize,
}

impl ContainerEntryBudget {
    fn new(maximum_bytes: usize) -> Self {
        // Brackets for both the `entries` and `page_groups` JSON arrays.
        Self {
            estimated_bytes: 4,
            maximum_bytes,
        }
    }

    fn try_include(&mut self, entry: &ContainerEntry) -> bool {
        let address_bytes = super::serialized_json_len(&entry.address).saturating_add(1);
        let entry_bytes = super::serialized_json_len(entry)
            .saturating_add(1)
            .saturating_add(address_bytes.saturating_mul(2))
            .saturating_add(PAGE_GROUP_JSON_OVERHEAD_PER_ENTRY);
        if self.estimated_bytes.saturating_add(entry_bytes) > self.maximum_bytes {
            return false;
        }
        self.estimated_bytes = self.estimated_bytes.saturating_add(entry_bytes);
        true
    }
}

fn container_limit_metadata(
    total_entries: usize,
    returned_entries: usize,
    byte_truncated: bool,
) -> (usize, bool) {
    let truncated = total_entries > CONTAINER_ENTRY_LIMIT || byte_truncated;
    (
        super::response_entry_limit(CONTAINER_ENTRY_LIMIT, returned_entries, truncated),
        truncated,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemotePageStage {
    Resolve,
    Source,
    Compose,
    Trim,
    Resize,
    Jpeg,
    Total,
}

impl RemotePageStage {
    #[cfg(test)]
    const ORDERED: [Self; 7] = [
        Self::Resolve,
        Self::Source,
        Self::Compose,
        Self::Trim,
        Self::Resize,
        Self::Jpeg,
        Self::Total,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Resolve => "resolve",
            Self::Source => "source",
            Self::Compose => "compose",
            Self::Trim => "trim",
            Self::Resize => "resize",
            Self::Jpeg => "jpeg",
            Self::Total => "total",
        }
    }

    fn counter(self) -> &'static RemotePageStageCounter {
        match self {
            Self::Resolve => &REMOTE_PAGE_RESOLVE_COUNTER,
            Self::Source => &REMOTE_PAGE_SOURCE_COUNTER,
            Self::Compose => &REMOTE_PAGE_COMPOSE_COUNTER,
            Self::Trim => &REMOTE_PAGE_TRIM_COUNTER,
            Self::Resize => &REMOTE_PAGE_RESIZE_COUNTER,
            Self::Jpeg => &REMOTE_PAGE_JPEG_COUNTER,
            Self::Total => &REMOTE_PAGE_TOTAL_COUNTER,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemotePageConcurrency {
    active_others: usize,
    active_total: usize,
}

fn remote_page_concurrency_on_enter(active_before: usize) -> RemotePageConcurrency {
    RemotePageConcurrency {
        active_others: active_before,
        active_total: active_before.saturating_add(1),
    }
}

fn remote_page_concurrency_on_exit(active_before: usize) -> Option<usize> {
    active_before.checked_sub(1)
}

struct RemotePageStageCounter {
    active: AtomicUsize,
}

impl RemotePageStageCounter {
    const fn new() -> Self {
        Self {
            active: AtomicUsize::new(0),
        }
    }

    fn enter(&self) -> RemotePageConcurrency {
        remote_page_concurrency_on_enter(self.active.fetch_add(1, Ordering::AcqRel))
    }

    fn leave(&self) {
        let mut active_before = self.active.load(Ordering::Acquire);
        while let Some(active_after) = remote_page_concurrency_on_exit(active_before) {
            match self.active.compare_exchange_weak(
                active_before,
                active_after,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(actual) => active_before = actual,
            }
        }
    }

    #[cfg(test)]
    fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }
}

struct RemotePageStageLease<'a> {
    counter: &'a RemotePageStageCounter,
    concurrency: RemotePageConcurrency,
}

impl<'a> RemotePageStageLease<'a> {
    fn new(counter: &'a RemotePageStageCounter) -> Self {
        Self {
            counter,
            concurrency: counter.enter(),
        }
    }
}

impl Drop for RemotePageStageLease<'_> {
    fn drop(&mut self) {
        self.counter.leave();
    }
}

static REMOTE_PAGE_RESOLVE_COUNTER: RemotePageStageCounter = RemotePageStageCounter::new();
static REMOTE_PAGE_SOURCE_COUNTER: RemotePageStageCounter = RemotePageStageCounter::new();
static REMOTE_PAGE_COMPOSE_COUNTER: RemotePageStageCounter = RemotePageStageCounter::new();
static REMOTE_PAGE_TRIM_COUNTER: RemotePageStageCounter = RemotePageStageCounter::new();
static REMOTE_PAGE_RESIZE_COUNTER: RemotePageStageCounter = RemotePageStageCounter::new();
static REMOTE_PAGE_JPEG_COUNTER: RemotePageStageCounter = RemotePageStageCounter::new();
static REMOTE_PAGE_TOTAL_COUNTER: RemotePageStageCounter = RemotePageStageCounter::new();

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RemotePageStageMetrics {
    pixels: u64,
    bytes: u64,
    output_pixels: u64,
    output_bytes: u64,
}

impl RemotePageStageMetrics {
    fn buffer(width: usize, height: usize, bytes_per_pixel: u64) -> Self {
        let pixels = (width as u64).saturating_mul(height as u64);
        Self {
            pixels,
            bytes: pixels.saturating_mul(bytes_per_pixel),
            output_pixels: pixels,
            output_bytes: pixels.saturating_mul(bytes_per_pixel),
        }
    }

    fn with_output(mut self, width: usize, height: usize, bytes_per_pixel: u64) -> Self {
        self.output_pixels = (width as u64).saturating_mul(height as u64);
        self.output_bytes = self.output_pixels.saturating_mul(bytes_per_pixel);
        self
    }
}

fn remote_page_file_metrics(file_size: i64) -> RemotePageStageMetrics {
    let bytes = u64::try_from(file_size).unwrap_or(0);
    RemotePageStageMetrics {
        bytes,
        output_bytes: bytes,
        ..RemotePageStageMetrics::default()
    }
}

struct RemotePagePerfContext {
    key: String,
    job_id: String,
    display_request_id: Option<String>,
    source_kind: &'static str,
    priority: &'static str,
}

#[derive(Clone, Default)]
struct RemotePagePerf(Option<Arc<RemotePagePerfContext>>);

impl RemotePagePerf {
    fn new(request: &PageRequest, source_kind: &'static str) -> Self {
        if !crate::perf::is_enabled() {
            return Self::default();
        }
        let key = remote_page_perf_key(&request.address);
        Self(Some(Arc::new(RemotePagePerfContext {
            key,
            job_id: request.job_id.clone(),
            display_request_id: request.display_request_id.clone(),
            source_kind,
            priority: if request.priority == PagePriority::Prefetch {
                "prefetch"
            } else {
                "foreground"
            },
        })))
    }

    fn enter(&self, stage: RemotePageStage) -> Option<RemotePageStageGuard> {
        let context = Arc::clone(self.0.as_ref()?);
        let lease = RemotePageStageLease::new(stage.counter());
        Some(RemotePageStageGuard {
            context,
            stage,
            started: Instant::now(),
            lease: Some(lease),
            wait_ms: 0.0,
            phases: Vec::new(),
            metrics: RemotePageStageMetrics::default(),
            outcome: "aborted",
        })
    }

    fn record_skipped(
        &self,
        stage: RemotePageStage,
        metrics: RemotePageStageMetrics,
        reason: &'static str,
    ) {
        if let Some(mut guard) = self.enter(stage) {
            guard.metrics = metrics;
            guard.outcome = reason;
        }
    }
}

struct RemotePageStageGuard {
    context: Arc<RemotePagePerfContext>,
    stage: RemotePageStage,
    started: Instant,
    lease: Option<RemotePageStageLease<'static>>,
    wait_ms: f64,
    phases: Vec<(&'static str, f64)>,
    metrics: RemotePageStageMetrics,
    outcome: &'static str,
}

impl RemotePageStageGuard {
    fn add_lock_wait(&mut self, started: Instant) {
        self.add_lock_wait_ms(started.elapsed().as_secs_f64() * 1000.0);
    }

    fn add_lock_wait_ms(&mut self, wait_ms: f64) {
        self.wait_ms += wait_ms;
    }

    fn add_phase(&mut self, name: &'static str, ms: f64) {
        self.phases.push((name, ms));
    }

    fn phase_from(&mut self, name: &'static str, started: Instant) {
        self.add_phase(name, started.elapsed().as_secs_f64() * 1000.0);
    }

    /// mutex 待ちを含み得る区間。待ちは `wait_ms` に別途積まれているので、
    /// そのまま記録すると同じ時間を 2 箇所で数えることになる。呼び出しの直前に
    /// 読んだ `wait_ms` を渡すと、その区間で増えた分だけを差し引く。
    fn phase_from_excluding_wait(
        &mut self,
        name: &'static str,
        started: Instant,
        wait_ms_before: f64,
    ) {
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let waited_ms = (self.wait_ms - wait_ms_before).max(0.0);
        self.add_phase(name, (elapsed_ms - waited_ms).max(0.0));
    }

    fn finish(&mut self, metrics: RemotePageStageMetrics) {
        self.finish_with_outcome(metrics, "ok");
    }

    fn finish_with_outcome(&mut self, metrics: RemotePageStageMetrics, outcome: &'static str) {
        self.metrics = metrics;
        self.outcome = outcome;
    }
}

impl Drop for RemotePageStageGuard {
    fn drop(&mut self) {
        let ms = self.started.elapsed().as_secs_f64() * 1000.0;
        let (phases, unaccounted_ms) = remote_page_phase_summary(ms, &self.phases);
        let concurrency = self
            .lease
            .as_ref()
            .map(|lease| lease.concurrency)
            .unwrap_or(remote_page_concurrency_on_enter(0));
        drop(self.lease.take());
        let mut extras = vec![
            ("stage", serde_json::Value::from(self.stage.name())),
            ("ms", serde_json::Value::from(ms)),
            ("wait_ms", serde_json::Value::from(self.wait_ms)),
            (
                "active_others",
                serde_json::Value::from(concurrency.active_others),
            ),
            (
                "active_total",
                serde_json::Value::from(concurrency.active_total),
            ),
            ("pixels", serde_json::Value::from(self.metrics.pixels)),
            ("bytes", serde_json::Value::from(self.metrics.bytes)),
            (
                "output_pixels",
                serde_json::Value::from(self.metrics.output_pixels),
            ),
            (
                "output_bytes",
                serde_json::Value::from(self.metrics.output_bytes),
            ),
            ("outcome", serde_json::Value::from(self.outcome)),
            ("unaccounted_ms", serde_json::Value::from(unaccounted_ms)),
            (
                "source_kind",
                serde_json::Value::from(self.context.source_kind),
            ),
            ("priority", serde_json::Value::from(self.context.priority)),
            (
                "job_id",
                serde_json::Value::from(self.context.job_id.clone()),
            ),
        ];
        if let Some(display_request_id) = self.context.display_request_id.as_ref() {
            extras.push((
                "display_request_id",
                serde_json::Value::from(display_request_id.clone()),
            ));
        }
        if let Some(phases) = phases {
            extras.push(("phases", phases));
        }
        crate::perf::event("remote_page", "stage", Some(&self.context.key), 0, &extras);
    }
}

fn remote_page_phase_summary(
    stage_ms: f64,
    phases: &[(&'static str, f64)],
) -> (Option<serde_json::Value>, f64) {
    let mut phase_map = serde_json::Map::new();
    let mut phase_ms = 0.0;
    for (name, ms) in phases {
        phase_ms += ms;
        let accumulated = phase_map
            .get(*name)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0)
            + ms;
        phase_map.insert((*name).to_owned(), serde_json::Value::from(accumulated));
    }
    let phases = (!phase_map.is_empty()).then(|| serde_json::Value::Object(phase_map));
    (phases, (stage_ms - phase_ms).max(0.0))
}

struct RemotePageLoadTiming {
    perf: RemotePagePerf,
    resolve: RemotePageStageGuard,
}

struct RemotePageEncodeTiming {
    perf: RemotePagePerf,
    trim: RemotePageStageGuard,
}

fn lock_with_remote_page_wait<'a, T>(
    mutex: &'a Mutex<T>,
    primary: Option<&mut RemotePageStageGuard>,
    fallback: Option<&mut RemotePageStageGuard>,
) -> MutexGuard<'a, T> {
    let started = (primary.is_some() || fallback.is_some()).then(Instant::now);
    let guard = mutex.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(started) = started {
        if let Some(stage) = primary {
            stage.add_lock_wait(started);
        } else if let Some(stage) = fallback {
            stage.add_lock_wait(started);
        }
    }
    guard
}

/// フルページ要求の成果物はページであってサムネイルではない。
///
/// ローダーは表示用画像をチャネルへ送った後もサムネイルの生成を続け、その間この呼び出しは
/// 戻ってこない (チャネルを drain するのは呼び出しが返ってから) ため、**読み手は自分が
/// 受け取らない成果物の完成を待たされる**。実測 269ms / ページ、42/42 の要求が該当した。
///
/// **本体のフルスクリーンは同じ待ちを持たない** — `render_page_for_display` を直接呼び、
/// 永続キャッシュを通らないため (`app.rs` の「フルスクリーンは fs_cache (memory) のみ」)。
/// リモートも成果物を一致させる。一覧のサムネイルは一覧が要求したときに従来どおり作られる。
fn remote_page_cache_decision(
    full_page: bool,
    settings: &crate::settings::Settings,
) -> crate::thumb_loader::CacheDecision {
    if full_page {
        crate::thumb_loader::CacheDecision::without_thumbnail()
    } else {
        crate::thumb_loader::CacheDecision::from_settings(settings)
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_remote_source(
    request: crate::thumb_loader::LoadRequest,
    cache_map: Arc<RwLock<HashMap<String, crate::catalog::CacheEntry>>>,
    catalog: Arc<crate::catalog::CatalogDb>,
    thumb_px: u32,
    thumb_quality: u8,
    target_px: u32,
    cache_decision: crate::thumb_loader::CacheDecision,
    stats: Arc<Mutex<crate::stats::ThumbStats>>,
    shared_cancel: &Arc<AtomicBool>,
    zip_directory: bool,
) -> Result<RemoteDecodedSource, MediaError> {
    let (tx, rx) = mpsc::channel();
    let done = Arc::new(AtomicUsize::new(0));
    let keep_start = Arc::new(AtomicUsize::new(0));
    let keep_end = Arc::new(AtomicUsize::new(usize::MAX));
    let load_request_started = Instant::now();
    // Remote raw requests never set the folder-pin or page-adjustment fields asserted by
    // RemoteSourceDecodeIdentity, so those DB handles cannot affect this Source raster.
    crate::thumb_loader::process_load_request(
        &request,
        &cache_map,
        &tx,
        Some(&catalog),
        thumb_px,
        thumb_quality,
        target_px,
        cache_decision,
        &done,
        &stats,
        Some(shared_cancel),
        &keep_start,
        &keep_end,
        None,
        None,
        None,
    );
    let load_request_ms = load_request_started.elapsed().as_secs_f64() * 1000.0;
    drop(tx);
    let drain_started = Instant::now();
    let mut saw_canceled = false;
    let loaded = rx.into_iter().find_map(|message| {
        saw_canceled |= message.canceled;
        if !message.finalized && !message.canceled {
            message.image.map(|image| (image, message.source_dims))
        } else {
            None
        }
    });
    let drain_ms = drain_started.elapsed().as_secs_f64() * 1000.0;
    let (color_image, decoded_source_dims) = loaded.ok_or_else(|| {
        remote_source_decode_error(
            saw_canceled || shared_cancel.load(Ordering::Acquire),
            zip_directory,
        )
    })?;
    Ok(RemoteDecodedSource {
        raster: RemoteSourceRaster {
            pixels: Arc::new(color_image),
            decoded_source_dims,
        },
        load_request_ms,
        drain_ms,
    })
}

fn remote_source_decode_error(canceled: bool, zip_directory: bool) -> MediaError {
    if canceled {
        return RemoteSourceSingleFlight::participant_cancelled_error();
    }
    if zip_directory {
        return media_error(
            MediaErrorCode::NotFound,
            "ZIP 内に代表サムネイルが見つかりません",
        );
    }
    media_error(
        MediaErrorCode::RenderFailed,
        "mIV 本体でページをレンダリングできませんでした",
    )
}

fn remote_page_perf_key(address: &RemoteAddress) -> String {
    match &address.subresource {
        RemoteSubresource::File => address.path.clone(),
        RemoteSubresource::ZipEntry { entry_name } => {
            format!("{}::{entry_name}", address.path)
        }
        RemoteSubresource::ZipDirectory { prefix } => {
            format!("{}::{prefix}", address.path)
        }
        RemoteSubresource::PdfPage { page_number } => {
            crate::grid_item::pdf_page_perf_key(Path::new(&address.path), *page_number)
        }
    }
}

/// Remote の表示ページ専用 encoder。サムネイル cache の WebP 形式とは共有しない。
fn encode_remote_page_jpeg(
    image: &egui::ColorImage,
    long_side: u32,
    view_trim_bbox: Option<egui::Rect>,
) -> Option<(Vec<u8>, u32, u32)> {
    encode_remote_page_jpeg_timed(image, long_side, view_trim_bbox, None)
}

fn encode_remote_page_jpeg_timed(
    image: &egui::ColorImage,
    long_side: u32,
    view_trim_bbox: Option<egui::Rect>,
    timing: Option<RemotePageEncodeTiming>,
) -> Option<(Vec<u8>, u32, u32)> {
    let page_perf = timing.as_ref().map(|timing| timing.perf.clone());
    let mut trim_stage = timing.map(|timing| timing.trim);
    let image_width = image.size[0];
    let image_height = image.size[1];
    let (x, y, width, height) = if let Some(bbox) = view_trim_bbox {
        let rect = crate::export_crop::CropRect {
            min_x: bbox.min.x * image_width as f32,
            min_y: bbox.min.y * image_height as f32,
            max_x: bbox.max.x * image_width as f32,
            max_y: bbox.max.y * image_height as f32,
        };
        rect.pixel_bounds(image_width, image_height)
    } else {
        (0, 0, image_width, image_height)
    };
    let width_u32 = u32::try_from(width).ok()?;
    let height_u32 = u32::try_from(height).ok()?;
    let (output_width, output_height) = crate::fast_resize::aspect_accurate_fit_dimensions(
        (width_u32, height_u32),
        (long_side, long_side),
        (width_u32, height_u32),
    );
    // 縮小が要るなら結局 `DynamicImage` を組むので、不透明かどうかを調べても答えは変わらない。
    // 走査は 46 MP を 1 往復するので、要らないときは走らせない。
    let needs_resize = (output_width, output_height) != (width_u32, height_u32);
    let use_zero_copy = !needs_resize && {
        let started = Instant::now();
        let is_opaque = image.pixels.iter().all(|pixel| pixel.a() == 255);
        // 走査はこの段の中で起きているので、この段の区間として記録する。
        // 段をまたいで時間を付け替えると、段の ms が「その段にかかった時間」でなくなる。
        if let Some(stage) = trim_stage.as_mut() {
            stage.phase_from("opacity_scan", started);
        }
        is_opaque
    };

    let mut legacy_image = None;
    if !use_zero_copy {
        let source = color_image_to_dynamic_image(image)?;
        legacy_image = Some(if view_trim_bbox.is_some() {
            source.crop_imm(x as u32, y as u32, width_u32, height_u32)
        } else {
            source
        });
    }
    let trim_input = RemotePageStageMetrics::buffer(image_width, image_height, 4);
    if let Some(stage) = trim_stage.as_mut() {
        stage.finish(trim_input.with_output(width, height, 4));
    }
    drop(trim_stage);
    let mut resize_stage = page_perf
        .as_ref()
        .and_then(|perf| perf.enter(RemotePageStage::Resize));
    let resize_input = RemotePageStageMetrics::buffer(width, height, 4);
    let resized = legacy_image.as_ref().map(|image| {
        crate::fast_resize::resize_dynamic_fit(
            image,
            long_side,
            long_side,
            crate::fast_resize::Quality::Lanczos3,
        )
    });
    if let Some(stage) = resize_stage.as_mut() {
        stage.finish(resize_input.with_output(output_width as usize, output_height as usize, 4));
    }
    drop(resize_stage);
    let mut jpeg_stage = page_perf
        .as_ref()
        .and_then(|perf| perf.enter(RemotePageStage::Jpeg));
    let bytes = if use_zero_copy {
        let pitch = image_width.checked_mul(4)?;
        let start = y.checked_mul(pitch)?.checked_add(x.checked_mul(4)?)?;
        let raw = image.as_raw();
        let input = turbojpeg::Image {
            pixels: raw.get(start..)?,
            width,
            pitch,
            height,
            format: turbojpeg::PixelFormat::RGBA,
        };
        turbojpeg::compress(input, PAGE_JPEG_QUALITY, turbojpeg::Subsamp::Sub2x2)
            .ok()?
            .to_vec()
    } else {
        let rgb = resized?.to_rgb8();
        turbojpeg::compress_image(&rgb, PAGE_JPEG_QUALITY, turbojpeg::Subsamp::Sub2x2)
            .ok()?
            .to_vec()
    };
    if let Some(stage) = jpeg_stage.as_mut() {
        let bytes_per_pixel = if use_zero_copy { 4 } else { 3 };
        let mut metrics = RemotePageStageMetrics::buffer(
            output_width as usize,
            output_height as usize,
            bytes_per_pixel,
        );
        metrics.output_bytes = bytes.len() as u64;
        stage.finish_with_outcome(
            metrics,
            if use_zero_copy {
                "zero_copy"
            } else {
                "unmultiplied"
            },
        );
    }
    Some((bytes, output_width, output_height))
}

fn harmonized_remote_auto_bbox(
    side: crate::view_trim::ViewTrimSpreadSide,
    current: Option<egui::Rect>,
    partner: Option<egui::Rect>,
) -> Option<egui::Rect> {
    let (left, right) = match side {
        crate::view_trim::ViewTrimSpreadSide::Left => {
            crate::view_trim::harmonize_spread_auto_bboxes(current, partner)
        }
        crate::view_trim::ViewTrimSpreadSide::Right => {
            crate::view_trim::harmonize_spread_auto_bboxes(partner, current)
        }
    };
    match side {
        crate::view_trim::ViewTrimSpreadSide::Left => left,
        crate::view_trim::ViewTrimSpreadSide::Right => right,
    }
}

fn complete_remote_view_trim_bbox_from_partner(
    plan: &RemoteViewTrimPlan,
    current_auto_trim_bbox: Option<egui::Rect>,
    partner: RemotePartnerResult<Option<egui::Rect>>,
) -> Option<egui::Rect> {
    match (plan, partner) {
        (RemoteViewTrimPlan::Stored(bbox), _) => *bbox,
        (RemoteViewTrimPlan::AutoSingle, _) => current_auto_trim_bbox,
        (
            RemoteViewTrimPlan::AutoSpread { side, .. },
            RemotePartnerResult::Resolved(partner_auto_trim_bbox),
        ) => harmonized_remote_auto_bbox(*side, current_auto_trim_bbox, partner_auto_trim_bbox),
        (RemoteViewTrimPlan::AutoSpread { .. }, RemotePartnerResult::NotRequired) => {
            unreachable!()
        }
    }
}

fn remote_auto_trim_cache_key(
    address: &RemoteAddress,
    resolved: &ResolvedPath,
    mtime: i64,
    file_size: i64,
    target_px: u32,
) -> Result<RemoteAutoTrimCacheKey, MediaError> {
    Ok(RemoteAutoTrimCacheKey {
        page_key: crate::edit_source::page_key_for_remote(&resolved.logical, &address.subresource)
            .ok_or_else(|| media_error(MediaErrorCode::BadRequest, "表示トリム対象が不正です"))?,
        mtime,
        file_size,
        target_px,
    })
}

pub(super) struct ContainerEngine {
    settings: Arc<crate::settings::Settings>,
    listing_settings: RemoteListingSettingsSource,
    stats: Arc<Mutex<crate::stats::ThumbStats>>,
    pdf_passwords: crate::pdf_passwords::PdfPasswordStore,
    pdf_page_counts: Mutex<HashMap<PdfIdentity, u32>>,
    spread_db: Mutex<Option<crate::spread_db::SpreadDb>>,
    view_trim_db: Mutex<Option<crate::view_trim_db::ViewTrimDb>>,
    resume_reader: Option<ResumeReader>,
    adjustment_settings: AdjustmentSettingsSource,
    creative_lut_cache: Mutex<RemoteCreativeLutCache>,
    page_composite_cache: Mutex<RemoteCompositeCache>,
    auto_trim_bbox_cache: Mutex<RemoteAutoTrimCache>,
    source_single_flight: Arc<RemoteSourceSingleFlight>,
    remote_ai_native_cache: Mutex<RemoteAiNativeCache>,
    comic_stamp_cache: Mutex<HashMap<String, Option<Arc<comic_core::RgbaOverlay>>>>,
    session: Option<super::session::SessionHandle>,
}

enum AdjustmentSettingsSource {
    Live,
    #[cfg(test)]
    Snapshot(crate::settings_db::AdjustmentRenderSettings),
}

enum RemoteListingSettingsSource {
    Live,
    #[cfg(test)]
    Snapshot(crate::settings_db::RemoteListingSettings),
}

impl RemoteListingSettingsSource {
    fn load(
        &self,
        fallback: &crate::settings::Settings,
    ) -> Result<crate::settings_db::RemoteListingSettings, String> {
        match self {
            Self::Live => {
                crate::settings_db::with_db_result(|db| db.load_remote_listing_settings(fallback))
                    .map_err(|error| error.to_string())
            }
            #[cfg(test)]
            Self::Snapshot(settings) => Ok(settings.clone()),
        }
    }
}

#[derive(Clone)]
struct RemoteAdjustmentIdentity {
    page_key: String,
    location_path: PathBuf,
    compiled_book: bool,
}

#[derive(Clone)]
struct RemotePreparedComposite {
    key: RemoteCompositeCacheKey,
    params: crate::adjustment::AdjustParams,
    lut_entry: Option<crate::creative_lut::CreativeLutEntry>,
    edits: RemoteEditSnapshot,
    settings: crate::settings_db::AdjustmentRenderSettings,
}

#[derive(Clone)]
struct RemoteEditSnapshot {
    erase: Option<crate::edit_source::MaskSnapshot>,
    erase_mono_tolerance: u8,
    local_adjust: Option<Vec<local_adjust_core::LocalAdjustmentLayer>>,
    conceal: Option<crate::edit_source::MaskSnapshot>,
    conceal_preset: crate::conceal::ConcealPreset,
    comic: Vec<comic_core::AnnotationObject>,
    export_crop: Option<crate::export_crop::CropSettings>,
    fingerprint: [u8; 32],
    pre_ai_fingerprint: [u8; 32],
}

struct RemoteMaterializedEdits {
    pixels: Arc<egui::ColorImage>,
    comic: Vec<comic_core::AnnotationObject>,
    export_crop: Option<crate::export_crop::CropSettings>,
    timing: crate::edit_source::EditSourceTiming,
    used_diffusion_fallback: bool,
}

/// Canonical pixel coordinate space in which persisted crop and comic edits were authored.
///
/// `ThumbMsg::source_dims` is catalog-compatible metadata: it is original-raster pixels for
/// regular images but PDF page-box fixed-point dimensions for layout/aspect calculations. A PDF's
/// edit space must instead be page-specific and independent of the requested render resolution;
/// using the raster rendered for the current request makes the saved crop move as target_px changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StoredEditSpace {
    canonical_dims: [usize; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StoredEditSkipReason {
    PdfVectorHasNoCanonicalRaster,
    PdfCanonicalAnalysisFailed,
    PdfCanonicalRasterUnavailable,
}

impl StoredEditSkipReason {
    fn log_value(self) -> &'static str {
        match self {
            Self::PdfVectorHasNoCanonicalRaster => "pdf_vector_no_canonical_raster",
            Self::PdfCanonicalAnalysisFailed => "pdf_canonical_analysis_failed",
            Self::PdfCanonicalRasterUnavailable => "pdf_canonical_raster_unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StoredEditPipeline {
    Page,
    FinalAi,
}

impl StoredEditPipeline {
    fn log_value(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::FinalAi => "final_ai",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SkippedStoredEdits {
    pipeline: StoredEditPipeline,
    page_number: u32,
    reason: StoredEditSkipReason,
    crop: bool,
    comic_objects: usize,
}

impl SkippedStoredEdits {
    fn log_line(self) -> String {
        format!(
            "remote_ipc: stored_edit outcome=skipped pipeline={} target=pdf_page page_number={} reason={} crop={} comic_objects={}",
            self.pipeline.log_value(),
            self.page_number,
            self.reason.log_value(),
            self.crop,
            self.comic_objects,
        )
    }
}

fn record_skipped_stored_edits_with(skipped: SkippedStoredEdits, emit: impl FnOnce(String)) {
    emit(skipped.log_line());
}

fn record_skipped_stored_edits(skipped: SkippedStoredEdits) {
    record_skipped_stored_edits_with(skipped, crate::logger::log);
}

fn crop_with_stored_edit_space(
    pixels: Arc<egui::ColorImage>,
    crop: Option<crate::export_crop::CropSettings>,
    stored_edit_space: Option<StoredEditSpace>,
) -> Result<Arc<egui::ColorImage>, String> {
    let (Some(crop), Some(stored_edit_space)) = (crop, stored_edit_space) else {
        // An unavailable canonical space is a deliberate skip, not permission to reinterpret the
        // saved rectangle in the current request raster. Return the full page unchanged.
        return Ok(pixels);
    };
    let rect = stored_edit_space.crop_rect(crop, pixels.size);
    crate::export_crop::crop_color_image(&pixels, rect).map(Arc::new)
}

impl StoredEditSpace {
    fn for_remote_source(
        subresource: &RemoteSubresource,
        rendered_raster_dims: [usize; 2],
        decoded_layout_or_source_dims: Option<[usize; 2]>,
        pdf_canonical_raster_dims: Option<[usize; 2]>,
    ) -> Option<Self> {
        let canonical_dims = match subresource {
            // Never fall back to rendered_raster_dims for PDF: that value changes with target_px.
            // Vector pages have no native pixel space, so their saved absolute edits must fail
            // explicitly instead of silently moving or being applied in the wrong coordinates.
            RemoteSubresource::PdfPage { .. } => pdf_canonical_raster_dims?,
            _ => decoded_layout_or_source_dims.unwrap_or(rendered_raster_dims),
        };
        Some(Self { canonical_dims })
    }

    fn comic_composite(
        self,
        base: &Arc<egui::ColorImage>,
        objects: &[comic_core::AnnotationObject],
        fonts: &comic_core::FontSet,
        stamp_cache: &mut HashMap<String, Option<Arc<comic_core::RgbaOverlay>>>,
        cancel: &AtomicBool,
    ) -> Arc<egui::ColorImage> {
        crate::edit_source::comic_composite(
            base,
            objects,
            self.canonical_dims,
            fonts,
            stamp_cache,
            cancel,
        )
    }

    fn crop_rect(
        self,
        settings: crate::export_crop::CropSettings,
        pixel_dims: [usize; 2],
    ) -> crate::export_crop::CropRect {
        crate::edit_source::export_crop_rect_for_pixels(settings, self.canonical_dims, pixel_dims)
    }
}

#[derive(Clone, PartialEq)]
struct RemoteCompositeCacheKey {
    page_key: String,
    mtime: i64,
    file_size: i64,
    target_px: u32,
    rotation: crate::rotation_db::Rotation,
    params: crate::adjustment::AdjustParams,
    lut_entry: Option<crate::creative_lut::CreativeLutEntry>,
    edit_fingerprint: [u8; 32],
}

struct RemoteCompositeCacheEntry {
    key: RemoteCompositeCacheKey,
    pixels: Arc<egui::ColorImage>,
    bytes: usize,
}

/// 本体の `(load_seq, pixels_ptr)` に相当する remote raw-raster identity。
/// remote は page slot を保持しないため、既存 decode/composite cache と同じ source stamp と
/// decode 上限で、同じ元 raster の再要求だけを再利用する。
#[derive(Clone, PartialEq, Eq)]
struct RemoteAutoTrimCacheKey {
    page_key: String,
    mtime: i64,
    file_size: i64,
    target_px: u32,
}

struct RemoteAutoTrimCacheEntry {
    key: RemoteAutoTrimCacheKey,
    bbox: Option<egui::Rect>,
}

/// Exact identity of the raw raster produced by the remote Source stage.
///
/// Keep this tied to `LoadRequest`, not to the later page-composite or auto-trim identities:
/// `AutoTrimReference` and `CompositedPageWithAutoTrim` intentionally differ after Source while
/// producing the same raw raster here.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct RemoteSourceDecodeIdentity {
    path: PathBuf,
    mtime: i64,
    file_size: i64,
    pdf_page: Option<u32>,
    zip_entry: Option<String>,
    zip_dir_prefix: Option<String>,
    cache_key_override: Option<String>,
    target_px: u32,
    full_page: bool,
}

impl RemoteSourceDecodeIdentity {
    fn from_load_request(
        request: &crate::thumb_loader::LoadRequest,
        target_px: u32,
        full_page: bool,
    ) -> Self {
        debug_assert_eq!(request.source_policy.bypasses_cache(), full_page);
        debug_assert!(request.relative_page_provenance.is_none());
        debug_assert!(request.edit_preview_key.is_none());
        debug_assert!(!request.edit_preview_validate_container);
        debug_assert!(request.pinned_page_adjustment_key.is_none());
        debug_assert_eq!(request.folder_thumb_depth, 0);
        debug_assert!(request.resolve_override.is_none());
        debug_assert!(request.pinned_only.is_none());
        debug_assert!(!request.force_cache);
        debug_assert_eq!(request.input_seq, 0);
        debug_assert_eq!(request.items_gen, 0);
        debug_assert_eq!(request.context_epoch, 0);
        // `resolve_override` being None is not on its own what keeps the folder-representative
        // branch out of reach: `process_load_request` also enters it when `cache_key_override`
        // starts with `CACHE_KEY_FOLDER`. That branch is the only reader of `folder_thumb_sort`
        // (omitted from this identity) and of the pin DB (passed as None by `decode_remote_source`),
        // so a future remote folder thumbnail would silently break both. Fail loudly instead.
        debug_assert!(
            !request
                .cache_key_override
                .as_deref()
                .is_some_and(|key| key.starts_with(crate::thumb_loader::CACHE_KEY_FOLDER))
        );

        // Included below: every varying LoadRequest field that can select or decode different
        // bytes in this remote path. The omitted fields are deliberately non-identities:
        // - idx routes ThumbMsg; input_seq only correlates perf events; items_gen only lets the
        //   UI reject stale ThumbMsg values. None changes pixels.
        // - priority changes scheduling, never pixels.
        // - source_policy is SourceOnly exactly for `full_page`, which is included (and also separates Thumbnail's
        //   cache_decision from full-page requests).
        // - pdf_password comes from this ContainerEngine's immutable password-store snapshot, so
        //   it cannot vary for the same path within this coordinator and is not retained as a
        //   plaintext cache key.
        // - relative_page_provenance is always None: remote paths were already resolved and
        //   guarded before this request is assembled.
        // - edit_preview_key and edit_preview_validate_container are inactive because remote
        //   Source deliberately loads the raw raster; edits are composed after Source.
        // - pinned_page_adjustment_key is None for the same reason, and therefore adjustment_db
        //   cannot affect this raster.
        // - folder_thumb_depth is zero, resolve_override and pinned_only are None, and force_cache
        //   is false in this remote path, as asserted above.
        // - context_epoch is the documented zero sentinel for this background path.
        // - folder_thumb_sort is populated for ZipDirectory but is only read by the
        //   folder-representative branch, which this path never enters: it needs either a
        //   ResolveStrategy override or a CACHE_KEY_FOLDER cache key, and remote Source produces
        //   neither (its keys are zipthumb:/pdfthumb:/zipdir:). Both are asserted above.
        Self {
            path: request.path.clone(),
            mtime: request.mtime,
            file_size: request.file_size,
            pdf_page: request.pdf_page,
            zip_entry: request.zip_entry.clone(),
            zip_dir_prefix: request.zip_dir_prefix.clone(),
            cache_key_override: request.cache_key_override.clone(),
            target_px,
            full_page,
        }
    }
}

#[derive(Clone, Debug)]
struct RemoteSourceRaster {
    pixels: Arc<egui::ColorImage>,
    decoded_source_dims: Option<(u32, u32)>,
}

#[derive(Debug)]
struct RemoteDecodedSource {
    raster: RemoteSourceRaster,
    load_request_ms: f64,
    drain_ms: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteSourceLoadOutcome {
    Decoded,
    Joined,
    Handoff,
}

impl RemoteSourceLoadOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Decoded => "decoded",
            Self::Joined => "joined",
            Self::Handoff => "handoff",
        }
    }
}

#[derive(Debug)]
struct RemoteSourceLoad {
    raster: RemoteSourceRaster,
    outcome: RemoteSourceLoadOutcome,
    singleflight_wait_ms: f64,
    load_request_ms: f64,
    drain_ms: f64,
}

enum RemoteSourceEntryStatus {
    InFlight,
    Ready(Arc<RemoteDecodedSource>),
    Broken(MediaError),
}

struct RemoteSourceEntryState {
    status: RemoteSourceEntryStatus,
    participants: usize,
}

struct RemoteSourceEntry {
    state: Mutex<RemoteSourceEntryState>,
    ready: Condvar,
    shared_cancel: Arc<AtomicBool>,
}

impl RemoteSourceEntry {
    fn new() -> Self {
        Self {
            state: Mutex::new(RemoteSourceEntryState {
                status: RemoteSourceEntryStatus::InFlight,
                participants: 1,
            }),
            ready: Condvar::new(),
            shared_cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

struct RemoteSourceParticipant {
    entry: Arc<RemoteSourceEntry>,
}

impl Drop for RemoteSourceParticipant {
    fn drop(&mut self) {
        let mut state = self
            .entry
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        debug_assert_ne!(state.participants, 0);
        state.participants = state.participants.saturating_sub(1);
        if state.participants == 0 && matches!(state.status, RemoteSourceEntryStatus::InFlight) {
            self.entry.shared_cancel.store(true, Ordering::Release);
        }
    }
}

struct RemoteSourceHandoff {
    identity: RemoteSourceDecodeIdentity,
    raster: RemoteSourceRaster,
}

#[derive(Default)]
struct RemoteSourceSingleFlightState {
    in_flight: HashMap<RemoteSourceDecodeIdentity, Arc<RemoteSourceEntry>>,
    handoff: Option<RemoteSourceHandoff>,
}

#[derive(Default)]
struct RemoteSourceSingleFlight {
    state: Mutex<RemoteSourceSingleFlightState>,
    #[cfg(test)]
    changed: Condvar,
}

enum RemoteSourceAcquire {
    Handoff(RemoteSourceRaster),
    Entry {
        entry: Arc<RemoteSourceEntry>,
        owner: bool,
    },
}

enum RemoteSourceWait {
    Ready(Arc<RemoteDecodedSource>),
    Broken(MediaError),
    Cancelled,
}

const REMOTE_SOURCE_CANCEL_POLL: Duration = Duration::from_millis(10);

#[cfg(debug_assertions)]
thread_local! {
    static REMOTE_SOURCE_DECODE_OWNER: RefCell<Option<RemoteSourceDecodeIdentity>> = const {
        RefCell::new(None)
    };
}

struct RemoteSourceDecodeOwnerScope;

impl RemoteSourceDecodeOwnerScope {
    fn enter(identity: RemoteSourceDecodeIdentity) -> Self {
        #[cfg(debug_assertions)]
        REMOTE_SOURCE_DECODE_OWNER.with(|owner| {
            let previous = owner.replace(Some(identity));
            debug_assert!(previous.is_none());
        });
        #[cfg(not(debug_assertions))]
        let _ = identity;
        Self
    }
}

impl Drop for RemoteSourceDecodeOwnerScope {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        REMOTE_SOURCE_DECODE_OWNER.with(|owner| {
            owner.replace(None);
        });
    }
}

impl RemoteSourceSingleFlight {
    fn participant_cancelled_error() -> MediaError {
        media_error(
            MediaErrorCode::Busy,
            "先読みは新しいページ要求に置き換えられました",
        )
    }

    fn debug_assert_can_wait(identity: &RemoteSourceDecodeIdentity) {
        // Source must finish before Trim requests a partner. Otherwise two spread jobs can own
        // A/B respectively and wait B/A, so reject that structural deadlock in debug builds.
        #[cfg(debug_assertions)]
        REMOTE_SOURCE_DECODE_OWNER.with(|owner| {
            if let Some(owned) = owner.borrow().as_ref() {
                debug_assert_eq!(owned, identity);
            }
        });
        #[cfg(not(debug_assertions))]
        let _ = identity;
    }

    fn acquire(&self, identity: &RemoteSourceDecodeIdentity) -> RemoteSourceAcquire {
        Self::debug_assert_can_wait(identity);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state
            .handoff
            .as_ref()
            .is_some_and(|handoff| handoff.identity == *identity)
        {
            return RemoteSourceAcquire::Handoff(state.handoff.as_ref().unwrap().raster.clone());
        }
        // This is a one-slot hand-off, not a cache. Any different identity releases it now.
        state.handoff = None;
        if let Some(entry) = state.in_flight.get(identity).cloned() {
            let mut entry_state = entry
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            entry_state.participants = entry_state.participants.saturating_add(1);
            drop(entry_state);
            #[cfg(test)]
            self.changed.notify_all();
            return RemoteSourceAcquire::Entry {
                entry,
                owner: false,
            };
        }
        let entry = Arc::new(RemoteSourceEntry::new());
        state.in_flight.insert(identity.clone(), Arc::clone(&entry));
        #[cfg(test)]
        self.changed.notify_all();
        RemoteSourceAcquire::Entry { entry, owner: true }
    }

    fn wait_for_entry(
        entry: &RemoteSourceEntry,
        participant_cancel: &AtomicBool,
    ) -> RemoteSourceWait {
        let mut state = entry
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            if participant_cancel.load(Ordering::Acquire) {
                return RemoteSourceWait::Cancelled;
            }
            match &state.status {
                RemoteSourceEntryStatus::Ready(decoded) => {
                    return RemoteSourceWait::Ready(Arc::clone(decoded));
                }
                RemoteSourceEntryStatus::Broken(error) => {
                    return RemoteSourceWait::Broken(error.clone());
                }
                RemoteSourceEntryStatus::InFlight => {}
            }
            // AtomicBool has no wake primitive. This bounded Condvar wait only observes a
            // participant cancellation; completion/failure always notifies and never times out.
            let (next, _) = entry
                .ready
                .wait_timeout(state, REMOTE_SOURCE_CANCEL_POLL)
                .unwrap_or_else(|error| error.into_inner());
            state = next;
        }
    }

    fn finish_decode(
        &self,
        identity: RemoteSourceDecodeIdentity,
        entry: Arc<RemoteSourceEntry>,
        mut result: Result<RemoteDecodedSource, MediaError>,
    ) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let mut entry_state = entry
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let is_current = state
            .in_flight
            .get(&identity)
            .is_some_and(|current| Arc::ptr_eq(current, &entry));
        if is_current {
            state.in_flight.remove(&identity);
        }
        if entry.shared_cancel.load(Ordering::Acquire) {
            result = Err(media_error(
                MediaErrorCode::Busy,
                "先読みは新しいページ要求に置き換えられました",
            ));
        }
        match result {
            Ok(decoded) => {
                let decoded = Arc::new(decoded);
                if is_current && entry_state.participants != 0 {
                    state.handoff = Some(RemoteSourceHandoff {
                        identity,
                        raster: decoded.raster.clone(),
                    });
                }
                entry_state.status = RemoteSourceEntryStatus::Ready(decoded);
            }
            Err(error) => entry_state.status = RemoteSourceEntryStatus::Broken(error),
        }
        drop(entry_state);
        drop(state);
        entry.ready.notify_all();
        #[cfg(test)]
        self.changed.notify_all();
    }

    fn start_decode<F>(
        self: &Arc<Self>,
        identity: RemoteSourceDecodeIdentity,
        entry: Arc<RemoteSourceEntry>,
        decode: F,
    ) where
        F: FnOnce(&Arc<AtomicBool>) -> Result<RemoteDecodedSource, MediaError> + Send + 'static,
    {
        let flight = Arc::clone(self);
        let worker_identity = identity.clone();
        let worker_entry = Arc::clone(&entry);
        let shared_cancel = Arc::clone(&entry.shared_cancel);
        let spawn = std::thread::Builder::new()
            .name("remote-source-decode".to_string())
            .spawn(move || {
                let _owner = RemoteSourceDecodeOwnerScope::enter(worker_identity.clone());
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    decode(&shared_cancel)
                }))
                .unwrap_or_else(|_| {
                    Err(media_error(
                        MediaErrorCode::Internal,
                        "共有デコードが失敗しました",
                    ))
                });
                flight.finish_decode(worker_identity, worker_entry, result);
            });
        if spawn.is_err() {
            self.finish_decode(
                identity,
                entry,
                Err(media_error(
                    MediaErrorCode::Internal,
                    "共有デコードを開始できません",
                )),
            );
        }
    }

    fn completed_load(
        decoded: &RemoteDecodedSource,
        outcome: RemoteSourceLoadOutcome,
        wait_ms: f64,
    ) -> RemoteSourceLoad {
        let decoded_here = outcome == RemoteSourceLoadOutcome::Decoded;
        RemoteSourceLoad {
            raster: decoded.raster.clone(),
            outcome,
            singleflight_wait_ms: wait_ms,
            load_request_ms: if decoded_here {
                decoded.load_request_ms
            } else {
                0.0
            },
            drain_ms: if decoded_here { decoded.drain_ms } else { 0.0 },
        }
    }

    fn handoff_load(raster: RemoteSourceRaster, wait_ms: f64) -> RemoteSourceLoad {
        RemoteSourceLoad {
            raster,
            outcome: RemoteSourceLoadOutcome::Handoff,
            singleflight_wait_ms: wait_ms,
            load_request_ms: 0.0,
            drain_ms: 0.0,
        }
    }

    fn load<F>(
        self: &Arc<Self>,
        identity: RemoteSourceDecodeIdentity,
        participant_cancel: Arc<AtomicBool>,
        decode: F,
    ) -> Result<RemoteSourceLoad, MediaError>
    where
        F: FnOnce(&Arc<AtomicBool>) -> Result<RemoteDecodedSource, MediaError> + Send + 'static,
    {
        let mut decode = Some(decode);
        let mut wait_ms = 0.0;
        loop {
            if participant_cancel.load(Ordering::Acquire) {
                return Err(Self::participant_cancelled_error());
            }
            match self.acquire(&identity) {
                RemoteSourceAcquire::Handoff(raster) => {
                    if participant_cancel.load(Ordering::Acquire) {
                        return Err(Self::participant_cancelled_error());
                    }
                    return Ok(Self::handoff_load(raster, wait_ms));
                }
                RemoteSourceAcquire::Entry { entry, owner } => {
                    if let Some(loaded) = self.wait_for_attempt(
                        &identity,
                        entry,
                        owner,
                        &participant_cancel,
                        &mut decode,
                        &mut wait_ms,
                    )? {
                        return Ok(loaded);
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn wait_for_attempt<F>(
        self: &Arc<Self>,
        identity: &RemoteSourceDecodeIdentity,
        entry: Arc<RemoteSourceEntry>,
        owner: bool,
        participant_cancel: &Arc<AtomicBool>,
        decode: &mut Option<F>,
        wait_ms: &mut f64,
    ) -> Result<Option<RemoteSourceLoad>, MediaError>
    where
        F: FnOnce(&Arc<AtomicBool>) -> Result<RemoteDecodedSource, MediaError> + Send + 'static,
    {
        let participant = RemoteSourceParticipant {
            entry: Arc::clone(&entry),
        };
        if owner {
            let factory = decode.take().unwrap();
            self.start_decode(identity.clone(), Arc::clone(&entry), factory);
        }
        let wait_started = (!owner).then(Instant::now);
        let waited = Self::wait_for_entry(&entry, participant_cancel);
        if let Some(started) = wait_started {
            *wait_ms += started.elapsed().as_secs_f64() * 1000.0;
        }
        drop(participant);
        match waited {
            RemoteSourceWait::Ready(decoded) => {
                if participant_cancel.load(Ordering::Acquire) {
                    return Err(Self::participant_cancelled_error());
                }
                let outcome = if owner {
                    RemoteSourceLoadOutcome::Decoded
                } else {
                    RemoteSourceLoadOutcome::Joined
                };
                Ok(Some(Self::completed_load(&decoded, outcome, *wait_ms)))
            }
            RemoteSourceWait::Broken(error) if owner => Err(error),
            // The broken entry is removed before wakeup. One waiter retries as owner; the rest
            // join it, using their still-unconsumed factories and no timeout fallback.
            RemoteSourceWait::Broken(_) => Ok(None),
            RemoteSourceWait::Cancelled => Err(Self::participant_cancelled_error()),
        }
    }
}

#[derive(Clone, PartialEq)]
struct RemoteAiNativeCacheKey {
    page_key: String,
    mtime: i64,
    file_size: i64,
    source_size: [usize; 2],
    pre_ai_params: crate::adjustment::AdjustParams,
    pre_ai_edit_fingerprint: [u8; 32],
    ai_feature_mode: crate::settings::AiFeatureMode,
    ai_upscale_limit: crate::ai::upscale::AiProcessSizeLimit,
    ai_denoise_limit: crate::ai::upscale::AiProcessSizeLimit,
    ai_backend: Option<String>,
    background_mode: u8,
    pipeline_schema: u32,
    model_epoch: [u8; 32],
}

#[derive(Clone, PartialEq)]
struct RemoteAiResultIdentity {
    composite: RemoteCompositeCacheKey,
    ai_feature_mode: crate::settings::AiFeatureMode,
    ai_upscale_limit: crate::ai::upscale::AiProcessSizeLimit,
    ai_denoise_limit: crate::ai::upscale::AiProcessSizeLimit,
    ai_backend: Option<String>,
    retained_max_entries: usize,
    retained_max_mib: u64,
    background_mode: u8,
}

impl RemoteAiResultIdentity {
    fn from_prepared(prepared: &RemotePreparedComposite, background_mode: u8) -> Self {
        Self {
            composite: prepared.key.clone(),
            ai_feature_mode: prepared.settings.ai_feature_mode,
            ai_upscale_limit: prepared.settings.ai_upscale_limit,
            ai_denoise_limit: prepared.settings.ai_denoise_limit,
            ai_backend: prepared.settings.ai_backend.clone(),
            retained_max_entries: prepared.settings.retained_final_ai_cache_max_entries,
            retained_max_mib: prepared.settings.retained_final_ai_cache_max_mib,
            background_mode,
        }
    }
}

struct RemoteAiNativeCacheEntry {
    key: RemoteAiNativeCacheKey,
    pixels: Arc<egui::ColorImage>,
    used_upscale: bool,
    bytes: u64,
}

#[derive(Default)]
struct RemoteAiNativeCache {
    entries: VecDeque<RemoteAiNativeCacheEntry>,
    bytes: u64,
}

impl RemoteAiNativeCache {
    fn enforce_budget(&mut self, max_entries: usize, max_bytes: u64) {
        if max_entries == 0 || max_bytes == 0 {
            self.entries.clear();
            self.bytes = 0;
            return;
        }
        while self.entries.len() > max_entries || self.bytes > max_bytes {
            let Some(removed) = self.entries.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(removed.bytes);
        }
    }

    fn get(&mut self, key: &RemoteAiNativeCacheKey) -> Option<(Arc<egui::ColorImage>, bool)> {
        let position = self.entries.iter().position(|entry| entry.key == *key)?;
        let entry = self.entries.remove(position)?;
        let result = (Arc::clone(&entry.pixels), entry.used_upscale);
        self.entries.push_back(entry);
        Some(result)
    }

    fn insert(
        &mut self,
        key: RemoteAiNativeCacheKey,
        pixels: Arc<egui::ColorImage>,
        used_upscale: bool,
        max_entries: usize,
        max_bytes: u64,
    ) {
        if max_entries == 0 || max_bytes == 0 {
            self.enforce_budget(max_entries, max_bytes);
            return;
        }
        let bytes = pixels.as_raw().len() as u64;
        if bytes > max_bytes {
            return;
        }
        if let Some(position) = self.entries.iter().position(|entry| entry.key == key)
            && let Some(previous) = self.entries.remove(position)
        {
            self.bytes = self.bytes.saturating_sub(previous.bytes);
        }
        self.entries.push_back(RemoteAiNativeCacheEntry {
            key,
            pixels,
            used_upscale,
            bytes,
        });
        self.bytes = self.bytes.saturating_add(bytes);
        self.enforce_budget(max_entries, max_bytes);
    }
}

#[derive(Default)]
struct RemoteCompositeCache {
    entries: VecDeque<RemoteCompositeCacheEntry>,
    bytes: usize,
}

impl RemoteCompositeCache {
    fn get(&mut self, key: &RemoteCompositeCacheKey) -> Option<Arc<egui::ColorImage>> {
        let position = self.entries.iter().position(|entry| entry.key == *key)?;
        let entry = self.entries.remove(position)?;
        let pixels = Arc::clone(&entry.pixels);
        self.entries.push_back(entry);
        Some(pixels)
    }

    fn insert(&mut self, key: RemoteCompositeCacheKey, pixels: Arc<egui::ColorImage>) {
        let bytes = pixels
            .pixels
            .len()
            .saturating_mul(std::mem::size_of::<egui::Color32>());
        if bytes > REMOTE_COMPOSITE_CACHE_BYTES {
            return;
        }
        if let Some(position) = self.entries.iter().position(|entry| entry.key == key)
            && let Some(entry) = self.entries.remove(position)
        {
            self.bytes = self.bytes.saturating_sub(entry.bytes);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.entries
            .push_back(RemoteCompositeCacheEntry { key, pixels, bytes });
        while self.entries.len() > REMOTE_COMPOSITE_CACHE_ENTRIES
            || self.bytes > REMOTE_COMPOSITE_CACHE_BYTES
        {
            let Some(entry) = self.entries.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(entry.bytes);
        }
    }
}

#[derive(Default)]
struct RemoteAutoTrimCache {
    entries: VecDeque<RemoteAutoTrimCacheEntry>,
}

impl RemoteAutoTrimCache {
    /// 外側の `Option` は cache hit、内側は「余白なし」という有効な検出結果を表す。
    fn get(&mut self, key: &RemoteAutoTrimCacheKey) -> Option<Option<egui::Rect>> {
        let position = self.entries.iter().position(|entry| entry.key == *key)?;
        let entry = self.entries.remove(position)?;
        let bbox = entry.bbox;
        self.entries.push_back(entry);
        Some(bbox)
    }

    fn insert(&mut self, key: RemoteAutoTrimCacheKey, bbox: Option<egui::Rect>) {
        if let Some(position) = self.entries.iter().position(|entry| entry.key == key) {
            self.entries.remove(position);
        }
        self.entries
            .push_back(RemoteAutoTrimCacheEntry { key, bbox });
        while self.entries.len() > REMOTE_AUTO_TRIM_CACHE_ENTRIES {
            self.entries.pop_front();
        }
    }
}

#[derive(Default)]
struct RemoteCreativeLutCache {
    entries: VecDeque<(
        crate::creative_lut::CreativeLutEntry,
        crate::creative_lut::SharedCreativeLut,
    )>,
}

impl RemoteCreativeLutCache {
    fn resolve(
        &mut self,
        entry: &crate::creative_lut::CreativeLutEntry,
    ) -> Result<crate::creative_lut::SharedCreativeLut, String> {
        if let Some(position) = self.entries.iter().position(|(cached, _)| cached == entry) {
            let cached = self.entries.remove(position).expect("position exists");
            let lut = Arc::clone(&cached.1);
            self.entries.push_back(cached);
            return Ok(lut);
        }
        self.entries.retain(|(cached, _)| cached.id != entry.id);
        let lut = crate::creative_lut::load_creative_lut_entry(entry)?;
        self.entries.push_back((entry.clone(), Arc::clone(&lut)));
        while self.entries.len() > REMOTE_LUT_CACHE_ENTRIES {
            self.entries.pop_front();
        }
        Ok(lut)
    }
}

enum ResumeReader {
    Session(super::session::SessionHandle),
    #[cfg(test)]
    Error(super::session::UiReadError),
}

impl ResumeReader {
    fn read_book_resume(&self, path: &Path) -> Result<Option<usize>, super::session::UiReadError> {
        match self {
            Self::Session(session) => session.read_book_resume(path),
            #[cfg(test)]
            Self::Error(error) => Err(*error),
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct PdfIdentity {
    path: std::path::PathBuf,
    mtime: i64,
    file_size: u64,
}

struct LoadedImage {
    pixels: Arc<egui::ColorImage>,
    auto_trim_bbox: Option<egui::Rect>,
    identity: RemoteAddress,
}

#[derive(Clone, Copy)]
enum RemoteImageLoadKind {
    Thumbnail,
    CompositedPage,
    CompositedPageWithAutoTrim,
    AutoTrimReference,
}

impl RemoteImageLoadKind {
    fn full_page(self) -> bool {
        !matches!(self, Self::Thumbnail)
    }

    fn composes_page(self) -> bool {
        matches!(
            self,
            Self::CompositedPage | Self::CompositedPageWithAutoTrim
        )
    }

    fn detects_auto_trim(self) -> bool {
        matches!(
            self,
            Self::CompositedPageWithAutoTrim | Self::AutoTrimReference
        )
    }
}

enum RemoteViewTrimPlan {
    Stored(Option<egui::Rect>),
    AutoSingle,
    AutoSpread {
        side: crate::view_trim::ViewTrimSpreadSide,
        partner: RemoteAddress,
    },
}

impl RemoteViewTrimPlan {
    fn requires_auto_detection(&self) -> bool {
        matches!(self, Self::AutoSingle | Self::AutoSpread { .. })
    }

    fn spread_partner(&self) -> Option<&RemoteAddress> {
        match self {
            Self::AutoSpread { partner, .. } => Some(partner),
            Self::Stored(_) | Self::AutoSingle => None,
        }
    }
}

enum RemotePartnerStart<T, I> {
    NotRequired,
    Cached(T),
    Resolve(I),
}

enum RemotePartnerResult<T> {
    NotRequired,
    Resolved(T),
}

enum ScopedRemotePartner<'scope, T, E> {
    NotRequired,
    Ready(T),
    Pending {
        participant_cancel: Arc<AtomicBool>,
        handle: std::thread::ScopedJoinHandle<'scope, Result<T, E>>,
    },
}

impl<T, E> ScopedRemotePartner<'_, T, E> {
    fn cancel(&self) {
        if let Self::Pending {
            participant_cancel, ..
        } = self
        {
            participant_cancel.store(true, Ordering::Release);
        }
    }

    fn collect(self) -> Result<RemotePartnerResult<T>, E> {
        match self {
            Self::NotRequired => Ok(RemotePartnerResult::NotRequired),
            Self::Ready(value) => Ok(RemotePartnerResult::Resolved(value)),
            Self::Pending { handle, .. } => match handle.join() {
                Ok(result) => result.map(RemotePartnerResult::Resolved),
                Err(payload) => std::panic::resume_unwind(payload),
            },
        }
    }
}

fn with_scoped_remote_partner<T, I, S, R, E, Resolve, Source, Finish>(
    start: RemotePartnerStart<T, I>,
    resolve: Resolve,
    source: Source,
    finish: Finish,
) -> Result<R, E>
where
    T: Send,
    I: Send,
    E: Send,
    Resolve: FnOnce(I, Arc<AtomicBool>) -> Result<T, E> + Send,
    Source: FnOnce() -> Result<S, E>,
    Finish: for<'scope> FnOnce(S, ScopedRemotePartner<'scope, T, E>) -> Result<R, E>,
{
    std::thread::scope(|scope| {
        let partner = match start {
            RemotePartnerStart::NotRequired => ScopedRemotePartner::NotRequired,
            RemotePartnerStart::Cached(value) => ScopedRemotePartner::Ready(value),
            RemotePartnerStart::Resolve(input) => {
                let participant_cancel = Arc::new(AtomicBool::new(false));
                let worker_cancel = Arc::clone(&participant_cancel);
                let handle = scope.spawn(move || resolve(input, worker_cancel));
                ScopedRemotePartner::Pending {
                    participant_cancel,
                    handle,
                }
            }
        };
        let source = match source() {
            Ok(source) => source,
            Err(error) => {
                partner.cancel();
                return Err(error);
            }
        };
        finish(source, partner)
    })
}

struct RemotePartnerAutoTrimRequest {
    address: RemoteAddress,
    resolved: ResolvedPath,
}

struct SpreadPayload {
    configured: RemoteSpreadMode,
    effective: RemoteSpreadMode,
    reading_direction: RemoteReadingDirection,
    image_count: usize,
    video_count: usize,
    other_count: usize,
    groups: Vec<PageGroup>,
}

struct ValidatedPageContext {
    page_index: u32,
    page_number: u32,
    page_count: u32,
    record_history: bool,
    record_resume: bool,
    bookmark_supported: bool,
}

struct PreparedZipBookmarkList {
    resolved: ResolvedPath,
    tree: crate::zip_tree::ZipTree,
}

struct RecomputedFolderListing {
    items: Vec<crate::grid_item::GridItem>,
    metas: Vec<Option<(i64, i64)>>,
    video_thumb_overrides: Vec<(std::path::PathBuf, std::path::PathBuf)>,
    scan_ms: f64,
    materialize_ms: f64,
    image_only: bool,
    compiled: bool,
    sort_order: crate::settings::SortOrder,
    thumb_aspect_height_ratio: f64,
    sort_locked: bool,
    auto_fullscreen_image_folders_enabled: bool,
}
fn folder_thumb_aspect_height_ratio(settings: &crate::settings::Settings, folder: &Path) -> f64 {
    let aspect = if settings.thumb_aspect_auto {
        crate::auto_aspect_cache::AutoAspectCacheDb::get_read_only(folder)
            .map(|entry| entry.aspect)
            .unwrap_or(crate::settings::ThumbAspect::Square)
    } else {
        settings.thumb_aspect
    };
    f64::from(aspect.height_ratio())
}

/// カタログに行が無いページの寸法を、**ヘッダだけ読んで**求める。
///
/// サムネイルキャッシュ方針が既定の `Auto` だと、**速くて小さい画像が並ぶフォルダには
/// カタログ行が 1 つも作られない**。横長判定をカタログだけに頼っていたため、その種の
/// フォルダでは全ページが「横長ではない」と判定され、見開きの単独表示も横長分割も
/// まったく効かなかった (2026-08-26 に利用者報告。分割を選んでも横長のまま)。
///
/// **表示の仕様がキャッシュ方針で変わってはいけない**ので、方針に依存しない読み取りを
/// 置く。画素は読まない (`into_dimensions` はヘッダのみ)。
///
/// 横長判定を作る場所はコンテナと横断コレクションの 2 つある。**どちらもここを通すこと。**
/// 片方だけ直すと、同じ本がフォルダから開けば分割され、レーティング一覧から開けば
/// 分割されない、という食い違いになる。
///
/// ZIP / PDF ページはここでは補わない。書庫は 1 エントリごとに展開が要り、PDF ページ寸法は
/// worker への往復が要るので、開くたびに全ページ分を払うわけにいかない。どちらも既定
/// (`cache_zip_always` / `cache_pdf_always`) でカタログが作られるため実害はそれらを切った
/// 場合に限られる。**黙って効かなくならないよう、寸法不明の件数はログへ残す。**
/// カタログに寸法が無い PDF ページが 1 つでもあるか。
///
/// **無いなら worker へ問い合わせない。**既にサムネイルを作った PDF に追加費用を出さない
/// ための条件で、費用の有無がここだけで決まるようにしてある。
fn pdf_page_sizes_needed(
    items: &[crate::grid_item::GridItem],
    cached: &std::collections::HashMap<String, Option<(u32, u32)>>,
) -> bool {
    items.iter().any(|item| match item {
        crate::grid_item::GridItem::PdfPage { page_num, .. } => {
            !cached.contains_key(&crate::grid_item::pdf_page_cache_key(*page_num))
        }
        _ => false,
    })
}

pub(super) fn page_dims_without_catalog(item: &crate::grid_item::GridItem) -> Option<(u32, u32)> {
    match item {
        crate::grid_item::GridItem::Image(path) => image::ImageReader::open(path)
            .ok()?
            .with_guessed_format()
            .ok()?
            .into_dimensions()
            .ok(),
        _ => None,
    }
}

impl ContainerEngine {
    #[cfg(test)]
    pub(super) fn new(settings: crate::settings::Settings) -> Self {
        let adjustment_settings = AdjustmentSettingsSource::Snapshot(
            crate::settings_db::AdjustmentRenderSettings::from_settings(&settings),
        );
        let listing_settings = RemoteListingSettingsSource::Snapshot(
            crate::settings_db::RemoteListingSettings::from_settings(&settings),
        );
        Self::new_inner(settings, None, None, adjustment_settings, listing_settings)
    }

    pub(super) fn new_with_session(
        settings: crate::settings::Settings,
        session: super::session::SessionHandle,
    ) -> Self {
        Self::new_inner(
            settings,
            Some(ResumeReader::Session(session.clone())),
            Some(session),
            AdjustmentSettingsSource::Live,
            RemoteListingSettingsSource::Live,
        )
    }

    #[cfg(test)]
    fn new_with_resume_error(
        settings: crate::settings::Settings,
        error: super::session::UiReadError,
    ) -> Self {
        let adjustment_settings = AdjustmentSettingsSource::Snapshot(
            crate::settings_db::AdjustmentRenderSettings::from_settings(&settings),
        );
        let listing_settings = RemoteListingSettingsSource::Snapshot(
            crate::settings_db::RemoteListingSettings::from_settings(&settings),
        );
        Self::new_inner(
            settings,
            Some(ResumeReader::Error(error)),
            None,
            adjustment_settings,
            listing_settings,
        )
    }

    fn new_inner(
        settings: crate::settings::Settings,
        resume_reader: Option<ResumeReader>,
        session: Option<super::session::SessionHandle>,
        adjustment_settings: AdjustmentSettingsSource,
        listing_settings: RemoteListingSettingsSource,
    ) -> Self {
        let spread_db_path = crate::data_dir::get().join("spread.db");
        let spread_db =
            match crate::spread_db::SpreadDb::open_existing_read_only_at(&spread_db_path) {
                Ok(db) => db,
                Err(error) => {
                    crate::logger::log(format!(
                        "remote_ipc: spread DB read-only open failed: {error}"
                    ));
                    None
                }
            };
        let view_trim_db_path = crate::data_dir::get().join("view_trim.db");
        let view_trim_db =
            match crate::view_trim_db::ViewTrimDb::open_existing_read_only_at(&view_trim_db_path) {
                Ok(db) => db,
                Err(error) => {
                    crate::logger::log(format!(
                        "remote_ipc: view trim DB read-only open failed: {error}"
                    ));
                    None
                }
            };
        Self {
            settings: Arc::new(settings),
            listing_settings,
            stats: Arc::new(Mutex::new(crate::stats::ThumbStats::new())),
            pdf_passwords: crate::pdf_passwords::PdfPasswordStore::load(),
            pdf_page_counts: Mutex::new(HashMap::new()),
            spread_db: Mutex::new(spread_db),
            view_trim_db: Mutex::new(view_trim_db),
            resume_reader,
            adjustment_settings,
            creative_lut_cache: Mutex::new(RemoteCreativeLutCache::default()),
            page_composite_cache: Mutex::new(RemoteCompositeCache::default()),
            auto_trim_bbox_cache: Mutex::new(RemoteAutoTrimCache::default()),
            source_single_flight: Arc::new(RemoteSourceSingleFlight::default()),
            remote_ai_native_cache: Mutex::new(RemoteAiNativeCache::default()),
            comic_stamp_cache: Mutex::new(HashMap::new()),
            session,
        }
    }
    fn adjustment_render_settings(
        &self,
    ) -> Result<crate::settings_db::AdjustmentRenderSettings, MediaError> {
        match &self.adjustment_settings {
            AdjustmentSettingsSource::Live => {
                crate::settings_db::with_db_result(|db| db.load_adjustment_render_settings())
                    .map_err(remote_adjustment_settings_error)
            }
            #[cfg(test)]
            AdjustmentSettingsSource::Snapshot(settings) => Ok(settings.clone()),
        }
    }

    fn adjustment_render_settings_timed(
        &self,
    ) -> Result<(crate::settings_db::AdjustmentRenderSettings, f64), MediaError> {
        match &self.adjustment_settings {
            AdjustmentSettingsSource::Live => crate::settings_db::with_db_result(|db| {
                db.load_adjustment_render_settings_with_lock_wait(true)
            })
            .map_err(remote_adjustment_settings_error),
            #[cfg(test)]
            AdjustmentSettingsSource::Snapshot(settings) => Ok((settings.clone(), 0.0)),
        }
    }

    pub(super) fn settings_for_listing(
        &self,
    ) -> Result<crate::settings::Settings, RemoteWriteError> {
        let mut settings = (*self.settings).clone();
        let live = self.listing_settings.load(&settings).map_err(|error| {
            crate::logger::log(format!(
                "remote_ipc: live listing settings read failed: {error}"
            ));
            RemoteWriteError::new(
                RemoteWriteErrorCode::PersistenceFailed,
                "最新の一覧設定を読み込めませんでした",
            )
        })?;
        live.apply_to(&mut settings);
        Ok(settings)
    }

    fn prepare_remote_edits(
        &self,
        page_key: &str,
        settings: &crate::settings_db::AdjustmentRenderSettings,
        context: &WorkerContext,
    ) -> Result<RemoteEditSnapshot, MediaError> {
        let erase = match context.mask_db.as_ref() {
            Some(db) => load_mask_snapshot(db, page_key)?,
            None => {
                let db = crate::mask_db::MaskDb::open_readonly()
                    .map_err(|error| remote_edit_db_open_error("erase", error))?;
                load_mask_snapshot(&db, page_key)?
            }
        };
        let local_adjust = match context.local_adjust_db.as_ref() {
            Some(db) => db
                .get_layers_checked(page_key)
                .map_err(|error| remote_edit_db_read_error("local-adjust", error))?,
            None => {
                let db = crate::local_adjust_db::LocalAdjustDb::open_readonly(
                    &crate::local_adjust_db::LocalAdjustDb::db_path(),
                )
                .map_err(|error| remote_edit_db_open_error("local-adjust", error))?;
                db.get_layers_checked(page_key)
                    .map_err(|error| remote_edit_db_read_error("local-adjust", error))?
            }
        };
        let conceal = match context.conceal_db.as_ref() {
            Some(db) => load_conceal_snapshot(db, page_key)?,
            None => {
                let db = crate::conceal_db::ConcealDb::open_readonly(
                    &crate::conceal_db::ConcealDb::db_path(),
                )
                .map_err(|error| remote_edit_db_open_error("conceal", error))?;
                load_conceal_snapshot(&db, page_key)?
            }
        };
        let comic = match context.comic_db.as_ref() {
            Some(db) => db
                .get_checked(page_key)
                .map_err(|error| remote_edit_db_read_error("comic", error))?
                .unwrap_or_default(),
            None => {
                let db = crate::comic_db::ComicDb::open_readonly()
                    .map_err(|error| remote_edit_db_open_error("comic", error))?;
                db.get_checked(page_key)
                    .map_err(|error| remote_edit_db_read_error("comic", error))?
                    .unwrap_or_default()
            }
        };
        let export_crop = match context.crop_db.as_ref() {
            Some(db) => db
                .get_checked(page_key)
                .map_err(|error| remote_edit_db_read_error("export-crop", error))?,
            None => {
                let db = crate::export_crop::CropDb::open_readonly(
                    &crate::export_crop::CropDb::db_path(),
                )
                .map_err(|error| remote_edit_db_open_error("export-crop", error))?;
                db.get_checked(page_key)
                    .map_err(|error| remote_edit_db_read_error("export-crop", error))?
            }
        };
        let conceal_preset = settings.conceal_preset.clone();
        let erase_mono_tolerance = settings.erase_inpaint_mono_tolerance;
        let fingerprint = remote_edit_fingerprint(
            erase.as_ref(),
            erase_mono_tolerance,
            local_adjust.as_ref(),
            conceal.as_ref(),
            &conceal_preset,
            &comic,
            export_crop.as_ref(),
        )?;
        let pre_ai_fingerprint = remote_pre_ai_edit_fingerprint(
            erase.as_ref(),
            erase_mono_tolerance,
            local_adjust.as_ref(),
            conceal.as_ref(),
            &conceal_preset,
        )?;
        Ok(RemoteEditSnapshot {
            erase,
            erase_mono_tolerance,
            local_adjust,
            conceal,
            conceal_preset,
            comic,
            export_crop,
            fingerprint,
            pre_ai_fingerprint,
        })
    }

    fn execute_remote_edits(
        &self,
        source: Arc<egui::ColorImage>,
        edits: RemoteEditSnapshot,
        cancel: &Arc<AtomicBool>,
    ) -> Result<RemoteMaterializedEdits, MediaError> {
        let ai_resources = edits.erase.as_ref().and_then(|_| {
            self.session
                .as_ref()
                .and_then(super::session::SessionHandle::remote_ai_resources)
        });
        let inpaint_runtime = ai_resources
            .as_ref()
            .map(|resources| Arc::clone(&resources.runtime));
        let inpaint_manager = ai_resources
            .map(|resources| resources.manager)
            .unwrap_or_else(|| Arc::new(crate::ai::model_manager::ModelManager::new()));
        let erase = match edits.erase {
            Some(mask) => {
                crate::edit_source::EditLayer::Materialize(crate::edit_source::EraseMaterialize {
                    mask,
                    runtime: inpaint_runtime,
                    manager: inpaint_manager,
                    mono_tolerance: edits.erase_mono_tolerance,
                    log_prefix: "remote page".to_string(),
                })
            }
            None => crate::edit_source::EditLayer::Absent,
        };
        let local_adjust =
            edits
                .local_adjust
                .map_or(crate::edit_source::EditLayer::Absent, |layers| {
                    crate::edit_source::EditLayer::Materialize(
                        crate::edit_source::LocalAdjustMaterialize { layers },
                    )
                });
        let conceal = edits
            .conceal
            .map_or(crate::edit_source::EditLayer::Absent, |mask| {
                crate::edit_source::EditLayer::Materialize(crate::edit_source::ConcealMaterialize {
                    mask,
                    preset: edits.conceal_preset,
                })
            });
        let result = crate::edit_source::execute_edit_source(
            crate::edit_source::EditSourceRequest {
                raw: source,
                erase,
                local_adjust,
                conceal,
            },
            cancel,
        )
        .map_err(|error| {
            crate::logger::log(format!("remote_ipc: edit materialization failed: {error}"));
            media_error(
                MediaErrorCode::RenderFailed,
                "編集結果をページへ合成できませんでした",
            )
        })?;
        let crate::edit_source::EditSourceResult::Ready(output) = result else {
            return Err(media_error(
                MediaErrorCode::Busy,
                "ページの編集結果合成は取り消されました",
            ));
        };
        Ok(RemoteMaterializedEdits {
            pixels: output.pixels,
            comic: edits.comic,
            export_crop: edits.export_crop,
            timing: output.timing,
            used_diffusion_fallback: output.used_diffusion_fallback,
        })
    }

    fn prepare_remote_composite(
        &self,
        address: &RemoteAddress,
        logical_path: &Path,
        mtime: i64,
        file_size: i64,
        target_px: u32,
        rotation: crate::rotation_db::Rotation,
        preview: Option<&mimageviewer_ipc::RemoteAdjustmentPreview>,
        context: &WorkerContext,
    ) -> Result<Option<RemotePreparedComposite>, MediaError> {
        self.prepare_remote_composite_timed(
            address,
            logical_path,
            mtime,
            file_size,
            target_px,
            rotation,
            preview,
            context,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_remote_composite_timed(
        &self,
        address: &RemoteAddress,
        logical_path: &Path,
        mtime: i64,
        file_size: i64,
        target_px: u32,
        rotation: crate::rotation_db::Rotation,
        preview: Option<&mimageviewer_ipc::RemoteAdjustmentPreview>,
        context: &WorkerContext,
        primary: Option<&mut RemotePageStageGuard>,
        fallback: Option<&mut RemotePageStageGuard>,
    ) -> Result<Option<RemotePreparedComposite>, MediaError> {
        let Some(mut identity) = remote_adjustment_identity(address, logical_path) else {
            return Ok(None);
        };
        identity.compiled_book = matches!(address.subresource, RemoteSubresource::File)
            && logical_path.parent().is_some_and(|parent| {
                crate::books::is_direct_book_folder(&self.settings.books_root_path(), parent)
            });
        let measure_lock_wait = primary.is_some() || fallback.is_some();
        let (settings, settings_lock_wait_ms) = if measure_lock_wait {
            self.adjustment_render_settings_timed()?
        } else {
            (self.adjustment_render_settings()?, 0.0)
        };
        if let Some(stage) = primary {
            stage.add_lock_wait_ms(settings_lock_wait_ms);
        } else if let Some(stage) = fallback {
            stage.add_lock_wait_ms(settings_lock_wait_ms);
        }
        let edits = self.prepare_remote_edits(&identity.page_key, &settings, context)?;
        // `WorkerContext` は worker 起動時に 1 度だけ DB を開く。その 1 回が失敗した状態を
        // そのまま合成失敗にすると、一過性の失敗でも **その worker を通る全ページ**が
        // 以後ずっと失敗する。開けない事実は隠さず、握り直しだけ試みる。
        let reopened;
        let adjustment_db = match context.adjustment_db.as_ref() {
            Some(db) => db,
            None => {
                reopened = crate::adjustment_db::AdjustmentDb::open().map_err(|error| {
                    crate::logger::log(format!(
                        "remote_ipc: adjustment db reopen failed for page composition: {error}"
                    ));
                    media_error(
                        MediaErrorCode::Internal,
                        "補正データベースを開けないためページを合成できません",
                    )
                })?;
                &reopened
            }
        };
        let page = adjustment_db
            .get_page_params_checked(&identity.page_key)
            .map_err(|error| remote_adjustment_read_error("page", error))?;
        let favorite_params = if page.is_none()
            || preview.is_some_and(|preview| {
                preview.scope == mimageviewer_ipc::RemoteAdjustmentScope::Standard
            }) {
            adjustment_db
                .load_all_favorite_params_checked()
                .map_err(|error| remote_adjustment_read_error("location", error))?
        } else {
            HashMap::new()
        };
        let selected_page = if preview.is_some_and(|preview| {
            preview.scope == mimageviewer_ipc::RemoteAdjustmentScope::Standard
        }) {
            None
        } else {
            page.as_ref()
        };
        let mut params = resolve_remote_effective_params(
            &identity,
            selected_page,
            &settings.favorites,
            &favorite_params,
            &settings.global_preset,
        );
        if let Some(preview) = preview {
            params = super::apply_remote_adjustment_values(params, &preview.values)
                .map_err(|message| media_error(MediaErrorCode::BadRequest, message))?;
        }
        let lut_entry = params.creative_lut.id.and_then(|id| {
            settings
                .creative_luts
                .iter()
                .find(|entry| entry.id == id)
                .cloned()
        });
        let key = RemoteCompositeCacheKey {
            page_key: identity.page_key,
            mtime,
            file_size,
            target_px,
            rotation,
            params: params.clone(),
            lut_entry: lut_entry.clone(),
            edit_fingerprint: edits.fingerprint,
        };
        Ok(Some(RemotePreparedComposite {
            key,
            params,
            lut_entry,
            edits,
            settings,
        }))
    }

    fn resolve_remote_lut(
        &self,
        entry: Option<&crate::creative_lut::CreativeLutEntry>,
    ) -> Result<Option<crate::creative_lut::SharedCreativeLut>, MediaError> {
        self.resolve_remote_lut_timed(entry, None, None)
    }

    fn resolve_remote_lut_timed(
        &self,
        entry: Option<&crate::creative_lut::CreativeLutEntry>,
        primary: Option<&mut RemotePageStageGuard>,
        fallback: Option<&mut RemotePageStageGuard>,
    ) -> Result<Option<crate::creative_lut::SharedCreativeLut>, MediaError> {
        let Some(entry) = entry else {
            return Ok(None);
        };
        lock_with_remote_page_wait(&self.creative_lut_cache, primary, fallback)
            .resolve(entry)
            .map(Some)
            .map_err(|error| {
                crate::logger::log(format!(
                    "remote_ipc: Creative LUT load failed id={}: {error}",
                    entry.id
                ));
                media_error(
                    MediaErrorCode::RenderFailed,
                    "Creative LUT を読み込めないためページを合成できません",
                )
            })
    }

    pub(super) fn container(&self, request: ContainerRequest) -> ContainerResponse {
        let started = Instant::now();
        let source_kind = media_source_kind(&request.address);
        let resolved = match self.resolve(&request.address) {
            Ok(resolved) => resolved,
            Err(error) => return ContainerResponse::Error(error),
        };
        let response = match self.enumerate(&request, &resolved) {
            Ok(payload) => ContainerResponse::Success(payload),
            Err(error) => ContainerResponse::Error(error),
        };
        let (outcome, entry_count, group_count, configured, effective, direction) = match &response
        {
            ContainerResponse::Success(payload) => (
                "ok",
                payload.entries.len(),
                payload.page_groups.len(),
                payload.configured_spread_mode.wire_name(),
                payload.effective_spread_mode.wire_name(),
                remote_reading_direction_name(payload.reading_direction),
            ),
            ContainerResponse::Error(_) => ("error", 0, 0, "none", "none", "none"),
        };
        crate::logger::log(format!(
            "remote_ipc: media_operation operation=container source_kind={source_kind} outcome={outcome} duration_ms={:.1} entry_count={entry_count} group_count={group_count} configured_spread={configured} effective_spread={effective} reading_direction={direction}",
            started.elapsed().as_secs_f64() * 1000.0
        ));
        response
    }

    pub(super) fn folder_list(&self, request: FolderListRequest) -> FolderListResponse {
        let started = Instant::now();
        let resolved = match self.resolve(&request.address) {
            Ok(resolved) => resolved,
            Err(error) => return FolderListResponse::Error(error),
        };
        if !matches!(request.address.subresource, RemoteSubresource::File)
            || !std::fs::metadata(&resolved.canonical).is_ok_and(|metadata| metadata.is_dir())
        {
            return FolderListResponse::Error(media_error(
                MediaErrorCode::BadRequest,
                "フォルダ一覧のアドレスが不正です",
            ));
        }
        let listing = match self.recompute_folder_listing(&resolved.logical) {
            Ok(listing) => listing,
            Err(error) => {
                return FolderListResponse::Error(media_error_from_remote_write(error));
            }
        };
        let thumbnail_sources =
            super::RemoteThumbnailSources::from_pairs(&listing.video_thumb_overrides);
        let entries = listing
            .items
            .iter()
            .zip(&listing.metas)
            .filter_map(|(item, meta)| {
                self.folder_list_entry(&request.address, item, *meta, &thumbnail_sources)
            })
            .collect::<Vec<_>>();
        let response = FolderListResponse::Success(FolderListPayload {
            effective_address: request.address,
            root_name: absolute_root_name(&resolved.logical),
            thumb_aspect_height_ratio: listing.thumb_aspect_height_ratio,
            sort_state: super::remote_grid_sort_state(
                if listing.sort_locked {
                    crate::app::BOOK_READING_PAGE_ORDER
                } else {
                    listing.sort_order
                },
                listing.sort_locked.then_some(super::BOOK_SORT_LOCK_REASON),
            ),
            entries,
            scan_ms: listing.scan_ms,
            materialize_ms: listing.materialize_ms,
        });
        let entry_count = match &response {
            FolderListResponse::Success(payload) => payload.entries.len(),
            FolderListResponse::Error(_) => 0,
        };
        crate::logger::log(format!(
            "remote_ipc: media_operation operation=folder_list outcome=ok duration_ms={:.1} entry_count={entry_count} scan_ms={:.1} materialize_ms={:.1}",
            started.elapsed().as_secs_f64() * 1000.0,
            listing.scan_ms,
            listing.materialize_ms,
        ));
        response
    }

    pub(super) fn validate_write_request(
        &self,
        request: &mut RemoteWriteRequest,
    ) -> Result<(), RemoteWriteError> {
        if let Some(address) = request.address_mut() {
            self.canonicalize_write_address(address)?;
        }
        if let Some(context_address) = request.context_address_mut() {
            self.canonicalize_write_address(context_address)?;
        }
        match request {
            RemoteWriteRequest::SetSpread { address, .. } => {
                let resolved = self
                    .resolve(address)
                    .map_err(remote_write_error_from_media)?;
                let metadata = std::fs::metadata(&resolved.canonical).ok();
                let is_file = metadata.as_ref().is_some_and(|value| value.is_file());
                let is_directory = metadata.as_ref().is_some_and(|value| value.is_dir());
                let supported = match address.subresource {
                    RemoteSubresource::File => {
                        is_directory
                            || is_file
                                && (is_archive_container(&resolved)
                                    || is_pdf_path(&resolved.logical))
                    }
                    RemoteSubresource::ZipDirectory { .. } => {
                        is_file && is_archive_container(&resolved)
                    }
                    RemoteSubresource::ZipEntry { .. } | RemoteSubresource::PdfPage { .. } => false,
                };
                supported.then_some(()).ok_or_else(|| {
                    RemoteWriteError::new(
                        RemoteWriteErrorCode::Unsupported,
                        "見開き設定を書き込めるコンテナではありません",
                    )
                })
            }
            RemoteWriteRequest::RecordReadingProgress {
                address,
                context_address,
                page_index,
                page_number,
                page_count,
                record_resume,
                record_history,
            } => {
                let validated = self.validate_page_context(address, context_address)?;
                if !validated.record_resume && !validated.record_history {
                    return Err(RemoteWriteError::new(
                        RemoteWriteErrorCode::Unsupported,
                        "この一覧は読書位置の記録対象ではありません",
                    ));
                }
                *page_index = validated.page_index;
                *page_number = validated.page_number;
                *page_count = validated.page_count;
                *record_resume = validated.record_resume;
                *record_history = validated.record_history;
                Ok(())
            }
            RemoteWriteRequest::SetRating { address, stars } => {
                if *stars > 5 {
                    return Err(RemoteWriteError::new(
                        RemoteWriteErrorCode::BadRequest,
                        "レーティングは 0〜5 で指定してください",
                    ));
                }
                self.validate_rating_page(address)
            }
            RemoteWriteRequest::SetBookmark {
                address,
                context_address,
                page_index,
                ..
            }
            | RemoteWriteRequest::SetBookBookmarkTitle {
                address,
                context_address,
                page_index,
                ..
            }
            | RemoteWriteRequest::RemoveBookBookmark {
                address,
                context_address,
                page_index,
                ..
            } => {
                let validated = self.validate_page_context(address, context_address)?;
                if !validated.bookmark_supported {
                    return Err(RemoteWriteError::new(
                        RemoteWriteErrorCode::Unsupported,
                        "このページは本のブックマーク対象ではありません",
                    ));
                }
                *page_index = validated.page_index;
                Ok(())
            }
            RemoteWriteRequest::GetItemState {
                address,
                context_address,
                page_index,
                bookmark_supported,
            } => {
                self.validate_rating_page(address)?;
                let validated = self.validate_page_context(address, context_address)?;
                *page_index = validated.page_index;
                *bookmark_supported = validated.bookmark_supported;
                Ok(())
            }
            RemoteWriteRequest::ListBookBookmarks {
                address,
                context_address,
                page_index,
                bookmark_supported,
            } => {
                let validated = self.validate_page_context(address, context_address)?;
                *page_index = validated.page_index;
                *bookmark_supported = validated.bookmark_supported;
                Ok(())
            }
            RemoteWriteRequest::SetAdjustment {
                address, values, ..
            } => {
                self.validate_rating_page(address)?;
                super::apply_remote_adjustment_values(
                    crate::adjustment::AdjustParams::default(),
                    values,
                )
                .map(|_| ())
                .map_err(|message| RemoteWriteError::new(RemoteWriteErrorCode::BadRequest, message))
            }
            RemoteWriteRequest::GetAdjustmentState { address } => {
                self.validate_rating_page(address)
            }
            RemoteWriteRequest::SetViewTrim {
                address,
                context_address,
                state,
            } => {
                self.validate_page_context(address, context_address)?;
                super::normalize_remote_view_trim_state(state)
                    .map(|_| ())
                    .map_err(|message| {
                        RemoteWriteError::new(RemoteWriteErrorCode::BadRequest, message)
                    })
            }
            RemoteWriteRequest::GetViewTrimState {
                address,
                context_address,
            } => self
                .validate_page_context(address, context_address)
                .map(|_| ()),
            RemoteWriteRequest::SetSortOrder { scope, sort_order } => {
                super::parse_sort_order_wire(sort_order).map_err(|message| {
                    RemoteWriteError::new(RemoteWriteErrorCode::BadRequest, message)
                })?;
                match scope {
                    mimageviewer_ipc::RemoteGridScope::Address { address } => {
                        let resolved = self
                            .resolve(address)
                            .map_err(remote_write_error_from_media)?;
                        if !std::fs::metadata(&resolved.canonical)
                            .is_ok_and(|metadata| metadata.is_dir())
                        {
                            return Err(RemoteWriteError::new(
                                RemoteWriteErrorCode::Unsupported,
                                super::BOOK_SORT_LOCK_REASON,
                            ));
                        }
                        let listing = self.recompute_folder_listing(&resolved.logical)?;
                        if crate::app::physical_page_order_locked(
                            &self.settings,
                            &resolved.logical,
                            &listing.items,
                        ) {
                            return Err(RemoteWriteError::new(
                                RemoteWriteErrorCode::Unsupported,
                                super::BOOK_SORT_LOCK_REASON,
                            ));
                        }
                        Ok(())
                    }
                    mimageviewer_ipc::RemoteGridScope::Collection { collection } => {
                        let mimageviewer_ipc::CollectionKind::SmartFolder { definition_id } =
                            collection
                        else {
                            return Err(RemoteWriteError::new(
                                RemoteWriteErrorCode::Unsupported,
                                super::FIXED_LIST_SORT_LOCK_REASON,
                            ));
                        };
                        let id = uuid::Uuid::parse_str(definition_id).map_err(|_| {
                            RemoteWriteError::new(
                                RemoteWriteErrorCode::BadRequest,
                                "ID が正しくありません",
                            )
                        })?;
                        self.settings
                            .smart_folders
                            .iter()
                            .any(|definition| definition.id == id)
                            .then_some(())
                            .ok_or_else(|| {
                                RemoteWriteError::new(
                                    RemoteWriteErrorCode::NotFound,
                                    "スマートフォルダが見つかりません",
                                )
                            })
                    }
                }
            }
        }
    }

    fn canonicalize_write_address(
        &self,
        address: &mut RemoteAddress,
    ) -> Result<(), RemoteWriteError> {
        let resolved = self
            .resolve(address)
            .map_err(remote_write_error_from_media)?;
        address.path = resolved.logical.to_string_lossy().into_owned();
        Ok(())
    }

    /// 現在の本の一覧を write worker 上で組み立てる。DB 読み出しとコンテナ列挙は
    /// UI thread に渡さず、同じ write FIFO 内で先行する mutation の完了後に行う。
    pub(super) fn book_bookmarks(&self, request: &mut RemoteWriteRequest) -> RemoteWriteResponse {
        let prepared_zip = match self.prepare_book_bookmark_list(request) {
            Ok(prepared) => prepared,
            Err(error) => return RemoteWriteResponse::Error(error),
        };
        let RemoteWriteRequest::ListBookBookmarks {
            context_address,
            bookmark_supported,
            ..
        } = request
        else {
            return RemoteWriteResponse::Error(RemoteWriteError::new(
                RemoteWriteErrorCode::Internal,
                "ブックマーク一覧要求の種別が一致しません",
            ));
        };
        if !*bookmark_supported {
            return RemoteWriteResponse::Success(RemoteWriteResult::book_bookmarks(
                RemoteBookBookmarkList {
                    supported: false,
                    rows: Vec::new(),
                },
            ));
        }

        let fallback_resolved;
        let resolved = if let Some(prepared) = prepared_zip.as_ref() {
            &prepared.resolved
        } else {
            fallback_resolved = match self.resolve(context_address) {
                Ok(resolved) => resolved,
                Err(error) => {
                    return RemoteWriteResponse::Error(remote_write_error_from_media(error));
                }
            };
            &fallback_resolved
        };
        let bookmarks =
            match crate::book_bookmarks::load_for_container_from_disk_readonly(&resolved.logical) {
                Ok(rows) => rows,
                Err(error) => {
                    crate::logger::log(format!(
                        "remote_ipc: book bookmark list read failed: {error}"
                    ));
                    return RemoteWriteResponse::Error(RemoteWriteError::new(
                        RemoteWriteErrorCode::PersistenceFailed,
                        "ブックマーク一覧を読み込めませんでした",
                    ));
                }
            };
        let container_address = RemoteAddress::file(context_address.path.clone());

        let rows = if std::fs::metadata(&resolved.canonical).is_ok_and(|metadata| metadata.is_dir())
        {
            let listing = match self.recompute_folder_listing(&resolved.logical) {
                Ok(listing) => listing,
                Err(error) => return RemoteWriteResponse::Error(error),
            };
            bookmarks
                .into_iter()
                .map(|bookmark| {
                    let target = match &bookmark.page_identity {
                        crate::book_bookmarks::PageIdentity::RelativePath(wanted) => listing
                            .items
                            .iter()
                            .enumerate()
                            .find_map(|(item_index, item)| {
                                let crate::grid_item::GridItem::Image(path) = item else {
                                    return None;
                                };
                                let relative = path.strip_prefix(&resolved.logical).ok()?;
                                (normalize_remote_bookmark_path(&relative.to_string_lossy())
                                    == normalize_remote_bookmark_path(wanted))
                                .then(|| {
                                    grid_item_address(&container_address, item).map(|address| {
                                        RemoteBookBookmarkTarget {
                                            address,
                                            context_address: container_address.clone(),
                                            item_index: u32::try_from(item_index)
                                                .unwrap_or(u32::MAX),
                                        }
                                    })
                                })
                                .flatten()
                            }),
                        _ => None,
                    };
                    remote_bookmark_row(bookmark, target)
                })
                .collect()
        } else if is_archive_container(resolved) {
            let tree = &prepared_zip
                .as_ref()
                .expect("ZIP bookmark list is prepared during validation")
                .tree;
            bookmarks
                .into_iter()
                .map(|bookmark| {
                    let target = match &bookmark.page_identity {
                        crate::book_bookmarks::PageIdentity::ArchiveEntry(entry_name) => {
                            crate::book_bookmarks::resolve_archive_bookmark_target(
                                &tree,
                                entry_name,
                                &self.settings.grid_display_order,
                            )
                            .and_then(|target| {
                                let address =
                                    zip_entry_address(&container_address, &target.entry_name);
                                address.validate_syntax().ok()?;
                                let context_address = RemoteAddress {
                                    path: container_address.path.clone(),
                                    subresource: if target.effective_prefix.is_empty() {
                                        RemoteSubresource::File
                                    } else {
                                        RemoteSubresource::ZipDirectory {
                                            prefix: target.effective_prefix,
                                        }
                                    },
                                };
                                Some(RemoteBookBookmarkTarget {
                                    address,
                                    context_address,
                                    item_index: u32::try_from(target.item_index).ok()?,
                                })
                            })
                        }
                        _ => None,
                    };
                    remote_bookmark_row(bookmark, target)
                })
                .collect()
        } else if is_pdf_path(&resolved.logical) {
            let metadata = match std::fs::metadata(&resolved.canonical) {
                Ok(metadata) => metadata,
                Err(_) => {
                    return RemoteWriteResponse::Error(RemoteWriteError::new(
                        RemoteWriteErrorCode::NotFound,
                        "PDF が見つかりません",
                    ));
                }
            };
            let page_count = match self.pdf_page_count(&resolved, &metadata) {
                Ok(page_count) => page_count,
                Err(error) => {
                    return RemoteWriteResponse::Error(remote_write_error_from_media(error));
                }
            };
            bookmarks
                .into_iter()
                .map(|bookmark| {
                    let target = match &bookmark.page_identity {
                        crate::book_bookmarks::PageIdentity::PdfPage(page_number)
                            if *page_number < page_count =>
                        {
                            Some(RemoteBookBookmarkTarget {
                                address: RemoteAddress {
                                    path: container_address.path.clone(),
                                    subresource: RemoteSubresource::PdfPage {
                                        page_number: *page_number,
                                    },
                                },
                                context_address: container_address.clone(),
                                item_index: *page_number,
                            })
                        }
                        _ => None,
                    };
                    remote_bookmark_row(bookmark, target)
                })
                .collect()
        } else {
            bookmarks
                .into_iter()
                .map(|bookmark| remote_bookmark_row(bookmark, None))
                .collect()
        };

        RemoteWriteResponse::Success(RemoteWriteResult::book_bookmarks(RemoteBookBookmarkList {
            supported: true,
            rows,
        }))
    }

    fn prepare_book_bookmark_list(
        &self,
        request: &mut RemoteWriteRequest,
    ) -> Result<Option<PreparedZipBookmarkList>, RemoteWriteError> {
        let is_zip_page = matches!(
            request,
            RemoteWriteRequest::ListBookBookmarks {
                address: RemoteAddress {
                    subresource: RemoteSubresource::ZipEntry { .. },
                    ..
                },
                ..
            }
        );
        if !is_zip_page {
            self.validate_write_request(request)?;
            return Ok(None);
        }

        let RemoteWriteRequest::ListBookBookmarks {
            address,
            context_address,
            page_index,
            bookmark_supported,
        } = request
        else {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::Internal,
                "ブックマーク一覧要求の種別が一致しません",
            ));
        };
        let RemoteSubresource::ZipEntry { entry_name } = &address.subresource else {
            unreachable!("ZIP bookmark list was checked above");
        };
        let (resolved, requested_prefix, enumeration) =
            self.enumerate_zip_page_context(address, context_address)?;

        // 現行のページ検証は archive 内の全 entry を見る。一方、一覧の移動先は
        // RemoteAddress として安全な entry だけを公開する。この差を保ったまま、重い
        // archive 列挙だけを共有し、tree の構築はメモリ上でそれぞれ行う。
        let validation_tree =
            crate::zip_tree::ZipTree::build(resolved.logical.clone(), enumeration.entries.clone());
        let validated =
            self.validate_zip_page_in_tree(&validation_tree, &requested_prefix, entry_name)?;
        *page_index = validated.page_index;
        *bookmark_supported = validated.bookmark_supported;

        let container_address = RemoteAddress::file(context_address.path.clone());
        let safe_entries = enumeration
            .entries
            .into_iter()
            .filter(|entry| {
                zip_entry_address(&container_address, &entry.entry_name)
                    .validate_syntax()
                    .is_ok()
            })
            .collect();
        Ok(Some(PreparedZipBookmarkList {
            tree: crate::zip_tree::ZipTree::build(resolved.logical.clone(), safe_entries),
            resolved,
        }))
    }

    fn validate_rating_page(&self, address: &RemoteAddress) -> Result<(), RemoteWriteError> {
        let resolved = self
            .resolve(address)
            .map_err(remote_write_error_from_media)?;
        let metadata = std::fs::metadata(&resolved.canonical).map_err(|_| {
            RemoteWriteError::new(RemoteWriteErrorCode::NotFound, "ページが見つかりません")
        })?;
        match &address.subresource {
            RemoteSubresource::File => {
                let extension = resolved
                    .logical
                    .extension()
                    .and_then(|value| value.to_str())
                    .map(str::to_ascii_lowercase)
                    .unwrap_or_default();
                if metadata.is_file() && crate::folder_tree::is_recognized_image_ext(&extension) {
                    Ok(())
                } else {
                    Err(RemoteWriteError::new(
                        RemoteWriteErrorCode::Unsupported,
                        "このファイルにはレーティングを付けられません",
                    ))
                }
            }
            RemoteSubresource::ZipEntry { entry_name } if is_archive_container(&resolved) => {
                let enumeration = crate::zip_loader::enumerate_image_entries_detailed(
                    resolved.readable_logical(),
                )
                .map_err(|_| {
                    RemoteWriteError::new(
                        RemoteWriteErrorCode::PersistenceFailed,
                        "ZIP を列挙できませんでした",
                    )
                })?;
                enumeration
                    .entries
                    .iter()
                    .any(|entry| entry.entry_name == *entry_name)
                    .then_some(())
                    .ok_or_else(|| {
                        RemoteWriteError::new(
                            RemoteWriteErrorCode::NotFound,
                            "ZIP 内のページが見つかりません",
                        )
                    })
            }
            RemoteSubresource::PdfPage { page_number } if is_pdf_path(&resolved.logical) => self
                .ensure_pdf_page_in_range(&resolved, &metadata, *page_number)
                .map_err(remote_write_error_from_media),
            _ => Err(RemoteWriteError::new(
                RemoteWriteErrorCode::Unsupported,
                "このページにはレーティングを付けられません",
            )),
        }
    }

    fn validate_page_context(
        &self,
        address: &RemoteAddress,
        context_address: &RemoteAddress,
    ) -> Result<ValidatedPageContext, RemoteWriteError> {
        match &address.subresource {
            RemoteSubresource::File => self.validate_folder_page(address, context_address),
            RemoteSubresource::ZipEntry { entry_name } => {
                self.validate_zip_page(address, context_address, entry_name)
            }
            RemoteSubresource::PdfPage { page_number } => {
                self.validate_pdf_page(address, context_address, *page_number)
            }
            RemoteSubresource::ZipDirectory { .. } => Err(RemoteWriteError::new(
                RemoteWriteErrorCode::Unsupported,
                "コンテナ自体はページではありません",
            )),
        }
    }

    fn validate_folder_page(
        &self,
        address: &RemoteAddress,
        context_address: &RemoteAddress,
    ) -> Result<ValidatedPageContext, RemoteWriteError> {
        if !matches!(context_address.subresource, RemoteSubresource::File) {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::BadRequest,
                "画像フォルダのコンテキストが不正です",
            ));
        }
        let page = self
            .resolve(address)
            .map_err(remote_write_error_from_media)?;
        let context = self
            .resolve(context_address)
            .map_err(remote_write_error_from_media)?;
        if !std::fs::metadata(&page.canonical).is_ok_and(|metadata| metadata.is_file())
            || !std::fs::metadata(&context.canonical).is_ok_and(|metadata| metadata.is_dir())
            || page.canonical.parent() != Some(context.canonical.as_path())
        {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::PathRejected,
                "画像が閲覧フォルダの直下にありません",
            ));
        }
        let listing = self.recompute_folder_listing(&context.logical)?;
        let items = listing.items;
        let image_only = listing.image_only;
        let compiled = listing.compiled;
        let auto_fullscreen_image_folders_enabled = listing.auto_fullscreen_image_folders_enabled;
        let index = items
            .iter()
            .position(|item| {
                matches!(
                    item,
                    crate::grid_item::GridItem::Image(path)
                        if crate::path_key::eq_keep_drive(path, &page.logical)
                )
            })
            .ok_or_else(|| {
                RemoteWriteError::new(
                    RemoteWriteErrorCode::NotFound,
                    "画像フォルダ内のページが見つかりません",
                )
            })?;
        let page_number = items[..=index]
            .iter()
            .filter(|item| item.has_page_data())
            .count();
        let page_count = items.iter().filter(|item| item.has_page_data()).count();
        validated_context(
            index,
            page_count,
            image_only,
            true,
            compiled || (image_only && auto_fullscreen_image_folders_enabled),
        )
        .map(|mut context| {
            context.page_number = u32::try_from(page_number).unwrap_or(u32::MAX);
            context
        })
    }

    fn recompute_folder_listing(
        &self,
        folder: &Path,
    ) -> Result<RecomputedFolderListing, RemoteWriteError> {
        let settings = self.settings_for_listing()?;
        let scan_started = Instant::now();
        let scan = crate::app::folder_scan::scan_directory_with_settings(folder, &settings)
            .map_err(|_| {
                RemoteWriteError::new(
                    RemoteWriteErrorCode::PersistenceFailed,
                    "画像フォルダを走査できませんでした",
                )
            })?;
        let scan_ms = scan_started.elapsed().as_secs_f64() * 1000.0;
        let compiled = crate::books::is_direct_book_folder(&settings.books_root_path(), folder);
        let image_only = if compiled {
            scan.all_media
                .iter()
                .any(|(_, kind, _, _)| *kind == crate::app::folder_scan::ScanMediaKind::Image)
        } else {
            crate::app::folder_scan::is_image_only_book_contents(
                !scan.folders.is_empty(),
                &scan.all_media,
            )
        };
        let materialize_started = Instant::now();
        let materialized = crate::app::materialize_local_folder_listing(folder, scan, &settings);
        let materialize_ms = materialize_started.elapsed().as_secs_f64() * 1000.0;
        let thumb_aspect_height_ratio = folder_thumb_aspect_height_ratio(&settings, folder);
        let sort_locked =
            crate::app::physical_page_order_locked(&settings, folder, &materialized.items);
        let auto_fullscreen_image_folders_enabled =
            settings.auto_fullscreen_image_folders_enabled();
        Ok(RecomputedFolderListing {
            items: materialized.items,
            metas: materialized.metas,
            video_thumb_overrides: materialized.video_thumb_overrides,
            scan_ms,
            materialize_ms,
            image_only,
            compiled,
            sort_order: settings.sort_order,
            thumb_aspect_height_ratio,
            sort_locked,
            auto_fullscreen_image_folders_enabled,
        })
    }

    fn folder_list_entry(
        &self,
        _container: &RemoteAddress,
        item: &crate::grid_item::GridItem,
        meta: Option<(i64, i64)>,
        thumbnail_sources: &super::RemoteThumbnailSources,
    ) -> Option<FolderListEntry> {
        let (path, kind) = match item {
            crate::grid_item::GridItem::Folder(path) => (path, RemoteEntryKind::Folder),
            crate::grid_item::GridItem::Image(path) => (path, RemoteEntryKind::Image),
            crate::grid_item::GridItem::Video(path) => (path, RemoteEntryKind::Video),
            crate::grid_item::GridItem::Audio(path) => (path, RemoteEntryKind::Audio),
            crate::grid_item::GridItem::ZipFile(path) => (path, RemoteEntryKind::Zip),
            crate::grid_item::GridItem::PdfFile(path) => (path, RemoteEntryKind::Pdf),
            crate::grid_item::GridItem::ConvertibleArchive { path, .. } => {
                (path, RemoteEntryKind::Archive)
            }
            _ => return None,
        };
        let address_for = |candidate: &Path| {
            let resolved = resolve_existing(candidate.to_string_lossy().as_ref()).ok()?;
            Some(RemoteAddress::file(
                resolved.logical.to_string_lossy().into_owned(),
            ))
        };
        let address = address_for(path)?;
        let thumbnail_address = thumbnail_sources
            .source_address(path, kind)
            .unwrap_or_else(|| address.clone());
        let (mtime, size) = meta.unwrap_or((0, 0));
        Some(FolderListEntry {
            address,
            thumbnail_address,
            name: item.name().into_owned(),
            kind,
            size: u64::try_from(size).unwrap_or(0),
            mtime,
        })
    }

    fn validate_zip_page(
        &self,
        address: &RemoteAddress,
        context_address: &RemoteAddress,
        entry_name: &str,
    ) -> Result<ValidatedPageContext, RemoteWriteError> {
        let (resolved, requested_prefix, enumeration) =
            self.enumerate_zip_page_context(address, context_address)?;
        let tree = crate::zip_tree::ZipTree::build(resolved.logical, enumeration.entries);
        self.validate_zip_page_in_tree(&tree, &requested_prefix, entry_name)
    }

    fn enumerate_zip_page_context(
        &self,
        address: &RemoteAddress,
        context_address: &RemoteAddress,
    ) -> Result<(ResolvedPath, String, crate::zip_loader::ZipEnumeration), RemoteWriteError> {
        let context = self
            .resolve(context_address)
            .map_err(remote_write_error_from_media)?;
        let resolved = self
            .resolve(address)
            .map_err(remote_write_error_from_media)?;
        if resolved.canonical != context.canonical {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::BadRequest,
                "ZIP ページとコンテキストが一致しません",
            ));
        }
        if !is_archive_container(&resolved) {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::Unsupported,
                "ZIP ページではありません",
            ));
        }
        let requested_prefix = match &context_address.subresource {
            RemoteSubresource::File => String::new(),
            RemoteSubresource::ZipDirectory { prefix } => prefix.clone(),
            _ => {
                return Err(RemoteWriteError::new(
                    RemoteWriteErrorCode::BadRequest,
                    "ZIP の閲覧コンテキストが不正です",
                ));
            }
        };
        let enumeration =
            crate::zip_loader::enumerate_image_entries_detailed(resolved.readable_logical())
                .map_err(|_| {
                    RemoteWriteError::new(
                        RemoteWriteErrorCode::PersistenceFailed,
                        "ZIP を列挙できませんでした",
                    )
                })?;
        Ok((resolved, requested_prefix, enumeration))
    }

    fn validate_zip_page_in_tree(
        &self,
        tree: &crate::zip_tree::ZipTree,
        requested_prefix: &str,
        entry_name: &str,
    ) -> Result<ValidatedPageContext, RemoteWriteError> {
        let requested_segments = zip_prefix_segments(&requested_prefix);
        if tree.node_at(&requested_segments).is_none() {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::NotFound,
                "ZIP 内の場所が見つかりません",
            ));
        }
        let effective_segments = tree.collapse_redundant(&requested_segments);
        let root_segments = tree.collapse_redundant(&[]);
        let (mut items, mut metas) =
            tree.materialize_level(&effective_segments, crate::app::BOOK_READING_PAGE_ORDER);
        crate::grid_item::arrange_grid_items(
            &mut items,
            &mut metas,
            &self.settings.grid_display_order,
            Some(crate::app::BOOK_READING_PAGE_ORDER),
        );
        let index = items
            .iter()
            .position(|item| {
                matches!(
                    item,
                    crate::grid_item::GridItem::ZipImage {
                        entry_name: candidate,
                        ..
                    } if candidate == entry_name
                )
            })
            .ok_or_else(|| {
                RemoteWriteError::new(
                    RemoteWriteErrorCode::NotFound,
                    "ZIP 内のページが見つかりません",
                )
            })?;
        let image_position = items[..=index]
            .iter()
            .filter(|item| item.has_page_data())
            .count()
            .saturating_sub(1);
        let image_count = items.iter().filter(|item| item.has_page_data()).count();
        validated_context(
            index,
            image_count,
            items.iter().all(|item| item.has_page_data()),
            effective_segments == root_segments,
            true,
        )
        .map(|mut context| {
            context.page_number =
                u32::try_from(image_position.saturating_add(1)).unwrap_or(u32::MAX);
            context
        })
    }

    fn validate_pdf_page(
        &self,
        address: &RemoteAddress,
        context_address: &RemoteAddress,
        page_number: u32,
    ) -> Result<ValidatedPageContext, RemoteWriteError> {
        if !matches!(context_address.subresource, RemoteSubresource::File) {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::BadRequest,
                "PDF ページとコンテキストが一致しません",
            ));
        }
        let resolved = self
            .resolve(address)
            .map_err(remote_write_error_from_media)?;
        let context = self
            .resolve(context_address)
            .map_err(remote_write_error_from_media)?;
        if resolved.canonical != context.canonical {
            return Err(RemoteWriteError::new(
                RemoteWriteErrorCode::BadRequest,
                "PDF ページとコンテキストが一致しません",
            ));
        }
        let metadata = std::fs::metadata(&resolved.canonical).map_err(|_| {
            RemoteWriteError::new(RemoteWriteErrorCode::NotFound, "PDF が見つかりません")
        })?;
        let page_count = self
            .pdf_page_count(&resolved, &metadata)
            .map_err(remote_write_error_from_media)?;
        validate_page_number(page_number, page_count).map_err(remote_write_error_from_media)?;
        validated_context(page_number as usize, page_count as usize, true, true, true)
    }

    pub(super) fn thumbnail(
        &self,
        request: &mimageviewer_ipc::ThumbnailRequest,
        context: &WorkerContext,
    ) -> ThumbnailResponse {
        let started = Instant::now();
        let source_kind = media_source_kind(&request.address);
        let resolved = match self.resolve(&request.address) {
            Ok(resolved) => resolved,
            Err(error) => return thumbnail_error_from_media(error),
        };
        let rotation =
            context.rotation_for_remote_page(&resolved.logical, &request.address.subresource);
        let response = match self.load_image(
            &request.address,
            &resolved,
            request.target_px,
            RemoteImageLoadKind::Thumbnail,
            rotation,
            false,
            context,
            None,
            None,
        ) {
            Ok(loaded) => color_image_to_dynamic_image(&loaded.pixels)
                .and_then(|image| {
                    crate::catalog::encode_thumb_webp(
                        &image,
                        request.target_px,
                        self.settings.thumb_quality as f32,
                    )
                })
                .map(|(webp_bytes, _, _)| ThumbnailResponse::Success { webp_bytes })
                .unwrap_or_else(|| {
                    thumbnail_error(
                        ThumbnailErrorCode::GenerationFailed,
                        "WebP エンコードに失敗しました",
                    )
                }),
            Err(error) => thumbnail_error_from_media(error),
        };
        let (outcome, output_bytes) = match &response {
            ThumbnailResponse::Success { webp_bytes } => ("ok", webp_bytes.len()),
            ThumbnailResponse::Error(_) => ("error", 0),
        };
        crate::logger::log(format!(
            "remote_ipc: media_operation operation=thumbnail source_kind={source_kind} outcome={outcome} duration_ms={:.1} output_bytes={output_bytes}",
            started.elapsed().as_secs_f64() * 1000.0
        ));
        response
    }

    pub(super) fn page_with_job_cancel(
        &self,
        request: PageRequest,
        context: &WorkerContext,
        job_cancel: Arc<AtomicBool>,
    ) -> PageResponse {
        self.page_inner(request, context, job_cancel)
    }

    fn page_inner(
        &self,
        request: PageRequest,
        context: &WorkerContext,
        cancel: Arc<AtomicBool>,
    ) -> PageResponse {
        let started = Instant::now();
        let source_kind = media_source_kind(&request.address);
        let priority = request.priority;
        let page_perf = RemotePagePerf::new(&request, source_kind);
        let mut total_stage = page_perf.enter(RemotePageStage::Total);
        let mut resolve_stage = page_perf.enter(RemotePageStage::Resolve);
        if cancel.load(Ordering::Acquire) {
            return cancelled_page_error();
        }
        if request.target_px == 0 || request.target_px > MAX_PAGE_RENDER_PX {
            return PageResponse::Error(media_error(
                MediaErrorCode::BadRequest,
                "画像サイズが範囲外です",
            ));
        }
        if let Some(preview) = request.adjustment_preview.as_ref()
            && let Err(message) = super::apply_remote_adjustment_values(
                crate::adjustment::AdjustParams::default(),
                &preview.values,
            )
        {
            return PageResponse::Error(media_error(MediaErrorCode::BadRequest, message));
        }
        let resolved = match self.resolve(&request.address) {
            Ok(resolved) => resolved,
            Err(error) => return PageResponse::Error(error),
        };
        let rotation =
            context.rotation_for_remote_page(&resolved.logical, &request.address.subresource);
        let view_trim_plan = match self.remote_view_trim_plan_timed(
            &request.address,
            &resolved,
            request.render_context.as_ref(),
            rotation,
            resolve_stage.as_mut(),
        ) {
            Ok(plan) => plan,
            Err(error) => return PageResponse::Error(error),
        };
        let load_kind = if view_trim_plan.requires_auto_detection() {
            RemoteImageLoadKind::CompositedPageWithAutoTrim
        } else {
            RemoteImageLoadKind::CompositedPage
        };
        let partner_start = match self.prepare_remote_auto_trim_partner_timed(
            &view_trim_plan,
            request.target_px,
            &cancel,
            resolve_stage.as_mut(),
        ) {
            Ok(start) => start,
            Err(error) => return PageResponse::Error(error),
        };
        let load_timing = resolve_stage.take().map(|resolve| RemotePageLoadTiming {
            perf: page_perf.clone(),
            resolve,
        });
        let foreground = priority == PagePriority::Foreground;
        let response = with_scoped_remote_partner(
            partner_start,
            |partner: RemotePartnerAutoTrimRequest, partner_cancel| {
                // 相手ページは AutoTrimReference なので合成せず、context の DB を 1 つも
                // 読まない。ここで WorkerContext::open() を呼ぶと、見開きページごとに
                // SQLite を 9 個開くことになる。
                let partner_context = WorkerContext::without_databases();
                self.remote_auto_trim_bbox_timed(
                    &partner.address,
                    &partner.resolved,
                    request.target_px,
                    foreground,
                    &partner_context,
                    &partner_cancel,
                    None,
                )
            },
            || {
                let loaded = self.load_image_timed(
                    &request.address,
                    &resolved,
                    request.target_px,
                    load_kind,
                    rotation,
                    foreground,
                    context,
                    Some(&cancel),
                    request.adjustment_preview.as_ref(),
                    load_timing,
                    None,
                )?;
                if cancel.load(Ordering::Acquire) {
                    return Err(media_error(
                        MediaErrorCode::Cancelled,
                        "ページの表示需要がなくなったため処理を取り消しました",
                    ));
                }
                Ok(loaded)
            },
            |loaded, partner| {
                let mut trim_stage = page_perf.enter(RemotePageStage::Trim);
                let partner_auto_trim_bbox = partner.collect()?;
                let view_trim_bbox = complete_remote_view_trim_bbox_from_partner(
                    &view_trim_plan,
                    loaded.auto_trim_bbox,
                    partner_auto_trim_bbox,
                );
                let encode_timing = trim_stage.take().map(|trim| RemotePageEncodeTiming {
                    perf: page_perf.clone(),
                    trim,
                });
                Ok(
                    match encode_remote_page_jpeg_timed(
                        &loaded.pixels,
                        request.target_px,
                        view_trim_bbox,
                        encode_timing,
                    ) {
                        Some((bytes, width, height)) => PageResponse::Success(PagePayload {
                            bytes,
                            content_type: "image/jpeg".to_owned(),
                            width,
                            height,
                            identity: loaded.identity.clone(),
                        }),
                        None => PageResponse::Error(media_error(
                            MediaErrorCode::RenderFailed,
                            "JPEG エンコードに失敗しました",
                        )),
                    },
                )
            },
        )
        .unwrap_or_else(PageResponse::Error);
        let response = if cancel.load(Ordering::Acquire) {
            cancelled_page_error()
        } else {
            response
        };
        let (outcome, output_bytes) = match &response {
            PageResponse::Success(payload) => ("ok", payload.bytes.len()),
            PageResponse::Error(_) => ("error", 0),
        };
        if let Some(stage) = total_stage.as_mut() {
            let metrics = match &response {
                PageResponse::Success(payload) => {
                    let pixels = u64::from(payload.width).saturating_mul(u64::from(payload.height));
                    RemotePageStageMetrics {
                        pixels,
                        bytes: payload.bytes.len() as u64,
                        output_pixels: pixels,
                        output_bytes: payload.bytes.len() as u64,
                    }
                }
                PageResponse::Error(_) => RemotePageStageMetrics::default(),
            };
            stage.finish_with_outcome(metrics, outcome);
        }
        crate::logger::log(format!(
            "remote_ipc: media_operation operation=page source_kind={source_kind} priority={} outcome={outcome} duration_ms={:.1} output_bytes={output_bytes}",
            if priority == PagePriority::Prefetch {
                "prefetch"
            } else {
                "foreground"
            },
            started.elapsed().as_secs_f64() * 1000.0
        ));
        response
    }

    fn resolve(&self, address: &RemoteAddress) -> Result<ResolvedPath, MediaError> {
        address.validate_syntax().map_err(|error| {
            media_error(
                MediaErrorCode::BadRequest,
                if error == mimageviewer_ipc::AddressError::NetworkPath {
                    mimageviewer_ipc::REMOTE_NETWORK_PATH_MESSAGE
                } else {
                    "コンテンツアドレスが不正です"
                },
            )
        })?;
        if let Some(registry) = self
            .session
            .as_ref()
            .and_then(super::session::SessionHandle::archive_job_registry)
        {
            match registry.active_resolved_path(&address.path) {
                Ok(Some(resolved)) => return Ok(resolved),
                Ok(None) => {}
                Err(error) => return Err(active_archive_media_error(error)),
            }
        }
        resolve_existing(&address.path).map_err(resolve_media_error)
    }

    pub(super) fn execute_remote_ai(
        &self,
        request: &RemoteAiStartRequest,
        progress: &dyn super::ai_job::RemoteAiProgressSink,
        cancel: &Arc<AtomicBool>,
    ) -> super::ai_job::RemoteAiExecutionOutcome {
        match self.execute_remote_ai_inner(request, progress, cancel) {
            Ok(results) => super::ai_job::RemoteAiExecutionOutcome::Completed(results),
            Err(RemoteAiRunError::NotApplicable {
                code,
                message,
                page_index,
            }) => super::ai_job::RemoteAiExecutionOutcome::Failed(format!(
                "page-local NotApplicable escaped aggregation at page {page_index}: {code:?}: {message}"
            )),
            Err(RemoteAiRunError::Superseded(message)) => {
                crate::logger::log(format!(
                    "remote_ipc: remote AI result rejected as stale: {message}"
                ));
                super::ai_job::RemoteAiExecutionOutcome::Superseded(
                    "表示中の画像または設定が変わったため、AI 処理結果を使用しませんでした"
                        .to_owned(),
                )
            }
            Err(RemoteAiRunError::Failed(message)) => {
                crate::logger::log(format!("remote_ipc: remote AI execution failed: {message}"));
                super::ai_job::RemoteAiExecutionOutcome::Failed(
                    "AI 処理を完了できませんでした".to_owned(),
                )
            }
        }
    }

    fn execute_remote_ai_inner(
        &self,
        request: &RemoteAiStartRequest,
        progress: &dyn super::ai_job::RemoteAiProgressSink,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Vec<super::ai_job::RemoteAiPageExecutionOutcome>, RemoteAiRunError> {
        self.execute_remote_ai_inner_with(
            request,
            progress,
            cancel,
            &|engine, address, logical_path, mtime, file_size, target_px, rotation, context| {
                engine.prepare_remote_composite(
                    address,
                    logical_path,
                    mtime,
                    file_size,
                    target_px,
                    rotation,
                    None,
                    context,
                )
            },
            &|engine, address, resolved, metadata, page_index, cancel| {
                engine.decode_remote_ai_source(address, resolved, metadata, page_index, cancel)
            },
            &|engine| {
                engine
                    .session
                    .as_ref()
                    .and_then(super::session::SessionHandle::remote_ai_resources)
            },
        )
    }

    fn execute_remote_ai_inner_with(
        &self,
        request: &RemoteAiStartRequest,
        progress: &dyn super::ai_job::RemoteAiProgressSink,
        cancel: &Arc<AtomicBool>,
        prepare_composite: &dyn Fn(
            &Self,
            &RemoteAddress,
            &Path,
            i64,
            i64,
            u32,
            crate::rotation_db::Rotation,
            &WorkerContext,
        ) -> Result<Option<RemotePreparedComposite>, MediaError>,
        decode_source: &dyn Fn(
            &Self,
            &RemoteAddress,
            &ResolvedPath,
            &std::fs::Metadata,
            usize,
            &Arc<AtomicBool>,
        )
            -> Result<(Arc<egui::ColorImage>, [usize; 2]), RemoteAiRunError>,
        resources_for_remote: &dyn Fn(&Self) -> Option<super::session::RemoteAiResources>,
    ) -> Result<Vec<super::ai_job::RemoteAiPageExecutionOutcome>, RemoteAiRunError> {
        let context = WorkerContext::open();
        let page_count = request.pages.len();
        let mut results = Vec::with_capacity(page_count);
        let mut identities = Vec::with_capacity(page_count);

        for (page_index, page) in request.pages.iter().enumerate() {
            let page_result = (|| -> Result<
                (PagePayload, (RemoteAddress, u32, RemoteAiResultIdentity)),
                RemoteAiRunError,
            > {
            check_remote_ai_cancel(cancel)?;
            progress.update(
                mimageviewer_ipc::RemoteAiJobState::PreparingSource,
                Some(remote_ai_progress(
                    RemoteAiProgressPhase::PreparingSource,
                    page_index,
                    page_count,
                    0,
                    None,
                )),
            );
            let resolved = self.resolve(&page.address).map_err(remote_ai_media_error)?;
            let metadata = std::fs::metadata(&resolved.canonical)
                .map_err(|error| RemoteAiRunError::Failed(format!("source metadata: {error}")))?;
            if !metadata.is_file() {
                return Err(RemoteAiRunError::Failed(
                    "AI source is not a file".to_owned(),
                ));
            }
            let mtime = crate::ui_helpers::mtime_secs(&metadata);
            let file_size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
            let rotation = context
                .rotation_for_remote_page(&resolved.logical, &page.address.subresource);
            let prepared = prepare_composite(
                self,
                &page.address,
                &resolved.logical,
                mtime,
                file_size,
                page.target_px,
                rotation,
                &context,
            )
            .map_err(remote_ai_media_error)?
            .ok_or_else(|| {
                RemoteAiRunError::Failed("address does not identify an image page".to_owned())
            })?;
            let (source, decoded_source_dims) = decode_source(
                self,
                &page.address,
                &resolved,
                &metadata,
                page_index,
                cancel,
            )?;
            let requires_stored_edit_space = !prepared.edits.comic.is_empty()
                || prepared.edits.export_crop.is_some();
            let pdf_canonical_raster_dims = matches!(
                &page.address.subresource,
                RemoteSubresource::PdfPage { .. }
            )
            .then_some(decoded_source_dims);
            let stored_edit_space = StoredEditSpace::for_remote_source(
                &page.address.subresource,
                source.size,
                Some(decoded_source_dims),
                pdf_canonical_raster_dims,
            );
            if requires_stored_edit_space && stored_edit_space.is_none() {
                if let RemoteSubresource::PdfPage { page_number } = &page.address.subresource {
                    record_skipped_stored_edits(SkippedStoredEdits {
                        pipeline: StoredEditPipeline::FinalAi,
                        page_number: *page_number,
                        reason: StoredEditSkipReason::PdfCanonicalRasterUnavailable,
                        crop: prepared.edits.export_crop.is_some(),
                        comic_objects: prepared.edits.comic.len(),
                    });
                }
            }
            check_remote_ai_cancel(cancel)?;
            let pre_ai_edit_fingerprint = prepared.edits.pre_ai_fingerprint;
            let materialized = self
                .execute_remote_edits(source, prepared.edits.clone(), cancel)
                .map_err(remote_ai_media_error)?;
            let selected = crate::ai::final_pipeline::select_final_ai_models(
                &materialized.pixels,
                &prepared.params,
                prepared.settings.ai_feature_mode,
                prepared.settings.ai_upscale_limit,
                prepared.settings.ai_denoise_limit,
            )
            .ok_or_else(|| RemoteAiRunError::NotApplicable {
                code: RemoteAiTerminalCode::SizeGate,
                message: "AI が無効か、元画像が設定された処理サイズ上限の対象外です"
                    .to_owned(),
                page_index,
            })?;
            // Animated/vector/size-gated pages become page-local NotApplicable outcomes above
            // without initializing or loading the shared runtime. Remaining pages continue, and
            // only an applicable final-AI request claims the bridge.
            let resources = resources_for_remote(self)
                .ok_or_else(|| RemoteAiRunError::Failed("AI runtime is unavailable".to_owned()))?;

            let native_key = RemoteAiNativeCacheKey {
                page_key: prepared.key.page_key.clone(),
                mtime,
                file_size,
                source_size: materialized.pixels.size,
                pre_ai_params: remote_ai_pre_params(&prepared.params),
                pre_ai_edit_fingerprint,
                ai_feature_mode: prepared.settings.ai_feature_mode,
                ai_upscale_limit: prepared.settings.ai_upscale_limit,
                ai_denoise_limit: prepared.settings.ai_denoise_limit,
                ai_backend: prepared.settings.ai_backend.clone(),
                background_mode: resources.background_mode,
                pipeline_schema: REMOTE_AI_PIPELINE_SCHEMA,
                model_epoch: remote_ai_model_epoch(
                    &resources.runtime,
                    &resources.manager,
                    selected,
                ),
            };
            let (max_entries, max_bytes) =
                remote_ai_native_budget(&prepared.settings).unwrap_or((0, 0));
            let cached = {
                let mut cache = self
                    .remote_ai_native_cache
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                // Re-apply the live retained-cache settings even on a hit. Otherwise lowering
                // the configured budget would not affect remote entries until the next miss.
                cache.enforce_budget(max_entries, max_bytes);
                cache.get(&native_key)
            };
            let (native_pixels, used_upscale) = match cached {
                Some(hit) => hit,
                None => {
                    let progress_adapter = ContainerFinalAiProgress {
                        sink: progress,
                        page_index,
                        page_count,
                    };
                    let output = crate::ai::final_pipeline::execute_selected_final_ai(
                        &resources.runtime,
                        &resources.manager,
                        crate::ai::final_pipeline::FinalAiExecutionRequest {
                            source: Arc::clone(&materialized.pixels),
                            adjust_before_ai: (!prepared.params.is_color_identity())
                                .then(|| prepared.params.clone()),
                            denoise_kind: selected.denoise,
                            upscale_kind: selected.upscale,
                            background_mode: resources.background_mode,
                        },
                        cancel,
                        &progress_adapter,
                    )
                    .map_err(|error| match error {
                        crate::ai::final_pipeline::FinalAiExecutionError::Cancelled => {
                            RemoteAiRunError::Failed("AI job was cancelled".to_owned())
                        }
                        crate::ai::final_pipeline::FinalAiExecutionError::Failed(error) => {
                            RemoteAiRunError::Failed(error)
                        }
                    })?;
                    let pixels = Arc::new(output.image);
                    self.remote_ai_native_cache
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .insert(
                            native_key,
                            Arc::clone(&pixels),
                            output.used_upscale,
                            max_entries,
                            max_bytes,
                        );
                    (pixels, output.used_upscale)
                }
            };

            progress.update(
                mimageviewer_ipc::RemoteAiJobState::Finalizing,
                Some(remote_ai_progress(
                    RemoteAiProgressPhase::Finalizing,
                    page_index,
                    page_count,
                    4,
                    None,
                )),
            );
            check_remote_ai_cancel(cancel)?;
            let lut = self
                .resolve_remote_lut(prepared.lut_entry.as_ref())
                .map_err(remote_ai_media_error)?;
            let plan = crate::final_composite::build_final_composite_plan_after_ai(
                &prepared.params,
                lut.map(|lut| (lut, prepared.params.creative_lut.strength)),
                used_upscale,
            );
            let mut pixels = match crate::final_composite::execute_final_composite(
                native_pixels,
                plan,
                cancel,
            ) {
                crate::final_composite::FinalCompositeResult::Ready { pixels, .. } => pixels,
                crate::final_composite::FinalCompositeResult::Cancelled => {
                    return Err(RemoteAiRunError::Failed("AI job was cancelled".to_owned()));
                }
            };
            if let Some(stored_edit_space) = stored_edit_space {
                if !materialized.comic.is_empty()
                    && let Some(fonts) =
                        crate::comic_overlay::load_comic_fonts_for(&materialized.comic)
                {
                    pixels = stored_edit_space.comic_composite(
                        &pixels,
                        &materialized.comic,
                        &fonts,
                        &mut self
                            .comic_stamp_cache
                            .lock()
                            .unwrap_or_else(|error| error.into_inner()),
                        cancel,
                    );
                }
            }
            pixels = crop_with_stored_edit_space(
                pixels,
                materialized.export_crop,
                stored_edit_space,
            )
            .map_err(RemoteAiRunError::Failed)?;
            check_remote_ai_cancel(cancel)?;
            pixels = apply_remote_page_rotation(pixels, rotation);
            let identity = super::path_guard::page_identity_from_resolved(
                &resolved,
                &page.address.subresource,
            );
            let image = loaded_image_from_color_image(&pixels, None, identity)
                .map_err(remote_ai_media_error)?;
            let view_trim_plan = self
                .remote_view_trim_plan(
                    &page.address,
                    &resolved,
                    page.render_context.as_ref(),
                    rotation,
                )
                .map_err(remote_ai_media_error)?;
            // Auto は AI 出力ではなく本体と同じ元ページ raster から検出する。bbox cache を
            // 参照することで、AI result へ切り替えても表示 bbox を変えない。
            let auto_trim_bbox = if view_trim_plan.requires_auto_detection() {
                self.remote_auto_trim_bbox(
                    &page.address,
                    &resolved,
                    page.target_px,
                    true,
                    &context,
                    cancel,
                )
                .map_err(remote_ai_media_error)?
            } else {
                None
            };
            let view_trim_bbox = self
                .complete_remote_view_trim_bbox(
                    &view_trim_plan,
                    auto_trim_bbox,
                    page.target_px,
                    true,
                    &context,
                    cancel,
                )
                .map_err(remote_ai_media_error)?;
            let (bytes, width, height) =
                encode_remote_page_jpeg(&image.pixels, page.target_px, view_trim_bbox)
                    .ok_or_else(|| RemoteAiRunError::Failed("JPEG encoding failed".to_owned()))?;
            Ok((
                PagePayload {
                    bytes,
                    content_type: "image/jpeg".to_owned(),
                    width,
                    height,
                    identity: image.identity.clone(),
                },
                (
                    image.identity,
                    page.target_px,
                    RemoteAiResultIdentity::from_prepared(&prepared, resources.background_mode),
                ),
            ))
            })();
            match page_result {
                Ok((payload, identity)) => {
                    results.push(super::ai_job::RemoteAiPageExecutionOutcome::Ready(payload));
                    identities.push(identity);
                }
                Err(RemoteAiRunError::NotApplicable { code, message, .. }) => {
                    results.push(super::ai_job::RemoteAiPageExecutionOutcome::NotApplicable {
                        code,
                        message,
                    });
                }
                Err(error) => return Err(error),
            }
        }

        if identities.is_empty() {
            return Ok(results);
        }
        // Result bytes are publishable only if source/edit/settings still match the snapshots
        // that produced them. Re-open worker DB handles so this is a true completion-time read.
        let validation_context = WorkerContext::open();
        let current_background = self
            .session
            .as_ref()
            .and_then(super::session::SessionHandle::remote_ai_resources)
            .map(|resources| resources.background_mode)
            .ok_or_else(|| RemoteAiRunError::Superseded("AI runtime was detached".to_owned()))?;
        for (address, target_px, expected) in identities {
            check_remote_ai_cancel(cancel)?;
            let resolved = self.resolve(&address).map_err(|_| {
                RemoteAiRunError::Superseded("source is no longer available".to_owned())
            })?;
            let metadata = std::fs::metadata(&resolved.canonical)
                .map_err(|_| RemoteAiRunError::Superseded("source metadata changed".to_owned()))?;
            let rotation = validation_context
                .rotation_for_remote_page(&resolved.logical, &address.subresource);
            let current = self
                .prepare_remote_composite(
                    &address,
                    &resolved.logical,
                    crate::ui_helpers::mtime_secs(&metadata),
                    i64::try_from(metadata.len()).unwrap_or(i64::MAX),
                    target_px,
                    rotation,
                    None,
                    &validation_context,
                )
                .map_err(|_| {
                    RemoteAiRunError::Superseded("source snapshot cannot be revalidated".to_owned())
                })?
                .map(|prepared| {
                    RemoteAiResultIdentity::from_prepared(&prepared, current_background)
                });
            if current.as_ref() != Some(&expected) {
                return Err(RemoteAiRunError::Superseded(
                    "source, edits, or AI settings changed while the job was running".to_owned(),
                ));
            }
        }
        Ok(results)
    }

    fn decode_remote_ai_source(
        &self,
        address: &RemoteAddress,
        resolved: &ResolvedPath,
        metadata: &std::fs::Metadata,
        page_index: usize,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(Arc<egui::ColorImage>, [usize; 2]), RemoteAiRunError> {
        match &address.subresource {
            RemoteSubresource::File if is_image_path(&resolved.logical) => {
                decode_remote_ai_canonical(
                    crate::canonical_image_loader::CanonicalImageSource::File {
                        path: &resolved.canonical,
                        verified_bytes: None,
                    },
                    page_index,
                    cancel,
                )
            }
            RemoteSubresource::ZipEntry { entry_name } if is_archive_container(resolved) => {
                decode_remote_ai_canonical(
                    crate::canonical_image_loader::CanonicalImageSource::ArchiveEntry {
                        archive_path: resolved.readable_canonical(),
                        entry_name,
                    },
                    page_index,
                    cancel,
                )
            }
            RemoteSubresource::PdfPage { page_number } if is_pdf_path(&resolved.logical) => {
                self.ensure_pdf_page_in_range(resolved, metadata, *page_number)
                    .map_err(remote_ai_media_error)?;
                let password = self.pdf_passwords.get(&resolved.logical);
                let analysis = crate::pdf_loader::analyze_page_content_type(
                    &resolved.canonical,
                    *page_number,
                    password.as_deref(),
                    Some(Arc::clone(cancel)),
                )
                .map_err(|error| RemoteAiRunError::Failed(error.to_string()))?;
                if matches!(
                    analysis.content_type,
                    crate::pdf_loader::PdfPageContentType::Vector
                ) {
                    return Err(RemoteAiRunError::NotApplicable {
                        code: RemoteAiTerminalCode::VectorPdf,
                        message: "ベクター PDF ページは AI 静止画処理の対象外です".to_owned(),
                        page_index,
                    });
                }
                match crate::pdf_loader::render_page_canonical_raster(
                    &resolved.canonical,
                    *page_number,
                    analysis.content_type,
                    password.as_deref(),
                    Some(Arc::clone(cancel)),
                    crate::pdf_loader::JobPriority::Normal,
                    0,
                    crate::pdf_loader::CancelWaitPolicy::AbortOnCancel,
                )
                .map_err(|error| RemoteAiRunError::Failed(error.to_string()))?
                {
                    crate::pdf_loader::CanonicalPdfPage::Vector => {
                        Err(RemoteAiRunError::NotApplicable {
                            code: RemoteAiTerminalCode::VectorPdf,
                            message:
                                "PDF ページがベクターとして判定されたため AI 静止画処理の対象外です"
                                    .to_owned(),
                            page_index,
                        })
                    }
                    crate::pdf_loader::CanonicalPdfPage::Raster {
                        image, native_dims, ..
                    } => Ok((
                        Arc::new(crate::canonical_image_loader::dynamic_image_to_color_image(
                            &image,
                        )),
                        [native_dims[0] as usize, native_dims[1] as usize],
                    )),
                }
            }
            _ => Err(RemoteAiRunError::Failed(
                "address is not a supported still-image page".to_owned(),
            )),
        }
    }

    fn enumerate(
        &self,
        request: &ContainerRequest,
        resolved: &ResolvedPath,
    ) -> Result<ContainerPayload, MediaError> {
        let metadata = std::fs::metadata(&resolved.canonical)
            .map_err(|_| media_error(MediaErrorCode::NotFound, "コンテナが見つかりません"))?;
        if metadata.is_dir() {
            return self.enumerate_folder(request, resolved);
        }
        if !metadata.is_file() {
            return Err(media_error(
                MediaErrorCode::Unsupported,
                "対象はフォルダまたは ZIP/PDF ファイルではありません",
            ));
        }
        if is_archive_container(resolved) {
            self.enumerate_zip(request, resolved)
        } else if is_pdf_path(&resolved.logical) {
            self.enumerate_pdf(request, resolved, &metadata)
        } else {
            Err(media_error(
                MediaErrorCode::Unsupported,
                "このコンテナ形式には対応していません",
            ))
        }
    }

    fn resume_page_for_items(
        &self,
        container: &RemoteAddress,
        container_path: &Path,
        items: &[crate::grid_item::GridItem],
        resume_supported: bool,
    ) -> Option<RemoteAddress> {
        if !resume_supported {
            return None;
        }
        let Some(reader) = self.resume_reader.as_ref() else {
            return None;
        };
        let saved = match reader.read_book_resume(container_path) {
            Ok(saved) => saved,
            Err(error) => {
                crate::logger::log(format!(
                    "remote_ipc: book resume read failed; falling back to first page error={error:?}"
                ));
                return None;
            }
        };
        let Some(saved) = saved else {
            return None;
        };
        let resolved = resolve_resume_page(container, items, saved);
        if resolved.is_none() {
            crate::logger::log(format!(
                "remote_ipc: saved book resume is outside the current readable pages saved_index={saved} item_count={}",
                items.len()
            ));
        }
        resolved
    }

    fn container_open_mode(&self, auto_open: bool) -> ContainerOpenMode {
        if !auto_open {
            ContainerOpenMode::Grid
        } else if self.settings.book_open_resume.resumes() {
            ContainerOpenMode::ResumePage
        } else {
            ContainerOpenMode::FirstPage
        }
    }

    fn enumerate_folder(
        &self,
        request: &ContainerRequest,
        resolved: &ResolvedPath,
    ) -> Result<ContainerPayload, MediaError> {
        if !matches!(request.address.subresource, RemoteSubresource::File) {
            return Err(media_error(
                MediaErrorCode::BadRequest,
                "画像フォルダの一覧アドレスが不正です",
            ));
        }
        let address = page_identity_from_resolved(resolved, &request.address.subresource);
        let listing = self
            .recompute_folder_listing(&resolved.logical)
            .map_err(media_error_from_remote_write)?;
        let resume_page =
            self.resume_page_for_items(&address, &resolved.logical, &listing.items, true);
        let source_items = listing.items;
        let total = source_items
            .iter()
            .filter(|item| matches!(item, crate::grid_item::GridItem::Image(_)))
            .count();
        let auto_open = listing.image_only && listing.auto_fullscreen_image_folders_enabled;
        let mut budget = ContainerEntryBudget::new(super::REMOTE_LIST_RESPONSE_BUDGET_BYTES);
        let mut items = Vec::new();
        let mut entries = Vec::new();
        let mut byte_truncated = false;
        for item in source_items
            .iter()
            .filter(|item| matches!(item, crate::grid_item::GridItem::Image(_)))
            .take(CONTAINER_ENTRY_LIMIT)
        {
            let Some(entry_address) = grid_item_address(&address, item) else {
                continue;
            };
            let entry = ContainerEntry {
                address: entry_address,
                name: item.name().into_owned(),
                kind: ContainerEntryKind::Image,
                page_count: None,
            };
            if !budget.try_include(&entry) {
                byte_truncated = true;
                break;
            }
            items.push(item.clone());
            entries.push(entry);
        }
        let spread = self.spread_payload(request, resolved, &items, Some(&source_items), None);
        let (entry_limit, truncated) =
            container_limit_metadata(total, entries.len(), byte_truncated);
        Ok(ContainerPayload {
            title: container_title(&resolved.logical),
            root_name: absolute_root_name(&resolved.logical),
            kind: ContainerKind::Folder,
            effective_address: address,
            entries,
            thumb_aspect_height_ratio: super::collections::aggregate_thumb_aspect_height_ratio(
                &self.settings,
            ),
            sort_state: super::remote_grid_sort_state(
                crate::app::BOOK_READING_PAGE_ORDER,
                Some(super::BOOK_SORT_LOCK_REASON),
            ),
            resume_page,
            open_mode: self.container_open_mode(auto_open),
            configured_spread_mode: spread.configured,
            effective_spread_mode: spread.effective,
            reading_direction: spread.reading_direction,
            image_count: spread.image_count,
            video_count: spread.video_count,
            other_count: spread.other_count,
            spread_page_gap_px: self.settings.spread_page_gap_px,
            page_groups: spread.groups,
            entry_limit,
            truncated,
        })
    }

    fn enumerate_zip(
        &self,
        request: &ContainerRequest,
        resolved: &ResolvedPath,
    ) -> Result<ContainerPayload, MediaError> {
        let address = page_identity_from_resolved(resolved, &request.address.subresource);
        let requested_prefix = match &address.subresource {
            RemoteSubresource::File => String::new(),
            RemoteSubresource::ZipDirectory { prefix } => prefix.clone(),
            _ => {
                return Err(media_error(
                    MediaErrorCode::BadRequest,
                    "ZIP の一覧アドレスが不正です",
                ));
            }
        };
        let enumeration_started = Instant::now();
        let enumeration =
            crate::zip_loader::enumerate_image_entries_detailed(resolved.readable_logical())
                .map_err(|error| {
                    crate::logger::log(format!("remote_ipc: zip_enumerate_failed error={error}"));
                    media_error(MediaErrorCode::RenderFailed, "ZIP を列挙できませんでした")
                })?;
        crate::logger::log(format!(
            "remote_ipc: zip_enumerate_complete duration_ms={:.1} raw_entry_count={}",
            enumeration_started.elapsed().as_secs_f64() * 1000.0,
            enumeration.entries.len()
        ));
        let safe_entries = enumeration
            .entries
            .into_iter()
            .filter(|entry| {
                let safe = zip_entry_address(&address, &entry.entry_name)
                    .validate_syntax()
                    .is_ok();
                if !safe {
                    crate::logger::log(
                        "remote_ipc: rejected unsafe ZIP entry during enumeration".to_owned(),
                    );
                }
                safe
            })
            .collect();
        let tree = crate::zip_tree::ZipTree::build(resolved.logical.clone(), safe_entries);
        let requested_segments = zip_prefix_segments(&requested_prefix);
        if tree.node_at(&requested_segments).is_none() {
            return Err(media_error(
                MediaErrorCode::NotFound,
                "ZIP 内の場所が見つかりません",
            ));
        }
        let effective_segments = tree.collapse_redundant(&requested_segments);
        let root_segments = tree.collapse_redundant(&[]);
        let effective_prefix = zip_prefix(&effective_segments);
        let (items, _) =
            tree.materialize_level(&effective_segments, crate::app::BOOK_READING_PAGE_ORDER);
        let total = items.len();
        let at_resume_root = effective_segments == root_segments;
        let resume_page =
            self.resume_page_for_items(&address, &resolved.logical, &items, at_resume_root);
        let mut budget = ContainerEntryBudget::new(super::REMOTE_LIST_RESPONSE_BUDGET_BYTES);
        let mut bounded_items = Vec::new();
        let mut entries = Vec::new();
        let mut byte_truncated = false;
        for item in items.into_iter().take(CONTAINER_ENTRY_LIMIT) {
            let name = item.name().into_owned();
            let entry = match &item {
                crate::grid_item::GridItem::ZipDir { dir_prefix, .. } => ContainerEntry {
                    name,
                    page_count: tree.page_count_for_prefix_str(&dir_prefix),
                    address: RemoteAddress {
                        path: address.path.clone(),
                        subresource: RemoteSubresource::ZipDirectory {
                            prefix: dir_prefix.clone(),
                        },
                    },
                    kind: ContainerEntryKind::Directory,
                },
                crate::grid_item::GridItem::ZipImage { entry_name, .. } => ContainerEntry {
                    name,
                    page_count: None,
                    address: zip_entry_address(&address, entry_name),
                    kind: ContainerEntryKind::Image,
                },
                _ => continue,
            };
            if !budget.try_include(&entry) {
                byte_truncated = true;
                break;
            }
            bounded_items.push(item);
            entries.push(entry);
        }
        let items = bounded_items;
        let spread = self.spread_payload(
            request,
            resolved,
            &items,
            None,
            Some((&effective_segments, &resolved.logical)),
        );
        let (entry_limit, truncated) =
            container_limit_metadata(total, entries.len(), byte_truncated);
        Ok(ContainerPayload {
            title: container_title(&resolved.logical),
            root_name: absolute_root_name(&resolved.logical),
            kind: ContainerKind::Zip,
            effective_address: RemoteAddress {
                path: address.path.clone(),
                subresource: if effective_prefix.is_empty() {
                    RemoteSubresource::File
                } else {
                    RemoteSubresource::ZipDirectory {
                        prefix: effective_prefix,
                    }
                },
            },
            entries,
            thumb_aspect_height_ratio: super::collections::aggregate_thumb_aspect_height_ratio(
                &self.settings,
            ),
            sort_state: super::remote_grid_sort_state(
                crate::app::BOOK_READING_PAGE_ORDER,
                Some(super::BOOK_SORT_LOCK_REASON),
            ),
            resume_page,
            open_mode: self.container_open_mode(
                at_resume_root && self.settings.effective_auto_fullscreen_zip_pdf(),
            ),
            configured_spread_mode: spread.configured,
            effective_spread_mode: spread.effective,
            reading_direction: spread.reading_direction,
            image_count: spread.image_count,
            video_count: spread.video_count,
            other_count: spread.other_count,
            spread_page_gap_px: self.settings.spread_page_gap_px,
            page_groups: spread.groups,
            entry_limit,
            truncated,
        })
    }

    fn enumerate_pdf(
        &self,
        request: &ContainerRequest,
        resolved: &ResolvedPath,
        metadata: &std::fs::Metadata,
    ) -> Result<ContainerPayload, MediaError> {
        let address = page_identity_from_resolved(resolved, &request.address.subresource);
        if !matches!(address.subresource, RemoteSubresource::File) {
            return Err(media_error(
                MediaErrorCode::BadRequest,
                "PDF の一覧アドレスが不正です",
            ));
        }
        let page_count = self.pdf_page_count(resolved, metadata)?;
        let mut budget = ContainerEntryBudget::new(super::REMOTE_LIST_RESPONSE_BUDGET_BYTES);
        let mut items = Vec::new();
        let mut entries = Vec::new();
        let mut byte_truncated = false;
        for page_number in (0..page_count).take(CONTAINER_ENTRY_LIMIT) {
            let item = crate::grid_item::GridItem::PdfPage {
                pdf_path: resolved.logical.clone(),
                page_num: page_number,
                content_type: None,
            };
            let entry = ContainerEntry {
                address: RemoteAddress {
                    path: address.path.clone(),
                    subresource: RemoteSubresource::PdfPage { page_number },
                },
                name: format!("Page {}", page_number + 1),
                kind: ContainerEntryKind::Image,
                page_count: None,
            };
            if !budget.try_include(&entry) {
                byte_truncated = true;
                break;
            }
            items.push(item);
            entries.push(entry);
        }
        let resume_page = self.resume_page_for_items(&address, &resolved.logical, &items, true);
        let spread = self.spread_payload(request, resolved, &items, None, None);
        let (entry_limit, truncated) =
            container_limit_metadata(page_count as usize, entries.len(), byte_truncated);
        Ok(ContainerPayload {
            title: container_title(&resolved.logical),
            root_name: absolute_root_name(&resolved.logical),
            kind: ContainerKind::Pdf,
            effective_address: address,
            entries,
            thumb_aspect_height_ratio: super::collections::aggregate_thumb_aspect_height_ratio(
                &self.settings,
            ),
            sort_state: super::remote_grid_sort_state(
                crate::app::BOOK_READING_PAGE_ORDER,
                Some(super::BOOK_SORT_LOCK_REASON),
            ),
            resume_page,
            open_mode: self.container_open_mode(self.settings.effective_auto_fullscreen_zip_pdf()),
            configured_spread_mode: spread.configured,
            effective_spread_mode: spread.effective,
            reading_direction: spread.reading_direction,
            image_count: spread.image_count,
            video_count: spread.video_count,
            other_count: spread.other_count,
            spread_page_gap_px: self.settings.spread_page_gap_px,
            page_groups: spread.groups,
            entry_limit,
            truncated,
        })
    }

    fn spread_payload(
        &self,
        request: &ContainerRequest,
        resolved: &ResolvedPath,
        items: &[crate::grid_item::GridItem],
        source_items: Option<&[crate::grid_item::GridItem]>,
        zip_context: Option<(&[String], &Path)>,
    ) -> SpreadPayload {
        let key = if let Some((segments, root)) = zip_context {
            crate::spread_db::container_key_with_fallback(root, segments)
        } else {
            crate::spread_db::container_key_with_fallback(&resolved.logical, &[])
        };
        let (stored_mode, stored_direction) =
            self.stored_spread_state(&key.exact, key.fallback.as_deref());
        let source_items = source_items.unwrap_or(items);
        let defaults = if crate::app::physical_page_order_locked(
            &self.settings,
            &resolved.logical,
            source_items,
        ) {
            crate::app::SpreadRestoreDefaults::for_book(&self.settings)
        } else {
            crate::app::SpreadRestoreDefaults::NON_BOOK
        };
        let (configured, effective, reading_direction) = resolve_spread_state(
            request.spread_mode,
            request.reading_direction,
            stored_mode,
            stored_direction,
            defaults.spread_mode(),
            defaults.reading_direction(),
            request.force_single_page,
        );
        let (image_count, video_count, other_count) =
            crate::ui_fullscreen::seek_overlay_media_counts(source_items);
        // Single は横長判定を一切見ない。**見ないものを作らない** (PDF は寸法の取得に
        // worker 往復が要るので、開くたびに払うと無駄になる)。
        let landscape = if effective == RemoteSpreadMode::Single {
            vec![false; items.len()]
        } else {
            self.cached_landscape_flags(&resolved.logical, items)
        };
        let index_groups = crate::ui_fullscreen::build_remote_spread_page_groups(
            items,
            core_spread_mode(effective),
            &landscape,
        );
        let groups = index_groups
            .into_iter()
            .filter_map(|group| {
                let container_address =
                    page_identity_from_resolved(resolved, &request.address.subresource);
                let pages = group
                    .indices
                    .into_iter()
                    .filter_map(|index| grid_item_address(&container_address, items.get(index)?))
                    .collect::<Vec<_>>();
                let anchor = if effective.is_rtl() && pages.len() == 2 {
                    pages.get(1).cloned()
                } else {
                    pages.first().cloned()
                }?;
                Some(PageGroup {
                    anchor,
                    pages,
                    slice: crate::ui_fullscreen::remote_page_slice(group.slice),
                })
            })
            .collect::<Vec<_>>();
        SpreadPayload {
            configured,
            effective,
            reading_direction,
            image_count,
            video_count,
            other_count,
            groups,
        }
    }

    fn stored_spread_state(
        &self,
        key: &Path,
        fallback: Option<&Path>,
    ) -> (
        Option<crate::settings::SpreadMode>,
        Option<crate::settings::ReadingDirection>,
    ) {
        let db = self
            .spread_db
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let stored = db
            .as_ref()
            .map(|db| db.get_state_with_fallback(key, fallback))
            .unwrap_or_default();
        (stored.mode, stored.direction)
    }

    fn remote_view_trim_plan(
        &self,
        page_address: &RemoteAddress,
        resolved: &ResolvedPath,
        render_context: Option<&RemotePageRenderContext>,
        rotation: crate::rotation_db::Rotation,
    ) -> Result<RemoteViewTrimPlan, MediaError> {
        self.remote_view_trim_plan_timed(page_address, resolved, render_context, rotation, None)
    }

    fn remote_view_trim_plan_timed(
        &self,
        page_address: &RemoteAddress,
        resolved: &ResolvedPath,
        render_context: Option<&RemotePageRenderContext>,
        rotation: crate::rotation_db::Rotation,
        wait_stage: Option<&mut RemotePageStageGuard>,
    ) -> Result<RemoteViewTrimPlan, MediaError> {
        // Keep the existing page/context ownership validation even when rotation makes the trim
        // result empty. Only the DB lookup and Auto source/partner work are skipped.
        let keys = self.remote_view_trim_keys(page_address, resolved, render_context)?;
        // Desktop display geometry drops the effective content bbox for every saved rotation.
        // Resolve that invariant at plan creation so Auto trim cannot start source/partner work
        // whose result is forbidden from reaching the rotated page.
        if !rotation.is_none() {
            return Ok(RemoteViewTrimPlan::Stored(None));
        }
        let page_key = crate::edit_source::page_key_for_remote(
            &resolved.logical,
            &page_address.subresource,
        )
        .ok_or_else(|| media_error(MediaErrorCode::BadRequest, "表示トリム対象が不正です"))?;
        let db = lock_with_remote_page_wait(&self.view_trim_db, wait_stage, None);
        let state = db
            .as_ref()
            .and_then(|db| db.get_book_state(&keys.exact))
            .or_else(|| {
                keys.fallback
                    .as_deref()
                    .and_then(|fallback| db.as_ref().and_then(|db| db.get_book_state(fallback)))
            })
            .unwrap_or_default();
        let page_override = db.as_ref().and_then(|db| db.get_page_override(&page_key));
        drop(db);
        let legacy_margin_fit = matches!(
            self.settings.fullscreen_fit_mode,
            crate::settings::FullscreenFitMode::MarginFit
        ) || self.settings.margin_fit_enabled;
        let base_mode = crate::view_trim::effective_view_trim_base_apply_mode(
            state.apply_mode,
            legacy_margin_fit,
        );
        let mode = crate::view_trim::effective_view_trim_apply_mode(base_mode, page_override);
        let spread_side = render_context.and_then(|context| match context.display_slot {
            RemotePageDisplaySlot::Single => None,
            RemotePageDisplaySlot::SpreadLeft => Some(crate::view_trim::ViewTrimSpreadSide::Left),
            RemotePageDisplaySlot::SpreadRight => Some(crate::view_trim::ViewTrimSpreadSide::Right),
        });
        if matches!(mode, crate::view_trim::ViewTrimApplyMode::Auto) {
            let Some(side) = spread_side else {
                return Ok(RemoteViewTrimPlan::AutoSingle);
            };
            let Some(partner) = render_context.and_then(|context| context.spread_partner.clone())
            else {
                return Ok(RemoteViewTrimPlan::AutoSingle);
            };
            if partner == *page_address {
                return Err(media_error(
                    MediaErrorCode::BadRequest,
                    "見開き Auto の相手ページが現在ページと同じです",
                ));
            }
            let partner_resolved = self.resolve(&partner)?;
            self.remote_view_trim_keys(&partner, &partner_resolved, render_context)?;
            return Ok(RemoteViewTrimPlan::AutoSpread { side, partner });
        }
        Ok(RemoteViewTrimPlan::Stored(
            crate::view_trim::stored_view_trim_bbox(
                mode,
                state.book_settings,
                page_override,
                spread_side,
            ),
        ))
    }

    fn prepare_remote_auto_trim_partner_timed(
        &self,
        plan: &RemoteViewTrimPlan,
        target_px: u32,
        cancel: &AtomicBool,
        wait_stage: Option<&mut RemotePageStageGuard>,
    ) -> Result<RemotePartnerStart<Option<egui::Rect>, RemotePartnerAutoTrimRequest>, MediaError>
    {
        let Some(partner) = plan.spread_partner() else {
            return Ok(RemotePartnerStart::NotRequired);
        };
        let resolved = self.resolve(partner)?;
        if let Some(bbox) = self.remote_auto_trim_bbox_cache_lookup_timed(
            partner, &resolved, target_px, cancel, wait_stage,
        )? {
            return Ok(RemotePartnerStart::Cached(bbox));
        }
        Ok(RemotePartnerStart::Resolve(RemotePartnerAutoTrimRequest {
            address: partner.clone(),
            resolved,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn complete_remote_view_trim_bbox(
        &self,
        plan: &RemoteViewTrimPlan,
        current_auto_trim_bbox: Option<egui::Rect>,
        target_px: u32,
        foreground: bool,
        context: &WorkerContext,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Option<egui::Rect>, MediaError> {
        self.complete_remote_view_trim_bbox_timed(
            plan,
            current_auto_trim_bbox,
            target_px,
            foreground,
            context,
            cancel,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn complete_remote_view_trim_bbox_timed(
        &self,
        plan: &RemoteViewTrimPlan,
        current_auto_trim_bbox: Option<egui::Rect>,
        target_px: u32,
        foreground: bool,
        context: &WorkerContext,
        cancel: &Arc<AtomicBool>,
        mut wait_stage: Option<&mut RemotePageStageGuard>,
    ) -> Result<Option<egui::Rect>, MediaError> {
        let partner = match plan {
            RemoteViewTrimPlan::AutoSpread { partner, .. } => {
                // 相手 request を待たない。heavy worker が 1 本でも進められるよう、cache miss
                // なら現在の worker が同じ cancel token で相手 raw raster を解決する。
                let partner_resolved = self.resolve(partner)?;
                let partner_auto_trim_bbox = self.remote_auto_trim_bbox_timed(
                    partner,
                    &partner_resolved,
                    target_px,
                    foreground,
                    context,
                    cancel,
                    wait_stage.as_deref_mut(),
                )?;
                RemotePartnerResult::Resolved(partner_auto_trim_bbox)
            }
            RemoteViewTrimPlan::Stored(_) | RemoteViewTrimPlan::AutoSingle => {
                RemotePartnerResult::NotRequired
            }
        };
        Ok(complete_remote_view_trim_bbox_from_partner(
            plan,
            current_auto_trim_bbox,
            partner,
        ))
    }

    fn remote_auto_trim_bbox(
        &self,
        address: &RemoteAddress,
        resolved: &ResolvedPath,
        target_px: u32,
        foreground: bool,
        context: &WorkerContext,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Option<egui::Rect>, MediaError> {
        self.remote_auto_trim_bbox_timed(
            address, resolved, target_px, foreground, context, cancel, None,
        )
    }

    fn remote_auto_trim_bbox_cache_lookup_timed(
        &self,
        address: &RemoteAddress,
        resolved: &ResolvedPath,
        target_px: u32,
        cancel: &AtomicBool,
        wait_stage: Option<&mut RemotePageStageGuard>,
    ) -> Result<Option<Option<egui::Rect>>, MediaError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(media_error(
                MediaErrorCode::Busy,
                "ページ要求は新しい処理に置き換えられました",
            ));
        }
        let metadata = std::fs::metadata(&resolved.canonical)
            .map_err(|_| media_error(MediaErrorCode::NotFound, "コンテナが見つかりません"))?;
        if !metadata.is_file() {
            return Err(media_error(
                MediaErrorCode::Unsupported,
                "対象はコンテナファイルではありません",
            ));
        }
        let key = remote_auto_trim_cache_key(
            address,
            resolved,
            crate::ui_helpers::mtime_secs(&metadata),
            i64::try_from(metadata.len()).unwrap_or(i64::MAX),
            target_px,
        )?;
        Ok(lock_with_remote_page_wait(&self.auto_trim_bbox_cache, wait_stage, None).get(&key))
    }

    #[allow(clippy::too_many_arguments)]
    fn remote_auto_trim_bbox_timed(
        &self,
        address: &RemoteAddress,
        resolved: &ResolvedPath,
        target_px: u32,
        foreground: bool,
        context: &WorkerContext,
        cancel: &Arc<AtomicBool>,
        mut wait_stage: Option<&mut RemotePageStageGuard>,
    ) -> Result<Option<egui::Rect>, MediaError> {
        if let Some(bbox) = self.remote_auto_trim_bbox_cache_lookup_timed(
            address,
            resolved,
            target_px,
            cancel,
            wait_stage.as_deref_mut(),
        )? {
            return Ok(bbox);
        }
        self.load_image_timed(
            address,
            resolved,
            target_px,
            RemoteImageLoadKind::AutoTrimReference,
            crate::rotation_db::Rotation::None,
            foreground,
            context,
            Some(cancel),
            None,
            None,
            wait_stage,
        )
        .map(|loaded| loaded.auto_trim_bbox)
    }

    fn remote_view_trim_keys(
        &self,
        page_address: &RemoteAddress,
        resolved: &ResolvedPath,
        render_context: Option<&RemotePageRenderContext>,
    ) -> Result<crate::spread_db::SpreadContainerKey, MediaError> {
        let Some(render_context) = render_context else {
            let root = match page_address.subresource {
                RemoteSubresource::File => resolved.logical.parent().ok_or_else(|| {
                    media_error(MediaErrorCode::PathRejected, "本の場所を解決できません")
                })?,
                RemoteSubresource::ZipEntry { .. } | RemoteSubresource::PdfPage { .. } => {
                    resolved.logical.as_path()
                }
                RemoteSubresource::ZipDirectory { .. } => {
                    return Err(media_error(
                        MediaErrorCode::BadRequest,
                        "コンテナ自体は表示トリム対象ではありません",
                    ));
                }
            };
            return Ok(crate::spread_db::container_key_with_fallback(root, &[]));
        };
        let context_address = &render_context.context_address;
        let context = self.resolve(context_address)?;
        match (&page_address.subresource, &context_address.subresource) {
            (RemoteSubresource::File, RemoteSubresource::File)
                if std::fs::metadata(&context.canonical)
                    .is_ok_and(|metadata| metadata.is_dir())
                    && resolved.canonical.parent() == Some(context.canonical.as_path()) =>
            {
                Ok(crate::spread_db::container_key_with_fallback(
                    &context.logical,
                    &[],
                ))
            }
            (
                RemoteSubresource::ZipEntry { entry_name },
                RemoteSubresource::File | RemoteSubresource::ZipDirectory { .. },
            ) if resolved.canonical == context.canonical => {
                let segments = match &context_address.subresource {
                    RemoteSubresource::ZipDirectory { prefix } => zip_prefix_segments(prefix),
                    _ => Vec::new(),
                };
                let effective_prefix = zip_prefix(&segments);
                if !effective_prefix.is_empty() && !entry_name.starts_with(&effective_prefix) {
                    return Err(media_error(
                        MediaErrorCode::BadRequest,
                        "ZIP ページと表示コンテキストが一致しません",
                    ));
                }
                Ok(crate::spread_db::container_key_with_fallback(
                    &resolved.logical,
                    &segments,
                ))
            }
            (RemoteSubresource::PdfPage { .. }, RemoteSubresource::File)
                if resolved.canonical == context.canonical =>
            {
                Ok(crate::spread_db::container_key_with_fallback(
                    &resolved.logical,
                    &[],
                ))
            }
            _ => Err(media_error(
                MediaErrorCode::BadRequest,
                "ページと表示コンテキストが一致しません",
            )),
        }
    }

    /// カタログに寸法が無い PDF ページがあるときだけ、文書の全ページ寸法を取りに行く。
    ///
    /// **1 文書につき 1 往復。**ページを読み込まない PDFium の API なので、レンダリング
    /// 1 枚より安い。カタログが揃っている間は呼ばないので、既にサムネイルを作った PDF に
    /// 追加費用は出ない。取得できなければ従来どおり「寸法不明」として扱う (件数はログへ)。
    fn pdf_page_sizes_if_needed(
        &self,
        container_path: &Path,
        items: &[crate::grid_item::GridItem],
        cached: &std::collections::HashMap<String, Option<(u32, u32)>>,
    ) -> Option<Vec<(u32, u32)>> {
        if !pdf_page_sizes_needed(items, cached) {
            return None;
        }
        let password = self.pdf_passwords.get(container_path);
        match crate::pdf_loader::get_page_sizes(container_path, password.as_deref()) {
            Ok(sizes) => Some(
                sizes
                    .into_iter()
                    // 縦横比だけを見る。points をそのまま丸めて幅・高さとして扱う。
                    .map(|(width, height)| {
                        (
                            width.max(0.0).round() as u32,
                            height.max(0.0).round() as u32,
                        )
                    })
                    .collect(),
            ),
            Err(error) => {
                crate::logger::log(format!(
                    "remote_ipc: pdf page sizes unavailable path={} error={error}",
                    container_path.display()
                ));
                None
            }
        }
    }

    fn cached_landscape_flags(
        &self,
        container_path: &Path,
        items: &[crate::grid_item::GridItem],
    ) -> Vec<bool> {
        // 寸法列だけを引く。`load_all` は thumbnail の blob も運ぶので、横長かどうかを
        // 知るためだけに 5 万枚で 1.7 GB を確保して即捨てることになる。
        let catalog = crate::catalog::CatalogDb::open_existing_read_only(
            &crate::catalog::default_cache_dir(),
            container_path,
        )
        .ok()
        .flatten();
        let cached = catalog
            .as_ref()
            .and_then(|catalog| catalog.load_source_dims().ok())
            .unwrap_or_default();
        // PDF はカタログが無くても寸法を取れる。ページを読み込まない PDFium の API を
        // **文書ごとに 1 往復だけ**呼ぶ。カタログに寸法がある間は呼ばない。
        let pdf_page_sizes = self.pdf_page_sizes_if_needed(container_path, items, &cached);
        let rotation_keys = items
            .iter()
            .map(crate::edit_source::page_key_for_grid_item)
            .collect::<Vec<_>>();
        // ページ画像と同じ既存キーを使い、全 item 分を 1 回の chunked query で読む。
        // DB が無い / 開けない場合は従来の回転なしとして扱う。
        let rotations = crate::rotation_db::RotationDb::open_readonly()
            .ok()
            .map(|db| db.get_many(rotation_keys.iter().filter_map(|key| key.as_deref())))
            .unwrap_or_default();
        let mut pages = 0usize;
        let mut unknown = 0usize;
        let flags = items
            .iter()
            .zip(rotation_keys)
            .map(|(item, rotation_key)| {
                let key = match item {
                    crate::grid_item::GridItem::Image(path) => path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned),
                    crate::grid_item::GridItem::ZipImage { entry_name, .. } => {
                        Some(entry_name.clone())
                    }
                    crate::grid_item::GridItem::PdfPage { page_num, .. } => {
                        Some(crate::grid_item::pdf_page_cache_key(*page_num))
                    }
                    _ => None,
                };
                if key.is_some() {
                    pages += 1;
                }
                let dims = key.and_then(|key| {
                    // A present-but-empty value is a row saved before the dimension columns
                    // existed, and only those are worth paying for a thumbnail read to recover.
                    match cached.get(&key) {
                        Some(recorded) => recorded.or_else(|| {
                            catalog
                                .as_ref()
                                .and_then(|catalog| catalog.load_one(&key).ok().flatten())
                                .and_then(|entry| {
                                    crate::catalog::decode_thumb_dims(&entry.jpeg_data)
                                })
                        }),
                        // カタログ行が 1 つも無い場合。カタログに依らない経路で補う。
                        None => match item {
                            crate::grid_item::GridItem::PdfPage { page_num, .. } => pdf_page_sizes
                                .as_ref()
                                .and_then(|sizes| sizes.get(*page_num as usize))
                                .copied(),
                            _ => page_dims_without_catalog(item),
                        },
                    }
                });
                if dims.is_none() {
                    unknown += 1;
                }
                // Catalog dimensions are a layout/aspect space. PDF page-box values are valid
                // here because only their ratio is observed; they are not an edit coordinate.
                dims.is_some_and(|(width, height)| {
                    let rotation = rotation_key
                        .as_ref()
                        .and_then(|key| rotations.get(key))
                        .copied()
                        .unwrap_or(crate::rotation_db::Rotation::None);
                    crate::rotation_db::landscape_after_rotation(width, height, rotation)
                })
            })
            .collect();
        // 寸法が分からないページは「横長ではない」として扱うしかないが、**黙って
        // 見開き単独表示も横長分割も効かなくなる**。原因を推測から始めずに済むよう、
        // 件数を残す。全ページ不明ならカタログもヘッダ読みも当たっていない。
        if unknown > 0 {
            crate::logger::log(format!(
                "remote_ipc: page dims unknown pages={unknown}/{pages}                  (landscape detection is off for those pages)"
            ));
        }
        flags
    }

    fn load_image(
        &self,
        address: &RemoteAddress,
        resolved: &ResolvedPath,
        target_px: u32,
        load_kind: RemoteImageLoadKind,
        rotation: crate::rotation_db::Rotation,
        foreground: bool,
        context: &WorkerContext,
        external_cancel: Option<&Arc<AtomicBool>>,
        adjustment_preview: Option<&mimageviewer_ipc::RemoteAdjustmentPreview>,
    ) -> Result<LoadedImage, MediaError> {
        self.load_image_timed(
            address,
            resolved,
            target_px,
            load_kind,
            rotation,
            foreground,
            context,
            external_cancel,
            adjustment_preview,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn load_image_timed(
        &self,
        address: &RemoteAddress,
        resolved: &ResolvedPath,
        target_px: u32,
        load_kind: RemoteImageLoadKind,
        rotation: crate::rotation_db::Rotation,
        foreground: bool,
        context: &WorkerContext,
        external_cancel: Option<&Arc<AtomicBool>>,
        adjustment_preview: Option<&mimageviewer_ipc::RemoteAdjustmentPreview>,
        mut page_timing: Option<RemotePageLoadTiming>,
        mut ambient_wait_stage: Option<&mut RemotePageStageGuard>,
    ) -> Result<LoadedImage, MediaError> {
        let full_page = load_kind.full_page();
        let compose_full_page = load_kind.composes_page();
        let detect_auto_trim = load_kind.detects_auto_trim();
        let page_perf = page_timing.as_ref().map(|timing| timing.perf.clone());
        if target_px == 0 || target_px > MAX_PAGE_RENDER_PX {
            return Err(media_error(
                MediaErrorCode::BadRequest,
                "画像サイズが範囲外です",
            ));
        }
        let metadata = std::fs::metadata(&resolved.canonical)
            .map_err(|_| media_error(MediaErrorCode::NotFound, "コンテナが見つかりません"))?;
        if !metadata.is_file() {
            return Err(media_error(
                MediaErrorCode::Unsupported,
                "対象はコンテナファイルではありません",
            ));
        }
        let mtime = crate::ui_helpers::mtime_secs(&metadata);
        let file_size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
        let mut request = crate::thumb_loader::LoadRequest {
            path: resolved.readable_logical().to_path_buf(),
            mtime,
            file_size,
            source_policy: if full_page {
                crate::thumb_loader::LoadSourcePolicy::SourceOnly
            } else {
                crate::thumb_loader::LoadSourcePolicy::CacheOrSource
            },
            // foreground でも HighNormal までとし、ローカル UI 用 Critical 予約枠は
            // 消費しない。prefetch は Normal lane へ分離する。
            priority: foreground,
            context_epoch: 0,
            ..Default::default()
        };
        let catalog_folder = match &address.subresource {
            RemoteSubresource::File if is_archive_container(resolved) => {
                request.cache_key_override = Some(container_thumb_key(
                    crate::thumb_loader::CACHE_KEY_ZIP,
                    &resolved.logical,
                )?);
                resolved.logical.parent().ok_or_else(|| {
                    media_error(MediaErrorCode::PathRejected, "親フォルダを解決できません")
                })?
            }
            RemoteSubresource::File if is_pdf_path(&resolved.logical) => {
                self.ensure_pdf_page_in_range_timed(
                    resolved,
                    &metadata,
                    0,
                    page_timing.as_mut().map(|timing| &mut timing.resolve),
                    ambient_wait_stage.as_deref_mut(),
                )?;
                request.pdf_page = Some(0);
                request.pdf_password = self.pdf_passwords.get(&resolved.logical);
                request.cache_key_override = Some(container_thumb_key(
                    crate::thumb_loader::CACHE_KEY_PDF,
                    &resolved.logical,
                )?);
                resolved.logical.parent().ok_or_else(|| {
                    media_error(MediaErrorCode::PathRejected, "親フォルダを解決できません")
                })?
            }
            RemoteSubresource::ZipEntry { entry_name } if is_archive_container(resolved) => {
                request.zip_entry = Some(entry_name.clone());
                &resolved.logical
            }
            RemoteSubresource::ZipDirectory { prefix } if is_archive_container(resolved) => {
                request.zip_dir_prefix = Some(prefix.clone());
                request.cache_key_override = Some(crate::grid_item::zipdir_cache_key(prefix));
                request.folder_thumb_sort = Some(crate::app::BOOK_READING_PAGE_ORDER);
                &resolved.logical
            }
            RemoteSubresource::PdfPage { page_number } if is_pdf_path(&resolved.logical) => {
                self.ensure_pdf_page_in_range_timed(
                    resolved,
                    &metadata,
                    *page_number,
                    page_timing.as_mut().map(|timing| &mut timing.resolve),
                    ambient_wait_stage.as_deref_mut(),
                )?;
                request.pdf_page = Some(*page_number);
                request.pdf_password = self.pdf_passwords.get(&resolved.logical);
                &resolved.logical
            }
            RemoteSubresource::File if is_image_path(&resolved.logical) => {
                resolved.logical.parent().ok_or_else(|| {
                    media_error(MediaErrorCode::PathRejected, "親フォルダを解決できません")
                })?
            }
            RemoteSubresource::File => {
                return Err(media_error(
                    MediaErrorCode::Unsupported,
                    "対象は画像または ZIP/PDF ではありません",
                ));
            }
            _ => {
                return Err(media_error(
                    MediaErrorCode::BadRequest,
                    "コンテナ種別と内部アドレスが一致しません",
                ));
            }
        };
        // identity は HTTP 要求値の echo ではなく、この描画要求が実際に使う
        // resolved.logical と subresource から画素生成境界で再構成する。
        let identity =
            super::path_guard::page_identity_from_resolved(resolved, &address.subresource);

        let prepared_composite = if compose_full_page {
            self.prepare_remote_composite_timed(
                address,
                &resolved.logical,
                mtime,
                file_size,
                target_px,
                rotation,
                adjustment_preview,
                context,
                page_timing.as_mut().map(|timing| &mut timing.resolve),
                ambient_wait_stage.as_deref_mut(),
            )?
        } else {
            None
        };
        let auto_trim_key = if detect_auto_trim {
            Some(remote_auto_trim_cache_key(
                address, resolved, mtime, file_size, target_px,
            )?)
        } else {
            None
        };
        let cached_auto_trim_bbox = auto_trim_key.as_ref().and_then(|key| {
            lock_with_remote_page_wait(
                &self.auto_trim_bbox_cache,
                page_timing.as_mut().map(|timing| &mut timing.resolve),
                ambient_wait_stage.as_deref_mut(),
            )
            .get(key)
        });
        let mut cached_composite_pixels = None;
        if let Some(prepared) = prepared_composite.as_ref()
            && let Some(pixels) = lock_with_remote_page_wait(
                &self.page_composite_cache,
                page_timing.as_mut().map(|timing| &mut timing.resolve),
                ambient_wait_stage.as_deref_mut(),
            )
            .get(&prepared.key)
        {
            if external_cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
                return Err(media_error(
                    MediaErrorCode::Busy,
                    "先読みは新しいページ要求に置き換えられました",
                ));
            }
            crate::logger::log(format!(
                "remote_ipc: final_composite cache=hit key={}",
                prepared.key.page_key
            ));
            if !detect_auto_trim || cached_auto_trim_bbox.is_some() {
                if let Some(mut timing) = page_timing.take() {
                    timing.resolve.finish(remote_page_file_metrics(file_size));
                }
                if let Some(perf) = page_perf.as_ref() {
                    let metrics = RemotePageStageMetrics::buffer(pixels.size[0], pixels.size[1], 4);
                    perf.record_skipped(RemotePageStage::Source, metrics, "composite_cache_hit");
                }
                let mut compose_stage = page_perf
                    .as_ref()
                    .and_then(|perf| perf.enter(RemotePageStage::Compose));
                let result = loaded_image_from_color_image(
                    &pixels,
                    cached_auto_trim_bbox.flatten(),
                    identity,
                );
                if let Some(stage) = compose_stage.as_mut() {
                    stage.finish_with_outcome(
                        RemotePageStageMetrics::buffer(pixels.size[0], pixels.size[1], 4),
                        "composite_cache_hit",
                    );
                }
                return result;
            }
            // Auto bbox だけが未計算なら raw raster を復号するが、補正済み pixels は保持し、
            // 後段の edit / final composite は再実行しない。
            cached_composite_pixels = Some(pixels);
        }

        let catalog = Arc::new(
            crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), catalog_folder)
                .map_err(|error| {
                crate::logger::log(format!(
                    "remote_ipc: container catalog open failed: {error}"
                ));
                media_error(
                    MediaErrorCode::Internal,
                    "サムネイルカタログを開けませんでした",
                )
            })?,
        );
        let cache_map = Arc::new(RwLock::new(HashMap::new()));
        if !full_page
            && let Some(key) = crate::thumb_loader::cache_key_for_request(&request)
            && let Ok(Some(entry)) = catalog.load_one(key.as_ref())
            && let Ok(mut map) = cache_map.write()
        {
            map.insert(key.into_owned(), entry);
        }

        if let Some(mut timing) = page_timing.take() {
            timing.resolve.finish(remote_page_file_metrics(file_size));
        }
        let mut source_stage = page_perf
            .as_ref()
            .and_then(|perf| perf.enter(RemotePageStage::Source));

        let cancel = external_cancel
            .cloned()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let source_identity =
            RemoteSourceDecodeIdentity::from_load_request(&request, target_px, full_page);
        let pdf_password = request.pdf_password.clone();
        let cache_decision = remote_page_cache_decision(full_page, &self.settings);
        let zip_directory = matches!(address.subresource, RemoteSubresource::ZipDirectory { .. });
        let stats = Arc::clone(&self.stats);
        let thumb_px = self.settings.thumb_px;
        let thumb_quality = self.settings.thumb_quality;
        let source_load = self.source_single_flight.load(
            source_identity,
            Arc::clone(&cancel),
            move |shared_cancel| {
                decode_remote_source(
                    request,
                    cache_map,
                    catalog,
                    thumb_px,
                    thumb_quality,
                    target_px,
                    cache_decision,
                    stats,
                    shared_cancel,
                    zip_directory,
                )
            },
        )?;

        if let Some(stage) = source_stage.as_mut() {
            if source_load.load_request_ms > 0.0 {
                stage.add_phase("load_request_ms", source_load.load_request_ms);
            }
            if source_load.drain_ms > 0.0 {
                stage.add_phase("drain_ms", source_load.drain_ms);
            }
            if source_load.singleflight_wait_ms > 0.0 {
                stage.add_phase("singleflight_wait_ms", source_load.singleflight_wait_ms);
            }
        }
        let source_outcome = source_load.outcome;
        let RemoteSourceRaster {
            pixels: color_image,
            decoded_source_dims,
        } = source_load.raster;
        if let Some(stage) = source_stage.as_mut() {
            stage.finish_with_outcome(
                RemotePageStageMetrics::buffer(color_image.size[0], color_image.size[1], 4),
                source_outcome.as_str(),
            );
        }
        drop(source_stage);
        let mut compose_stage = page_perf
            .as_ref()
            .and_then(|perf| perf.enter(RemotePageStage::Compose));
        let requires_stored_edit_space = prepared_composite.as_ref().is_some_and(|prepared| {
            !prepared.edits.comic.is_empty() || prepared.edits.export_crop.is_some()
        });
        let (pdf_canonical_raster_dims, unavailable_edit_space_reason) =
            if requires_stored_edit_space {
                match &address.subresource {
                    RemoteSubresource::PdfPage { page_number } => {
                        let edit_space_started = Instant::now();
                        let analysis_result = crate::pdf_loader::analyze_page_content_type(
                            &resolved.canonical,
                            *page_number,
                            pdf_password.as_deref(),
                            Some(Arc::clone(&cancel)),
                        );
                        if let Some(stage) = compose_stage.as_mut() {
                            stage.phase_from("edit_space", edit_space_started);
                        }
                        match analysis_result {
                            Ok(analysis) => match crate::pdf_loader::canonical_pdf_raster_dims(
                                analysis.content_type,
                            ) {
                                Some([width, height]) => {
                                    (Some([width as usize, height as usize]), None)
                                }
                                None => (
                                    None,
                                    Some(StoredEditSkipReason::PdfVectorHasNoCanonicalRaster),
                                ),
                            },
                            Err(error) => {
                                crate::logger::log(format!(
                                    "remote_ipc: PDF canonical edit space analysis failed: {error}"
                                ));
                                (None, Some(StoredEditSkipReason::PdfCanonicalAnalysisFailed))
                            }
                        }
                    }
                    _ => (None, None),
                }
            } else {
                (None, None)
            };
        let stored_edit_space = StoredEditSpace::for_remote_source(
            &address.subresource,
            color_image.size,
            decoded_source_dims.map(|(width, height)| [width as usize, height as usize]),
            pdf_canonical_raster_dims,
        );
        if requires_stored_edit_space && stored_edit_space.is_none() {
            if let RemoteSubresource::PdfPage { page_number } = &address.subresource {
                let prepared = prepared_composite
                    .as_ref()
                    .expect("stored edits require a prepared composite");
                record_skipped_stored_edits(SkippedStoredEdits {
                    pipeline: StoredEditPipeline::Page,
                    page_number: *page_number,
                    reason: unavailable_edit_space_reason
                        .unwrap_or(StoredEditSkipReason::PdfCanonicalRasterUnavailable),
                    crop: prepared.edits.export_crop.is_some(),
                    comic_objects: prepared.edits.comic.len(),
                });
            }
        }
        let auto_trim_bbox = match cached_auto_trim_bbox {
            Some(bbox) => bbox,
            None if detect_auto_trim => {
                let auto_trim_started = Instant::now();
                let bbox = crate::margin_fit::detect_content_bbox(
                    &color_image,
                    crate::margin_fit::DEFAULT_TOLERANCE,
                );
                if let Some(stage) = compose_stage.as_mut() {
                    stage.phase_from("auto_trim", auto_trim_started);
                }
                if let Some(key) = auto_trim_key {
                    lock_with_remote_page_wait(
                        &self.auto_trim_bbox_cache,
                        compose_stage.as_mut(),
                        ambient_wait_stage.as_deref_mut(),
                    )
                    .insert(key, bbox);
                }
                bbox
            }
            None => None,
        };
        if let Some(pixels) = cached_composite_pixels {
            let result = loaded_image_from_color_image(&pixels, auto_trim_bbox, identity);
            if let Some(stage) = compose_stage.as_mut() {
                stage.finish(RemotePageStageMetrics::buffer(
                    pixels.size[0],
                    pixels.size[1],
                    4,
                ));
            }
            return result;
        }
        // 生 raster は共有されたまま渡す。この下の編集・合成・切り抜きはどれも `&` で読んで
        // 新しい buffer を返すので、一意な `Arc` を要求しない (本体のローカル表示も
        // `fs_cache` の共有 `Arc` をそのまま同じ関数へ渡している)。事前に deep clone すると、
        // 46MP のページで実測 62ms を毎ページ払うだけで何も買えない。
        let cache_composite = prepared_composite.is_some();
        let mut pixels = color_image;
        if let Some(prepared) = prepared_composite {
            let edit_started = Instant::now();
            let materialized_result = self.execute_remote_edits(pixels, prepared.edits, &cancel);
            if let Some(stage) = compose_stage.as_mut() {
                stage.phase_from("edits", edit_started);
            }
            let materialized = materialized_result?;
            pixels = materialized.pixels;
            crate::logger::log(format!(
                "remote_ipc: edit_materialize elapsed_ms={:.1} erase_ms={:.1} local_adjust_ms={:.1} conceal_ms={:.1} diffusion_fallback={}",
                edit_started.elapsed().as_secs_f64() * 1000.0,
                materialized.timing.erase_ms,
                materialized.timing.local_adjust_ms,
                materialized.timing.conceal_ms,
                materialized.used_diffusion_fallback,
            ));
            let lut_started = Instant::now();
            let lut_wait_ms = compose_stage.as_ref().map_or(0.0, |stage| stage.wait_ms);
            let lut_result = self.resolve_remote_lut_timed(
                prepared.lut_entry.as_ref(),
                compose_stage.as_mut(),
                ambient_wait_stage.as_deref_mut(),
            );
            if let Some(stage) = compose_stage.as_mut() {
                stage.phase_from_excluding_wait("lut", lut_started, lut_wait_ms);
            }
            let lut = lut_result?;
            let composite_started = Instant::now();
            let composite_result = execute_remote_composite(pixels, &prepared.params, lut, &cancel);
            if let Some(stage) = compose_stage.as_mut() {
                stage.phase_from("composite", composite_started);
            }
            pixels = composite_result?;
            if let Some(stored_edit_space) = stored_edit_space {
                if !materialized.comic.is_empty()
                    && let Some(fonts) =
                        crate::comic_overlay::load_comic_fonts_for(&materialized.comic)
                {
                    let comic_started = Instant::now();
                    let comic_wait_ms = compose_stage.as_ref().map_or(0.0, |stage| stage.wait_ms);
                    pixels = stored_edit_space.comic_composite(
                        &pixels,
                        &materialized.comic,
                        &fonts,
                        &mut lock_with_remote_page_wait(
                            &self.comic_stamp_cache,
                            compose_stage.as_mut(),
                            ambient_wait_stage.as_deref_mut(),
                        ),
                        &cancel,
                    );
                    if let Some(stage) = compose_stage.as_mut() {
                        stage.phase_from_excluding_wait("comic", comic_started, comic_wait_ms);
                    }
                }
            }
            let crop_started = Instant::now();
            let crop_result =
                crop_with_stored_edit_space(pixels, materialized.export_crop, stored_edit_space);
            if let Some(stage) = compose_stage.as_mut() {
                stage.phase_from("crop", crop_started);
            }
            pixels = crop_result.map_err(|error| {
                crate::logger::log(format!("remote_ipc: export crop failed: {error}"));
                media_error(
                    MediaErrorCode::RenderFailed,
                    "ページの切り取り結果を作成できませんでした",
                )
            })?;
            if cancel.load(Ordering::Acquire) {
                return Err(media_error(
                    MediaErrorCode::Cancelled,
                    "ページの表示需要がなくなったため処理を取り消しました",
                ));
            }
            pixels = apply_remote_page_rotation(pixels, rotation);
            let cache_insert_started = Instant::now();
            let cache_insert_wait_ms = compose_stage.as_ref().map_or(0.0, |stage| stage.wait_ms);
            lock_with_remote_page_wait(
                &self.page_composite_cache,
                compose_stage.as_mut(),
                ambient_wait_stage.as_deref_mut(),
            )
            .insert(prepared.key.clone(), Arc::clone(&pixels));
            if let Some(stage) = compose_stage.as_mut() {
                stage.phase_from_excluding_wait(
                    "cache_insert",
                    cache_insert_started,
                    cache_insert_wait_ms,
                );
            }
            crate::logger::log(format!(
                "remote_ipc: final_composite cache=miss key={}",
                prepared.key.page_key
            ));
        }
        if !cache_composite {
            pixels = apply_remote_page_rotation(pixels, rotation);
        }
        let result = loaded_image_from_color_image(&pixels, auto_trim_bbox, identity);
        if result.is_ok()
            && let Some(stage) = compose_stage.as_mut()
        {
            stage.finish(RemotePageStageMetrics::buffer(
                pixels.size[0],
                pixels.size[1],
                4,
            ));
        }
        result
    }

    fn ensure_pdf_page_in_range(
        &self,
        resolved: &ResolvedPath,
        metadata: &std::fs::Metadata,
        page_number: u32,
    ) -> Result<(), MediaError> {
        self.ensure_pdf_page_in_range_timed(resolved, metadata, page_number, None, None)
    }

    fn ensure_pdf_page_in_range_timed(
        &self,
        resolved: &ResolvedPath,
        metadata: &std::fs::Metadata,
        page_number: u32,
        primary: Option<&mut RemotePageStageGuard>,
        fallback: Option<&mut RemotePageStageGuard>,
    ) -> Result<(), MediaError> {
        validate_page_number(
            page_number,
            self.pdf_page_count_timed(resolved, metadata, primary, fallback)?,
        )
    }

    /// 本体の PDF 一覧と同じ `pdf_meta` を先に引き、miss 時だけ PDFium で列挙する。
    /// `container_page_meta` は ZIP / folder / converted archive 用であり、PDF は
    /// password_required も保持する専用テーブルが正本になる。
    fn pdf_page_count(
        &self,
        resolved: &ResolvedPath,
        metadata: &std::fs::Metadata,
    ) -> Result<u32, MediaError> {
        self.pdf_page_count_timed(resolved, metadata, None, None)
    }

    fn pdf_page_count_timed(
        &self,
        resolved: &ResolvedPath,
        metadata: &std::fs::Metadata,
        mut primary: Option<&mut RemotePageStageGuard>,
        mut fallback: Option<&mut RemotePageStageGuard>,
    ) -> Result<u32, MediaError> {
        let identity = PdfIdentity {
            path: resolved.canonical.clone(),
            mtime: crate::ui_helpers::mtime_secs(metadata),
            file_size: metadata.len(),
        };
        let cached = lock_with_remote_page_wait(
            &self.pdf_page_counts,
            primary.as_deref_mut(),
            fallback.as_deref_mut(),
        )
        .get(&identity)
        .copied();
        let count = match cached {
            Some(count) => count,
            None => {
                let password = self.pdf_passwords.get(&resolved.logical);
                let catalog = open_parent_catalog(&resolved.logical);
                let filename = resolved.logical.file_name().and_then(|name| name.to_str());
                let persistent = catalog
                    .as_ref()
                    .zip(filename)
                    .and_then(|(catalog, filename)| {
                        catalog
                            .get_pdf_meta(
                                filename,
                                identity.mtime,
                                i64::try_from(identity.file_size).unwrap_or(i64::MAX),
                            )
                            .map_err(|error| {
                                crate::logger::log(format!(
                                    "remote_ipc: pdf meta lookup failed: {error}"
                                ));
                            })
                            .ok()
                            .flatten()
                    });
                let count = match persistent {
                    Some((_, true)) if password.is_none() => {
                        return Err(media_error(
                            MediaErrorCode::PasswordRequired,
                            "この PDF はパスワード保護されているため Web から開けません",
                        ));
                    }
                    Some((count, _)) if count > 0 => count,
                    _ => {
                        let pages = crate::pdf_loader::enumerate_pages(
                            &resolved.logical,
                            password.as_deref(),
                        )
                        .map_err(pdf_error)?;
                        let count = u32::try_from(pages.len()).unwrap_or(u32::MAX);
                        if let (Some(catalog), Some(filename)) = (catalog.as_ref(), filename) {
                            let write_result = if password.is_none() {
                                catalog.set_pdf_meta_safe(
                                    filename,
                                    identity.mtime,
                                    i64::try_from(identity.file_size).unwrap_or(i64::MAX),
                                    count,
                                )
                            } else {
                                catalog.set_pdf_meta_thumb(
                                    filename,
                                    identity.mtime,
                                    i64::try_from(identity.file_size).unwrap_or(i64::MAX),
                                    count,
                                )
                            };
                            if let Err(error) = write_result {
                                crate::logger::log(format!(
                                    "remote_ipc: pdf meta update failed: {error}"
                                ));
                            }
                        }
                        count
                    }
                };
                lock_with_remote_page_wait(
                    &self.pdf_page_counts,
                    primary.as_deref_mut(),
                    fallback.as_deref_mut(),
                )
                .insert(identity, count);
                count
            }
        };
        Ok(count)
    }
}

fn load_mask_snapshot(
    db: &crate::mask_db::MaskDb,
    page_key: &str,
) -> Result<Option<crate::edit_source::MaskSnapshot>, MediaError> {
    let loaded = db
        .get_full_checked(page_key)
        .map_err(|error| remote_edit_db_read_error("erase", error))?;
    Ok(
        loaded.map(|(bitmap, shapes, size)| crate::edit_source::MaskSnapshot {
            bitmap,
            shapes,
            size,
        }),
    )
}

fn load_conceal_snapshot(
    db: &crate::conceal_db::ConcealDb,
    page_key: &str,
) -> Result<Option<crate::edit_source::MaskSnapshot>, MediaError> {
    let loaded = db
        .get_full_checked(page_key)
        .map_err(|error| remote_edit_db_read_error("conceal", error))?;
    Ok(
        loaded.map(|(bitmap, shapes, size)| crate::edit_source::MaskSnapshot {
            bitmap,
            shapes,
            size,
        }),
    )
}

fn remote_edit_fingerprint(
    erase: Option<&crate::edit_source::MaskSnapshot>,
    erase_mono_tolerance: u8,
    local_adjust: Option<&Vec<local_adjust_core::LocalAdjustmentLayer>>,
    conceal: Option<&crate::edit_source::MaskSnapshot>,
    conceal_preset: &crate::conceal::ConcealPreset,
    comic: &[comic_core::AnnotationObject],
    export_crop: Option<&crate::export_crop::CropSettings>,
) -> Result<[u8; 32], MediaError> {
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    hash_remote_edit_value(
        &mut digest,
        b"erase",
        &erase.map(|mask| (&mask.bitmap, &mask.shapes, mask.size)),
    )?;
    if erase.is_some() {
        hash_remote_edit_value(&mut digest, b"erase-mono-tolerance", &erase_mono_tolerance)?;
    }
    hash_remote_edit_value(&mut digest, b"local", &local_adjust)?;
    hash_remote_edit_value(
        &mut digest,
        b"conceal",
        &conceal.map(|mask| (&mask.bitmap, &mask.shapes, mask.size)),
    )?;
    if conceal.is_some() {
        hash_remote_edit_value(&mut digest, b"conceal-preset", conceal_preset)?;
    }
    hash_remote_edit_value(&mut digest, b"comic", &comic)?;
    hash_remote_edit_value(&mut digest, b"crop", &export_crop)?;
    Ok(digest.finalize().into())
}

fn remote_pre_ai_edit_fingerprint(
    erase: Option<&crate::edit_source::MaskSnapshot>,
    erase_mono_tolerance: u8,
    local_adjust: Option<&Vec<local_adjust_core::LocalAdjustmentLayer>>,
    conceal: Option<&crate::edit_source::MaskSnapshot>,
    conceal_preset: &crate::conceal::ConcealPreset,
) -> Result<[u8; 32], MediaError> {
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    hash_remote_edit_value(
        &mut digest,
        b"erase",
        &erase.map(|mask| (&mask.bitmap, &mask.shapes, mask.size)),
    )?;
    if erase.is_some() {
        hash_remote_edit_value(&mut digest, b"erase-mono-tolerance", &erase_mono_tolerance)?;
    }
    hash_remote_edit_value(&mut digest, b"local", &local_adjust)?;
    hash_remote_edit_value(
        &mut digest,
        b"conceal",
        &conceal.map(|mask| (&mask.bitmap, &mask.shapes, mask.size)),
    )?;
    if conceal.is_some() {
        hash_remote_edit_value(&mut digest, b"conceal-preset", conceal_preset)?;
    }
    Ok(digest.finalize().into())
}

fn hash_remote_edit_value<T: serde::Serialize>(
    digest: &mut sha2::Sha256,
    label: &[u8],
    value: &T,
) -> Result<(), MediaError> {
    use sha2::Digest;
    digest.update(label);
    let bytes = serde_json::to_vec(value).map_err(|error| {
        crate::logger::log(format!(
            "remote_ipc: edit snapshot fingerprint failed: {error}"
        ));
        media_error(
            MediaErrorCode::Internal,
            "編集結果のキャッシュキーを作成できませんでした",
        )
    })?;
    digest.update(bytes);
    Ok(())
}

fn remote_edit_db_open_error(kind: &str, error: rusqlite::Error) -> MediaError {
    crate::logger::log(format!(
        "remote_ipc: {kind} DB reopen failed for edit materialization: {error}"
    ));
    media_error(
        MediaErrorCode::Internal,
        "編集データベースを開けないためページを合成できません",
    )
}

fn remote_edit_db_read_error(kind: &str, error: String) -> MediaError {
    crate::logger::log(format!(
        "remote_ipc: {kind} DB read failed for edit materialization: {error}"
    ));
    media_error(
        MediaErrorCode::Internal,
        "編集データベースを読めないためページを合成できません",
    )
}

fn remote_adjustment_identity(
    address: &RemoteAddress,
    logical_path: &Path,
) -> Option<RemoteAdjustmentIdentity> {
    let page_key = crate::edit_source::page_key_for_remote(logical_path, &address.subresource)?;
    match &address.subresource {
        RemoteSubresource::File => {
            let location_path = if is_zip_path(logical_path) || is_pdf_path(logical_path) {
                logical_path.to_path_buf()
            } else {
                logical_path.parent()?.to_path_buf()
            };
            Some(RemoteAdjustmentIdentity {
                page_key,
                location_path,
                compiled_book: false,
            })
        }
        RemoteSubresource::ZipEntry { .. } => Some(RemoteAdjustmentIdentity {
            page_key,
            location_path: logical_path.to_path_buf(),
            compiled_book: false,
        }),
        RemoteSubresource::PdfPage { .. } => Some(RemoteAdjustmentIdentity {
            page_key,
            location_path: logical_path.to_path_buf(),
            compiled_book: false,
        }),
        RemoteSubresource::ZipDirectory { .. } => None,
    }
}

fn resolve_remote_effective_params(
    identity: &RemoteAdjustmentIdentity,
    page: Option<&crate::adjustment::AdjustParams>,
    favorites: &[crate::settings::FavoriteEntry],
    favorite_params: &HashMap<uuid::Uuid, crate::adjustment::AdjustParams>,
    global: &crate::adjustment::AdjustParams,
) -> crate::adjustment::AdjustParams {
    if identity.compiled_book {
        return page.cloned().unwrap_or_default();
    }
    crate::final_composite::resolve_effective_params(
        page,
        || {
            crate::final_composite::active_favorite_default_id_for_path(
                &identity.location_path,
                favorites,
                None,
                |id| favorite_params.contains_key(&id),
            )
            .and_then(|id| favorite_params.get(&id))
        },
        global,
    )
    .clone()
}

#[cfg(test)]
pub(crate) fn resolve_remote_effective_params_for_test(
    logical_path: &Path,
    subresource: &RemoteSubresource,
    page: Option<&crate::adjustment::AdjustParams>,
    favorites: &[crate::settings::FavoriteEntry],
    favorite_params: &HashMap<uuid::Uuid, crate::adjustment::AdjustParams>,
    global: &crate::adjustment::AdjustParams,
) -> crate::adjustment::AdjustParams {
    let address = RemoteAddress {
        path: logical_path.to_string_lossy().into_owned(),
        subresource: subresource.clone(),
    };
    let identity = remote_adjustment_identity(&address, logical_path)
        .expect("test subresource must identify a page");
    resolve_remote_effective_params(&identity, page, favorites, favorite_params, global)
}

fn execute_remote_composite(
    source: Arc<egui::ColorImage>,
    params: &crate::adjustment::AdjustParams,
    lut: Option<crate::creative_lut::SharedCreativeLut>,
    cancel: &AtomicBool,
) -> Result<Arc<egui::ColorImage>, MediaError> {
    let creative_lut = lut.map(|lut| (lut, params.creative_lut.strength));
    let plan = crate::final_composite::build_final_composite_plan_without_ai(params, creative_lut);
    match crate::final_composite::execute_final_composite(source, plan, cancel) {
        crate::final_composite::FinalCompositeResult::Ready {
            pixels,
            elapsed_ms,
            timing,
        } => {
            crate::logger::log(format!(
                "remote_ipc: final_composite elapsed_ms={elapsed_ms:.1} adjust_ms={:.1} sharpen_ms={:.1} colorize_check_ms={:.1} colorize_apply_ms={:.1} colorize_applied={} creative_lut_ms={:.1} post_filter_ms={:.1}",
                timing.adjust_ms,
                timing.sharpen_ms,
                timing.colorize_check_ms,
                timing.colorize_apply_ms,
                timing.colorize_applied,
                timing.creative_lut_ms,
                timing.post_filter_ms,
            ));
            Ok(pixels)
        }
        crate::final_composite::FinalCompositeResult::Cancelled => Err(media_error(
            MediaErrorCode::Busy,
            "先読みは新しいページ要求に置き換えられました",
        )),
    }
}

#[derive(Debug)]
enum RemoteAiRunError {
    NotApplicable {
        code: RemoteAiTerminalCode,
        message: String,
        page_index: usize,
    },
    Superseded(String),
    Failed(String),
}

fn remote_ai_media_error(error: MediaError) -> RemoteAiRunError {
    RemoteAiRunError::Failed(error.message)
}

fn check_remote_ai_cancel(cancel: &AtomicBool) -> Result<(), RemoteAiRunError> {
    if cancel.load(Ordering::Relaxed) {
        Err(RemoteAiRunError::Failed("AI job was cancelled".to_owned()))
    } else {
        Ok(())
    }
}

fn remote_ai_pre_params(
    params: &crate::adjustment::AdjustParams,
) -> crate::adjustment::AdjustParams {
    let mut result = params.clone();
    result.post_filter = crate::adjustment::PostFilter::None;
    result.creative_lut = crate::creative_lut::CreativeLutSelection::default();
    result.colorize = crate::colorize::ColorizeParams::default();
    result.smart_sharpen = 0;
    result
}

fn remote_ai_native_budget(
    settings: &crate::settings_db::AdjustmentRenderSettings,
) -> Option<(usize, u64)> {
    let max_entries = settings.retained_final_ai_cache_max_entries;
    let max_mib = settings.retained_final_ai_cache_max_mib;
    if max_entries == 0 || max_mib == 0 {
        return None;
    }
    Some((max_entries, max_mib.saturating_mul(1024 * 1024)))
}

fn remote_ai_model_epoch(
    runtime: &crate::ai::runtime::AiRuntime,
    manager: &crate::ai::model_manager::ModelManager,
    selected: crate::ai::final_pipeline::SelectedFinalAiModels,
) -> [u8; 32] {
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    digest.update(REMOTE_AI_PIPELINE_SCHEMA.to_le_bytes());
    digest.update(runtime.active_backend().requested.as_str().as_bytes());
    digest.update(runtime.active_backend().effective.as_str().as_bytes());
    digest.update(crate::ai::tensorrt_pack::EXPECTED_TRT_PACK_VERSION.to_le_bytes());
    for kind in [selected.denoise, selected.upscale].into_iter().flatten() {
        digest.update(format!("{kind:?}").as_bytes());
        digest.update([u8::from(runtime.should_route_to_worker(kind))]);
        if let Some(path) = manager.model_path(kind) {
            digest.update(path.as_os_str().to_string_lossy().as_bytes());
            if let Ok(metadata) = std::fs::metadata(path) {
                digest.update(metadata.len().to_le_bytes());
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos())
                    .unwrap_or_default();
                digest.update(modified.to_le_bytes());
            }
        }
    }
    digest.finalize().into()
}

fn remote_ai_progress(
    phase: RemoteAiProgressPhase,
    page_index: usize,
    page_count: usize,
    stage_index: u32,
    tiles: Option<(usize, usize)>,
) -> mimageviewer_ipc::RemoteAiProgress {
    mimageviewer_ipc::RemoteAiProgress {
        phase,
        page_index: page_index as u32,
        page_count: page_count as u32,
        stage_index,
        stage_count: 5,
        completed_tiles: tiles.map(|(completed, _)| completed as u32),
        total_tiles: tiles.map(|(_, total)| total as u32),
    }
}

struct ContainerFinalAiProgress<'a> {
    sink: &'a dyn super::ai_job::RemoteAiProgressSink,
    page_index: usize,
    page_count: usize,
}

impl crate::ai::final_pipeline::FinalAiProgressSink for ContainerFinalAiProgress<'_> {
    fn loading_model(&self, _kind: crate::ai::ModelKind) {
        self.sink.update(
            mimageviewer_ipc::RemoteAiJobState::LoadingModel,
            Some(remote_ai_progress(
                RemoteAiProgressPhase::LoadingModel,
                self.page_index,
                self.page_count,
                1,
                None,
            )),
        );
    }

    fn denoising(&self, completed_tiles: usize, total_tiles: usize) {
        self.sink.update(
            mimageviewer_ipc::RemoteAiJobState::Denoising,
            Some(remote_ai_progress(
                RemoteAiProgressPhase::Denoising,
                self.page_index,
                self.page_count,
                2,
                Some((completed_tiles, total_tiles)),
            )),
        );
    }

    fn upscaling(&self, completed_tiles: usize, total_tiles: usize) {
        self.sink.update(
            mimageviewer_ipc::RemoteAiJobState::Upscaling,
            Some(remote_ai_progress(
                RemoteAiProgressPhase::Upscaling,
                self.page_index,
                self.page_count,
                3,
                Some((completed_tiles, total_tiles)),
            )),
        );
    }
}

fn decode_remote_ai_canonical(
    source: crate::canonical_image_loader::CanonicalImageSource<'_>,
    page_index: usize,
    cancel: &Arc<AtomicBool>,
) -> Result<(Arc<egui::ColorImage>, [usize; 2]), RemoteAiRunError> {
    let decoded = crate::canonical_image_loader::decode_canonical_image(
        source,
        crate::canonical_image_loader::CanonicalDecodeOptions {
            susie_priority: true,
            susie_cancel: Some(cancel),
            animation_policy: crate::canonical_image_loader::AnimationPolicy::FullFrames,
        },
    )
    .map_err(|error| RemoteAiRunError::Failed(error.to_string()))?;
    match decoded {
        crate::canonical_image_loader::CanonicalImageDecode::Static(image) => {
            let raster = image.into_gpu_raster();
            Ok((Arc::new(raster.pixels), raster.source_dims))
        }
        crate::canonical_image_loader::CanonicalImageDecode::Animated { format, .. } => {
            let (code, label) = match format {
                crate::canonical_image_loader::CanonicalAnimatedFormat::Gif => {
                    (RemoteAiTerminalCode::AnimatedGif, "アニメーション GIF")
                }
                crate::canonical_image_loader::CanonicalAnimatedFormat::Apng => {
                    (RemoteAiTerminalCode::AnimatedApng, "アニメーション PNG")
                }
                crate::canonical_image_loader::CanonicalAnimatedFormat::WebP => {
                    (RemoteAiTerminalCode::AnimatedWebp, "アニメーション WebP")
                }
            };
            Err(RemoteAiRunError::NotApplicable {
                code,
                message: format!("{label} は AI 静止画処理の対象外です"),
                page_index,
            })
        }
    }
}

fn apply_remote_page_rotation(
    pixels: Arc<egui::ColorImage>,
    rotation: crate::rotation_db::Rotation,
) -> Arc<egui::ColorImage> {
    if rotation.is_none() {
        pixels
    } else {
        Arc::new(crate::capture::rotate_color_image(&pixels, rotation))
    }
}

fn loaded_image_from_color_image(
    pixels: &Arc<egui::ColorImage>,
    auto_trim_bbox: Option<egui::Rect>,
    identity: RemoteAddress,
) -> Result<LoadedImage, MediaError> {
    Ok(LoadedImage {
        pixels: Arc::clone(pixels),
        auto_trim_bbox,
        identity,
    })
}

fn color_image_to_dynamic_image(pixels: &egui::ColorImage) -> Option<image::DynamicImage> {
    let width = u32::try_from(pixels.size[0]).ok()?;
    let height = u32::try_from(pixels.size[1]).ok()?;
    let rgba = crate::capture::color_image_to_rgba(pixels);
    let image =
        image::RgbaImage::from_raw(width, height, rgba).map(image::DynamicImage::ImageRgba8)?;
    Some(image)
}

fn remote_adjustment_read_error(scope: &str, error: String) -> MediaError {
    crate::logger::log(format!(
        "remote_ipc: live adjustment DB read failed scope={scope}: {error}"
    ));
    media_error(
        MediaErrorCode::Internal,
        format!("最新の補正データを読み込めませんでした ({scope})"),
    )
}

fn remote_adjustment_settings_error(error: crate::settings_db::SettingsDbError) -> MediaError {
    crate::logger::log(format!(
        "remote_ipc: live adjustment settings read failed: {error}"
    ));
    media_error(
        MediaErrorCode::Internal,
        "最新の補正設定を読み込めませんでした",
    )
}

fn validated_context(
    page_index: usize,
    page_count: usize,
    record_history: bool,
    record_resume: bool,
    bookmark_supported: bool,
) -> Result<ValidatedPageContext, RemoteWriteError> {
    let page_index = u32::try_from(page_index).map_err(|_| {
        RemoteWriteError::new(
            RemoteWriteErrorCode::Unsupported,
            "ページ index が上限を超えています",
        )
    })?;
    let page_count = u32::try_from(page_count).map_err(|_| {
        RemoteWriteError::new(
            RemoteWriteErrorCode::Unsupported,
            "ページ数が上限を超えています",
        )
    })?;
    Ok(ValidatedPageContext {
        page_index,
        page_number: page_index.saturating_add(1),
        page_count,
        record_history,
        record_resume,
        bookmark_supported,
    })
}

fn open_parent_catalog(path: &Path) -> Option<crate::catalog::CatalogDb> {
    let parent = path.parent()?;
    crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), parent)
        .map_err(|error| {
            crate::logger::log(format!(
                "remote_ipc: PDF parent catalog open failed: {error}"
            ));
        })
        .ok()
}

fn validate_page_number(page_number: u32, page_count: u32) -> Result<(), MediaError> {
    if page_number < page_count {
        Ok(())
    } else {
        Err(media_error(
            MediaErrorCode::PageOutOfRange,
            "PDF ページ番号が範囲外です",
        ))
    }
}

fn zip_entry_address(container: &RemoteAddress, entry_name: &str) -> RemoteAddress {
    RemoteAddress {
        path: container.path.clone(),
        subresource: RemoteSubresource::ZipEntry {
            entry_name: entry_name.to_owned(),
        },
    }
}

fn normalize_remote_bookmark_path(value: &str) -> String {
    value.replace('\\', "/").to_lowercase()
}

fn remote_bookmark_row(
    bookmark: crate::book_bookmarks::BookBookmark,
    target: Option<RemoteBookBookmarkTarget>,
) -> RemoteBookBookmarkRow {
    RemoteBookBookmarkRow {
        id: bookmark.id,
        title: bookmark.title,
        page_index_hint: u32::try_from(bookmark.page_index_hint).unwrap_or(u32::MAX),
        page_label: bookmark.page_identity.display_name(),
        target,
    }
}

fn grid_item_address(
    container: &RemoteAddress,
    item: &crate::grid_item::GridItem,
) -> Option<RemoteAddress> {
    match item {
        crate::grid_item::GridItem::Image(path) => Some(RemoteAddress::file(
            resolve_existing(path.to_string_lossy().as_ref())
                .ok()?
                .logical
                .to_string_lossy()
                .into_owned(),
        )),
        crate::grid_item::GridItem::ZipImage { entry_name, .. } => {
            Some(zip_entry_address(container, entry_name))
        }
        crate::grid_item::GridItem::PdfPage { page_num, .. } => Some(RemoteAddress {
            path: container.path.clone(),
            subresource: RemoteSubresource::PdfPage {
                page_number: *page_num,
            },
        }),
        _ => None,
    }
}

fn resolve_resume_page(
    container: &RemoteAddress,
    items: &[crate::grid_item::GridItem],
    saved_index: usize,
) -> Option<RemoteAddress> {
    items
        .get(saved_index)
        .and_then(|item| grid_item_address(container, item))
}

pub(super) fn core_spread_mode(mode: RemoteSpreadMode) -> crate::settings::SpreadMode {
    match mode {
        RemoteSpreadMode::Single => crate::settings::SpreadMode::Single,
        RemoteSpreadMode::Ltr => crate::settings::SpreadMode::Ltr,
        RemoteSpreadMode::LtrCover => crate::settings::SpreadMode::LtrCover,
        RemoteSpreadMode::Rtl => crate::settings::SpreadMode::Rtl,
        RemoteSpreadMode::RtlCover => crate::settings::SpreadMode::RtlCover,
        RemoteSpreadMode::SplitLtr => crate::settings::SpreadMode::SplitLtr,
        RemoteSpreadMode::SplitRtl => crate::settings::SpreadMode::SplitRtl,
    }
}

fn remote_spread_mode(mode: crate::settings::SpreadMode) -> RemoteSpreadMode {
    match mode {
        crate::settings::SpreadMode::Ltr => RemoteSpreadMode::Ltr,
        crate::settings::SpreadMode::LtrCover => RemoteSpreadMode::LtrCover,
        crate::settings::SpreadMode::Rtl => RemoteSpreadMode::Rtl,
        crate::settings::SpreadMode::RtlCover => RemoteSpreadMode::RtlCover,
        crate::settings::SpreadMode::SplitLtr => RemoteSpreadMode::SplitLtr,
        crate::settings::SpreadMode::SplitRtl => RemoteSpreadMode::SplitRtl,
        // 旧 DB 互換の `Vertical` は本体側で Single へ解決してから返す。
        crate::settings::SpreadMode::Single | crate::settings::SpreadMode::Vertical => {
            RemoteSpreadMode::Single
        }
    }
}

fn remote_reading_direction(
    direction: crate::settings::ReadingDirection,
) -> RemoteReadingDirection {
    match direction {
        crate::settings::ReadingDirection::Ltr => RemoteReadingDirection::Ltr,
        crate::settings::ReadingDirection::Rtl => RemoteReadingDirection::Rtl,
    }
}

fn remote_reading_direction_name(direction: RemoteReadingDirection) -> &'static str {
    match direction {
        RemoteReadingDirection::Ltr => "ltr",
        RemoteReadingDirection::Rtl => "rtl",
    }
}

pub(super) fn resolve_spread_state(
    requested: Option<RemoteSpreadMode>,
    requested_direction: Option<RemoteReadingDirection>,
    stored_mode: Option<crate::settings::SpreadMode>,
    stored_direction: Option<crate::settings::ReadingDirection>,
    default_mode: crate::settings::SpreadMode,
    default_direction: crate::settings::ReadingDirection,
    force_single_page: bool,
) -> (RemoteSpreadMode, RemoteSpreadMode, RemoteReadingDirection) {
    let configured =
        requested.unwrap_or_else(|| remote_spread_mode(stored_mode.unwrap_or(default_mode)));
    let mut reading_direction = requested_direction
        .unwrap_or_else(|| remote_reading_direction(stored_direction.unwrap_or(default_direction)));
    if configured.is_rtl() {
        reading_direction = RemoteReadingDirection::Rtl;
    } else if matches!(
        configured,
        RemoteSpreadMode::Ltr | RemoteSpreadMode::LtrCover
    ) {
        reading_direction = RemoteReadingDirection::Ltr;
    }
    // 縦持ち強制は「1 画面に 2 ページ出さない」ための表示限定 Single。
    // **分割は既にそれを達成している**ので重ねない。重ねると縦持ちで分割が消え、
    // 分割が一番効く場面 (横長 scan をスマホで読む) で無効になる。
    let effective = if force_single_page && !configured.is_split() {
        RemoteSpreadMode::Single
    } else {
        configured
    };
    (configured, effective, reading_direction)
}

fn zip_prefix_segments(prefix: &str) -> Vec<String> {
    prefix
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect()
}

fn zip_prefix(segments: &[String]) -> String {
    if segments.is_empty() {
        String::new()
    } else {
        format!("{}/", segments.join("/"))
    }
}

fn container_title(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "コンテナ".to_owned())
}

fn container_thumb_key(prefix: &str, path: &Path) -> Result<String, MediaError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("{prefix}{name}"))
        .ok_or_else(|| media_error(MediaErrorCode::Unsupported, "ファイル名を解釈できません"))
}

fn is_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            crate::folder_tree::is_zip_extension(&extension.to_ascii_lowercase())
        })
}

fn is_archive_container(resolved: &ResolvedPath) -> bool {
    resolved.has_archive_backing() || is_zip_path(&resolved.logical)
}

fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            crate::folder_tree::is_recognized_image_ext(&extension.to_ascii_lowercase())
        })
}

fn media_source_kind(address: &RemoteAddress) -> &'static str {
    match address.subresource {
        RemoteSubresource::ZipDirectory { .. } | RemoteSubresource::ZipEntry { .. } => "zip",
        RemoteSubresource::PdfPage { .. } => "pdf",
        RemoteSubresource::File => {
            let path = Path::new(&address.path);
            if is_zip_path(path) {
                "zip"
            } else if is_pdf_path(path) {
                "pdf"
            } else {
                "file"
            }
        }
    }
}

fn pdf_error(error: std::io::Error) -> MediaError {
    let message = error.to_string();
    if crate::pdf_loader::is_password_required_error(&error) {
        media_error(
            MediaErrorCode::PasswordRequired,
            "この PDF はパスワード保護されているため Web から開けません",
        )
    } else {
        crate::logger::log(format!("remote_ipc: pdf operation failed: {message}"));
        media_error(MediaErrorCode::RenderFailed, "PDF を開けませんでした")
    }
}

fn resolve_media_error(error: ResolveError) -> MediaError {
    match error {
        ResolveError::InvalidPath => media_error(MediaErrorCode::BadRequest, "絶対パスが不正です"),
        ResolveError::NetworkPath => media_error(
            MediaErrorCode::BadRequest,
            mimageviewer_ipc::REMOTE_NETWORK_PATH_MESSAGE,
        ),
        ResolveError::Unavailable => media_error(MediaErrorCode::NotFound, "対象が見つかりません"),
    }
}

fn active_archive_media_error(error: mimageviewer_ipc::RemoteArchiveJobError) -> MediaError {
    crate::logger::log(format!(
        "remote_archive: active backing rejected code={:?}",
        error.code
    ));
    media_error(
        MediaErrorCode::NotFound,
        "アーカイブの元ファイルまたは変換結果が更新されました",
    )
}

fn absolute_root_name(path: &Path) -> String {
    path.components()
        .next()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn remote_write_error_from_media(error: MediaError) -> RemoteWriteError {
    let code = match error.code {
        MediaErrorCode::BadRequest => RemoteWriteErrorCode::BadRequest,
        MediaErrorCode::FavoriteNotFound => RemoteWriteErrorCode::FavoriteNotFound,
        MediaErrorCode::PathRejected => RemoteWriteErrorCode::PathRejected,
        MediaErrorCode::NotFound => RemoteWriteErrorCode::NotFound,
        MediaErrorCode::Unsupported => RemoteWriteErrorCode::Unsupported,
        MediaErrorCode::Busy => RemoteWriteErrorCode::Busy,
        MediaErrorCode::PasswordRequired
        | MediaErrorCode::PageOutOfRange
        | MediaErrorCode::Cancelled
        | MediaErrorCode::RenderFailed
        | MediaErrorCode::Internal => RemoteWriteErrorCode::Internal,
    };
    RemoteWriteError::new(code, error.message)
}

fn media_error_from_remote_write(error: RemoteWriteError) -> MediaError {
    let code = match error.code {
        RemoteWriteErrorCode::BadRequest => MediaErrorCode::BadRequest,
        RemoteWriteErrorCode::FavoriteNotFound => MediaErrorCode::FavoriteNotFound,
        RemoteWriteErrorCode::PathRejected => MediaErrorCode::PathRejected,
        RemoteWriteErrorCode::NotFound => MediaErrorCode::NotFound,
        RemoteWriteErrorCode::Unsupported => MediaErrorCode::Unsupported,
        RemoteWriteErrorCode::Busy => MediaErrorCode::Busy,
        RemoteWriteErrorCode::UiTimeout
        | RemoteWriteErrorCode::PersistenceFailed
        | RemoteWriteErrorCode::Internal => MediaErrorCode::Internal,
    };
    MediaError::new(code, error.message)
}

fn media_error(code: MediaErrorCode, message: impl Into<String>) -> MediaError {
    MediaError::new(code, message)
}

fn cancelled_page_error() -> PageResponse {
    PageResponse::Error(media_error(
        MediaErrorCode::Cancelled,
        "ページの表示需要がなくなったため処理を取り消しました",
    ))
}

fn thumbnail_error_from_media(error: MediaError) -> ThumbnailResponse {
    let code = match error.code {
        MediaErrorCode::BadRequest => ThumbnailErrorCode::BadRequest,
        MediaErrorCode::FavoriteNotFound => ThumbnailErrorCode::FavoriteNotFound,
        MediaErrorCode::PathRejected => ThumbnailErrorCode::PathRejected,
        MediaErrorCode::NotFound => ThumbnailErrorCode::NotFound,
        MediaErrorCode::Unsupported => ThumbnailErrorCode::Unsupported,
        MediaErrorCode::PasswordRequired => ThumbnailErrorCode::PasswordRequired,
        MediaErrorCode::PageOutOfRange => ThumbnailErrorCode::PageOutOfRange,
        MediaErrorCode::Cancelled => ThumbnailErrorCode::Busy,
        MediaErrorCode::Busy => ThumbnailErrorCode::Busy,
        MediaErrorCode::RenderFailed => ThumbnailErrorCode::GenerationFailed,
        MediaErrorCode::Internal => ThumbnailErrorCode::Internal,
    };
    thumbnail_error(code, error.message)
}

fn thumbnail_error(code: ThumbnailErrorCode, message: impl Into<String>) -> ThumbnailResponse {
    ThumbnailResponse::Error(ThumbnailError::new(code, message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::FavoriteEntry;
    use std::io::{Cursor, Write};

    fn source_test_identity(
        name: &str,
        target_px: u32,
        full_page: bool,
    ) -> RemoteSourceDecodeIdentity {
        RemoteSourceDecodeIdentity {
            path: PathBuf::from(name),
            mtime: 10,
            file_size: 20,
            pdf_page: None,
            zip_entry: None,
            zip_dir_prefix: None,
            cache_key_override: None,
            target_px,
            full_page,
        }
    }

    fn source_test_decoded(color: egui::Color32) -> RemoteDecodedSource {
        RemoteDecodedSource {
            raster: RemoteSourceRaster {
                pixels: Arc::new(egui::ColorImage::new([2, 1], vec![color; 2])),
                decoded_source_dims: Some((2, 1)),
            },
            load_request_ms: 1.0,
            drain_ms: 0.5,
        }
    }

    type SourceTestGate = Arc<(Mutex<bool>, Condvar)>;

    fn source_test_gate_wait(gate: &SourceTestGate, cancel: &AtomicBool) -> bool {
        let (open, ready) = &**gate;
        let mut open = open.lock().unwrap_or_else(|error| error.into_inner());
        while !*open && !cancel.load(Ordering::Acquire) {
            let (next, _) = ready
                .wait_timeout(open, Duration::from_millis(5))
                .unwrap_or_else(|error| error.into_inner());
            open = next;
        }
        *open
    }

    fn source_test_gate_open(gate: &SourceTestGate) {
        let (open, ready) = &**gate;
        *open.lock().unwrap_or_else(|error| error.into_inner()) = true;
        ready.notify_all();
    }

    fn wait_for_source_participants(
        flight: &RemoteSourceSingleFlight,
        identity: &RemoteSourceDecodeIdentity,
        expected: usize,
    ) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut state = flight
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            let participants = state
                .in_flight
                .get(identity)
                .map(|entry| {
                    entry
                        .state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .participants
                })
                .unwrap_or(0);
            if participants == expected {
                return true;
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            let (next, timed) = flight
                .changed
                .wait_timeout(state, remaining.min(Duration::from_millis(10)))
                .unwrap_or_else(|error| error.into_inner());
            state = next;
            if timed.timed_out() && Instant::now() >= deadline {
                return false;
            }
        }
    }

    fn source_shared_cancelled(
        flight: &RemoteSourceSingleFlight,
        identity: &RemoteSourceDecodeIdentity,
    ) -> Option<bool> {
        flight
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .in_flight
            .get(identity)
            .map(|entry| entry.shared_cancel.load(Ordering::Acquire))
    }

    #[test]
    fn auto_spread_partner_starts_before_current_source_finishes() {
        let current_bbox = Some(egui::Rect::from_min_max(
            egui::pos2(0.10, 0.20),
            egui::pos2(0.80, 0.90),
        ));
        let partner_bbox = Some(egui::Rect::from_min_max(
            egui::pos2(0.25, 0.05),
            egui::pos2(0.95, 0.70),
        ));
        let plan = RemoteViewTrimPlan::AutoSpread {
            side: crate::view_trim::ViewTrimSpreadSide::Left,
            partner: RemoteAddress::file("partner.png"),
        };
        let (partner_started_tx, partner_started_rx) = mpsc::channel();
        let (source_finished_tx, source_finished_rx) = mpsc::channel();

        let actual = with_scoped_remote_partner(
            RemotePartnerStart::Resolve(()),
            move |(), _| {
                partner_started_tx.send(()).unwrap();
                source_finished_rx
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap();
                Ok::<_, MediaError>(partner_bbox)
            },
            || {
                partner_started_rx
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap();
                source_finished_tx.send(()).unwrap();
                Ok::<_, MediaError>(current_bbox)
            },
            |current_bbox, partner| {
                let partner = partner.collect()?;
                Ok(complete_remote_view_trim_bbox_from_partner(
                    &plan,
                    current_bbox,
                    partner,
                ))
            },
        )
        .unwrap();

        let expected = crate::view_trim::harmonize_spread_auto_bboxes(current_bbox, partner_bbox).0;
        assert_eq!(actual, expected);
    }

    #[test]
    fn non_spread_trim_plans_do_not_start_partner_resolution() {
        for plan in [
            RemoteViewTrimPlan::AutoSingle,
            RemoteViewTrimPlan::Stored(None),
        ] {
            let resolutions = Arc::new(AtomicUsize::new(0));
            let resolver_count = Arc::clone(&resolutions);
            let start: RemotePartnerStart<(), ()> = if plan.spread_partner().is_some() {
                RemotePartnerStart::Resolve(())
            } else {
                RemotePartnerStart::NotRequired
            };
            let result = with_scoped_remote_partner(
                start,
                move |(), _| {
                    resolver_count.fetch_add(1, Ordering::AcqRel);
                    Ok::<_, MediaError>(())
                },
                || Ok::<_, MediaError>(()),
                |(), partner| partner.collect().map(|_| ()),
            );
            assert!(result.is_ok());
            assert_eq!(resolutions.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn cached_spread_partner_does_not_spawn_or_decode() {
        let bbox = Some(egui::Rect::from_min_max(
            egui::pos2(0.1, 0.2),
            egui::pos2(0.9, 0.8),
        ));
        let resolutions = Arc::new(AtomicUsize::new(0));
        let resolver_count = Arc::clone(&resolutions);
        let result = with_scoped_remote_partner(
            RemotePartnerStart::<_, ()>::Cached(bbox),
            move |(), _| {
                resolver_count.fetch_add(1, Ordering::AcqRel);
                Ok::<_, MediaError>(None)
            },
            || Ok::<_, MediaError>(()),
            |(), partner| partner.collect(),
        )
        .unwrap();

        assert!(matches!(result, RemotePartnerResult::Resolved(value) if value == bbox));
        assert_eq!(resolutions.load(Ordering::Acquire), 0);
    }

    #[test]
    fn source_failure_cancels_partner_and_leaves_scope() {
        let cancel_seen = Arc::new(AtomicBool::new(false));
        let worker_cancel_seen = Arc::clone(&cancel_seen);
        let (started_tx, started_rx) = mpsc::channel();
        let started = Instant::now();

        let result = with_scoped_remote_partner(
            RemotePartnerStart::Resolve(()),
            move |(), participant_cancel| {
                started_tx.send(()).unwrap();
                let deadline = Instant::now() + Duration::from_millis(250);
                while !participant_cancel.load(Ordering::Acquire) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(1));
                }
                worker_cancel_seen.store(
                    participant_cancel.load(Ordering::Acquire),
                    Ordering::Release,
                );
                Ok::<_, MediaError>(())
            },
            || {
                started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                Err::<(), _>(media_error(MediaErrorCode::RenderFailed, "source failed"))
            },
            |(), _| -> Result<(), MediaError> { unreachable!() },
        );

        assert_eq!(result.unwrap_err().code, MediaErrorCode::RenderFailed);
        assert!(cancel_seen.load(Ordering::Acquire));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn speculative_partner_cancel_keeps_decode_for_partner_request() {
        let flight = Arc::new(RemoteSourceSingleFlight::default());
        let identity = source_test_identity("partner.png", 4096, true);
        let gate: SourceTestGate = Arc::new((Mutex::new(false), Condvar::new()));
        let (started_tx, started_rx) = mpsc::channel();
        let owner = {
            let flight = Arc::clone(&flight);
            let identity = identity.clone();
            let gate = Arc::clone(&gate);
            std::thread::spawn(move || {
                flight.load(
                    identity,
                    Arc::new(AtomicBool::new(false)),
                    move |shared_cancel| {
                        started_tx.send(()).unwrap();
                        assert!(source_test_gate_wait(&gate, shared_cancel));
                        Ok(source_test_decoded(egui::Color32::GREEN))
                    },
                )
            })
        };
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let speculative_flight = Arc::clone(&flight);
        let source_flight = Arc::clone(&flight);
        let source_identity = identity.clone();
        let result = with_scoped_remote_partner(
            RemotePartnerStart::Resolve(identity.clone()),
            move |identity, participant_cancel| {
                speculative_flight
                    .load(identity, participant_cancel, |_| {
                        panic!("speculative participant must join the existing decode")
                    })
                    .map(|_| ())
            },
            || {
                assert!(wait_for_source_participants(
                    &source_flight,
                    &source_identity,
                    2,
                ));
                Err::<(), _>(media_error(MediaErrorCode::RenderFailed, "source failed"))
            },
            |(), _| -> Result<(), MediaError> { unreachable!() },
        );

        assert_eq!(result.unwrap_err().code, MediaErrorCode::RenderFailed);
        assert!(wait_for_source_participants(&flight, &identity, 1));
        assert_eq!(source_shared_cancelled(&flight, &identity), Some(false));
        source_test_gate_open(&gate);
        assert_eq!(
            owner.join().unwrap().unwrap().outcome,
            RemoteSourceLoadOutcome::Decoded
        );
    }

    #[test]
    fn scoped_partner_collection_preserves_harmonized_bbox() {
        let current_bbox = Some(egui::Rect::from_min_max(
            egui::pos2(0.10, 0.20),
            egui::pos2(0.70, 0.95),
        ));
        let partner_bbox = Some(egui::Rect::from_min_max(
            egui::pos2(0.30, 0.05),
            egui::pos2(0.90, 0.75),
        ));

        for side in [
            crate::view_trim::ViewTrimSpreadSide::Left,
            crate::view_trim::ViewTrimSpreadSide::Right,
        ] {
            let plan = RemoteViewTrimPlan::AutoSpread {
                side,
                partner: RemoteAddress::file("partner.png"),
            };
            let (expected_left, expected_right) = match side {
                crate::view_trim::ViewTrimSpreadSide::Left => {
                    crate::view_trim::harmonize_spread_auto_bboxes(current_bbox, partner_bbox)
                }
                crate::view_trim::ViewTrimSpreadSide::Right => {
                    crate::view_trim::harmonize_spread_auto_bboxes(partner_bbox, current_bbox)
                }
            };
            let expected = match side {
                crate::view_trim::ViewTrimSpreadSide::Left => expected_left,
                crate::view_trim::ViewTrimSpreadSide::Right => expected_right,
            };
            let actual = with_scoped_remote_partner(
                RemotePartnerStart::<_, ()>::Cached(partner_bbox),
                |(), _| Ok::<_, MediaError>(None),
                || Ok::<_, MediaError>(current_bbox),
                |current_bbox, partner| {
                    Ok(complete_remote_view_trim_bbox_from_partner(
                        &plan,
                        current_bbox,
                        partner.collect()?,
                    ))
                },
            )
            .unwrap();

            assert_eq!(actual, expected);
        }
    }

    fn test_container_entries(count: usize, path: &str) -> (Vec<ContainerEntry>, bool) {
        let mut budget = ContainerEntryBudget::new(super::super::REMOTE_LIST_RESPONSE_BUDGET_BYTES);
        let mut entries = Vec::with_capacity(count.min(CONTAINER_ENTRY_LIMIT));
        for index in 0..count.min(CONTAINER_ENTRY_LIMIT) {
            let page_number = u32::try_from(index).unwrap();
            let entry = ContainerEntry {
                address: RemoteAddress {
                    path: path.to_owned(),
                    subresource: RemoteSubresource::PdfPage { page_number },
                },
                name: format!("Page {}", index + 1),
                kind: ContainerEntryKind::Image,
                page_count: None,
            };
            if !budget.try_include(&entry) {
                return (entries, true);
            }
            entries.push(entry);
        }
        (entries, false)
    }

    fn test_container_payload(
        total: usize,
        entries: Vec<ContainerEntry>,
        byte_truncated: bool,
    ) -> ContainerPayload {
        let (entry_limit, truncated) =
            container_limit_metadata(total, entries.len(), byte_truncated);
        let page_groups = entries
            .iter()
            .map(|entry| PageGroup {
                anchor: entry.address.clone(),
                pages: vec![entry.address.clone()],
                slice: mimageviewer_ipc::RemotePageSlice::Full,
            })
            .collect();
        ContainerPayload {
            title: "test".to_owned(),
            root_name: "C:".to_owned(),
            kind: ContainerKind::Pdf,
            effective_address: RemoteAddress::file("C:/p.pdf"),
            entries,
            thumb_aspect_height_ratio: 1.0,
            sort_state: super::super::remote_grid_sort_state(
                crate::settings::SortOrder::FileName,
                None,
            ),
            resume_page: None,
            open_mode: ContainerOpenMode::Grid,
            configured_spread_mode: RemoteSpreadMode::Single,
            effective_spread_mode: RemoteSpreadMode::Single,
            reading_direction: RemoteReadingDirection::Ltr,
            image_count: total,
            video_count: 0,
            other_count: 0,
            spread_page_gap_px: 0,
            page_groups,
            entry_limit,
            truncated,
        }
    }

    #[test]
    fn container_accepts_one_hundred_thousand_short_entries_and_truncates_the_next() {
        let (entries, byte_truncated) = test_container_entries(CONTAINER_ENTRY_LIMIT, "C:/p.pdf");
        let payload = test_container_payload(CONTAINER_ENTRY_LIMIT, entries, byte_truncated);

        assert_eq!(payload.entries.len(), CONTAINER_ENTRY_LIMIT);
        assert!(!payload.truncated);
        assert_eq!(payload.entry_limit, CONTAINER_ENTRY_LIMIT);
        assert!(
            serde_json::to_vec(&ContainerResponse::Success(payload))
                .unwrap()
                .len()
                < mimageviewer_ipc::MAX_RESPONSE_FRAME_BYTES
        );

        let (entry_limit, truncated) =
            container_limit_metadata(CONTAINER_ENTRY_LIMIT + 1, CONTAINER_ENTRY_LIMIT, false);
        assert_eq!(entry_limit, CONTAINER_ENTRY_LIMIT);
        assert!(truncated);
    }

    #[test]
    fn container_long_entries_and_page_groups_stay_below_the_ipc_frame_limit() {
        let long_path = format!("C:/{}.pdf", "x".repeat(400));
        let (entries, byte_truncated) = test_container_entries(CONTAINER_ENTRY_LIMIT, &long_path);
        let payload = test_container_payload(CONTAINER_ENTRY_LIMIT, entries, byte_truncated);

        assert!(payload.truncated);
        assert_eq!(payload.entry_limit, payload.entries.len());
        assert!(
            serde_json::to_vec(&ContainerResponse::Success(payload))
                .unwrap()
                .len()
                < mimageviewer_ipc::MAX_RESPONSE_FRAME_BYTES
        );
    }

    #[test]
    fn remote_page_stage_split_is_stable() {
        assert_eq!(
            RemotePageStage::ORDERED.map(RemotePageStage::name),
            [
                "resolve", "source", "compose", "trim", "resize", "jpeg", "total",
            ]
        );
    }

    #[test]
    fn remote_source_singleflight_decodes_same_identity_once() {
        let mut request = crate::thumb_loader::LoadRequest {
            path: PathBuf::from("page.png"),
            mtime: 10,
            file_size: 20,
            source_policy: crate::thumb_loader::LoadSourcePolicy::SourceOnly,
            ..Default::default()
        };
        let identity = RemoteSourceDecodeIdentity::from_load_request(
            &request,
            4096,
            RemoteImageLoadKind::AutoTrimReference.full_page(),
        );
        request.priority = true;
        let page_identity = RemoteSourceDecodeIdentity::from_load_request(
            &request,
            4096,
            RemoteImageLoadKind::CompositedPageWithAutoTrim.full_page(),
        );
        assert_eq!(identity, page_identity);

        let flight = Arc::new(RemoteSourceSingleFlight::default());
        let gate: SourceTestGate = Arc::new((Mutex::new(false), Condvar::new()));
        let decodes = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = mpsc::channel();
        let first = {
            let flight = Arc::clone(&flight);
            let identity = identity.clone();
            let gate = Arc::clone(&gate);
            let decodes = Arc::clone(&decodes);
            std::thread::spawn(move || {
                flight.load(identity, Arc::new(AtomicBool::new(false)), move |cancel| {
                    decodes.fetch_add(1, Ordering::AcqRel);
                    started_tx.send(()).unwrap();
                    assert!(source_test_gate_wait(&gate, cancel));
                    Ok(source_test_decoded(egui::Color32::RED))
                })
            })
        };
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let second = {
            let flight = Arc::clone(&flight);
            let identity = identity.clone();
            let gate = Arc::clone(&gate);
            let decodes = Arc::clone(&decodes);
            std::thread::spawn(move || {
                flight.load(identity, Arc::new(AtomicBool::new(false)), move |cancel| {
                    decodes.fetch_add(1, Ordering::AcqRel);
                    assert!(source_test_gate_wait(&gate, cancel));
                    Ok(source_test_decoded(egui::Color32::BLUE))
                })
            })
        };
        let joined = wait_for_source_participants(&flight, &identity, 2);
        source_test_gate_open(&gate);
        let first = first.join().unwrap().unwrap();
        let second = second.join().unwrap().unwrap();
        assert!(joined);
        assert_eq!(decodes.load(Ordering::Acquire), 1);
        assert!(Arc::ptr_eq(&first.raster.pixels, &second.raster.pixels));
        assert_eq!(first.outcome, RemoteSourceLoadOutcome::Decoded);
        assert_eq!(second.outcome, RemoteSourceLoadOutcome::Joined);
    }

    #[test]
    fn remote_source_handoff_reuses_sequential_raster() {
        let flight = Arc::new(RemoteSourceSingleFlight::default());
        let identity = source_test_identity("page.png", 4096, true);
        let decodes = Arc::new(AtomicUsize::new(0));
        let first = {
            let decodes = Arc::clone(&decodes);
            flight
                .load(
                    identity.clone(),
                    Arc::new(AtomicBool::new(false)),
                    move |_| {
                        decodes.fetch_add(1, Ordering::AcqRel);
                        Ok(source_test_decoded(egui::Color32::RED))
                    },
                )
                .unwrap()
        };
        let second = flight
            .load(identity, Arc::new(AtomicBool::new(false)), |_| {
                panic!("handoff must not decode")
            })
            .unwrap();
        assert_eq!(decodes.load(Ordering::Acquire), 1);
        assert_eq!(second.outcome, RemoteSourceLoadOutcome::Handoff);
        assert!(Arc::ptr_eq(&first.raster.pixels, &second.raster.pixels));
    }

    #[test]
    fn remote_source_target_px_is_part_of_identity() {
        let flight = Arc::new(RemoteSourceSingleFlight::default());
        let decodes = Arc::new(AtomicUsize::new(0));
        for target_px in [1024, 4096] {
            let decodes = Arc::clone(&decodes);
            let loaded = flight
                .load(
                    source_test_identity("page.png", target_px, true),
                    Arc::new(AtomicBool::new(false)),
                    move |_| {
                        decodes.fetch_add(1, Ordering::AcqRel);
                        Ok(source_test_decoded(egui::Color32::RED))
                    },
                )
                .unwrap();
            assert_eq!(loaded.outcome, RemoteSourceLoadOutcome::Decoded);
        }
        assert_eq!(decodes.load(Ordering::Acquire), 2);
    }

    #[test]
    fn remote_source_thumbnail_and_full_page_do_not_share() {
        let flight = Arc::new(RemoteSourceSingleFlight::default());
        let decodes = Arc::new(AtomicUsize::new(0));
        for full_page in [false, true] {
            let decodes = Arc::clone(&decodes);
            let loaded = flight
                .load(
                    source_test_identity("page.png", 1024, full_page),
                    Arc::new(AtomicBool::new(false)),
                    move |_| {
                        decodes.fetch_add(1, Ordering::AcqRel);
                        Ok(source_test_decoded(egui::Color32::RED))
                    },
                )
                .unwrap();
            assert_eq!(loaded.outcome, RemoteSourceLoadOutcome::Decoded);
        }
        assert_eq!(decodes.load(Ordering::Acquire), 2);
    }

    #[test]
    fn remote_source_waiter_retries_after_owner_failure() {
        let flight = Arc::new(RemoteSourceSingleFlight::default());
        let identity = source_test_identity("page.png", 4096, true);
        let gate: SourceTestGate = Arc::new((Mutex::new(false), Condvar::new()));
        let decodes = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = mpsc::channel();
        let owner = {
            let flight = Arc::clone(&flight);
            let identity = identity.clone();
            let gate = Arc::clone(&gate);
            let decodes = Arc::clone(&decodes);
            std::thread::spawn(move || {
                flight.load(identity, Arc::new(AtomicBool::new(false)), move |cancel| {
                    decodes.fetch_add(1, Ordering::AcqRel);
                    started_tx.send(()).unwrap();
                    assert!(source_test_gate_wait(&gate, cancel));
                    Err(media_error(MediaErrorCode::RenderFailed, "owner failed"))
                })
            })
        };
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let waiter = {
            let flight = Arc::clone(&flight);
            let identity = identity.clone();
            let decodes = Arc::clone(&decodes);
            std::thread::spawn(move || {
                flight.load(identity, Arc::new(AtomicBool::new(false)), move |_| {
                    decodes.fetch_add(1, Ordering::AcqRel);
                    Ok(source_test_decoded(egui::Color32::GREEN))
                })
            })
        };
        let joined = wait_for_source_participants(&flight, &identity, 2);
        source_test_gate_open(&gate);
        let owner_error = owner.join().unwrap().unwrap_err();
        let waiter = waiter.join().unwrap().unwrap();
        assert!(joined);
        assert_eq!(owner_error.code, MediaErrorCode::RenderFailed);
        assert_eq!(waiter.outcome, RemoteSourceLoadOutcome::Decoded);
        assert_eq!(decodes.load(Ordering::Acquire), 2);
    }

    #[test]
    fn remote_source_all_participants_cancel_shared_decode() {
        let flight = Arc::new(RemoteSourceSingleFlight::default());
        let identity = source_test_identity("page.png", 4096, true);
        let owner_cancel = Arc::new(AtomicBool::new(false));
        let waiter_cancel = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = mpsc::channel();
        let (shared_tx, shared_rx) = mpsc::channel();
        let owner = {
            let flight = Arc::clone(&flight);
            let identity = identity.clone();
            let cancel = Arc::clone(&owner_cancel);
            std::thread::spawn(move || {
                flight.load(identity, cancel, move |shared_cancel| {
                    started_tx.send(()).unwrap();
                    while !shared_cancel.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    shared_tx.send(()).unwrap();
                    Err(RemoteSourceSingleFlight::participant_cancelled_error())
                })
            })
        };
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let waiter = {
            let flight = Arc::clone(&flight);
            let identity = identity.clone();
            let cancel = Arc::clone(&waiter_cancel);
            std::thread::spawn(move || {
                flight.load(identity, cancel, |_| {
                    panic!("cancelled waiter must not decode")
                })
            })
        };
        let joined = wait_for_source_participants(&flight, &identity, 2);
        owner_cancel.store(true, Ordering::Release);
        waiter_cancel.store(true, Ordering::Release);
        shared_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let owner = owner.join().unwrap();
        let waiter = waiter.join().unwrap();
        assert!(joined);
        assert_eq!(owner.unwrap_err().code, MediaErrorCode::Busy);
        assert_eq!(waiter.unwrap_err().code, MediaErrorCode::Busy);
    }

    #[test]
    fn remote_source_one_participant_cancel_does_not_cancel_remaining_decode() {
        let flight = Arc::new(RemoteSourceSingleFlight::default());
        let identity = source_test_identity("page.png", 4096, true);
        let gate: SourceTestGate = Arc::new((Mutex::new(false), Condvar::new()));
        let owner_cancel = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = mpsc::channel();
        let owner = {
            let flight = Arc::clone(&flight);
            let identity = identity.clone();
            let gate = Arc::clone(&gate);
            let cancel = Arc::clone(&owner_cancel);
            std::thread::spawn(move || {
                flight.load(identity, cancel, move |shared_cancel| {
                    started_tx.send(()).unwrap();
                    assert!(source_test_gate_wait(&gate, shared_cancel));
                    Ok(source_test_decoded(egui::Color32::GREEN))
                })
            })
        };
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let waiter = {
            let flight = Arc::clone(&flight);
            let identity = identity.clone();
            std::thread::spawn(move || {
                flight.load(identity, Arc::new(AtomicBool::new(false)), |_| {
                    panic!("joined participant must not decode")
                })
            })
        };
        assert!(wait_for_source_participants(&flight, &identity, 2));
        owner_cancel.store(true, Ordering::Release);
        assert!(wait_for_source_participants(&flight, &identity, 1));
        assert_eq!(source_shared_cancelled(&flight, &identity), Some(false));
        source_test_gate_open(&gate);
        let owner = owner.join().unwrap().unwrap_err();
        let waiter = waiter.join().unwrap().unwrap();
        assert_eq!(owner.code, MediaErrorCode::Busy);
        assert_eq!(waiter.outcome, RemoteSourceLoadOutcome::Joined);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic]
    fn remote_source_owner_cannot_wait_on_different_identity() {
        let owned = source_test_identity("left.png", 4096, true);
        let other = source_test_identity("right.png", 4096, true);
        let flight = RemoteSourceSingleFlight::default();
        let _owner = RemoteSourceDecodeOwnerScope::enter(owned);

        let _ = flight.acquire(&other);
    }

    #[test]
    fn a_full_page_request_does_not_stop_to_make_a_thumbnail() {
        let mut settings = crate::settings::Settings::default();
        settings.cache_policy = crate::settings::CachePolicy::Always;
        settings.cache_pdf_always = true;
        let page = Path::new("book.pdf");

        let decision = remote_page_cache_decision(true, &settings);

        // 設定が「常に保存」でも、PDF の無条件保存に当たっても、遅くても、大きくても保存しない。
        assert!(!decision.should_cache(page, 40 * 1024 * 1024, 5_000.0, 5_000.0));
    }

    #[test]
    fn a_thumbnail_request_still_follows_the_settings() {
        let mut settings = crate::settings::Settings::default();
        settings.cache_policy = crate::settings::CachePolicy::Always;
        let page = Path::new("book.pdf");

        let decision = remote_page_cache_decision(false, &settings);

        assert!(decision.should_cache(page, 1, 0.0, 0.0));
    }

    #[test]
    fn remote_page_phase_summary_keeps_empty_stage_time_unaccounted() {
        let (phases, unaccounted_ms) = remote_page_phase_summary(12.5, &[]);

        assert_eq!(phases, None);
        assert_eq!(unaccounted_ms, 12.5);
    }

    #[test]
    fn remote_page_phase_summary_accumulates_duplicate_names() {
        let (phases, unaccounted_ms) =
            remote_page_phase_summary(10.0, &[("decode", 2.0), ("decode", 3.0), ("display", 1.0)]);

        assert_eq!(
            phases,
            Some(serde_json::json!({
                "decode": 5.0,
                "display": 1.0,
            }))
        );
        assert_eq!(unaccounted_ms, 4.0);
    }

    #[test]
    fn remote_page_phase_summary_clamps_negative_unaccounted_time() {
        let (phases, unaccounted_ms) =
            remote_page_phase_summary(5.0, &[("first", 3.0), ("second", 4.0)]);

        assert!(phases.is_some());
        assert_eq!(unaccounted_ms, 0.0);
    }

    #[test]
    fn remote_page_concurrency_transition_counts_other_requests() {
        assert_eq!(
            remote_page_concurrency_on_enter(0),
            RemotePageConcurrency {
                active_others: 0,
                active_total: 1,
            }
        );
        assert_eq!(
            remote_page_concurrency_on_enter(2),
            RemotePageConcurrency {
                active_others: 2,
                active_total: 3,
            }
        );
        assert_eq!(remote_page_concurrency_on_exit(3), Some(2));
        assert_eq!(remote_page_concurrency_on_exit(0), None);
    }

    fn cancel_after_remote_page_stage_enter(counter: &RemotePageStageCounter) -> Result<(), ()> {
        let _lease = RemotePageStageLease::new(counter);
        Err(())
    }

    #[test]
    fn remote_page_stage_lease_releases_on_nested_drop_and_cancel_return() {
        let counter = RemotePageStageCounter::new();
        {
            let first = RemotePageStageLease::new(&counter);
            assert_eq!(first.concurrency.active_others, 0);
            assert_eq!(counter.active(), 1);
            {
                let second = RemotePageStageLease::new(&counter);
                assert_eq!(second.concurrency.active_others, 1);
                assert_eq!(counter.active(), 2);
            }
            assert_eq!(counter.active(), 1);
        }
        assert_eq!(counter.active(), 0);
        assert_eq!(cancel_after_remote_page_stage_enter(&counter), Err(()));
        assert_eq!(counter.active(), 0);
    }

    fn stored_edit_test_crop() -> crate::export_crop::CropSettings {
        crate::export_crop::CropSettings {
            rect: crate::export_crop::CropRect {
                min_x: 14.0,
                min_y: 22.0,
                max_x: 126.0,
                max_y: 198.0,
            },
            aspect_mode: crate::export_crop::CropAspectMode::Free,
            source_size: None,
        }
    }

    fn stored_edit_test_raster(size: [usize; 2]) -> egui::ColorImage {
        egui::ColorImage::new(size, vec![egui::Color32::BLUE; size[0] * size[1]])
    }

    #[test]
    fn pdf_saved_crop_range_is_independent_of_requested_resolution() {
        let canonical_dims = crate::pdf_loader::canonical_pdf_raster_dims(
            crate::pdf_loader::PdfPageContentType::Raster { w: 3905, h: 5953 },
        )
        .map(|[width, height]| [width as usize, height as usize])
        .unwrap();
        let crop = crate::export_crop::CropSettings {
            rect: crate::export_crop::CropRect {
                min_x: canonical_dims[0] as f32 * 0.2,
                min_y: canonical_dims[1] as f32 * 0.25,
                max_x: canonical_dims[0] as f32 * 0.8,
                max_y: canonical_dims[1] as f32 * 0.75,
            },
            aspect_mode: crate::export_crop::CropAspectMode::Free,
            source_size: None,
        };
        let expected_page_fractions = [0.2_f32, 0.25, 0.8, 0.75];

        for target_px in [1024_u32, 4096, 8192] {
            let (width, height) = crate::fast_resize::aspect_accurate_fit_dimensions(
                (target_px, target_px),
                (target_px, target_px),
                (canonical_dims[0] as u32, canonical_dims[1] as u32),
            );
            let rendered_dims = [width as usize, height as usize];
            let stored_edit_space = StoredEditSpace::for_remote_source(
                &RemoteSubresource::PdfPage { page_number: 0 },
                rendered_dims,
                // Real catalog page-box values remain ratio metadata and must not affect crop.
                Some([468_600, 714_360]),
                Some(canonical_dims),
            )
            .unwrap();

            assert_eq!(stored_edit_space.canonical_dims, canonical_dims);
            let rect = stored_edit_space.crop_rect(crop, rendered_dims);
            let actual_page_fractions = [
                rect.min_x / rendered_dims[0] as f32,
                rect.min_y / rendered_dims[1] as f32,
                rect.max_x / rendered_dims[0] as f32,
                rect.max_y / rendered_dims[1] as f32,
            ];
            for (actual, expected) in actual_page_fractions
                .into_iter()
                .zip(expected_page_fractions)
            {
                assert!(
                    (actual - expected).abs() < 1e-5,
                    "target_px={target_px}: actual={actual}, expected={expected}"
                );
            }
        }
    }

    #[test]
    fn pdf_saved_edits_do_not_fall_back_to_the_requested_raster() {
        for rendered_dims in [[663, 1024], [2687, 4096], [5374, 8192]] {
            assert_eq!(
                StoredEditSpace::for_remote_source(
                    &RemoteSubresource::PdfPage { page_number: 0 },
                    rendered_dims,
                    Some([468_600, 714_360]),
                    None,
                ),
                None
            );
        }
    }

    #[test]
    fn vector_pdf_keeps_the_full_page_skips_edits_and_emits_a_typed_record() {
        let page = Arc::new(stored_edit_test_raster([70, 110]));
        let stored_edit_space = StoredEditSpace::for_remote_source(
            &RemoteSubresource::PdfPage { page_number: 4 },
            page.size,
            Some([468_600, 714_360]),
            None,
        );

        assert!(
            stored_edit_space.is_none(),
            "vector PDF must not use target raster"
        );
        let output = crop_with_stored_edit_space(
            Arc::clone(&page),
            Some(stored_edit_test_crop()),
            stored_edit_space,
        )
        .expect("missing canonical space skips the crop instead of failing the page");
        assert!(Arc::ptr_eq(&output, &page));
        assert_eq!(output.size, [70, 110]);
        assert!(
            stored_edit_space.is_none(),
            "the same unavailable space also prevents comic composition"
        );

        let mut records = Vec::new();
        record_skipped_stored_edits_with(
            SkippedStoredEdits {
                pipeline: StoredEditPipeline::Page,
                page_number: 4,
                reason: StoredEditSkipReason::PdfVectorHasNoCanonicalRaster,
                crop: true,
                comic_objects: 3,
            },
            |line| records.push(line),
        );
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0],
            "remote_ipc: stored_edit outcome=skipped pipeline=page target=pdf_page page_number=4 reason=pdf_vector_no_canonical_raster crop=true comic_objects=3"
        );
    }

    #[test]
    fn regular_image_saved_crop_keeps_original_pixel_coordinate_space() {
        let raster = stored_edit_test_raster([70, 110]);
        let stored_edit_space = StoredEditSpace::for_remote_source(
            &RemoteSubresource::File,
            raster.size,
            Some([140, 220]),
            None,
        )
        .unwrap();

        assert_eq!(stored_edit_space.canonical_dims, [140, 220]);
        let crop = stored_edit_space.crop_rect(stored_edit_test_crop(), raster.size);
        let output = crate::export_crop::crop_color_image(&raster, crop).unwrap();
        assert_eq!(output.size, [56, 88]);
    }

    #[test]
    /// **キャッシュ方針で表示の仕様が変わってはいけない。**
    ///
    /// 既定の `Auto` 方針では、速くて小さい画像が並ぶフォルダにカタログ行が 1 つも
    /// 作られない。カタログだけに頼っていたため横長判定が全ページ false になり、
    /// 見開きの単独表示も横長分割も効かなかった (2026-08-26 の利用者報告)。
    fn a_plain_image_file_reports_its_shape_without_any_catalog() {
        let dir = tempfile::tempdir().expect("tempdir");
        let landscape = dir.path().join("wide.png");
        image::RgbImage::new(120, 40)
            .save(&landscape)
            .expect("write landscape");
        let portrait = dir.path().join("tall.png");
        image::RgbImage::new(40, 120)
            .save(&portrait)
            .expect("write portrait");

        assert_eq!(
            page_dims_without_catalog(&crate::grid_item::GridItem::Image(landscape)),
            Some((120, 40))
        );
        assert_eq!(
            page_dims_without_catalog(&crate::grid_item::GridItem::Image(portrait)),
            Some((40, 120))
        );
        // 存在しないファイルは「不明」。横長として扱わない。
        assert_eq!(
            page_dims_without_catalog(&crate::grid_item::GridItem::Image(
                dir.path().join("missing.png")
            )),
            None
        );
    }

    #[test]
    /// **カタログが揃っている PDF に追加費用を出さない。**
    ///
    /// 1 ページでも寸法が無ければ文書ごと 1 往復で取りに行くが、揃っていれば行かない。
    fn pdf_page_sizes_are_fetched_only_when_the_catalog_misses_a_page() {
        let items = (0..3)
            .map(|page_num| crate::grid_item::GridItem::PdfPage {
                pdf_path: std::path::PathBuf::from("book.pdf"),
                page_num,
                content_type: None,
            })
            .collect::<Vec<_>>();
        let mut cached = std::collections::HashMap::new();
        for page_num in 0..3 {
            cached.insert(
                crate::grid_item::pdf_page_cache_key(page_num),
                Some((600u32, 800u32)),
            );
        }
        assert!(!pdf_page_sizes_needed(&items, &cached));

        // 寸法列が空の古い行でも「行はある」ので取りに行かない (サムネイルから復元できる)。
        cached.insert(crate::grid_item::pdf_page_cache_key(1), None);
        assert!(!pdf_page_sizes_needed(&items, &cached));

        // 行そのものが無いページが 1 つでもあれば取りに行く。
        cached.remove(&crate::grid_item::pdf_page_cache_key(2));
        assert!(pdf_page_sizes_needed(&items, &cached));

        // PDF ページが 1 つも無ければ関係ない。
        assert!(!pdf_page_sizes_needed(
            &[crate::grid_item::GridItem::Image(std::path::PathBuf::from(
                "a.jpg"
            ))],
            &std::collections::HashMap::new()
        ));
    }

    #[test]
    /// ZIP / PDF ページはこの経路では補わない (書庫展開と worker 往復が要るため)。
    /// **意図した範囲であることを固定する。**広げるときはここが落ちる。
    fn archive_and_pdf_pages_still_depend_on_the_catalog() {
        assert_eq!(
            page_dims_without_catalog(&crate::grid_item::GridItem::ZipImage {
                zip_path: std::path::PathBuf::from("book.zip"),
                entry_name: "001.jpg".into(),
            }),
            None
        );
        assert_eq!(
            page_dims_without_catalog(&crate::grid_item::GridItem::PdfPage {
                pdf_path: std::path::PathBuf::from("book.pdf"),
                page_num: 0,
                content_type: None,
            }),
            None
        );
    }

    #[test]
    fn cached_landscape_flags_apply_saved_pdf_page_rotation() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let pdf_path = data_dir.path().join("rotated.pdf");
        let items = (0..2)
            .map(|page_num| crate::grid_item::GridItem::PdfPage {
                pdf_path: pdf_path.clone(),
                page_num,
                content_type: None,
            })
            .collect::<Vec<_>>();
        let catalog =
            crate::catalog::CatalogDb::open(&crate::catalog::default_cache_dir(), &pdf_path)
                .unwrap();
        for page_num in 0..2 {
            catalog
                .save(
                    &crate::grid_item::pdf_page_cache_key(page_num),
                    1,
                    10,
                    100,
                    144,
                    Some((503, 727)),
                    b"thumb",
                )
                .unwrap();
        }
        drop(catalog);

        let rotation_db = crate::rotation_db::RotationDb::open().unwrap();
        let page_key = crate::edit_source::page_key_for_grid_item(&items[0]).unwrap();
        rotation_db
            .set_key(&page_key, crate::rotation_db::Rotation::Cw270)
            .unwrap();
        drop(rotation_db);

        let engine = ContainerEngine::new(crate::settings::Settings::default());
        assert_eq!(
            engine.cached_landscape_flags(&pdf_path, &items),
            [true, false]
        );
    }

    #[test]
    /// **カタログが 1 行も無いフォルダでも横長を見つける。**
    ///
    /// 既定のキャッシュ方針 `Auto` は、速くて小さい画像のフォルダにカタログを作らない。
    /// カタログだけを見ていたため、その種のフォルダでは横長ページが 1 枚も見つからず、
    /// 見開きの単独表示も横長分割も効かなかった (2026-08-26 の利用者報告)。
    fn landscape_is_found_in_a_folder_that_has_no_catalog_at_all() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let folder = data_dir.path().join("book");
        std::fs::create_dir_all(&folder).unwrap();
        let wide = folder.join("001.png");
        let tall = folder.join("002.png");
        image::RgbImage::new(200, 100).save(&wide).unwrap();
        image::RgbImage::new(100, 200).save(&tall).unwrap();
        let items = vec![
            crate::grid_item::GridItem::Image(wide),
            crate::grid_item::GridItem::Image(tall),
        ];

        // カタログは作らない。この状態が既定方針での通常のフォルダである。
        assert!(
            crate::catalog::CatalogDb::open_existing_read_only(
                &crate::catalog::default_cache_dir(),
                &folder,
            )
            .ok()
            .flatten()
            .is_none(),
            "この試験はカタログが無い状態を確かめるもの"
        );

        let engine = ContainerEngine::new(crate::settings::Settings::default());
        assert_eq!(
            engine.cached_landscape_flags(&folder, &items),
            [true, false]
        );
    }

    #[test]
    fn page_render_observes_the_registry_job_token() {
        // WorkerContext::open() と CatalogDb::open() は data_dir::get() へ落ちる。
        // ガードが無いと本物の %APPDATA% のカタログを開き、同時に走る他テストの
        // TEST_OVERRIDE (プロセス共有) を踏んで開けなくなる。
        let _data_dir = crate::data_dir::TestDataDirGuard::new();
        let engine = ContainerEngine::new(crate::settings::Settings::default());
        let context = WorkerContext::open();
        let cancel = Arc::new(AtomicBool::new(true));
        let response = engine.page_with_job_cancel(
            PageRequest {
                job_id: "cancelled-page".to_owned(),
                display_request_id: Some("cancelled-display".to_owned()),
                address: RemoteAddress::file(r"C:\Pictures\page.jpg"),
                target_px: 2048,
                priority: PagePriority::Foreground,
                render_context: None,
                adjustment_preview: None,
            },
            &context,
            cancel,
        );
        assert!(matches!(
            response,
            PageResponse::Error(MediaError {
                code: MediaErrorCode::Cancelled,
                ..
            })
        ));
    }

    fn favorite_address(favorite: &FavoriteEntry, relative: impl AsRef<Path>) -> RemoteAddress {
        let path = favorite.path.join(relative);
        let logical = super::super::path_guard::resolve_existing(path.to_string_lossy().as_ref())
            .map(|resolved| resolved.logical)
            .unwrap_or(path);
        RemoteAddress::file(logical.to_string_lossy().into_owned())
    }

    struct NoRemoteAiProgress;

    impl super::super::ai_job::RemoteAiProgressSink for NoRemoteAiProgress {
        fn update(
            &self,
            _state: mimageviewer_ipc::RemoteAiJobState,
            _progress: Option<mimageviewer_ipc::RemoteAiProgress>,
        ) {
        }
    }

    fn remote_ai_test_png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([
                (x * 53 + y * 7) as u8,
                (x * 11 + y * 61) as u8,
                (x * 29 + y * 17) as u8,
                255,
            ])
        });
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn remote_page_test_image(width: usize, height: usize) -> egui::ColorImage {
        let pixels = (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    egui::Color32::from_rgb(
                        (x * 37 + y * 11) as u8,
                        (x * 13 + y * 47) as u8,
                        (x * 29 + y * 19) as u8,
                    )
                })
            })
            .collect();
        egui::ColorImage::new([width, height], pixels)
    }

    fn encode_remote_page_jpeg_legacy(
        image: &egui::ColorImage,
        long_side: u32,
        view_trim_bbox: Option<egui::Rect>,
    ) -> Option<(Vec<u8>, u32, u32)> {
        let width = u32::try_from(image.size[0]).ok()?;
        let height = u32::try_from(image.size[1]).ok()?;
        let rgba = crate::capture::color_image_to_rgba(image);
        let source =
            image::RgbaImage::from_raw(width, height, rgba).map(image::DynamicImage::ImageRgba8)?;
        let cropped;
        let source = if let Some(bbox) = view_trim_bbox {
            let rect = crate::export_crop::CropRect {
                min_x: bbox.min.x * source.width() as f32,
                min_y: bbox.min.y * source.height() as f32,
                max_x: bbox.max.x * source.width() as f32,
                max_y: bbox.max.y * source.height() as f32,
            };
            let (x, y, width, height) =
                rect.pixel_bounds(source.width() as usize, source.height() as usize);
            cropped = source.crop_imm(x as u32, y as u32, width as u32, height as u32);
            &cropped
        } else {
            &source
        };
        let resized = crate::fast_resize::resize_dynamic_fit(
            source,
            long_side,
            long_side,
            crate::fast_resize::Quality::Lanczos3,
        );
        let rgb = resized.to_rgb8();
        let (width, height) = (rgb.width(), rgb.height());
        let bytes = turbojpeg::compress_image(&rgb, PAGE_JPEG_QUALITY, turbojpeg::Subsamp::Sub2x2)
            .ok()?
            .to_vec();
        Some((bytes, width, height))
    }

    #[test]
    fn remote_page_jpeg_opaque_zero_copy_is_byte_identical_to_legacy() {
        let image = remote_page_test_image(40, 20);

        let actual = encode_remote_page_jpeg(&image, 8192, None).expect("JPEG encode");
        let legacy = encode_remote_page_jpeg_legacy(&image, 8192, None).expect("legacy encode");

        assert_eq!(actual.0, legacy.0);
        assert_eq!((actual.1, actual.2), (legacy.1, legacy.2));
        assert_eq!((actual.1, actual.2), (40, 20));
    }

    #[test]
    fn remote_page_jpeg_translucent_pixel_falls_back_byte_identical_to_legacy() {
        let mut image = remote_page_test_image(40, 20);
        image.pixels[17] = egui::Color32::from_rgba_unmultiplied(220, 140, 60, 64);
        let raw = &image.as_raw()[17 * 4..17 * 4 + 4];
        assert_ne!(raw, image.pixels[17].to_srgba_unmultiplied());

        let actual = encode_remote_page_jpeg(&image, 8192, None).expect("JPEG encode");
        let legacy = encode_remote_page_jpeg_legacy(&image, 8192, None).expect("legacy encode");

        assert_eq!(actual.0, legacy.0);
        assert_eq!((actual.1, actual.2), (legacy.1, legacy.2));
    }

    #[test]
    fn remote_page_jpeg_zero_copy_crop_with_odd_width_is_byte_identical_to_legacy() {
        let image = remote_page_test_image(13, 9);
        let bbox = egui::Rect::from_min_max(egui::pos2(0.2, 0.2), egui::pos2(0.5, 0.8));
        let rect = crate::export_crop::CropRect {
            min_x: bbox.min.x * image.size[0] as f32,
            min_y: bbox.min.y * image.size[1] as f32,
            max_x: bbox.max.x * image.size[0] as f32,
            max_y: bbox.max.y * image.size[1] as f32,
        };
        let (_, _, crop_width, crop_height) = rect.pixel_bounds(image.size[0], image.size[1]);
        assert_eq!((crop_width, crop_height), (5, 7));

        let actual = encode_remote_page_jpeg(&image, 8192, Some(bbox)).expect("JPEG encode");
        let legacy =
            encode_remote_page_jpeg_legacy(&image, 8192, Some(bbox)).expect("legacy encode");

        assert_eq!(actual.0, legacy.0);
        assert_eq!((actual.1, actual.2), (legacy.1, legacy.2));
        assert_eq!((actual.1, actual.2), (5, 7));
    }

    #[test]
    fn remote_page_jpeg_resize_matches_legacy_bytes_and_fit_dimensions() {
        let image = remote_page_test_image(40, 21);
        let expected =
            crate::fast_resize::aspect_accurate_fit_dimensions((40, 21), (10, 10), (40, 21));

        let actual = encode_remote_page_jpeg(&image, 10, None).expect("JPEG encode");
        let legacy = encode_remote_page_jpeg_legacy(&image, 10, None).expect("legacy encode");

        assert_eq!(actual.0, legacy.0);
        assert_eq!((actual.1, actual.2), (legacy.1, legacy.2));
        assert_eq!((actual.1, actual.2), expected);
    }

    #[test]
    fn remote_page_rotation_matches_all_saved_quarter_turns() {
        let source = Arc::new(remote_page_test_image(3, 2));
        let original = source.pixels.clone();

        let cases = [
            (
                crate::rotation_db::Rotation::None,
                [3, 2],
                vec![0, 1, 2, 3, 4, 5],
            ),
            (
                crate::rotation_db::Rotation::Cw90,
                [2, 3],
                vec![3, 0, 4, 1, 5, 2],
            ),
            (
                crate::rotation_db::Rotation::Cw180,
                [3, 2],
                vec![5, 4, 3, 2, 1, 0],
            ),
            (
                crate::rotation_db::Rotation::Cw270,
                [2, 3],
                vec![2, 5, 1, 4, 0, 3],
            ),
        ];

        for (rotation, expected_size, expected_indices) in cases {
            let actual = apply_remote_page_rotation(Arc::clone(&source), rotation);
            assert_eq!(actual.size, expected_size);
            assert_eq!(
                actual.pixels,
                expected_indices
                    .into_iter()
                    .map(|index| original[index])
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn remote_page_rotation_disables_trim_and_separates_composite_cache_entries() {
        let data_dir = crate::data_dir::TestDataDirGuard::new();
        let book = data_dir.path().join("book");
        std::fs::create_dir_all(&book).unwrap();
        let page_path = book.join("page.png");
        std::fs::write(&page_path, remote_ai_test_png(100, 60)).unwrap();

        let view_trim_db =
            crate::view_trim_db::ViewTrimDb::open_at(&data_dir.path().join("view_trim.db"))
                .unwrap();
        view_trim_db
            .set_book_state(
                &book,
                crate::view_trim::ViewTrimBookState {
                    apply_mode: crate::view_trim::ViewTrimApplyMode::Book,
                    book_settings: crate::view_trim::ViewTrimBookSettings {
                        enabled: true,
                        single: crate::view_trim::ViewTrimMargins {
                            left: 0.10,
                            top: 0.20,
                            right: 0.15,
                            bottom: 0.05,
                        },
                        ..Default::default()
                    },
                },
            )
            .unwrap();
        drop(view_trim_db);

        // A composited page treats every edit database as part of one coherent snapshot.
        // Create the empty schemas before the worker opens its read-only handles.
        drop(crate::adjustment_db::AdjustmentDb::open().unwrap());
        drop(crate::mask_db::MaskDb::open().unwrap());
        drop(crate::local_adjust_db::LocalAdjustDb::open().unwrap());
        drop(crate::conceal_db::ConcealDb::open().unwrap());
        drop(crate::comic_db::ComicDb::open().unwrap());
        drop(crate::export_crop::CropDb::open().unwrap());

        let engine = ContainerEngine::new(crate::settings::Settings::default());
        let address = RemoteAddress::file(page_path.to_string_lossy().into_owned());
        let resolved = engine.resolve(&address).unwrap();
        let page_key =
            crate::edit_source::page_key_for_remote(&resolved.logical, &address.subresource)
                .unwrap();
        let rotation_db = crate::rotation_db::RotationDb::open().unwrap();
        let context = WorkerContext::open();
        let render = |rotation: crate::rotation_db::Rotation| {
            rotation_db.set_key(&page_key, rotation).unwrap();
            match engine.page_with_job_cancel(
                PageRequest {
                    job_id: format!("rotation-{}", rotation.degrees()),
                    display_request_id: Some(format!("display-{}", rotation.degrees())),
                    address: address.clone(),
                    target_px: 1024,
                    priority: PagePriority::Foreground,
                    render_context: None,
                    adjustment_preview: None,
                },
                &context,
                Arc::new(AtomicBool::new(false)),
            ) {
                PageResponse::Success(payload) => payload,
                PageResponse::Error(error) => panic!("remote page failed: {error:?}"),
            }
        };

        let unrotated = render(crate::rotation_db::Rotation::None);
        assert_eq!((unrotated.width, unrotated.height), (75, 45));
        let clockwise = render(crate::rotation_db::Rotation::Cw90);
        assert_eq!((clockwise.width, clockwise.height), (60, 100));
        let upside_down = render(crate::rotation_db::Rotation::Cw180);
        assert_eq!((upside_down.width, upside_down.height), (100, 60));
        let counterclockwise = render(crate::rotation_db::Rotation::Cw270);
        assert_eq!((counterclockwise.width, counterclockwise.height), (60, 100));

        let invalid_context = RemotePageRenderContext {
            context_address: address.clone(),
            display_slot: RemotePageDisplaySlot::Single,
            spread_partner: None,
        };
        assert!(matches!(
            engine.remote_view_trim_plan(
                &address,
                &resolved,
                Some(&invalid_context),
                crate::rotation_db::Rotation::Cw90,
            ),
            Err(MediaError {
                code: MediaErrorCode::BadRequest,
                ..
            })
        ));

        let cache = engine
            .page_composite_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for rotation in [
            crate::rotation_db::Rotation::None,
            crate::rotation_db::Rotation::Cw90,
            crate::rotation_db::Rotation::Cw180,
            crate::rotation_db::Rotation::Cw270,
        ] {
            assert!(
                cache
                    .entries
                    .iter()
                    .any(|entry| entry.key.rotation == rotation)
            );
        }
    }

    fn auto_trim_test_page(top: usize, bottom: usize) -> egui::ColorImage {
        let mut image = egui::ColorImage::new([200, 200], vec![egui::Color32::WHITE; 200 * 200]);
        for y in top..(200 - bottom) {
            for x in 20..180 {
                image.pixels[y * 200 + x] = egui::Color32::BLACK;
            }
        }
        image
    }

    fn write_auto_trim_test_page(path: &Path, top: u32, bottom: u32) {
        let image = image::RgbaImage::from_fn(200, 200, |x, y| {
            if (20..180).contains(&x) && y >= top && y < 200 - bottom {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        });
        image::DynamicImage::ImageRgba8(image)
            .save_with_format(path, image::ImageFormat::Png)
            .unwrap();
    }

    #[test]
    fn remote_auto_trim_detects_raw_pages_and_harmonizes_spread_top_and_bottom() {
        let left = auto_trim_test_page(40, 20);
        let right = auto_trim_test_page(20, 40);
        let left_bbox =
            crate::margin_fit::detect_content_bbox(&left, crate::margin_fit::DEFAULT_TOLERANCE)
                .unwrap();
        let right_bbox =
            crate::margin_fit::detect_content_bbox(&right, crate::margin_fit::DEFAULT_TOLERANCE)
                .unwrap();

        let harmonized_left = harmonized_remote_auto_bbox(
            crate::view_trim::ViewTrimSpreadSide::Left,
            Some(left_bbox),
            Some(right_bbox),
        )
        .unwrap();
        let harmonized_right = harmonized_remote_auto_bbox(
            crate::view_trim::ViewTrimSpreadSide::Right,
            Some(right_bbox),
            Some(left_bbox),
        )
        .unwrap();

        assert!((harmonized_left.min.y - right_bbox.min.y).abs() < 1e-6);
        assert!((harmonized_left.max.y - left_bbox.max.y).abs() < 1e-6);
        assert!((harmonized_right.min.y - right_bbox.min.y).abs() < 1e-6);
        assert!((harmonized_right.max.y - left_bbox.max.y).abs() < 1e-6);
        assert!((harmonized_left.min.x - left_bbox.min.x).abs() < 1e-6);
        assert!((harmonized_right.min.x - right_bbox.min.x).abs() < 1e-6);
    }

    /// `cached_spread_partner_does_not_spawn_or_decode` only proves the scope helper honours a
    /// `Cached` start. Nothing checked that a warm cache actually produces one, and a regression
    /// there is silent: every page of an already-read spread would spawn a thread and decode the
    /// partner again while still returning the right picture.
    #[test]
    fn a_warm_partner_bbox_skips_the_speculative_resolution() {
        // 下の `remote_auto_trim_page_responses_...` と同じ理由 (data_dir::get() 到達)。
        let _data_dir = crate::data_dir::TestDataDirGuard::new();
        let temp = tempfile::tempdir().unwrap();
        let book = temp.path().join("book");
        std::fs::create_dir(&book).unwrap();
        write_auto_trim_test_page(&book.join("left.png"), 40, 20);
        write_auto_trim_test_page(&book.join("right.png"), 20, 40);
        let db_path = temp.path().join("view_trim.db");
        let db = crate::view_trim_db::ViewTrimDb::open_at(&db_path).unwrap();
        db.set_book_state(
            &book,
            crate::view_trim::ViewTrimBookState {
                apply_mode: crate::view_trim::ViewTrimApplyMode::Auto,
                ..Default::default()
            },
        )
        .unwrap();
        drop(db);

        let favorite = FavoriteEntry::new("test".to_owned(), temp.path().to_path_buf());
        let engine = ContainerEngine::new(crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        });
        *engine
            .view_trim_db
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            crate::view_trim_db::ViewTrimDb::open_existing_read_only_at(&db_path).unwrap();
        let left = favorite_address(&favorite, "book/left.png");
        let right = favorite_address(&favorite, "book/right.png");
        let left_resolved = engine.resolve(&left).unwrap();
        let plan = engine
            .remote_view_trim_plan(
                &left,
                &left_resolved,
                Some(&RemotePageRenderContext {
                    context_address: favorite_address(&favorite, "book"),
                    display_slot: RemotePageDisplaySlot::SpreadLeft,
                    spread_partner: Some(right.clone()),
                }),
                crate::rotation_db::Rotation::None,
            )
            .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));

        assert!(
            matches!(
                engine
                    .prepare_remote_auto_trim_partner_timed(&plan, 1024, &cancel, None)
                    .unwrap(),
                RemotePartnerStart::Resolve(_)
            ),
            "a cold partner has to be resolved"
        );

        // Warm it the way the partner's own request would, so the key this lookup builds has to
        // match the key that insert built.
        let worker = WorkerContext::open();
        let right_resolved = engine.resolve(&right).unwrap();
        let expected = engine
            .remote_auto_trim_bbox(&right, &right_resolved, 1024, true, &worker, &cancel)
            .unwrap();

        match engine
            .prepare_remote_auto_trim_partner_timed(&plan, 1024, &cancel, None)
            .unwrap()
        {
            RemotePartnerStart::Cached(bbox) => assert_eq!(bbox, expected),
            _ => panic!("a warm partner bbox must not start a speculative resolution"),
        }

        assert!(
            matches!(
                engine
                    .prepare_remote_auto_trim_partner_timed(&plan, 2048, &cancel, None)
                    .unwrap(),
                RemotePartnerStart::Resolve(_)
            ),
            "another target_px is another raster, so it has to be resolved again"
        );
    }

    #[test]
    fn remote_auto_trim_cache_keeps_none_and_invalidates_on_source_or_decode_change() {
        let key = RemoteAutoTrimCacheKey {
            page_key: "book/page.png".to_owned(),
            mtime: 10,
            file_size: 20,
            target_px: 4096,
        };
        let mut cache = RemoteAutoTrimCache::default();
        cache.insert(key.clone(), None);
        assert_eq!(cache.get(&key), Some(None));

        let mut changed_source = key.clone();
        changed_source.mtime += 1;
        assert_eq!(cache.get(&changed_source), None);

        let mut changed_size = key.clone();
        changed_size.file_size += 1;
        assert_eq!(cache.get(&changed_size), None);

        let mut changed_decode = key;
        changed_decode.target_px = 2048;
        assert_eq!(cache.get(&changed_decode), None);
    }

    #[test]
    fn remote_view_trim_resolves_book_and_page_rows_for_spread_side() {
        let temp = tempfile::tempdir().unwrap();
        let book = temp.path().join("book");
        std::fs::create_dir(&book).unwrap();
        let page_override_path = book.join("override.png");
        let page_book_path = book.join("book.png");
        std::fs::write(&page_override_path, b"page").unwrap();
        std::fs::write(&page_book_path, b"page").unwrap();
        let db_path = temp.path().join("view_trim.db");
        let db = crate::view_trim_db::ViewTrimDb::open_at(&db_path).unwrap();
        db.set_book_state(
            &book,
            crate::view_trim::ViewTrimBookState {
                apply_mode: crate::view_trim::ViewTrimApplyMode::Book,
                book_settings: crate::view_trim::ViewTrimBookSettings {
                    enabled: true,
                    spread_linked: crate::view_trim::ViewTrimLinkedMargins {
                        inner: 0.08,
                        outer: 0.02,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            },
        )
        .unwrap();
        db.set_page_override(
            &crate::adjustment_db::normalize_path(&page_override_path),
            crate::view_trim::ViewTrimPageOverride::from_spread_margins(
                crate::view_trim::ViewTrimMargins {
                    left: 0.03,
                    right: 0.09,
                    ..Default::default()
                },
                crate::view_trim::ViewTrimSpreadSide::Left,
            ),
        )
        .unwrap();
        drop(db);

        let favorite = FavoriteEntry::new("test".to_owned(), temp.path().to_path_buf());
        let engine = ContainerEngine::new(crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        });
        *engine
            .view_trim_db
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            crate::view_trim_db::ViewTrimDb::open_existing_read_only_at(&db_path).unwrap();
        let context = RemotePageRenderContext {
            context_address: favorite_address(&favorite, "book"),
            display_slot: RemotePageDisplaySlot::SpreadRight,
            spread_partner: None,
        };

        let override_address = favorite_address(&favorite, "book/override.png");
        let override_resolved = engine.resolve(&override_address).unwrap();
        let override_bbox = match engine
            .remote_view_trim_plan(
                &override_address,
                &override_resolved,
                Some(&context),
                crate::rotation_db::Rotation::None,
            )
            .unwrap()
        {
            RemoteViewTrimPlan::Stored(Some(bbox)) => bbox,
            _ => panic!("expected stored page bbox"),
        };
        let override_margins = crate::view_trim::ViewTrimMargins::from_bbox(override_bbox);
        assert!((override_margins.left - 0.09).abs() < 1e-6);
        assert!((override_margins.right - 0.03).abs() < 1e-6);

        let book_address = favorite_address(&favorite, "book/book.png");
        let book_resolved = engine.resolve(&book_address).unwrap();
        let book_bbox = match engine
            .remote_view_trim_plan(
                &book_address,
                &book_resolved,
                Some(&context),
                crate::rotation_db::Rotation::None,
            )
            .unwrap()
        {
            RemoteViewTrimPlan::Stored(Some(bbox)) => bbox,
            _ => panic!("expected stored book bbox"),
        };
        let book_margins = crate::view_trim::ViewTrimMargins::from_bbox(book_bbox);
        assert!((book_margins.left - 0.08).abs() < 1e-6);
        assert!((book_margins.right - 0.02).abs() < 1e-6);
    }

    #[test]
    fn remote_auto_trim_plan_falls_back_without_and_validates_a_present_spread_partner() {
        let temp = tempfile::tempdir().unwrap();
        let book = temp.path().join("book");
        std::fs::create_dir(&book).unwrap();
        std::fs::write(book.join("left.png"), b"page").unwrap();
        std::fs::write(book.join("right.png"), b"page").unwrap();
        let db_path = temp.path().join("view_trim.db");
        let db = crate::view_trim_db::ViewTrimDb::open_at(&db_path).unwrap();
        db.set_book_state(
            &book,
            crate::view_trim::ViewTrimBookState {
                apply_mode: crate::view_trim::ViewTrimApplyMode::Auto,
                ..Default::default()
            },
        )
        .unwrap();
        drop(db);

        let favorite = FavoriteEntry::new("test".to_owned(), temp.path().to_path_buf());
        let engine = ContainerEngine::new(crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        });
        *engine
            .view_trim_db
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            crate::view_trim_db::ViewTrimDb::open_existing_read_only_at(&db_path).unwrap();
        let left = favorite_address(&favorite, "book/left.png");
        let right = favorite_address(&favorite, "book/right.png");
        let resolved = engine.resolve(&left).unwrap();
        let context_address = favorite_address(&favorite, "book");

        let missing_partner = RemotePageRenderContext {
            context_address: context_address.clone(),
            display_slot: RemotePageDisplaySlot::SpreadLeft,
            spread_partner: None,
        };
        assert!(matches!(
            engine
                .remote_view_trim_plan(
                    &left,
                    &resolved,
                    Some(&missing_partner),
                    crate::rotation_db::Rotation::None,
                )
                .unwrap(),
            RemoteViewTrimPlan::AutoSingle
        ));

        let spread = RemotePageRenderContext {
            context_address: context_address.clone(),
            display_slot: RemotePageDisplaySlot::SpreadLeft,
            spread_partner: Some(right.clone()),
        };
        assert!(matches!(
            engine
                .remote_view_trim_plan(
                    &left,
                    &resolved,
                    Some(&spread),
                    crate::rotation_db::Rotation::None,
                )
                .unwrap(),
            RemoteViewTrimPlan::AutoSpread {
                side: crate::view_trim::ViewTrimSpreadSide::Left,
                partner
            } if partner == right
        ));

        let single = RemotePageRenderContext {
            context_address,
            display_slot: RemotePageDisplaySlot::Single,
            spread_partner: None,
        };
        assert!(matches!(
            engine
                .remote_view_trim_plan(
                    &left,
                    &resolved,
                    Some(&single),
                    crate::rotation_db::Rotation::None,
                )
                .unwrap(),
            RemoteViewTrimPlan::AutoSingle
        ));
    }

    #[test]
    fn remote_auto_trim_page_responses_share_the_harmonized_spread_height() {
        // このテストは自前の tempdir に本と view_trim.db を作るが、load_image() の
        // 途中で開くサムネイルカタログだけは data_dir::get() 側にある。ガードを
        // 取らないと利用者の実カタログを開き、並列実行で稀に失敗していた。
        let _data_dir = crate::data_dir::TestDataDirGuard::new();
        let temp = tempfile::tempdir().unwrap();
        let book = temp.path().join("book");
        std::fs::create_dir(&book).unwrap();
        write_auto_trim_test_page(&book.join("left.png"), 40, 20);
        write_auto_trim_test_page(&book.join("right.png"), 20, 40);
        let db_path = temp.path().join("view_trim.db");
        let db = crate::view_trim_db::ViewTrimDb::open_at(&db_path).unwrap();
        db.set_book_state(
            &book,
            crate::view_trim::ViewTrimBookState {
                apply_mode: crate::view_trim::ViewTrimApplyMode::Auto,
                ..Default::default()
            },
        )
        .unwrap();
        drop(db);

        let favorite = FavoriteEntry::new("test".to_owned(), temp.path().to_path_buf());
        let engine = ContainerEngine::new(crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        });
        *engine
            .view_trim_db
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            crate::view_trim_db::ViewTrimDb::open_existing_read_only_at(&db_path).unwrap();
        let left = favorite_address(&favorite, "book/left.png");
        let right = favorite_address(&favorite, "book/right.png");
        let context_address = favorite_address(&favorite, "book");
        let worker = WorkerContext::open();
        let render = |address: RemoteAddress,
                      display_slot: RemotePageDisplaySlot,
                      partner: RemoteAddress| {
            let resolved = engine.resolve(&address).unwrap();
            let render_context = RemotePageRenderContext {
                context_address: context_address.clone(),
                display_slot,
                spread_partner: Some(partner),
            };
            let plan = engine
                .remote_view_trim_plan(
                    &address,
                    &resolved,
                    Some(&render_context),
                    crate::rotation_db::Rotation::None,
                )
                .unwrap();
            let cancel = Arc::new(AtomicBool::new(false));
            let loaded = engine
                .load_image(
                    &address,
                    &resolved,
                    1024,
                    RemoteImageLoadKind::AutoTrimReference,
                    crate::rotation_db::Rotation::None,
                    true,
                    &worker,
                    Some(&cancel),
                    None,
                )
                .unwrap();
            let bbox = engine
                .complete_remote_view_trim_bbox(
                    &plan,
                    loaded.auto_trim_bbox,
                    1024,
                    true,
                    &worker,
                    &cancel,
                )
                .unwrap();
            encode_remote_page_jpeg(&loaded.pixels, 1024, bbox).unwrap()
        };

        let left_payload = render(
            left.clone(),
            RemotePageDisplaySlot::SpreadLeft,
            right.clone(),
        );
        let right_payload = render(
            right.clone(),
            RemotePageDisplaySlot::SpreadRight,
            left.clone(),
        );
        assert_eq!(left_payload.2, right_payload.2);
        assert_eq!(left_payload.1, right_payload.1);
        assert!(left_payload.2 > 140);
        assert_eq!(
            engine
                .auto_trim_bbox_cache
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .entries
                .len(),
            2
        );

        let decode_count_before = engine
            .stats
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .count_png;
        let right_resolved = engine.resolve(&right).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        assert!(
            engine
                .remote_auto_trim_bbox(&right, &right_resolved, 1024, true, &worker, &cancel,)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            engine
                .stats
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .count_png,
            decode_count_before,
            "bbox cache hit must not decode the spread partner again"
        );
    }

    fn remote_ai_test_zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut output);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, bytes) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        output.into_inner()
    }

    fn write_remote_ai_test_zip(path: &Path, entries: &[(&str, &[u8])]) {
        std::fs::write(path, remote_ai_test_zip_bytes(entries)).unwrap();
    }

    fn remote_ai_test_request(address: RemoteAddress) -> RemoteAiStartRequest {
        RemoteAiStartRequest {
            request_id: "remote-ai-container-test".to_owned(),
            pages: vec![mimageviewer_ipc::RemoteAiPageRequest {
                address,
                target_px: 1024,
                render_context: None,
            }],
        }
    }

    fn remote_ai_cache_key(page_key: &str) -> RemoteAiNativeCacheKey {
        RemoteAiNativeCacheKey {
            page_key: page_key.to_owned(),
            mtime: 1,
            file_size: 2,
            source_size: [2, 2],
            pre_ai_params: crate::adjustment::AdjustParams::default(),
            pre_ai_edit_fingerprint: [0; 32],
            ai_feature_mode: crate::settings::AiFeatureMode::Light,
            ai_upscale_limit: crate::ai::upscale::AiProcessSizeLimit::square(4096),
            ai_denoise_limit: crate::ai::upscale::AiProcessSizeLimit::square(4096),
            ai_backend: Some("directml".to_owned()),
            background_mode: 0,
            pipeline_schema: REMOTE_AI_PIPELINE_SCHEMA,
            model_epoch: [0; 32],
        }
    }

    #[test]
    fn remote_ai_native_budget_is_derived_exactly_from_retained_settings() {
        let mut settings = crate::settings::Settings::default();
        settings.retained_final_ai_cache_max_entries = 7;
        settings.retained_final_ai_cache_max_mib = 23;
        let snapshot = crate::settings_db::AdjustmentRenderSettings::from_settings(&settings);
        assert_eq!(
            remote_ai_native_budget(&snapshot),
            Some((7, 23 * 1024 * 1024))
        );

        settings.retained_final_ai_cache_max_entries = 0;
        let snapshot = crate::settings_db::AdjustmentRenderSettings::from_settings(&settings);
        assert_eq!(remote_ai_native_budget(&snapshot), None);
    }

    #[test]
    fn remote_ai_native_cache_obeys_both_independent_lru_bounds() {
        let pixels = Arc::new(egui::ColorImage::new([2, 2], vec![egui::Color32::BLACK; 4]));
        let bytes = pixels.as_raw().len() as u64;
        let mut cache = RemoteAiNativeCache::default();
        cache.insert(
            remote_ai_cache_key("one"),
            Arc::clone(&pixels),
            false,
            1,
            bytes * 2,
        );
        cache.insert(
            remote_ai_cache_key("two"),
            Arc::clone(&pixels),
            true,
            1,
            bytes * 2,
        );
        assert!(cache.get(&remote_ai_cache_key("one")).is_none());
        assert_eq!(
            cache.get(&remote_ai_cache_key("two")).map(|hit| hit.1),
            Some(true)
        );

        cache.insert(
            remote_ai_cache_key("three"),
            Arc::clone(&pixels),
            false,
            3,
            bytes,
        );
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.entries[0].key.page_key, "three");

        cache.insert(remote_ai_cache_key("disabled"), pixels, false, 0, bytes);
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn remote_ai_native_cache_applies_lowered_live_budget_before_lookup() {
        let pixels = Arc::new(egui::ColorImage::new([2, 2], vec![egui::Color32::BLACK; 4]));
        let bytes = pixels.as_raw().len() as u64;
        let mut cache = RemoteAiNativeCache::default();
        for page_key in ["one", "two"] {
            cache.insert(
                remote_ai_cache_key(page_key),
                Arc::clone(&pixels),
                false,
                2,
                bytes * 2,
            );
        }

        cache.enforce_budget(1, bytes * 2);
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.entries[0].key.page_key, "two");

        cache.enforce_budget(0, bytes * 2);
        assert!(cache.entries.is_empty());
        assert_eq!(cache.bytes, 0);
    }

    #[test]
    fn remote_executor_rejects_vector_pdf_and_size_gate_before_runtime_acquisition() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("vector.pdf"),
            b"classification is supplied by the test seam",
        )
        .unwrap();
        std::fs::write(
            root.join("large.png"),
            b"decode is supplied by the test seam",
        )
        .unwrap();
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let mut settings = crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        };
        settings.ai_feature_mode = crate::settings::AiFeatureMode::Light;
        settings.global_preset.upscale_model = Some("realesr_general_v3".to_owned());
        settings.ai_upscale_size_limit = Some(crate::ai::upscale::AiProcessSizeLimit::square(4));
        let render_settings =
            crate::settings_db::AdjustmentRenderSettings::from_settings(&settings);
        let engine = ContainerEngine::new(settings);
        let cancel = Arc::new(AtomicBool::new(false));
        let runtime_acquisitions = AtomicUsize::new(0);
        let resources = |_engine: &ContainerEngine| {
            runtime_acquisitions.fetch_add(1, Ordering::Relaxed);
            None
        };
        let prepare = |_engine: &ContainerEngine,
                       address: &RemoteAddress,
                       logical_path: &Path,
                       mtime: i64,
                       file_size: i64,
                       target_px: u32,
                       rotation: crate::rotation_db::Rotation,
                       _context: &WorkerContext| {
            let params = render_settings.global_preset.clone();
            let page_key =
                crate::edit_source::page_key_for_remote(logical_path, &address.subresource)
                    .expect("test address identifies a page");
            Ok(Some(RemotePreparedComposite {
                key: RemoteCompositeCacheKey {
                    page_key,
                    mtime,
                    file_size,
                    target_px,
                    rotation,
                    params: params.clone(),
                    lut_entry: None,
                    edit_fingerprint: [0; 32],
                },
                params,
                lut_entry: None,
                edits: RemoteEditSnapshot {
                    erase: None,
                    erase_mono_tolerance: render_settings.erase_inpaint_mono_tolerance,
                    local_adjust: None,
                    conceal: None,
                    conceal_preset: render_settings.conceal_preset.clone(),
                    comic: Vec::new(),
                    export_crop: None,
                    fingerprint: [0; 32],
                    pre_ai_fingerprint: [0; 32],
                },
                settings: render_settings.clone(),
            }))
        };
        let decode = |_engine: &ContainerEngine,
                      address: &RemoteAddress,
                      _resolved: &ResolvedPath,
                      _metadata: &std::fs::Metadata,
                      page_index: usize,
                      _cancel: &Arc<AtomicBool>| {
            if matches!(address.subresource, RemoteSubresource::PdfPage { .. }) {
                Err(RemoteAiRunError::NotApplicable {
                    code: RemoteAiTerminalCode::VectorPdf,
                    message: "vector fixture".to_owned(),
                    page_index,
                })
            } else {
                Ok((
                    Arc::new(egui::ColorImage::new(
                        [4, 3],
                        vec![egui::Color32::BLACK; 12],
                    )),
                    [4, 3],
                ))
            }
        };

        let mut mixed = remote_ai_test_request(RemoteAddress {
            path: favorite
                .path
                .join("vector.pdf")
                .to_string_lossy()
                .into_owned(),
            subresource: RemoteSubresource::PdfPage { page_number: 0 },
        });
        mixed.pages.push(mimageviewer_ipc::RemoteAiPageRequest {
            address: favorite_address(&favorite, "large.png"),
            target_px: 1024,
            render_context: None,
        });
        let outcomes = engine
            .execute_remote_ai_inner_with(
                &mixed,
                &NoRemoteAiProgress,
                &cancel,
                &prepare,
                &decode,
                &resources,
            )
            .unwrap();
        assert!(matches!(
            outcomes.as_slice(),
            [
                super::super::ai_job::RemoteAiPageExecutionOutcome::NotApplicable {
                    code: RemoteAiTerminalCode::VectorPdf,
                    ..
                },
                super::super::ai_job::RemoteAiPageExecutionOutcome::NotApplicable {
                    code: RemoteAiTerminalCode::SizeGate,
                    ..
                }
            ]
        ));
        assert_eq!(runtime_acquisitions.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn decode_remote_ai_source_routes_nested_zip_through_the_canonical_decoder() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        std::fs::create_dir_all(&root).unwrap();
        let page = remote_ai_test_png(4, 3);
        let inner = remote_ai_test_zip_bytes(&[("page.png", &page)]);
        let outer = root.join("book.cbz");
        write_remote_ai_test_zip(&outer, &[("chapter.zip", &inner)]);
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let engine = ContainerEngine::new(crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        });
        let address = RemoteAddress {
            path: outer.to_string_lossy().into_owned(),
            subresource: RemoteSubresource::ZipEntry {
                entry_name: "chapter.zip/page.png".to_owned(),
            },
        };
        let resolved = engine.resolve(&address).unwrap();
        let metadata = std::fs::metadata(&resolved.canonical).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));

        let Ok((actual, actual_dims)) =
            engine.decode_remote_ai_source(&address, &resolved, &metadata, 0, &cancel)
        else {
            panic!("nested ZIP remote source must decode");
        };
        let Ok((expected, expected_dims)) = decode_remote_ai_canonical(
            crate::canonical_image_loader::CanonicalImageSource::File {
                path: Path::new("page.png"),
                verified_bytes: Some(&page),
            },
            0,
            &cancel,
        ) else {
            panic!("verified page bytes must decode canonically");
        };

        assert_eq!(actual_dims, [4, 3]);
        assert_eq!(actual_dims, expected_dims);
        assert_eq!(actual.size, expected.size);
        assert_eq!(actual.pixels, expected.pixels);
    }

    #[test]
    fn remote_default_adjustment_preserves_pixels() {
        let source = Arc::new(egui::ColorImage::new(
            [3, 2],
            vec![
                egui::Color32::from_rgba_unmultiplied(1, 2, 3, 255),
                egui::Color32::from_rgba_unmultiplied(40, 50, 60, 200),
                egui::Color32::from_rgba_unmultiplied(70, 80, 90, 128),
                egui::Color32::from_rgba_unmultiplied(100, 110, 120, 255),
                egui::Color32::from_rgba_unmultiplied(130, 140, 150, 64),
                egui::Color32::from_rgba_unmultiplied(200, 210, 220, 255),
            ],
        ));
        let result = execute_remote_composite(
            Arc::clone(&source),
            &crate::adjustment::AdjustParams::default(),
            None,
            &AtomicBool::new(false),
        )
        .unwrap();

        assert!(Arc::ptr_eq(&result, &source));
        assert_eq!(result.pixels, source.pixels);
    }

    #[test]
    fn remote_composite_cache_key_includes_effective_params() {
        let mut cache = RemoteCompositeCache::default();
        let pixels = Arc::new(egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]));
        let mut key = RemoteCompositeCacheKey {
            page_key: "page".to_owned(),
            mtime: 1,
            file_size: 2,
            target_px: 1024,
            rotation: crate::rotation_db::Rotation::None,
            params: crate::adjustment::AdjustParams::default(),
            lut_entry: None,
            edit_fingerprint: [0; 32],
        };
        cache.insert(key.clone(), Arc::clone(&pixels));
        let base_key = key.clone();
        key.params.brightness = 10.0;
        assert!(cache.get(&key).is_none());
        let mut edited_key = base_key;
        edited_key.edit_fingerprint[0] = 1;
        assert!(cache.get(&edited_key).is_none());
    }

    #[test]
    fn remote_edit_fingerprint_includes_erase_tone_tolerance_only_for_erase() {
        let erase = crate::edit_source::MaskSnapshot {
            bitmap: vec![true],
            shapes: Vec::new(),
            size: [1, 1],
        };
        let preset = crate::conceal::ConcealPreset::default();
        let with_low =
            remote_edit_fingerprint(Some(&erase), 1, None, None, &preset, &[], None).unwrap();
        let with_high =
            remote_edit_fingerprint(Some(&erase), 64, None, None, &preset, &[], None).unwrap();
        assert_ne!(with_low, with_high);

        let without_low = remote_edit_fingerprint(None, 1, None, None, &preset, &[], None).unwrap();
        let without_high =
            remote_edit_fingerprint(None, 64, None, None, &preset, &[], None).unwrap();
        assert_eq!(without_low, without_high);
    }

    #[test]
    fn compiled_book_remote_adjustment_uses_identity_until_a_page_override_exists() {
        let identity = RemoteAdjustmentIdentity {
            page_key: "compiled-page".to_owned(),
            location_path: PathBuf::from("C:/books/compiled"),
            compiled_book: true,
        };
        let mut global = crate::adjustment::AdjustParams::default();
        global.brightness = 45.0;
        let resolved =
            resolve_remote_effective_params(&identity, None, &[], &HashMap::new(), &global);
        assert_eq!(resolved, crate::adjustment::AdjustParams::default());

        let mut page = crate::adjustment::AdjustParams::default();
        page.brightness = 18.0;
        let resolved =
            resolve_remote_effective_params(&identity, Some(&page), &[], &HashMap::new(), &global);
        assert_eq!(resolved, page);
    }

    #[test]
    fn remote_edit_adapter_materializes_conceal_before_final_composite() {
        let engine = ContainerEngine::new(crate::settings::Settings::default());
        let source = Arc::new(egui::ColorImage::new(
            [2, 1],
            vec![egui::Color32::RED, egui::Color32::GREEN],
        ));
        let mut preset = crate::conceal::ConcealPreset::default();
        preset.conceal_type = crate::conceal::ConcealType::BlackFill;
        preset.fill_opacity_percent = 100;
        let result = engine
            .execute_remote_edits(
                source,
                RemoteEditSnapshot {
                    erase: None,
                    erase_mono_tolerance: crate::settings::default_erase_inpaint_mono_tolerance(),
                    local_adjust: None,
                    conceal: Some(crate::edit_source::MaskSnapshot {
                        bitmap: vec![true, false],
                        shapes: Vec::new(),
                        size: [2, 1],
                    }),
                    conceal_preset: preset,
                    comic: Vec::new(),
                    export_crop: None,
                    fingerprint: [0; 32],
                    pre_ai_fingerprint: [0; 32],
                },
                &Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        assert_eq!(result.pixels.pixels[0], egui::Color32::BLACK);
        assert_eq!(result.pixels.pixels[1], egui::Color32::GREEN);
    }

    #[test]
    fn remote_virtual_page_identity_uses_the_app_adjustment_keys_and_container_location() {
        let container = PathBuf::from("C:/books/nested/book.zip");
        let zip_address = RemoteAddress {
            path: container.to_string_lossy().into_owned(),
            subresource: RemoteSubresource::ZipEntry {
                entry_name: "Chapter/001.JPG".to_owned(),
            },
        };
        let zip = remote_adjustment_identity(&zip_address, &container).unwrap();
        assert_eq!(
            zip.page_key,
            crate::adjustment_db::zip_entry_key(&container, "Chapter/001.JPG")
        );
        assert_eq!(zip.location_path, container);

        let pdf_path = PathBuf::from("C:/books/nested/book.pdf");
        let pdf_address = RemoteAddress {
            path: pdf_path.to_string_lossy().into_owned(),
            subresource: RemoteSubresource::PdfPage { page_number: 7 },
        };
        let pdf = remote_adjustment_identity(&pdf_address, &pdf_path).unwrap();
        assert_eq!(
            pdf.page_key,
            crate::adjustment_db::zip_entry_key(&pdf_path, "page_7")
        );
        assert_eq!(pdf.location_path, pdf_path);
    }
    #[test]
    fn resume_page_resolution_rejects_positions_outside_the_current_pages() {
        let container = RemoteAddress::file("C:/Books/book.pdf");
        let items = (0..3)
            .map(|page_num| crate::grid_item::GridItem::PdfPage {
                pdf_path: std::path::PathBuf::from("book.pdf"),
                page_num,
                content_type: None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            resolve_resume_page(&container, &items, 1),
            Some(RemoteAddress {
                path: "C:/Books/book.pdf".to_owned(),
                subresource: RemoteSubresource::PdfPage { page_number: 1 },
            })
        );
        assert_eq!(resolve_resume_page(&container, &items, items.len()), None);
        assert_eq!(
            resolve_resume_page(&container, &items, items.len() + 20),
            None
        );
    }

    #[test]
    fn container_open_mode_matches_the_local_auto_open_and_resume_settings() {
        let mut settings = crate::settings::Settings::default();
        settings.book_open_resume = crate::settings::ResumeMode::Resume;
        let engine = ContainerEngine::new(settings.clone());
        assert_eq!(engine.container_open_mode(false), ContainerOpenMode::Grid);
        assert_eq!(
            engine.container_open_mode(true),
            ContainerOpenMode::ResumePage
        );

        settings.book_open_resume = crate::settings::ResumeMode::FromStart;
        let engine = ContainerEngine::new(settings);
        assert_eq!(
            engine.container_open_mode(true),
            ContainerOpenMode::FirstPage
        );
    }

    #[test]
    fn resume_read_failures_fall_back_without_failing_container_enumeration() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let album = root.join("album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("001.jpg"), b"one").unwrap();
        std::fs::write(album.join("002.jpg"), b"two").unwrap();
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let address = favorite_address(&favorite, "album");

        for error in [
            super::super::session::UiReadError::Busy,
            super::super::session::UiReadError::Timeout,
            super::super::session::UiReadError::Stopped,
        ] {
            let mut settings = crate::settings::Settings {
                favorites: vec![favorite.clone()],
                ..Default::default()
            };
            settings.auto_fullscreen_zip_pdf = true;
            settings.auto_fullscreen_image_folders = true;
            settings.book_open_resume = crate::settings::ResumeMode::Resume;
            let engine = ContainerEngine::new_with_resume_error(settings, error);
            let response = engine.container(ContainerRequest {
                address: address.clone(),
                spread_mode: None,
                reading_direction: None,
                force_single_page: false,
            });

            let ContainerResponse::Success(payload) = response else {
                panic!("resume read failure must not fail container enumeration: {error:?}");
            };
            assert_eq!(payload.entries.len(), 2);
            assert_eq!(payload.resume_page, None);
            assert_eq!(payload.open_mode, ContainerOpenMode::ResumePage);
        }
    }

    #[test]
    fn pdf_page_range_rejects_the_upper_bound() {
        assert!(validate_page_number(0, 1).is_ok());
        assert!(matches!(
            validate_page_number(1, 1),
            Err(MediaError {
                code: MediaErrorCode::PageOutOfRange,
                ..
            })
        ));
        assert!(validate_page_number(0, 0).is_err());
    }

    #[test]
    fn password_protected_pdf_is_reported_distinctly() {
        let error = pdf_error(std::io::Error::other(
            "worker error: MIV_PDF_PASSWORD_REQUIRED",
        ));
        assert_eq!(error.code, MediaErrorCode::PasswordRequired);
        assert!(error.message.contains("パスワード保護"));
    }

    #[test]
    fn container_resolution_accepts_absolute_paths_outside_favorites_but_rejects_zip_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let outside = temp.path().join("outside.zip");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&outside, b"not a zip").unwrap();
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let settings = crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        };
        let engine = ContainerEngine::new(settings);
        let address = RemoteAddress::file(outside.to_string_lossy().into_owned());
        assert_eq!(
            engine.resolve(&address).unwrap().canonical,
            std::fs::canonicalize(&outside).unwrap()
        );

        let safe_root = favorite.path.clone();
        std::fs::write(safe_root.join("book.zip"), b"not a zip").unwrap();
        let unsafe_entry = RemoteAddress {
            path: safe_root.join("book.zip").to_string_lossy().into_owned(),
            subresource: RemoteSubresource::ZipEntry {
                entry_name: "../secret.jpg".to_owned(),
            },
        };
        assert!(matches!(
            engine.resolve(&unsafe_entry),
            Err(MediaError {
                code: MediaErrorCode::BadRequest,
                ..
            })
        ));
    }

    #[test]
    fn zip_materialization_uses_book_filename_order() {
        let tree = crate::zip_tree::ZipTree::build(
            "book.zip".into(),
            vec![
                crate::zip_loader::ZipImageEntry {
                    entry_name: "10.jpg".to_owned(),
                    uncompressed_size: 1,
                    mtime: 0,
                },
                crate::zip_loader::ZipImageEntry {
                    entry_name: "2.jpg".to_owned(),
                    uncompressed_size: 1,
                    mtime: 0,
                },
            ],
        );
        let (items, _) = tree.materialize_level(&[], crate::app::BOOK_READING_PAGE_ORDER);
        let names = items
            .iter()
            .map(|item| item.name().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, ["2.jpg", "10.jpg"]);
    }

    #[test]
    fn folder_progress_validation_recomputes_local_index_count_and_bookmark_support() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let album = root.join("album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("001.jpg"), b"one").unwrap();
        std::fs::write(album.join("002.jpg"), b"two").unwrap();
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let mut settings = crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        };
        settings.auto_fullscreen_zip_pdf = true;
        settings.auto_fullscreen_image_folders = true;
        let engine = ContainerEngine::new(settings);
        let context = favorite_address(&favorite, "album");
        let page = favorite_address(&favorite, "album/002.jpg");
        let mut request = RemoteWriteRequest::RecordReadingProgress {
            address: page.clone(),
            context_address: context.clone(),
            page_index: 999,
            page_number: 999,
            page_count: 999,
            record_resume: false,
            record_history: false,
        };
        engine.validate_write_request(&mut request).unwrap();
        assert!(matches!(
            request,
            RemoteWriteRequest::RecordReadingProgress {
                page_index: 1,
                page_number: 2,
                page_count: 2,
                record_resume: true,
                record_history: true,
                ..
            }
        ));

        let mut query = RemoteWriteRequest::GetItemState {
            address: page.clone(),
            context_address: context.clone(),
            page_index: 999,
            bookmark_supported: false,
        };
        engine.validate_write_request(&mut query).unwrap();
        assert!(matches!(
            query,
            RemoteWriteRequest::GetItemState {
                page_index: 1,
                bookmark_supported: true,
                ..
            }
        ));

        let mut list = RemoteWriteRequest::ListBookBookmarks {
            address: page.clone(),
            context_address: context.clone(),
            page_index: 999,
            bookmark_supported: false,
        };
        engine.validate_write_request(&mut list).unwrap();
        assert!(matches!(
            list,
            RemoteWriteRequest::ListBookBookmarks {
                page_index: 1,
                bookmark_supported: true,
                ..
            }
        ));

        for mut mutation in [
            RemoteWriteRequest::SetBookBookmarkTitle {
                address: page.clone(),
                context_address: context.clone(),
                page_index: 999,
                id: 7,
                title: "page".to_owned(),
            },
            RemoteWriteRequest::RemoveBookBookmark {
                address: page.clone(),
                context_address: context.clone(),
                page_index: 999,
                id: 7,
            },
        ] {
            engine.validate_write_request(&mut mutation).unwrap();
            assert_eq!(mutation.context_address(), Some(&context));
            assert!(matches!(
                mutation,
                RemoteWriteRequest::SetBookBookmarkTitle { page_index: 1, .. }
                    | RemoteWriteRequest::RemoveBookBookmark { page_index: 1, .. }
            ));
        }
    }

    #[test]
    fn folder_bookmark_list_keeps_db_order_hint_and_resolved_target_separate() {
        let _data_dir = crate::data_dir::TestDataDirGuard::new();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let album = root.join("album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("001.jpg"), b"one").unwrap();
        std::fs::write(album.join("002.jpg"), b"two").unwrap();
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let mut settings = crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        };
        settings.auto_fullscreen_zip_pdf = true;
        settings.auto_fullscreen_image_folders = true;
        let engine = ContainerEngine::new(settings);

        let service = crate::book_bookmarks::BookBookmarkService::spawn().unwrap();
        service.add(
            1,
            crate::book_bookmarks::NewBookBookmark {
                container_path: album,
                container_kind: crate::book_bookmarks::BookContainerKind::ImageFolder,
                page_identity: crate::book_bookmarks::PageIdentity::RelativePath(
                    "002.jpg".to_owned(),
                ),
                page_index_hint: 99,
            },
        );
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match service.try_recv() {
                Ok(crate::book_bookmarks::BookBookmarkEvent::Added { result: Ok(_), .. }) => break,
                Ok(event) => panic!("unexpected bookmark event: {event:?}"),
                Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                Err(error) => panic!("bookmark service did not add the row: {error}"),
            }
        }

        let context = favorite_address(&favorite, "album");
        let mut request = RemoteWriteRequest::ListBookBookmarks {
            address: favorite_address(&favorite, "album/001.jpg"),
            context_address: context.clone(),
            page_index: 999,
            bookmark_supported: false,
        };
        let RemoteWriteResponse::Success(result) = engine.book_bookmarks(&mut request) else {
            panic!("bookmark list failed");
        };
        let list = result.book_bookmarks.unwrap();
        assert!(list.supported);
        assert_eq!(list.rows.len(), 1);
        let row = &list.rows[0];
        assert_eq!(row.page_index_hint, 99);
        assert_eq!(row.page_label, "002.jpg");
        let target = row.target.as_ref().unwrap();
        assert_eq!(target.item_index, 1);
        assert_eq!(target.context_address, context);
        assert_eq!(target.address, favorite_address(&favorite, "album/002.jpg"));
    }

    #[test]
    fn zip_bookmark_list_combines_validation_with_cross_prefix_resolution() {
        let _data_dir = crate::data_dir::TestDataDirGuard::new();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        std::fs::create_dir_all(&root).unwrap();
        let zip_path = root.join("book.zip");
        write_remote_ai_test_zip(
            &zip_path,
            &[("part-a/001.jpg", b"one"), ("part-b/002.jpg", b"two")],
        );
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let engine = ContainerEngine::new(crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        });

        let service = crate::book_bookmarks::BookBookmarkService::spawn().unwrap();
        service.add(
            1,
            crate::book_bookmarks::NewBookBookmark {
                container_path: zip_path,
                container_kind: crate::book_bookmarks::BookContainerKind::OtherArchive,
                page_identity: crate::book_bookmarks::PageIdentity::ArchiveEntry(
                    "part-b/002.jpg".to_owned(),
                ),
                page_index_hint: 99,
            },
        );
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match service.try_recv() {
                Ok(crate::book_bookmarks::BookBookmarkEvent::Added { result: Ok(_), .. }) => break,
                Ok(event) => panic!("unexpected bookmark event: {event:?}"),
                Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                Err(error) => panic!("bookmark service did not add the row: {error}"),
            }
        }

        let mut request = RemoteWriteRequest::ListBookBookmarks {
            address: RemoteAddress {
                path: favorite
                    .path
                    .join("book.zip")
                    .to_string_lossy()
                    .into_owned(),
                subresource: RemoteSubresource::ZipEntry {
                    entry_name: "part-a/001.jpg".to_owned(),
                },
            },
            context_address: RemoteAddress {
                path: favorite
                    .path
                    .join("book.zip")
                    .to_string_lossy()
                    .into_owned(),
                subresource: RemoteSubresource::ZipDirectory {
                    prefix: "part-a/".to_owned(),
                },
            },
            page_index: 999,
            bookmark_supported: false,
        };
        let RemoteWriteResponse::Success(result) = engine.book_bookmarks(&mut request) else {
            panic!("ZIP bookmark list failed");
        };
        let list = result.book_bookmarks.unwrap();
        assert!(list.supported);
        assert_eq!(list.rows.len(), 1);
        let target = list.rows[0].target.as_ref().unwrap();
        assert_eq!(target.item_index, 0);
        assert_eq!(
            target.address.subresource,
            RemoteSubresource::ZipEntry {
                entry_name: "part-b/002.jpg".to_owned(),
            }
        );
        assert_eq!(
            target.context_address.subresource,
            RemoteSubresource::ZipDirectory {
                prefix: "part-b/".to_owned(),
            }
        );
    }

    #[test]
    fn mixed_folder_publishes_resume_index_but_not_history_or_bookmark_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let album = root.join("album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("001.jpg"), b"one").unwrap();
        std::fs::write(album.join("clip.mp4"), b"video").unwrap();
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let engine = ContainerEngine::new(crate::settings::Settings {
            favorites: vec![favorite.clone()],
            ..Default::default()
        });
        let mut request = RemoteWriteRequest::RecordReadingProgress {
            address: favorite_address(&favorite, "album/001.jpg"),
            context_address: favorite_address(&favorite, "album"),
            page_index: 0,
            page_number: 1,
            page_count: 1,
            record_resume: false,
            record_history: true,
        };
        engine.validate_write_request(&mut request).unwrap();
        assert!(matches!(
            request,
            RemoteWriteRequest::RecordReadingProgress {
                page_index: 0,
                page_number: 1,
                page_count: 1,
                record_resume: true,
                record_history: false,
                ..
            }
        ));

        let mut list = RemoteWriteRequest::ListBookBookmarks {
            address: favorite_address(&favorite, "album/001.jpg"),
            context_address: favorite_address(&favorite, "album"),
            page_index: 999,
            bookmark_supported: true,
        };
        let RemoteWriteResponse::Success(result) = engine.book_bookmarks(&mut list) else {
            panic!("unsupported bookmark list should be a successful capability response");
        };
        assert_eq!(
            result.book_bookmarks,
            Some(RemoteBookBookmarkList {
                supported: false,
                rows: Vec::new(),
            })
        );
    }

    fn assert_local_remote_folder_listing_match(
        engine: &ContainerEngine,
        favorite: &FavoriteEntry,
        relative_folder: &str,
    ) -> FolderListPayload {
        let folder = favorite.path.join(relative_folder);
        let scan = crate::app::folder_scan::scan_directory_with_settings(&folder, &engine.settings)
            .unwrap();
        let local = crate::app::materialize_local_folder_listing(&folder, scan, &engine.settings);
        let response = engine.folder_list(FolderListRequest {
            address: favorite_address(favorite, relative_folder),
        });
        let FolderListResponse::Success(remote) = response else {
            panic!("remote folder listing failed for {relative_folder}");
        };

        assert_eq!(
            remote.entries.len(),
            local.items.len(),
            "local and remote folder entry counts drifted for {relative_folder}"
        );
        for ((entry, item), meta) in remote.entries.iter().zip(&local.items).zip(&local.metas) {
            let (expected_kind, path) = match item {
                crate::grid_item::GridItem::Folder(path) => (RemoteEntryKind::Folder, path),
                crate::grid_item::GridItem::Image(path) => (RemoteEntryKind::Image, path),
                crate::grid_item::GridItem::Video(path) => (RemoteEntryKind::Video, path),
                crate::grid_item::GridItem::Audio(path) => (RemoteEntryKind::Audio, path),
                crate::grid_item::GridItem::ZipFile(path) => (RemoteEntryKind::Zip, path),
                crate::grid_item::GridItem::PdfFile(path) => (RemoteEntryKind::Pdf, path),
                crate::grid_item::GridItem::ConvertibleArchive { path, .. } => {
                    (RemoteEntryKind::Archive, path)
                }
                _ => panic!("physical folder listing produced a virtual item"),
            };
            let name = path.file_name().unwrap().to_string_lossy();
            let expected_address = favorite_address(favorite, format!("{relative_folder}/{name}"));
            let expected_thumbnail = if expected_kind == RemoteEntryKind::Video {
                local
                    .video_thumb_overrides
                    .iter()
                    .rev()
                    .find(|(video, _)| crate::path_key::eq_keep_drive(video, path))
                    .map(|(_, image)| {
                        favorite_address(
                            favorite,
                            format!(
                                "{relative_folder}/{}",
                                image.file_name().unwrap().to_string_lossy()
                            ),
                        )
                    })
                    .unwrap_or_else(|| expected_address.clone())
            } else {
                expected_address.clone()
            };
            let (expected_mtime, expected_size) = meta.unwrap_or((0, 0));

            assert_eq!(entry.kind, expected_kind, "kind drifted for {name}");
            assert_eq!(entry.name, name, "name drifted for {name}");
            assert_eq!(
                entry.address, expected_address,
                "address drifted for {name}"
            );
            assert_eq!(
                entry.thumbnail_address, expected_thumbnail,
                "thumbnail source drifted for {name}"
            );
            assert_eq!(entry.mtime, expected_mtime, "mtime drifted for {name}");
            assert_eq!(
                entry.size,
                u64::try_from(expected_size).unwrap_or(0),
                "size drifted for {name}"
            );
        }

        let page_count = local
            .items
            .iter()
            .filter(|item| item.has_page_data())
            .count();
        let context = favorite_address(favorite, relative_folder);
        for (expected_index, item) in local.items.iter().enumerate() {
            let crate::grid_item::GridItem::Image(path) = item else {
                continue;
            };
            let name = path.file_name().unwrap().to_string_lossy();
            let page = favorite_address(favorite, format!("{relative_folder}/{name}"));
            let validated = engine.validate_folder_page(&page, &context).unwrap();
            assert_eq!(validated.page_index as usize, expected_index);
            assert_eq!(validated.page_count as usize, page_count);
            assert_eq!(
                validated.page_number as usize,
                local.items[..=expected_index]
                    .iter()
                    .filter(|candidate| candidate.has_page_data())
                    .count()
            );
        }

        remote
    }
    #[test]
    fn folder_recomputation_matches_local_listing_for_required_materials() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let image_only = root.join("image-only");
        let mixed = root.join("mixed");
        let duplicate_ext = root.join("duplicate-ext");
        let virtual_duplicate = root.join("virtual-duplicate");
        for folder in [&image_only, &mixed, &duplicate_ext, &virtual_duplicate] {
            std::fs::create_dir_all(folder).unwrap();
        }

        std::fs::write(image_only.join("10.jpg"), b"ten").unwrap();
        std::fs::write(image_only.join("2.jpg"), b"two").unwrap();

        std::fs::write(mixed.join("page.jpg"), b"page").unwrap();
        std::fs::write(mixed.join("clip.mp4"), b"video").unwrap();
        std::fs::write(mixed.join("clip.jpg"), b"sidecar").unwrap();

        std::fs::write(duplicate_ext.join("same.jpg"), b"jpeg").unwrap();
        std::fs::write(duplicate_ext.join("same.png"), b"png").unwrap();
        std::fs::write(duplicate_ext.join("other.jpg"), b"other").unwrap();

        std::fs::create_dir_all(virtual_duplicate.join("volume")).unwrap();
        std::fs::write(virtual_duplicate.join("volume.zip"), b"zip").unwrap();
        std::fs::write(virtual_duplicate.join("page.jpg"), b"page").unwrap();

        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let settings = crate::settings::Settings {
            favorites: vec![favorite.clone()],
            skip_duplicate_images: true,
            skip_zip_if_folder_exists: true,
            skip_image_if_video_exists: true,
            video_thumb_use_sidecar_image: true,
            auto_fullscreen_zip_pdf: true,
            auto_fullscreen_image_folders: true,
            thumb_aspect_auto: false,
            thumb_aspect: crate::settings::ThumbAspect::Landscape3x2,
            ..Default::default()
        };
        let engine = ContainerEngine::new(settings);

        for relative in ["image-only", "mixed", "duplicate-ext", "virtual-duplicate"] {
            let remote = assert_local_remote_folder_listing_match(&engine, &favorite, relative);
            if relative == "mixed" {
                assert!(
                    remote.entries.iter().all(|entry| entry.name != "clip.jpg"),
                    "the absorbed sidecar must not remain as an independent remote tile"
                );
                assert!((remote.thumb_aspect_height_ratio - (2.0 / 3.0)).abs() < 1e-6);
                let video = remote
                    .entries
                    .iter()
                    .find(|entry| entry.name == "clip.mp4")
                    .expect("video entry");
                assert_eq!(video.kind, RemoteEntryKind::Video);
                assert!(video.address.path.ends_with("mixed\\clip.mp4"));
                assert!(video.thumbnail_address.path.ends_with("mixed\\clip.jpg"));
                assert!(remote.sort_state.locked_reason.is_none());
            } else if relative == "image-only" {
                assert_eq!(
                    remote.sort_state.selected,
                    super::super::sort_order_wire_value(crate::app::BOOK_READING_PAGE_ORDER)
                );
                assert_eq!(
                    remote.sort_state.locked_reason.as_deref(),
                    Some(super::super::BOOK_SORT_LOCK_REASON)
                );
            }
        }
    }

    #[test]
    fn folder_spread_groups_share_cover_landscape_rtl_and_portrait_rules() {
        let items = (0..5)
            .map(|index| {
                crate::grid_item::GridItem::Image(std::path::PathBuf::from(format!(
                    "page-{index}.jpg"
                )))
            })
            .collect::<Vec<_>>();
        let portrait = vec![false; items.len()];

        assert_eq!(
            crate::ui_fullscreen::build_remote_spread_page_groups(
                &items,
                crate::settings::SpreadMode::LtrCover,
                &portrait,
            )
            .into_iter()
            .map(|group| group.indices)
            .collect::<Vec<_>>(),
            vec![vec![0], vec![1, 2], vec![3, 4]]
        );

        let mut with_landscape = portrait.clone();
        with_landscape[2] = true;
        assert_eq!(
            crate::ui_fullscreen::build_remote_spread_page_groups(
                &items,
                crate::settings::SpreadMode::Ltr,
                &with_landscape,
            )
            .into_iter()
            .map(|group| group.indices)
            .collect::<Vec<_>>(),
            vec![vec![0, 1], vec![2], vec![3, 4]]
        );
        assert_eq!(
            crate::ui_fullscreen::build_remote_spread_page_groups(
                &items,
                crate::settings::SpreadMode::Rtl,
                &portrait,
            )
            .into_iter()
            .map(|group| group.indices)
            .collect::<Vec<_>>(),
            vec![vec![1, 0], vec![3, 2], vec![4]]
        );

        let (_, effective, _) = resolve_spread_state(
            Some(RemoteSpreadMode::RtlCover),
            Some(RemoteReadingDirection::Rtl),
            None,
            None,
            crate::settings::SpreadMode::Single,
            crate::settings::ReadingDirection::Ltr,
            true,
        );
        assert_eq!(
            crate::ui_fullscreen::build_remote_spread_page_groups(
                &items,
                core_spread_mode(effective),
                &portrait,
            )
            .into_iter()
            .map(|group| group.indices)
            .collect::<Vec<_>>(),
            vec![vec![0], vec![1], vec![2], vec![3], vec![4]]
        );
    }

    #[test]
    fn folder_container_uses_page_groups_and_accepts_spread_writes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let album = root.join("album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("001.jpg"), b"one").unwrap();
        std::fs::write(album.join("002.jpg"), b"two").unwrap();
        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let engine = ContainerEngine::new(crate::settings::Settings {
            favorites: vec![favorite.clone()],
            sort_order: crate::settings::SortOrder::DateDesc,
            ..Default::default()
        });
        let address = favorite_address(&favorite, "album");
        let response = engine.container(ContainerRequest {
            address: address.clone(),
            spread_mode: Some(RemoteSpreadMode::Ltr),
            reading_direction: Some(RemoteReadingDirection::Ltr),
            force_single_page: false,
        });
        let ContainerResponse::Success(payload) = response else {
            panic!("folder container enumeration failed");
        };
        assert_eq!(payload.kind, ContainerKind::Folder);
        assert_eq!(payload.entries.len(), 2);
        assert_eq!(payload.image_count, 2);
        assert_eq!(payload.video_count, 0);
        assert_eq!(payload.other_count, 0);
        assert_eq!(payload.page_groups.len(), 1);
        assert_eq!(payload.page_groups[0].pages.len(), 2);
        assert_eq!(
            payload.sort_state.selected,
            super::super::sort_order_wire_value(crate::app::BOOK_READING_PAGE_ORDER)
        );
        assert_eq!(
            payload.sort_state.locked_reason.as_deref(),
            Some(super::super::BOOK_SORT_LOCK_REASON)
        );

        let mut write = RemoteWriteRequest::SetSpread {
            address,
            spread_mode: RemoteSpreadMode::RtlCover,
            reading_direction: RemoteReadingDirection::Rtl,
        };
        engine.validate_write_request(&mut write).unwrap();
    }

    #[test]
    fn folder_spread_defaults_follow_the_core_book_predicate_and_keep_stored_values() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        let image_only = root.join("image-only");
        let mixed_media = root.join("mixed-media");
        let mixed_containers = root.join("mixed-containers");
        for folder in [&image_only, &mixed_media, &mixed_containers] {
            std::fs::create_dir_all(folder).unwrap();
            std::fs::write(folder.join("001.jpg"), b"one").unwrap();
            std::fs::write(folder.join("002.jpg"), b"two").unwrap();
        }
        std::fs::write(mixed_media.join("clip.mp4"), b"video").unwrap();
        std::fs::create_dir_all(mixed_containers.join("child")).unwrap();
        write_remote_ai_test_zip(
            &mixed_containers.join("appendix.zip"),
            &[("001.jpg", b"zip-page")],
        );

        let favorite = FavoriteEntry::new("test".to_owned(), root);
        let engine = ContainerEngine::new(crate::settings::Settings {
            favorites: vec![favorite.clone()],
            auto_fullscreen_zip_pdf: true,
            auto_fullscreen_image_folders: true,
            default_spread_mode: crate::settings::SpreadMode::RtlCover,
            default_reading_direction: crate::settings::ReadingDirection::Rtl,
            ..Default::default()
        });
        *engine
            .spread_db
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        let open = |relative: &str| {
            let ContainerResponse::Success(payload) = engine.container(ContainerRequest {
                address: favorite_address(&favorite, relative),
                spread_mode: None,
                reading_direction: None,
                force_single_page: false,
            }) else {
                panic!("folder container enumeration failed for {relative}");
            };
            payload
        };

        let image_only_payload = open("image-only");
        assert_eq!(
            image_only_payload.configured_spread_mode,
            RemoteSpreadMode::RtlCover
        );
        assert_eq!(image_only_payload.image_count, 2);
        assert_eq!(image_only_payload.video_count, 0);
        assert_eq!(image_only_payload.other_count, 0);

        let mixed_media_payload = open("mixed-media");
        assert_eq!(
            mixed_media_payload.configured_spread_mode,
            RemoteSpreadMode::Single
        );
        assert_eq!(
            mixed_media_payload.reading_direction,
            RemoteReadingDirection::Ltr
        );
        assert_eq!(mixed_media_payload.image_count, 2);
        assert_eq!(mixed_media_payload.video_count, 1);
        assert_eq!(mixed_media_payload.other_count, 0);

        let mixed_containers_payload = open("mixed-containers");
        assert_eq!(
            mixed_containers_payload.configured_spread_mode,
            RemoteSpreadMode::Single
        );

        let spread_path = temp.path().join("spread.db");
        let writable = crate::spread_db::SpreadDb::open_at(&spread_path).unwrap();
        let key = crate::spread_db::container_key_with_fallback(&mixed_media, &[]);
        writable
            .set(
                &key.exact,
                crate::settings::SpreadMode::Rtl,
                crate::settings::SpreadMode::Single,
                crate::settings::ReadingFlow::Paged,
                crate::settings::ReadingDirection::Rtl,
            )
            .unwrap();
        *engine
            .spread_db
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(writable);

        let stored_payload = open("mixed-media");
        assert_eq!(stored_payload.configured_spread_mode, RemoteSpreadMode::Rtl);
        assert_eq!(stored_payload.effective_spread_mode, RemoteSpreadMode::Rtl);
        assert_eq!(
            stored_payload.reading_direction,
            RemoteReadingDirection::Rtl
        );
    }

    #[test]
    fn zip_and_pdf_spread_defaults_remain_book_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let engine = ContainerEngine::new(crate::settings::Settings {
            default_spread_mode: crate::settings::SpreadMode::LtrCover,
            default_reading_direction: crate::settings::ReadingDirection::Ltr,
            ..Default::default()
        });
        *engine
            .spread_db
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;

        for extension in ["zip", "pdf"] {
            let path = temp.path().join(format!("book.{extension}"));
            std::fs::write(&path, b"container").unwrap();
            let resolved = resolve_existing(path.to_string_lossy().as_ref()).unwrap();
            let item = if extension == "zip" {
                crate::grid_item::GridItem::ZipImage {
                    zip_path: path.clone(),
                    entry_name: "001.jpg".to_owned(),
                }
            } else {
                crate::grid_item::GridItem::PdfPage {
                    pdf_path: path.clone(),
                    page_num: 0,
                    content_type: None,
                }
            };
            let spread = engine.spread_payload(
                &ContainerRequest {
                    address: RemoteAddress::file(path.to_string_lossy().into_owned()),
                    spread_mode: None,
                    reading_direction: None,
                    force_single_page: false,
                },
                &resolved,
                &[item],
                None,
                None,
            );
            assert_eq!(spread.configured, RemoteSpreadMode::LtrCover);
            assert_eq!(spread.effective, RemoteSpreadMode::LtrCover);
            assert_eq!(spread.image_count, 1);
            assert_eq!(spread.video_count, 0);
            assert_eq!(spread.other_count, 0);
        }
    }

    #[test]
    fn portrait_forces_single_without_changing_the_configured_mode() {
        assert_eq!(
            resolve_spread_state(
                None,
                None,
                Some(crate::settings::SpreadMode::RtlCover),
                Some(crate::settings::ReadingDirection::Ltr),
                crate::settings::SpreadMode::Ltr,
                crate::settings::ReadingDirection::Ltr,
                true,
            ),
            (
                RemoteSpreadMode::RtlCover,
                RemoteSpreadMode::Single,
                RemoteReadingDirection::Rtl,
            )
        );
        assert_eq!(
            resolve_spread_state(
                Some(RemoteSpreadMode::LtrCover),
                Some(RemoteReadingDirection::Rtl),
                Some(crate::settings::SpreadMode::Rtl),
                Some(crate::settings::ReadingDirection::Rtl),
                crate::settings::SpreadMode::Single,
                crate::settings::ReadingDirection::Rtl,
                false,
            ),
            (
                RemoteSpreadMode::LtrCover,
                RemoteSpreadMode::LtrCover,
                RemoteReadingDirection::Ltr,
            )
        );
        assert_eq!(
            resolve_spread_state(
                None,
                None,
                Some(crate::settings::SpreadMode::Single),
                Some(crate::settings::ReadingDirection::Rtl),
                crate::settings::SpreadMode::Ltr,
                crate::settings::ReadingDirection::Ltr,
                false,
            ),
            (
                RemoteSpreadMode::Single,
                RemoteSpreadMode::Single,
                RemoteReadingDirection::Rtl,
            )
        );
    }

    #[test]
    fn spread_mode_resolution_uses_stored_then_default_and_never_writes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("spread.db");
        let book = temp.path().join("book.zip");
        let writable = crate::spread_db::SpreadDb::open_at(&path).unwrap();
        writable
            .set(
                &book,
                crate::settings::SpreadMode::Rtl,
                crate::settings::SpreadMode::Single,
                crate::settings::ReadingFlow::Paged,
                crate::settings::ReadingDirection::Ltr,
            )
            .unwrap();
        drop(writable);
        let read_only = crate::spread_db::SpreadDb::open_existing_read_only_at(&path)
            .unwrap()
            .unwrap();
        let stored = read_only.get(&book);
        let stored_direction = read_only.get_direction(&book);
        assert_eq!(
            resolve_spread_state(
                Some(RemoteSpreadMode::Ltr),
                None,
                stored,
                stored_direction,
                crate::settings::SpreadMode::Single,
                crate::settings::ReadingDirection::Rtl,
                false,
            ),
            (
                RemoteSpreadMode::Ltr,
                RemoteSpreadMode::Ltr,
                RemoteReadingDirection::Ltr,
            )
        );
        assert_eq!(read_only.get(&book), Some(crate::settings::SpreadMode::Rtl));
        assert_eq!(
            read_only.get_direction(&book),
            Some(crate::settings::ReadingDirection::Rtl)
        );
        assert_eq!(
            resolve_spread_state(
                None,
                None,
                read_only.get(&book),
                read_only.get_direction(&book),
                crate::settings::SpreadMode::Single,
                crate::settings::ReadingDirection::Ltr,
                true,
            ),
            (
                RemoteSpreadMode::Rtl,
                RemoteSpreadMode::Single,
                RemoteReadingDirection::Rtl,
            )
        );
        assert_eq!(
            read_only.get(&book),
            Some(crate::settings::SpreadMode::Rtl),
            "portrait-only effective Single must not overwrite the configured value"
        );
        assert_eq!(
            resolve_spread_state(
                None,
                None,
                None,
                None,
                crate::settings::SpreadMode::LtrCover,
                crate::settings::ReadingDirection::Rtl,
                false,
            )
            .0,
            RemoteSpreadMode::LtrCover
        );
    }
}
