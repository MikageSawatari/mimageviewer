use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{ModelKind, model_manager::ModelManager, runtime::AiRuntime};

pub(crate) trait FinalAiProgressSink {
    fn loading_model(&self, _kind: ModelKind) {}
    fn denoising(&self, _completed_tiles: usize, _total_tiles: usize) {}
    fn upscaling(&self, _completed_tiles: usize, _total_tiles: usize) {}
}

pub(crate) struct NoFinalAiProgress;
impl FinalAiProgressSink for NoFinalAiProgress {}

pub(crate) struct FinalAiExecutionRequest {
    pub(crate) source: Arc<egui::ColorImage>,
    pub(crate) adjust_before_ai: Option<crate::adjustment::AdjustParams>,
    pub(crate) denoise_kind: Option<ModelKind>,
    pub(crate) upscale_kind: Option<ModelKind>,
    pub(crate) background_mode: u8,
}

pub(crate) struct FinalAiExecutionOutput {
    pub(crate) image: egui::ColorImage,
    pub(crate) source_size: [usize; 2],
    pub(crate) used_upscale: bool,
}

pub(crate) enum FinalAiExecutionError {
    Cancelled,
    Failed(String),
}

pub(crate) fn effective_upscale_request(
    mode: crate::settings::AiFeatureMode,
    params: &crate::adjustment::AdjustParams,
) -> Option<Option<ModelKind>> {
    if matches!(mode, crate::settings::AiFeatureMode::Disabled) {
        return None;
    }
    let request = params.upscale_model_kind()?;
    match request {
        None => Some(None),
        Some(kind) if mode.allows_upscale_model(kind) => Some(Some(kind)),
        Some(_) => None,
    }
}

pub(crate) fn effective_denoise_request(
    mode: crate::settings::AiFeatureMode,
    params: &crate::adjustment::AdjustParams,
) -> Option<ModelKind> {
    mode.allows_denoise()
        .then(|| params.denoise_model_kind())
        .flatten()
}

#[derive(Clone, Copy)]
pub(crate) struct SelectedFinalAiModels {
    pub(crate) denoise: Option<ModelKind>,
    pub(crate) upscale: Option<ModelKind>,
}

/// `category_override` は「表示側が既に分類済みならその答えを使う」ための入口。
/// auto upscale のモデル選択は分類結果に依存するので、同じページの別経路 (Ctrl+E の
/// プリセット再合成など) が表示と違うモデルを選ばないようにする。`None` ならここで分類する。
pub(crate) fn select_final_ai_models(
    source: &egui::ColorImage,
    params: &crate::adjustment::AdjustParams,
    mode: crate::settings::AiFeatureMode,
    upscale_limit: crate::ai::upscale::AiProcessSizeLimit,
    denoise_limit: crate::ai::upscale::AiProcessSizeLimit,
    category_override: Option<crate::ai::ImageCategory>,
) -> Option<SelectedFinalAiModels> {
    let [width, height] = source.size;
    let upscale_request = effective_upscale_request(mode, params);
    let denoise_request = effective_denoise_request(mode, params);
    let upscale_in_range = upscale_request.is_some()
        && crate::ai::upscale::should_process_rect(width as u32, height as u32, upscale_limit);
    let denoise_in_range = denoise_request.is_some()
        && crate::ai::upscale::should_process_rect(width as u32, height as u32, denoise_limit);
    if !upscale_in_range && !denoise_in_range {
        return None;
    }
    let denoise = denoise_in_range.then_some(denoise_request).flatten();
    let upscale = if upscale_in_range {
        match upscale_request.expect("in-range upscale has a request") {
            Some(kind) => Some(kind),
            None => {
                let category = category_override.unwrap_or_else(|| {
                    let source = crate::app::color_image_to_dynamic(source);
                    crate::ai::classify::classify_heuristic(&source)
                });
                mode.auto_upscale_model(category)
            }
        }
    } else {
        None
    };
    (denoise.is_some() || upscale.is_some()).then_some(SelectedFinalAiModels { denoise, upscale })
}

pub(crate) fn execute_selected_final_ai(
    runtime: &AiRuntime,
    manager: &ModelManager,
    request: FinalAiExecutionRequest,
    cancel: &Arc<AtomicBool>,
    progress: &dyn FinalAiProgressSink,
) -> Result<FinalAiExecutionOutput, FinalAiExecutionError> {
    let source_size = request.source.size;
    let denoise_model = load_requested_model(runtime, manager, request.denoise_kind, progress);
    let upscale_model = load_requested_model(runtime, manager, request.upscale_kind, progress);
    if denoise_model.is_none() && upscale_model.is_none() {
        return Err(FinalAiExecutionError::Failed(
            "no usable AI model (load failed)".to_owned(),
        ));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(FinalAiExecutionError::Cancelled);
    }
    let source = request.adjust_before_ai.as_ref().map_or_else(
        || Arc::clone(&request.source),
        |params| {
            Arc::new(crate::adjustment::apply_adjustments_fast(
                &request.source,
                params,
            ))
        },
    );
    if cancel.load(Ordering::Relaxed) {
        return Err(FinalAiExecutionError::Cancelled);
    }
    let background = if request.background_mode == 1 {
        [255, 255, 255]
    } else {
        [0, 0, 0]
    };
    let mut image = if upscale_model.is_some() {
        crate::app::color_image_to_dynamic_composited(&source, background)
    } else {
        crate::app::color_image_to_dynamic(&source)
    };

    if let Some(kind) = denoise_model {
        let tile_progress = |completed, total| progress.denoising(completed, total);
        match crate::ai::denoise::denoise_with_progress(
            runtime,
            kind,
            &image,
            cancel,
            Some(&tile_progress),
        ) {
            Ok(denoised) if upscale_model.is_some() => {
                image = crate::app::color_image_to_dynamic(&denoised);
            }
            Ok(denoised) => {
                return Ok(FinalAiExecutionOutput {
                    image: denoised,
                    source_size,
                    used_upscale: false,
                });
            }
            Err(_error) if cancel.load(Ordering::Relaxed) => {
                return Err(FinalAiExecutionError::Cancelled);
            }
            Err(error) if upscale_model.is_none() => {
                return Err(FinalAiExecutionError::Failed(error.to_string()));
            }
            Err(error) => {
                crate::logger::log(format!("[AI] Final denoise failed: {error}"));
            }
        }
    }

    let Some(kind) = upscale_model else {
        return Err(FinalAiExecutionError::Failed(
            "no upscale model after denoise".to_owned(),
        ));
    };
    let tile_progress = |completed, total| progress.upscaling(completed, total);
    match crate::ai::upscale::upscale_to_max_dim_with_progress(
        runtime,
        kind,
        &image,
        cancel,
        crate::canonical_image_loader::CANONICAL_RASTER_MAX_LONG_EDGE,
        Some(&tile_progress),
    ) {
        Ok(result) => Ok(FinalAiExecutionOutput {
            image: crate::app::clamp_color_image_for_gpu(result.image),
            source_size,
            used_upscale: result.used_upscale,
        }),
        Err(_) if cancel.load(Ordering::Relaxed) => Err(FinalAiExecutionError::Cancelled),
        Err(error) => Err(FinalAiExecutionError::Failed(error.to_string())),
    }
}

fn load_requested_model(
    runtime: &AiRuntime,
    manager: &ModelManager,
    kind: Option<ModelKind>,
    progress: &dyn FinalAiProgressSink,
) -> Option<ModelKind> {
    let kind = kind?;
    progress.loading_model(kind);
    if runtime.is_loaded(kind) {
        return Some(kind);
    }
    let path = manager.model_path(kind)?;
    match runtime.load_model(kind, &path) {
        Ok(()) => Some(kind),
        Err(error) => {
            crate::logger::log(format!(
                "[AI] Final model load failed kind={kind:?}: {error}"
            ));
            None
        }
    }
}
