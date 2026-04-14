//! 見開きページ中央欠落の AI 補完（MI-GAN Inpainting）。
//!
//! 左右ページ画像を gap_width 分の空白を挟んで合成し、
//! gap + trim 領域をマスクとして MI-GAN モデルで inpainting する。
//!
//! MI-GAN は 512×512 固定入力、DirectML で 22ms の高速推論が可能。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::runtime::AiRuntime;
use super::{AiError, ModelKind};

/// Inpainting の結果。
pub struct InpaintResult {
    pub left_idx: usize,
    pub right_idx: usize,
    pub gap_width: u32,
    pub trim: u32,
    pub combined: egui::ColorImage,
}

/// ギャップ幅の上限（ピクセル）。
pub const MAX_GAP_WIDTH: u32 = 400;

/// MI-GAN モデルの固定入力サイズ。
const MIGAN_SIZE: u32 = 512;

/// gap 左右から切り出すコンテキスト幅（ピクセル、元画像スケール）。
const DEFAULT_CONTEXT_PIXELS: u32 = 256;

/// 見開きページの中央欠落を AI で補完する。
///
/// `trim` はページ端の汚れを除去するピクセル数。
pub fn inpaint_spread(
    runtime: &AiRuntime,
    left_image: &egui::ColorImage,
    right_image: &egui::ColorImage,
    gap_width: u32,
    trim: u32,
    cancel: &Arc<AtomicBool>,
) -> Result<egui::ColorImage, AiError> {
    let gap_width = gap_width.min(MAX_GAP_WIDTH);
    let lw = left_image.size[0] as u32;
    let rw = right_image.size[0] as u32;
    let lh = left_image.size[1] as u32;
    let rh = right_image.size[1] as u32;
    let combined_h = lh.max(rh);
    let left_trim = trim.min(lw / 4);
    let right_trim = trim.min(rw / 4);

    crate::logger::log(format!(
        "[AI] MI-GAN inpaint: {}x{} + gap({}) + {}x{}, trim={}, combined_h={}",
        lw, lh, gap_width, rw, rh, trim, combined_h
    ));

    if cancel.load(Ordering::Relaxed) {
        return Err(AiError::Cancelled);
    }

    // ── Step 1: gap + trim 周辺ストリップを切り出す ──
    let left_ctx = DEFAULT_CONTEXT_PIXELS.min(lw.saturating_sub(left_trim));
    let right_ctx = DEFAULT_CONTEXT_PIXELS.min(rw.saturating_sub(right_trim));
    let inpaint_w = left_trim + gap_width + right_trim;
    let strip_w = left_ctx + inpaint_w + right_ctx;
    let strip_h = combined_h;

    let mut strip_rgb = vec![0.0f32; (strip_w * strip_h * 3) as usize];

    // 左コンテキスト + 左トリム
    let left_total = left_ctx + left_trim;
    let left_src_x0 = lw - left_total;
    for y in 0..lh.min(strip_h) {
        for x in 0..left_total {
            let src_idx = (y * lw + left_src_x0 + x) as usize;
            let dst_base = ((y * strip_w + x) * 3) as usize;
            let c = left_image.pixels[src_idx];
            strip_rgb[dst_base] = c.r() as f32 / 255.0;
            strip_rgb[dst_base + 1] = c.g() as f32 / 255.0;
            strip_rgb[dst_base + 2] = c.b() as f32 / 255.0;
        }
    }

    // 右トリム + 右コンテキスト
    let right_total = right_trim + right_ctx;
    let right_dst_x0 = left_total + gap_width;
    for y in 0..rh.min(strip_h) {
        for x in 0..right_total {
            let src_idx = (y * rw + x) as usize;
            let dst_base = ((y * strip_w + right_dst_x0 + x) * 3) as usize;
            let c = right_image.pixels[src_idx];
            strip_rgb[dst_base] = c.r() as f32 / 255.0;
            strip_rgb[dst_base + 1] = c.g() as f32 / 255.0;
            strip_rgb[dst_base + 2] = c.b() as f32 / 255.0;
        }
    }

    if cancel.load(Ordering::Relaxed) {
        return Err(AiError::Cancelled);
    }

    // ── Step 2: 512×512 にリサイズ ──
    let migan_rgb = resize_rgb_bilinear(&strip_rgb, strip_w, strip_h, MIGAN_SIZE, MIGAN_SIZE);

    // マスク X 座標（512×512 スケール）
    let scale_x = MIGAN_SIZE as f32 / strip_w as f32;
    let mask_x0 = (left_ctx as f32 * scale_x).round() as u32;
    let mask_x1 = (((left_ctx + inpaint_w) as f32) * scale_x).round().min(MIGAN_SIZE as f32) as u32;

    crate::logger::log(format!(
        "[AI] MI-GAN strip: {}x{} → {}x{}, mask x=[{}..{}]",
        strip_w, strip_h, MIGAN_SIZE, MIGAN_SIZE, mask_x0, mask_x1
    ));

    // ── Step 3: MI-GAN 4ch テンソル構築 ──
    // ch0: mask - 0.5 (known=0.5, inpaint=-0.5)
    // ch1-3: image[-1,1] * mask (known 領域のみ)
    let s = MIGAN_SIZE as usize;
    let mut input_nchw = ndarray::Array4::<f32>::zeros((1, 4, s, s));

    for y in 0..s {
        for x in 0..s {
            let base = (y * s + x) * 3;
            let is_masked = x >= mask_x0 as usize && x < mask_x1 as usize;
            let m = if is_masked { 0.0f32 } else { 1.0f32 };

            input_nchw[[0, 0, y, x]] = m - 0.5;
            input_nchw[[0, 1, y, x]] = (migan_rgb[base] * 2.0 - 1.0) * m;
            input_nchw[[0, 2, y, x]] = (migan_rgb[base + 1] * 2.0 - 1.0) * m;
            input_nchw[[0, 3, y, x]] = (migan_rgb[base + 2] * 2.0 - 1.0) * m;
        }
    }

    let input_tensor = ort::value::Tensor::from_array(input_nchw)
        .map_err(|e| AiError::Ort(format!("Input tensor: {e}")))?;

    if cancel.load(Ordering::Relaxed) {
        return Err(AiError::Cancelled);
    }

    // ── Step 4: MI-GAN 推論 ──
    let gap_rgb_small = runtime.with_session(ModelKind::InpaintMiGan, |session| {
        let outputs = session
            .run(ort::inputs!["input" => input_tensor])
            .map_err(|e| AiError::Ort(format!("MI-GAN run: {e}")))?;

        let (_shape, raw) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| AiError::Ort(format!("MI-GAN extract: {e}")))?;

        // 出力 NCHW [1, 3, 512, 512], 範囲 [-1,1] → gap 部分を [0,1] で取り出す
        let gap_w = (mask_x1 - mask_x0) as usize;
        let mut gap_rgb = vec![0.0f32; gap_w * s * 3];
        for y in 0..s {
            for x in 0..gap_w {
                let src_x = mask_x0 as usize + x;
                let dst = (y * gap_w + x) * 3;
                gap_rgb[dst] = (raw.get(0 * s * s + y * s + src_x).copied().unwrap_or(0.0) * 0.5 + 0.5).clamp(0.0, 1.0);
                gap_rgb[dst + 1] = (raw.get(1 * s * s + y * s + src_x).copied().unwrap_or(0.0) * 0.5 + 0.5).clamp(0.0, 1.0);
                gap_rgb[dst + 2] = (raw.get(2 * s * s + y * s + src_x).copied().unwrap_or(0.0) * 0.5 + 0.5).clamp(0.0, 1.0);
            }
        }
        Ok((gap_rgb, gap_w as u32))
    })?;

    let (gap_rgb_lama, gap_small_w) = gap_rgb_small;

    crate::logger::log("[AI] MI-GAN inference done, compositing...".to_string());

    if cancel.load(Ordering::Relaxed) {
        return Err(AiError::Cancelled);
    }

    // ── Step 5: gap 部分を元解像度にリサイズ ──
    let inpaint_rgb_full = if gap_small_w != inpaint_w || MIGAN_SIZE != combined_h {
        resize_rgb_bilinear(&gap_rgb_lama, gap_small_w, MIGAN_SIZE, inpaint_w, combined_h)
    } else {
        gap_rgb_lama
    };

    // ── Step 6: 左 + 補完領域 + 右 の合成画像を生成 ──
    let total_w = lw + gap_width + rw;
    let left_show = lw - left_trim;
    let right_start = right_trim;
    let mut pixels = Vec::with_capacity((total_w * combined_h) as usize);

    for y in 0..combined_h {
        for x in 0..left_show {
            if y < lh {
                pixels.push(left_image.pixels[(y * lw + x) as usize]);
            } else {
                pixels.push(egui::Color32::BLACK);
            }
        }
        for x in 0..inpaint_w {
            let base = ((y * inpaint_w + x) * 3) as usize;
            if base + 2 < inpaint_rgb_full.len() {
                let r = (inpaint_rgb_full[base] * 255.0).clamp(0.0, 255.0) as u8;
                let g = (inpaint_rgb_full[base + 1] * 255.0).clamp(0.0, 255.0) as u8;
                let b = (inpaint_rgb_full[base + 2] * 255.0).clamp(0.0, 255.0) as u8;
                pixels.push(egui::Color32::from_rgb(r, g, b));
            } else {
                pixels.push(egui::Color32::BLACK);
            }
        }
        for x in right_start..rw {
            if y < rh {
                pixels.push(right_image.pixels[(y * rw + x) as usize]);
            } else {
                pixels.push(egui::Color32::BLACK);
            }
        }
    }

    crate::logger::log(format!(
        "[AI] MI-GAN inpaint complete: {}x{}", total_w, combined_h
    ));
    Ok(egui::ColorImage::new([total_w as usize, combined_h as usize], pixels))
}

/// バイリニア補間による RGB f32 画像のリサイズ。
fn resize_rgb_bilinear(
    src: &[f32],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Vec<f32> {
    let mut dst = vec![0.0f32; (dst_w * dst_h * 3) as usize];
    let x_ratio = src_w as f32 / dst_w as f32;
    let y_ratio = src_h as f32 / dst_h as f32;

    for dy in 0..dst_h {
        let sy = dy as f32 * y_ratio;
        let y0 = (sy as u32).min(src_h.saturating_sub(1));
        let y1 = (y0 + 1).min(src_h.saturating_sub(1));
        let fy = sy - y0 as f32;

        for dx in 0..dst_w {
            let sx = dx as f32 * x_ratio;
            let x0 = (sx as u32).min(src_w.saturating_sub(1));
            let x1 = (x0 + 1).min(src_w.saturating_sub(1));
            let fx = sx - x0 as f32;

            for c in 0..3 {
                let v00 = src[((y0 * src_w + x0) * 3 + c) as usize];
                let v10 = src[((y0 * src_w + x1) * 3 + c) as usize];
                let v01 = src[((y1 * src_w + x0) * 3 + c) as usize];
                let v11 = src[((y1 * src_w + x1) * 3 + c) as usize];
                let v = v00 * (1.0 - fx) * (1.0 - fy)
                    + v10 * fx * (1.0 - fy)
                    + v01 * (1.0 - fx) * fy
                    + v11 * fx * fy;
                dst[((dy * dst_w + dx) * 3 + c) as usize] = v;
            }
        }
    }
    dst
}
