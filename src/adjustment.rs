//! 画像補正パラメータ定義と CPU ベースの画像処理パイプライン。
//!
//! スライダー操作後はバックグラウンドスレッドで `apply_adjustments_fast()` を実行し、
//! 完了後にメインスレッドでテクスチャに反映する。
//!
//! 処理順序:
//!   1. 自動補正モード解決 (ヒストグラム分析 → パラメータ上書き)
//!   2. レベル補正 (黒点・白点・中間点)
//!   3. 明るさ + コントラスト
//!   4. ガンマ
//!   5. 彩度
//!   6. 色温度

use serde::{Deserialize, Serialize};

/// ヒストグラムサンプリングの最大ピクセル数。
/// これ以上のピクセル数がある場合はランダムサンプリングを行う。
const HISTOGRAM_SAMPLE_LIMIT: usize = 1_000_000;

/// 自動補正モード。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutoMode {
    /// ヒストグラム 0.5%/99.5% パーセンタイルで黒点・白点を自動設定
    Auto,
    /// グレースケール + 紙色→白 + 黒強化 + S 字カーブ
    MangaCleanup,
}

/// 画像補正パラメータ。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdjustParams {
    pub brightness: f32,     // -100..+100
    pub contrast: f32,       // -100..+100
    pub gamma: f32,          // 0.2..5.0
    pub saturation: f32,     // -100..+100
    pub temperature: f32,    // -100..+100
    pub black_point: u8,     // 0..255
    pub white_point: u8,     // 0..255
    pub midtone: f32,        // 0.1..10.0
    pub auto_mode: Option<AutoMode>,
    /// AI アップスケールモデル。None = off, Some("auto") = 自動判別, Some("realcugan_4x") = 指定
    pub upscale_model: Option<String>,
    /// AI デノイズモデル。None = off, Some("denoise_realplksr") = 指定
    #[serde(default)]
    pub denoise_model: Option<String>,
}

impl Default for AdjustParams {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 0.0,
            gamma: 1.0,
            saturation: 0.0,
            temperature: 0.0,
            black_point: 0,
            white_point: 255,
            midtone: 1.0,
            auto_mode: None,
            upscale_model: None,
            denoise_model: None,
        }
    }
}

impl AdjustParams {
    /// すべてのパラメータがデフォルト値 (無補正) か。
    pub fn is_identity(&self) -> bool {
        self.brightness == 0.0
            && self.contrast == 0.0
            && self.gamma == 1.0
            && self.saturation == 0.0
            && self.temperature == 0.0
            && self.black_point == 0
            && self.white_point == 255
            && self.midtone == 1.0
            && self.auto_mode.is_none()
    }

    /// アップスケールが設定されているか。
    pub fn needs_upscale(&self) -> bool {
        self.upscale_model.is_some()
    }

    /// デノイズが設定されているか。
    pub fn needs_denoise(&self) -> bool {
        self.denoise_model.is_some()
    }

    /// アップスケールモデル種別を解決する。
    pub fn upscale_model_kind(&self) -> Option<Option<crate::ai::ModelKind>> {
        match self.upscale_model.as_deref() {
            None => None,
            Some("auto") => Some(None),
            Some(s) => Some(Some(crate::ai::ModelKind::from_str(s)?)),
        }
    }

    /// デノイズモデル種別を解決する。
    pub fn denoise_model_kind(&self) -> Option<crate::ai::ModelKind> {
        self.denoise_model.as_deref().and_then(crate::ai::ModelKind::from_str)
    }
}

/// フォルダ/ZIP/PDF 単位の 4 プリセット (個別プリセット 1-4)。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdjustPresets {
    pub presets: [AdjustParams; 4],
    pub names: [String; 4],
}

impl Default for AdjustPresets {
    fn default() -> Self {
        Self {
            presets: std::array::from_fn(|_| AdjustParams::default()),
            names: [
                "プリセット 1".to_string(),
                "プリセット 2".to_string(),
                "プリセット 3".to_string(),
                "プリセット 4".to_string(),
            ],
        }
    }
}

impl AdjustPresets {
    /// すべてのプリセットがデフォルト値か (DB から削除可能か)。
    pub fn is_all_default(&self) -> bool {
        self.presets.iter().all(|p| p.is_identity() && !p.needs_upscale() && !p.needs_denoise())
    }
}

/// 保存スロット (10 個)。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PresetSlot {
    pub name: String,
    pub params: AdjustParams,
}

/// 保存スロット 10 個のコンテナ。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PresetSlots {
    pub slots: Vec<Option<PresetSlot>>,
}

impl Default for PresetSlots {
    fn default() -> Self {
        Self {
            slots: (0..10).map(|_| None).collect(),
        }
    }
}

// ── 画像処理 ────────────────────────────────────────────────────

/// フルサイズ画像に全補正を適用する。
pub fn apply_adjustments(src: &egui::ColorImage, params: &AdjustParams) -> egui::ColorImage {
    let [w, h] = src.size;
    let effective = resolve_auto_mode(src, params);
    let mut buf = pixels_to_f32(src);

    apply_color_pipeline(&mut buf, &effective);

    f32_to_image(buf, w, h)
}

/// 色調補正を適用する。
/// 可能な場合は u8→u8 LUT + rayon 並列で高速処理する。
/// `num_threads`: 並列度（settings.parallelism.thread_count()）
pub fn apply_adjustments_fast(src: &egui::ColorImage, params: &AdjustParams, num_threads: usize) -> egui::ColorImage {
    let [w, h] = src.size;
    let effective = resolve_auto_mode(src, params);

    // 色温度がゼロなら全パイプラインを u8 LUT で処理可能
    // （色温度は R/B チャンネルに異なる値を加算するため、単一LUTでは不可）
    if effective.temperature == 0.0 {
        return apply_pipeline_u8_lut(src, &effective, num_threads);
    }

    // 色温度ありの場合は従来の f32 パイプライン
    let mut buf = pixels_to_f32(src);
    apply_color_pipeline(&mut buf, &effective);
    f32_to_image(buf, w, h)
}

/// u8→u8 LUT + rayon 並列で全パイプラインを処理する高速パス。
/// levels, gamma, brightness, contrast, saturation を全て 256 エントリの LUT に統合。
/// 色温度がゼロの場合のみ使用可能。
fn apply_pipeline_u8_lut(src: &egui::ColorImage, params: &AdjustParams, num_threads: usize) -> egui::ColorImage {
    let [w, h] = src.size;
    let bp = params.black_point as f32;
    let range = (params.white_point as f32 - bp).max(1.0);
    let inv_midtone = 1.0 / params.midtone;
    let inv_gamma = 1.0 / params.gamma;
    let bc_factor = (259.0 * (params.contrast + 255.0)) / (255.0 * (259.0 - params.contrast));
    let bright_add = params.brightness * 2.55;
    let needs_levels = params.black_point != 0 || params.white_point != 255 || params.midtone != 1.0;
    let needs_gamma = (params.gamma - 1.0).abs() >= 0.001;
    let needs_bc = params.brightness != 0.0 || params.contrast != 0.0;

    // LUT: input u8 → levels → gamma → brightness/contrast → output u8
    let mut lut = [0u8; 256];
    for i in 0..256 {
        let mut v = i as f32;
        if needs_levels {
            v = ((v - bp) / range).clamp(0.0, 1.0).powf(inv_midtone) * 255.0;
        }
        if needs_gamma {
            v = (v / 255.0).clamp(0.0, 1.0).powf(inv_gamma) * 255.0;
        }
        if needs_bc {
            v = bc_factor * (v - 128.0) + 128.0 + bright_add;
        }
        lut[i] = v.clamp(0.0, 255.0) as u8;
    }

    // 彩度処理の設定
    let full_desat = params.saturation == -100.0;
    let has_sat = params.saturation != 0.0;
    let sat_factor = 1.0 + params.saturation / 100.0;

    // ピクセル処理クロージャ（LUT適用 + 彩度）
    let process_pixel = |c: &egui::Color32| -> egui::Color32 {
        let r = lut[c.r() as usize];
        let g = lut[c.g() as usize];
        let b = lut[c.b() as usize];

        if full_desat {
            let lum = ((r as u32 * 77 + g as u32 * 150 + b as u32 * 29) >> 8) as u8;
            egui::Color32::from_rgb(lum, lum, lum)
        } else if has_sat {
            let (r01, g01, b01) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
            let max = r01.max(g01).max(b01);
            let min = r01.min(g01).min(b01);
            let lum = (max + min) * 0.5;
            if (max - min).abs() < 1e-6 {
                egui::Color32::from_rgb(r, g, b)
            } else {
                let nr = ((lum + (r01 - lum) * sat_factor) * 255.0).clamp(0.0, 255.0) as u8;
                let ng = ((lum + (g01 - lum) * sat_factor) * 255.0).clamp(0.0, 255.0) as u8;
                let nb = ((lum + (b01 - lum) * sat_factor) * 255.0).clamp(0.0, 255.0) as u8;
                egui::Color32::from_rgb(nr, ng, nb)
            }
        } else {
            egui::Color32::from_rgb(r, g, b)
        }
    };

    // u8 LUT 参照はメモリバウンド処理のため、シングルスレッドが最速。
    // rayon 並列化はスレッドプール生成 + 同期のオーバーヘッドが処理本体を上回る。
    let _ = num_threads; // 将来の拡張用に引数は残す
    let pixels: Vec<egui::Color32> = src.pixels.iter().map(process_pixel).collect();

    egui::ColorImage::new([w, h], pixels)
}

/// ColorImage → f32 バッファ変換。
fn pixels_to_f32(src: &egui::ColorImage) -> Vec<[f32; 3]> {
    src.pixels
        .iter()
        .map(|c| [c.r() as f32, c.g() as f32, c.b() as f32])
        .collect()
}

/// f32 バッファ → ColorImage 変換。
fn f32_to_image(buf: Vec<[f32; 3]>, w: usize, h: usize) -> egui::ColorImage {
    let pixels = buf
        .iter()
        .map(|[r, g, b]| {
            egui::Color32::from_rgb(
                r.clamp(0.0, 255.0) as u8,
                g.clamp(0.0, 255.0) as u8,
                b.clamp(0.0, 255.0) as u8,
            )
        })
        .collect();
    egui::ColorImage::new([w, h], pixels)
}

/// 色調パイプラインを適用する（f32 パス、色温度ありの場合に使用）。
/// levels + gamma を統合 LUT で高速化し、彩度 -100 を専用パスで処理する。
fn apply_color_pipeline(buf: &mut Vec<[f32; 3]>, params: &AdjustParams) {
    // ── levels + gamma を統合 LUT で一括処理 ──
    // levels: [bp, wp] → [0, 255] + midtone ガンマ
    // gamma: powf(1/gamma)
    // 両方とも各チャンネル独立の単調変換なので、256 エントリの LUT に統合できる。
    let needs_levels = params.black_point != 0 || params.white_point != 255 || params.midtone != 1.0;
    let needs_gamma = (params.gamma - 1.0).abs() >= 0.001;
    if needs_levels || needs_gamma {
        let bp = params.black_point as f32;
        let range = (params.white_point as f32 - bp).max(1.0);
        let inv_midtone = 1.0 / params.midtone;
        let inv_gamma = 1.0 / params.gamma;

        // LUT: input 0..255 → levels → gamma → output 0..255
        let mut lut = [0.0_f32; 256];
        for i in 0..256 {
            let mut v = i as f32;
            // levels
            if needs_levels {
                v = ((v - bp) / range).clamp(0.0, 1.0).powf(inv_midtone) * 255.0;
            }
            // gamma
            if needs_gamma {
                v = (v / 255.0).clamp(0.0, 1.0).powf(inv_gamma) * 255.0;
            }
            lut[i] = v;
        }

        for px in buf.iter_mut() {
            for ch in px.iter_mut() {
                let idx = (*ch).clamp(0.0, 255.0) as u8 as usize;
                *ch = lut[idx];
            }
        }
    }

    apply_brightness_contrast(buf, params.brightness, params.contrast);

    // 彩度: -100 の場合は高速グレースケール化
    if params.saturation == -100.0 {
        for px in buf.iter_mut() {
            let lum = px[0] * 0.299 + px[1] * 0.587 + px[2] * 0.114;
            px[0] = lum;
            px[1] = lum;
            px[2] = lum;
        }
    } else if params.saturation != 0.0 {
        apply_saturation(buf, params.saturation);
    }

    apply_temperature(buf, params.temperature);
}

// ── 内部処理関数 ────────────────────────────────────────────────

/// ヒストグラム用のサンプリングインデックスを生成する。
/// 画像ピクセル数が HISTOGRAM_SAMPLE_LIMIT 以下なら None（全数走査）。
/// それ以上なら LCG（線形合同法）で疑似ランダムなインデックス列を生成する。
/// スクリーントーン等の規則的パターンとの干渉を完全に回避する。
fn histogram_sample_indices(pixel_count: usize) -> Option<Vec<usize>> {
    if pixel_count <= HISTOGRAM_SAMPLE_LIMIT {
        return None; // 全数走査
    }
    let sample_count = HISTOGRAM_SAMPLE_LIMIT;
    // LCG パラメータ (Numerical Recipes 推奨値)
    // x_{n+1} = (a * x_n + c) mod m
    let m = pixel_count as u64;
    let a: u64 = 6364136223846793005;
    let c: u64 = 1442695040888963407;
    let mut indices = Vec::with_capacity(sample_count);
    let mut x: u64 = 42; // seed
    for _ in 0..sample_count {
        x = x.wrapping_mul(a).wrapping_add(c);
        indices.push((x % m) as usize);
    }
    Some(indices)
}

/// 自動補正モードを解決して実効パラメータを返す。
fn resolve_auto_mode(src: &egui::ColorImage, params: &AdjustParams) -> AdjustParams {
    let mut p = params.clone();
    match p.auto_mode {
        None => {}
        Some(AutoMode::Auto) => {
            let (bp, wp) = compute_auto_levels(src, 0.005);
            p.black_point = bp;
            p.white_point = wp;
        }
        Some(AutoMode::MangaCleanup) => {
            p.saturation = -100.0; // グレースケール化
            let (bp, wp) = compute_manga_levels(src);
            p.black_point = bp;
            p.white_point = wp;
            p.gamma = 0.85; // 中間調を少し暗めに
            p.contrast = p.contrast.max(15.0); // コントラスト強化
        }
    }
    p
}

/// ヒストグラムからパーセンタイルベースの黒点・白点を算出する。
/// 大きな画像ではランダムサンプリングで高速化する。
fn compute_auto_levels(src: &egui::ColorImage, clip_ratio: f64) -> (u8, u8) {
    let mut hist = [0u64; 256];
    let sample_count;

    if let Some(indices) = histogram_sample_indices(src.pixels.len()) {
        sample_count = indices.len() as u64;
        for &idx in &indices {
            let c = src.pixels[idx];
            let lum = (c.r() as u32 * 77 + c.g() as u32 * 150 + c.b() as u32 * 29) >> 8;
            hist[lum.min(255) as usize] += 1;
        }
    } else {
        sample_count = src.pixels.len() as u64;
        for c in &src.pixels {
            let lum = (c.r() as u32 * 77 + c.g() as u32 * 150 + c.b() as u32 * 29) >> 8;
            hist[lum.min(255) as usize] += 1;
        }
    }

    let total = sample_count as f64;
    let low_threshold = (total * clip_ratio) as u64;
    let high_threshold = (total * (1.0 - clip_ratio)) as u64;

    let mut cum = 0u64;
    let mut bp = 0u8;
    for (i, &count) in hist.iter().enumerate() {
        cum += count;
        if cum >= low_threshold {
            bp = i as u8;
            break;
        }
    }

    cum = 0;
    let mut wp = 255u8;
    for (i, &count) in hist.iter().enumerate() {
        cum += count;
        if cum >= high_threshold {
            wp = i as u8;
            break;
        }
    }

    (bp, wp.max(bp + 1))
}

/// 漫画向けレベル補正。紙色（高輝度ピーク）を白に飛ばし、黒を締める。
/// 大きな画像ではランダムサンプリングで高速化する。
fn compute_manga_levels(src: &egui::ColorImage) -> (u8, u8) {
    let mut hist = [0u64; 256];

    if let Some(indices) = histogram_sample_indices(src.pixels.len()) {
        for &idx in &indices {
            let c = src.pixels[idx];
            let lum = (c.r() as u32 * 77 + c.g() as u32 * 150 + c.b() as u32 * 29) >> 8;
            hist[lum.min(255) as usize] += 1;
        }
    } else {
        for c in &src.pixels {
            let lum = (c.r() as u32 * 77 + c.g() as u32 * 150 + c.b() as u32 * 29) >> 8;
            hist[lum.min(255) as usize] += 1;
        }
    }

    // 高輝度側のピーク（紙の色）を検出
    let mut max_count = 0u64;
    let mut paper_lum = 240u8;
    for i in 180..256 {
        if hist[i] > max_count {
            max_count = hist[i];
            paper_lum = i as u8;
        }
    }
    // 紙色より少し暗い点を白点にする
    let wp = paper_lum.saturating_sub(10);

    // 暗部のピーク（インクの色）を検出
    let mut max_dark = 0u64;
    let mut ink_lum = 20u8;
    for i in 0..80 {
        if hist[i] > max_dark {
            max_dark = hist[i];
            ink_lum = i as u8;
        }
    }
    let bp = ink_lum.saturating_add(5);

    (bp, wp.max(bp + 1))
}

/// 明るさ + コントラスト。
fn apply_brightness_contrast(buf: &mut [[f32; 3]], brightness: f32, contrast: f32) {
    if brightness == 0.0 && contrast == 0.0 {
        return;
    }
    let factor = (259.0 * (contrast + 255.0)) / (255.0 * (259.0 - contrast));
    let bright_add = brightness * 2.55;
    for px in buf.iter_mut() {
        for ch in px.iter_mut() {
            *ch = factor * (*ch - 128.0) + 128.0 + bright_add;
        }
    }
}

/// 彩度調整。
fn apply_saturation(buf: &mut [[f32; 3]], saturation: f32) {
    if saturation == 0.0 {
        return;
    }
    let factor = 1.0 + saturation / 100.0;
    for px in buf.iter_mut() {
        let [r, g, b] = *px;
        let (r01, g01, b01) = (r / 255.0, g / 255.0, b / 255.0);
        let max = r01.max(g01).max(b01);
        let min = r01.min(g01).min(b01);
        let lum = (max + min) * 0.5;

        if (max - min).abs() < 1e-6 {
            continue;
        }

        px[0] = (lum + (r01 - lum) * factor) * 255.0;
        px[1] = (lum + (g01 - lum) * factor) * 255.0;
        px[2] = (lum + (b01 - lum) * factor) * 255.0;
    }
}

/// 色温度シフト。正 = 暖色（R+, B-）、負 = 寒色（R-, B+）。
fn apply_temperature(buf: &mut [[f32; 3]], temperature: f32) {
    if temperature == 0.0 {
        return;
    }
    let shift = temperature * 0.5;
    for px in buf.iter_mut() {
        px[0] += shift;
        px[2] -= shift;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_params_no_change() {
        let params = AdjustParams::default();
        assert!(params.is_identity());

        let pixels = vec![
            egui::Color32::from_rgb(100, 150, 200),
            egui::Color32::from_rgb(50, 50, 50),
            egui::Color32::from_rgb(200, 200, 200),
            egui::Color32::from_rgb(0, 0, 0),
        ];
        let src = egui::ColorImage::new([2, 2], pixels.clone());
        let result = apply_adjustments(&src, &params);
        assert_eq!(result.pixels, pixels);
    }

    #[test]
    fn brightness_increases() {
        let params = AdjustParams {
            brightness: 50.0,
            ..Default::default()
        };
        let pixels = vec![egui::Color32::from_rgb(100, 100, 100)];
        let src = egui::ColorImage::new([1, 1], pixels);
        let result = apply_adjustments(&src, &params);
        assert!(result.pixels[0].r() > 100);
    }

    #[test]
    fn auto_level_sets_points() {
        let mut pixels = Vec::new();
        for _ in 0..100 {
            pixels.push(egui::Color32::from_rgb(50, 50, 50));
        }
        for _ in 0..800 {
            pixels.push(egui::Color32::from_rgb(128, 128, 128));
        }
        for _ in 0..100 {
            pixels.push(egui::Color32::from_rgb(230, 230, 230));
        }
        let src = egui::ColorImage::new([100, 10], pixels);
        let (bp, wp) = compute_auto_levels(&src, 0.05);
        assert!(bp >= 40);
        assert!(wp <= 240);
    }

    #[test]
    fn histogram_sampling_small_image() {
        // 100万ピクセル以下 → None（全数走査）
        assert!(histogram_sample_indices(500_000).is_none());
        assert!(histogram_sample_indices(1_000_000).is_none());
    }

    #[test]
    fn histogram_sampling_large_image() {
        // 100万ピクセル超 → ランダムサンプリング
        let indices = histogram_sample_indices(10_000_000).unwrap();
        assert_eq!(indices.len(), HISTOGRAM_SAMPLE_LIMIT);
        // 全インデックスが範囲内
        assert!(indices.iter().all(|&i| i < 10_000_000));
        // ランダム性: 先頭10個が全て同じ値ではない
        let first = indices[0];
        assert!(indices[1..10].iter().any(|&i| i != first));
    }
}

#[cfg(test)]
mod sampling_quality_tests {
    use super::*;

    #[test]
    fn manga_levels_sampling_matches_full_scan() {
        // 200万ピクセルの漫画風画像を生成（紙色240 + インク色20）
        let mut pixels = Vec::with_capacity(2_000_000);
        for i in 0..2_000_000u32 {
            let lum = if i % 5 == 0 { 20 } else { 240 }; // 20% ink, 80% paper
            pixels.push(egui::Color32::from_rgb(lum, lum, lum));
        }
        let src = egui::ColorImage::new([2000, 1000], pixels.clone());

        // サンプリング版
        let (bp_sampled, wp_sampled) = compute_manga_levels(&src);

        // 全数走査版（サンプリング閾値を超える値で強制全数走査）
        let mut hist_full = [0u64; 256];
        for c in &src.pixels {
            let lum = (c.r() as u32 * 77 + c.g() as u32 * 150 + c.b() as u32 * 29) >> 8;
            hist_full[lum.min(255) as usize] += 1;
        }
        // 全数走査のピーク検出
        let mut max_count = 0u64;
        let mut paper_lum_full = 240u8;
        for i in 180..256 {
            if hist_full[i] > max_count { max_count = hist_full[i]; paper_lum_full = i as u8; }
        }
        let wp_full = paper_lum_full.saturating_sub(10);
        let mut max_dark = 0u64;
        let mut ink_lum_full = 20u8;
        for i in 0..80 {
            if hist_full[i] > max_dark { max_dark = hist_full[i]; ink_lum_full = i as u8; }
        }
        let bp_full = ink_lum_full.saturating_add(5);

        // サンプリング版と全数走査版が近い値であること
        assert!((bp_sampled as i16 - bp_full as i16).unsigned_abs() <= 3,
            "bp: sampled={} full={}", bp_sampled, bp_full);
        assert!((wp_sampled as i16 - wp_full as i16).unsigned_abs() <= 3,
            "wp: sampled={} full={}", wp_sampled, wp_full);
    }
}
