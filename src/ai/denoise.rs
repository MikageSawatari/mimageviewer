//! JPEG ノイズ除去（AI デノイズ）。
//!
//! 1x スケールの ONNX モデルを使い、JPEG 圧縮由来のブロックノイズ・
//! モスキートノイズを除去する。タイル分割推論は `upscale` モジュールを流用。

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use super::runtime::AiRuntime;
use super::{AiError, ModelKind};

/// JPEG ノイズ除去を実行する。
///
/// 内部では `upscale::upscale()` と同じタイル分割パイプラインを使用する。
/// 1x スケールモデルなので解像度は変わらず、ノイズのみ除去される。
pub fn denoise(
    runtime: &AiRuntime,
    model_kind: ModelKind,
    input: &image::DynamicImage,
    cancel: &Arc<AtomicBool>,
) -> Result<egui::ColorImage, AiError> {
    let (w, h) = (input.width(), input.height());
    crate::logger::log(format!(
        "[AI] Denoise {}x{} with {:?}",
        w, h, model_kind
    ));

    let result = super::upscale::upscale(runtime, model_kind, input, cancel)?;

    crate::logger::log(format!(
        "[AI] Denoise complete: {}x{} with {:?}",
        w, h, model_kind
    ));

    Ok(result)
}
