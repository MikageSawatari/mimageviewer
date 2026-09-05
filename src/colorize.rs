//! モノクロ系画像のカラー化とスクリーントーン濃度復元。
//!
//! 処理順は final pipeline のスマートシャープ後、ポストフィルタ前。
//! カスタムパレットは NeeView の ColorizeEffect と同じ「色 + 強さ」の制御点から
//! 256-entry LUT を生成する。画像ごとの濃さを整える自動レベル補正と
//! スクリーントーン濃度復元は、直接着色する前の輝度だけを置き換える。

use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Color32, ColorImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

const MONO_SAMPLE_LIMIT: usize = 16_384;
const MONO_INLIER_RATIO: f32 = 0.95;
const DENSITY_SAMPLE_LIMIT: usize = 262_144;
const DENSITY_CLIP_RATIO: f32 = 0.005;
const DENSITY_MIN_RANGE: u8 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorizeMode {
    #[default]
    Disabled,
    MonochromeOnly,
    AllImages,
}

impl ColorizeMode {
    pub fn enabled(self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorizePalette {
    #[default]
    Legacy4Color,
    LegacySkin,
    Custom,
}

impl ColorizePalette {
    pub fn label(self) -> &'static str {
        match self {
            Self::Legacy4Color => "4色刷り（従来互換）",
            Self::LegacySkin => "肌色（従来互換）",
            Self::Custom => "カスタム",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorizeControlPoint {
    pub color: [u8; 3],
    pub strength: f32,
}

impl ColorizeControlPoint {
    pub fn new(color: [u8; 3], strength: f32) -> Self {
        Self { color, strength }
    }
}

fn default_control_points() -> Vec<ColorizeControlPoint> {
    vec![
        ColorizeControlPoint::new([0, 0, 0], 3.0),
        ColorizeControlPoint::new([75, 0, 130], 1.0),
        ColorizeControlPoint::new([205, 92, 92], 1.0),
        ColorizeControlPoint::new([245, 222, 179], 1.0),
        ColorizeControlPoint::new([240, 248, 255], 1.0),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToneDensityMethod {
    #[default]
    Off,
    Fast,
    LocalMean,
    #[serde(alias = "edge_preserving", alias = "multi_scale")]
    Gaussian,
}

impl ToneDensityMethod {
    pub const ALL: &'static [Self] = &[Self::Off, Self::Fast, Self::LocalMean, Self::Gaussian];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF（画素の輝度をそのまま使用）",
            Self::Fast => "高速（縮小平均）",
            Self::LocalMean => "弱（局所平均）",
            Self::Gaussian => "強（ガウシアン）",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Off => "スクリーントーンの網点を濃淡へ変換しません。",
            Self::Fast => {
                "縮小平均で網点を濃淡化します。大きな画像でも高速ですが、細部は少し滑らかになります。"
            }
            Self::LocalMean => {
                "局所平均を1回適用します。網点を少しだけなじませたい場合に向きます。"
            }
            Self::Gaussian => {
                "局所平均を3回重ねてガウスぼかしを近似します。より広く滑らかに濃淡化します。"
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorizeParams {
    #[serde(default)]
    pub mode: ColorizeMode,
    #[serde(default = "default_mono_tolerance")]
    pub mono_tolerance: u8,
    #[serde(default)]
    pub palette: ColorizePalette,
    #[serde(default = "default_control_points")]
    pub control_points: Vec<ColorizeControlPoint>,
    /// LUT 色の輝度を元画像の輝度へ寄せる割合。0..=100。
    #[serde(default = "default_luminance_weight")]
    pub luminance_weight: u8,
    /// 画像ごとの輝度分布を自動レベル補正へ寄せる割合。0..=100。
    #[serde(default)]
    pub density_normalization_strength: u8,
    #[serde(default)]
    pub tone_method: ToneDensityMethod,
    /// 長辺 2048px を基準にしたトーン密度の検出スケール。0.1..=4.0。
    #[serde(default = "default_tone_radius")]
    pub tone_radius: f32,
    /// 元輝度から推定濃度へ寄せる割合。0..=100。
    #[serde(default = "default_tone_strength")]
    pub tone_strength: u8,
}

const fn default_mono_tolerance() -> u8 {
    12
}

const fn default_luminance_weight() -> u8 {
    100
}

const fn default_tone_radius() -> f32 {
    1.0
}

const TONE_RADIUS_REFERENCE_LONG_EDGE: f32 = 2048.0;
const MAX_EFFECTIVE_TONE_RADIUS: f32 = 64.0;

const fn default_tone_strength() -> u8 {
    100
}

impl Default for ColorizeParams {
    fn default() -> Self {
        Self {
            mode: ColorizeMode::Disabled,
            mono_tolerance: default_mono_tolerance(),
            palette: ColorizePalette::Legacy4Color,
            control_points: default_control_points(),
            luminance_weight: default_luminance_weight(),
            density_normalization_strength: 0,
            tone_method: ToneDensityMethod::Off,
            tone_radius: default_tone_radius(),
            tone_strength: default_tone_strength(),
        }
    }
}

impl ColorizeParams {
    pub fn is_enabled(&self) -> bool {
        self.mode.enabled()
    }

    pub fn enable_with_palette(&mut self, palette: ColorizePalette) {
        self.palette = palette;
        if !self.mode.enabled() {
            self.mode = ColorizeMode::MonochromeOnly;
        }
    }

    pub fn legacy_all_images(palette: ColorizePalette) -> Self {
        Self {
            mode: ColorizeMode::AllImages,
            palette,
            // 旧ポストフィルタは LUT の輝度をそのまま使う。設定移行と
            // 従来互換プリセットだけは新規設定の既定 100% に追従させない。
            luminance_weight: 0,
            ..Self::default()
        }
    }

    pub fn sanitize(&mut self) {
        self.mono_tolerance = self.mono_tolerance.clamp(1, 64);
        self.luminance_weight = self.luminance_weight.min(100);
        self.density_normalization_strength = self.density_normalization_strength.min(100);
        self.tone_radius = if self.tone_radius.is_finite() {
            self.tone_radius.clamp(0.1, 4.0)
        } else {
            default_tone_radius()
        };
        self.tone_strength = self.tone_strength.min(100);
        self.control_points.truncate(10);
        if self.control_points.len() < 2 {
            self.control_points = default_control_points();
        }
        for point in &mut self.control_points {
            if !point.strength.is_finite() {
                point.strength = 1.0;
            }
            point.strength = point.strength.clamp(0.0, 10.0);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ColorizePresetSlots {
    pub slots: [Option<ColorizeParams>; 4],
}

fn required_mono_inliers(sample_count: usize) -> usize {
    debug_assert!(sample_count > 0);
    let count = sample_count as f32;
    let mut required = (count * MONO_INLIER_RATIO).ceil() as usize;
    while required > 0 && (required - 1) as f32 / count >= MONO_INLIER_RATIO {
        required -= 1;
    }
    while required as f32 / count < MONO_INLIER_RATIO {
        required += 1;
    }
    required
}

/// 画像内の RGB がほぼ 1 本の色の線に並ぶかを表す要約。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonoToneAxis {
    pub mean: [f32; 3],
    pub axis: [f32; 3],
    pub p95_residual: f32,
}

/// 黄ばみ・青みなど一方向の紙色を許容する近モノクロ判定の要約値と色の軸を求める。
///
/// サンプル RGB の主成分軸を power iteration で求め、軸からの直交残差を
/// 小さい順に並べたとき、従来の「内点が 95% 以上」と同じ最小内点数に対応する
/// 残差を返す。平均・共分散・主成分軸は許容値に依存しないため、この値は
/// `mono_tolerance` が変わっても再利用できる。判定不能な極小サンプル、
/// ほぼ単色な画像、主成分軸が縮退した画像では `None` を返す。
/// Wall and executed cycles for the three parts of `mono_tone_axis`.
///
/// Its caller measured 7.5% busy - it waits rather than computes - and the only
/// operations here that can wait are the two allocations and touching the source
/// pixels. This says which, without guessing a tenth time.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MonoToneAxisTiming {
    pub sample_ms: f64,
    pub sample_cycles: u64,
    pub compute_ms: f64,
    pub compute_cycles: u64,
    pub residual_ms: f64,
    pub residual_cycles: u64,
}

#[cfg(windows)]
static MONO_TIMING: std::sync::Mutex<Option<MonoToneAxisTiming>> = std::sync::Mutex::new(None);

#[cfg(windows)]
fn cycles_now() -> u64 {
    let mut c = 0u64;
    unsafe {
        let _ = windows::Win32::System::WindowsProgramming::QueryThreadCycleTime(
            windows::Win32::System::Threading::GetCurrentThread(),
            &mut c,
        );
    }
    c
}

/// Take and clear the accumulated split. Returns `None` when nothing ran.
#[cfg(windows)]
pub fn take_mono_tone_axis_timing() -> Option<MonoToneAxisTiming> {
    MONO_TIMING.lock().ok().and_then(|mut slot| slot.take())
}

#[cfg(not(windows))]
pub fn take_mono_tone_axis_timing() -> Option<()> {
    None
}

pub fn mono_tone_axis(src: &ColorImage) -> Option<MonoToneAxis> {
    let total = src.pixels.len();
    if total == 0 {
        return None;
    }
    #[cfg(windows)]
    let timing_on = crate::perf::is_enabled();
    #[cfg(windows)]
    let (t_sample, c_sample) = (std::time::Instant::now(), cycles_now());
    let stride = total.div_ceil(MONO_SAMPLE_LIMIT).max(1);
    let samples: Vec<[f32; 3]> = src
        .pixels
        .iter()
        .step_by(stride)
        .filter(|pixel| pixel.a() >= 16)
        .take(MONO_SAMPLE_LIMIT)
        .map(|pixel| [pixel.r() as f32, pixel.g() as f32, pixel.b() as f32])
        .collect();
    #[cfg(windows)]
    let (sample_ms, sample_cycles) = (
        t_sample.elapsed().as_secs_f64() * 1000.0,
        cycles_now().saturating_sub(c_sample),
    );
    if samples.len() < 8 {
        return None;
    }
    #[cfg(windows)]
    let (t_compute, c_compute) = (std::time::Instant::now(), cycles_now());

    let inv_n = 1.0 / samples.len() as f32;
    let mut mean = [0.0_f32; 3];
    for sample in &samples {
        for channel in 0..3 {
            mean[channel] += sample[channel] * inv_n;
        }
    }

    let mut covariance = [[0.0_f32; 3]; 3];
    for sample in &samples {
        let d = [
            sample[0] - mean[0],
            sample[1] - mean[1],
            sample[2] - mean[2],
        ];
        for row in 0..3 {
            for col in 0..3 {
                covariance[row][col] += d[row] * d[col] * inv_n;
            }
        }
    }

    let total_variance = covariance[0][0] + covariance[1][1] + covariance[2][2];
    if total_variance <= 1.0 {
        return None;
    }
    let mut axis = [0.577_350_26_f32; 3];
    for _ in 0..12 {
        let next = [
            covariance[0][0] * axis[0] + covariance[0][1] * axis[1] + covariance[0][2] * axis[2],
            covariance[1][0] * axis[0] + covariance[1][1] * axis[1] + covariance[1][2] * axis[2],
            covariance[2][0] * axis[0] + covariance[2][1] * axis[1] + covariance[2][2] * axis[2],
        ];
        let length = (next[0] * next[0] + next[1] * next[1] + next[2] * next[2]).sqrt();
        if length <= 1e-5 {
            return None;
        }
        axis = [next[0] / length, next[1] / length, next[2] / length];
    }

    #[cfg(windows)]
    let (compute_ms, compute_cycles) = (
        t_compute.elapsed().as_secs_f64() * 1000.0,
        cycles_now().saturating_sub(c_compute),
    );
    #[cfg(windows)]
    let (t_res, c_res) = (std::time::Instant::now(), cycles_now());
    let mut residuals = samples
        .iter()
        .map(|sample| {
            let d = [
                sample[0] - mean[0],
                sample[1] - mean[1],
                sample[2] - mean[2],
            ];
            let projection = d[0] * axis[0] + d[1] * axis[1] + d[2] * axis[2];
            let residual_sq =
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2] - projection * projection).max(0.0);
            residual_sq.sqrt()
        })
        .collect::<Vec<_>>();
    let percentile_index = required_mono_inliers(residuals.len()) - 1;
    let (_, p95, _) = residuals.select_nth_unstable_by(percentile_index, |a, b| a.total_cmp(b));
    #[cfg(windows)]
    if timing_on && let Ok(mut slot) = MONO_TIMING.lock() {
        let acc = slot.get_or_insert_with(MonoToneAxisTiming::default);
        acc.sample_ms += sample_ms;
        acc.sample_cycles += sample_cycles;
        acc.compute_ms += compute_ms;
        acc.compute_cycles += compute_cycles;
        acc.residual_ms += t_res.elapsed().as_secs_f64() * 1000.0;
        acc.residual_cycles += cycles_now().saturating_sub(c_res);
    }
    Some(MonoToneAxis {
        mean,
        axis,
        p95_residual: *p95,
    })
}

/// 許容値に依存しない近モノクロ判定の要約値。
///
/// 軸を求められない画像は、従来の早期 `true` と等価な `0.0` を返す。
pub fn near_monochrome_p95_residual(src: &ColorImage) -> f32 {
    mono_tone_axis(src)
        .map(|summary| summary.p95_residual)
        .unwrap_or(0.0)
}

/// 許容値に依存しない近モノクロ要約値を、現在の UI 設定と比較する。
pub fn is_near_monochrome_residual(p95_residual: f32, tolerance: u8) -> bool {
    p95_residual <= f32::from(tolerance)
}

/// 黄ばみ・青みなど一方向の紙色を許容する近モノクロ判定。
///
/// サンプル RGB の主成分軸からの直交残差が `tolerance` 以下の画素が
/// 95% 以上ならモノクロ系とみなす。純グレーだけでなく、黒インクから
/// 有色紙へ伸びる 1 次元の色分布を通す。
pub fn is_near_monochrome(src: &ColorImage, tolerance: u8) -> bool {
    is_near_monochrome_residual(near_monochrome_p95_residual(src), tolerance)
}

pub fn should_apply(src: &ColorImage, params: &ColorizeParams) -> bool {
    match params.mode {
        ColorizeMode::Disabled => false,
        ColorizeMode::AllImages => true,
        ColorizeMode::MonochromeOnly => is_near_monochrome(src, params.mono_tolerance),
    }
}

pub fn apply(src: &ColorImage, params: &ColorizeParams) -> ColorImage {
    let cancel = AtomicBool::new(false);
    apply_with_cancel(src, params, &cancel).unwrap_or_else(|| src.clone())
}

pub fn apply_with_cancel(
    src: &ColorImage,
    params: &ColorizeParams,
    cancel: &AtomicBool,
) -> Option<ColorImage> {
    if !should_apply(src, params) {
        return Some(src.clone());
    }
    apply_applicable_with_cancel(src, params, cancel)
}

/// `should_apply` 済みの呼び出し元向け。final effect worker は不適用時の
/// `ColorImage` 全体 clone を避けるため先に判定するので、近モノクロ判定を
/// もう一度走らせず本体処理へ入る。
pub(crate) fn apply_applicable_with_cancel(
    src: &ColorImage,
    params: &ColorizeParams,
    cancel: &AtomicBool,
) -> Option<ColorImage> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let [width, height] = src.size;
    if width == 0 || height == 0 {
        return Some(src.clone());
    }
    let effective_tone_radius = effective_tone_radius(params.tone_radius, src.size);
    let lut = build_lut(params);
    let mut luma: Vec<u8> = src
        .pixels
        .par_iter()
        .map(|pixel| {
            (crate::adjustment::pixel_lum_f32(*pixel) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8
        })
        .collect();
    normalize_density_luma(
        src,
        &mut luma,
        params.density_normalization_strength,
        cancel,
    )?;
    let fast_tone = if params.tone_method == ToneDensityMethod::Fast {
        Some(fast_tone_density_luma(
            &luma,
            width,
            height,
            effective_tone_radius,
            cancel,
        )?)
    } else {
        None
    };
    let tone = if fast_tone.is_none() {
        tone_density_luma(
            &luma,
            width,
            height,
            params.tone_method,
            effective_tone_radius,
            cancel,
        )?
    } else {
        None
    };
    let tone_strength = f32::from(params.tone_strength) / 100.0;
    let luminance_weight = f32::from(params.luminance_weight) / 100.0;
    let output_lut: [[u8; 3]; 256] = std::array::from_fn(|index| {
        preserve_luminance(lut[index], index as f32 / 255.0, luminance_weight)
    });

    let pixels: Vec<Color32> = src
        .pixels
        .par_iter()
        .enumerate()
        .map(|(index, pixel)| {
            let original_y = f32::from(luma[index]) / 255.0;
            let candidate = fast_tone
                .as_ref()
                .map(|tone| tone.sample(index, width, original_y))
                .or_else(|| tone.as_ref().map(|tone| f32::from(tone[index]) / 255.0));
            let mapped_y = if let Some(candidate) = candidate {
                original_y * (1.0 - tone_strength) + candidate * tone_strength
            } else {
                original_y
            };
            let lut_index = (mapped_y * 255.0).round().clamp(0.0, 255.0) as usize;
            let result = output_lut[lut_index];
            Color32::from_rgba_unmultiplied(result[0], result[1], result[2], pixel.a())
        })
        .collect();
    if cancel.load(Ordering::Relaxed) {
        None
    } else {
        Some(ColorImage::new([width, height], pixels))
    }
}

fn normalize_density_luma(
    src: &ColorImage,
    luma: &mut [u8],
    strength: u8,
    cancel: &AtomicBool,
) -> Option<()> {
    let strength = strength.min(100);
    if strength == 0 || luma.is_empty() {
        return Some(());
    }
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let Some((black_point, white_point)) = density_normalization_bounds(src, luma) else {
        return Some(());
    };
    let black_point = f32::from(black_point);
    let range = f32::from(white_point) - black_point;
    let mix = f32::from(strength) / 100.0;
    luma.par_iter_mut().for_each(|value| {
        let original = f32::from(*value);
        let normalized = ((original - black_point) / range).clamp(0.0, 1.0) * 255.0;
        *value = (original * (1.0 - mix) + normalized * mix)
            .round()
            .clamp(0.0, 255.0) as u8;
    });
    if cancel.load(Ordering::Relaxed) {
        None
    } else {
        Some(())
    }
}

fn density_normalization_bounds(src: &ColorImage, luma: &[u8]) -> Option<(u8, u8)> {
    if src.pixels.len() != luma.len() || luma.is_empty() {
        return None;
    }
    let mut histogram = [0_u32; 256];
    let mut sample_count = 0_u32;
    let mut add_sample = |index: usize| {
        if src.pixels[index].a() < 16 {
            return;
        }
        histogram[luma[index] as usize] += 1;
        sample_count += 1;
    };
    if luma.len() <= DENSITY_SAMPLE_LIMIT {
        for index in 0..luma.len() {
            add_sample(index);
        }
    } else {
        let modulus = luma.len() as u64;
        let mut state = 42_u64;
        for _ in 0..DENSITY_SAMPLE_LIMIT {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            add_sample((state % modulus) as usize);
        }
    }
    if sample_count < 8 {
        return None;
    }

    let clipped = (sample_count as f32 * DENSITY_CLIP_RATIO).floor() as u32;
    let low_rank = clipped.min(sample_count - 1);
    let high_rank = sample_count - 1 - low_rank;
    let mut cumulative = 0_u32;
    let mut black_point = 0_u8;
    for (index, count) in histogram.iter().enumerate() {
        cumulative += count;
        if cumulative > low_rank {
            black_point = index as u8;
            break;
        }
    }
    cumulative = 0;
    let mut white_point = 255_u8;
    for (index, count) in histogram.iter().enumerate() {
        cumulative += count;
        if cumulative > high_rank {
            white_point = index as u8;
            break;
        }
    }
    if white_point.saturating_sub(black_point) < DENSITY_MIN_RANGE {
        None
    } else {
        Some((black_point, white_point))
    }
}

pub(crate) fn effective_tone_radius(configured_radius: f32, size: [usize; 2]) -> f32 {
    let configured_radius = if configured_radius.is_finite() {
        configured_radius.clamp(0.1, 4.0)
    } else {
        default_tone_radius()
    };
    let long_edge = size[0].max(size[1]) as f32;
    (configured_radius * long_edge / TONE_RADIUS_REFERENCE_LONG_EDGE).min(MAX_EFFECTIVE_TONE_RADIUS)
}

fn build_lut(params: &ColorizeParams) -> [[u8; 3]; 256] {
    match params.palette {
        ColorizePalette::Legacy4Color => {
            std::array::from_fn(crate::post_filter::legacy_pseudocolor4_rgb)
        }
        ColorizePalette::LegacySkin => {
            std::array::from_fn(crate::post_filter::legacy_pseudocolor_skin_rgb)
        }
        ColorizePalette::Custom => build_custom_lut(&params.control_points),
    }
}

/// 設定パネルの階調プレビュー用 LUT。
///
/// 実画像処理と同じ palette LUT と輝度保持を 0..=255 の入力輝度へ適用する。
/// 画像固有の分布を必要とする濃度正規化と、近傍画素を必要とするトーン変換は
/// 1 次元バーでは表現しない。
pub fn preview_lut(params: &ColorizeParams) -> [[u8; 3]; 256] {
    let lut = build_lut(params);
    let luminance_weight = f32::from(params.luminance_weight.min(100)) / 100.0;
    std::array::from_fn(|index| {
        preserve_luminance(lut[index], index as f32 / 255.0, luminance_weight)
    })
}

fn build_custom_lut(points: &[ColorizeControlPoint]) -> [[u8; 3]; 256] {
    let fallback;
    let points = if points.len() >= 2 {
        &points[..points.len().min(10)]
    } else {
        fallback = default_control_points();
        &fallback
    };
    let mut lengths = Vec::with_capacity(points.len() - 1);
    let mut total = 0.0_f32;
    for pair in points.windows(2) {
        let length = ((pair[0].strength + pair[1].strength) * 0.5)
            .max(0.01)
            .min(10.0);
        lengths.push(length);
        total += length;
    }
    let mut knots = vec![0.0_f32; points.len()];
    for index in 0..lengths.len() {
        knots[index + 1] = knots[index] + lengths[index] / total.max(0.01);
    }
    std::array::from_fn(|index| {
        let luminance = index as f32 / 255.0;
        let mut section = 0;
        while section < points.len() - 2
            && !(knots[section] <= luminance && luminance <= knots[section + 1])
        {
            section += 1;
        }
        let span = (knots[section + 1] - knots[section]).max(1e-6);
        let v = ((luminance - knots[section]) / span).clamp(0.0, 1.0);
        let s0 = points[section].strength.clamp(0.0, 10.0);
        let s1 = points[section + 1].strength.clamp(0.0, 10.0);
        let midpoint = if s0 + s1 > 0.0 { s1 / (s0 + s1) } else { 0.5 };
        let a = 2.0 - 4.0 * midpoint;
        let b = 4.0 * midpoint - 1.0;
        let curved = (a * v * v + b * v).clamp(0.0, 1.0);
        std::array::from_fn(|channel| {
            (f32::from(points[section].color[channel]) * (1.0 - curved)
                + f32::from(points[section + 1].color[channel]) * curved)
                .clamp(0.0, 255.0) as u8
        })
    })
}

fn preserve_luminance(color: [u8; 3], y: f32, weight: f32) -> [u8; 3] {
    if weight <= 0.0 {
        return color;
    }
    let r = f32::from(color[0]) / 255.0;
    let g = f32::from(color[1]) / 255.0;
    let b = f32::from(color[2]) / 255.0;
    let cb = -0.1146 * r - 0.3854 * g + 0.5 * b;
    let cr = 0.5 * r - 0.4545 * g - 0.0455 * b;
    let preserved = [
        (y + 1.5748 * cr).clamp(0.0, 1.0),
        (y - 0.1873 * cb - 0.4681 * cr).clamp(0.0, 1.0),
        (y + 1.8556 * cb).clamp(0.0, 1.0),
    ];
    std::array::from_fn(|channel| {
        (([r, g, b][channel] * (1.0 - weight) + preserved[channel] * weight) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8
    })
}

struct ReducedToneLuma {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    factor: usize,
}

impl ReducedToneLuma {
    /// 全解像度の tone buffer を作らず、対応する縮小ブロックを直接参照する。
    /// 高速モードでは数px単位の滑らかさよりメモリ帯域削減を優先する。
    #[inline]
    fn sample(&self, index: usize, source_width: usize) -> f32 {
        let x = index % source_width;
        let y = index / source_width;
        let grid_x = (x / self.factor).min(self.width - 1);
        let grid_y = (y / self.factor).min(self.height - 1);
        f32::from(self.pixels[grid_y * self.width + grid_x]) / 255.0
    }
}

struct FastToneLuma {
    reduced: ReducedToneLuma,
    original_weight: f32,
}

impl FastToneLuma {
    #[inline]
    fn sample(&self, index: usize, source_width: usize, original_y: f32) -> f32 {
        let reduced = self.reduced.sample(index, source_width);
        original_y * self.original_weight + reduced * (1.0 - self.original_weight)
    }
}

/// 高速トーン濃度推定。
///
/// 全解像度画像へ box blur を繰り返す代わりに、指定スケール相当のブロック平均を
/// 低解像度で作り、3x3 平均を1回だけ掛ける。最終着色ループから直接参照するため、
/// 全解像度への再拡大 buffer とその読み書きも不要。50ms 級を優先するモードなので
/// 1px 以上の実効スケールは最寄り整数へ丸めて縮小画像を1系統だけ作る。1px 未満は
/// 元輝度から連続的に立ち上げる。
fn fast_tone_density_luma(
    luma: &[u8],
    width: usize,
    height: usize,
    radius: f32,
    cancel: &AtomicBool,
) -> Option<FastToneLuma> {
    let radius = if radius.is_finite() {
        radius.clamp(0.0, MAX_EFFECTIVE_TONE_RADIUS)
    } else {
        default_tone_radius()
    };
    let (factor, original_weight) = if radius < 1.0 {
        (1, 1.0 - radius)
    } else {
        (radius.round().max(1.0) as usize, 0.0)
    };
    Some(FastToneLuma {
        reduced: build_reduced_tone_luma(luma, width, height, factor, cancel)?,
        original_weight,
    })
}

fn build_reduced_tone_luma(
    luma: &[u8],
    width: usize,
    height: usize,
    factor: usize,
    cancel: &AtomicBool,
) -> Option<ReducedToneLuma> {
    debug_assert!(factor > 0);
    let reduced_width = width.div_ceil(factor);
    let reduced_height = height.div_ceil(factor);
    if factor == 1 {
        return blur_reduced_tone_luma(luma, reduced_width, reduced_height, factor, cancel);
    }
    let mut reduced = vec![0_u8; reduced_width * reduced_height];
    reduced
        .par_chunks_mut(reduced_width)
        .enumerate()
        .for_each(|(reduced_y, row)| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let y0 = reduced_y * factor;
            let y1 = (y0 + factor).min(height);
            for (reduced_x, value) in row.iter_mut().enumerate() {
                let x0 = reduced_x * factor;
                let x1 = (x0 + factor).min(width);
                let mut sum = 0_u32;
                for y in y0..y1 {
                    for x in x0..x1 {
                        sum += u32::from(luma[y * width + x]);
                    }
                }
                *value = (sum / ((x1 - x0) * (y1 - y0)) as u32) as u8;
            }
        });
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    blur_reduced_tone_luma(&reduced, reduced_width, reduced_height, factor, cancel)
}

fn blur_reduced_tone_luma(
    reduced: &[u8],
    width: usize,
    height: usize,
    factor: usize,
    cancel: &AtomicBool,
) -> Option<ReducedToneLuma> {
    let mut pixels = vec![0_u8; reduced.len()];
    pixels
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(height - 1);
            for (x, value) in row.iter_mut().enumerate() {
                let x0 = x.saturating_sub(1);
                let x1 = (x + 1).min(width - 1);
                let mut sum = 0_u32;
                let mut count = 0_u32;
                for sample_y in y0..=y1 {
                    for sample_x in x0..=x1 {
                        sum += u32::from(reduced[sample_y * width + sample_x]);
                        count += 1;
                    }
                }
                *value = (sum / count) as u8;
            }
        });
    (!cancel.load(Ordering::Relaxed)).then_some(ReducedToneLuma {
        pixels,
        width,
        height,
        factor,
    })
}

fn tone_density_luma(
    luma: &[u8],
    width: usize,
    height: usize,
    method: ToneDensityMethod,
    radius: f32,
    cancel: &AtomicBool,
) -> Option<Option<Vec<u8>>> {
    if method == ToneDensityMethod::Off {
        return Some(None);
    }
    let radius = if radius.is_finite() {
        radius.clamp(0.0, MAX_EFFECTIVE_TONE_RADIUS)
    } else {
        default_tone_radius()
    };
    let lower_radius = radius.floor() as usize;
    let upper_radius = radius.ceil() as usize;
    if upper_radius == lower_radius {
        let lower = tone_density_luma_at_radius(luma, width, height, method, lower_radius, cancel)?;
        return Some(Some(lower));
    }
    let shared_gaussian_pair = if method == ToneDensityMethod::Gaussian
        && lower_radius > 0
        && gaussian_first_radius(lower_radius) == gaussian_first_radius(upper_radius)
    {
        Some(gaussian_approx_adjacent_with_shared_first_pass(
            luma,
            width,
            height,
            lower_radius,
            upper_radius,
            cancel,
        )?)
    } else {
        None
    };
    let (lower, upper) = if let Some(pair) = shared_gaussian_pair {
        pair
    } else {
        (
            tone_density_luma_at_radius(luma, width, height, method, lower_radius, cancel)?,
            tone_density_luma_at_radius(luma, width, height, method, upper_radius, cancel)?,
        )
    };
    let fraction = radius - lower_radius as f32;
    let blended = lower
        .into_par_iter()
        .zip(upper.into_par_iter())
        .map(|(a, b)| {
            (f32::from(a) * (1.0 - fraction) + f32::from(b) * fraction)
                .round()
                .clamp(0.0, 255.0) as u8
        })
        .collect();
    (!cancel.load(Ordering::Relaxed)).then_some(Some(blended))
}

fn tone_density_luma_at_radius(
    luma: &[u8],
    width: usize,
    height: usize,
    method: ToneDensityMethod,
    radius: usize,
    cancel: &AtomicBool,
) -> Option<Vec<u8>> {
    if radius == 0 {
        return Some(luma.to_vec());
    }
    let result = match method {
        ToneDensityMethod::Off => return Some(luma.to_vec()),
        ToneDensityMethod::Fast => unreachable!("fast tone uses reduced-resolution path"),
        ToneDensityMethod::LocalMean => box_blur(luma, width, height, radius, cancel)?,
        ToneDensityMethod::Gaussian => gaussian_approx(luma, width, height, radius, cancel)?,
    };
    Some(result)
}

fn horizontal_box_blur(
    src: &[u8],
    width: usize,
    _height: usize,
    radius: usize,
    cancel: &AtomicBool,
) -> Option<Vec<u8>> {
    let mut out = vec![0_u8; src.len()];
    out.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let source = &src[y * width..(y + 1) * width];
        let mut sum = 0_u32;
        let mut right = radius.min(width - 1);
        for value in &source[..=right] {
            sum += u32::from(*value);
        }
        let mut left = 0_usize;
        for x in 0..width {
            row[x] = (sum / (right - left + 1) as u32) as u8;
            let next_right = x.saturating_add(radius).saturating_add(1);
            if next_right < width {
                right = next_right;
                sum += u32::from(source[right]);
            }
            if x >= radius {
                sum -= u32::from(source[left]);
                left += 1;
            }
        }
    });
    (!cancel.load(Ordering::Relaxed)).then_some(out)
}

fn transpose(src: &[u8], width: usize, height: usize, cancel: &AtomicBool) -> Option<Vec<u8>> {
    let mut out = vec![0_u8; src.len()];
    out.par_chunks_mut(height)
        .enumerate()
        .for_each(|(x, column)| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            for y in 0..height {
                column[y] = src[y * width + x];
            }
        });
    (!cancel.load(Ordering::Relaxed)).then_some(out)
}

fn box_blur(
    src: &[u8],
    width: usize,
    height: usize,
    radius: usize,
    cancel: &AtomicBool,
) -> Option<Vec<u8>> {
    if width == 0 || height == 0 {
        return Some(Vec::new());
    }
    let horizontal = horizontal_box_blur(src, width, height, radius.min(width - 1), cancel)?;
    let transposed = transpose(&horizontal, width, height, cancel)?;
    drop(horizontal);
    let transposed_blurred =
        horizontal_box_blur(&transposed, height, width, radius.min(height - 1), cancel)?;
    drop(transposed);
    transpose(&transposed_blurred, height, width, cancel)
}

fn gaussian_approx(
    src: &[u8],
    width: usize,
    height: usize,
    radius: usize,
    cancel: &AtomicBool,
) -> Option<Vec<u8>> {
    let r0 = (radius / 2).max(1);
    let r1 = ((radius + 1) / 2).max(1);
    let pass0 = box_blur(src, width, height, r0, cancel)?;
    let pass1 = box_blur(&pass0, width, height, r1, cancel)?;
    drop(pass0);
    box_blur(&pass1, width, height, r0, cancel)
}

#[inline]
fn gaussian_first_radius(radius: usize) -> usize {
    (radius / 2).max(1)
}

/// 隣接する整数半径のガウシアン近似で第1 box blur が同じ場合、その結果を共有する。
///
/// 長辺基準の検出スケールは多くの画像で小数半径になる。従来は floor / ceil の
/// 3-pass 近似を独立に計6回実行してから補間していたが、たとえば半径 2.x は
/// radius=2 (`1,1,1`) と radius=3 (`1,2,1`) の先頭 radius=1 が同一である。
/// この1回を共有しても各整数半径の画素値と最終補間結果は変わらない。
fn gaussian_approx_adjacent_with_shared_first_pass(
    src: &[u8],
    width: usize,
    height: usize,
    lower_radius: usize,
    upper_radius: usize,
    cancel: &AtomicBool,
) -> Option<(Vec<u8>, Vec<u8>)> {
    debug_assert_eq!(upper_radius, lower_radius + 1);
    let lower_r0 = gaussian_first_radius(lower_radius);
    let upper_r0 = gaussian_first_radius(upper_radius);
    debug_assert_eq!(lower_r0, upper_r0);
    let lower_r1 = lower_radius.div_ceil(2).max(1);
    let upper_r1 = upper_radius.div_ceil(2).max(1);

    let shared = box_blur(src, width, height, lower_r0, cancel)?;
    let lower_pass1 = box_blur(&shared, width, height, lower_r1, cancel)?;
    let lower = box_blur(&lower_pass1, width, height, lower_r0, cancel)?;
    let upper_pass1 = box_blur(&shared, width, height, upper_r1, cancel)?;
    let upper = box_blur(&upper_pass1, width, height, upper_r0, cancel)?;
    Some((lower, upper))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: usize, height: usize, f: impl Fn(usize, usize) -> Color32) -> ColorImage {
        let f = &f;
        ColorImage::new(
            [width, height],
            (0..height)
                .flat_map(|y| (0..width).map(move |x| f(x, y)))
                .collect(),
        )
    }

    fn legacy_is_near_monochrome(src: &ColorImage, tolerance: u8) -> bool {
        let total = src.pixels.len();
        if total == 0 {
            return true;
        }
        let stride = total.div_ceil(MONO_SAMPLE_LIMIT).max(1);
        let samples: Vec<[f32; 3]> = src
            .pixels
            .iter()
            .step_by(stride)
            .filter(|pixel| pixel.a() >= 16)
            .take(MONO_SAMPLE_LIMIT)
            .map(|pixel| [pixel.r() as f32, pixel.g() as f32, pixel.b() as f32])
            .collect();
        if samples.len() < 8 {
            return true;
        }

        let inv_n = 1.0 / samples.len() as f32;
        let mut mean = [0.0_f32; 3];
        for sample in &samples {
            for channel in 0..3 {
                mean[channel] += sample[channel] * inv_n;
            }
        }
        let mut covariance = [[0.0_f32; 3]; 3];
        for sample in &samples {
            let d = [
                sample[0] - mean[0],
                sample[1] - mean[1],
                sample[2] - mean[2],
            ];
            for row in 0..3 {
                for col in 0..3 {
                    covariance[row][col] += d[row] * d[col] * inv_n;
                }
            }
        }

        let total_variance = covariance[0][0] + covariance[1][1] + covariance[2][2];
        if total_variance <= 1.0 {
            return true;
        }
        let mut axis = [0.577_350_26_f32; 3];
        for _ in 0..12 {
            let next = [
                covariance[0][0] * axis[0]
                    + covariance[0][1] * axis[1]
                    + covariance[0][2] * axis[2],
                covariance[1][0] * axis[0]
                    + covariance[1][1] * axis[1]
                    + covariance[1][2] * axis[2],
                covariance[2][0] * axis[0]
                    + covariance[2][1] * axis[1]
                    + covariance[2][2] * axis[2],
            ];
            let length = (next[0] * next[0] + next[1] * next[1] + next[2] * next[2]).sqrt();
            if length <= 1e-5 {
                return true;
            }
            axis = [next[0] / length, next[1] / length, next[2] / length];
        }

        let tolerance_sq = f32::from(tolerance).powi(2);
        let inliers = samples
            .iter()
            .filter(|sample| {
                let d = [
                    sample[0] - mean[0],
                    sample[1] - mean[1],
                    sample[2] - mean[2],
                ];
                let projection = d[0] * axis[0] + d[1] * axis[1] + d[2] * axis[2];
                let residual_sq =
                    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2] - projection * projection).max(0.0);
                residual_sq <= tolerance_sq
            })
            .count();
        inliers as f32 / samples.len() as f32 >= MONO_INLIER_RATIO
    }

    #[test]
    fn legacy_palette_matches_post_filter() {
        let source = image(256, 1, |x, _| Color32::from_gray(x as u8));
        for (palette, filter) in [
            (
                ColorizePalette::Legacy4Color,
                crate::adjustment::PostFilter::PseudoColor4,
            ),
            (
                ColorizePalette::LegacySkin,
                crate::adjustment::PostFilter::PseudoColorSkin,
            ),
        ] {
            let params = ColorizeParams::legacy_all_images(palette);
            assert_eq!(
                apply(&source, &params),
                crate::post_filter::apply(&source, filter)
            );
        }
    }

    #[test]
    fn tinted_grayscale_is_detected_but_color_grid_is_not() {
        let tinted = image(64, 64, |x, y| {
            let v = ((x + y) * 2) as u8;
            Color32::from_rgb(v, v.saturating_add(18), v.saturating_add(28))
        });
        assert!(is_near_monochrome(&tinted, 12));

        let colored = image(64, 64, |x, y| match (x / 16 + y / 16) % 4 {
            0 => Color32::RED,
            1 => Color32::GREEN,
            2 => Color32::BLUE,
            _ => Color32::YELLOW,
        });
        assert!(!is_near_monochrome(&colored, 12));
    }

    #[test]
    fn mono_tone_axis_returns_mean_axis_and_same_residual_summary() {
        let source = image(64, 64, |x, y| {
            let v = ((x * 3 + y * 2) % 180) as u8;
            Color32::from_rgb(v, v.saturating_add(20), v.saturating_add(35))
        });

        let summary = mono_tone_axis(&source).expect("tinted grayscale has a stable color axis");
        assert_eq!(summary.p95_residual, near_monochrome_p95_residual(&source));
        assert!(summary.axis.iter().all(|component| component.is_finite()));
        assert!(summary.mean.iter().all(|component| component.is_finite()));
    }

    #[test]
    fn mono_tone_axis_is_none_when_the_axis_cannot_be_estimated() {
        let tiny = image(2, 2, |x, y| {
            Color32::from_rgb((x * 127) as u8, (y * 127) as u8, 80)
        });
        let uniform = ColorImage::filled([64, 64], Color32::from_rgb(210, 190, 150));

        assert_eq!(mono_tone_axis(&tiny), None);
        assert_eq!(mono_tone_axis(&uniform), None);
        assert_eq!(near_monochrome_p95_residual(&tiny), 0.0);
        assert_eq!(near_monochrome_p95_residual(&uniform), 0.0);
    }

    #[test]
    fn p95_residual_comparison_matches_legacy_inlier_ratio() {
        let cases = [
            image(64, 64, |x, y| {
                Color32::from_gray(((x * 3 + y * 2) % 256) as u8)
            }),
            image(64, 64, |x, y| match (x / 16 + y / 16) % 4 {
                0 => Color32::RED,
                1 => Color32::GREEN,
                2 => Color32::BLUE,
                _ => Color32::YELLOW,
            }),
            image(64, 64, |x, y| {
                let v = ((x * 3 + y * 2) % 220) as u8;
                Color32::from_rgb(v, v.saturating_add(12), v.saturating_add(24))
            }),
            image(2, 2, |x, y| {
                Color32::from_rgb((x * 127) as u8, (y * 127) as u8, 80)
            }),
            ColorImage::filled([64, 64], Color32::from_rgb(210, 190, 150)),
        ];

        for source in cases {
            let residual = near_monochrome_p95_residual(&source);
            for tolerance in 0..=u8::MAX {
                let legacy = legacy_is_near_monochrome(&source, tolerance);
                assert_eq!(is_near_monochrome_residual(residual, tolerance), legacy);
                assert_eq!(is_near_monochrome(&source, tolerance), legacy);
            }
        }
    }

    #[test]
    fn density_normalization_interpolates_auto_levels() {
        let source = image(200, 1, |x, _| {
            Color32::from_gray(if x < 100 { 40 } else { 220 })
        });
        let cancel = AtomicBool::new(false);

        let mut full = vec![40_u8; 100];
        full.extend(std::iter::repeat_n(220_u8, 100));
        normalize_density_luma(&source, &mut full, 100, &cancel).unwrap();
        assert!(full[..100].iter().all(|value| *value == 0));
        assert!(full[100..].iter().all(|value| *value == 255));

        let mut half = vec![40_u8; 100];
        half.extend(std::iter::repeat_n(220_u8, 100));
        normalize_density_luma(&source, &mut half, 50, &cancel).unwrap();
        assert!(half[..100].iter().all(|value| *value == 20));
        assert!(half[100..].iter().all(|value| *value == 238));
    }

    #[test]
    fn density_normalization_does_not_clip_small_samples() {
        let values = [20_u8, 50, 80, 110, 140, 170, 200, 240];
        let source = ColorImage::new(
            [values.len(), 1],
            values
                .iter()
                .map(|value| Color32::from_gray(*value))
                .collect(),
        );
        let cancel = AtomicBool::new(false);
        let mut luma = values.to_vec();
        assert_eq!(
            density_normalization_bounds(&source, &luma),
            Some((20, 240))
        );
        normalize_density_luma(&source, &mut luma, 100, &cancel).unwrap();
        assert_eq!(luma[0], 0);
        assert_eq!(luma[luma.len() - 1], 255);
    }

    #[test]
    fn density_normalization_ignores_transparent_samples() {
        let mut pixels = vec![Color32::TRANSPARENT; 8];
        pixels.extend(std::iter::repeat_n(Color32::from_gray(40), 4));
        pixels.extend(std::iter::repeat_n(Color32::from_gray(220), 4));
        let source = ColorImage::new([pixels.len(), 1], pixels);
        let mut luma = vec![0_u8, 255, 0, 255, 0, 255, 0, 255];
        luma.extend(std::iter::repeat_n(40_u8, 4));
        luma.extend(std::iter::repeat_n(220_u8, 4));
        assert_eq!(
            density_normalization_bounds(&source, &luma),
            Some((40, 220))
        );
    }

    #[test]
    fn density_normalization_skips_nearly_flat_images() {
        let source = image(8, 1, |x, _| {
            Color32::from_gray(if x % 2 == 0 { 100 } else { 110 })
        });
        let cancel = AtomicBool::new(false);
        let mut luma = vec![100_u8, 110, 100, 110, 100, 110, 100, 110];
        let original = luma.clone();
        assert_eq!(density_normalization_bounds(&source, &luma), None);
        normalize_density_luma(&source, &mut luma, 100, &cancel).unwrap();
        assert_eq!(luma, original);
    }

    #[test]
    fn monochrome_only_density_normalization_leaves_color_images_unchanged() {
        let colored = image(64, 64, |x, y| match (x / 16 + y / 16) % 4 {
            0 => Color32::RED,
            1 => Color32::GREEN,
            2 => Color32::BLUE,
            _ => Color32::YELLOW,
        });
        let params = ColorizeParams {
            mode: ColorizeMode::MonochromeOnly,
            density_normalization_strength: 100,
            ..ColorizeParams::default()
        };
        assert_eq!(apply(&colored, &params), colored);
    }

    #[test]
    fn weak_and_strong_tone_modes_have_ordered_smoothing() {
        let width = 65;
        let height = 65;
        let mut luma = vec![255_u8; width * height];
        luma[(height / 2) * width + width / 2] = 0;
        let cancel = AtomicBool::new(false);
        let weak = tone_density_luma(
            &luma,
            width,
            height,
            ToneDensityMethod::LocalMean,
            1.0,
            &cancel,
        )
        .unwrap()
        .unwrap();
        let strong = tone_density_luma(
            &luma,
            width,
            height,
            ToneDensityMethod::Gaussian,
            1.0,
            &cancel,
        )
        .unwrap()
        .unwrap();
        let center = (height / 2) * width + width / 2;
        let two_pixels_away = center + 2;
        assert!(weak[center] < strong[center]);
        assert_eq!(weak[two_pixels_away], 255);
        assert!(strong[two_pixels_away] < 255);
    }

    #[test]
    fn fast_tone_preserves_constant_luma_at_fractional_scale() {
        let width = 37;
        let height = 29;
        let luma = vec![123_u8; width * height];
        let cancel = AtomicBool::new(false);
        let tone = fast_tone_density_luma(&luma, width, height, 2.75, &cancel).unwrap();
        for index in 0..luma.len() {
            let sampled =
                (tone.sample(index, width, f32::from(luma[index]) / 255.0) * 255.0).round() as u8;
            assert_eq!(sampled, 123);
        }
    }

    #[test]
    fn fast_tone_subpixel_scale_blends_from_original() {
        let width = 17;
        let height = 17;
        let mut luma = vec![255_u8; width * height];
        let center = (height / 2) * width + width / 2;
        luma[center] = 0;
        let cancel = AtomicBool::new(false);
        let full = fast_tone_density_luma(&luma, width, height, 1.0, &cancel).unwrap();
        let quarter = fast_tone_density_luma(&luma, width, height, 0.25, &cancel).unwrap();
        let full_center = full.sample(center, width, 0.0);
        let quarter_center = quarter.sample(center, width, 0.0);
        assert!((quarter_center - full_center * 0.25).abs() < 1e-6);
    }

    #[test]
    fn fast_colorize_keeps_size_and_alpha() {
        let source = image(64, 48, |x, y| {
            Color32::from_rgba_unmultiplied(
                ((x * 5 + y * 3) % 256) as u8,
                ((x * 5 + y * 3) % 256) as u8,
                ((x * 5 + y * 3) % 256) as u8,
                ((x + y) % 256) as u8,
            )
        });
        let params = ColorizeParams {
            mode: ColorizeMode::AllImages,
            tone_method: ToneDensityMethod::Fast,
            tone_radius: 1.0,
            ..ColorizeParams::default()
        };
        let output = apply(&source, &params);
        assert_eq!(output.size, source.size);
        for (actual, original) in output.pixels.iter().zip(&source.pixels) {
            assert_eq!(actual.a(), original.a());
        }
    }

    #[test]
    fn fractional_tone_radius_interpolates_adjacent_results() {
        let width = 48;
        let height = 32;
        let luma: Vec<u8> = (0..width * height)
            .map(|index| {
                let x = index % width;
                let y = index / width;
                if ((x / 3) + (y / 2)) % 2 == 0 {
                    24
                } else {
                    236
                }
            })
            .collect();
        let cancel = AtomicBool::new(false);
        let at_four = tone_density_luma(
            &luma,
            width,
            height,
            ToneDensityMethod::LocalMean,
            4.0,
            &cancel,
        )
        .unwrap()
        .unwrap();
        let at_five = tone_density_luma(
            &luma,
            width,
            height,
            ToneDensityMethod::LocalMean,
            5.0,
            &cancel,
        )
        .unwrap()
        .unwrap();
        let at_half = tone_density_luma(
            &luma,
            width,
            height,
            ToneDensityMethod::LocalMean,
            4.5,
            &cancel,
        )
        .unwrap()
        .unwrap();
        assert!(at_four.iter().zip(&at_five).any(|(a, b)| a != b));
        for ((a, b), middle) in at_four.iter().zip(&at_five).zip(&at_half) {
            let expected = (f32::from(*a) * 0.5 + f32::from(*b) * 0.5).round() as i16;
            assert!((i16::from(*middle) - expected).abs() <= 1);
        }
    }

    #[test]
    fn fractional_gaussian_reuses_first_pass_without_changing_pixels() {
        let width = 48;
        let height = 32;
        let luma: Vec<u8> = (0..width * height)
            .map(|index| {
                let x = index % width;
                let y = index / width;
                if ((x / 3) + (y / 2)) % 2 == 0 {
                    24
                } else {
                    236
                }
            })
            .collect();
        let cancel = AtomicBool::new(false);
        let lower = gaussian_approx(&luma, width, height, 2, &cancel).unwrap();
        let upper = gaussian_approx(&luma, width, height, 3, &cancel).unwrap();
        let fraction = 0.75_f32;
        let expected: Vec<u8> = lower
            .into_iter()
            .zip(upper)
            .map(|(a, b)| {
                (f32::from(a) * (1.0 - fraction) + f32::from(b) * fraction)
                    .round()
                    .clamp(0.0, 255.0) as u8
            })
            .collect();
        let actual = tone_density_luma(
            &luma,
            width,
            height,
            ToneDensityMethod::Gaussian,
            2.75,
            &cancel,
        )
        .unwrap()
        .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn subpixel_tone_radius_interpolates_from_the_original() {
        let width = 17;
        let height = 17;
        let mut luma = vec![255_u8; width * height];
        let center = (height / 2) * width + width / 2;
        luma[center] = 0;
        let cancel = AtomicBool::new(false);
        let at_one = tone_density_luma(
            &luma,
            width,
            height,
            ToneDensityMethod::LocalMean,
            1.0,
            &cancel,
        )
        .unwrap()
        .unwrap();
        let at_tenth = tone_density_luma(
            &luma,
            width,
            height,
            ToneDensityMethod::LocalMean,
            0.1,
            &cancel,
        )
        .unwrap()
        .unwrap();
        let expected =
            (f32::from(luma[center]) * 0.9 + f32::from(at_one[center]) * 0.1).round() as i16;
        assert!((i16::from(at_tenth[center]) - expected).abs() <= 1);
    }

    #[test]
    fn tone_radius_scales_with_image_long_edge() {
        assert_eq!(effective_tone_radius(4.0, [2048, 1200]), 4.0);
        assert_eq!(effective_tone_radius(4.0, [1024, 800]), 2.0);
        assert_eq!(effective_tone_radius(4.0, [8192, 4000]), 16.0);
        assert!((effective_tone_radius(0.1, [1024, 800]) - 0.05).abs() < f32::EPSILON);
    }

    #[test]
    fn older_tone_settings_load_without_removed_fields() {
        let mut params: ColorizeParams = serde_json::from_str(
            r#"{
                    "tone_radius": 4,
                    "tone_method": "edge_preserving",
                    "tone_line_protection": 100,
                    "tone_periodicity_gate": true,
                    "tone_periodicity_threshold": 55
                }"#,
        )
        .unwrap();
        params.sanitize();
        assert_eq!(params.tone_radius, 4.0);
        assert_eq!(params.tone_method, ToneDensityMethod::Gaussian);
        assert_eq!(params.luminance_weight, 100);
        assert_eq!(params.density_normalization_strength, 0);
        assert_eq!(
            ColorizeParams::legacy_all_images(ColorizePalette::Legacy4Color).luminance_weight,
            0
        );

        let multiscale: ColorizeParams =
            serde_json::from_str(r#"{"tone_method":"multi_scale"}"#).unwrap();
        assert_eq!(multiscale.tone_method, ToneDensityMethod::Gaussian);
    }

    #[test]
    fn custom_lut_uses_endpoints() {
        let params = ColorizeParams {
            mode: ColorizeMode::AllImages,
            palette: ColorizePalette::Custom,
            luminance_weight: 0,
            control_points: vec![
                ColorizeControlPoint::new([10, 20, 30], 1.0),
                ColorizeControlPoint::new([210, 220, 230], 1.0),
            ],
            ..ColorizeParams::default()
        };
        let source = image(2, 1, |x, _| {
            if x == 0 {
                Color32::BLACK
            } else {
                Color32::WHITE
            }
        });
        let output = apply(&source, &params);
        assert_eq!(output.pixels[0], Color32::from_rgb(10, 20, 30));
        assert_eq!(output.pixels[1], Color32::from_rgb(210, 220, 230));
    }

    #[test]
    fn preview_lut_matches_grayscale_image_colorization() {
        let source = image(256, 1, |x, _| Color32::from_gray(x as u8));
        let params = ColorizeParams {
            mode: ColorizeMode::AllImages,
            palette: ColorizePalette::Custom,
            luminance_weight: 37,
            control_points: vec![
                ColorizeControlPoint::new([4, 8, 18], 2.0),
                ColorizeControlPoint::new([200, 70, 55], 1.0),
                ColorizeControlPoint::new([245, 235, 210], 3.0),
            ],
            ..ColorizeParams::default()
        };

        let preview = preview_lut(&params);
        let output = apply(&source, &params);
        for (index, pixel) in output.pixels.iter().enumerate() {
            assert_eq!(
                [pixel.r(), pixel.g(), pixel.b()],
                preview[index],
                "preview must use the same mapping as a grayscale input at {index}"
            );
        }
    }

    #[test]
    fn quantized_luminance_lut_differs_by_at_most_one_level() {
        let params = ColorizeParams {
            palette: ColorizePalette::Custom,
            ..ColorizeParams::default()
        };
        let lut = build_lut(&params);
        for weight in [0.0_f32, 0.37, 1.0] {
            for index in 0..256 {
                let quantized_y = index as f32 / 255.0;
                let precomputed = preserve_luminance(lut[index], quantized_y, weight);
                for delta in [-0.499_f32, 0.499] {
                    let exact_y = ((index as f32 + delta) / 255.0).clamp(0.0, 1.0);
                    let exact = preserve_luminance(lut[index], exact_y, weight);
                    for channel in 0..3 {
                        assert!(
                            (i16::from(precomputed[channel]) - i16::from(exact[channel])).abs()
                                <= 1
                        );
                    }
                }
            }
        }
    }

    /// Manual measurement for page-turn thumbnail effect costs.
    /// Run with: `cargo test --release -p mimageviewer --lib
    /// thumbnail_effect_cost_measurement -- --ignored --nocapture`
    #[test]
    #[ignore = "manual performance measurement; run with --release and --nocapture"]
    fn thumbnail_effect_cost_measurement() {
        use std::hint::black_box;
        use std::time::Instant;

        const RUNS: usize = 15;
        const SIZES: &[(usize, usize)] = &[(347, 506), (800, 1200), (1123, 1648), (2480, 3508)];

        fn measure_ms<T>(runs: usize, mut operation: impl FnMut() -> T) -> (f64, f64) {
            // Discard allocator and Rayon pool initialization effects.
            black_box(operation());
            let mut samples = Vec::with_capacity(runs);
            for _ in 0..runs {
                let started = Instant::now();
                let output = operation();
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                black_box(output);
                samples.push(elapsed_ms);
            }
            samples.sort_unstable_by(f64::total_cmp);
            let median = if runs % 2 == 0 {
                (samples[runs / 2 - 1] + samples[runs / 2]) * 0.5
            } else {
                samples[runs / 2]
            };
            (median, *samples.last().expect("at least one measured run"))
        }

        let master = image::load_from_memory(include_bytes!(
            "../samples/tone-algorithm-comparison/01_source.png"
        ))
        .expect("decode deterministic monochrome screentone fixture");
        let adjust_params = crate::adjustment::AdjustParams {
            contrast: 20.0,
            ..crate::adjustment::AdjustParams::default()
        };
        let colorize_params = ColorizeParams {
            mode: ColorizeMode::AllImages,
            palette: ColorizePalette::Legacy4Color,
            luminance_weight: 100,
            density_normalization_strength: 0,
            tone_method: ToneDensityMethod::Gaussian,
            tone_radius: 1.0,
            tone_strength: 100,
            ..ColorizeParams::default()
        };
        let cancel = AtomicBool::new(false);

        println!(
            "input=samples/tone-algorithm-comparison/01_source.png source={}x{} runs={} warmup_discarded=1 release={} rayon_threads={}",
            master.width(),
            master.height(),
            RUNS,
            !cfg!(debug_assertions),
            rayon::current_num_threads(),
        );
        println!(
            "adjustment=contrast:+20,colorize=all_images/legacy4color/luminance100/density_normalization0/gaussian/radius1.0/strength100"
        );
        println!(
            "size\tpixels\teffective_tone_radius\tadjust_median_ms\tadjust_max_ms\tcolorize_median_ms\tcolorize_max_ms\ttone_blur_median_ms\ttone_blur_max_ms"
        );

        for &(width, height) in SIZES {
            let resized = crate::fast_resize::resize_dynamic_exact(
                &master,
                width as u32,
                height as u32,
                crate::fast_resize::Quality::Lanczos3,
            )
            .to_rgba8();
            let source = ColorImage::from_rgba_unmultiplied([width, height], resized.as_raw());
            assert_eq!(source.size, [width, height]);
            let luma: Vec<u8> = source
                .pixels
                .par_iter()
                .map(|pixel| {
                    (crate::adjustment::pixel_lum_f32(*pixel) * 255.0)
                        .round()
                        .clamp(0.0, 255.0) as u8
                })
                .collect();
            let tone_radius = effective_tone_radius(colorize_params.tone_radius, source.size);

            let (adjust_median, adjust_max) = measure_ms(RUNS, || {
                crate::adjustment::apply_adjustments_fast(
                    black_box(&source),
                    black_box(&adjust_params),
                )
            });
            let (colorize_median, colorize_max) = measure_ms(RUNS, || {
                apply_applicable_with_cancel(
                    black_box(&source),
                    black_box(&colorize_params),
                    black_box(&cancel),
                )
                .expect("measurement is never cancelled")
            });
            let (tone_median, tone_max) = measure_ms(RUNS, || {
                tone_density_luma(
                    black_box(&luma),
                    width,
                    height,
                    ToneDensityMethod::Gaussian,
                    tone_radius,
                    black_box(&cancel),
                )
                .expect("measurement is never cancelled")
                .expect("Gaussian tone density returns a buffer")
            });

            println!(
                "{}x{}\t{}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}",
                width,
                height,
                width * height,
                tone_radius,
                adjust_median,
                adjust_max,
                colorize_median,
                colorize_max,
                tone_median,
                tone_max,
            );
        }
    }
}
