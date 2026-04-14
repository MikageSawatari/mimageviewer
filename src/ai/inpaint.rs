//! 見開きページ中央欠落の AI 補完（LaMa Inpainting）。
//!
//! 左右ページ画像を gap_width 分の空白を挟んで合成し、
//! gap 領域をマスクとして LaMa モデルで inpainting する。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::runtime::AiRuntime;
use super::{AiError, ModelKind};

/// Inpainting の結果。
pub struct InpaintResult {
    /// 左ページのアイテムインデックス
    pub left_idx: usize,
    /// 右ページのアイテムインデックス
    pub right_idx: usize,
    /// 補完時のギャップ幅
    pub gap_width: u32,
    /// 合成済み画像（左 + 補完 gap + 右）
    pub combined: egui::ColorImage,
}

/// ギャップ幅の上限（ピクセル）。
pub const MAX_GAP_WIDTH: u32 = 200;

/// 見開きページの中央欠落を AI で補完する。
///
/// 1. 左右画像を gap_width 分の空白を挟んで合成
/// 2. gap 領域を白マスクとして作成
/// 3. LaMa モデルで inpainting 推論
/// 4. 合成画像を返却
pub fn inpaint_spread(
    runtime: &AiRuntime,
    left_image: &egui::ColorImage,
    right_image: &egui::ColorImage,
    gap_width: u32,
    cancel: &Arc<AtomicBool>,
) -> Result<egui::ColorImage, AiError> {
    let gap_width = gap_width.min(MAX_GAP_WIDTH);
    let lw = left_image.size[0] as u32;
    let rw = right_image.size[0] as u32;
    let lh = left_image.size[1] as u32;
    let rh = right_image.size[1] as u32;

    // 両ページの高さを揃える（高い方に合わせる）
    let combined_h = lh.max(rh);
    let combined_w = lw + gap_width + rw;

    crate::logger::log(format!(
        "[AI] Inpainting spread: {}x{} + gap({}) + {}x{} → {}x{}",
        lw, lh, gap_width, rw, rh, combined_w, combined_h
    ));

    if cancel.load(Ordering::Relaxed) {
        return Err(AiError::Cancelled);
    }

    // ── Step 1: 合成画像を作成（gap 部分は黒） ──
    let mut combined_rgb = ndarray::Array3::<f32>::zeros((combined_h as usize, combined_w as usize, 3));

    // 左ページを配置
    for y in 0..lh.min(combined_h) as usize {
        for x in 0..lw as usize {
            let c = left_image.pixels[y * lw as usize + x];
            combined_rgb[[y, x, 0]] = c.r() as f32 / 255.0;
            combined_rgb[[y, x, 1]] = c.g() as f32 / 255.0;
            combined_rgb[[y, x, 2]] = c.b() as f32 / 255.0;
        }
    }

    // 右ページを配置
    let right_x_offset = (lw + gap_width) as usize;
    for y in 0..rh.min(combined_h) as usize {
        for x in 0..rw as usize {
            let c = right_image.pixels[y * rw as usize + x];
            combined_rgb[[y, right_x_offset + x, 0]] = c.r() as f32 / 255.0;
            combined_rgb[[y, right_x_offset + x, 1]] = c.g() as f32 / 255.0;
            combined_rgb[[y, right_x_offset + x, 2]] = c.b() as f32 / 255.0;
        }
    }

    if cancel.load(Ordering::Relaxed) {
        return Err(AiError::Cancelled);
    }

    // ── Step 2: マスク画像を作成（gap 領域 = 1.0, その他 = 0.0） ──
    let mut mask = ndarray::Array2::<f32>::zeros((combined_h as usize, combined_w as usize));
    for y in 0..combined_h as usize {
        for x in lw as usize..(lw + gap_width) as usize {
            mask[[y, x]] = 1.0;
        }
    }

    // ── Step 3: LaMa 推論 ──
    // LaMa の入力形式: image [1, 3, H, W], mask [1, 1, H, W]
    let h = combined_h as usize;
    let w = combined_w as usize;

    let mut image_nchw = ndarray::Array4::<f32>::zeros((1, 3, h, w));
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                image_nchw[[0, c, y, x]] = combined_rgb[[y, x, c]];
            }
        }
    }

    let mut mask_nchw = ndarray::Array4::<f32>::zeros((1, 1, h, w));
    for y in 0..h {
        for x in 0..w {
            mask_nchw[[0, 0, y, x]] = mask[[y, x]];
        }
    }

    let image_tensor = ort::value::Tensor::from_array(image_nchw)
        .map_err(|e| AiError::Ort(format!("Image tensor: {e}")))?;
    let mask_tensor = ort::value::Tensor::from_array(mask_nchw)
        .map_err(|e| AiError::Ort(format!("Mask tensor: {e}")))?;

    if cancel.load(Ordering::Relaxed) {
        return Err(AiError::Cancelled);
    }

    let result = runtime.with_session(ModelKind::InpaintLama, |session| {
        let outputs = session
            .run(ort::inputs!["image" => image_tensor, "mask" => mask_tensor])
            .map_err(|e| AiError::Ort(format!("Inpaint run: {e}")))?;

        let (_shape, raw) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| AiError::Ort(format!("Inpaint extract: {e}")))?;

        // 出力は NCHW [1, 3, H, W], 値域 [0, 1]
        let mut pixels = Vec::with_capacity(h * w);
        for y in 0..h {
            for x in 0..w {
                let r = (raw.get(0 * h * w + y * w + x).copied().unwrap_or(0.0) * 255.0).clamp(0.0, 255.0) as u8;
                let g = (raw.get(1 * h * w + y * w + x).copied().unwrap_or(0.0) * 255.0).clamp(0.0, 255.0) as u8;
                let b = (raw.get(2 * h * w + y * w + x).copied().unwrap_or(0.0) * 255.0).clamp(0.0, 255.0) as u8;
                pixels.push(egui::Color32::from_rgb(r, g, b));
            }
        }

        Ok(egui::ColorImage::new([w, h], pixels))
    })?;

    crate::logger::log("[AI] Inpainting complete".to_string());
    Ok(result)
}
