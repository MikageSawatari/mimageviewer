//! AI アップスケール推論（タイル分割方式 + オーバーラップブレンド）。
//!
//! 入力画像をオーバーラップ付きタイルに分割し、
//! 各タイルを ONNX モデルでアップスケールして結合する。
//! オーバーラップ領域は線形ブレンド（フェザリング）でタイル境界の継ぎ目を除去する。
//! モデルの倍率（2x/4x）は推論結果のシェイプから自動検出する。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::runtime::AiRuntime;
use super::{AiError, ModelKind};

/// ModelKind ごとのスケール倍率キャッシュ。
/// 同一モデルの detect_scale_factor を毎回実行する無駄を省く。
static SCALE_CACHE: std::sync::LazyLock<Mutex<HashMap<ModelKind, u32>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// デフォルトのタイルサイズ（入力ピクセル）。
const TILE_SIZE: u32 = 192;

/// タイル間のオーバーラップ（入力ピクセル）。
/// スクリーントーン等の規則的パターンで境界が目立たないよう、十分な幅を確保。
const TILE_OVERLAP: u32 = 32;

/// モデルごとの固定タイルサイズ。None の場合はデフォルト (TILE_SIZE) を使用。
/// 一部の ONNX モデルは固定入力サイズでエクスポートされているため、
/// そのサイズに合わせたタイル分割が必要。
fn model_tile_size(kind: ModelKind) -> u32 {
    match kind {
        // RealPLKSR は 256x256 固定入力
        ModelKind::DenoiseRealplksr => 256,
        _ => TILE_SIZE,
    }
}


/// アップスケール要求の結果。
pub struct UpscaleResult {
    pub idx: usize,
    pub image: egui::ColorImage,
}

/// 画像サイズがしきい値未満か（しきい値以上ならスキップ）。
pub fn should_process(width: u32, height: u32, threshold: u32) -> bool {
    width < threshold && height < threshold
}

/// 1 タイルを推論してスケール倍率を検出する（結果をキャッシュ）。
fn detect_scale_factor(
    runtime: &AiRuntime,
    model_kind: ModelKind,
) -> Result<u32, AiError> {
    // キャッシュ済みならそのまま返す
    if let Some(&scale) = SCALE_CACHE.lock().unwrap().get(&model_kind) {
        return Ok(scale);
    }

    let test_size = model_tile_size(model_kind) as usize;
    let dummy = ndarray::Array4::<f32>::zeros((1, 3, test_size, test_size));
    let tensor = ort::value::Tensor::from_array(dummy)
        .map_err(|e| AiError::Ort(format!("Tensor: {e}")))?;

    let scale = runtime.with_session(model_kind, |session| {
        let outputs = session
            .run(ort::inputs![tensor])
            .map_err(|e| AiError::Ort(format!("detect_scale run: {e}")))?;
        let (shape, _) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| AiError::Ort(format!("detect_scale extract: {e}")))?;

        let dims: Vec<i64> = shape.iter().copied().collect();
        if dims.len() >= 4 {
            let out_h = dims[2] as f64;
            let s = (out_h / test_size as f64).round() as u32;
            crate::logger::log(format!(
                "[AI] detect_scale: {model_kind:?} input={test_size}x{test_size} → output={}x{} → scale={s}x",
                dims[3], dims[2]
            ));
            Ok(s.max(1))
        } else {
            Ok(4)
        }
    })?;

    SCALE_CACHE.lock().unwrap().insert(model_kind, scale);
    Ok(scale)
}

/// 画像をアップスケールする。
///
/// タイル分割 + オーバーラップ線形ブレンドで VRAM オーバーフローを防止し、
/// タイル境界の継ぎ目を除去する。
pub fn upscale(
    runtime: &AiRuntime,
    model_kind: ModelKind,
    input: &image::DynamicImage,
    cancel: &Arc<AtomicBool>,
) -> Result<egui::ColorImage, AiError> {
    let (in_w, in_h) = (input.width(), input.height());

    let scale = detect_scale_factor(runtime, model_kind)?;
    let out_w = in_w * scale;
    let out_h = in_h * scale;

    let tile_size = model_tile_size(model_kind);

    crate::logger::log(format!(
        "[AI] Upscaling {}x{} → {}x{} ({}x) with {:?}, tile={}px overlap={}px",
        in_w, in_h, out_w, out_h, scale, model_kind, tile_size, TILE_OVERLAP
    ));

    let rgb = input.to_rgb8();
    let tiles = compute_tiles(in_w, in_h, tile_size, TILE_OVERLAP);

    // 出力バッファ: RGB float 累積 + 重み累積（ブレンド用）
    let npixels = (out_w * out_h) as usize;
    let mut accum_r = vec![0.0f32; npixels];
    let mut accum_g = vec![0.0f32; npixels];
    let mut accum_b = vec![0.0f32; npixels];
    let mut accum_w = vec![0.0f32; npixels];

    for (tile_idx, tile) in tiles.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(AiError::Cancelled);
        }

        let tile_input = extract_tile(&rgb, tile);
        let tile_out = run_tile_inference(runtime, model_kind, tile_input)?;

        // タイル出力を重み付きで出力バッファに加算
        blend_tile(
            &mut accum_r, &mut accum_g, &mut accum_b, &mut accum_w,
            out_w, out_h,
            &tile_out,
            tile, scale,
            in_w, in_h,
        );

        if (tile_idx + 1) % 10 == 0 {
            crate::logger::log(format!(
                "[AI] Upscale progress: {}/{} tiles",
                tile_idx + 1, tiles.len()
            ));
        }
    }

    crate::logger::log(format!(
        "[AI] Upscale complete: {} tiles, {}x scale",
        tiles.len(), scale
    ));

    // 累積バッファを正規化して RGBA ColorImage に変換
    let pixels: Vec<egui::Color32> = (0..npixels)
        .map(|i| {
            let w = accum_w[i].max(1e-6);
            let r = (accum_r[i] / w).clamp(0.0, 255.0) as u8;
            let g = (accum_g[i] / w).clamp(0.0, 255.0) as u8;
            let b = (accum_b[i] / w).clamp(0.0, 255.0) as u8;
            egui::Color32::from_rgb(r, g, b)
        })
        .collect();

    Ok(egui::ColorImage::new(
        [out_w as usize, out_h as usize],
        pixels,
    ))
}

#[derive(Debug, Clone)]
struct TileRect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

fn compute_tiles(img_w: u32, img_h: u32, tile_size: u32, overlap: u32) -> Vec<TileRect> {
    let mut tiles = Vec::new();
    let step = tile_size.saturating_sub(overlap).max(1);

    let mut y = 0u32;
    loop {
        let ty = y;
        let th = tile_size.min(img_h.saturating_sub(ty));
        if th == 0 { break; }

        let mut x = 0u32;
        loop {
            let tx = x;
            let tw = tile_size.min(img_w.saturating_sub(tx));
            if tw == 0 { break; }
            tiles.push(TileRect { x: tx, y: ty, w: tw, h: th });

            if tx + tw >= img_w { break; }
            x += step;
            if x + tile_size > img_w {
                x = img_w.saturating_sub(tile_size);
            }
        }

        if ty + th >= img_h { break; }
        y += step;
        if y + tile_size > img_h {
            y = img_h.saturating_sub(tile_size);
        }
    }

    tiles
}

fn extract_tile(rgb: &image::RgbImage, tile: &TileRect) -> ndarray::Array4<f32> {
    let tw = tile.w as usize;
    let th = tile.h as usize;
    let mut tensor = ndarray::Array4::<f32>::zeros((1, 3, th, tw));

    for dy in 0..th {
        for dx in 0..tw {
            let px = rgb.get_pixel(tile.x + dx as u32, tile.y + dy as u32);
            for c in 0..3 {
                tensor[[0, c, dy, dx]] = px.0[c] as f32 / 255.0;
            }
        }
    }

    tensor
}

struct TileOutput {
    /// RGB float [0, 255] のピクセルデータ（3チャンネル平面: [R..., G..., B...])
    data: Vec<f32>,
    width: u32,
    height: u32,
}

/// 1 タイルの推論を実行する。
fn run_tile_inference(
    runtime: &AiRuntime,
    model_kind: ModelKind,
    input: ndarray::Array4<f32>,
) -> Result<TileOutput, AiError> {
    let input_tensor = ort::value::Tensor::from_array(input)
        .map_err(|e| AiError::Ort(format!("Tensor: {e}")))?;

    runtime.with_session(model_kind, |session| {
        let outputs = session
            .run(ort::inputs![input_tensor])
            .map_err(|e| AiError::Ort(format!("run ({model_kind:?}): {e}")))?;

        let (shape, raw) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| AiError::Ort(format!("extract ({model_kind:?}): {e}")))?;

        let dims: Vec<i64> = shape.iter().copied().collect();
        let (out_ch, actual_out_h, actual_out_w) = if dims.len() >= 4 {
            (dims[1] as usize, dims[2] as usize, dims[3] as usize)
        } else {
            return Err(AiError::Ort(format!("Unexpected output shape: {dims:?}")));
        };

        // NCHW float → RGB float [0, 255] の平面配置で保存
        let ch = out_ch.min(3);
        let plane_size = actual_out_h * actual_out_w;
        let mut data = vec![0.0f32; 3 * plane_size];
        for c in 0..ch {
            for y in 0..actual_out_h {
                for x in 0..actual_out_w {
                    let src_idx = c * plane_size + y * actual_out_w + x;
                    let dst_idx = c * plane_size + y * actual_out_w + x;
                    let val = raw.get(src_idx).copied().unwrap_or(0.0);
                    data[dst_idx] = (val * 255.0).clamp(0.0, 255.0);
                }
            }
        }

        Ok(TileOutput {
            data,
            width: actual_out_w as u32,
            height: actual_out_h as u32,
        })
    })
}

/// タイル出力を重み付きで累積バッファに加算する（距離ベース線形ブレンド）。
///
/// 各ピクセルの重みは「タイルの各辺からの距離」の最小値に基づく。
/// 辺に近いほど重みが小さく、中心ほど大きい。
/// 画像の端に接する辺は常に高重み（ランプなし）。
/// 隣接タイルのオーバーラップ量が不均一でも正しく正規化される。
fn blend_tile(
    accum_r: &mut [f32], accum_g: &mut [f32], accum_b: &mut [f32], accum_w: &mut [f32],
    out_w: u32, out_h: u32,
    tile_out: &TileOutput,
    tile: &TileRect,
    scale: u32,
    img_w: u32, img_h: u32,
) {
    let tw = tile_out.width as usize;
    let th = tile_out.height as usize;
    let plane_size = tw * th;

    let is_first_x = tile.x == 0;
    let is_first_y = tile.y == 0;
    let is_last_x = tile.x + tile.w >= img_w;
    let is_last_y = tile.y + tile.h >= img_h;

    // ランプ幅（出力ピクセル単位）
    let ramp = (TILE_OVERLAP * scale) as f32;

    let dst_x0 = (tile.x * scale) as usize;
    let dst_y0 = (tile.y * scale) as usize;

    for sy in 0..th {
        let dy = dst_y0 + sy;
        if dy >= out_h as usize { break; }

        // Y方向の辺からの距離
        let dist_top = if is_first_y { ramp } else { sy as f32 };
        let dist_bot = if is_last_y { ramp } else { (th - 1 - sy) as f32 };
        let wy = (dist_top.min(dist_bot) / ramp).clamp(1e-4, 1.0);

        for sx in 0..tw {
            let dx = dst_x0 + sx;
            if dx >= out_w as usize { break; }

            // X方向の辺からの距離
            let dist_left = if is_first_x { ramp } else { sx as f32 };
            let dist_right = if is_last_x { ramp } else { (tw - 1 - sx) as f32 };
            let wx = (dist_left.min(dist_right) / ramp).clamp(1e-4, 1.0);

            let weight = wx * wy;
            let dst_idx = dy * out_w as usize + dx;
            let src_idx = sy * tw + sx;

            accum_r[dst_idx] += tile_out.data[src_idx] * weight;
            accum_g[dst_idx] += tile_out.data[plane_size + src_idx] * weight;
            accum_b[dst_idx] += tile_out.data[2 * plane_size + src_idx] * weight;
            accum_w[dst_idx] += weight;
        }
    }
}
