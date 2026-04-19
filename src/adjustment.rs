//! 画像補正パラメータ定義と CPU ベースの画像処理パイプライン。
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
const HISTOGRAM_SAMPLE_LIMIT: usize = 1_000_000;

/// 自動補正モード。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutoMode {
    /// ヒストグラム 0.5%/99.5% パーセンタイルで黒点・白点を自動設定
    Auto,
    /// グレースケール + 紙色→白 + 黒強化 + S 字カーブ
    MangaCleanup,
}

/// ポストフィルタ (レトロ系表示エフェクト)。
///
/// 色調補正の後段で CPU 処理として適用される。`None` = 現状挙動、
/// `Nearest` = CPU 変換はしないが NEAREST サンプラーで拡大する。
/// その他は CRT ブラウン管風 / 減色 / 複合プリセット。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PostFilter {
    /// フィルタ無し (補間あり、LINEAR サンプラー) = デフォルト
    #[default]
    None,
    /// 補間なし (NEAREST サンプラー、ピクセル転送のみ)
    Nearest,
    /// CRT シンプル: スキャンライン + シャドウマスク
    CrtSimple,
    /// CRT フル: CRT Simple + 樽型歪み + bloom
    CrtFull,
    /// CRT アーケード: 太スキャンライン + 高輝度
    CrtArcade,
    /// GameBoy 風 4 階調 (緑系)
    GameBoy,
    /// PC-98 アナログモード相当 (画像ごとに適応的に 16 色を選択)
    Pc98,
    /// ファミコン (NES) 固定 52 色パレット
    Famicom,
    /// MSX2+ SCREEN 8 (256 色固定 GRB 3:3:2)
    Msx2Plus,
    /// メガドライブ (9bit 色空間から 61 色適応)
    MegaDrive,
    /// ゲームギア (12bit 色空間から 32 色適応)
    GameGear,
    /// スーパーファミコン (15bit 色空間から 256 色適応)
    Sfc,
    /// 2 階調 Bayer ディザ
    Dither1bit,
    /// 複合: ファミコン減色 + CRT シンプル (非液晶機種の実機体験風)
    ComboFamicomCrt,
    /// 複合: PC-98 減色 + CRT シンプル
    ComboPc98Crt,
    /// 複合: MSX2+ 減色 + CRT シンプル
    ComboMsx2PlusCrt,
    /// 複合: メガドライブ減色 + CRT シンプル
    ComboMegaDriveCrt,
    /// 複合: スーパーファミコン減色 + CRT シンプル
    ComboSfcCrt,
}

impl PostFilter {
    /// 全プリセットを UI 順で列挙する (減色は色数昇順、複合は機種順)。
    pub const ALL: &'static [Self] = &[
        Self::None, Self::Nearest,
        Self::CrtSimple, Self::CrtFull, Self::CrtArcade,
        Self::Dither1bit, Self::GameBoy,
        Self::Pc98, Self::GameGear, Self::Famicom, Self::MegaDrive,
        Self::Msx2Plus, Self::Sfc,
        Self::ComboFamicomCrt, Self::ComboPc98Crt, Self::ComboMsx2PlusCrt,
        Self::ComboMegaDriveCrt, Self::ComboSfcCrt,
    ];

    /// UI 表示用の日本語ラベル。
    pub fn display_label(self) -> &'static str {
        match self {
            Self::None => "標準（補間あり）",
            Self::Nearest => "ニアレスト（補間なし）",
            Self::CrtSimple => "CRT シンプル（控えめ）",
            Self::CrtFull => "CRT フル（歪み+強グロー）",
            Self::CrtArcade => "CRT アーケード（高コントラスト）",
            Self::GameBoy => "GameBoy（緑4階調）",
            Self::Pc98 => "PC-98（16色・適応）",
            Self::Famicom => "ファミコン（52色・固定）",
            Self::Msx2Plus => "MSX2+（256色・GRB）",
            Self::MegaDrive => "メガドライブ（61色・9bit）",
            Self::GameGear => "ゲームギア（32色・12bit）",
            Self::Sfc => "スーパーファミコン（256色・15bit）",
            Self::Dither1bit => "1bit ディザ",
            Self::ComboFamicomCrt => "CRT × ファミコン",
            Self::ComboPc98Crt => "CRT × PC-98",
            Self::ComboMsx2PlusCrt => "CRT × MSX2+",
            Self::ComboMegaDriveCrt => "CRT × メガドライブ",
            Self::ComboSfcCrt => "CRT × スーパーファミコン",
        }
    }

    /// NEAREST サンプラーでテクスチャを作るべきか。
    /// `Nearest` プリセットのみ NEAREST を使う (ユーザーが「補間なし」を明示的に選んだ場合)。
    /// CRT はアナログ感を出すために LINEAR を使う (縮小時のモアレも防げる)。
    /// 減色系も LINEAR (画面スケールでディザドットを滑らかに表示)。
    pub fn needs_nearest_sampler(self) -> bool {
        matches!(self, Self::Nearest)
    }
}

/// AI アップスケールモデル UI メニューの項目一覧を返す。
///
/// ラジオボタン / キーサイクルの描画・遷移に使う (ラベル, モデルキー) の組。
/// 先頭 2 項目 (なし / 自動) は `ModelKind` に無い仮想エントリ。
/// それ以降は [`crate::ai::ModelKind::upscale_models`] から自動生成して
/// モデル定義側との二重管理を避ける。
pub fn upscale_menu_items() -> Vec<(&'static str, Option<&'static str>)> {
    let mut items: Vec<(&'static str, Option<&'static str>)> = vec![
        ("なし", None),
        ("自動 (画像タイプ判別)", Some("auto")),
    ];
    for kind in crate::ai::ModelKind::upscale_models() {
        items.push((kind.display_label(), Some(kind.as_str())));
    }
    items
}

/// アップスケールモデルキーから表示ラベルを取得する。
pub fn upscale_model_label(key: Option<&str>) -> &'static str {
    upscale_menu_items()
        .into_iter()
        .find(|(_, k)| *k == key)
        .map(|(label, _)| label)
        .unwrap_or("不明")
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
    /// AI アップスケールモデル。None = off, Some("auto") = 自動判別
    pub upscale_model: Option<String>,
    /// AI デノイズモデル。None = off
    #[serde(default)]
    pub denoise_model: Option<String>,
    /// ポストフィルタ (レトロ系表示エフェクト)。デフォルト = None
    #[serde(default)]
    pub post_filter: PostFilter,
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
            post_filter: PostFilter::None,
        }
    }
}

impl AdjustParams {
    /// すべてのパラメータがデフォルト値 (無補正かつ post-filter 無し) か。
    pub fn is_identity(&self) -> bool {
        self.is_color_identity() && self.post_filter == PostFilter::None
    }

    /// 色調パラメータのみ無補正か (post-filter は問わない)。
    /// 消しゴム / 分析モードで post-filter をバイパスするときの早期 return 判定に使う。
    pub fn is_color_identity(&self) -> bool {
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

    pub fn needs_upscale(&self) -> bool { self.upscale_model.is_some() }
    pub fn needs_denoise(&self) -> bool { self.denoise_model.is_some() }

    /// ページ個別設定として保存する価値がないか (= identity かつ AI も使わない)。
    /// true のときは DB から該当行を削除する / 個別設定を作らないで済む。
    pub fn is_removable(&self) -> bool {
        self.is_identity() && !self.needs_upscale() && !self.needs_denoise()
    }

    /// AI 設定 (upscale/denoise) が other と同じか。
    /// 色調パラメータのみ変わった場合に AI キャッシュを保持するための比較。
    pub fn ai_settings_eq(&self, other: &Self) -> bool {
        self.upscale_model == other.upscale_model && self.denoise_model == other.denoise_model
    }

    pub fn upscale_model_kind(&self) -> Option<Option<crate::ai::ModelKind>> {
        match self.upscale_model.as_deref() {
            None => None,
            Some("auto") => Some(None),
            Some(s) => Some(Some(crate::ai::ModelKind::from_str(s)?)),
        }
    }

    pub fn denoise_model_kind(&self) -> Option<crate::ai::ModelKind> {
        self.denoise_model.as_deref().and_then(crate::ai::ModelKind::from_str)
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
    pub slots: [Option<PresetSlot>; 10],
}

impl Default for PresetSlots {
    fn default() -> Self {
        Self { slots: Default::default() }
    }
}

/// スロットインデックス (0-9) → 表示用キーラベル ("1"-"9", "0")
pub fn slot_key_label(slot_idx: usize) -> String {
    if slot_idx == 9 { "0".to_string() } else { (slot_idx + 1).to_string() }
}

// ── 画像処理 ────────────────────────────────────────────────────

/// ピクセルの輝度 (ITU-R BT.601 整数近似)。
#[inline]
fn pixel_lum(c: &egui::Color32) -> u8 {
    ((c.r() as u32 * 77 + c.g() as u32 * 150 + c.b() as u32 * 29) >> 8) as u8
}

/// ピクセルの相対輝度 0..1 (ITU-R BT.601 厳密)。
/// 微細な閾値比較 (bloom, 1bit dither 等) で誤差を出したくないときに使う。
#[inline]
pub fn pixel_lum_f32(c: egui::Color32) -> f32 {
    (c.r() as f32 * 0.299 + c.g() as f32 * 0.587 + c.b() as f32 * 0.114) / 255.0
}

/// フルサイズ画像に全補正を適用する (テスト用)。
pub fn apply_adjustments(src: &egui::ColorImage, params: &AdjustParams) -> egui::ColorImage {
    apply_adjustments_fast(src, params)
}

/// 色調補正を適用する。
/// 可能な場合は u8→u8 LUT で f32 変換を省略し高速処理する。
pub fn apply_adjustments_fast(src: &egui::ColorImage, params: &AdjustParams) -> egui::ColorImage {
    let [w, h] = src.size;
    let effective = resolve_auto_mode(src, params);

    // 色温度ゼロなら u8 LUT 高速パス
    if effective.temperature == 0.0 {
        return apply_pipeline_u8_lut(src, &effective);
    }

    // 色温度ありは f32 パイプライン
    let mut buf = pixels_to_f32(src);
    apply_color_pipeline(&mut buf, &effective);
    f32_to_image(src, buf, w, h)
}

/// levels + gamma + brightness/contrast を統合した u8 LUT を生成する。
fn build_u8_lut(params: &AdjustParams) -> [u8; 256] {
    let bp = params.black_point as f32;
    let range = (params.white_point as f32 - bp).max(1.0);
    let inv_midtone = 1.0 / params.midtone;
    let inv_gamma = 1.0 / params.gamma;
    let bc_factor = (259.0 * (params.contrast + 255.0)) / (255.0 * (259.0 - params.contrast));
    let bright_add = params.brightness * 2.55;
    let needs_levels = params.black_point != 0 || params.white_point != 255 || params.midtone != 1.0;
    let needs_gamma = (params.gamma - 1.0).abs() >= 0.001;
    let needs_bc = params.brightness != 0.0 || params.contrast != 0.0;

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
    lut
}

/// u8→u8 LUT で全パイプラインを処理する高速パス。
fn apply_pipeline_u8_lut(src: &egui::ColorImage, params: &AdjustParams) -> egui::ColorImage {
    let [w, h] = src.size;
    let lut = build_u8_lut(params);
    let full_desat = params.saturation == -100.0;
    let has_sat = params.saturation != 0.0;
    let sat_factor = 1.0 + params.saturation / 100.0;

    let pixels: Vec<egui::Color32> = src.pixels.iter().map(|c| {
        // Color32 は premultiplied 格納。元のアルファを保持するため unmultiplied 経由で再構築する。
        let [_, _, _, a] = c.to_srgba_unmultiplied();
        let r = lut[c.r() as usize];
        let g = lut[c.g() as usize];
        let b = lut[c.b() as usize];

        let (nr, ng, nb) = if full_desat {
            let lum = pixel_lum(&egui::Color32::from_rgb(r, g, b));
            (lum, lum, lum)
        } else if has_sat {
            let (r01, g01, b01) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
            let max = r01.max(g01).max(b01);
            let min = r01.min(g01).min(b01);
            let lum = (max + min) * 0.5;
            if (max - min).abs() < 1e-6 {
                (r, g, b)
            } else {
                (
                    ((lum + (r01 - lum) * sat_factor) * 255.0).clamp(0.0, 255.0) as u8,
                    ((lum + (g01 - lum) * sat_factor) * 255.0).clamp(0.0, 255.0) as u8,
                    ((lum + (b01 - lum) * sat_factor) * 255.0).clamp(0.0, 255.0) as u8,
                )
            }
        } else {
            (r, g, b)
        };
        egui::Color32::from_rgba_unmultiplied(nr, ng, nb, a)
    }).collect();

    egui::ColorImage::new([w, h], pixels)
}

fn pixels_to_f32(src: &egui::ColorImage) -> Vec<[f32; 3]> {
    src.pixels.iter().map(|c| [c.r() as f32, c.g() as f32, c.b() as f32]).collect()
}

fn f32_to_image(src: &egui::ColorImage, buf: Vec<[f32; 3]>, w: usize, h: usize) -> egui::ColorImage {
    let pixels = buf.iter().zip(src.pixels.iter()).map(|([r, g, b], c)| {
        let [_, _, _, a] = c.to_srgba_unmultiplied();
        egui::Color32::from_rgba_unmultiplied(
            r.clamp(0.0, 255.0) as u8, g.clamp(0.0, 255.0) as u8, b.clamp(0.0, 255.0) as u8, a,
        )
    }).collect();
    egui::ColorImage::new([w, h], pixels)
}

/// f32 パイプライン (色温度ありの場合に使用)。
fn apply_color_pipeline(buf: &mut Vec<[f32; 3]>, params: &AdjustParams) {
    let needs_levels = params.black_point != 0 || params.white_point != 255 || params.midtone != 1.0;
    let needs_gamma = (params.gamma - 1.0).abs() >= 0.001;
    if needs_levels || needs_gamma {
        // levels + gamma を f32 LUT で処理
        let bp = params.black_point as f32;
        let range = (params.white_point as f32 - bp).max(1.0);
        let inv_midtone = 1.0 / params.midtone;
        let inv_gamma = 1.0 / params.gamma;
        let mut lut = [0.0_f32; 256];
        for i in 0..256 {
            let mut v = i as f32;
            if needs_levels { v = ((v - bp) / range).clamp(0.0, 1.0).powf(inv_midtone) * 255.0; }
            if needs_gamma { v = (v / 255.0).clamp(0.0, 1.0).powf(inv_gamma) * 255.0; }
            lut[i] = v;
        }
        for px in buf.iter_mut() {
            for ch in px.iter_mut() { *ch = lut[(*ch).clamp(0.0, 255.0) as u8 as usize]; }
        }
    }
    apply_brightness_contrast(buf, params.brightness, params.contrast);
    if params.saturation == -100.0 {
        for px in buf.iter_mut() {
            let lum = px[0] * 0.299 + px[1] * 0.587 + px[2] * 0.114;
            *px = [lum, lum, lum];
        }
    } else if params.saturation != 0.0 {
        apply_saturation(buf, params.saturation);
    }
    apply_temperature(buf, params.temperature);
}

// ── 内部処理関数 ────────────────────────────────────────────────

/// 輝度ヒストグラムを構築する。大きな画像では LCG ランダムサンプリングで高速化。
/// 戻り値: (histogram[256], sample_count)
fn build_luma_histogram(pixels: &[egui::Color32]) -> ([u64; 256], u64) {
    let mut hist = [0u64; 256];
    let n = pixels.len();
    if n <= HISTOGRAM_SAMPLE_LIMIT {
        for c in pixels {
            hist[pixel_lum(c) as usize] += 1;
        }
        (hist, n as u64)
    } else {
        // LCG ランダムサンプリング (Vec 割り当てなし)
        let m = n as u64;
        let a: u64 = 6364136223846793005;
        let c: u64 = 1442695040888963407;
        let mut x: u64 = 42;
        let sample_count = HISTOGRAM_SAMPLE_LIMIT as u64;
        for _ in 0..sample_count {
            x = x.wrapping_mul(a).wrapping_add(c);
            let idx = (x % m) as usize;
            hist[pixel_lum(&pixels[idx]) as usize] += 1;
        }
        (hist, sample_count)
    }
}

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
            p.saturation = -100.0;
            let (bp, wp) = compute_manga_levels(src);
            p.black_point = bp;
            p.white_point = wp;
            p.gamma = 0.85;
            p.contrast = p.contrast.max(15.0);
        }
    }
    p
}

fn compute_auto_levels(src: &egui::ColorImage, clip_ratio: f64) -> (u8, u8) {
    let (hist, sample_count) = build_luma_histogram(&src.pixels);
    let total = sample_count as f64;
    let low_threshold = (total * clip_ratio) as u64;
    let high_threshold = (total * (1.0 - clip_ratio)) as u64;

    let mut cum = 0u64;
    let mut bp = 0u8;
    for (i, &count) in hist.iter().enumerate() {
        cum += count;
        if cum >= low_threshold { bp = i as u8; break; }
    }
    cum = 0;
    let mut wp = 255u8;
    for (i, &count) in hist.iter().enumerate() {
        cum += count;
        if cum >= high_threshold { wp = i as u8; break; }
    }
    (bp, wp.max(bp + 1))
}

fn compute_manga_levels(src: &egui::ColorImage) -> (u8, u8) {
    let (hist, _) = build_luma_histogram(&src.pixels);

    let mut max_count = 0u64;
    let mut paper_lum = 240u8;
    for i in 180..256 {
        if hist[i] > max_count { max_count = hist[i]; paper_lum = i as u8; }
    }
    let wp = paper_lum.saturating_sub(10);

    let mut max_dark = 0u64;
    let mut ink_lum = 20u8;
    for i in 0..80 {
        if hist[i] > max_dark { max_dark = hist[i]; ink_lum = i as u8; }
    }
    let bp = ink_lum.saturating_add(5);
    (bp, wp.max(bp + 1))
}

fn apply_brightness_contrast(buf: &mut [[f32; 3]], brightness: f32, contrast: f32) {
    if brightness == 0.0 && contrast == 0.0 { return; }
    let factor = (259.0 * (contrast + 255.0)) / (255.0 * (259.0 - contrast));
    let bright_add = brightness * 2.55;
    for px in buf.iter_mut() {
        for ch in px.iter_mut() { *ch = factor * (*ch - 128.0) + 128.0 + bright_add; }
    }
}

fn apply_saturation(buf: &mut [[f32; 3]], saturation: f32) {
    if saturation == 0.0 { return; }
    let factor = 1.0 + saturation / 100.0;
    for px in buf.iter_mut() {
        let [r, g, b] = *px;
        let (r01, g01, b01) = (r / 255.0, g / 255.0, b / 255.0);
        let max = r01.max(g01).max(b01);
        let min = r01.min(g01).min(b01);
        if (max - min).abs() < 1e-6 { continue; }
        let lum = (max + min) * 0.5;
        px[0] = (lum + (r01 - lum) * factor) * 255.0;
        px[1] = (lum + (g01 - lum) * factor) * 255.0;
        px[2] = (lum + (b01 - lum) * factor) * 255.0;
    }
}

fn apply_temperature(buf: &mut [[f32; 3]], temperature: f32) {
    if temperature == 0.0 { return; }
    let shift = temperature * 0.5;
    for px in buf.iter_mut() { px[0] += shift; px[2] -= shift; }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_params_no_change() {
        let params = AdjustParams::default();
        assert!(params.is_identity());
        let pixels = vec![
            egui::Color32::from_rgb(100, 150, 200), egui::Color32::from_rgb(50, 50, 50),
            egui::Color32::from_rgb(200, 200, 200), egui::Color32::from_rgb(0, 0, 0),
        ];
        let src = egui::ColorImage::new([2, 2], pixels.clone());
        let result = apply_adjustments(&src, &params);
        assert_eq!(result.pixels, pixels);
    }

    #[test]
    fn brightness_increases() {
        let params = AdjustParams { brightness: 50.0, ..Default::default() };
        let src = egui::ColorImage::new([1, 1], vec![egui::Color32::from_rgb(100, 100, 100)]);
        let result = apply_adjustments(&src, &params);
        assert!(result.pixels[0].r() > 100);
    }

    #[test]
    fn auto_level_sets_points() {
        let mut pixels = Vec::new();
        for _ in 0..100 { pixels.push(egui::Color32::from_rgb(50, 50, 50)); }
        for _ in 0..800 { pixels.push(egui::Color32::from_rgb(128, 128, 128)); }
        for _ in 0..100 { pixels.push(egui::Color32::from_rgb(230, 230, 230)); }
        let src = egui::ColorImage::new([100, 10], pixels);
        let (bp, wp) = compute_auto_levels(&src, 0.05);
        assert!(bp >= 40);
        assert!(wp <= 240);
    }

    #[test]
    fn histogram_sampling_large_image() {
        let (hist, count) = build_luma_histogram(
            &vec![egui::Color32::from_rgb(128, 128, 128); 2_000_000]
        );
        assert_eq!(count, HISTOGRAM_SAMPLE_LIMIT as u64);
        assert!(hist[128] > 0);
    }

    #[test]
    fn ai_settings_eq_check() {
        let a = AdjustParams { brightness: 50.0, ..Default::default() };
        let b = AdjustParams { brightness: -20.0, ..Default::default() };
        assert!(a.ai_settings_eq(&b)); // 色調のみ異なる

        let c = AdjustParams { upscale_model: Some("auto".into()), ..Default::default() };
        assert!(!a.ai_settings_eq(&c)); // AI設定が異なる
    }

    #[test]
    fn slot_key_labels() {
        assert_eq!(slot_key_label(0), "1");
        assert_eq!(slot_key_label(8), "9");
        assert_eq!(slot_key_label(9), "0");
    }
}

#[cfg(test)]
mod sampling_quality_tests {
    use super::*;

    #[test]
    fn manga_levels_sampling_matches_full_scan() {
        let mut pixels = Vec::with_capacity(2_000_000);
        for i in 0..2_000_000u32 {
            let lum = if i % 5 == 0 { 20 } else { 240 };
            pixels.push(egui::Color32::from_rgb(lum, lum, lum));
        }
        let src = egui::ColorImage::new([2000, 1000], pixels);
        let (bp_sampled, wp_sampled) = compute_manga_levels(&src);
        // Full scan reference
        let mut hist_full = [0u64; 256];
        for c in &src.pixels { hist_full[pixel_lum(c) as usize] += 1; }
        let mut mc = 0u64; let mut pl = 240u8;
        for i in 180..256 { if hist_full[i] > mc { mc = hist_full[i]; pl = i as u8; } }
        let wp_full = pl.saturating_sub(10);
        let mut md = 0u64; let mut il = 20u8;
        for i in 0..80 { if hist_full[i] > md { md = hist_full[i]; il = i as u8; } }
        let bp_full = il.saturating_add(5);
        assert!((bp_sampled as i16 - bp_full as i16).unsigned_abs() <= 3);
        assert!((wp_sampled as i16 - wp_full as i16).unsigned_abs() <= 3);
    }
}
