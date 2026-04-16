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
    let perf_enabled = crate::perf::is_enabled();
    let t0 = std::time::Instant::now();
    if perf_enabled {
        crate::perf::event("ai", "denoise_begin", None, 0, &[
            ("model", serde_json::Value::from(format!("{:?}", model_kind))),
            ("w", serde_json::Value::from(w)),
            ("h", serde_json::Value::from(h)),
        ]);
    }
    crate::logger::log(format!(
        "[AI] Denoise {}x{} with {:?}",
        w, h, model_kind
    ));

    let result = super::upscale::upscale(runtime, model_kind, input, cancel)?;

    crate::logger::log(format!(
        "[AI] Denoise complete: {}x{} with {:?}",
        w, h, model_kind
    ));
    if perf_enabled {
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        crate::perf::event("ai", "denoise_end", None, 0, &[
            ("model", serde_json::Value::from(format!("{:?}", model_kind))),
            ("ms", serde_json::Value::from(ms)),
        ]);
    }

    Ok(result)
}
