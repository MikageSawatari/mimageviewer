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

/// `model_tile_size` に渡すべきバックエンド種別を解決する。
///
/// Phase 3 アーキテクチャではメイン runtime は常に DirectML だが、TRT
/// ワーカーへルーティングされるモデルでは TRT 用 tile size (256) を使うべき。
/// `active_backend().effective` だけ見ると DirectML になってしまい、TRT 用
/// engine cache (256) が使えず再コンパイルが走るので、ここで dispatch を
/// 反映する。
fn effective_backend_for_tile(runtime: &AiRuntime, kind: ModelKind) -> super::AiBackend {
    if runtime.should_route_to_worker(kind) {
        super::AiBackend::TensorRt
    } else {
        runtime.active_backend().effective
    }
}

/// モデルごとの最適タイルサイズ (RTX 4090 で実測、`bench_ai --tile-size` 参照)。
///
/// - 固定入力サイズのモデルはそのサイズでしか動かない (例: RealPLKSR 256)。
/// - それ以外のモデルは本来任意サイズで動くが、ONNX ランタイム/GPU の実行効率が
///   特定サイズでピークになる。
/// - DirectML: per-call overhead が super-linear に増えるため 192 が最適 (軽量
///   モデル UpscaleRealEsrGeneralV3 のみ 512 で 18% 高速)。
/// - TensorRT: 256 が最適 (192/384/512 をスイープして比較。256 比 384 は +20-47%、
///   512 は +11-44% 遅い)。原因は L2 キャッシュ飽和と TRT のカーネル最適化が
///   power-of-2 単位 (256) でピークになる特性。per-pixel 時間で見ても 256 が
///   最小なので、画像が大きくなっても 256 優位は維持される (4K/8K でも検証済み)。
fn model_tile_size(kind: ModelKind, backend: super::AiBackend) -> u32 {
    match (kind, backend) {
        // RealPLKSR は 256x256 固定入力 (バックエンドに依らない)
        (ModelKind::DenoiseRealplksr, _) => 256,
        // 軽量モデル: 大タイルで GPU カーネル起動オーバーヘッドを削減
        (ModelKind::UpscaleRealEsrGeneralV3, _) => 512,
        // TensorRT は大タイルが効く (per-tile overhead 削減 + GPU 飽和)
        (_, super::AiBackend::TensorRt) => 256,
        // DirectML / CPU は 192 (DirectML 256 はかえって遅い)
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

/// AI 処理対象サイズの上限 (長辺 / 短辺で指定)。
///
/// 「長辺 < `long_edge_px` かつ 短辺 < `short_edge_px`」の画像のみ AI 処理対象。
/// 旧来の単一しきい値 `N` (`width < N && height < N`) は `N x N` と等価
/// (`max(w,h) < N` と同値) なので、`square(N)` で読み替えられる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AiProcessSizeLimit {
    pub long_edge_px: u32,
    pub short_edge_px: u32,
}

impl AiProcessSizeLimit {
    /// 旧単一しきい値 `N` を `N x N` として読み替える。
    pub fn square(px: u32) -> Self {
        Self {
            long_edge_px: px,
            short_edge_px: px,
        }
    }

    /// UI 表示用の「長辺 x 短辺」ラベル (例: `4096 x 2048`)。
    pub fn label(&self) -> String {
        format!("{} x {}", self.long_edge_px, self.short_edge_px)
    }
}

/// 画像の長辺・短辺がどちらもサイズ上限未満か (上限以上ならスキップ)。
/// 縦横どちら向きでも同じ判定になるよう、画像側も上限側も長辺/短辺へ正規化して比べる。
pub fn should_process_rect(width: u32, height: u32, limit: AiProcessSizeLimit) -> bool {
    let long = width.max(height);
    let short = width.min(height);
    let limit_long = limit.long_edge_px.max(limit.short_edge_px);
    let limit_short = limit.long_edge_px.min(limit.short_edge_px);
    long < limit_long && short < limit_short
}

/// PDF ラスターページの content_type (native 寸法) 判明後に、native 解像度で再レンダして
/// final AI を起動すべきか判定する純関数 (GitHub issue #1)。
///
/// 初回フルスクリーンは content_type 未解析のため 4096px 固定でレンダされ、final AI は
/// レンダ後のピクセルサイズで判定するため、サイズ上限内のラスター PDF でも初回表示では
/// AI がスキップされる。判定には **fs_cache のレンダ後サイズではなく native 寸法 (`native_w`,
/// `native_h`)** を渡すこと (レンダ後サイズを渡すと常に false になり意味がない)。
///
/// AI が実効適用される場合 (= 設定 ON かつ native 寸法がサイズ上限内) のみ true。
/// 非 AI 利用時に range 内ラスターを native へ落とすと表示解像度が下がるため、
/// あえて設定 ON を AND 条件にしている。
pub fn pdf_should_native_rerender_for_ai(
    native_w: u32,
    native_h: u32,
    upscale_on: bool,
    upscale_limit: AiProcessSizeLimit,
    denoise_on: bool,
    denoise_limit: AiProcessSizeLimit,
) -> bool {
    let upscale_will_apply = upscale_on && should_process_rect(native_w, native_h, upscale_limit);
    let denoise_will_apply = denoise_on && should_process_rect(native_w, native_h, denoise_limit);
    upscale_will_apply || denoise_will_apply
}

/// 1 タイルを推論してスケール倍率を検出する（結果をキャッシュ）。
fn detect_scale_factor(runtime: &AiRuntime, model_kind: ModelKind) -> Result<u32, AiError> {
    // キャッシュ済みならそのまま返す
    if let Some(&scale) = SCALE_CACHE.lock().unwrap().get(&model_kind) {
        return Ok(scale);
    }

    let test_size =
        model_tile_size(model_kind, effective_backend_for_tile(runtime, model_kind)) as usize;
    let dummy = ndarray::Array4::<f32>::zeros((1, 3, test_size, test_size));
    let tensor =
        ort::value::Tensor::from_array(dummy).map_err(|e| AiError::Ort(format!("Tensor: {e}")))?;

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

    let tile_size = tile_size_override.unwrap_or_else(|| {
        model_tile_size(model_kind, effective_backend_for_tile(runtime, model_kind))
    });

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
                &alpha_img,
                out_w,
                out_h,
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
        crate::perf::event(
            "ai",
            "upscale_begin",
            None,
            0,
            &[
                (
                    "model",
                    serde_json::Value::from(format!("{:?}", model_kind)),
                ),
                ("in_w", serde_json::Value::from(in_w)),
                ("in_h", serde_json::Value::from(in_h)),
                ("scale", serde_json::Value::from(scale)),
                ("tiles", serde_json::Value::from(tiles.len())),
                ("tile_size", serde_json::Value::from(tile_size)),
            ],
        );
    }

    // 出力バッファ: RGB float 累積 + 重み累積（ブレンド用）
    //
    // ⚠ メモリ過大ガードは**意図的に持たない** (2026-06-10 ユーザー判断、Codex P2 回答):
    // サイズ上限の最悪ケース (4095x4095 → 4x = 268MP) で累積 4 面 ≈ 4.3 GB、最終
    // ColorImage と合わせたピークは ≈ 5.4 GB に達するが、空きメモリ量で AI 適用可否を
    // 変えると挙動が予測できなくなるため、判定はサイズ上限のみで決定的にする。
    // 高負荷上限を明示的に選んだ環境で実メモリ不足ならクラッシュ (alloc abort) で良い。
    // `try_reserve` での graceful fail も非決定性を生むため入れない。
    // 詳細: docs/ai-processing-size-threshold-plan.md「メモリ目安」。
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
                        accum_r_ref,
                        accum_g_ref,
                        accum_b_ref,
                        accum_w_ref,
                        out_w,
                        out_h,
                        &tile_out,
                        &tile,
                        scale,
                        in_w,
                        in_h,
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
                        crate::perf::event(
                            "ai",
                            "upscale_cancel",
                            None,
                            0,
                            &[("after_tile", serde_json::Value::from(tile_idx))],
                        );
                    }
                    drop(tx);
                    let _ = blender.join();
                    return Err(AiError::Cancelled);
                }

                let tile_t0 = std::time::Instant::now();
                // 不完全 tile (img < tile_size のケース) でも tile_size 固定 shape の
                // テンソルを作る (= TRT engine の固定 shape プロファイルに合わせる)。
                let tile_input = extract_tile(&rgb, tile, tile_size);
                // 出力 crop 寸法: 実 tile サイズ × scale。pad 領域は捨てる。
                let crop_w = (tile.w * scale) as usize;
                let crop_h = (tile.h * scale) as usize;
                let t_infer_begin = std::time::Instant::now();
                let extract_ms = t_infer_begin.duration_since(tile_t0).as_secs_f64() * 1000.0;
                let (tile_out, breakdown) =
                    match run_tile_inference(runtime, model_kind, tile_input, crop_w, crop_h) {
                        Ok(out) => out,
                        Err(e) => {
                            drop(tx);
                            let _ = blender.join();
                            return Err(e);
                        }
                    };
                let t_send = std::time::Instant::now();
                let infer_ms = t_send.duration_since(t_infer_begin).as_secs_f64() * 1000.0;

                if tx
                    .send((tile.clone(), tile_out, extract_ms, infer_ms, breakdown))
                    .is_err()
                {
                    let _ = blender.join();
                    return Err(AiError::Ort(String::from("blender thread died")));
                }

                if perf_enabled {
                    let tile_ms = tile_t0.elapsed().as_secs_f64() * 1000.0;
                    crate::perf::event(
                        "ai",
                        "upscale_tile",
                        None,
                        0,
                        &[
                            ("tile", serde_json::Value::from(tile_idx)),
                            ("ms", serde_json::Value::from(tile_ms)),
                        ],
                    );
                }

                if (tile_idx + 1) % 10 == 0 {
                    crate::logger::log(format!(
                        "[AI] Upscale progress: {}/{} tiles",
                        tile_idx + 1,
                        tiles.len()
                    ));
                }
            }

            // 全タイル送信完了 → blender の残作業を待つ (blend_wait_ms)
            let t_wait_begin = std::time::Instant::now();
            drop(tx);
            let timings = blender
                .join()
                .map_err(|_| AiError::Ort(String::from("blender thread panicked")))?;
            let blend_wait_ms = t_wait_begin.elapsed().as_secs_f64() * 1000.0;
            Ok((timings, blend_wait_ms))
        })?;

    crate::logger::log(format!(
        "[AI] Upscale complete: {} tiles, {}x scale",
        tiles.len(),
        scale
    ));
    if perf_enabled {
        let total_ms = t_upscale.elapsed().as_secs_f64() * 1000.0;
        crate::perf::event(
            "ai",
            "upscale_end",
            None,
            0,
            &[
                (
                    "model",
                    serde_json::Value::from(format!("{:?}", model_kind)),
                ),
                ("tiles", serde_json::Value::from(tiles.len())),
                ("out_w", serde_json::Value::from(out_w)),
                ("out_h", serde_json::Value::from(out_h)),
                ("total_ms", serde_json::Value::from(total_ms)),
            ],
        );
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
        if th == 0 {
            break;
        }

        let mut x = 0u32;
        loop {
            let tx = x;
            let tw = tile_size.min(img_w.saturating_sub(tx));
            if tw == 0 {
                break;
            }
            tiles.push(TileRect {
                x: tx,
                y: ty,
                w: tw,
                h: th,
            });

            if tx + tw >= img_w {
                break;
            }
            x += step;
            if x + tile_size > img_w {
                x = img_w.saturating_sub(tile_size);
            }
        }

        if ty + th >= img_h {
            break;
        }
        y += step;
        if y + tile_size > img_h {
            y = img_h.saturating_sub(tile_size);
        }
    }

    tiles
}

/// `extract_tile` 第 3 引数: `tile_size` (= 物理 tile サイズ、tile.w/.h より大きいことがある)。
/// 画像端の不完全 tile (= img_w/h < tile_size のとき発生) でも、テンソルは常に
/// `tile_size × tile_size` の形状で生成し、不足領域はゼロ詰めする。
///
/// なぜ必要か (Apr 29 ユーザー報告):
///   pre-built TRT engine は warmup shape (例: 512×512) で固定されている。
///   640×480 画像を tile_size=512 で処理すると th=480 の不完全 tile が出来、
///   shape (1,3,480,512) でセッション実行 → engine と不一致 → TRT が再 build を
///   試みるが pack に builder_resource が無いため `TensorRT EP failed to create
///   engine from network` エラー。常に shape を tile_size に揃えれば回避できる。
///
/// 出力テンソルの不要領域 (tile.w..tile_size の右端、tile.h..tile_size の下端) は
/// 後段の `build_tile_output` で `tile.w * scale × tile.h * scale` に crop して
/// 捨てるので、最終画像には混入しない。pad は zero (= 黒)。
fn extract_tile(rgb: &image::RgbImage, tile: &TileRect, tile_size: u32) -> ndarray::Array4<f32> {
    let canvas = tile_size as usize;
    let tw = tile.w as usize;
    let th = tile.h as usize;
    debug_assert!(tw <= canvas && th <= canvas, "tile larger than tile_size");
    let mut tensor = ndarray::Array4::<f32>::zeros((1, 3, canvas, canvas));

    // 実データ領域 (左上 [0..th, 0..tw]) を埋める。残りは zeros のまま。
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

/// 出力 raw f32 (model 出力、値域 [0,1]) と shape から TileOutput を構築する。
/// 戻り値は (TileOutput, post_copy_ms)。
///
/// NCHW 形式の input で、ch < 3 のときは残り channel を 0 のままにする。
///
/// `crop_w` / `crop_h` は出力テンソル (`raw`) を上から **crop_w × crop_h** に切り出す
/// (= extract_tile が tile_size 固定で zero-pad した不要領域を捨てる)。
/// 通常は `dims[3]` / `dims[2]` をそのまま使うが、入力を pad した場合は呼び出し側で
/// `tile.w * scale` / `tile.h * scale` を渡す。
fn build_tile_output(
    raw: &[f32],
    dims: &[i64],
    crop_w: usize,
    crop_h: usize,
) -> Result<(TileOutput, f64), AiError> {
    let (out_ch, actual_out_h, actual_out_w) = if dims.len() >= 4 {
        (dims[1] as usize, dims[2] as usize, dims[3] as usize)
    } else {
        return Err(AiError::Ort(format!("Unexpected output shape: {dims:?}")));
    };
    if crop_w > actual_out_w || crop_h > actual_out_h {
        return Err(AiError::Ort(format!(
            "crop {}x{} larger than output {}x{}",
            crop_w, crop_h, actual_out_w, actual_out_h
        )));
    }

    // NCHW float → RGB float [0, 255] の平面配置で保存。
    // 各 channel の左上 crop_w × crop_h を切り出す (= 残りはゼロ pad の garbage)。
    let t_copy = std::time::Instant::now();
    let ch = out_ch.min(3);
    let in_plane_size = actual_out_h * actual_out_w;
    let out_plane_size = crop_h * crop_w;
    let mut data = vec![0.0f32; 3 * out_plane_size];
    for c in 0..ch {
        for y in 0..crop_h {
            for x in 0..crop_w {
                let in_idx = c * in_plane_size + y * actual_out_w + x;
                let out_idx = c * out_plane_size + y * crop_w + x;
                let v = raw.get(in_idx).copied().unwrap_or(0.0);
                data[out_idx] = (v * 255.0).clamp(0.0, 255.0);
            }
        }
    }
    let post_copy_ms = t_copy.elapsed().as_secs_f64() * 1000.0;

    Ok((
        TileOutput {
            data,
            width: crop_w as u32,
            height: crop_h as u32,
        },
        post_copy_ms,
    ))
}

/// 1 タイルの推論を実行する。
///
/// `should_route_to_worker(kind) == true` ならば TRT ワーカープロセスにルーティング、
/// そうでなければ従来通り `with_session` でローカル DirectML 推論。
/// どちらの経路でも (TileOutput, InferBreakdown) を返すので呼び出し側は同じ。
fn run_tile_inference(
    runtime: &AiRuntime,
    model_kind: ModelKind,
    input: ndarray::Array4<f32>,
    crop_w: usize,
    crop_h: usize,
) -> Result<(TileOutput, InferBreakdown), AiError> {
    let worker_route = runtime.should_route_to_worker(model_kind);
    if worker_route {
        // TRT ワーカー経路: tensor_build / extract / shm 転送はワーカー内で完結
        let t_run = std::time::Instant::now();
        match runtime.infer_via_worker(model_kind, &input) {
            Ok((shape, raw)) => {
                let session_run_ms = t_run.elapsed().as_secs_f64() * 1000.0;
                let (output, post_copy_ms) = build_tile_output(&raw, &shape, crop_w, crop_h)?;
                return Ok((
                    output,
                    InferBreakdown {
                        tensor_build_ms: 0.0,   // ワーカー内部、計測されない
                        session_run_ms,         // ワーカー往復時間 (内部 GPU + IPC overhead)
                        tensor_extract_ms: 0.0, // ワーカー内部
                        post_copy_ms,
                    },
                ));
            }
            Err(e) => {
                // T51 (Codex P2 / 2026-05-16): worker 由来エラーで in-flight upscale 全体を
                // 中断するのではなく、このタイルだけ DirectML フォールバックで再 inference する。
                // worker mark_dead 自体は trt_worker_pool の `classify_io_error` が担当 (T48)。
                // mark_dead 後の次回 `should_route_to_worker` は false を返すので、以降のタイルは
                // 全てローカル DirectML 経由になる。
                crate::logger::log(format!(
                    "[AI upscale] TRT worker failed for tile ({model_kind:?}): {e} — DirectML フォールバックを試行"
                ));
                // fall through to the local DirectML branch below
            }
        }
    }
    {
        // ローカル DirectML 経路 (既存)
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
            let (output, post_copy_ms) = build_tile_output(raw, &dims, crop_w, crop_h)?;
            Ok((
                output,
                InferBreakdown {
                    tensor_build_ms,
                    session_run_ms,
                    tensor_extract_ms,
                    post_copy_ms,
                },
            ))
        })
    }
}

/// タイル出力を重み付きで累積バッファに加算する（距離ベース線形ブレンド）。
///
/// 各ピクセルの重みは「タイルの各辺からの距離」の最小値に基づく。
/// 辺に近いほど重みが小さく、中心ほど大きい。
/// 画像の端に接する辺は常に高重み（ランプなし）。
/// 隣接タイルのオーバーラップ量が不均一でも正しく正規化される。
fn blend_tile(
    accum_r: &mut [f32],
    accum_g: &mut [f32],
    accum_b: &mut [f32],
    accum_w: &mut [f32],
    out_w: u32,
    out_h: u32,
    tile_out: &TileOutput,
    tile: &TileRect,
    scale: u32,
    img_w: u32,
    img_h: u32,
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
        if dy >= out_h as usize {
            break;
        }

        // Y方向の辺からの距離
        let dist_top = if is_first_y { ramp } else { sy as f32 };
        let dist_bot = if is_last_y {
            ramp
        } else {
            (th - 1 - sy) as f32
        };
        let wy = (dist_top.min(dist_bot) / ramp).clamp(1e-4, 1.0);

        for sx in 0..tw {
            let dx = dst_x0 + sx;
            if dx >= out_w as usize {
                break;
            }

            // X方向の辺からの距離
            let dist_left = if is_first_x { ramp } else { sx as f32 };
            let dist_right = if is_last_x {
                ramp
            } else {
                (tw - 1 - sx) as f32
            };
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧単一しきい値 `N` (`w < N && h < N`) と `square(N)` が同じ判定になる。
    #[test]
    fn square_limit_matches_legacy_threshold() {
        let limit = AiProcessSizeLimit::square(2048);
        // 旧: w < 2048 && h < 2048
        assert!(should_process_rect(2047, 2047, limit));
        assert!(!should_process_rect(2048, 2047, limit));
        assert!(!should_process_rect(2047, 2048, limit));
        assert!(!should_process_rect(4000, 100, limit));
    }

    /// 2720x1920 は 2048x2048 では対象外、4096x2048 では対象になる (計画書のテスト観点)。
    #[test]
    fn wide_image_allowed_by_long_edge_limit() {
        assert!(!should_process_rect(
            2720,
            1920,
            AiProcessSizeLimit::square(2048)
        ));
        let limit = AiProcessSizeLimit {
            long_edge_px: 4096,
            short_edge_px: 2048,
        };
        assert!(should_process_rect(2720, 1920, limit));
        // 短辺が上限以上ならスキップ (3000x3000 は長辺 4096 未満でも短辺 2048 以上)
        assert!(!should_process_rect(3000, 3000, limit));
    }

    /// 縦横が逆でも同じ判定になる。
    #[test]
    fn orientation_independent() {
        let limit = AiProcessSizeLimit {
            long_edge_px: 4096,
            short_edge_px: 2048,
        };
        assert_eq!(
            should_process_rect(2720, 1920, limit),
            should_process_rect(1920, 2720, limit)
        );
        assert_eq!(
            should_process_rect(4096, 2047, limit),
            should_process_rect(2047, 4096, limit)
        );
    }

    /// 上限側の長辺/短辺が入れ替わっていても正規化されて同じ判定になる。
    #[test]
    fn limit_fields_normalized() {
        let swapped = AiProcessSizeLimit {
            long_edge_px: 2048,
            short_edge_px: 4096,
        };
        assert!(should_process_rect(2720, 1920, swapped));
        assert!(!should_process_rect(3000, 3000, swapped));
    }

    /// 境界値: 長辺・短辺ともちょうど上限と同じならスキップ (`<` 判定)。
    #[test]
    fn boundary_is_exclusive() {
        let limit = AiProcessSizeLimit {
            long_edge_px: 4096,
            short_edge_px: 2048,
        };
        assert!(should_process_rect(4095, 2047, limit));
        assert!(!should_process_rect(4096, 2047, limit));
        assert!(!should_process_rect(4095, 2048, limit));
    }

    /// GitHub issue #1: native 寸法がサイズ上限内 + AI 設定 ON の PDF ラスターは
    /// native 解像度で再レンダして AI を起動する。
    #[test]
    fn pdf_native_rerender_when_ai_applies() {
        let limit = AiProcessSizeLimit::square(2048);
        // issue 添付の native 寸法 824x1200 (両軸 2048 未満) + アップスケール ON → 再レンダ。
        assert!(pdf_should_native_rerender_for_ai(
            824, 1200, true, limit, false, limit
        ));
        // デノイズだけ ON でも range 内なら再レンダ。
        assert!(pdf_should_native_rerender_for_ai(
            824, 1200, false, limit, true, limit
        ));
    }

    /// AI 設定が両方 OFF なら、range 内ラスターでも再レンダしない
    /// (非 AI ユーザーの 4096px 表示解像度を保つ)。
    #[test]
    fn pdf_no_native_rerender_when_ai_off() {
        let limit = AiProcessSizeLimit::square(2048);
        assert!(!pdf_should_native_rerender_for_ai(
            824, 1200, false, limit, false, limit
        ));
    }

    /// native 寸法がサイズ上限超なら、AI 設定 ON でも再レンダしない
    /// (AI はどのみちスキップされるので 4096px 固定のまま)。
    #[test]
    fn pdf_no_native_rerender_when_native_over_limit() {
        let limit = AiProcessSizeLimit::square(2048);
        // 3000x4000 は短辺 3000 ≥ 2048 で range 外。
        assert!(!pdf_should_native_rerender_for_ai(
            3000, 4000, true, limit, true, limit
        ));
    }

    /// アップスケールとデノイズで別々のサイズ上限を持つケース:
    /// デノイズ上限だけ広ければデノイズ起動で再レンダする。
    #[test]
    fn pdf_native_rerender_respects_separate_limits() {
        let upscale_limit = AiProcessSizeLimit::square(1024);
        let denoise_limit = AiProcessSizeLimit::square(2048);
        // 1500x1500: upscale 上限 1024 では range 外、denoise 上限 2048 では range 内。
        assert!(pdf_should_native_rerender_for_ai(
            1500,
            1500,
            true,
            upscale_limit,
            true,
            denoise_limit
        ));
        // upscale だけ ON なら range 外なので再レンダしない。
        assert!(!pdf_should_native_rerender_for_ai(
            1500,
            1500,
            true,
            upscale_limit,
            false,
            denoise_limit
        ));
    }
}
