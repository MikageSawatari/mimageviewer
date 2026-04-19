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

/// モデルごとの最適タイルサイズ (RTX 4090 + DirectML で実測、`bench_ai --tile-size` 参照)。
///
/// - 固定入力サイズのモデルはそのサイズでしか動かない (例: RealPLKSR 256)。
/// - それ以外のモデルは本来任意サイズで動くが、ONNX ランタイム/GPU の実行効率が
///   特定サイズでピークになる。実測結果は概ね 192 が最適だったが、
///   軽量モデルの `UpscaleRealEsrGeneralV3` のみ 512 の方が 18% 速い
///   (タイル当たり GPU 処理が短くカーネル起動オーバーヘッドが支配的なため、
///    大タイル化で起動回数を 1/8 に減らすと効果が大きい)。
///   画質差は平均 0.06/255、p99.9 ≤ 2/255 で実質同等。
fn model_tile_size(kind: ModelKind) -> u32 {
    match kind {
        // RealPLKSR は 256x256 固定入力
        ModelKind::DenoiseRealplksr => 256,
        // 軽量モデル: 大タイルで GPU カーネル起動オーバーヘッドを削減
        ModelKind::UpscaleRealEsrGeneralV3 => 512,
        _ => TILE_SIZE,
    }
}


/// アップスケール要求の結果。
pub struct UpscaleResult {
    pub idx: usize,
    pub image: egui::ColorImage,
}

/// 1 タイル分のタイミング内訳（ベンチマーク用）。
#[derive(Debug, Clone, Copy)]
pub struct TileTiming {
    /// RgbImage → Array4<f32> コピー (CPU)
    pub extract_ms: f64,
    /// `run_tile_inference` 全体 (以下 4 項目の合計 + ORT 呼び出しオーバーヘッド)
    pub infer_ms: f64,
    /// `ort::value::Tensor::from_array` (CPU, ndarray → ORT tensor)
    pub tensor_build_ms: f64,
    /// `session.run(...)` (GPU 計算 + host↔device 転送を含む)
    pub session_run_ms: f64,
    /// `outputs[0].try_extract_tensor::<f32>()` (CPU, ORT tensor → 参照取得)
    pub tensor_extract_ms: f64,
    /// 出力テンソルから `data: Vec<f32>` へのスカラ変換コピー (CPU)
    pub post_copy_ms: f64,
    /// 累積バッファへの blend (CPU、Case B 以降は別スレッドで並列実行)
    pub blend_ms: f64,
}

/// `upscale_with_timings` が返す全体タイミング内訳。
#[derive(Debug, Clone)]
pub struct UpscaleTimings {
    pub total_ms: f64,
    pub prep_ms: f64,
    pub alpha_resample_ms: f64,
    pub finalize_ms: f64,
    /// GPU 推論ループ完了後に blender スレッドの残タスクを待った時間。
    /// 0 に近いほど blend がボトルネックになっていない (推論と並走できている)。
    pub blend_wait_ms: f64,
    pub tiles: Vec<TileTiming>,
    pub tile_size: u32,
    pub scale: u32,
    pub in_w: u32,
    pub in_h: u32,
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
    upscale_with_timings(runtime, model_kind, input, cancel, None).map(|(img, _)| img)
}

/// `upscale` のタイミング計測版。ベンチマーク用。
///
/// `tile_size_override` で既定タイルサイズを上書きできる。
/// 固定入力サイズの ONNX モデル (例: RealPLKSR 256) では override すると推論が失敗する。
pub fn upscale_with_timings(
    runtime: &AiRuntime,
    model_kind: ModelKind,
    input: &image::DynamicImage,
    cancel: &Arc<AtomicBool>,
    tile_size_override: Option<u32>,
) -> Result<(egui::ColorImage, UpscaleTimings), AiError> {
    let t_all = std::time::Instant::now();
    let t_prep = std::time::Instant::now();

    let (in_w, in_h) = (input.width(), input.height());

    let scale = detect_scale_factor(runtime, model_kind)?;
    let out_w = in_w * scale;
    let out_h = in_h * scale;

    let tile_size = tile_size_override.unwrap_or_else(|| model_tile_size(model_kind));

    crate::logger::log(format!(
        "[AI] Upscaling {}x{} → {}x{} ({}x) with {:?}, tile={}px overlap={}px",
        in_w, in_h, out_w, out_h, scale, model_kind, tile_size, TILE_OVERLAP
    ));

    let rgb = input.to_rgb8();

    // 透明度を持つ画像はアルファを別途 Lanczos3 でリサイズして再結合する。
    // AI モデル (Real-ESRGAN 等) は RGB 3ch 専用で、アルファを直接扱えないため。
    let t_alpha = std::time::Instant::now();
    let alpha_resized: Option<Vec<u8>> = if input.color().has_alpha() {
        let rgba = input.to_rgba8();
        let any_transparent = rgba.pixels().any(|p| p.0[3] < 255);
        if any_transparent {
            let alpha_data: Vec<u8> = rgba.pixels().map(|p| p.0[3]).collect();
            let alpha_img = image::GrayImage::from_raw(in_w, in_h, alpha_data)
                .expect("alpha buffer dimensions match");
            let resized = image::imageops::resize(
                &alpha_img, out_w, out_h,
                image::imageops::FilterType::Lanczos3,
            );
            crate::logger::log(format!(
                "[AI] Upscale: alpha channel resampled via Lanczos3 ({}x{} → {}x{})",
                in_w, in_h, out_w, out_h
            ));
            Some(resized.into_raw())
        } else {
            None
        }
    } else {
        None
    };
    let alpha_resample_ms = t_alpha.elapsed().as_secs_f64() * 1000.0;

    let tiles = compute_tiles(in_w, in_h, tile_size, TILE_OVERLAP);
    let perf_enabled = crate::perf::is_enabled();
    let t_upscale = std::time::Instant::now();
    if perf_enabled {
        crate::perf::event("ai", "upscale_begin", None, 0, &[
            ("model", serde_json::Value::from(format!("{:?}", model_kind))),
            ("in_w", serde_json::Value::from(in_w)),
            ("in_h", serde_json::Value::from(in_h)),
            ("scale", serde_json::Value::from(scale)),
            ("tiles", serde_json::Value::from(tiles.len())),
            ("tile_size", serde_json::Value::from(tile_size)),
        ]);
    }

    // 出力バッファ: RGB float 累積 + 重み累積（ブレンド用）
    let npixels = (out_w * out_h) as usize;
    let mut accum_r = vec![0.0f32; npixels];
    let mut accum_g = vec![0.0f32; npixels];
    let mut accum_b = vec![0.0f32; npixels];
    let mut accum_w = vec![0.0f32; npixels];
    let prep_ms = t_prep.elapsed().as_secs_f64() * 1000.0 - alpha_resample_ms;

    // パイプライン: 推論スレッド (メイン) が GPU 推論を回し、タイル出力を
    // blender スレッドへ mpsc で流す。blender が累積バッファに blend_tile する。
    // これにより GPU 推論中に前タイルの blend が走り、GPU アイドルを削減。
    //
    // blend_tile は accum_r/g/b/w を排他的に書くので、スレッドは 1 本で十分。
    // std::thread::scope を使い、accum と timings は `&mut` 借用で受け渡す。
    let (tile_timings, blend_wait_ms): (Vec<TileTiming>, f64) =
        std::thread::scope(|s| -> Result<(Vec<TileTiming>, f64), AiError> {
        // sync_channel(2): 推論と blend の 1 タイル分オーバーラップは保ちつつ、
        // blend が詰まっても TileOutput が 2 個以上積まれないように背圧を掛ける
        // (VRAM が厳しい環境で OOM を避けるための保険)。
        type Msg = (TileRect, TileOutput, f64, f64, InferBreakdown);
        let (tx, rx) = std::sync::mpsc::sync_channel::<Msg>(2);
        let accum_r_ref = &mut accum_r;
        let accum_g_ref = &mut accum_g;
        let accum_b_ref = &mut accum_b;
        let accum_w_ref = &mut accum_w;
        let tiles_len = tiles.len();

        let blender = s.spawn(move || -> Vec<TileTiming> {
            let mut timings: Vec<TileTiming> = Vec::with_capacity(tiles_len);
            while let Ok((tile, tile_out, extract_ms, infer_ms, brk)) = rx.recv() {
                let t_blend = std::time::Instant::now();
                blend_tile(
                    accum_r_ref, accum_g_ref, accum_b_ref, accum_w_ref,
                    out_w, out_h,
                    &tile_out,
                    &tile, scale,
                    in_w, in_h,
                );
                let blend_ms = t_blend.elapsed().as_secs_f64() * 1000.0;
                timings.push(TileTiming {
                    extract_ms,
                    infer_ms,
                    tensor_build_ms: brk.tensor_build_ms,
                    session_run_ms: brk.session_run_ms,
                    tensor_extract_ms: brk.tensor_extract_ms,
                    post_copy_ms: brk.post_copy_ms,
                    blend_ms,
                });
            }
            timings
        });

        // メインスレッド: 推論ループ
        for (tile_idx, tile) in tiles.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                if perf_enabled {
                    crate::perf::event("ai", "upscale_cancel", None, 0, &[
                        ("after_tile", serde_json::Value::from(tile_idx)),
                    ]);
                }
                drop(tx);
                let _ = blender.join();
                return Err(AiError::Cancelled);
            }

            let tile_t0 = std::time::Instant::now();
            let tile_input = extract_tile(&rgb, tile);
            let t_infer_begin = std::time::Instant::now();
            let extract_ms = t_infer_begin.duration_since(tile_t0).as_secs_f64() * 1000.0;
            let (tile_out, breakdown) = match run_tile_inference(runtime, model_kind, tile_input) {
                Ok(out) => out,
                Err(e) => {
                    drop(tx);
                    let _ = blender.join();
                    return Err(e);
                }
            };
            let t_send = std::time::Instant::now();
            let infer_ms = t_send.duration_since(t_infer_begin).as_secs_f64() * 1000.0;

            if tx.send((tile.clone(), tile_out, extract_ms, infer_ms, breakdown)).is_err() {
                let _ = blender.join();
                return Err(AiError::Ort(String::from("blender thread died")));
            }

            if perf_enabled {
                let tile_ms = tile_t0.elapsed().as_secs_f64() * 1000.0;
                crate::perf::event("ai", "upscale_tile", None, 0, &[
                    ("tile", serde_json::Value::from(tile_idx)),
                    ("ms", serde_json::Value::from(tile_ms)),
                ]);
            }

            if (tile_idx + 1) % 10 == 0 {
                crate::logger::log(format!(
                    "[AI] Upscale progress: {}/{} tiles",
                    tile_idx + 1, tiles.len()
                ));
            }
        }

        // 全タイル送信完了 → blender の残作業を待つ (blend_wait_ms)
        let t_wait_begin = std::time::Instant::now();
        drop(tx);
        let timings = blender.join().map_err(|_| {
            AiError::Ort(String::from("blender thread panicked"))
        })?;
        let blend_wait_ms = t_wait_begin.elapsed().as_secs_f64() * 1000.0;
        Ok((timings, blend_wait_ms))
    })?;

    crate::logger::log(format!(
        "[AI] Upscale complete: {} tiles, {}x scale",
        tiles.len(), scale
    ));
    if perf_enabled {
        let total_ms = t_upscale.elapsed().as_secs_f64() * 1000.0;
        crate::perf::event("ai", "upscale_end", None, 0, &[
            ("model", serde_json::Value::from(format!("{:?}", model_kind))),
            ("tiles", serde_json::Value::from(tiles.len())),
            ("out_w", serde_json::Value::from(out_w)),
            ("out_h", serde_json::Value::from(out_h)),
            ("total_ms", serde_json::Value::from(total_ms)),
        ]);
    }

    // 累積バッファを正規化して RGBA ColorImage に変換
    let t_finalize = std::time::Instant::now();
    let pixels: Vec<egui::Color32> = (0..npixels)
        .map(|i| {
            let w = accum_w[i].max(1e-6);
            let r = (accum_r[i] / w).clamp(0.0, 255.0) as u8;
            let g = (accum_g[i] / w).clamp(0.0, 255.0) as u8;
            let b = (accum_b[i] / w).clamp(0.0, 255.0) as u8;
            let a = alpha_resized.as_ref().map_or(255, |v| v[i]);
            egui::Color32::from_rgba_unmultiplied(r, g, b, a)
        })
        .collect();
    let finalize_ms = t_finalize.elapsed().as_secs_f64() * 1000.0;

    let color_image = egui::ColorImage::new([out_w as usize, out_h as usize], pixels);
    let total_ms = t_all.elapsed().as_secs_f64() * 1000.0;

    let timings = UpscaleTimings {
        total_ms,
        prep_ms,
        alpha_resample_ms,
        finalize_ms,
        blend_wait_ms,
        tiles: tile_timings,
        tile_size,
        scale,
        in_w,
        in_h,
    };

    Ok((color_image, timings))
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

/// `run_tile_inference` 内部の CPU/GPU 時間内訳 (ms)。
#[derive(Debug, Clone, Copy, Default)]
struct InferBreakdown {
    tensor_build_ms: f64,
    session_run_ms: f64,
    tensor_extract_ms: f64,
    post_copy_ms: f64,
}

/// 1 タイルの推論を実行する。
fn run_tile_inference(
    runtime: &AiRuntime,
    model_kind: ModelKind,
    input: ndarray::Array4<f32>,
) -> Result<(TileOutput, InferBreakdown), AiError> {
    let t0 = std::time::Instant::now();
    let input_tensor = ort::value::Tensor::from_array(input)
        .map_err(|e| AiError::Ort(format!("Tensor: {e}")))?;
    let tensor_build_ms = t0.elapsed().as_secs_f64() * 1000.0;

    runtime.with_session(model_kind, |session| {
        let t_run = std::time::Instant::now();
        let outputs = session
            .run(ort::inputs![input_tensor])
            .map_err(|e| AiError::Ort(format!("run ({model_kind:?}): {e}")))?;
        let session_run_ms = t_run.elapsed().as_secs_f64() * 1000.0;

        let t_extract = std::time::Instant::now();
        let (shape, raw) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| AiError::Ort(format!("extract ({model_kind:?}): {e}")))?;
        let tensor_extract_ms = t_extract.elapsed().as_secs_f64() * 1000.0;

        let dims: Vec<i64> = shape.iter().copied().collect();
        let (out_ch, actual_out_h, actual_out_w) = if dims.len() >= 4 {
            (dims[1] as usize, dims[2] as usize, dims[3] as usize)
        } else {
            return Err(AiError::Ort(format!("Unexpected output shape: {dims:?}")));
        };

        // NCHW float → RGB float [0, 255] の平面配置で保存。
        // 入出力のインデックス構造は同一 (同じ c * plane_size + y * W + x) なので、
        // 先頭 ch * plane_size 要素を単純スカラ変換するだけで済む。
        // ch < 3 の場合は余りを 0 のままにする (vec! の初期値)。
        let t_copy = std::time::Instant::now();
        let ch = out_ch.min(3);
        let plane_size = actual_out_h * actual_out_w;
        let filled = ch * plane_size;
        let mut data = vec![0.0f32; 3 * plane_size];
        for i in 0..filled {
            let v = raw.get(i).copied().unwrap_or(0.0);
            data[i] = (v * 255.0).clamp(0.0, 255.0);
        }
        let post_copy_ms = t_copy.elapsed().as_secs_f64() * 1000.0;

        Ok((
            TileOutput {
                data,
                width: actual_out_w as u32,
                height: actual_out_h as u32,
            },
            InferBreakdown {
                tensor_build_ms,
                session_run_ms,
                tensor_extract_ms,
                post_copy_ms,
            },
        ))
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
