//! 自動検出の受付・所有・結果採用。要求の世代は表示位置の付け直しで変更しない。

use super::*;
use crate::ai::detect::{DetectError, Detection, FaceDetectParams};
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

impl MaskDetectTool {
    fn name(self) -> &'static str {
        match self {
            Self::Erase => "erase",
            Self::Conceal => "conceal",
            Self::LocalAdjust => "local_adjust",
        }
    }
}

pub(crate) struct MaskDetectRequest {
    pub(super) context: ViewerContextId,
    pub(super) page_key: String,
    items_generation: u64,
    target: MaskDetectTarget,
    source: Arc<egui::ColorImage>,
    mask_size: [usize; 2],
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

struct MaskDetectOutput {
    boxes: Vec<DetectedBox>,
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
    MaskDetectOutput { boxes }
}

impl App {
    pub(crate) fn mask_detect_active(&self) -> bool {
        self.mask_detect_dialog.is_some() || self.mask_detect_pending.is_some()
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
        self.mask_detect_dialog = None;
        self.mask_detect_pending = None;
    }

    pub(crate) fn cancel_mask_detect_for_tool(&mut self, tool: MaskDetectTool) {
        if self.mask_detect_dialog == Some(tool) {
            self.mask_detect_dialog = None;
        }
        if self
            .mask_detect_pending
            .as_ref()
            .is_some_and(|p| p.request.target.tool() == tool)
        {
            self.mask_detect_pending = None;
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
            self.mask_detect_dialog = Some(tool);
        }
    }

    fn capture_mask_detect_request(&mut self, tool: MaskDetectTool) -> Option<MaskDetectRequest> {
        if self.mask_detect_pending.is_some() || !self.mask_detect_can_start(tool) {
            return None;
        }
        let (idx, source, mask_size, source_kind) = self.mask_detect_source(tool)?;
        if source.size != mask_size || mask_size.contains(&0) {
            self.show_feedback_error("画像とマスクの大きさが一致しません".to_string());
            return None;
        }
        let target = match tool {
            MaskDetectTool::Erase => MaskDetectTarget::Erase {
                session: self.erase_session,
            },
            MaskDetectTool::Conceal => MaskDetectTarget::Conceal {
                session: self.conceal_session,
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
                    self.show_feedback_error("画像とマスクの大きさが一致しません".into());
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
        let options = self.settings.mask_detect.clone();
        let trace_label = tool.name();
        self.launch_mask_detect_worker(request, move |worker_cancel| {
            match runtime_init.wait_terminal_while(|| !worker_cancel.load(Ordering::Acquire)) {
                crate::ai::runtime::AiRuntimeInitWait::Ready(runtime) => {
                    match manager.face_detector_yunet_path() {
                        Some(path) => crate::ai::detect::detect_faces_with_trace(
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
                        .map(|detections| mask_detect_boxes(detections, &options))
                        .map_err(MaskDetectError::Detect),
                        None => Err(MaskDetectError::MissingModel),
                    }
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
        if self.mask_detect_pending.is_some() {
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
                self.mask_detect_dialog = None;
                self.mask_detect_pending = Some(Box::new(MaskDetectPending {
                    request,
                    cancel,
                    rx,
                }));
            }
            Err(error) => {
                self.show_feedback_error(format!("顔の検出を開始できませんでした: {error}"))
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
            MaskDetectTarget::Erase { session } => *session == self.erase_session,
            MaskDetectTarget::Conceal {
                session,
                source_kind,
            } => *session == self.conceal_session && *source_kind == kind,
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
            self.show_feedback_toast("画像が変わったため中止しました".into());
            return;
        }
        let count = output.boxes.len();
        if count == 0 {
            self.show_feedback_toast("見つかりませんでした".into());
            return;
        }
        if self.remote_session_blocks_local_control() || !self.admit_book_append_followup_mutation()
        {
            return;
        }
        let start = std::time::Instant::now();
        let tool = request.target.tool();
        let shapes = output
            .boxes
            .into_iter()
            .map(DetectedBox::into_shape)
            .collect();
        match &request.target {
            MaskDetectTarget::Erase { .. } => self.apply_mask_detect_to_erase(shapes),
            MaskDetectTarget::Conceal { .. } => self.apply_mask_detect_to_conceal(shapes),
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
        self.show_feedback_toast(format!("{count} か所を追加しました"));
    }

    pub(crate) fn poll_mask_detect(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.mask_detect_pending.as_ref() else {
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
                self.mask_detect_pending = None;
                self.show_feedback_error("顔の検出処理が停止しました".into());
                return;
            }
            Ok(result) => result,
        };
        let pending = self.mask_detect_pending.take().unwrap();
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
                            DetectError::LoadFailed(_) => "顔の検出モデルを読み込めませんでした",
                            DetectError::InferenceFailed(_) => "顔の検出に失敗しました",
                            DetectError::OutputShape(_) => "顔の検出結果の形式を読めませんでした",
                            DetectError::InvalidInput(_) => "顔の検出に使う画像を読めませんでした",
                            _ => unreachable!(),
                        };
                        crate::logger::log(format!("mask_detect: {error}"));
                        message.into()
                    }
                };
                crate::logger::log(format!("mask_detect: {message}"));
                self.show_feedback_error(message);
            }
        }
        ctx.request_repaint();
    }

    pub(crate) fn show_mask_detect_dialog(&mut self, ctx: &egui::Context) {
        if self.mask_detect_pending.is_some() {
            crate::os_theme::with_dark_context_style(ctx, || {
                egui::Window::new("顔を検出中")
                    .id(egui::Id::unique("mask_detect_progress"))
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
        let Some(tool) = self.mask_detect_dialog else {
            return;
        };
        let mut open = true;
        let mut detect = false;
        let mut close = self.dialog_escape_pressed(ctx);
        let _window = crate::os_theme::with_dark_context_style(ctx, || {
            egui::Window::new("自動検出")
                .id(egui::Id::unique("mask_detect_settings"))
                .open(&mut open)
                .order(egui::Order::Foreground)
                .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
                .default_width(340.0)
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
                    ui.label("顔を検出してマスクを追加します");
                    ui.separator();
                    let spacing = ui.spacing().item_spacing.y;
                    let footer = ui.spacing().interact_size.y
                        + spacing * 4.0
                        + egui::Frame::window(ui.style()).total_margin().bottom;
                    let body_height = (ctx.content_rect().bottom() - ui.cursor().top() - footer)
                        .min(ui.available_height() - footer)
                        .max(1.0);
                    let _body = egui::ScrollArea::vertical()
                        .id_salt("mask_detect_body")
                        .max_height(body_height)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            // 検出器の行は段 2 で増やす。段 1 は同梱の顔だけ。
                            ui.label("顔 (内蔵)");
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
                            ui.add(
                                egui::Slider::new(
                                    &mut self.settings.mask_detect.margin_percent,
                                    0.0..=100.0,
                                )
                                .text("余白 (%)"),
                            )
                            .on_hover_text("幅・高さの指定割合を各辺の外側に足します");
                            let _details = ui.collapsing("詳細", |ui| {
                                ui.add(
                                    egui::Slider::new(
                                        &mut self.settings.mask_detect.threshold,
                                        0.0..=1.0,
                                    )
                                    .text("閾値"),
                                );
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
                    let enabled = self.mask_detect_can_start(tool);
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
            self.mask_detect_dialog = None;
        } else if detect {
            self.start_mask_detect(tool);
        }
    }
}

#[cfg(test)]
mod input_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod tests;
