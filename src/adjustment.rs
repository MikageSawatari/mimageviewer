//! 画像補正パラメータ定義と CPU ベースの画像処理パイプライン。
//!
//! スライダー操作中は `apply_adjustments_preview()` で低解像度プレビューを生成し、
//! 操作終了時に `apply_adjustments()` でフルサイズ画像を処理する。
//!
//! 処理順序:
//!   1. 自動補正モード解決 (ヒストグラム分析 → パラメータ上書き)
//!   2. レベル補正 (黒点・白点・中間点)
//!   3. 明るさ + コントラスト
//!   4. ガンマ
//!   5. 彩度
//!   6. 色温度
//!   7. シャープネス (Unsharp Mask)

use serde::{Deserialize, Serialize};

/// 自動補正モード。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutoMode {
    /// ヒストグラム 0.5%/99.5% パーセンタイルで黒点・白点を自動設定
    AutoLevel,
    /// 上下 1% クリップ + ストレッチ
    AutoContrast,
    /// グレースケール + 紙色→白 + 黒強化 + S 字カーブ
    MangaCleanup,
    /// 自動レベル + シャープネス 30
    ScanFix,
}

/// 画像補正パラメータ。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdjustParams {
    pub brightness: f32,     // -100..+100
    pub contrast: f32,       // -100..+100
    pub gamma: f32,          // 0.2..5.0
    pub saturation: f32,     // -100..+100
    pub temperature: f32,    // -100..+100
    pub sharpness: f32,      // 0..100
    pub sharpen_radius: u8,  // 1..5
    pub black_point: u8,     // 0..255
    pub white_point: u8,     // 0..255
    pub midtone: f32,        // 0.1..10.0
    pub auto_mode: Option<AutoMode>,
    /// AI アップスケールモデル。None = off, Some("auto") = 自動判別, Some("realcugan_4x") = 指定
    pub upscale_model: Option<String>,
}

impl Default for AdjustParams {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 0.0,
            gamma: 1.0,
            saturation: 0.0,
            temperature: 0.0,
            sharpness: 0.0,
            sharpen_radius: 1,
            black_point: 0,
            white_point: 255,
            midtone: 1.0,
            auto_mode: None,
            upscale_model: None,
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
            && self.sharpness == 0.0
            && self.black_point == 0
            && self.white_point == 255
            && self.midtone == 1.0
            && self.auto_mode.is_none()
    }

    /// アップスケールが設定されているか。
    pub fn needs_upscale(&self) -> bool {
        self.upscale_model.is_some()
    }

    /// アップスケールモデル種別を解決する。
    /// "auto" → Some(None), "realcugan_4x" → Some(Some(ModelKind)), None → None
    pub fn upscale_model_kind(&self) -> Option<Option<crate::ai::ModelKind>> {
        match self.upscale_model.as_deref() {
            None => None,
            Some("auto") => Some(None),
            Some(s) => Some(Some(crate::ai::ModelKind::from_str(s)?)),
        }
    }
}

/// フォルダ/ZIP/PDF 単位の 4 プリセット。
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
        self.presets.iter().all(|p| p.is_identity() && !p.needs_upscale())
    }
}

// ── 画像処理 ────────────────────────────────────────────────────

/// フルサイズ画像に全補正を適用する（シャープネス含む）。
pub fn apply_adjustments(src: &egui::ColorImage, params: &AdjustParams) -> egui::ColorImage {
    let [w, h] = src.size;
    let effective = resolve_auto_mode(src, params);
    let mut buf = pixels_to_f32(src);

    apply_color_pipeline(&mut buf, &effective);

    if effective.sharpness > 0.0 {
        apply_sharpen(&mut buf, w, h, effective.sharpness, effective.sharpen_radius);
    }

    f32_to_image(buf, w, h)
}

/// シャープネスを除く色調補正のみを適用する（高速、同期処理向け）。
/// poll_prefetch / poll_ai_upscale の中で即座に呼べるよう軽量に保つ。
pub fn apply_adjustments_fast(src: &egui::ColorImage, params: &AdjustParams) -> egui::ColorImage {
    let [w, h] = src.size;
    let effective = resolve_auto_mode(src, params);
    let mut buf = pixels_to_f32(src);
    apply_color_pipeline(&mut buf, &effective);
    f32_to_image(buf, w, h)
}

/// シャープネスのみを適用する（重い畳み込み処理、バックグラウンド向け）。
pub fn apply_sharpen_only(src: &egui::ColorImage, params: &AdjustParams) -> egui::ColorImage {
    let [w, h] = src.size;
    let effective = resolve_auto_mode(src, params);
    if effective.sharpness <= 0.0 {
        return src.clone();
    }
    let mut buf = pixels_to_f32(src);
    apply_sharpen(&mut buf, w, h, effective.sharpness, effective.sharpen_radius);
    f32_to_image(buf, w, h)
}

/// シャープネスが必要か（バックグラウンド処理を開始すべきか）。
pub fn needs_sharpen(params: &AdjustParams) -> bool {
    // ScanFix は sharpness < 30 のとき 30 に引き上げるので、auto_mode を考慮
    if params.sharpness > 0.0 {
        return true;
    }
    matches!(params.auto_mode, Some(AutoMode::ScanFix))
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

/// シャープネスを除く色調パイプラインを適用する。
fn apply_color_pipeline(buf: &mut Vec<[f32; 3]>, params: &AdjustParams) {
    apply_levels(buf, params.black_point, params.white_point, params.midtone);
    apply_brightness_contrast(buf, params.brightness, params.contrast);
    apply_gamma(buf, params.gamma);
    apply_saturation(buf, params.saturation);
    apply_temperature(buf, params.temperature);
}

/// 低解像度プレビュー用。1/4 に縮小してから補正を適用する。
pub fn apply_adjustments_preview(src: &egui::ColorImage, params: &AdjustParams) -> egui::ColorImage {
    let [w, h] = src.size;
    let scale = 4;
    let sw = (w / scale).max(1);
    let sh = (h / scale).max(1);

    // ダウンサンプル（最近傍）
    let mut small_pixels = Vec::with_capacity(sw * sh);
    for sy in 0..sh {
        let y = sy * scale;
        for sx in 0..sw {
            let x = sx * scale;
            small_pixels.push(src.pixels[y * w + x]);
        }
    }
    let small = egui::ColorImage::new([sw, sh], small_pixels);
    apply_adjustments(&small, params)
}

// ── 内部処理関数 ────────────────────────────────────────────────

/// 自動補正モードを解決して実効パラメータを返す。
fn resolve_auto_mode(src: &egui::ColorImage, params: &AdjustParams) -> AdjustParams {
    let mut p = params.clone();
    match p.auto_mode {
        None => {}
        Some(AutoMode::AutoLevel) => {
            let (bp, wp) = compute_auto_levels(src, 0.005);
            p.black_point = bp;
            p.white_point = wp;
        }
        Some(AutoMode::AutoContrast) => {
            let (bp, wp) = compute_auto_levels(src, 0.01);
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
        Some(AutoMode::ScanFix) => {
            let (bp, wp) = compute_auto_levels(src, 0.005);
            p.black_point = bp;
            p.white_point = wp;
            if p.sharpness < 30.0 {
                p.sharpness = 30.0;
            }
        }
    }
    p
}

/// ヒストグラムからパーセンタイルベースの黒点・白点を算出する。
fn compute_auto_levels(src: &egui::ColorImage, clip_ratio: f64) -> (u8, u8) {
    let mut hist = [0u64; 256];
    for c in &src.pixels {
        let lum = (c.r() as u32 * 77 + c.g() as u32 * 150 + c.b() as u32 * 29) >> 8;
        hist[lum.min(255) as usize] += 1;
    }
    let total = src.pixels.len() as f64;
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
fn compute_manga_levels(src: &egui::ColorImage) -> (u8, u8) {
    let mut hist = [0u64; 256];
    for c in &src.pixels {
        let lum = (c.r() as u32 * 77 + c.g() as u32 * 150 + c.b() as u32 * 29) >> 8;
        hist[lum.min(255) as usize] += 1;
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

/// レベル補正: [black_point, white_point] → [0, 255] + 中間点ガンマ。
fn apply_levels(buf: &mut [[f32; 3]], bp: u8, wp: u8, midtone: f32) {
    if bp == 0 && wp == 255 && midtone == 1.0 {
        return;
    }
    let range = (wp as f32 - bp as f32).max(1.0);
    let inv_gamma = 1.0 / midtone;
    for px in buf.iter_mut() {
        for ch in px.iter_mut() {
            let v = (*ch - bp as f32) / range;
            *ch = v.clamp(0.0, 1.0).powf(inv_gamma) * 255.0;
        }
    }
}

/// 明るさ + コントラスト。
fn apply_brightness_contrast(buf: &mut [[f32; 3]], brightness: f32, contrast: f32) {
    if brightness == 0.0 && contrast == 0.0 {
        return;
    }
    // コントラスト係数
    let factor = (259.0 * (contrast + 255.0)) / (255.0 * (259.0 - contrast));
    let bright_add = brightness * 2.55; // -100..+100 → -255..+255
    for px in buf.iter_mut() {
        for ch in px.iter_mut() {
            *ch = factor * (*ch - 128.0) + 128.0 + bright_add;
        }
    }
}

/// ガンマ補正。
fn apply_gamma(buf: &mut [[f32; 3]], gamma: f32) {
    if (gamma - 1.0).abs() < 0.001 {
        return;
    }
    let inv_gamma = 1.0 / gamma;
    for px in buf.iter_mut() {
        for ch in px.iter_mut() {
            *ch = (*ch / 255.0).clamp(0.0, 1.0).powf(inv_gamma) * 255.0;
        }
    }
}

/// 彩度調整。RGB → HSL 変換を使用。
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
            continue; // 無彩色
        }

        // 簡易彩度調整: 各チャンネルと輝度の差を伸縮
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
    let shift = temperature * 0.5; // ±50 の範囲でシフト
    for px in buf.iter_mut() {
        px[0] += shift;       // R
        px[2] -= shift;       // B
    }
}

/// Unsharp Mask によるシャープネス強調。
fn apply_sharpen(buf: &mut [[f32; 3]], w: usize, h: usize, amount: f32, radius: u8) {
    let r = radius.max(1).min(5) as i32;
    let kernel_size = (2 * r + 1) as usize;
    let sigma = r as f32 * 0.5 + 0.5;

    // ガウシアンカーネル生成
    let mut kernel = vec![0.0f32; kernel_size * kernel_size];
    let mut sum = 0.0f32;
    for ky in 0..kernel_size {
        for kx in 0..kernel_size {
            let dx = kx as f32 - r as f32;
            let dy = ky as f32 - r as f32;
            let val = (-(dx * dx + dy * dy) / (2.0 * sigma * sigma)).exp();
            kernel[ky * kernel_size + kx] = val;
            sum += val;
        }
    }
    for v in kernel.iter_mut() {
        *v /= sum;
    }

    // ブラー画像生成
    let mut blurred = buf.to_vec();
    for y in 0..h {
        for x in 0..w {
            let mut sr = 0.0f32;
            let mut sg = 0.0f32;
            let mut sb = 0.0f32;
            for ky in 0..kernel_size {
                for kx in 0..kernel_size {
                    let sx = (x as i32 + kx as i32 - r).clamp(0, w as i32 - 1) as usize;
                    let sy = (y as i32 + ky as i32 - r).clamp(0, h as i32 - 1) as usize;
                    let k = kernel[ky * kernel_size + kx];
                    let [pr, pg, pb] = buf[sy * w + sx];
                    sr += pr * k;
                    sg += pg * k;
                    sb += pb * k;
                }
            }
            blurred[y * w + x] = [sr, sg, sb];
        }
    }

    // Unsharp Mask: original + amount * (original - blurred)
    let factor = amount / 100.0;
    for i in 0..buf.len() {
        buf[i][0] += (buf[i][0] - blurred[i][0]) * factor;
        buf[i][1] += (buf[i][1] - blurred[i][1]) * factor;
        buf[i][2] += (buf[i][2] - blurred[i][2]) * factor;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_params_no_change() {
        let params = AdjustParams::default();
        assert!(params.is_identity());

        // 2x2 テスト画像
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
}
