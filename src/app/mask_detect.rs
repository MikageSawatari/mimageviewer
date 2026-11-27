//! 自動検出の受付・所有・結果採用。要求の世代は表示位置の付け直しで変更しない。

use super::*;
use crate::ai::detect::{
    DetectError, Detection, FaceDetectParams, MaskPatch, UltralyticsOutput, UltralyticsParams,
};
use crate::ai::detector_catalog::{DetectorEntry, DetectorFormat, DetectorInfo, DetectorShape};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MaskDetectTool {
    Erase,
    Conceal,
    LocalAdjust,
}

/// Reconcile the front modal's order against tool panels, without overtaking
/// popups/other foreground UI. No display-start timer or pending repair state.
fn maintain_mask_detect_window_layer(ctx: &egui::Context, layer: egui::LayerId) {
    if egui::Popup::is_any_open(ctx) {
        return;
    }
    let needs_raise = ctx.memory(|memory| {
        if memory.top_modal_layer() != Some(layer) {
            return false;
        }
        // top_modal_layer is one pass old. Also respect another visible Area
        // above us, including a Modal that appeared in the previous pass.
        let panels = [
            "erase_tool_panel",
            "conceal_panel",
            "local_adjust_panel",
            "local_adjust_tool_panel",
        ]
        .map(egui::Id::unique);
        let mut found = false;
        let mut panel_above = false;
        for candidate in memory.layer_ids() {
            if candidate == layer {
                found = true;
                continue;
            }
            if found
                && candidate.order == egui::Order::Foreground
                && memory.areas().is_visible(&candidate)
            {
                if !panels.contains(&candidate.id) {
                    return false;
                }
                panel_above = true;
            }
        }
        panel_above
    });
    if needs_raise {
        ctx.move_to_top(layer);
        ctx.request_repaint(); // Recheck after the stable sort, only on a repair.
        #[cfg(test)]
        {
            let key = egui::Id::unique(("mask_detect_window_raises", ctx.viewport_id(), layer.id));
            let pass = ctx.cumulative_pass_nr();
            ctx.data_mut(|data| data.get_temp_mut_or_default::<Vec<u64>>(key).push(pass));
        }
    }
}

impl MaskDetectTool {
    fn name(self) -> &'static str {
        match self {
            Self::Erase => "erase",
            Self::Conceal => "conceal",
            Self::LocalAdjust => "local_adjust",
        }
    }
}

/// App-common catalog stays put while only `context` is exchanged with a bundle.
#[derive(Default)]
pub(crate) struct MaskDetectState {
    pub(crate) catalog: DetectorCatalogState,
    pub(crate) context: Box<MaskDetectContextState>,
}

/// One viewer's dialog, inference and edit identities move and drop together.
#[derive(Default)]
pub(crate) struct MaskDetectContextState {
    pub(crate) pending: Option<Box<MaskDetectPending>>,
    pub(crate) dialog: Option<MaskDetectTool>,
    pub(crate) erase_session: u64,
    pub(crate) conceal_session: u64,
}

pub(crate) struct MaskDetectRequest {
    pub(super) context: ViewerContextId,
    pub(super) page_key: String,
    items_generation: u64,
    target: MaskDetectTarget,
    source: Arc<egui::ColorImage>,
    mask_size: [usize; 2],
    options: crate::settings::MaskDetectSettings,
    detectors: Vec<ChosenDetector>,
}

#[derive(Clone)]
struct ChosenDetector {
    info: DetectorInfo,
    params: UltralyticsParams,
    shape: DetectorShape,
}

#[derive(Default)]
pub(crate) enum DetectorCatalogState {
    #[default]
    Idle,
    Loading {
        cancel: Arc<AtomicBool>,
        rx: mpsc::Receiver<Result<Vec<DetectorEntry>, String>>,
    },
    Ready(Result<Vec<DetectorEntry>, String>),
}

impl Drop for DetectorCatalogState {
    fn drop(&mut self) {
        if let Self::Loading { cancel, .. } = self {
            cancel.store(true, Ordering::Release);
        }
    }
}

pub(crate) enum MaskDetectTarget {
    Erase {
        session: u64,
    },
    Conceal {
        session: u64,
        source_kind: &'static str,
    },
    LocalAdjust {
        owner: page_edit_owner::PageEditOwner,
        document: local_adjust_core::LocalAdjustmentLayers,
        layer_idx: usize,
        edit_target: LocalAdjustMaskEditTarget,
    },
}

impl MaskDetectTarget {
    fn tool(&self) -> MaskDetectTool {
        match self {
            Self::Erase { .. } => MaskDetectTool::Erase,
            Self::Conceal { .. } => MaskDetectTool::Conceal,
            Self::LocalAdjust { .. } => MaskDetectTool::LocalAdjust,
        }
    }
}

pub(crate) struct MaskDetectPending {
    pub(super) request: MaskDetectRequest,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<MaskDetectOutput, MaskDetectError>>,
}

impl Drop for MaskDetectPending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl MaskDetectPending {
    pub(super) fn local_document_differs(
        &self,
        key: &str,
        layers: &local_adjust_core::LocalAdjustmentLayers,
    ) -> bool {
        matches!(&self.request.target, MaskDetectTarget::LocalAdjust { document, .. }
            if self.request.page_key == key && document != layers)
    }
}

#[derive(Default)]
struct MaskDetectOutput {
    boxes: Vec<DetectedBox>,
    patches: Vec<MaskPatch>,
    counts: Vec<(String, usize)>,
}

impl MaskDetectOutput {
    /// Merge worker results after detection has discarded empty contours, keeping
    /// the reported class counts aligned with the masks actually being adopted.
    fn append_ultralytics_result(
        &mut self,
        chosen: &ChosenDetector,
        result: UltralyticsOutput,
        margin_percent: f32,
    ) {
        for (class_idx, name) in chosen.info.names.iter().enumerate() {
            let count = result
                .detections
                .iter()
                .filter(|d| d.class_idx == class_idx)
                .count();
            if count > 0 {
                self.counts
                    .push((format!("{} / {name}", chosen.info.name), count));
            }
        }
        if chosen.shape == DetectorShape::Contour {
            self.patches.extend(result.patches);
        } else {
            let shape_options = crate::settings::MaskDetectSettings {
                ellipse: chosen.shape == DetectorShape::Ellipse,
                margin_percent,
                ..Default::default()
            };
            self.boxes
                .extend(mask_detect_boxes(result.detections, &shape_options).boxes);
        }
    }
}

/// Worker output in image coordinates; mask types are constructed only on the UI.
struct DetectedBox {
    ellipse: bool,
    center: (f32, f32),
    half_w: f32,
    half_h: f32,
}

impl DetectedBox {
    fn into_shape(self) -> crate::mask_db::Shape {
        let Self {
            ellipse,
            center,
            half_w,
            half_h,
        } = self;
        if ellipse {
            crate::mask_db::Shape::Ellipse {
                op: crate::mask_db::ShapeOp::Add,
                center,
                rx: half_w,
                ry: half_h,
                rotation_rad: 0.0,
            }
        } else {
            crate::mask_db::Shape::Rect {
                op: crate::mask_db::ShapeOp::Add,
                center,
                half_w,
                half_h,
                rotation_rad: 0.0,
            }
        }
    }
}

enum MaskDetectError {
    Detect(DetectError),
    Runtime(String),
    MissingModel,
}

fn mask_detect_boxes(
    detections: Vec<Detection>,
    options: &crate::settings::MaskDetectSettings,
) -> MaskDetectOutput {
    let count = detections.len();
    let boxes = detections
        .into_iter()
        .map(|d| {
            let [x0, y0, x1, y1] = d.rect;
            let center = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
            // Each edge receives the selected percentage of the box's width/height.
            let scale = 1.0 + 2.0 * options.margin_percent / 100.0;
            let half_w = (x1 - x0) * 0.5 * scale;
            let half_h = (y1 - y0) * 0.5 * scale;
            DetectedBox {
                ellipse: options.ellipse,
                center,
                half_w,
                half_h,
            }
        })
        .collect();
    MaskDetectOutput {
        boxes,
        counts: vec![("顔".into(), count)],
        ..Default::default()
    }
}

fn detector_shape(
    info: &DetectorInfo,
    setting: &crate::settings::ExternalDetectorSettings,
) -> DetectorShape {
    let shape = setting.shape.or(info.shape).unwrap_or(match info.format {
        DetectorFormat::UltralyticsDetect => DetectorShape::Rect,
        DetectorFormat::UltralyticsSegment => DetectorShape::Contour,
    });
    // A model may have been replaced under the same filename since settings were saved.
    if info.format == DetectorFormat::UltralyticsDetect && shape == DetectorShape::Contour {
        DetectorShape::Rect
    } else {
        shape
    }
}

fn chosen_detectors(
    catalog: &DetectorCatalogState,
    options: &crate::settings::MaskDetectSettings,
) -> Vec<ChosenDetector> {
    let DetectorCatalogState::Ready(Ok(entries)) = catalog else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let info = entry.result.as_ref().ok()?;
            let setting = options.detectors.get(&entry.name)?;
            if !setting.selected {
                return None;
            }
            let selected_classes: Vec<bool> = info
                .names
                .iter()
                .map(|name| {
                    setting
                        .classes
                        .as_ref()
                        .is_none_or(|classes| classes.contains(name))
                })
                .collect();
            if !selected_classes.iter().any(|selected| *selected) {
                return None;
            }
            Some(ChosenDetector {
                info: info.clone(),
                shape: detector_shape(info, setting),
                params: UltralyticsParams {
                    selected_classes,
                    thresholds: setting.threshold.map_or_else(
                        || info.thresholds.clone(),
                        |value| vec![value; info.names.len()],
                    ),
                    contour_margin_percent: options.contour_margin_percent,
                },
            })
        })
        .collect()
}

fn mask_detect_margin_visibility(
    catalog: &DetectorCatalogState,
    options: &crate::settings::MaskDetectSettings,
) -> (bool, bool) {
    let mut boxes = options.face_selected;
    let mut contours = false;
    if let DetectorCatalogState::Ready(Ok(entries)) = catalog {
        for entry in entries {
            let (Ok(info), Some(setting)) = (&entry.result, options.detectors.get(&entry.name))
            else {
                continue;
            };
            if !setting.selected {
                continue;
            }
            contours |= info.format == DetectorFormat::UltralyticsSegment;
            // Segment models can also add boxes/ellipses, using the box margin.
            boxes |= info.format == DetectorFormat::UltralyticsDetect
                || detector_shape(info, setting) != DetectorShape::Contour;
        }
    }
    (boxes, contours)
}

struct DetectorCatalogReport {
    readable: usize,
    unreadable: usize,
    lines: Vec<String>,
}

/// Pure formatting keeps the completion diagnostics testable without installing
/// a global logger. Debug quoting preserves one line even for multiline errors.
fn detector_catalog_report(
    folder: &std::path::Path,
    result: &Result<Vec<DetectorEntry>, String>,
) -> DetectorCatalogReport {
    match result {
        Ok(entries) => {
            let readable = entries.iter().filter(|entry| entry.result.is_ok()).count();
            let unreadable = entries.len() - readable;
            let mut lines = vec![format!(
                "mask_detect/catalog folder={folder:?} readable={readable} unreadable={unreadable}"
            )];
            for entry in entries {
                if let Err(error) = &entry.result {
                    lines.push(format!(
                        "mask_detect/catalog unreadable name={:?} reason={error:?} detail={:?}",
                        entry.name,
                        error.to_string()
                    ));
                }
            }
            DetectorCatalogReport {
                readable,
                unreadable,
                lines,
            }
        }
        Err(error) => DetectorCatalogReport {
            readable: 0,
            unreadable: 0,
            lines: vec![format!(
                "mask_detect/catalog folder={folder:?} readable=0 unreadable=0 error={error:?}"
            )],
        },
    }
}

/// Only the returned patch area is visited; existing subtract shapes still apply later.
pub(crate) fn apply_mask_patches(
    patches: &[MaskPatch],
    size: [usize; 2],
    mut set: impl FnMut(usize),
) {
    for patch in patches {
        for row in 0..patch.h.min(size[1].saturating_sub(patch.y)) {
            for col in 0..patch.w.min(size[0].saturating_sub(patch.x)) {
                if patch.bits[row * patch.w + col] {
                    set((patch.y + row) * size[0] + patch.x + col);
                }
            }
        }
    }
}

fn detection_summary(count: usize, counts: &[(String, usize)]) -> String {
    let nonzero: Vec<_> = counts.iter().filter(|(_, n)| *n > 0).collect();
    let details = nonzero
        .iter()
        .take(3)
        .map(|(name, n)| {
            let mut short: String = name.chars().take(24).collect();
            if name.chars().count() > 24 {
                short.push('…');
            }
            format!("{short} {n}")
        })
        .collect::<Vec<_>>()
        .join("、");
    if details.is_empty() {
        return format!("{count} か所を追加しました");
    }
    format!(
        "{count} か所を追加しました（{details}{}）",
        if nonzero.len() > 3 { "、ほか" } else { "" }
    )
}

fn show_detector_entries(
    ui: &mut egui::Ui,
    catalog: &DetectorCatalogState,
    options: &mut crate::settings::MaskDetectSettings,
) {
    match catalog {
        DetectorCatalogState::Idle | DetectorCatalogState::Loading { .. } => {
            ui.label("検出器を読み込み中…");
        }
        DetectorCatalogState::Ready(Err(error)) => {
            ui.weak(error);
        }
        DetectorCatalogState::Ready(Ok(entries)) => {
            if entries.is_empty() {
                ui.weak("追加の検出器はありません");
            }
            for entry in entries {
                ui.push_id(&entry.name, |ui| {
                    ui.separator();
                    let info = match &entry.result {
                        Ok(info) => info,
                        Err(error) => {
                            ui.weak(&entry.name);
                            ui.weak(error.message());
                            return;
                        }
                    };
                    let setting = options.detectors.entry(entry.name.clone()).or_default();
                    ui.checkbox(&mut setting.selected, &entry.name);
                    let kind = match info.format {
                        DetectorFormat::UltralyticsDetect => "箱",
                        DetectorFormat::UltralyticsSegment => "輪郭",
                    };
                    ui.weak(format!("{} 分類・{kind}", info.names.len()));
                    if let Some(license) = &info.license {
                        ui.weak(format!("ライセンス: {license}"));
                    }
                    if !setting.selected {
                        return;
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label("形");
                        let current = detector_shape(info, setting);
                        if info.format == DetectorFormat::UltralyticsSegment
                            && ui
                                .selectable_label(current == DetectorShape::Contour, "輪郭に沿う")
                                .clicked()
                        {
                            setting.shape = Some(DetectorShape::Contour);
                        }
                        if ui
                            .selectable_label(current == DetectorShape::Ellipse, "楕円")
                            .clicked()
                        {
                            setting.shape = Some(DetectorShape::Ellipse);
                        }
                        if ui
                            .selectable_label(current == DetectorShape::Rect, "矩形")
                            .clicked()
                        {
                            setting.shape = Some(DetectorShape::Rect);
                        }
                    });
                    let mut all = setting.classes.is_none();
                    if ui.checkbox(&mut all, "すべての分類").changed() {
                        setting.classes = if all { None } else { Some(Vec::new()) };
                    }
                    ui.horizontal_wrapped(|ui| {
                        for name in &info.names {
                            let mut selected = setting
                                .classes
                                .as_ref()
                                .is_none_or(|classes| classes.contains(name));
                            if ui.checkbox(&mut selected, name).changed() {
                                let classes =
                                    setting.classes.get_or_insert_with(|| info.names.clone());
                                if selected {
                                    if !classes.contains(name) {
                                        classes.push(name.clone());
                                    }
                                } else {
                                    classes.retain(|class| class != name);
                                }
                            }
                        }
                    });
                    ui.collapsing("詳細", |ui| {
                        let mut custom = setting.threshold.is_some();
                        if ui.checkbox(&mut custom, "閾値を指定").changed() {
                            setting.threshold = custom.then_some(0.25);
                        }
                        if let Some(threshold) = &mut setting.threshold {
                            ui.add(egui::Slider::new(threshold, 0.0..=1.0).text("閾値"));
                        }
                    });
                });
            }
        }
    }
}

impl App {
    pub(crate) fn apply_detect_mask_patches(
        patches: &[MaskPatch],
        size: [usize; 2],
        set: impl FnMut(usize),
    ) {
        apply_mask_patches(patches, size, set);
    }
    fn reload_mask_detect_catalog(&mut self) {
        if matches!(
            &self.mask_detect.catalog,
            DetectorCatalogState::Loading { .. }
        ) {
            return;
        }
        self.ensure_ai_runtime();
        let runtime_init = Arc::clone(&self.ai_runtime_init);
        let folder = crate::data_dir::get().join("detectors");
        let scan_folder = folder.clone();
        self.launch_mask_detect_catalog_worker(folder, move |cancel| {
            match runtime_init.wait_terminal_while(|| !cancel.load(Ordering::Acquire)) {
                crate::ai::runtime::AiRuntimeInitWait::Ready(_) => {
                    crate::ai::detector_catalog::scan_detectors(&scan_folder)
                }
                crate::ai::runtime::AiRuntimeInitWait::Failed(error) => {
                    Err(format!("AI の準備に失敗しました: {error}"))
                }
                crate::ai::runtime::AiRuntimeInitWait::Cancelled => {
                    Err("読み込みを中止しました".into())
                }
            }
        });
    }

    fn launch_mask_detect_catalog_worker(
        &mut self,
        folder: PathBuf,
        work: impl FnOnce(Arc<AtomicBool>) -> Result<Vec<DetectorEntry>, String> + Send + 'static,
    ) {
        if matches!(
            &self.mask_detect.catalog,
            DetectorCatalogState::Loading { .. }
        ) {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let activity = self.local_ai_activity_lease();
        let (tx, rx) = mpsc::channel();
        match std::thread::Builder::new()
            .name("detector-catalog".into())
            .spawn(move || {
                let _activity = activity;
                let started = std::time::Instant::now();
                let result = work(worker_cancel);
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                let report = detector_catalog_report(&folder, &result);
                for line in report.lines {
                    crate::logger::log(line);
                }
                crate::perf::event(
                    "mask_detect",
                    "catalog",
                    None,
                    0,
                    &[
                        ("readable", report.readable.into()),
                        ("unreadable", report.unreadable.into()),
                        ("ms", elapsed_ms.into()),
                    ],
                );
                let _ = tx.send(result);
            }) {
            Ok(_) => self.mask_detect.catalog = DetectorCatalogState::Loading { cancel, rx },
            Err(error) => {
                self.mask_detect.catalog = DetectorCatalogState::Ready(Err(format!(
                    "検出器の読み込みを開始できません: {error}"
                )))
            }
        }
    }

    fn poll_mask_detect_catalog(&mut self, ctx: &egui::Context) {
        let DetectorCatalogState::Loading { rx, .. } = &self.mask_detect.catalog else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Disconnected) => Err("検出器の読み込みが停止しました".into()),
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
        };
        self.mask_detect.catalog = DetectorCatalogState::Ready(result);
    }

    fn open_mask_detect_folder(&mut self, ctx: &egui::Context) {
        let folder = crate::data_dir::get().join("detectors");
        let failure_tx = self.external_open_failure_tx.clone();
        let ctx = ctx.clone();
        let spawn = std::thread::Builder::new()
            .name("detector-folder-open".into())
            .spawn(move || {
                let result = std::fs::create_dir_all(&folder)
                    .map_err(|error| error.to_string())
                    .and_then(|()| {
                        #[cfg(windows)]
                        {
                            crate::open_with::shell_execute_open(&folder, None, None, true, None)
                        }
                        #[cfg(not(windows))]
                        {
                            opener::open(&folder).map_err(|error| error.to_string())
                        }
                    });
                if let Err(error) = result {
                    crate::logger::log(format!("検出器フォルダを開けません: {error}"));
                    let _ =
                        failure_tx.send(crate::external_links::ExternalOpenFailure::DetectorFolder);
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            });
        if let Err(error) = spawn {
            self.show_mask_detect_error(format!("検出器フォルダを開けません: {error}"));
        }
    }

    fn mask_detect_selection_valid(&self) -> bool {
        // Until the catalog is terminal, selected BYO models cannot be resolved
        // into the immutable request. Do not silently run only the selected face.
        if self
            .settings
            .mask_detect
            .detectors
            .values()
            .any(|setting| setting.selected)
            && !matches!(&self.mask_detect.catalog, DetectorCatalogState::Ready(_))
        {
            return false;
        }
        if self.settings.mask_detect.face_selected {
            return true;
        }
        let DetectorCatalogState::Ready(Ok(entries)) = &self.mask_detect.catalog else {
            return false;
        };
        entries.iter().any(|entry| {
            let (Ok(info), Some(setting)) = (
                &entry.result,
                self.settings.mask_detect.detectors.get(&entry.name),
            ) else {
                return false;
            };
            setting.selected
                && info.names.iter().any(|name| {
                    setting
                        .classes
                        .as_ref()
                        .is_none_or(|classes| classes.contains(name))
                })
        })
    }

    pub(crate) fn mask_detect_active(&self) -> bool {
        self.mask_detect.context.dialog.is_some() || self.mask_detect.context.pending.is_some()
    }

    /// この context で今描かれる窓だけを共通モーダルへ登録する。
    pub(crate) fn mask_detect_dialog_visible(&self) -> bool {
        self.fullscreen_idx.is_some() && self.mask_detect_active()
    }

    pub(crate) fn erase_input_blocked(&self) -> bool {
        self.book_structural_op.is_some() || self.mask_detect_active()
    }
    pub(crate) fn conceal_input_blocked(&self) -> bool {
        self.book_structural_op.is_some() || self.mask_detect_active()
    }
    pub(crate) fn local_adjust_mask_input_blocked(&self) -> bool {
        self.book_structural_op.is_some() || self.mask_detect_active()
    }

    pub(crate) fn cancel_mask_detect(&mut self) {
        self.mask_detect.context.dialog = None;
        self.mask_detect.context.pending = None;
    }

    pub(crate) fn cancel_mask_detect_for_tool(&mut self, tool: MaskDetectTool) {
        if self.mask_detect.context.dialog == Some(tool) {
            self.mask_detect.context.dialog = None;
        }
        if self
            .mask_detect
            .context
            .pending
            .as_ref()
            .is_some_and(|p| p.request.target.tool() == tool)
        {
            self.mask_detect.context.pending = None;
        }
    }

    fn mask_detect_source(
        &mut self,
        tool: MaskDetectTool,
    ) -> Option<(usize, Arc<egui::ColorImage>, [usize; 2], &'static str)> {
        let idx = match tool {
            MaskDetectTool::LocalAdjust => self.current_local_adjust_edit_idx()?,
            _ => self.fullscreen_idx?,
        };
        let (source, size, kind) = match tool {
            MaskDetectTool::Erase if self.erase_mode => (
                Arc::clone(&self.erase_base_cache.get(&idx)?.pixels),
                self.erase_mask_size,
                "erase_base",
            ),
            MaskDetectTool::Conceal if self.conceal_mode => {
                let (pixels, kind) = self.current_conceal_source_pixels(idx)?;
                (pixels, self.conceal_mask_size, kind)
            }
            MaskDetectTool::LocalAdjust if self.local_adjust_mode => {
                let source = self.current_local_adjust_source_pixels(idx)?;
                let layer_idx = self.selected_local_adjust_layer_idx(idx)?;
                let layer = self.local_adjust_page_layers.get(&idx)?.get(layer_idx)?;
                let target = crate::ui_adjustment_panel::effective_local_mask_edit_target(
                    layer,
                    self.local_adjust_mask_edit_target,
                )?;
                let size = crate::ui_adjustment_panel::mask_detect_local_target_size(layer, target)
                    .unwrap_or(source.size);
                (source, size, "local_adjust_source")
            }
            _ => return None,
        };
        Some((idx, source, size, kind))
    }

    pub(crate) fn mask_detect_available(&mut self, tool: MaskDetectTool) -> bool {
        !self.mask_detect_active() && self.mask_detect_can_start(tool)
    }

    fn mask_detect_can_start(&mut self, tool: MaskDetectTool) -> bool {
        self.book_structural_op.is_none()
            && !self.book_order_saving()
            && !self.book_append_undo_in_progress()
            && !self.remote_session_blocks_local_control()
            && (tool != MaskDetectTool::LocalAdjust
                || (self.local_adjust_segmentation_pending.is_none()
                    && self.local_adjust_lut_pending.is_none()))
            && self.mask_detect_source(tool).is_some()
    }

    pub(crate) fn open_mask_detect_dialog(&mut self, tool: MaskDetectTool) {
        if self.mask_detect_available(tool) {
            self.mask_detect.context.dialog = Some(tool);
            self.reload_mask_detect_catalog();
        }
    }

    fn capture_mask_detect_request(&mut self, tool: MaskDetectTool) -> Option<MaskDetectRequest> {
        if self.mask_detect.context.pending.is_some()
            || !self.mask_detect_can_start(tool)
            || !self.mask_detect_selection_valid()
        {
            return None;
        }
        let (idx, source, mask_size, source_kind) = self.mask_detect_source(tool)?;
        if source.size != mask_size || mask_size.contains(&0) {
            self.show_mask_detect_error("画像とマスクの大きさが一致しません".to_string());
            return None;
        }
        let target = match tool {
            MaskDetectTool::Erase => MaskDetectTarget::Erase {
                session: self.mask_detect.context.erase_session,
            },
            MaskDetectTool::Conceal => MaskDetectTarget::Conceal {
                session: self.mask_detect.context.conceal_session,
                source_kind,
            },
            MaskDetectTool::LocalAdjust => {
                self.prepare_page_edit_history_boundary();
                if !self.begin_local_page_edit(idx) {
                    return None;
                }
                let owner = self.capture_page_edit_owner(idx)?;
                let document = self.local_page_edit_layers_for_target(&owner.target);
                let layer_idx = self.selected_local_adjust_layer_idx(idx)?;
                let edit_target = crate::ui_adjustment_panel::effective_local_mask_edit_target(
                    document.get(layer_idx)?,
                    self.local_adjust_mask_edit_target,
                )?;
                if crate::ui_adjustment_panel::mask_detect_local_target_size(
                    document.get(layer_idx)?,
                    edit_target,
                )
                .is_some_and(|size| size != mask_size)
                {
                    self.show_mask_detect_error("画像とマスクの大きさが一致しません".into());
                    return None;
                }
                MaskDetectTarget::LocalAdjust {
                    owner,
                    document,
                    layer_idx,
                    edit_target,
                }
            }
        };
        Some(MaskDetectRequest {
            context: self.projected_viewer_context_id(),
            page_key: self.page_path_key(idx)?,
            items_generation: self.items_generation,
            target,
            source,
            mask_size,
            options: self.settings.mask_detect.as_ref().clone(),
            detectors: chosen_detectors(&self.mask_detect.catalog, &self.settings.mask_detect),
        })
    }

    fn start_mask_detect(&mut self, tool: MaskDetectTool) {
        let Some(request) = self.capture_mask_detect_request(tool) else {
            return;
        };
        self.ensure_ai_runtime();
        let runtime_init = Arc::clone(&self.ai_runtime_init);
        let manager = Arc::clone(&self.ai_model_manager);
        let source = Arc::clone(&request.source);
        let options = request.options.clone();
        let detectors = request.detectors.clone();
        let trace_label = tool.name();
        self.launch_mask_detect_worker(request, move |worker_cancel| {
            match runtime_init.wait_terminal_while(|| !worker_cancel.load(Ordering::Acquire)) {
                crate::ai::runtime::AiRuntimeInitWait::Ready(runtime) => {
                    let mut output = MaskDetectOutput::default();
                    if options.face_selected {
                        let path = manager
                            .face_detector_yunet_path()
                            .ok_or(MaskDetectError::MissingModel)?;
                        let detections = crate::ai::detect::detect_faces_with_trace(
                            &runtime,
                            &path,
                            &source,
                            &FaceDetectParams {
                                score_threshold: options.threshold,
                                ..Default::default()
                            },
                            &worker_cancel,
                            trace_label,
                        )
                        .map_err(MaskDetectError::Detect)?;
                        output = mask_detect_boxes(detections, &options);
                    }
                    for chosen in detectors {
                        if worker_cancel.load(Ordering::Acquire) {
                            return Err(MaskDetectError::Detect(DetectError::Cancelled));
                        }
                        let contour = chosen.shape == DetectorShape::Contour;
                        let result = crate::ai::detect::detect_ultralytics_with_trace(
                            &runtime,
                            &chosen.info,
                            &source,
                            &chosen.params,
                            contour,
                            &worker_cancel,
                            trace_label,
                        )
                        .map_err(MaskDetectError::Detect)?;
                        output.append_ultralytics_result(&chosen, result, options.margin_percent);
                    }
                    Ok(output)
                }
                crate::ai::runtime::AiRuntimeInitWait::Failed(error) => {
                    Err(MaskDetectError::Runtime(error.to_string()))
                }
                crate::ai::runtime::AiRuntimeInitWait::Cancelled => {
                    Err(MaskDetectError::Detect(DetectError::Cancelled))
                }
            }
        });
    }

    /// The same worker owner is used by injected inference in headless tests.
    fn launch_mask_detect_worker(
        &mut self,
        request: MaskDetectRequest,
        work: impl FnOnce(Arc<AtomicBool>) -> Result<MaskDetectOutput, MaskDetectError> + Send + 'static,
    ) {
        if self.mask_detect.context.pending.is_some() {
            return;
        }
        let tool = request.target.tool();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let activity = self.local_ai_activity_lease();
        let (tx, rx) = mpsc::channel();
        let start = std::time::Instant::now();
        let spawn = std::thread::Builder::new()
            .name("mask-detect".into())
            .spawn(move || {
                let _activity = activity;
                let result = work(worker_cancel);
                let _ = tx.send(result);
            });
        match spawn {
            Ok(_) => {
                self.mask_detect.context.dialog = None;
                self.mask_detect.context.pending = Some(Box::new(MaskDetectPending {
                    request,
                    cancel,
                    rx,
                }));
            }
            Err(error) => {
                self.show_mask_detect_error(format!("検出を開始できませんでした: {error}"))
            }
        }
        crate::perf::event(
            "mask_detect",
            "start",
            None,
            0,
            &[
                ("tool", tool.name().into()),
                ("trace_label", tool.name().into()),
                ("ms", (start.elapsed().as_secs_f64() * 1000.0).into()),
            ],
        );
    }

    fn mask_detect_request_is_current(&mut self, request: &MaskDetectRequest) -> bool {
        if request.context != self.projected_viewer_context_id()
            || request.items_generation != self.items_generation
        {
            return false;
        }
        let tool = request.target.tool();
        let Some((idx, source, size, kind)) = self.mask_detect_source(tool) else {
            return false;
        };
        if self.page_path_key(idx).as_deref() != Some(request.page_key.as_str())
            || size != request.mask_size
            || source.size != size
            || !Arc::ptr_eq(&source, &request.source)
        {
            return false;
        }
        match &request.target {
            MaskDetectTarget::Erase { session } => {
                *session == self.mask_detect.context.erase_session
            }
            MaskDetectTarget::Conceal {
                session,
                source_kind,
            } => *session == self.mask_detect.context.conceal_session && *source_kind == kind,
            MaskDetectTarget::LocalAdjust {
                owner, document, ..
            } => {
                self.page_edit_owner_is_current(owner)
                    && self.local_page_edit_document_for_producer(owner, document) == *document
            }
        }
    }

    fn apply_mask_detect_output(&mut self, request: &MaskDetectRequest, output: MaskDetectOutput) {
        if !self.mask_detect_request_is_current(request) {
            self.show_feedback_toast_on(
                "画像が変わったため中止しました".into(),
                ActionSurface::Viewer,
            );
            return;
        }
        let count = output.boxes.len() + output.patches.len();
        if count == 0 {
            self.show_feedback_toast_on("見つかりませんでした".into(), ActionSurface::Viewer);
            return;
        }
        if self.remote_session_blocks_local_control() || !self.admit_book_append_followup_mutation()
        {
            return;
        }
        let start = std::time::Instant::now();
        let tool = request.target.tool();
        let summary = detection_summary(count, &output.counts);
        let shapes = output
            .boxes
            .into_iter()
            .map(DetectedBox::into_shape)
            .collect();
        match &request.target {
            MaskDetectTarget::Erase { .. } => {
                self.apply_mask_detect_to_erase(shapes, &output.patches)
            }
            MaskDetectTarget::Conceal { .. } => {
                self.apply_mask_detect_to_conceal(shapes, &output.patches)
            }
            MaskDetectTarget::LocalAdjust {
                owner,
                document,
                layer_idx,
                edit_target,
            } => {
                self.apply_mask_detect_to_local_adjust(
                    owner,
                    document,
                    *layer_idx,
                    *edit_target,
                    request.mask_size,
                    shapes,
                    &output.patches,
                );
            }
        }
        crate::perf::event(
            "mask_detect",
            "apply",
            None,
            0,
            &[
                ("tool", tool.name().into()),
                ("trace_label", tool.name().into()),
                ("count", count.into()),
                ("width", request.mask_size[0].into()),
                ("height", request.mask_size[1].into()),
                ("ms", (start.elapsed().as_secs_f64() * 1000.0).into()),
            ],
        );
        self.show_feedback_toast_on(summary, ActionSurface::Viewer);
    }

    fn show_mask_detect_error(&mut self, message: String) {
        self.show_feedback_result_on(
            message,
            ActionSurface::Viewer,
            crate::feedback::FeedbackKind::Error,
        );
    }

    pub(crate) fn poll_mask_detect(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.mask_detect.context.pending.as_ref() else {
            return;
        };
        let result = match pending.rx.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {
                // All-context polling uses the live root Context even when this owner
                // is parked. Keep that poll alive until its receiver reaches terminal.
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.mask_detect.context.pending = None;
                self.show_mask_detect_error("検出処理が停止しました".into());
                return;
            }
            Ok(result) => result,
        };
        let pending = self.mask_detect.context.pending.take().unwrap();
        if pending.cancel.load(Ordering::Acquire) {
            return;
        }
        match result {
            Ok(output) => self.apply_mask_detect_output(&pending.request, output),
            Err(MaskDetectError::Detect(DetectError::Cancelled)) => {}
            Err(error) => {
                let message = match error {
                    MaskDetectError::MissingModel
                    | MaskDetectError::Detect(DetectError::ModelMissing(_)) => {
                        "顔の検出モデルが見つかりません".to_string()
                    }
                    MaskDetectError::Runtime(detail) => {
                        format!("AI の準備に失敗しました: {detail}")
                    }
                    MaskDetectError::Detect(error) => {
                        let message = match &error {
                            DetectError::LoadFailed(_) => "検出器を読み込めませんでした",
                            DetectError::InferenceFailed(_) => "検出に失敗しました",
                            DetectError::OutputShape(_) => "検出結果の形式を読めませんでした",
                            DetectError::InvalidInput(_) => "検出に使う画像を読めませんでした",
                            DetectError::DetectorChanged => {
                                "検出器のファイルが変わりました。一覧を読み直してください"
                            }
                            _ => unreachable!(),
                        };
                        crate::logger::log(format!("mask_detect: {error}"));
                        message.into()
                    }
                };
                crate::logger::log(format!("mask_detect: {message}"));
                self.show_mask_detect_error(message);
            }
        }
        ctx.request_repaint();
    }

    pub(crate) fn show_mask_detect_dialog(&mut self, ctx: &egui::Context) {
        self.poll_mask_detect_catalog(ctx);
        if self.mask_detect.context.pending.is_some() {
            let id = egui::Id::unique("mask_detect_progress");
            let layer = egui::LayerId::new(egui::Order::Foreground, id);
            // Like egui::Modal: block lower layers without covering Tooltip.
            // Window::id is also its Area id.
            ctx.memory_mut(|memory| memory.set_modal_layer(layer));
            maintain_mask_detect_window_layer(ctx, layer);
            crate::os_theme::with_dark_context_style(ctx, || {
                egui::Window::new("検出中")
                    .id(id)
                    .order(egui::Order::Foreground)
                    .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        #[cfg(test)]
                        ctx.data_mut(|data| {
                            data.insert_temp(
                                egui::Id::unique("mask_detect_progress_dark_mode"),
                                ui.visuals().dark_mode,
                            );
                        });
                        ui.spinner();
                        if ui.button("キャンセル").clicked() {
                            self.cancel_mask_detect();
                        }
                    });
            });
            return;
        }
        let Some(tool) = self.mask_detect.context.dialog else {
            return;
        };
        let id = egui::Id::unique("mask_detect_settings");
        let layer = egui::LayerId::new(egui::Order::Foreground, id);
        // Raw key reads are not gated by set_modal_layer. Like Modal::should_close,
        // only the front modal owns Escape; None permits a cold first frame.
        let owns_escape =
            ctx.memory(|memory| memory.top_modal_layer().is_none_or(|top| top == layer));
        let mut open = true;
        let mut detect = false;
        let mut close = owns_escape && self.dialog_escape_pressed(ctx);
        let mut reload = false;
        let mut folder = false;
        ctx.memory_mut(|memory| memory.set_modal_layer(layer));
        maintain_mask_detect_window_layer(ctx, layer);
        let _window = crate::os_theme::with_dark_context_style(ctx, || {
            egui::Window::new("自動検出")
                .id(id)
                .open(&mut open)
                .order(egui::Order::Foreground)
                .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
                .default_width(380.0)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    #[cfg(test)]
                    ctx.data_mut(|data| {
                        data.insert_temp(
                            egui::Id::unique("mask_detect_settings_dark_mode"),
                            ui.visuals().dark_mode,
                        );
                    });
                    ui.label("検出した領域をマスクに追加します");
                    ui.separator();
                    let spacing = ui.spacing().item_spacing.y;
                    let footer = ui.spacing().interact_size.y
                        + spacing * 4.0
                        + egui::Frame::window(ui.style()).total_margin().bottom;
                    let body_height = (ctx.content_rect().bottom() - ui.cursor().top() - footer)
                        .min(ui.available_height() - footer)
                        .max(1.0);
                    let scroll = &mut ui.style_mut().spacing.scroll;
                    scroll.floating_allocated_width = scroll.floating_allocated_width.max(scroll.bar_inner_margin + scroll.bar_width);
                    let _body = egui::ScrollArea::vertical()
                        .id_salt("mask_detect_body")
                        .max_height(body_height)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.checkbox(&mut self.settings.mask_detect.face_selected, "顔 (内蔵)");
                            let (show_box_margin, _) = mask_detect_margin_visibility(&self.mask_detect.catalog, &self.settings.mask_detect);
                            if self.settings.mask_detect.face_selected {
                            ui.horizontal(|ui| {
                                ui.label("形");
                                ui.selectable_value(
                                    &mut self.settings.mask_detect.ellipse,
                                    true,
                                    "楕円",
                                );
                                ui.selectable_value(
                                    &mut self.settings.mask_detect.ellipse,
                                    false,
                                    "矩形",
                                );
                            });
                            }
                            if show_box_margin { let _margin = ui.add(
                                egui::Slider::new(
                                    &mut self.settings.mask_detect.margin_percent,
                                    0.0..=100.0,
                                )
                                .text("箱の余白 (%)"),
                            )
                            .on_hover_text("幅・高さの指定割合を各辺の外側に足します");
                            #[cfg(test)]
                            ctx.data_mut(|data| data.insert_temp(egui::Id::unique("mask_detect_box_margin_rect"), _margin.rect));
                            }
                            if self.settings.mask_detect.face_selected {
                            let _details = ui.collapsing("詳細", |ui| {
                                let _threshold = ui.add(
                                    egui::Slider::new(
                                        &mut self.settings.mask_detect.threshold,
                                        0.0..=1.0,
                                    )
                                    .text("閾値"),
                                );
                                #[cfg(test)]
                                ctx.data_mut(|data| data.insert_temp(egui::Id::unique("mask_detect_threshold_rect"), _threshold.rect));
                            });
                            #[cfg(test)]
                            ctx.data_mut(|data| {
                                data.insert_temp(
                                    egui::Id::unique("mask_detect_details"),
                                    _details.header_response.rect,
                                );
                                data.insert_temp(
                                    egui::Id::unique("mask_detect_details_open"),
                                    _details.body_response.is_some(),
                                );
                            });
                            }
                            show_detector_entries(ui, &self.mask_detect.catalog, &mut self.settings.mask_detect);
                            ui.separator();
                            // Resolve again after detector-row controls, so a new
                            // selection is reflected in this same UI pass.
                            let (_, show_contour_margin) = mask_detect_margin_visibility(&self.mask_detect.catalog, &self.settings.mask_detect);
                            if show_contour_margin { ui.add(egui::Slider::new(&mut self.settings.mask_detect.contour_margin_percent, 0.0..=5.0).text("輪郭の余白 (%)"))
                                .on_hover_text("元の画像の長辺に対する割合で太らせます (最小 2px)"); }
                            ui.horizontal_wrapped(|ui| {
                                folder = ui.button("フォルダを開く").clicked();
                                let loading = matches!(&self.mask_detect.catalog, DetectorCatalogState::Loading { .. });
                                let button = ui.add_enabled(!loading, egui::Button::new("一覧を読み直す"));
                                reload = button.clicked();
                                #[cfg(test)]
                                ctx.data_mut(|data| data.insert_temp(egui::Id::unique("mask_detect_reload_enabled"), button.enabled()));
                            });
                            ui.weak("mIV は追加の検出器を配布していません。入手した検出器は配布元のライセンスに従って使ってください");
                        });
                    #[cfg(test)]
                    ctx.data_mut(|data| {
                        let mut layout =
                            crate::test_ui::scroll_layout::ScrollLayout::from_output(&_body);
                        // ScrollAreaOutput.inner_rect is captured before auto_shrink in
                        // show_viewport_dyn. The parent is advanced by end() using the
                        // final allocation. In a vertical-only area this is its bottom.
                        layout.inner.max.y = ui.min_rect().bottom();
                        data.insert_temp(egui::Id::unique("mask_detect_body_layout"), layout)
                    });
                    ui.add(egui::Separator::default().spacing(spacing));
                    let enabled = self.mask_detect_can_start(tool) && self.mask_detect_selection_valid();
                    ui.horizontal(|ui| {
                        let start = ui.add_enabled(enabled, egui::Button::new("検出"));
                        let dismiss = ui.button("閉じる");
                        detect = start.clicked();
                        close |= dismiss.clicked();
                        #[cfg(test)]
                        ctx.data_mut(|data| {
                            data.insert_temp(
                                egui::Id::unique("mask_detect_actions"),
                                vec![start.rect, dismiss.rect],
                            )
                        });
                        #[cfg(test)]
                        ctx.data_mut(|data| data.insert_temp(egui::Id::unique("mask_detect_start_enabled"), start.enabled()));
                    });
                })
        });
        #[cfg(test)]
        if let Some(window) = _window {
            ctx.data_mut(|data| {
                data.insert_temp(egui::Id::unique("mask_detect_window"), window.response.rect)
            });
        }
        if !open || close {
            self.mask_detect.context.dialog = None;
        } else if detect {
            self.start_mask_detect(tool);
        } else if reload {
            self.reload_mask_detect_catalog();
        }
        if folder {
            self.open_mask_detect_folder(ctx);
        }
    }
}

#[cfg(test)]
mod input_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod stage2_tests;
#[cfg(test)]
mod tests;
