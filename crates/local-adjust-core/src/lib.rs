//! Core image operations for local adjustment layer prototypes.
//!
//! The public boundary is intentionally small: RGBA input plus an ordered list
//! of local adjustment layers returns an RGBA image with the same dimensions.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalAdjustError {
    InvalidImageBuffer { expected: usize, actual: usize },
    InvalidMaskBuffer { expected: usize, actual: usize },
    Cancelled,
}

impl fmt::Display for LocalAdjustError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidImageBuffer { expected, actual } => {
                write!(
                    f,
                    "invalid RGBA buffer length: expected {expected}, got {actual}"
                )
            }
            Self::InvalidMaskBuffer { expected, actual } => {
                write!(
                    f,
                    "invalid mask buffer length: expected {expected}, got {actual}"
                )
            }
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for LocalAdjustError {}

pub type Result<T> = std::result::Result<T, LocalAdjustError>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalAdjustProgress {
    pub layer_index: usize,
    pub layer_count: usize,
    pub effect_name: &'static str,
    pub percent: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RgbaImageBuf {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl RgbaImageBuf {
    pub fn new(width: usize, height: usize, pixels: Vec<u8>) -> Result<Self> {
        let expected = width.saturating_mul(height).saturating_mul(4);
        if pixels.len() != expected {
            return Err(LocalAdjustError::InvalidImageBuffer {
                expected,
                actual: pixels.len(),
            });
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn as_ref(&self) -> RgbaImageRef<'_> {
        RgbaImageRef {
            width: self.width,
            height: self.height,
            pixels: &self.pixels,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RgbaImageRef<'a> {
    pub width: usize,
    pub height: usize,
    pub pixels: &'a [u8],
}

impl<'a> RgbaImageRef<'a> {
    pub fn validate(self) -> Result<Self> {
        let expected = self.width.saturating_mul(self.height).saturating_mul(4);
        if self.pixels.len() != expected {
            return Err(LocalAdjustError::InvalidImageBuffer {
                expected,
                actual: self.pixels.len(),
            });
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalAdjustmentLayer {
    pub name: String,
    pub enabled: bool,
    pub opacity: f32,
    pub mask: LocalMask,
    #[serde(default, skip_serializing_if = "ManualMaskOverride::is_empty")]
    pub manual_override: ManualMaskOverride,
    pub mask_inverted: bool,
    pub mask_expand_px: f32,
    pub mask_feather_px: f32,
    pub effect: LocalEffect,
}

impl LocalAdjustmentLayer {
    pub fn new(name: impl Into<String>, mask: LocalMask, effect: LocalEffect) -> Self {
        Self {
            name: name.into(),
            enabled: true,
            opacity: 1.0,
            mask,
            manual_override: ManualMaskOverride::default(),
            mask_inverted: false,
            mask_expand_px: 0.0,
            mask_feather_px: 0.0,
            effect,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ManualMaskOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<RasterVectorMask>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtract: Option<RasterVectorMask>,
}

impl ManualMaskOverride {
    pub fn is_empty(&self) -> bool {
        self.add.is_none() && self.subtract.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LocalMask {
    Full,
    Raster(RasterMask),
    /// Manual mask data for brush, lasso, and editable vector shapes.
    ///
    /// This is intentionally independent from local adjustment UI so the same
    /// mask engine can later back eraser, conceal, and local adjustment tools.
    RasterVector(RasterVectorMask),
    LinearGradient(LinearGradientMask),
    RadialGradient(RadialGradientMask),
    LumaRange(RangeMask),
    ColorRange(ColorRangeMask),
    /// Foreground/background matte from a salient-object or character matting model.
    Subject(RasterMask),
    /// Region candidates that can be toggled independently.
    Segmentation(RegionMask),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RasterMask {
    pub width: usize,
    pub height: usize,
    pub alpha: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionMask {
    pub width: usize,
    pub height: usize,
    /// 0 means no region / background. Positive labels index into `selected`.
    pub labels: Vec<u32>,
    pub selected: Vec<bool>,
}

impl RegionMask {
    pub fn empty(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            labels: vec![0; width.saturating_mul(height)],
            selected: vec![false],
        }
    }

    pub fn label_count(&self) -> usize {
        self.selected.len().saturating_sub(1)
    }

    pub fn validate(&self, width: usize, height: usize) -> Result<()> {
        let expected = width.saturating_mul(height);
        if self.width != width || self.height != height || self.labels.len() != expected {
            return Err(LocalAdjustError::InvalidMaskBuffer {
                expected,
                actual: self.labels.len(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RasterVectorMask {
    pub width: usize,
    pub height: usize,
    /// Bitmap strokes and filled polygon results.
    pub alpha: Vec<f32>,
    /// Editable object mask shapes, applied on top of the bitmap alpha.
    pub shapes: Vec<MaskShape>,
}

impl RasterVectorMask {
    pub fn empty(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            alpha: vec![0.0; width.saturating_mul(height)],
            shapes: Vec::new(),
        }
    }

    pub fn validate(&self, width: usize, height: usize) -> Result<()> {
        let expected = width.saturating_mul(height);
        if self.width != width || self.height != height || self.alpha.len() != expected {
            return Err(LocalAdjustError::InvalidMaskBuffer {
                expected,
                actual: self.alpha.len(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeOp {
    Add,
    Subtract,
}

impl ShapeOp {
    pub fn is_add(self) -> bool {
        matches!(self, Self::Add)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineKind {
    Vertical,
    Horizontal,
    Diagonal,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MaskShape {
    Line {
        op: ShapeOp,
        kind: LineKind,
        p0: [f32; 2],
        p1: [f32; 2],
        thickness: f32,
    },
    Rect {
        op: ShapeOp,
        center: [f32; 2],
        half_w: f32,
        half_h: f32,
        rotation_rad: f32,
    },
    Ellipse {
        op: ShapeOp,
        center: [f32; 2],
        rx: f32,
        ry: f32,
        rotation_rad: f32,
    },
}

impl MaskShape {
    pub fn op(self) -> ShapeOp {
        match self {
            Self::Line { op, .. } | Self::Rect { op, .. } | Self::Ellipse { op, .. } => op,
        }
    }

    pub fn with_op(mut self, new_op: ShapeOp) -> Self {
        match &mut self {
            Self::Line { op, .. } | Self::Rect { op, .. } | Self::Ellipse { op, .. } => {
                *op = new_op;
            }
        }
        self
    }

    pub fn center(self) -> [f32; 2] {
        match self {
            Self::Line { p0, p1, .. } => [(p0[0] + p1[0]) * 0.5, (p0[1] + p1[1]) * 0.5],
            Self::Rect { center, .. } | Self::Ellipse { center, .. } => center,
        }
    }
}

impl RasterMask {
    pub fn empty(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            alpha: vec![0.0; width.saturating_mul(height)],
        }
    }

    pub fn full(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            alpha: vec![1.0; width.saturating_mul(height)],
        }
    }

    pub fn validate(&self, width: usize, height: usize) -> Result<()> {
        let expected = width.saturating_mul(height);
        if self.width != width || self.height != height || self.alpha.len() != expected {
            return Err(LocalAdjustError::InvalidMaskBuffer {
                expected,
                actual: self.alpha.len(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LinearGradientMask {
    pub initialized: bool,
    pub start: [f32; 2],
    pub end: [f32; 2],
}

impl Default for LinearGradientMask {
    fn default() -> Self {
        Self {
            initialized: false,
            start: [0.5, 0.5],
            end: [0.5, 0.5],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RadialGradientMask {
    pub initialized: bool,
    pub center: [f32; 2],
    pub inner_radius: f32,
    pub inner_radius_y: f32,
    pub outer_radius: f32,
    pub outer_radius_y: f32,
}

impl Default for RadialGradientMask {
    fn default() -> Self {
        Self {
            initialized: false,
            center: [0.5, 0.5],
            inner_radius: 0.0,
            inner_radius_y: 0.0,
            outer_radius: 0.0,
            outer_radius_y: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RangeMask {
    pub min: f32,
    pub max: f32,
    pub feather: f32,
}

impl Default for RangeMask {
    fn default() -> Self {
        Self {
            min: 0.35,
            max: 1.0,
            feather: 0.08,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorRangeMask {
    pub initialized: bool,
    pub target_rgb: [u8; 3],
    pub tolerance: f32,
    pub feather: f32,
}

impl Default for ColorRangeMask {
    fn default() -> Self {
        Self {
            initialized: false,
            target_rgb: [255, 255, 255],
            tolerance: 0.16,
            feather: 0.08,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LocalEffect {
    None,
    Tone(ToneParams),
    ToneCurve(ToneCurveParams),
    RgbToneCurve(RgbToneCurveParams),
    ColorBalance(ColorBalanceParams),
    ThreeWayColorGrading(ThreeWayColorGradingParams),
    SelectiveColor(SelectiveColorParams),
    ChannelMixer(ChannelMixerParams),
    Clarity(ClarityParams),
    Texture(TextureParams),
    HighPass(HighPassParams),
    HighlightsShadows(HighlightsShadowsParams),
    Dehaze(DehazeParams),
    Blur(BlurParams),
    MotionBlur(MotionBlurParams),
    TiltShift(TiltShiftParams),
    LensBlur(LensBlurParams),
    RadialBlur(RadialBlurParams),
    WaveDistortion(WaveDistortionParams),
    PinchSpherize(PinchSpherizeParams),
    SoftFocus(SoftFocusParams),
    Mosaic(MosaicParams),
    Sharpen(SharpenParams),
    SmartSharpen(SmartSharpenParams),
    Hsl(HslParams),
    ColorMixer(ColorMixerParams),
    Look(LookParams),
    CubeLut(CubeLutParams),
    Posterize(PosterizeParams),
    Threshold(ThresholdParams),
    Invert(InvertParams),
    Duotone(DuotoneParams),
    Equalize(EqualizeParams),
    GradientMap(GradientMapParams),
    Bloom(BloomParams),
    Vignette(VignetteParams),
    FilmGrain(FilmGrainParams),
    ChromaticAberration(ChromaticAberrationParams),
    Halftone(HalftoneParams),
    StarGlow(StarGlowParams),
    EdgeSmooth(EdgeSmoothParams),
    Median(MedianParams),
}

impl LocalEffect {
    fn progress_label(&self) -> &'static str {
        match self {
            Self::None => "効果なし",
            Self::Tone(_) => "色調補正",
            Self::ToneCurve(_) => "トーンカーブ",
            Self::RgbToneCurve(_) => "RGBトーンカーブ",
            Self::ColorBalance(_) => "カラーバランス",
            Self::ThreeWayColorGrading(_) => "3ウェイカラー",
            Self::SelectiveColor(_) => "セレクティブカラー",
            Self::ChannelMixer(_) => "チャンネルミキサー",
            Self::Clarity(_) => "明瞭度",
            Self::Texture(_) => "テクスチャ",
            Self::HighPass(_) => "ハイパス",
            Self::HighlightsShadows(_) => "ハイライト/シャドウ",
            Self::Dehaze(_) => "かすみ除去",
            Self::Blur(_) => "ぼかし",
            Self::MotionBlur(_) => "モーションぼかし",
            Self::TiltShift(_) => "チルトぼかし",
            Self::LensBlur(_) => "レンズぼかし",
            Self::RadialBlur(_) => "放射ぼかし",
            Self::WaveDistortion(_) => "波形ゆがみ",
            Self::PinchSpherize(_) => "つまむ/魚眼",
            Self::SoftFocus(_) => "ソフトフォーカス",
            Self::Mosaic(_) => "モザイク",
            Self::Sharpen(_) => "シャープ",
            Self::SmartSharpen(_) => "スマートシャープ",
            Self::Hsl(_) => "色相/HSL",
            Self::ColorMixer(_) => "カラーミキサー",
            Self::Look(_) => "ルック",
            Self::CubeLut(_) => "LUT",
            Self::Posterize(_) => "ポスタライズ",
            Self::Threshold(_) => "しきい値",
            Self::Invert(_) => "ネガ",
            Self::Duotone(_) => "デュオトーン",
            Self::Equalize(_) => "ヒストグラム均等化",
            Self::GradientMap(_) => "グラデーションマップ",
            Self::Bloom(_) => "ブルーム",
            Self::Vignette(_) => "ビネット",
            Self::FilmGrain(_) => "フィルム粒子",
            Self::ChromaticAberration(_) => "色収差",
            Self::Halftone(_) => "ハーフトーン",
            Self::StarGlow(_) => "クロス光",
            Self::EdgeSmooth(_) => "エッジ保持ぼかし",
            Self::Median(_) => "メディアンフィルタ",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ToneParams {
    pub brightness: f32,
    pub contrast: f32,
    pub gamma: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub temperature: f32,
    #[serde(default)]
    pub tint: f32,
}

impl Default for ToneParams {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 0.0,
            gamma: 1.0,
            saturation: 0.0,
            vibrance: 0.0,
            temperature: 0.0,
            tint: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ToneCurveParams {
    pub points: [f32; 5],
}

impl Default for ToneCurveParams {
    fn default() -> Self {
        Self {
            points: [0.0, 0.25, 0.5, 0.75, 1.0],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RgbToneCurveParams {
    pub master: [f32; 5],
    pub red: [f32; 5],
    pub green: [f32; 5],
    pub blue: [f32; 5],
}

impl Default for RgbToneCurveParams {
    fn default() -> Self {
        let identity = [0.0, 0.25, 0.5, 0.75, 1.0];
        Self {
            master: identity,
            red: identity,
            green: identity,
            blue: identity,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorBalanceRange {
    /// Positive values move toward red, negative values toward cyan.
    pub cyan_red: f32,
    /// Positive values move toward green, negative values toward magenta.
    pub magenta_green: f32,
    /// Positive values move toward blue, negative values toward yellow.
    pub yellow_blue: f32,
}

impl Default for ColorBalanceRange {
    fn default() -> Self {
        Self {
            cyan_red: 0.0,
            magenta_green: 0.0,
            yellow_blue: 0.0,
        }
    }
}

impl ColorBalanceRange {
    fn is_identity(self) -> bool {
        self.cyan_red.abs() <= f32::EPSILON
            && self.magenta_green.abs() <= f32::EPSILON
            && self.yellow_blue.abs() <= f32::EPSILON
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorBalanceParams {
    pub shadows: ColorBalanceRange,
    pub midtones: ColorBalanceRange,
    pub highlights: ColorBalanceRange,
    pub preserve_luma: bool,
}

impl Default for ColorBalanceParams {
    fn default() -> Self {
        Self {
            shadows: ColorBalanceRange::default(),
            midtones: ColorBalanceRange::default(),
            highlights: ColorBalanceRange::default(),
            preserve_luma: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorGradeWheel {
    pub hue_degrees: f32,
    pub saturation: f32,
    pub luminance: f32,
}

impl Default for ColorGradeWheel {
    fn default() -> Self {
        Self {
            hue_degrees: 0.0,
            saturation: 0.0,
            luminance: 0.0,
        }
    }
}

impl ColorGradeWheel {
    fn is_identity(self) -> bool {
        self.saturation.abs() <= f32::EPSILON && self.luminance.abs() <= f32::EPSILON
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ThreeWayColorGradingParams {
    pub shadows: ColorGradeWheel,
    pub midtones: ColorGradeWheel,
    pub highlights: ColorGradeWheel,
    pub balance: f32,
}

impl Default for ThreeWayColorGradingParams {
    fn default() -> Self {
        Self {
            shadows: ColorGradeWheel {
                hue_degrees: 220.0,
                ..Default::default()
            },
            midtones: ColorGradeWheel::default(),
            highlights: ColorGradeWheel {
                hue_degrees: 40.0,
                ..Default::default()
            },
            balance: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SelectiveColorParams {
    pub target_hue_degrees: f32,
    pub range_degrees: f32,
    pub feather_degrees: f32,
    pub hue_degrees: f32,
    pub saturation: f32,
    pub lightness: f32,
}

impl Default for SelectiveColorParams {
    fn default() -> Self {
        Self {
            target_hue_degrees: 0.0,
            range_degrees: 18.0,
            feather_degrees: 16.0,
            hue_degrees: 0.0,
            saturation: 0.0,
            lightness: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChannelMixerParams {
    pub monochrome: bool,
    pub red_output: [f32; 3],
    pub green_output: [f32; 3],
    pub blue_output: [f32; 3],
    pub mono_output: [f32; 3],
}

impl Default for ChannelMixerParams {
    fn default() -> Self {
        Self {
            monochrome: false,
            red_output: [100.0, 0.0, 0.0],
            green_output: [0.0, 100.0, 0.0],
            blue_output: [0.0, 0.0, 100.0],
            mono_output: [30.0, 59.0, 11.0],
        }
    }
}

impl ChannelMixerParams {
    fn is_identity(self) -> bool {
        !self.monochrome
            && self.red_output == [100.0, 0.0, 0.0]
            && self.green_output == [0.0, 100.0, 0.0]
            && self.blue_output == [0.0, 0.0, 100.0]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ClarityParams {
    /// Positive values increase local contrast; negative values soften it.
    pub amount: f32,
    pub radius_px: f32,
}

impl Default for ClarityParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius_px: 18.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TextureParams {
    /// Positive values enhance medium-frequency detail; negative values smooth it.
    pub amount: f32,
    pub radius_px: f32,
}

impl Default for TextureParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius_px: 10.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HighPassParams {
    /// Strength of the overlay high-pass sharpening. Ignored when detail_only is true.
    pub amount: f32,
    /// Blur radius used to separate low frequencies from detail.
    pub radius_px: f32,
    /// Contrast applied to the extracted detail before overlaying it.
    pub contrast: f32,
    /// Show the extracted high-pass plate around neutral gray instead of overlaying it.
    pub detail_only: bool,
}

impl Default for HighPassParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius_px: 8.0,
            contrast: 1.0,
            detail_only: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HighlightsShadowsParams {
    /// Positive values lift shadows; negative values deepen them.
    pub shadows: f32,
    /// Positive values recover/darken highlights; negative values brighten them.
    pub highlights: f32,
}

impl Default for HighlightsShadowsParams {
    fn default() -> Self {
        Self {
            shadows: 0.0,
            highlights: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DehazeParams {
    pub amount: f32,
    pub radius_px: f32,
    pub min_transmission: f32,
    pub saturation: f32,
}

impl Default for DehazeParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius_px: 14.0,
            min_transmission: 0.30,
            saturation: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BlurParams {
    pub radius_px: f32,
}

impl Default for BlurParams {
    fn default() -> Self {
        Self { radius_px: 0.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MotionBlurParams {
    pub distance_px: f32,
    pub angle_degrees: f32,
    pub strength: f32,
}

impl Default for MotionBlurParams {
    fn default() -> Self {
        Self {
            distance_px: 0.0,
            angle_degrees: 0.0,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TiltShiftMode {
    #[default]
    Linear,
    Radial,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TiltShiftParams {
    pub mode: TiltShiftMode,
    #[serde(default = "default_tilt_shift_mode_selected")]
    pub mode_selected: bool,
    #[serde(default = "default_tilt_shift_range_initialized")]
    pub range_initialized: bool,
    pub center: [f32; 2],
    pub angle_degrees: f32,
    pub focus_width: f32,
    pub falloff: f32,
    pub radius: [f32; 2],
    pub max_radius_px: f32,
    pub strength: f32,
    pub far_only: bool,
}

fn default_tilt_shift_range_initialized() -> bool {
    true
}

fn default_tilt_shift_mode_selected() -> bool {
    true
}

impl Default for TiltShiftParams {
    fn default() -> Self {
        Self {
            mode: TiltShiftMode::Linear,
            mode_selected: true,
            range_initialized: false,
            center: [0.5, 0.5],
            angle_degrees: -90.0,
            focus_width: 0.12,
            falloff: 0.32,
            radius: [0.32, 0.32],
            max_radius_px: 20.0,
            strength: 1.0,
            far_only: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LensBlurAperture {
    #[default]
    Circular,
    Hexagon,
    Octagon,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LensBlurParams {
    pub radius_px: f32,
    pub aperture: LensBlurAperture,
    pub rotation_degrees: f32,
    pub highlight_threshold: f32,
    pub highlight_boost: f32,
    pub strength: f32,
}

impl Default for LensBlurParams {
    fn default() -> Self {
        Self {
            radius_px: 0.0,
            aperture: LensBlurAperture::Circular,
            rotation_degrees: 0.0,
            highlight_threshold: 0.96,
            highlight_boost: 0.0,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RadialBlurMode {
    #[default]
    Zoom,
    Spin,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RadialBlurParams {
    pub mode: RadialBlurMode,
    pub center: [f32; 2],
    pub zoom_px: f32,
    pub spin_degrees: f32,
    pub samples: u32,
    pub strength: f32,
}

impl Default for RadialBlurParams {
    fn default() -> Self {
        Self {
            mode: RadialBlurMode::Zoom,
            center: [0.5, 0.5],
            zoom_px: 0.0,
            spin_degrees: 0.0,
            samples: 25,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WaveDistortionMode {
    #[default]
    Horizontal,
    Vertical,
    Ripple,
    Zigzag,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WaveDistortionParams {
    pub mode: WaveDistortionMode,
    pub amplitude_px: f32,
    pub wavelength_px: f32,
    pub phase_degrees: f32,
    pub center: [f32; 2],
    pub strength: f32,
}

impl Default for WaveDistortionParams {
    fn default() -> Self {
        Self {
            mode: WaveDistortionMode::Horizontal,
            amplitude_px: 0.0,
            wavelength_px: 64.0,
            phase_degrees: 0.0,
            center: [0.5, 0.5],
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PinchSpherizeParams {
    /// Positive values create a fisheye/bulge; negative values pinch toward the center.
    pub amount: f32,
    pub radius_px: f32,
    pub center: [f32; 2],
    pub strength: f32,
}

impl Default for PinchSpherizeParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius_px: 0.0,
            center: [0.5, 0.5],
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SoftFocusParams {
    pub radius_px: f32,
    pub strength: f32,
}

impl Default for SoftFocusParams {
    fn default() -> Self {
        Self {
            radius_px: 16.0,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MosaicBoundary {
    #[default]
    Opaque,
    Translucent,
    MaskShape,
}

impl MosaicBoundary {
    pub fn process_description(self) -> &'static str {
        match self {
            MosaicBoundary::Opaque => "マスクを含むタイルを不透明で描画",
            MosaicBoundary::Translucent => "マスクを含むタイルをマスクの割合に応じた不透明度で描画",
            MosaicBoundary::MaskShape => {
                "マスクの形に沿って描画 (マスク内の各画素をその画素が属するタイルの平均色で塗る)"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "value", rename_all = "snake_case")]
pub enum MosaicTileMode {
    LongEdgeRatio(f32),
    FixedPx(u32),
}

impl Default for MosaicTileMode {
    fn default() -> Self {
        Self::FixedPx(1)
    }
}

pub const MOSAIC_TILE_RATIO_MIN: f32 = 0.25;
pub const MOSAIC_TILE_RATIO_MAX: f32 = 5.0;
pub const MOSAIC_TILE_RATIO_STEP: f32 = 0.25;
pub const MOSAIC_TILE_FIXED_MIN: u32 = 1;
pub const MOSAIC_TILE_FIXED_MAX: u32 = 200;

pub fn compute_mosaic_tile_size(image_long_edge: u32, mode: MosaicTileMode) -> u32 {
    match mode {
        MosaicTileMode::LongEdgeRatio(multiplier) => {
            let base = ((image_long_edge as f32 / 100.0).round().max(4.0)) as u32;
            ((base as f32 * multiplier).round() as u32).max(4)
        }
        MosaicTileMode::FixedPx(px) => {
            if px <= 1 {
                1
            } else {
                px.max(4)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MosaicParams {
    #[serde(default)]
    pub tile_mode: MosaicTileMode,
    #[serde(default)]
    pub boundary: MosaicBoundary,
    #[serde(default)]
    pub block_px: u32,
}

impl Default for MosaicParams {
    fn default() -> Self {
        Self {
            tile_mode: MosaicTileMode::default(),
            boundary: MosaicBoundary::default(),
            block_px: 0,
        }
    }
}

impl MosaicParams {
    pub fn effective_tile_mode(self) -> MosaicTileMode {
        if self.block_px > 0 {
            MosaicTileMode::FixedPx(self.block_px)
        } else {
            self.tile_mode
        }
    }

    pub fn clear_legacy_block_px(&mut self) {
        self.block_px = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SharpenParams {
    pub amount: f32,
    pub radius_px: f32,
    #[serde(default)]
    pub threshold: f32,
}

impl Default for SharpenParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius_px: 1.0,
            threshold: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SmartSharpenParams {
    pub amount: f32,
    pub radius_px: f32,
    pub edge_threshold: f32,
    pub halo_suppression: f32,
}

impl Default for SmartSharpenParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius_px: 2.0,
            edge_threshold: 0.08,
            halo_suppression: 0.5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HslParams {
    pub hue_degrees: f32,
    pub saturation: f32,
    pub lightness: f32,
}

impl Default for HslParams {
    fn default() -> Self {
        Self {
            hue_degrees: 0.0,
            saturation: 0.0,
            lightness: 0.0,
        }
    }
}

pub const COLOR_MIXER_BAND_COUNT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorMixerBand {
    pub hue_degrees: f32,
    pub saturation: f32,
    pub lightness: f32,
}

impl Default for ColorMixerBand {
    fn default() -> Self {
        Self {
            hue_degrees: 0.0,
            saturation: 0.0,
            lightness: 0.0,
        }
    }
}

impl ColorMixerBand {
    fn is_identity(self) -> bool {
        self.hue_degrees.abs() <= f32::EPSILON
            && self.saturation.abs() <= f32::EPSILON
            && self.lightness.abs() <= f32::EPSILON
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorMixerParams {
    pub bands: [ColorMixerBand; COLOR_MIXER_BAND_COUNT],
    pub range_degrees: f32,
}

impl Default for ColorMixerParams {
    fn default() -> Self {
        Self {
            bands: [ColorMixerBand::default(); COLOR_MIXER_BAND_COUNT],
            range_degrees: 34.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LookPreset {
    None,
    Sunset,
    Night,
    BrightSun,
    Pale,
    Cool,
    Warm,
    RetroFilm,
    TealOrange,
    CherryBlossom,
    FreshGreen,
    Moonlight,
    HighKey,
    LowKey,
    Sepia,
    Cyberpunk,
}

impl Default for LookPreset {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LookParams {
    pub preset: LookPreset,
    pub strength: f32,
}

impl Default for LookParams {
    fn default() -> Self {
        Self {
            preset: LookPreset::None,
            strength: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CubeLutParams {
    pub name: String,
    pub size: usize,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub table: Vec<[f32; 3]>,
    pub strength: f32,
}

impl Default for CubeLutParams {
    fn default() -> Self {
        Self {
            name: String::new(),
            size: 0,
            domain_min: [0.0, 0.0, 0.0],
            domain_max: [1.0, 1.0, 1.0],
            table: Vec::new(),
            strength: 1.0,
        }
    }
}

impl CubeLutParams {
    pub fn is_loaded(&self) -> bool {
        self.size >= 2 && self.table.len() == self.size.saturating_pow(3)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PosterizeParams {
    pub levels: usize,
    pub strength: f32,
}

impl Default for PosterizeParams {
    fn default() -> Self {
        Self {
            levels: 256,
            strength: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ThresholdParams {
    pub threshold: f32,
    pub invert: bool,
    pub strength: f32,
}

impl Default for ThresholdParams {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            invert: false,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct InvertParams {
    pub strength: f32,
}

impl Default for InvertParams {
    fn default() -> Self {
        Self { strength: 0.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DuotonePreset {
    None,
    SepiaInk,
    Cyanotype,
    BlackRed,
    PurpleGold,
    TealCream,
    SunsetTritone,
    ComicTritone,
    NoirTritone,
}

impl Default for DuotonePreset {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DuotoneParams {
    pub preset: DuotonePreset,
    pub strength: f32,
    pub contrast: f32,
}

impl Default for DuotoneParams {
    fn default() -> Self {
        Self {
            preset: DuotonePreset::None,
            strength: 1.0,
            contrast: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EqualizeParams {
    pub strength: f32,
    pub preserve_color: bool,
}

impl Default for EqualizeParams {
    fn default() -> Self {
        Self {
            strength: 0.0,
            preserve_color: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GradientMapPreset {
    None,
    Mono,
    Sepia,
    Sunset,
    Twilight,
    TealOrange,
    Cherry,
    Forest,
    Fire,
    Ice,
}

impl Default for GradientMapPreset {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradientMapParams {
    pub preset: GradientMapPreset,
    pub strength: f32,
    pub contrast: f32,
}

impl Default for GradientMapParams {
    fn default() -> Self {
        Self {
            preset: GradientMapPreset::None,
            strength: 1.0,
            contrast: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BloomParams {
    pub threshold: f32,
    pub radius_px: f32,
    pub strength: f32,
}

impl Default for BloomParams {
    fn default() -> Self {
        Self {
            threshold: 0.72,
            radius_px: 18.0,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VignetteParams {
    /// Positive values darken the edge; negative values brighten it.
    pub strength: f32,
    pub radius: f32,
    pub feather: f32,
}

impl Default for VignetteParams {
    fn default() -> Self {
        Self {
            strength: 0.0,
            radius: 0.52,
            feather: 0.36,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FilmGrainParams {
    pub amount: f32,
    pub size_px: u32,
    pub seed: u32,
}

impl Default for FilmGrainParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            size_px: 1,
            seed: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChromaticAberrationParams {
    pub offset_px: f32,
}

impl Default for ChromaticAberrationParams {
    fn default() -> Self {
        Self { offset_px: 0.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HalftoneParams {
    pub cell_px: u32,
    pub strength: f32,
}

impl Default for HalftoneParams {
    fn default() -> Self {
        Self {
            cell_px: 8,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StarGlowParams {
    pub ray_count: u32,
    pub rotation_degrees: f32,
    pub threshold: f32,
    pub length_px: f32,
    pub strength: f32,
}

impl Default for StarGlowParams {
    fn default() -> Self {
        Self {
            ray_count: 4,
            rotation_degrees: 0.0,
            threshold: 0.995,
            length_px: 48.0,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EdgeSmoothParams {
    pub radius_px: f32,
    pub strength: f32,
    pub edge_threshold: f32,
}

impl Default for EdgeSmoothParams {
    fn default() -> Self {
        Self {
            radius_px: 3.0,
            strength: 0.0,
            edge_threshold: 28.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MedianParams {
    pub radius_px: f32,
    pub strength: f32,
}

impl Default for MedianParams {
    fn default() -> Self {
        Self {
            radius_px: 1.0,
            strength: 0.0,
        }
    }
}

pub fn apply_layers(
    src: RgbaImageRef<'_>,
    layers: &[LocalAdjustmentLayer],
) -> Result<RgbaImageBuf> {
    apply_layers_with_progress(src, layers, None, |_| {})
}

pub fn apply_layers_with_progress<F>(
    src: RgbaImageRef<'_>,
    layers: &[LocalAdjustmentLayer],
    cancel: Option<&AtomicBool>,
    progress: F,
) -> Result<RgbaImageBuf>
where
    F: FnMut(LocalAdjustProgress),
{
    apply_layers_impl(src, layers, cancel, progress)
}

fn apply_layers_impl<F>(
    src: RgbaImageRef<'_>,
    layers: &[LocalAdjustmentLayer],
    cancel: Option<&AtomicBool>,
    mut progress: F,
) -> Result<RgbaImageBuf>
where
    F: FnMut(LocalAdjustProgress),
{
    let src = src.validate()?;
    let mut out = RgbaImageBuf::new(src.width, src.height, src.pixels.to_vec())?;
    let layer_count = layers
        .iter()
        .filter(|layer| layer.enabled && layer.opacity > 0.0)
        .count();
    for layer in layers
        .iter()
        .filter(|layer| layer.enabled && layer.opacity > 0.0)
        .enumerate()
    {
        let (layer_index, layer) = layer;
        check_cancel(cancel)?;
        progress(LocalAdjustProgress {
            layer_index,
            layer_count,
            effect_name: layer.effect.progress_label(),
            percent: 0.0,
        });
        apply_layer(
            &mut out,
            layer,
            layer_index,
            layer_count,
            cancel,
            &mut progress,
        )?;
        check_cancel(cancel)?;
        progress(LocalAdjustProgress {
            layer_index,
            layer_count,
            effect_name: layer.effect.progress_label(),
            percent: 1.0,
        });
    }
    Ok(out)
}

fn check_cancel(cancel: Option<&AtomicBool>) -> Result<()> {
    if cancel
        .map(|cancel| cancel.load(Ordering::Relaxed))
        .unwrap_or(false)
    {
        return Err(LocalAdjustError::Cancelled);
    }
    Ok(())
}

pub fn evaluate_layer_mask(
    image: RgbaImageRef<'_>,
    layer: &LocalAdjustmentLayer,
) -> Result<Vec<f32>> {
    let image = image.validate()?;
    let mut alpha = evaluate_raw_mask(image, &layer.mask)?;
    apply_manual_override(
        &mut alpha,
        image.width,
        image.height,
        &layer.manual_override,
    )?;
    if layer.mask_inverted {
        for a in &mut alpha {
            *a = 1.0 - *a;
        }
    }
    if layer.mask_expand_px.abs() >= 0.5 {
        alpha = morph_alpha(
            &alpha,
            image.width,
            image.height,
            layer.mask_expand_px.round() as i32,
        );
    }
    if layer.mask_feather_px >= 0.5 {
        alpha = box_blur_alpha(
            &alpha,
            image.width,
            image.height,
            layer.mask_feather_px.round() as usize,
        );
    }
    let opacity = layer.opacity.clamp(0.0, 1.0);
    for a in &mut alpha {
        *a = (*a * opacity).clamp(0.0, 1.0);
    }
    Ok(alpha)
}

fn apply_manual_override(
    alpha: &mut [f32],
    width: usize,
    height: usize,
    manual_override: &ManualMaskOverride,
) -> Result<()> {
    if let Some(add) = &manual_override.add {
        let add_alpha = eval_raster_vector_mask(add, width, height)?;
        for (a, add) in alpha.iter_mut().zip(add_alpha) {
            if add >= 0.5 {
                *a = 1.0;
            }
        }
    }
    if let Some(subtract) = &manual_override.subtract {
        let subtract_alpha = eval_raster_vector_mask(subtract, width, height)?;
        for (a, subtract) in alpha.iter_mut().zip(subtract_alpha) {
            if subtract >= 0.5 {
                *a = 0.0;
            }
        }
    }
    Ok(())
}

fn apply_layer<F>(
    image: &mut RgbaImageBuf,
    layer: &LocalAdjustmentLayer,
    layer_index: usize,
    layer_count: usize,
    cancel: Option<&AtomicBool>,
    progress: &mut F,
) -> Result<()>
where
    F: FnMut(LocalAdjustProgress),
{
    if matches!(&layer.effect, LocalEffect::None) {
        return Ok(());
    }
    check_cancel(cancel)?;
    let mask = evaluate_layer_mask(image.as_ref(), layer)?;
    if let LocalEffect::Mosaic(params) = &layer.effect {
        image.pixels =
            apply_mosaic_with_mask(&image.pixels, image.width, image.height, &mask, *params);
        return Ok(());
    }
    let effected = match &layer.effect {
        LocalEffect::None => unreachable!("None is handled before mask evaluation"),
        LocalEffect::Tone(params) => apply_tone_image(&image.pixels, *params),
        LocalEffect::ToneCurve(params) => apply_tone_curve(&image.pixels, *params),
        LocalEffect::RgbToneCurve(params) => apply_rgb_tone_curve(&image.pixels, *params),
        LocalEffect::ColorBalance(params) => apply_color_balance(&image.pixels, *params),
        LocalEffect::ThreeWayColorGrading(params) => {
            apply_three_way_color_grading(&image.pixels, *params)
        }
        LocalEffect::SelectiveColor(params) => apply_selective_color(&image.pixels, *params),
        LocalEffect::ChannelMixer(params) => apply_channel_mixer(&image.pixels, *params),
        LocalEffect::Clarity(params) => apply_clarity(
            &image.pixels,
            image.width,
            image.height,
            params.radius_px.round().max(0.0) as usize,
            params.amount.clamp(-1.0, 1.0),
        ),
        LocalEffect::Texture(params) => apply_texture(
            &image.pixels,
            image.width,
            image.height,
            params.radius_px.round().max(0.0) as usize,
            params.amount.clamp(-1.0, 1.0),
        ),
        LocalEffect::HighPass(params) => apply_high_pass(
            &image.pixels,
            image.width,
            image.height,
            params.radius_px.round().max(0.0) as usize,
            params.amount.clamp(0.0, 2.0),
            params.contrast.clamp(0.1, 4.0),
            params.detail_only,
        ),
        LocalEffect::HighlightsShadows(params) => apply_highlights_shadows(&image.pixels, *params),
        LocalEffect::Dehaze(params) => {
            apply_dehaze(&image.pixels, image.width, image.height, *params)
        }
        LocalEffect::Blur(params) => box_blur_rgba(
            &image.pixels,
            image.width,
            image.height,
            params.radius_px.round().max(0.0) as usize,
        ),
        LocalEffect::MotionBlur(params) => {
            apply_motion_blur(&image.pixels, image.width, image.height, *params)
        }
        LocalEffect::TiltShift(params) => {
            apply_tilt_shift(&image.pixels, image.width, image.height, *params)
        }
        LocalEffect::LensBlur(params) => {
            apply_lens_blur(&image.pixels, image.width, image.height, *params)
        }
        LocalEffect::RadialBlur(params) => {
            apply_radial_blur(&image.pixels, image.width, image.height, *params)
        }
        LocalEffect::WaveDistortion(params) => {
            apply_wave_distortion(&image.pixels, image.width, image.height, *params)
        }
        LocalEffect::PinchSpherize(params) => {
            apply_pinch_spherize(&image.pixels, image.width, image.height, *params)
        }
        LocalEffect::SoftFocus(params) => apply_soft_focus(
            &image.pixels,
            image.width,
            image.height,
            params.radius_px.round().max(0.0) as usize,
            params.strength.clamp(0.0, 1.0),
        ),
        LocalEffect::Mosaic(_) => unreachable!("Mosaic is handled before generic mask blending"),
        LocalEffect::Sharpen(params) => apply_sharpen(
            &image.pixels,
            image.width,
            image.height,
            params.radius_px.round().max(0.0) as usize,
            params.amount.clamp(0.0, 2.0),
            params.threshold.clamp(0.0, 255.0),
        ),
        LocalEffect::SmartSharpen(params) => apply_smart_sharpen(
            &image.pixels,
            image.width,
            image.height,
            params.radius_px.round().max(0.0) as usize,
            params.amount.clamp(0.0, 2.0),
            params.edge_threshold.clamp(0.0, 1.0),
            params.halo_suppression.clamp(0.0, 1.0),
        ),
        LocalEffect::Hsl(params) => apply_hsl(&image.pixels, *params),
        LocalEffect::ColorMixer(params) => apply_color_mixer(&image.pixels, *params),
        LocalEffect::Look(params) => apply_look(&image.pixels, *params),
        LocalEffect::CubeLut(params) => apply_cube_lut(&image.pixels, params),
        LocalEffect::Posterize(params) => apply_posterize(&image.pixels, *params),
        LocalEffect::Threshold(params) => apply_threshold(&image.pixels, *params),
        LocalEffect::Invert(params) => apply_invert(&image.pixels, *params),
        LocalEffect::Duotone(params) => apply_duotone(&image.pixels, *params),
        LocalEffect::Equalize(params) => apply_equalize(&image.pixels, *params),
        LocalEffect::GradientMap(params) => apply_gradient_map(&image.pixels, *params),
        LocalEffect::Bloom(params) => {
            apply_bloom(&image.pixels, image.width, image.height, *params)
        }
        LocalEffect::Vignette(params) => {
            apply_vignette(&image.pixels, image.width, image.height, *params)
        }
        LocalEffect::FilmGrain(params) => {
            apply_film_grain(&image.pixels, image.width, image.height, *params)
        }
        LocalEffect::ChromaticAberration(params) => apply_chromatic_aberration(
            &image.pixels,
            image.width,
            image.height,
            params.offset_px.clamp(0.0, 24.0),
        ),
        LocalEffect::Halftone(params) => {
            apply_halftone(&image.pixels, image.width, image.height, *params)
        }
        LocalEffect::StarGlow(params) => {
            apply_star_glow(&image.pixels, image.width, image.height, *params)
        }
        LocalEffect::EdgeSmooth(params) => {
            apply_edge_smooth(&image.pixels, image.width, image.height, *params)
        }
        LocalEffect::Median(params) => apply_median(
            &image.pixels,
            image.width,
            image.height,
            *params,
            cancel,
            |percent| {
                progress(LocalAdjustProgress {
                    layer_index,
                    layer_count,
                    effect_name: layer.effect.progress_label(),
                    percent,
                });
            },
        )?,
    };
    check_cancel(cancel)?;
    blend_rgb_with_mask(&mut image.pixels, &effected, &mask);
    Ok(())
}

fn evaluate_raw_mask(image: RgbaImageRef<'_>, mask: &LocalMask) -> Result<Vec<f32>> {
    let len = image.width * image.height;
    match mask {
        LocalMask::Full => Ok(vec![1.0; len]),
        LocalMask::Raster(mask) | LocalMask::Subject(mask) => {
            mask.validate(image.width, image.height)?;
            Ok(mask.alpha.iter().map(|v| v.clamp(0.0, 1.0)).collect())
        }
        LocalMask::Segmentation(mask) => {
            mask.validate(image.width, image.height)?;
            Ok(mask
                .labels
                .iter()
                .map(|&label| {
                    if mask.selected.get(label as usize).copied().unwrap_or(false) {
                        1.0
                    } else {
                        0.0
                    }
                })
                .collect())
        }
        LocalMask::RasterVector(mask) => eval_raster_vector_mask(mask, image.width, image.height),
        LocalMask::LinearGradient(mask) => {
            Ok(eval_linear_gradient(image.width, image.height, *mask))
        }
        LocalMask::RadialGradient(mask) => {
            Ok(eval_radial_gradient(image.width, image.height, *mask))
        }
        LocalMask::LumaRange(mask) => Ok(eval_luma_range(image, *mask)),
        LocalMask::ColorRange(mask) => Ok(eval_color_range(image, *mask)),
    }
}

fn eval_raster_vector_mask(
    mask: &RasterVectorMask,
    width: usize,
    height: usize,
) -> Result<Vec<f32>> {
    mask.validate(width, height)?;
    let mut alpha: Vec<f32> = mask.alpha.iter().map(|v| v.clamp(0.0, 1.0)).collect();
    rasterize_shapes_into(&mut alpha, width, height, &mask.shapes);
    Ok(alpha)
}

pub fn rasterize_shapes_into(alpha: &mut [f32], width: usize, height: usize, shapes: &[MaskShape]) {
    for shape in shapes {
        rasterize_shape_into(alpha, width, height, *shape);
    }
}

fn rasterize_shape_into(alpha: &mut [f32], width: usize, height: usize, shape: MaskShape) {
    let add = shape.op().is_add();
    match shape {
        MaskShape::Line {
            p0, p1, thickness, ..
        } => {
            let corners = line_corners(p0, p1, thickness.max(1.0));
            fill_polygon_alpha(alpha, width, height, &corners, add);
        }
        MaskShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
            ..
        } => {
            let corners = rect_corners(center, half_w.max(0.5), half_h.max(0.5), rotation_rad);
            fill_polygon_alpha(alpha, width, height, &corners, add);
        }
        MaskShape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
            ..
        } => {
            fill_ellipse_alpha(
                alpha,
                width,
                height,
                center,
                rx.max(0.5),
                ry.max(0.5),
                rotation_rad,
                add,
            );
        }
    }
}

fn line_corners(p0: [f32; 2], p1: [f32; 2], thickness: f32) -> Vec<[f32; 2]> {
    let dx = p1[0] - p0[0];
    let dy = p1[1] - p0[1];
    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
    let nx = -dy / len;
    let ny = dx / len;
    let half = thickness * 0.5;
    vec![
        [p0[0] + nx * half, p0[1] + ny * half],
        [p1[0] + nx * half, p1[1] + ny * half],
        [p1[0] - nx * half, p1[1] - ny * half],
        [p0[0] - nx * half, p0[1] - ny * half],
    ]
}

fn rect_corners(center: [f32; 2], half_w: f32, half_h: f32, rotation_rad: f32) -> Vec<[f32; 2]> {
    let (s, c) = rotation_rad.sin_cos();
    let mut out = Vec::with_capacity(4);
    for [x, y] in [
        [-half_w, -half_h],
        [half_w, -half_h],
        [half_w, half_h],
        [-half_w, half_h],
    ] {
        out.push([center[0] + x * c - y * s, center[1] + x * s + y * c]);
    }
    out
}

fn fill_polygon_alpha(
    alpha: &mut [f32],
    width: usize,
    height: usize,
    points: &[[f32; 2]],
    add: bool,
) {
    if points.len() < 3 || width == 0 || height == 0 {
        return;
    }
    let min_x = points
        .iter()
        .map(|p| p[0])
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as usize;
    let max_x = points
        .iter()
        .map(|p| p[0])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(width as f32 - 1.0) as usize;
    let min_y = points
        .iter()
        .map(|p| p[1])
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as usize;
    let max_y = points
        .iter()
        .map(|p| p[1])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(height as f32 - 1.0) as usize;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            if point_in_polygon(x as f32 + 0.5, y as f32 + 0.5, points) {
                alpha[y * width + x] = if add { 1.0 } else { 0.0 };
            }
        }
    }
}

fn fill_ellipse_alpha(
    alpha: &mut [f32],
    width: usize,
    height: usize,
    center: [f32; 2],
    rx: f32,
    ry: f32,
    rotation_rad: f32,
    add: bool,
) {
    if width == 0 || height == 0 {
        return;
    }
    let radius = rx.max(ry);
    let min_x = (center[0] - radius).floor().max(0.0) as usize;
    let max_x = (center[0] + radius).ceil().min(width as f32 - 1.0) as usize;
    let min_y = (center[1] - radius).floor().max(0.0) as usize;
    let max_y = (center[1] + radius).ceil().min(height as f32 - 1.0) as usize;
    let (s, c) = (-rotation_rad).sin_cos();
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f32 + 0.5 - center[0];
            let dy = y as f32 + 0.5 - center[1];
            let lx = dx * c - dy * s;
            let ly = dx * s + dy * c;
            if (lx / rx).powi(2) + (ly / ry).powi(2) <= 1.0 {
                alpha[y * width + x] = if add { 1.0 } else { 0.0 };
            }
        }
    }
}

fn point_in_polygon(x: f32, y: f32, points: &[[f32; 2]]) -> bool {
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let xi = points[i][0];
        let yi = points[i][1];
        let xj = points[j][0];
        let yj = points[j][1];
        let dy = yj - yi;
        let intersects =
            ((yi > y) != (yj > y)) && dy.abs() > 1e-6 && (x < (xj - xi) * (y - yi) / dy + xi);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn eval_linear_gradient(width: usize, height: usize, mask: LinearGradientMask) -> Vec<f32> {
    if !mask.initialized {
        return vec![0.0; width * height];
    }
    let sx = mask.start[0];
    let sy = mask.start[1];
    let dx = mask.end[0] - sx;
    let dy = mask.end[1] - sy;
    let denom = dx * dx + dy * dy;
    if denom <= f32::EPSILON {
        return vec![1.0; width * height];
    }
    let mut out = vec![0.0; width * height];
    let wf = width.max(1) as f32;
    let hf = height.max(1) as f32;
    for y in 0..height {
        for x in 0..width {
            let nx = (x as f32 + 0.5) / wf;
            let ny = (y as f32 + 0.5) / hf;
            out[y * width + x] = (((nx - sx) * dx + (ny - sy) * dy) / denom).clamp(0.0, 1.0);
        }
    }
    out
}

fn eval_radial_gradient(width: usize, height: usize, mask: RadialGradientMask) -> Vec<f32> {
    if !mask.initialized {
        return vec![0.0; width * height];
    }
    let mut out = vec![0.0; width * height];
    let wf = width.max(1) as f32;
    let hf = height.max(1) as f32;
    let inner_x = mask.inner_radius.max(0.0);
    let inner_y = mask.inner_radius_y.max(0.0);
    let outer_x = mask.outer_radius.max(inner_x + 0.0001);
    let outer_y = mask.outer_radius_y.max(inner_y + 0.0001);
    for y in 0..height {
        for x in 0..width {
            let nx = (x as f32 + 0.5) / wf;
            let ny = (y as f32 + 0.5) / hf;
            let dx = nx - mask.center[0];
            let dy = ny - mask.center[1];
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= f32::EPSILON {
                out[y * width + x] = 1.0;
                continue;
            }
            let ux = dx / dist;
            let uy = dy / dist;
            let inner = ellipse_radius_for_direction(inner_x, inner_y, ux, uy);
            let outer = ellipse_radius_for_direction(outer_x, outer_y, ux, uy).max(inner + 0.0001);
            out[y * width + x] = (1.0 - ((dist - inner) / (outer - inner))).clamp(0.0, 1.0);
        }
    }
    out
}

fn ellipse_radius_for_direction(rx: f32, ry: f32, ux: f32, uy: f32) -> f32 {
    let rx = rx.max(0.0);
    let ry = ry.max(0.0);
    if rx <= f32::EPSILON || ry <= f32::EPSILON {
        return 0.0;
    }
    let denom = (ux / rx).powi(2) + (uy / ry).powi(2);
    if denom <= f32::EPSILON {
        0.0
    } else {
        1.0 / denom.sqrt()
    }
}

fn eval_luma_range(image: RgbaImageRef<'_>, mask: RangeMask) -> Vec<f32> {
    let mut out = vec![0.0; image.width * image.height];
    let (min, max) = ordered_pair(mask.min, mask.max);
    for (i, px) in image.pixels.chunks_exact(4).enumerate() {
        let luma = (0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32) / 255.0;
        out[i] = range_alpha(luma, min, max, mask.feather);
    }
    out
}

fn eval_color_range(image: RgbaImageRef<'_>, mask: ColorRangeMask) -> Vec<f32> {
    if !mask.initialized {
        return vec![0.0; image.width * image.height];
    }
    let mut out = vec![0.0; image.width * image.height];
    let tr = mask.target_rgb[0] as f32 / 255.0;
    let tg = mask.target_rgb[1] as f32 / 255.0;
    let tb = mask.target_rgb[2] as f32 / 255.0;
    let tol = mask.tolerance.max(0.0);
    let feather = mask.feather.max(0.0001);
    for (i, px) in image.pixels.chunks_exact(4).enumerate() {
        let dr = px[0] as f32 / 255.0 - tr;
        let dg = px[1] as f32 / 255.0 - tg;
        let db = px[2] as f32 / 255.0 - tb;
        let dist = ((dr * dr + dg * dg + db * db) / 3.0).sqrt();
        out[i] = if dist <= tol {
            1.0
        } else {
            (1.0 - (dist - tol) / feather).clamp(0.0, 1.0)
        };
    }
    out
}

fn range_alpha(value: f32, min: f32, max: f32, feather: f32) -> f32 {
    let feather = feather.max(0.0001);
    if value >= min && value <= max {
        1.0
    } else if value < min {
        (1.0 - (min - value) / feather).clamp(0.0, 1.0)
    } else {
        (1.0 - (value - max) / feather).clamp(0.0, 1.0)
    }
}

fn ordered_pair(a: f32, b: f32) -> (f32, f32) {
    if a <= b { (a, b) } else { (b, a) }
}

fn morph_alpha(src: &[f32], width: usize, height: usize, radius: i32) -> Vec<f32> {
    let r = radius.unsigned_abs() as i32;
    if r == 0 {
        return src.to_vec();
    }
    let offsets = circle_offsets(r);
    let mut out = vec![0.0; src.len()];
    let dilate = radius > 0;
    for y in 0..height {
        for x in 0..width {
            let mut v = if dilate { 0.0_f32 } else { 1.0_f32 };
            for (ox, oy) in &offsets {
                let nx = x as i32 + *ox;
                let ny = y as i32 + *oy;
                if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                    continue;
                }
                let sample = src[ny as usize * width + nx as usize];
                if dilate {
                    v = v.max(sample);
                } else {
                    v = v.min(sample);
                }
            }
            out[y * width + x] = v;
        }
    }
    out
}

fn circle_offsets(radius: i32) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    let r2 = radius * radius;
    for y in -radius..=radius {
        for x in -radius..=radius {
            if x * x + y * y <= r2 {
                out.push((x, y));
            }
        }
    }
    out
}

fn box_blur_alpha(src: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    if radius == 0 || width == 0 || height == 0 {
        return src.to_vec();
    }
    let mut tmp = vec![0.0; src.len()];
    let mut out = vec![0.0; src.len()];
    for y in 0..height {
        for x in 0..width {
            let x0 = x.saturating_sub(radius);
            let x1 = (x + radius).min(width - 1);
            let mut sum = 0.0;
            for xx in x0..=x1 {
                sum += src[y * width + xx];
            }
            tmp[y * width + x] = sum / (x1 - x0 + 1) as f32;
        }
    }
    for y in 0..height {
        let y0 = y.saturating_sub(radius);
        let y1 = (y + radius).min(height - 1);
        for x in 0..width {
            let mut sum = 0.0;
            for yy in y0..=y1 {
                sum += tmp[yy * width + x];
            }
            out[y * width + x] = sum / (y1 - y0 + 1) as f32;
        }
    }
    out
}

fn apply_tone_image(src: &[u8], params: ToneParams) -> Vec<u8> {
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        let adjusted = tone_rgb([px[0], px[1], px[2]], params);
        px[0] = adjusted[0];
        px[1] = adjusted[1];
        px[2] = adjusted[2];
    }
    out
}

fn apply_tone_curve(src: &[u8], params: ToneCurveParams) -> Vec<u8> {
    let lut = tone_curve_lut(params);
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        px[0] = lut[px[0] as usize];
        px[1] = lut[px[1] as usize];
        px[2] = lut[px[2] as usize];
    }
    out
}

fn apply_rgb_tone_curve(src: &[u8], params: RgbToneCurveParams) -> Vec<u8> {
    let red_lut = rgb_tone_curve_lut(params.master, params.red);
    let green_lut = rgb_tone_curve_lut(params.master, params.green);
    let blue_lut = rgb_tone_curve_lut(params.master, params.blue);
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        px[0] = red_lut[px[0] as usize];
        px[1] = green_lut[px[1] as usize];
        px[2] = blue_lut[px[2] as usize];
    }
    out
}

fn tone_curve_lut(params: ToneCurveParams) -> [u8; 256] {
    let mut lut = [0_u8; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        *v = to_u8(tone_curve_value(i as f32 / 255.0, params.points));
    }
    lut
}

fn rgb_tone_curve_lut(master: [f32; 5], channel: [f32; 5]) -> [u8; 256] {
    let mut lut = [0_u8; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        let master_value = tone_curve_value(i as f32 / 255.0, master);
        *v = to_u8(tone_curve_value(master_value, channel));
    }
    lut
}

fn tone_curve_value(x: f32, points: [f32; 5]) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let seg = ((x * 4.0).floor() as usize).min(3);
    let x0 = seg as f32 * 0.25;
    let t = ((x - x0) * 4.0).clamp(0.0, 1.0);
    lerp_f32(
        points[seg].clamp(0.0, 1.0),
        points[seg + 1].clamp(0.0, 1.0),
        t,
    )
}

fn apply_color_balance(src: &[u8], params: ColorBalanceParams) -> Vec<u8> {
    if params.shadows.is_identity()
        && params.midtones.is_identity()
        && params.highlights.is_identity()
    {
        return src.to_vec();
    }
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        let r = px[0] as f32 / 255.0;
        let g = px[1] as f32 / 255.0;
        let b = px[2] as f32 / 255.0;
        let luma = luma01(r, g, b);
        let shadow_weight = 1.0 - smoothstep(0.15, 0.55, luma);
        let highlight_weight = smoothstep(0.45, 0.85, luma);
        let midtone_weight = (1.0 - shadow_weight - highlight_weight).clamp(0.0, 1.0);
        let delta = color_balance_delta(params.shadows, shadow_weight);
        let delta = add_rgb_delta(delta, color_balance_delta(params.midtones, midtone_weight));
        let delta = add_rgb_delta(
            delta,
            color_balance_delta(params.highlights, highlight_weight),
        );
        let mut adjusted = [
            (r + delta[0]).clamp(0.0, 1.0),
            (g + delta[1]).clamp(0.0, 1.0),
            (b + delta[2]).clamp(0.0, 1.0),
        ];
        if params.preserve_luma {
            let adjusted_luma = luma01(adjusted[0], adjusted[1], adjusted[2]);
            let luma_delta = luma - adjusted_luma;
            for c in &mut adjusted {
                *c = (*c + luma_delta).clamp(0.0, 1.0);
            }
        }
        px[0] = to_u8(adjusted[0]);
        px[1] = to_u8(adjusted[1]);
        px[2] = to_u8(adjusted[2]);
    }
    out
}

fn color_balance_delta(range: ColorBalanceRange, weight: f32) -> [f32; 3] {
    let scale = 0.24 * weight.clamp(0.0, 1.0);
    let cyan_red = (range.cyan_red / 100.0).clamp(-1.0, 1.0) * scale;
    let magenta_green = (range.magenta_green / 100.0).clamp(-1.0, 1.0) * scale;
    let yellow_blue = (range.yellow_blue / 100.0).clamp(-1.0, 1.0) * scale;
    [
        cyan_red - magenta_green * 0.5 - yellow_blue * 0.5,
        -cyan_red * 0.5 + magenta_green - yellow_blue * 0.5,
        -cyan_red * 0.5 - magenta_green * 0.5 + yellow_blue,
    ]
}

fn add_rgb_delta(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn apply_three_way_color_grading(src: &[u8], params: ThreeWayColorGradingParams) -> Vec<u8> {
    if params.shadows.is_identity()
        && params.midtones.is_identity()
        && params.highlights.is_identity()
    {
        return src.to_vec();
    }
    let mut out = src.to_vec();
    let balance = (params.balance / 100.0).clamp(-1.0, 1.0) * 0.18;
    for px in out.chunks_exact_mut(4) {
        let r = px[0] as f32 / 255.0;
        let g = px[1] as f32 / 255.0;
        let b = px[2] as f32 / 255.0;
        let luma = luma01(r, g, b);
        let shadow_weight = 1.0 - smoothstep(0.16 + balance, 0.58 + balance, luma);
        let highlight_weight = smoothstep(0.42 + balance, 0.84 + balance, luma);
        let midtone_weight = (1.0 - shadow_weight - highlight_weight).clamp(0.0, 1.0);
        let mut delta = grade_wheel_delta(params.shadows, shadow_weight);
        delta = add_rgb_delta(delta, grade_wheel_delta(params.midtones, midtone_weight));
        delta = add_rgb_delta(
            delta,
            grade_wheel_delta(params.highlights, highlight_weight),
        );
        px[0] = to_u8(r + delta[0]);
        px[1] = to_u8(g + delta[1]);
        px[2] = to_u8(b + delta[2]);
    }
    out
}

fn grade_wheel_delta(wheel: ColorGradeWheel, weight: f32) -> [f32; 3] {
    let weight = weight.clamp(0.0, 1.0);
    if weight <= f32::EPSILON || wheel.is_identity() {
        return [0.0, 0.0, 0.0];
    }
    let saturation = (wheel.saturation / 100.0).clamp(-1.0, 1.0);
    let luminance = (wheel.luminance / 100.0).clamp(-1.0, 1.0);
    let hue_rgb = hsl_to_rgb(wrap01(wheel.hue_degrees / 360.0), 1.0, 0.5);
    let neutral = luma01(hue_rgb[0], hue_rgb[1], hue_rgb[2]);
    let tint_scale = saturation * 0.34 * weight;
    let luma_delta = luminance * 0.30 * weight;
    [
        (hue_rgb[0] - neutral) * tint_scale + luma_delta,
        (hue_rgb[1] - neutral) * tint_scale + luma_delta,
        (hue_rgb[2] - neutral) * tint_scale + luma_delta,
    ]
}

fn apply_selective_color(src: &[u8], params: SelectiveColorParams) -> Vec<u8> {
    let hue_shift = params.hue_degrees / 360.0;
    let sat_delta = (params.saturation / 100.0).clamp(-1.0, 1.0);
    let light_delta = (params.lightness / 100.0).clamp(-1.0, 1.0);
    if hue_shift.abs() <= f32::EPSILON
        && sat_delta.abs() <= f32::EPSILON
        && light_delta.abs() <= f32::EPSILON
    {
        return src.to_vec();
    }
    let target = params.target_hue_degrees.rem_euclid(360.0);
    let range = params.range_degrees.clamp(1.0, 180.0);
    let feather = params.feather_degrees.clamp(0.0, 180.0);
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        let (mut h, mut s, mut l) = rgb_to_hsl(
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        );
        let hue_degrees = h * 360.0;
        let weight =
            selective_hue_weight(hue_degrees, target, range, feather) * smoothstep(0.03, 0.16, s);
        if weight <= f32::EPSILON {
            continue;
        }
        h = wrap01(h + hue_shift * weight);
        s = (s * (1.0 + sat_delta * weight)).clamp(0.0, 1.0);
        l = (l + light_delta * 0.5 * weight).clamp(0.0, 1.0);
        let [r, g, b] = hsl_to_rgb(h, s, l);
        px[0] = to_u8(r);
        px[1] = to_u8(g);
        px[2] = to_u8(b);
    }
    out
}

fn selective_hue_weight(
    hue_degrees: f32,
    target_degrees: f32,
    range_degrees: f32,
    feather_degrees: f32,
) -> f32 {
    let delta = (hue_degrees - target_degrees).rem_euclid(360.0);
    let distance = delta.min(360.0 - delta);
    let outer = (range_degrees + feather_degrees).max(range_degrees + 0.001);
    1.0 - smoothstep(range_degrees, outer, distance)
}

fn apply_channel_mixer(src: &[u8], params: ChannelMixerParams) -> Vec<u8> {
    if params.is_identity() {
        return src.to_vec();
    }
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        let rgb = [
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        ];
        if params.monochrome {
            let gray = mix_channels(rgb, params.mono_output);
            px[0] = to_u8(gray);
            px[1] = to_u8(gray);
            px[2] = to_u8(gray);
        } else {
            px[0] = to_u8(mix_channels(rgb, params.red_output));
            px[1] = to_u8(mix_channels(rgb, params.green_output));
            px[2] = to_u8(mix_channels(rgb, params.blue_output));
        }
    }
    out
}

fn mix_channels(rgb: [f32; 3], coeffs: [f32; 3]) -> f32 {
    (rgb[0] * coeffs[0] + rgb[1] * coeffs[1] + rgb[2] * coeffs[2]) / 100.0
}

fn apply_clarity(src: &[u8], width: usize, height: usize, radius: usize, amount: f32) -> Vec<u8> {
    if radius == 0 || amount.abs() <= f32::EPSILON {
        return src.to_vec();
    }
    let blur = box_blur_rgba(src, width, height, radius);
    let mut out = src.to_vec();
    for i in (0..src.len()).step_by(4) {
        for c in 0..3 {
            let base = src[i + c] as f32;
            let low = blur[i + c] as f32;
            out[i + c] = (base + (base - low) * amount).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn apply_texture(src: &[u8], width: usize, height: usize, radius: usize, amount: f32) -> Vec<u8> {
    if radius < 2 || amount.abs() <= f32::EPSILON {
        return src.to_vec();
    }
    let fine_radius = (radius / 3).clamp(1, radius.saturating_sub(1));
    let fine = box_blur_rgba(src, width, height, fine_radius);
    let coarse = box_blur_rgba(src, width, height, radius);
    let mut out = src.to_vec();
    for i in (0..src.len()).step_by(4) {
        for c in 0..3 {
            let base = src[i + c] as f32;
            let detail = fine[i + c] as f32 - coarse[i + c] as f32;
            out[i + c] = (base + detail * amount).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn apply_high_pass(
    src: &[u8],
    width: usize,
    height: usize,
    radius: usize,
    amount: f32,
    contrast: f32,
    detail_only: bool,
) -> Vec<u8> {
    if radius == 0 || (!detail_only && amount <= f32::EPSILON) {
        return src.to_vec();
    }
    let blur = box_blur_rgba(src, width, height, radius);
    let mut out = src.to_vec();
    for i in (0..src.len()).step_by(4) {
        for c in 0..3 {
            let base = src[i + c] as f32;
            let low = blur[i + c] as f32;
            let high_pass = (128.0 + (base - low) * contrast).round().clamp(0.0, 255.0) as u8;
            if detail_only {
                out[i + c] = high_pass;
            } else {
                let overlay = overlay_channel(src[i + c], high_pass);
                out[i + c] = (base + (overlay as f32 - base) * amount)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

fn overlay_channel(base: u8, blend: u8) -> u8 {
    let base = base as u32;
    let blend = blend as u32;
    if base < 128 {
        ((2 * base * blend + 127) / 255).min(255) as u8
    } else {
        (255 - ((2 * (255 - base) * (255 - blend) + 127) / 255)).min(255) as u8
    }
}

fn apply_highlights_shadows(src: &[u8], params: HighlightsShadowsParams) -> Vec<u8> {
    let mut out = src.to_vec();
    let shadows = (params.shadows / 100.0).clamp(-1.0, 1.0);
    let highlights = (params.highlights / 100.0).clamp(-1.0, 1.0);
    for px in out.chunks_exact_mut(4) {
        let r = px[0] as f32 / 255.0;
        let g = px[1] as f32 / 255.0;
        let b = px[2] as f32 / 255.0;
        let luma = (0.2126 * r + 0.7152 * g + 0.0722 * b).clamp(0.0, 1.0);
        let shadow_weight = (1.0 - luma).powi(2);
        let highlight_weight = luma.powi(2);
        let delta = shadows * 0.45 * shadow_weight - highlights * 0.45 * highlight_weight;
        px[0] = to_u8(r + delta);
        px[1] = to_u8(g + delta);
        px[2] = to_u8(b + delta);
    }
    out
}

fn apply_dehaze(src: &[u8], width: usize, height: usize, params: DehazeParams) -> Vec<u8> {
    let amount = params.amount.clamp(0.0, 1.0);
    if amount <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }
    let radius = params.radius_px.round().clamp(0.0, 48.0) as usize;
    let air = estimate_airlight(src);
    let mut dark = Vec::with_capacity(width * height);
    for i in (0..src.len()).step_by(4) {
        let r = src[i] as f32 / 255.0 / air[0].max(0.05);
        let g = src[i + 1] as f32 / 255.0 / air[1].max(0.05);
        let b = src[i + 2] as f32 / 255.0 / air[2].max(0.05);
        dark.push(r.min(g).min(b).clamp(0.0, 1.0));
    }
    let dark = min_filter_f32(&dark, width, height, radius);
    let omega = 0.95 * amount;
    let min_t = params.min_transmission.clamp(0.10, 0.90);
    let mut transmission: Vec<f32> = dark
        .iter()
        .map(|d| (1.0 - omega * *d).clamp(min_t, 1.0))
        .collect();
    let smooth_radius = (radius / 3).min(16);
    if smooth_radius > 0 {
        transmission = box_blur_alpha(&transmission, width, height, smooth_radius);
    }
    let sat = (1.0 + params.saturation / 100.0).max(0.0);
    let mut out = src.to_vec();
    for (idx, px) in out.chunks_exact_mut(4).enumerate() {
        let t = transmission[idx].clamp(min_t, 1.0);
        let mut rgb = [0.0_f32; 3];
        for c in 0..3 {
            let i = src[idx * 4 + c] as f32 / 255.0;
            rgb[c] = ((i - air[c]) / t + air[c]).clamp(0.0, 1.0);
        }
        if (sat - 1.0).abs() > f32::EPSILON {
            rgb = adjust_saturation(rgb, sat);
        }
        px[0] = to_u8(rgb[0]);
        px[1] = to_u8(rgb[1]);
        px[2] = to_u8(rgb[2]);
    }
    out
}

fn estimate_airlight(src: &[u8]) -> [f32; 3] {
    let mut best_luma = -1.0_f32;
    let mut best = [1.0_f32; 3];
    for i in (0..src.len()).step_by(4) {
        let r = src[i] as f32 / 255.0;
        let g = src[i + 1] as f32 / 255.0;
        let b = src[i + 2] as f32 / 255.0;
        let luma = luma01(r, g, b);
        if luma > best_luma {
            best_luma = luma;
            best = [r.max(0.05), g.max(0.05), b.max(0.05)];
        }
    }
    best
}

fn min_filter_f32(src: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    if radius == 0 || width == 0 || height == 0 {
        return src.to_vec();
    }
    let mut tmp = vec![0.0; src.len()];
    let mut out = vec![0.0; src.len()];
    for y in 0..height {
        for x in 0..width {
            let x0 = x.saturating_sub(radius);
            let x1 = (x + radius).min(width - 1);
            let mut v = f32::INFINITY;
            for xx in x0..=x1 {
                v = v.min(src[y * width + xx]);
            }
            tmp[y * width + x] = v;
        }
    }
    for y in 0..height {
        let y0 = y.saturating_sub(radius);
        let y1 = (y + radius).min(height - 1);
        for x in 0..width {
            let mut v = f32::INFINITY;
            for yy in y0..=y1 {
                v = v.min(tmp[yy * width + x]);
            }
            out[y * width + x] = v;
        }
    }
    out
}

fn tone_rgb(rgb: [u8; 3], params: ToneParams) -> [u8; 3] {
    let mut r = rgb[0] as f32 / 255.0;
    let mut g = rgb[1] as f32 / 255.0;
    let mut b = rgb[2] as f32 / 255.0;

    let temp = (params.temperature / 100.0).clamp(-1.0, 1.0);
    r += temp * 0.08;
    b -= temp * 0.08;

    let tint = (params.tint / 100.0).clamp(-1.0, 1.0);
    r += tint * 0.05;
    g -= tint * 0.07;
    b += tint * 0.05;

    let brightness = params.brightness / 100.0;
    r += brightness;
    g += brightness;
    b += brightness;

    let contrast = (1.0 + params.contrast / 100.0).max(0.0);
    r = (r - 0.5) * contrast + 0.5;
    g = (g - 0.5) * contrast + 0.5;
    b = (b - 0.5) * contrast + 0.5;

    let gamma = params.gamma.clamp(0.1, 5.0);
    let inv_gamma = 1.0 / gamma;
    r = r.clamp(0.0, 1.0).powf(inv_gamma);
    g = g.clamp(0.0, 1.0).powf(inv_gamma);
    b = b.clamp(0.0, 1.0).powf(inv_gamma);

    let sat = (1.0 + params.saturation / 100.0).max(0.0);
    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    r = luma + (r - luma) * sat;
    g = luma + (g - luma) * sat;
    b = luma + (b - luma) * sat;

    let vibrance = (params.vibrance / 100.0).clamp(-1.0, 1.0);
    if vibrance.abs() > f32::EPSILON {
        let max_c = r.max(g).max(b);
        let min_c = r.min(g).min(b);
        let current_sat = (max_c - min_c).clamp(0.0, 1.0);
        let vibrance_scale = if vibrance >= 0.0 {
            1.0 + vibrance * (1.0 - current_sat).powf(1.4)
        } else {
            1.0 + vibrance * 0.85
        };
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        r = luma + (r - luma) * vibrance_scale;
        g = luma + (g - luma) * vibrance_scale;
        b = luma + (b - luma) * vibrance_scale;
    }

    [to_u8(r), to_u8(g), to_u8(b)]
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn box_blur_rgba(src: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
    if radius == 0 || width == 0 || height == 0 {
        return src.to_vec();
    }
    let mut tmp = vec![0_u8; src.len()];
    let mut out = vec![0_u8; src.len()];
    for y in 0..height {
        for x in 0..width {
            let x0 = x.saturating_sub(radius);
            let x1 = (x + radius).min(width - 1);
            let count = (x1 - x0 + 1) as u32;
            let mut sum = [0_u32; 4];
            for xx in x0..=x1 {
                let i = (y * width + xx) * 4;
                for c in 0..4 {
                    sum[c] += src[i + c] as u32;
                }
            }
            let o = (y * width + x) * 4;
            for c in 0..4 {
                tmp[o + c] = (sum[c] / count) as u8;
            }
        }
    }
    for y in 0..height {
        let y0 = y.saturating_sub(radius);
        let y1 = (y + radius).min(height - 1);
        let count = (y1 - y0 + 1) as u32;
        for x in 0..width {
            let mut sum = [0_u32; 4];
            for yy in y0..=y1 {
                let i = (yy * width + x) * 4;
                for c in 0..4 {
                    sum[c] += tmp[i + c] as u32;
                }
            }
            let o = (y * width + x) * 4;
            for c in 0..4 {
                out[o + c] = (sum[c] / count) as u8;
            }
        }
    }
    out
}

fn apply_motion_blur(src: &[u8], width: usize, height: usize, params: MotionBlurParams) -> Vec<u8> {
    let distance = params.distance_px.clamp(0.0, 240.0);
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || distance <= 0.5 || strength <= f32::EPSILON {
        return src.to_vec();
    }

    let sample_count = ((distance.ceil() as usize) + 1).clamp(3, 65);
    let half = distance * 0.5;
    let angle = params.angle_degrees.to_radians();
    let dir_x = angle.cos();
    let dir_y = angle.sin();
    let mut out = src.to_vec();

    for y in 0..height {
        for x in 0..width {
            let mut sum = [0.0_f32; 4];
            for i in 0..sample_count {
                let t = if sample_count <= 1 {
                    0.0
                } else {
                    i as f32 / (sample_count - 1) as f32
                };
                let offset = lerp_f32(-half, half, t);
                let sx = x as f32 + dir_x * offset;
                let sy = y as f32 + dir_y * offset;
                let si = nearest_pixel_index(width, height, sx, sy);
                for c in 0..4 {
                    sum[c] += src[si + c] as f32;
                }
            }
            let oi = (y * width + x) * 4;
            for c in 0..4 {
                let blurred = (sum[c] / sample_count as f32).round() as u8;
                out[oi + c] = lerp_u8(src[oi + c], blurred, strength);
            }
        }
    }
    out
}

fn apply_tilt_shift(src: &[u8], width: usize, height: usize, params: TiltShiftParams) -> Vec<u8> {
    let radius = params.max_radius_px.round().clamp(0.0, 160.0) as usize;
    let strength = params.strength.clamp(0.0, 1.0);
    if !params.range_initialized
        || width == 0
        || height == 0
        || radius == 0
        || strength <= f32::EPSILON
    {
        return src.to_vec();
    }

    let blurred = box_blur_rgba(src, width, height, radius);
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let weight = tilt_shift_weight(x, y, width, height, params) * strength;
            if weight <= f32::EPSILON {
                continue;
            }
            let i = (y * width + x) * 4;
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], blurred[i + c], weight);
            }
        }
    }
    out
}

fn tilt_shift_weight(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    params: TiltShiftParams,
) -> f32 {
    let nx = normalized_coord(x, width);
    let ny = normalized_coord(y, height);
    let center_x = params.center[0].clamp(0.0, 1.0);
    let center_y = params.center[1].clamp(0.0, 1.0);
    let falloff = params.falloff.max(0.001);
    let distance = match params.mode {
        TiltShiftMode::Linear => {
            let angle = params.angle_degrees.to_radians();
            let dir_x = angle.cos();
            let dir_y = angle.sin();
            let signed_depth = (nx - center_x) * dir_x + (ny - center_y) * dir_y;
            if params.far_only {
                signed_depth - params.focus_width.max(0.0)
            } else {
                signed_depth.abs() - params.focus_width.max(0.0)
            }
        }
        TiltShiftMode::Radial => {
            let rx = params.radius[0].max(0.001);
            let ry = params.radius[1].max(0.001);
            let dx = (nx - center_x) / rx;
            let dy = (ny - center_y) / ry;
            (dx * dx + dy * dy).sqrt() - 1.0
        }
    };
    smoothstep(0.0, falloff, distance.max(0.0))
}

fn normalized_coord(value: usize, size: usize) -> f32 {
    if size <= 1 {
        0.5
    } else {
        value as f32 / (size - 1) as f32
    }
}

fn apply_lens_blur(src: &[u8], width: usize, height: usize, params: LensBlurParams) -> Vec<u8> {
    let radius = params.radius_px.clamp(0.0, 96.0);
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || radius <= 0.5 || strength <= f32::EPSILON {
        return src.to_vec();
    }

    let offsets = lens_blur_offsets(radius, params.aperture, params.rotation_degrees);
    if offsets.len() <= 1 {
        return src.to_vec();
    }

    let threshold = params.highlight_threshold.clamp(0.0, 0.999);
    let inv_highlight_range = 1.0 / (1.0 - threshold).max(0.001);
    let highlight_boost = params.highlight_boost.clamp(0.0, 3.0);
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let mut sum = [0.0_f32; 3];
            for (dx, dy) in &offsets {
                let si = nearest_pixel_index(width, height, x as f32 + *dx, y as f32 + *dy);
                let r = src[si] as f32 / 255.0;
                let g = src[si + 1] as f32 / 255.0;
                let b = src[si + 2] as f32 / 255.0;
                let luma = luma01(r, g, b);
                let highlight = ((luma - threshold) * inv_highlight_range).clamp(0.0, 1.0);
                let boost = 1.0 + highlight_boost * highlight.powf(1.5);
                sum[0] += r * boost;
                sum[1] += g * boost;
                sum[2] += b * boost;
            }

            let inv_count = 1.0 / offsets.len() as f32;
            let oi = (y * width + x) * 4;
            for c in 0..3 {
                let blurred = to_u8(sum[c] * inv_count);
                out[oi + c] = lerp_u8(src[oi + c], blurred, strength);
            }
        }
    }
    out
}

fn lens_blur_offsets(
    radius: f32,
    aperture: LensBlurAperture,
    rotation_degrees: f32,
) -> Vec<(f32, f32)> {
    let ring_count = ((radius / 10.0).ceil() as usize).clamp(1, 5);
    let tau = std::f32::consts::PI * 2.0;
    let rotation = rotation_degrees.to_radians();
    let mut offsets = Vec::with_capacity(1 + ring_count * 24);
    offsets.push((0.0, 0.0));
    for ring in 1..=ring_count {
        let ring_t = ring as f32 / ring_count as f32;
        let samples = (ring * 8).clamp(8, 40);
        let stagger = if ring % 2 == 0 { 0.5 } else { 0.0 };
        for sample in 0..samples {
            let angle = tau * ((sample as f32 + stagger) / samples as f32);
            let aperture_radius = aperture_radius_at_angle(aperture, angle, rotation);
            let sample_radius = radius * ring_t.sqrt() * aperture_radius;
            offsets.push((sample_radius * angle.cos(), sample_radius * angle.sin()));
        }
    }
    offsets
}

fn aperture_radius_at_angle(aperture: LensBlurAperture, angle: f32, rotation: f32) -> f32 {
    let sides = match aperture {
        LensBlurAperture::Circular => return 1.0,
        LensBlurAperture::Hexagon => 6.0,
        LensBlurAperture::Octagon => 8.0,
    };
    let sector = std::f32::consts::PI * 2.0 / sides;
    let local = (angle - rotation + sector * 0.5).rem_euclid(sector) - sector * 0.5;
    (std::f32::consts::PI / sides).cos() / local.cos().max(0.001)
}

fn apply_radial_blur(src: &[u8], width: usize, height: usize, params: RadialBlurParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || strength <= f32::EPSILON {
        return src.to_vec();
    }

    let samples = params.samples.clamp(3, 65) as usize;
    let zoom_px = params.zoom_px.clamp(0.0, 240.0);
    let spin_radians = params.spin_degrees.clamp(-180.0, 180.0).to_radians();
    match params.mode {
        RadialBlurMode::Zoom if zoom_px <= 0.5 => return src.to_vec(),
        RadialBlurMode::Spin if spin_radians.abs() <= 0.001 => return src.to_vec(),
        _ => {}
    }

    let cx = (width.saturating_sub(1)) as f32 * params.center[0].clamp(0.0, 1.0);
    let cy = (height.saturating_sub(1)) as f32 * params.center[1].clamp(0.0, 1.0);
    let max_dist = farthest_corner_distance(width, height, cx, cy).max(1.0);
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= f32::EPSILON {
                continue;
            }
            let dist_factor = (dist / max_dist).clamp(0.0, 1.0);
            let mut sum = [0.0_f32; 3];
            for sample in 0..samples {
                let t = if samples <= 1 {
                    0.0
                } else {
                    sample as f32 / (samples - 1) as f32 - 0.5
                };
                let (sx, sy) = match params.mode {
                    RadialBlurMode::Zoom => {
                        let offset = zoom_px * dist_factor * t;
                        (x as f32 + dx / dist * offset, y as f32 + dy / dist * offset)
                    }
                    RadialBlurMode::Spin => {
                        let theta = spin_radians * dist_factor * t;
                        let cos_t = theta.cos();
                        let sin_t = theta.sin();
                        (cx + dx * cos_t - dy * sin_t, cy + dx * sin_t + dy * cos_t)
                    }
                };
                let rgb = sample_rgb_bilinear(src, width, height, sx, sy);
                sum[0] += rgb[0];
                sum[1] += rgb[1];
                sum[2] += rgb[2];
            }
            let inv_samples = 1.0 / samples as f32;
            let oi = (y * width + x) * 4;
            for c in 0..3 {
                let blurred = to_u8(sum[c] * inv_samples);
                out[oi + c] = lerp_u8(src[oi + c], blurred, strength);
            }
        }
    }
    out
}

fn apply_wave_distortion(
    src: &[u8],
    width: usize,
    height: usize,
    params: WaveDistortionParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let amplitude = params.amplitude_px.clamp(-240.0, 240.0);
    if width == 0 || height == 0 || strength <= f32::EPSILON || amplitude.abs() <= f32::EPSILON {
        return src.to_vec();
    }
    let wavelength = params.wavelength_px.max(2.0);
    let phase = params.phase_degrees.to_radians();
    let cx = (width.saturating_sub(1)) as f32 * params.center[0].clamp(0.0, 1.0);
    let cy = (height.saturating_sub(1)) as f32 * params.center[1].clamp(0.0, 1.0);
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let xf = x as f32;
            let yf = y as f32;
            let (sx, sy) = match params.mode {
                WaveDistortionMode::Horizontal => {
                    let wave = (yf / wavelength * std::f32::consts::TAU + phase).sin();
                    (xf + amplitude * wave, yf)
                }
                WaveDistortionMode::Vertical => {
                    let wave = (xf / wavelength * std::f32::consts::TAU + phase).sin();
                    (xf, yf + amplitude * wave)
                }
                WaveDistortionMode::Ripple => {
                    let dx = xf - cx;
                    let dy = yf - cy;
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist <= f32::EPSILON {
                        (xf, yf)
                    } else {
                        let wave = (dist / wavelength * std::f32::consts::TAU + phase).sin();
                        let offset = amplitude * wave;
                        (xf + dx / dist * offset, yf + dy / dist * offset)
                    }
                }
                WaveDistortionMode::Zigzag => {
                    let wave = zigzag_wave(yf / wavelength + phase / std::f32::consts::TAU);
                    (xf + amplitude * wave, yf)
                }
            };
            let sampled = sample_rgb_bilinear(src, width, height, sx, sy);
            let i = (y * width + x) * 4;
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], to_u8(sampled[c]), strength);
            }
        }
    }
    out
}

fn zigzag_wave(t: f32) -> f32 {
    let t = t.rem_euclid(1.0);
    if t < 0.5 {
        t * 4.0 - 1.0
    } else {
        3.0 - t * 4.0
    }
}

fn apply_pinch_spherize(
    src: &[u8],
    width: usize,
    height: usize,
    params: PinchSpherizeParams,
) -> Vec<u8> {
    let amount = params.amount.clamp(-1.0, 1.0);
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || amount.abs() <= f32::EPSILON || strength <= f32::EPSILON {
        return src.to_vec();
    }
    let cx = (width.saturating_sub(1)) as f32 * params.center[0].clamp(0.0, 1.0);
    let cy = (height.saturating_sub(1)) as f32 * params.center[1].clamp(0.0, 1.0);
    let max_radius = farthest_corner_distance(width, height, cx, cy).max(1.0);
    let radius = if params.radius_px > 0.0 {
        params.radius_px.min(max_radius).max(1.0)
    } else {
        max_radius
    };
    let exponent = if amount >= 0.0 {
        1.0 + amount * 2.0
    } else {
        1.0 / (1.0 + (-amount) * 2.0)
    };
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= f32::EPSILON || dist >= radius {
                continue;
            }
            let t = (dist / radius).clamp(0.0, 1.0);
            let warped_dist = radius * t.powf(exponent);
            let sx = cx + dx / dist * warped_dist;
            let sy = cy + dy / dist * warped_dist;
            let sampled = sample_rgb_bilinear(src, width, height, sx, sy);
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], to_u8(sampled[c]), strength);
            }
        }
    }
    out
}

fn farthest_corner_distance(width: usize, height: usize, cx: f32, cy: f32) -> f32 {
    let max_x = width.saturating_sub(1) as f32;
    let max_y = height.saturating_sub(1) as f32;
    [(0.0, 0.0), (max_x, 0.0), (0.0, max_y), (max_x, max_y)]
        .iter()
        .map(|(x, y)| {
            let dx = x - cx;
            let dy = y - cy;
            (dx * dx + dy * dy).sqrt()
        })
        .fold(0.0_f32, f32::max)
}

fn apply_soft_focus(
    src: &[u8],
    width: usize,
    height: usize,
    radius: usize,
    strength: f32,
) -> Vec<u8> {
    if radius == 0 || strength <= 0.0 {
        return src.to_vec();
    }
    let blur = box_blur_rgba(src, width, height, radius);
    let mut out = src.to_vec();
    for i in (0..src.len()).step_by(4) {
        for c in 0..3 {
            let base = src[i + c] as u32;
            let b = blur[i + c] as u32;
            let screen = 255 - ((255 - base) * (255 - b) / 255);
            out[i + c] = lerp_u8(src[i + c], screen as u8, strength);
        }
    }
    out
}

fn apply_mosaic_with_mask(
    src: &[u8],
    width: usize,
    height: usize,
    mask: &[f32],
    params: MosaicParams,
) -> Vec<u8> {
    let long_edge = width.max(height) as u32;
    let block = compute_mosaic_tile_size(long_edge, params.effective_tile_mode()) as usize;
    if block <= 1 || width == 0 || height == 0 {
        return src.to_vec();
    }
    debug_assert_eq!(src.len(), width.saturating_mul(height).saturating_mul(4));
    debug_assert_eq!(mask.len(), width.saturating_mul(height));

    let tiles_x = width.div_ceil(block);
    let tiles_y = height.div_ceil(block);
    let mut tile_stats = vec![(0_u64, 0_u64, 0_u64, 0_u32, 0.0_f32, 0.0_f32); tiles_x * tiles_y];
    for y in 0..height {
        let ty = y / block;
        for x in 0..width {
            let tx = x / block;
            let pi = y * width + x;
            let o = pi * 4;
            let entry = &mut tile_stats[ty * tiles_x + tx];
            entry.0 += src[o] as u64;
            entry.1 += src[o + 1] as u64;
            entry.2 += src[o + 2] as u64;
            entry.3 += 1;
            let alpha = mask.get(pi).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            entry.4 += alpha;
            entry.5 = entry.5.max(alpha);
        }
    }

    let mut out = src.to_vec();
    for y in 0..height {
        let ty = y / block;
        for x in 0..width {
            let tx = x / block;
            let pi = y * width + x;
            let o = pi * 4;
            let (sum_r, sum_g, sum_b, total_count, sum_alpha, max_alpha) =
                tile_stats[ty * tiles_x + tx];
            if total_count == 0 || max_alpha <= f32::EPSILON {
                continue;
            }
            let avg = [
                (sum_r / total_count as u64) as u8,
                (sum_g / total_count as u64) as u8,
                (sum_b / total_count as u64) as u8,
            ];
            let amount = match params.boundary {
                MosaicBoundary::Opaque => max_alpha,
                MosaicBoundary::Translucent => sum_alpha / total_count as f32,
                MosaicBoundary::MaskShape => mask.get(pi).copied().unwrap_or(0.0),
            }
            .clamp(0.0, 1.0);
            if amount <= f32::EPSILON {
                continue;
            }
            for c in 0..3 {
                out[o + c] = lerp_u8(src[o + c], avg[c], amount);
            }
        }
    }
    out
}

fn apply_sharpen(
    src: &[u8],
    width: usize,
    height: usize,
    radius: usize,
    amount: f32,
    threshold: f32,
) -> Vec<u8> {
    if radius == 0 || amount <= f32::EPSILON {
        return src.to_vec();
    }
    let blur = box_blur_rgba(src, width, height, radius);
    let mut out = src.to_vec();
    for i in (0..src.len()).step_by(4) {
        let details = [
            src[i] as f32 - blur[i] as f32,
            src[i + 1] as f32 - blur[i + 1] as f32,
            src[i + 2] as f32 - blur[i + 2] as f32,
        ];
        let detail_strength = details
            .iter()
            .fold(0.0_f32, |max, detail| max.max(detail.abs()));
        let gate = if threshold <= f32::EPSILON {
            1.0
        } else if detail_strength <= threshold {
            0.0
        } else {
            1.0 - threshold / detail_strength
        };
        if gate <= f32::EPSILON {
            continue;
        }
        for c in 0..3 {
            let base = src[i + c] as f32;
            out[i + c] = (base + details[c] * amount * gate)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn apply_smart_sharpen(
    src: &[u8],
    width: usize,
    height: usize,
    radius: usize,
    amount: f32,
    edge_threshold: f32,
    halo_suppression: f32,
) -> Vec<u8> {
    if radius == 0 || amount <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }
    let blur = box_blur_rgba(src, width, height, radius);
    let mut out = src.to_vec();
    let edge_softness = 0.12_f32.max(edge_threshold * 0.75);
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let edge = luma_edge_strength(src, width, height, x, y);
            let edge_weight = smoothstep(edge_threshold, edge_threshold + edge_softness, edge);
            if edge_weight <= f32::EPSILON {
                continue;
            }
            let base_luma = luma01(
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            );
            for c in 0..3 {
                let base = src[i + c] as f32;
                let detail = base - blur[i + c] as f32;
                let headroom = if detail >= 0.0 {
                    1.0 - base_luma
                } else {
                    base_luma
                };
                let halo_gate = lerp_f32(1.0, smoothstep(0.02, 0.42, headroom), halo_suppression);
                out[i + c] = (base + detail * amount * edge_weight * halo_gate)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

fn luma_edge_strength(src: &[u8], width: usize, height: usize, x: usize, y: usize) -> f32 {
    let xm = x.saturating_sub(1);
    let xp = (x + 1).min(width - 1);
    let ym = y.saturating_sub(1);
    let yp = (y + 1).min(height - 1);
    let left = pixel_luma01(src, width, xm, y);
    let right = pixel_luma01(src, width, xp, y);
    let top = pixel_luma01(src, width, x, ym);
    let bottom = pixel_luma01(src, width, x, yp);
    let dx = right - left;
    let dy = bottom - top;
    (dx * dx + dy * dy).sqrt().min(1.0)
}

fn pixel_luma01(src: &[u8], width: usize, x: usize, y: usize) -> f32 {
    let i = (y * width + x) * 4;
    luma01(
        src[i] as f32 / 255.0,
        src[i + 1] as f32 / 255.0,
        src[i + 2] as f32 / 255.0,
    )
}

fn apply_hsl(src: &[u8], params: HslParams) -> Vec<u8> {
    let hue_shift = params.hue_degrees / 360.0;
    let sat_delta = (params.saturation / 100.0).clamp(-1.0, 1.0);
    let light_delta = (params.lightness / 100.0).clamp(-1.0, 1.0);
    if hue_shift.abs() <= f32::EPSILON
        && sat_delta.abs() <= f32::EPSILON
        && light_delta.abs() <= f32::EPSILON
    {
        return src.to_vec();
    }
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        let (mut h, mut s, mut l) = rgb_to_hsl(
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        );
        h = wrap01(h + hue_shift);
        s = (s * (1.0 + sat_delta)).clamp(0.0, 1.0);
        l = (l + light_delta * 0.5).clamp(0.0, 1.0);
        let [r, g, b] = hsl_to_rgb(h, s, l);
        px[0] = to_u8(r);
        px[1] = to_u8(g);
        px[2] = to_u8(b);
    }
    out
}

const COLOR_MIXER_BAND_CENTERS: [f32; COLOR_MIXER_BAND_COUNT] =
    [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 285.0, 320.0];

fn apply_color_mixer(src: &[u8], params: ColorMixerParams) -> Vec<u8> {
    if params.bands.iter().all(|band| band.is_identity()) {
        return src.to_vec();
    }
    let range = params.range_degrees.clamp(8.0, 90.0);
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        let (mut h, mut s, mut l) = rgb_to_hsl(
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        );
        let hue_degrees = h * 360.0;
        let saturation_guard = smoothstep(0.03, 0.16, s);
        if saturation_guard <= f32::EPSILON {
            continue;
        }
        let mut hue_delta = 0.0_f32;
        let mut sat_delta = 0.0_f32;
        let mut light_delta = 0.0_f32;
        for (idx, band) in params.bands.iter().enumerate() {
            if band.is_identity() {
                continue;
            }
            let weight = hue_band_weight(hue_degrees, COLOR_MIXER_BAND_CENTERS[idx], range)
                * saturation_guard;
            if weight <= f32::EPSILON {
                continue;
            }
            hue_delta += weight * (band.hue_degrees / 360.0).clamp(-0.5, 0.5);
            sat_delta += weight * (band.saturation / 100.0).clamp(-1.0, 1.0);
            light_delta += weight * (band.lightness / 100.0).clamp(-1.0, 1.0);
        }
        if hue_delta.abs() <= f32::EPSILON
            && sat_delta.abs() <= f32::EPSILON
            && light_delta.abs() <= f32::EPSILON
        {
            continue;
        }
        h = wrap01(h + hue_delta);
        s = (s * (1.0 + sat_delta.clamp(-1.0, 1.0))).clamp(0.0, 1.0);
        l = (l + light_delta.clamp(-1.0, 1.0) * 0.5).clamp(0.0, 1.0);
        let [r, g, b] = hsl_to_rgb(h, s, l);
        px[0] = to_u8(r);
        px[1] = to_u8(g);
        px[2] = to_u8(b);
    }
    out
}

fn hue_band_weight(hue_degrees: f32, center_degrees: f32, range_degrees: f32) -> f32 {
    let delta = (hue_degrees - center_degrees).rem_euclid(360.0);
    let distance = delta.min(360.0 - delta);
    if distance >= range_degrees {
        0.0
    } else {
        let t = 1.0 - distance / range_degrees.max(f32::EPSILON);
        t * t * (3.0 - 2.0 * t)
    }
}

fn apply_look(src: &[u8], params: LookParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if params.preset == LookPreset::None || strength <= f32::EPSILON {
        return src.to_vec();
    }
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        let r = px[0] as f32 / 255.0;
        let g = px[1] as f32 / 255.0;
        let b = px[2] as f32 / 255.0;
        let graded = look_rgb([r, g, b], params.preset);
        px[0] = to_u8(lerp_f32(r, graded[0], strength));
        px[1] = to_u8(lerp_f32(g, graded[1], strength));
        px[2] = to_u8(lerp_f32(b, graded[2], strength));
    }
    out
}

fn apply_cube_lut(src: &[u8], params: &CubeLutParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if !params.is_loaded() || strength <= f32::EPSILON {
        return src.to_vec();
    }
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        let rgb = [
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        ];
        let sampled = sample_cube_lut(params, rgb);
        px[0] = to_u8(lerp_f32(rgb[0], sampled[0], strength));
        px[1] = to_u8(lerp_f32(rgb[1], sampled[1], strength));
        px[2] = to_u8(lerp_f32(rgb[2], sampled[2], strength));
    }
    out
}

fn sample_cube_lut(params: &CubeLutParams, rgb: [f32; 3]) -> [f32; 3] {
    let size = params.size;
    let normalize = |value: f32, min: f32, max: f32| {
        ((value - min) / (max - min).max(f32::EPSILON)).clamp(0.0, 1.0)
    };
    let r = normalize(rgb[0], params.domain_min[0], params.domain_max[0]) * (size - 1) as f32;
    let g = normalize(rgb[1], params.domain_min[1], params.domain_max[1]) * (size - 1) as f32;
    let b = normalize(rgb[2], params.domain_min[2], params.domain_max[2]) * (size - 1) as f32;
    let r0 = r.floor() as usize;
    let g0 = g.floor() as usize;
    let b0 = b.floor() as usize;
    let r1 = (r0 + 1).min(size - 1);
    let g1 = (g0 + 1).min(size - 1);
    let b1 = (b0 + 1).min(size - 1);
    let tr = r - r0 as f32;
    let tg = g - g0 as f32;
    let tb = b - b0 as f32;

    let c000 = cube_lut_at(params, r0, g0, b0);
    let c100 = cube_lut_at(params, r1, g0, b0);
    let c010 = cube_lut_at(params, r0, g1, b0);
    let c110 = cube_lut_at(params, r1, g1, b0);
    let c001 = cube_lut_at(params, r0, g0, b1);
    let c101 = cube_lut_at(params, r1, g0, b1);
    let c011 = cube_lut_at(params, r0, g1, b1);
    let c111 = cube_lut_at(params, r1, g1, b1);

    let lerp3 = |a: [f32; 3], b: [f32; 3], t: f32| {
        [
            lerp_f32(a[0], b[0], t),
            lerp_f32(a[1], b[1], t),
            lerp_f32(a[2], b[2], t),
        ]
    };
    let c00 = lerp3(c000, c100, tr);
    let c10 = lerp3(c010, c110, tr);
    let c01 = lerp3(c001, c101, tr);
    let c11 = lerp3(c011, c111, tr);
    let c0 = lerp3(c00, c10, tg);
    let c1 = lerp3(c01, c11, tg);
    lerp3(c0, c1, tb)
}

fn cube_lut_at(params: &CubeLutParams, r: usize, g: usize, b: usize) -> [f32; 3] {
    let size = params.size;
    let idx = (b * size + g) * size + r;
    params.table.get(idx).copied().unwrap_or([0.0, 0.0, 0.0])
}

pub fn parse_cube_lut(
    text: &str,
    fallback_name: &str,
) -> std::result::Result<CubeLutParams, String> {
    let mut name = fallback_name.to_string();
    let mut size = 0_usize;
    let mut domain_min = [0.0, 0.0, 0.0];
    let mut domain_max = [1.0, 1.0, 1.0];
    let mut table = Vec::new();

    for (line_idx, raw_line) in text.lines().enumerate() {
        let line_no = line_idx + 1;
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(first) = parts.next() else {
            continue;
        };
        match first.to_ascii_uppercase().as_str() {
            "TITLE" => {
                let title = line[first.len()..].trim().trim_matches('"');
                if !title.is_empty() {
                    name = title.to_string();
                }
            }
            "LUT_3D_SIZE" => {
                let value = parts
                    .next()
                    .ok_or_else(|| format!("LUT_3D_SIZE の値がありません: {line_no}行目"))?;
                size = value
                    .parse::<usize>()
                    .map_err(|_| format!("LUT_3D_SIZE が数値ではありません: {line_no}行目"))?;
                if !(2..=128).contains(&size) {
                    return Err(format!(
                        "対応していない LUT サイズです: {size} ({line_no}行目)"
                    ));
                }
            }
            "LUT_1D_SIZE" => {
                return Err(format!(
                    "1D LUT は未対応です。3D LUT (.cube) を指定してください: {line_no}行目"
                ));
            }
            "DOMAIN_MIN" => {
                domain_min = parse_cube_triplet(parts, line_no, "DOMAIN_MIN")?;
            }
            "DOMAIN_MAX" => {
                domain_max = parse_cube_triplet(parts, line_no, "DOMAIN_MAX")?;
            }
            "LUT_3D_INPUT_RANGE" => {
                let [min, max] = parse_cube_pair(parts, line_no, "LUT_3D_INPUT_RANGE")?;
                domain_min = [min, min, min];
                domain_max = [max, max, max];
            }
            _ => {
                let r = first
                    .parse::<f32>()
                    .map_err(|_| format!("未対応の .cube 行です: {line_no}行目"))?;
                let g = parts
                    .next()
                    .ok_or_else(|| format!("RGB 値が不足しています: {line_no}行目"))?
                    .parse::<f32>()
                    .map_err(|_| format!("RGB 値が数値ではありません: {line_no}行目"))?;
                let b = parts
                    .next()
                    .ok_or_else(|| format!("RGB 値が不足しています: {line_no}行目"))?
                    .parse::<f32>()
                    .map_err(|_| format!("RGB 値が数値ではありません: {line_no}行目"))?;
                table.push([r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]);
            }
        }
    }

    if size == 0 {
        return Err("LUT_3D_SIZE が見つかりません。".to_string());
    }
    let expected = size.saturating_pow(3);
    if table.len() != expected {
        return Err(format!(
            "LUT データ数が一致しません: 期待 {expected}, 実際 {}",
            table.len()
        ));
    }
    Ok(CubeLutParams {
        name,
        size,
        domain_min,
        domain_max,
        table,
        strength: 1.0,
    })
}

fn parse_cube_triplet<'a>(
    mut parts: impl Iterator<Item = &'a str>,
    line_no: usize,
    label: &str,
) -> std::result::Result<[f32; 3], String> {
    let mut values = [0.0; 3];
    for value in &mut values {
        *value = parts
            .next()
            .ok_or_else(|| format!("{label} の値が不足しています: {line_no}行目"))?
            .parse::<f32>()
            .map_err(|_| format!("{label} が数値ではありません: {line_no}行目"))?;
    }
    Ok(values)
}

fn parse_cube_pair<'a>(
    mut parts: impl Iterator<Item = &'a str>,
    line_no: usize,
    label: &str,
) -> std::result::Result<[f32; 2], String> {
    let mut values = [0.0; 2];
    for value in &mut values {
        *value = parts
            .next()
            .ok_or_else(|| format!("{label} の値が不足しています: {line_no}行目"))?
            .parse::<f32>()
            .map_err(|_| format!("{label} が数値ではありません: {line_no}行目"))?;
    }
    Ok(values)
}

fn apply_posterize(src: &[u8], params: PosterizeParams) -> Vec<u8> {
    let levels = params.levels.clamp(2, 256);
    let strength = params.strength.clamp(0.0, 1.0);
    if levels >= 256 || strength <= f32::EPSILON {
        return src.to_vec();
    }
    let max_level = (levels - 1) as f32;
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        for channel in &mut px[0..3] {
            let original = *channel as f32 / 255.0;
            let quantized = (original * max_level).round() / max_level;
            *channel = to_u8(lerp_f32(original, quantized, strength));
        }
    }
    out
}

fn apply_threshold(src: &[u8], params: ThresholdParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON {
        return src.to_vec();
    }
    let threshold = params.threshold.clamp(0.0, 1.0);
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        let rgb = [
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        ];
        let mut target = if luma01(rgb[0], rgb[1], rgb[2]) >= threshold {
            1.0
        } else {
            0.0
        };
        if params.invert {
            target = 1.0 - target;
        }
        px[0] = to_u8(lerp_f32(rgb[0], target, strength));
        px[1] = to_u8(lerp_f32(rgb[1], target, strength));
        px[2] = to_u8(lerp_f32(rgb[2], target, strength));
    }
    out
}

fn apply_invert(src: &[u8], params: InvertParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON {
        return src.to_vec();
    }
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        for channel in &mut px[0..3] {
            let original = *channel as f32 / 255.0;
            *channel = to_u8(lerp_f32(original, 1.0 - original, strength));
        }
    }
    out
}

fn apply_duotone(src: &[u8], params: DuotoneParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if params.preset == DuotonePreset::None || strength <= f32::EPSILON {
        return src.to_vec();
    }
    let contrast = (params.contrast / 100.0).clamp(-1.0, 1.0);
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        let rgb = [
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        ];
        let mut t = luma01(rgb[0], rgb[1], rgb[2]);
        if contrast.abs() > f32::EPSILON {
            t = ((t - 0.5) * (1.0 + contrast) + 0.5).clamp(0.0, 1.0);
        }
        let mapped = duotone_rgb(t, params.preset);
        px[0] = to_u8(lerp_f32(rgb[0], mapped[0], strength));
        px[1] = to_u8(lerp_f32(rgb[1], mapped[1], strength));
        px[2] = to_u8(lerp_f32(rgb[2], mapped[2], strength));
    }
    out
}

fn duotone_rgb(t: f32, preset: DuotonePreset) -> [f32; 3] {
    match preset {
        DuotonePreset::None => [t, t, t],
        DuotonePreset::SepiaInk => {
            sample_gradient_stops(t, &[(0.0, [0.12, 0.08, 0.04]), (1.0, [0.98, 0.86, 0.58])])
        }
        DuotonePreset::Cyanotype => {
            sample_gradient_stops(t, &[(0.0, [0.02, 0.08, 0.18]), (1.0, [0.74, 0.92, 1.0])])
        }
        DuotonePreset::BlackRed => {
            sample_gradient_stops(t, &[(0.0, [0.02, 0.00, 0.00]), (1.0, [1.0, 0.18, 0.10])])
        }
        DuotonePreset::PurpleGold => {
            sample_gradient_stops(t, &[(0.0, [0.12, 0.04, 0.22]), (1.0, [1.0, 0.78, 0.22])])
        }
        DuotonePreset::TealCream => {
            sample_gradient_stops(t, &[(0.0, [0.02, 0.24, 0.28]), (1.0, [1.0, 0.94, 0.74])])
        }
        DuotonePreset::SunsetTritone => sample_gradient_stops(
            t,
            &[
                (0.0, [0.06, 0.02, 0.14]),
                (0.50, [0.78, 0.18, 0.24]),
                (1.0, [1.0, 0.82, 0.30]),
            ],
        ),
        DuotonePreset::ComicTritone => sample_gradient_stops(
            t,
            &[
                (0.0, [0.02, 0.04, 0.10]),
                (0.54, [0.12, 0.55, 0.92]),
                (1.0, [1.0, 0.95, 0.20]),
            ],
        ),
        DuotonePreset::NoirTritone => sample_gradient_stops(
            t,
            &[
                (0.0, [0.02, 0.02, 0.03]),
                (0.48, [0.28, 0.24, 0.22]),
                (1.0, [0.92, 0.90, 0.82]),
            ],
        ),
    }
}

fn apply_equalize(src: &[u8], params: EqualizeParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || src.len() < 4 {
        return src.to_vec();
    }

    let pixel_count = src.len() / 4;
    let mut hist = [0_usize; 256];
    for px in src.chunks_exact(4) {
        let luma = luma01(
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        );
        hist[(luma * 255.0).round() as usize] += 1;
    }

    let mut cumulative = 0_usize;
    let mut cdf_min = None;
    for count in hist {
        cumulative += count;
        if cumulative > 0 {
            cdf_min = Some(cumulative);
            break;
        }
    }
    let Some(cdf_min) = cdf_min else {
        return src.to_vec();
    };
    if cdf_min >= pixel_count {
        return src.to_vec();
    }

    let denom = (pixel_count - cdf_min) as f32;
    let mut lut = [0.0_f32; 256];
    cumulative = 0;
    for (i, count) in hist.iter().copied().enumerate() {
        cumulative += count;
        lut[i] = (cumulative.saturating_sub(cdf_min) as f32 / denom).clamp(0.0, 1.0);
    }

    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        let r = px[0] as f32 / 255.0;
        let g = px[1] as f32 / 255.0;
        let b = px[2] as f32 / 255.0;
        let luma = luma01(r, g, b);
        let mapped_luma = lut[(luma * 255.0).round() as usize];
        let mapped = if params.preserve_color {
            if luma > 1.0 / 255.0 {
                let scale = mapped_luma / luma;
                [
                    (r * scale).clamp(0.0, 1.0),
                    (g * scale).clamp(0.0, 1.0),
                    (b * scale).clamp(0.0, 1.0),
                ]
            } else {
                [mapped_luma, mapped_luma, mapped_luma]
            }
        } else {
            [mapped_luma, mapped_luma, mapped_luma]
        };
        px[0] = to_u8(lerp_f32(r, mapped[0], strength));
        px[1] = to_u8(lerp_f32(g, mapped[1], strength));
        px[2] = to_u8(lerp_f32(b, mapped[2], strength));
    }
    out
}

fn apply_gradient_map(src: &[u8], params: GradientMapParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if params.preset == GradientMapPreset::None || strength <= f32::EPSILON {
        return src.to_vec();
    }
    let contrast = (params.contrast / 100.0).clamp(-1.0, 1.0);
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        let r = px[0] as f32 / 255.0;
        let g = px[1] as f32 / 255.0;
        let b = px[2] as f32 / 255.0;
        let mut t = luma01(r, g, b);
        if contrast.abs() > f32::EPSILON {
            t = ((t - 0.5) * (1.0 + contrast) + 0.5).clamp(0.0, 1.0);
        }
        let mapped = gradient_map_rgb(t, params.preset);
        px[0] = to_u8(lerp_f32(r, mapped[0], strength));
        px[1] = to_u8(lerp_f32(g, mapped[1], strength));
        px[2] = to_u8(lerp_f32(b, mapped[2], strength));
    }
    out
}

fn gradient_map_rgb(t: f32, preset: GradientMapPreset) -> [f32; 3] {
    match preset {
        GradientMapPreset::None | GradientMapPreset::Mono => {
            sample_gradient_stops(t, &[(0.0, [0.0, 0.0, 0.0]), (1.0, [1.0, 1.0, 1.0])])
        }
        GradientMapPreset::Sepia => sample_gradient_stops(
            t,
            &[
                (0.0, [0.10, 0.07, 0.04]),
                (0.45, [0.55, 0.35, 0.20]),
                (1.0, [1.00, 0.88, 0.62]),
            ],
        ),
        GradientMapPreset::Sunset => sample_gradient_stops(
            t,
            &[
                (0.0, [0.10, 0.03, 0.16]),
                (0.45, [0.86, 0.26, 0.16]),
                (1.0, [1.00, 0.82, 0.42]),
            ],
        ),
        GradientMapPreset::Twilight => sample_gradient_stops(
            t,
            &[
                (0.0, [0.03, 0.05, 0.20]),
                (0.50, [0.30, 0.22, 0.58]),
                (1.0, [0.92, 0.70, 0.92]),
            ],
        ),
        GradientMapPreset::TealOrange => sample_gradient_stops(
            t,
            &[
                (0.0, [0.02, 0.13, 0.18]),
                (0.50, [0.18, 0.46, 0.50]),
                (1.0, [1.00, 0.62, 0.28]),
            ],
        ),
        GradientMapPreset::Cherry => sample_gradient_stops(
            t,
            &[
                (0.0, [0.16, 0.04, 0.10]),
                (0.45, [0.82, 0.34, 0.48]),
                (1.0, [1.00, 0.84, 0.90]),
            ],
        ),
        GradientMapPreset::Forest => sample_gradient_stops(
            t,
            &[
                (0.0, [0.03, 0.10, 0.06]),
                (0.50, [0.18, 0.45, 0.22]),
                (1.0, [0.78, 0.95, 0.58]),
            ],
        ),
        GradientMapPreset::Fire => sample_gradient_stops(
            t,
            &[
                (0.0, [0.05, 0.00, 0.00]),
                (0.38, [0.70, 0.06, 0.02]),
                (0.72, [1.00, 0.46, 0.06]),
                (1.0, [1.00, 0.95, 0.62]),
            ],
        ),
        GradientMapPreset::Ice => sample_gradient_stops(
            t,
            &[
                (0.0, [0.02, 0.06, 0.14]),
                (0.50, [0.22, 0.62, 0.92]),
                (1.0, [0.88, 1.00, 1.00]),
            ],
        ),
    }
}

fn sample_gradient_stops(t: f32, stops: &[(f32, [f32; 3])]) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    let Some(&(first_t, first_rgb)) = stops.first() else {
        return [t, t, t];
    };
    if t <= first_t {
        return first_rgb;
    }
    for pair in stops.windows(2) {
        let (a_t, a_rgb) = pair[0];
        let (b_t, b_rgb) = pair[1];
        if t <= b_t {
            let u = ((t - a_t) / (b_t - a_t).max(f32::EPSILON)).clamp(0.0, 1.0);
            return [
                lerp_f32(a_rgb[0], b_rgb[0], u),
                lerp_f32(a_rgb[1], b_rgb[1], u),
                lerp_f32(a_rgb[2], b_rgb[2], u),
            ];
        }
    }
    stops.last().map(|&(_, rgb)| rgb).unwrap_or(first_rgb)
}

fn look_rgb(rgb: [f32; 3], preset: LookPreset) -> [f32; 3] {
    let [mut r, mut g, mut b] = rgb;
    let luma = luma01(r, g, b);
    match preset {
        LookPreset::None => rgb,
        LookPreset::Sunset => {
            r = (r + 0.12 + luma * 0.08).clamp(0.0, 1.0);
            g = (g + 0.04).clamp(0.0, 1.0);
            b = (b - 0.10 * (1.0 - luma)).clamp(0.0, 1.0);
            adjust_saturation([r, g, b], 1.10)
        }
        LookPreset::Night => {
            r = (r * 0.72).clamp(0.0, 1.0);
            g = (g * 0.86 + 0.02).clamp(0.0, 1.0);
            b = (b * 1.12 + 0.05).clamp(0.0, 1.0);
            adjust_saturation([r, g, b], 0.92)
        }
        LookPreset::BrightSun => {
            r = (r * 1.08 + 0.06).clamp(0.0, 1.0);
            g = (g * 1.06 + 0.05).clamp(0.0, 1.0);
            b = (b * 0.98 + 0.02).clamp(0.0, 1.0);
            adjust_saturation([r, g, b], 1.08)
        }
        LookPreset::Pale => {
            r = (r + 0.05).clamp(0.0, 1.0);
            g = (g + 0.05).clamp(0.0, 1.0);
            b = (b + 0.06).clamp(0.0, 1.0);
            adjust_saturation([r, g, b], 0.78)
        }
        LookPreset::Cool => {
            r = (r * 0.92).clamp(0.0, 1.0);
            g = (g * 1.01 + 0.01).clamp(0.0, 1.0);
            b = (b * 1.10 + 0.04).clamp(0.0, 1.0);
            adjust_saturation([r, g, b], 0.98)
        }
        LookPreset::Warm => {
            r = (r * 1.10 + 0.05).clamp(0.0, 1.0);
            g = (g * 1.03 + 0.02).clamp(0.0, 1.0);
            b = (b * 0.90).clamp(0.0, 1.0);
            adjust_saturation([r, g, b], 1.03)
        }
        LookPreset::RetroFilm => {
            let contrast = 0.88;
            r = ((r - 0.5) * contrast + 0.55).clamp(0.0, 1.0);
            g = ((g - 0.5) * contrast + 0.51).clamp(0.0, 1.0);
            b = ((b - 0.5) * contrast + 0.44).clamp(0.0, 1.0);
            adjust_saturation([r, g, b], 0.86)
        }
        LookPreset::TealOrange => {
            let shadow = 1.0 - luma;
            let highlight = luma;
            r = (r + 0.10 * highlight - 0.05 * shadow).clamp(0.0, 1.0);
            g = (g + 0.04 * shadow).clamp(0.0, 1.0);
            b = (b + 0.10 * shadow - 0.04 * highlight).clamp(0.0, 1.0);
            adjust_saturation([r, g, b], 1.06)
        }
        LookPreset::CherryBlossom => {
            r = (r + 0.08).clamp(0.0, 1.0);
            g = (g + 0.03).clamp(0.0, 1.0);
            b = (b + 0.07).clamp(0.0, 1.0);
            adjust_saturation([r, g, b], 0.88)
        }
        LookPreset::FreshGreen => {
            r = (r * 0.95 + 0.02).clamp(0.0, 1.0);
            g = (g * 1.10 + 0.04).clamp(0.0, 1.0);
            b = (b * 0.96).clamp(0.0, 1.0);
            adjust_saturation([r, g, b], 1.04)
        }
        LookPreset::Moonlight => {
            r = (r * 0.78).clamp(0.0, 1.0);
            g = (g * 0.88 + 0.02).clamp(0.0, 1.0);
            b = (b * 1.18 + 0.08).clamp(0.0, 1.0);
            adjust_saturation([r, g, b], 0.82)
        }
        LookPreset::HighKey => {
            r = (r * 0.90 + 0.12).clamp(0.0, 1.0);
            g = (g * 0.90 + 0.12).clamp(0.0, 1.0);
            b = (b * 0.92 + 0.12).clamp(0.0, 1.0);
            adjust_saturation([r, g, b], 0.86)
        }
        LookPreset::LowKey => {
            r = ((r - 0.5) * 1.10 + 0.40).clamp(0.0, 1.0);
            g = ((g - 0.5) * 1.08 + 0.40).clamp(0.0, 1.0);
            b = ((b - 0.5) * 1.05 + 0.42).clamp(0.0, 1.0);
            adjust_saturation([r, g, b], 0.94)
        }
        LookPreset::Sepia => {
            let tr = 0.393 * r + 0.769 * g + 0.189 * b;
            let tg = 0.349 * r + 0.686 * g + 0.168 * b;
            let tb = 0.272 * r + 0.534 * g + 0.131 * b;
            [tr.clamp(0.0, 1.0), tg.clamp(0.0, 1.0), tb.clamp(0.0, 1.0)]
        }
        LookPreset::Cyberpunk => {
            r = (r * 1.10 + 0.05).clamp(0.0, 1.0);
            g = (g * 0.88).clamp(0.0, 1.0);
            b = (b * 1.18 + 0.06).clamp(0.0, 1.0);
            adjust_saturation([r, g, b], 1.18)
        }
    }
}

fn adjust_saturation(rgb: [f32; 3], scale: f32) -> [f32; 3] {
    let luma = luma01(rgb[0], rgb[1], rgb[2]);
    [
        (luma + (rgb[0] - luma) * scale).clamp(0.0, 1.0),
        (luma + (rgb[1] - luma) * scale).clamp(0.0, 1.0),
        (luma + (rgb[2] - luma) * scale).clamp(0.0, 1.0),
    ]
}

fn apply_bloom(src: &[u8], width: usize, height: usize, params: BloomParams) -> Vec<u8> {
    let radius = params.radius_px.round().clamp(0.0, 120.0) as usize;
    let strength = params.strength.clamp(0.0, 2.0);
    if radius == 0 || strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }
    let threshold = params.threshold.clamp(0.90, 0.9999);
    let inv_range = 1.0 / (1.0 - threshold).max(0.001);
    let mut bright = vec![0_u8; src.len()];
    for i in (0..src.len()).step_by(4) {
        let r = src[i] as f32 / 255.0;
        let g = src[i + 1] as f32 / 255.0;
        let b = src[i + 2] as f32 / 255.0;
        let weight = ((luma01(r, g, b) - threshold) * inv_range).clamp(0.0, 1.0);
        bright[i] = to_u8(r * weight);
        bright[i + 1] = to_u8(g * weight);
        bright[i + 2] = to_u8(b * weight);
        bright[i + 3] = src[i + 3];
    }
    let glow = box_blur_rgba(&bright, width, height, radius);
    let mut out = src.to_vec();
    for i in (0..src.len()).step_by(4) {
        for c in 0..3 {
            let base = src[i + c] as f32 / 255.0;
            let add = glow[i + c] as f32 / 255.0 * strength;
            out[i + c] = to_u8(base + add);
        }
    }
    out
}

fn apply_vignette(src: &[u8], width: usize, height: usize, params: VignetteParams) -> Vec<u8> {
    let strength = params.strength.clamp(-1.0, 1.0);
    if strength.abs() <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }
    let radius = params.radius.clamp(0.0, 1.0);
    let feather = params.feather.clamp(0.001, 1.0);
    let cx = (width.saturating_sub(1)) as f32 * 0.5;
    let cy = (height.saturating_sub(1)) as f32 * 0.5;
    let max_dist = (cx * cx + cy * cy).sqrt().max(1.0);
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let d = ((dx * dx + dy * dy).sqrt() / max_dist).clamp(0.0, 1.0);
            let amount = smoothstep(radius, (radius + feather).min(1.0), d);
            let i = (y * width + x) * 4;
            for c in 0..3 {
                let base = src[i + c] as f32 / 255.0;
                let target = if strength >= 0.0 {
                    base * (1.0 - strength * amount)
                } else {
                    base + (1.0 - base) * (-strength) * amount
                };
                out[i + c] = to_u8(target);
            }
        }
    }
    out
}

fn apply_film_grain(src: &[u8], width: usize, height: usize, params: FilmGrainParams) -> Vec<u8> {
    let amount = params.amount.clamp(0.0, 1.0);
    if amount <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }
    let grain_size = params.size_px.max(1) as usize;
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let noise = signed_noise(
                (x / grain_size) as u32,
                (y / grain_size) as u32,
                params.seed,
            );
            let i = (y * width + x) * 4;
            let luma = luma01(
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            );
            let delta = noise * amount * (0.14 + (1.0 - luma) * 0.08);
            for c in 0..3 {
                out[i + c] = to_u8(src[i + c] as f32 / 255.0 + delta);
            }
        }
    }
    out
}

fn apply_chromatic_aberration(src: &[u8], width: usize, height: usize, offset_px: f32) -> Vec<u8> {
    if offset_px <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }
    let cx = (width.saturating_sub(1)) as f32 * 0.5;
    let cy = (height.saturating_sub(1)) as f32 * 0.5;
    let max_axis = cx.max(cy).max(1.0);
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let nx = (x as f32 - cx) / max_axis;
            let ny = (y as f32 - cy) / max_axis;
            let distance = (nx * nx + ny * ny).sqrt().clamp(0.0, 1.0);
            let shift_x = nx * offset_px * distance;
            let shift_y = ny * offset_px * distance;
            let i = (y * width + x) * 4;
            out[i] = sample_channel_nearest(
                src,
                width,
                height,
                x as f32 + shift_x,
                y as f32 + shift_y,
                0,
            );
            out[i + 1] = src[i + 1];
            out[i + 2] = sample_channel_nearest(
                src,
                width,
                height,
                x as f32 - shift_x,
                y as f32 - shift_y,
                2,
            );
        }
    }
    out
}

fn apply_halftone(src: &[u8], width: usize, height: usize, params: HalftoneParams) -> Vec<u8> {
    let cell = params.cell_px.clamp(2, 96) as usize;
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }
    let mut out = src.to_vec();
    for y0 in (0..height).step_by(cell) {
        for x0 in (0..width).step_by(cell) {
            let x1 = (x0 + cell).min(width);
            let y1 = (y0 + cell).min(height);
            let mut sum_luma = 0.0;
            let mut count = 0.0;
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * width + x) * 4;
                    sum_luma += luma01(
                        src[i] as f32 / 255.0,
                        src[i + 1] as f32 / 255.0,
                        src[i + 2] as f32 / 255.0,
                    );
                    count += 1.0;
                }
            }
            let avg_luma = if count > 0.0 { sum_luma / count } else { 1.0 };
            let cx = x0 as f32 + (x1 - x0) as f32 * 0.5;
            let cy = y0 as f32 + (y1 - y0) as f32 * 0.5;
            let max_radius = (x1 - x0).min(y1 - y0) as f32 * 0.58;
            let dot_radius = (1.0 - avg_luma).sqrt() * max_radius;
            for y in y0..y1 {
                for x in x0..x1 {
                    let dx = x as f32 + 0.5 - cx;
                    let dy = y as f32 + 0.5 - cy;
                    let inside_dot = (dx * dx + dy * dy).sqrt() <= dot_radius;
                    let i = (y * width + x) * 4;
                    for c in 0..3 {
                        let base = src[i + c] as f32 / 255.0;
                        let target = if inside_dot {
                            base * 0.42
                        } else {
                            (base + 0.08).clamp(0.0, 1.0)
                        };
                        out[i + c] = to_u8(lerp_f32(base, target, strength));
                    }
                }
            }
        }
    }
    out
}

fn apply_star_glow(src: &[u8], width: usize, height: usize, params: StarGlowParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 3.0);
    let length = params.length_px.clamp(1.0, 240.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }
    let ray_count = normalize_star_ray_count(params.ray_count);
    let max_steps = length.round().clamp(1.0, 240.0) as usize;
    let rotation = params.rotation_degrees.to_radians();
    let mut dirs = Vec::with_capacity(ray_count as usize);
    for ray in 0..ray_count {
        let angle = rotation + std::f32::consts::TAU * ray as f32 / ray_count as f32;
        dirs.push((angle.cos(), angle.sin()));
    }
    let threshold = params.threshold.clamp(0.0, 0.99);
    let inv_range = 1.0 / (1.0 - threshold).max(0.001);
    let mut streak = vec![0.0_f32; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let o = (y * width + x) * 4;
            let r = src[o] as f32 / 255.0;
            let g = src[o + 1] as f32 / 255.0;
            let b = src[o + 2] as f32 / 255.0;
            let weight = ((luma01(r, g, b) - threshold) * inv_range).clamp(0.0, 1.0);
            let weight = weight * weight;
            if weight <= 0.001 {
                continue;
            }
            let color = [r * weight, g * weight, b * weight];
            for &(dx, dy) in &dirs {
                for step in 1..=max_steps {
                    let distance = step as f32;
                    let sx = x as f32 + dx * distance;
                    let sy = y as f32 + dy * distance;
                    if sx < -0.001
                        || sy < -0.001
                        || sx > width as f32 - 1.0 + 0.001
                        || sy > height as f32 - 1.0 + 0.001
                    {
                        break;
                    }
                    let linear = 1.0 - distance / (max_steps as f32 + 1.0);
                    let falloff = linear.max(0.0) * (-distance / length).exp();
                    add_bilinear_rgb(&mut streak, width, height, sx, sy, color, falloff);
                }
            }
        }
    }
    let mut out = src.to_vec();
    let scale = strength / (ray_count as f32 * 0.5).max(1.0);
    for i in 0..width * height {
        let si = i * 3;
        let oi = i * 4;
        for c in 0..3 {
            let base = src[oi + c] as f32 / 255.0;
            out[oi + c] = to_u8(base + streak[si + c] * scale);
        }
    }
    out
}

fn normalize_star_ray_count(ray_count: u32) -> u32 {
    let mut count = ray_count.clamp(2, 12);
    if count % 2 != 0 {
        count += 1;
    }
    count.clamp(2, 12)
}

fn add_bilinear_rgb(
    dst: &mut [f32],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    rgb: [f32; 3],
    weight: f32,
) {
    if weight <= f32::EPSILON || width == 0 || height == 0 {
        return;
    }
    let x0 = x.floor().clamp(0.0, width.saturating_sub(1) as f32) as usize;
    let y0 = y.floor().clamp(0.0, height.saturating_sub(1) as f32) as usize;
    let x1 = (x0 + 1).min(width.saturating_sub(1));
    let y1 = (y0 + 1).min(height.saturating_sub(1));
    let wx = (x - x0 as f32).clamp(0.0, 1.0);
    let wy = (y - y0 as f32).clamp(0.0, 1.0);
    for (xx, xw) in [(x0, 1.0 - wx), (x1, wx)] {
        for (yy, yw) in [(y0, 1.0 - wy), (y1, wy)] {
            let w = weight * xw * yw;
            if w <= f32::EPSILON {
                continue;
            }
            let i = (yy * width + xx) * 3;
            for c in 0..3 {
                dst[i + c] += rgb[c] * w;
            }
        }
    }
}

fn apply_edge_smooth(src: &[u8], width: usize, height: usize, params: EdgeSmoothParams) -> Vec<u8> {
    let radius = params.radius_px.round().clamp(0.0, 8.0) as i32;
    let strength = params.strength.clamp(0.0, 1.0);
    if radius == 0 || strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }
    let threshold = params.edge_threshold.clamp(1.0, 255.0);
    let threshold_sq = threshold * threshold * 3.0;
    let radius_sq = (radius * radius) as i32;
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let center = [src[i] as f32, src[i + 1] as f32, src[i + 2] as f32];
            let mut sum = [0.0_f32; 3];
            let mut count = 0.0_f32;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx * dx + dy * dy > radius_sq {
                        continue;
                    }
                    let xx = (x as i32 + dx).clamp(0, width as i32 - 1) as usize;
                    let yy = (y as i32 + dy).clamp(0, height as i32 - 1) as usize;
                    let j = (yy * width + xx) * 4;
                    let dr = src[j] as f32 - center[0];
                    let dg = src[j + 1] as f32 - center[1];
                    let db = src[j + 2] as f32 - center[2];
                    if dr * dr + dg * dg + db * db <= threshold_sq {
                        sum[0] += src[j] as f32;
                        sum[1] += src[j + 1] as f32;
                        sum[2] += src[j + 2] as f32;
                        count += 1.0;
                    }
                }
            }
            if count > 0.0 {
                for c in 0..3 {
                    let smoothed = sum[c] / count;
                    out[i + c] = lerp_u8(
                        src[i + c],
                        smoothed.round().clamp(0.0, 255.0) as u8,
                        strength,
                    );
                }
            }
        }
    }
    out
}

fn apply_median<F>(
    src: &[u8],
    width: usize,
    height: usize,
    params: MedianParams,
    cancel: Option<&AtomicBool>,
    mut progress: F,
) -> Result<Vec<u8>>
where
    F: FnMut(f32),
{
    let radius = params.radius_px.round().clamp(0.0, 8.0) as i32;
    let strength = params.strength.clamp(0.0, 1.0);
    if radius == 0 || strength <= f32::EPSILON || width == 0 || height == 0 {
        progress(1.0);
        return Ok(src.to_vec());
    }

    let offsets = circle_offsets(radius);
    let mut out = src.to_vec();
    let mut red = Vec::with_capacity(offsets.len());
    let mut green = Vec::with_capacity(offsets.len());
    let mut blue = Vec::with_capacity(offsets.len());
    for y in 0..height {
        if y % 8 == 0 {
            check_cancel(cancel)?;
            progress((y as f32 / height as f32).clamp(0.0, 1.0));
        }
        for x in 0..width {
            red.clear();
            green.clear();
            blue.clear();
            for (dx, dy) in &offsets {
                let xx = (x as i32 + *dx).clamp(0, width as i32 - 1) as usize;
                let yy = (y as i32 + *dy).clamp(0, height as i32 - 1) as usize;
                let i = (yy * width + xx) * 4;
                red.push(src[i]);
                green.push(src[i + 1]);
                blue.push(src[i + 2]);
            }
            red.sort_unstable();
            green.sort_unstable();
            blue.sort_unstable();
            let mid = offsets.len() / 2;
            let i = (y * width + x) * 4;
            out[i] = lerp_u8(src[i], red[mid], strength);
            out[i + 1] = lerp_u8(src[i + 1], green[mid], strength);
            out[i + 2] = lerp_u8(src[i + 2], blue[mid], strength);
        }
    }
    check_cancel(cancel)?;
    progress(1.0);
    Ok(out)
}

fn blend_rgb_with_mask(base: &mut [u8], effected: &[u8], mask: &[f32]) {
    for (i, amount) in mask.iter().enumerate() {
        let o = i * 4;
        let amount = amount.clamp(0.0, 1.0);
        for c in 0..3 {
            base[o + c] = lerp_u8(base[o + c], effected[o + c], amount);
        }
        // Keep source alpha stable; local adjustments are visual RGB operations.
    }
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0))
        .round()
        .clamp(0.0, 255.0) as u8
}

fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

fn luma01(r: f32, g: f32, b: f32) -> f32 {
    (0.2126 * r + 0.7152 * g + 0.0722 * b).clamp(0.0, 1.0)
}

fn wrap01(v: f32) -> f32 {
    v.rem_euclid(1.0)
}

fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;
    let delta = max - min;
    if delta <= f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = if l > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let h = if (max - r).abs() <= f32::EPSILON {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if (max - g).abs() <= f32::EPSILON {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    (wrap01(h), s.clamp(0.0, 1.0), l.clamp(0.0, 1.0))
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    if s <= f32::EPSILON {
        return [l, l, l];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    [
        hue_to_rgb(p, q, h + 1.0 / 3.0),
        hue_to_rgb(p, q, h),
        hue_to_rgb(p, q, h - 1.0 / 3.0),
    ]
}

fn hue_to_rgb(p: f32, q: f32, t: f32) -> f32 {
    let t = wrap01(t);
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge1 <= edge0 {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn signed_noise(x: u32, y: u32, seed: u32) -> f32 {
    let h = hash_u32(
        seed ^ x.wrapping_mul(0x9E37_79B1)
            ^ y.wrapping_mul(0x85EB_CA77)
            ^ x.rotate_left(13)
            ^ y.rotate_right(7),
    );
    h as f32 / u32::MAX as f32 * 2.0 - 1.0
}

fn hash_u32(mut v: u32) -> u32 {
    v ^= v >> 16;
    v = v.wrapping_mul(0x7FEB_352D);
    v ^= v >> 15;
    v = v.wrapping_mul(0x846C_A68B);
    v ^ (v >> 16)
}

fn sample_channel_nearest(
    src: &[u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    channel: usize,
) -> u8 {
    let xx = x.round().clamp(0.0, width.saturating_sub(1) as f32) as usize;
    let yy = y.round().clamp(0.0, height.saturating_sub(1) as f32) as usize;
    src[(yy * width + xx) * 4 + channel.min(3)]
}

fn sample_rgb_bilinear(src: &[u8], width: usize, height: usize, x: f32, y: f32) -> [f32; 3] {
    if width == 0 || height == 0 {
        return [0.0; 3];
    }
    let x = x.clamp(0.0, width.saturating_sub(1) as f32);
    let y = y.clamp(0.0, height.saturating_sub(1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let i00 = (y0 * width + x0) * 4;
    let i10 = (y0 * width + x1) * 4;
    let i01 = (y1 * width + x0) * 4;
    let i11 = (y1 * width + x1) * 4;
    let mut out = [0.0_f32; 3];
    for c in 0..3 {
        let top = lerp_f32(src[i00 + c] as f32 / 255.0, src[i10 + c] as f32 / 255.0, tx);
        let bottom = lerp_f32(src[i01 + c] as f32 / 255.0, src[i11 + c] as f32 / 255.0, tx);
        out[c] = lerp_f32(top, bottom, ty);
    }
    out
}

fn nearest_pixel_index(width: usize, height: usize, x: f32, y: f32) -> usize {
    let xx = x.round().clamp(0.0, width.saturating_sub(1) as f32) as usize;
    let yy = y.round().clamp(0.0, height.saturating_sub(1) as f32) as usize;
    (yy * width + xx) * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: usize, height: usize, rgba: [u8; 4]) -> RgbaImageBuf {
        let mut pixels = Vec::with_capacity(width * height * 4);
        for _ in 0..width * height {
            pixels.extend_from_slice(&rgba);
        }
        RgbaImageBuf::new(width, height, pixels).unwrap()
    }

    #[test]
    fn tone_brightness_changes_full_mask() {
        let src = solid(1, 1, [64, 64, 64, 255]);
        let layer = LocalAdjustmentLayer::new(
            "bright",
            LocalMask::Full,
            LocalEffect::Tone(ToneParams {
                brightness: 20.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > 64);
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn tone_tint_shifts_green_magenta_axis() {
        let src = solid(1, 1, [128, 128, 128, 77]);
        let magenta_layer = LocalAdjustmentLayer::new(
            "magenta",
            LocalMask::Full,
            LocalEffect::Tone(ToneParams {
                tint: 100.0,
                ..Default::default()
            }),
        );
        let magenta = apply_layers(src.as_ref(), &[magenta_layer]).unwrap();
        assert!(magenta.pixels[0] > magenta.pixels[1]);
        assert!(magenta.pixels[2] > magenta.pixels[1]);
        assert_eq!(magenta.pixels[3], 77);

        let green_layer = LocalAdjustmentLayer::new(
            "green",
            LocalMask::Full,
            LocalEffect::Tone(ToneParams {
                tint: -100.0,
                ..Default::default()
            }),
        );
        let green = apply_layers(src.as_ref(), &[green_layer]).unwrap();
        assert!(green.pixels[1] > green.pixels[0]);
        assert!(green.pixels[1] > green.pixels[2]);
        assert_eq!(green.pixels[3], 77);
    }

    #[test]
    fn none_effect_is_identity() {
        let src = solid(2, 2, [64, 96, 128, 255]);
        let layer = LocalAdjustmentLayer::new("none", LocalMask::Full, LocalEffect::None);
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn apply_layers_with_progress_honors_cancel_flag() {
        let src = solid(2, 2, [64, 96, 128, 255]);
        let layer = LocalAdjustmentLayer::new(
            "tone",
            LocalMask::Full,
            LocalEffect::Tone(ToneParams {
                brightness: 10.0,
                ..ToneParams::default()
            }),
        );
        let cancel = AtomicBool::new(true);
        let err =
            apply_layers_with_progress(src.as_ref(), &[layer], Some(&cancel), |_| {}).unwrap_err();
        assert_eq!(err, LocalAdjustError::Cancelled);
    }

    #[test]
    fn median_reports_incremental_progress() {
        let src = solid(4, 16, [64, 96, 128, 255]);
        let layer = LocalAdjustmentLayer::new(
            "median",
            LocalMask::Full,
            LocalEffect::Median(MedianParams {
                radius_px: 1.0,
                strength: 1.0,
            }),
        );
        let mut progress = Vec::new();
        apply_layers_with_progress(src.as_ref(), &[layer], None, |p| {
            if p.effect_name == "メディアンフィルタ" {
                progress.push(p.percent);
            }
        })
        .unwrap();
        assert!(progress.first().copied().unwrap_or(1.0) <= f32::EPSILON);
        assert!(progress.iter().any(|&p| p >= 0.5));
        assert!(progress.last().copied().unwrap_or(0.0) >= 1.0);
    }

    #[test]
    fn inverted_linear_gradient_flips_sides() {
        let src = solid(4, 1, [0, 0, 0, 255]);
        let mut layer = LocalAdjustmentLayer::new(
            "mask",
            LocalMask::LinearGradient(LinearGradientMask {
                initialized: true,
                start: [0.0, 0.5],
                end: [1.0, 0.5],
            }),
            LocalEffect::Blur(BlurParams::default()),
        );
        let normal = evaluate_layer_mask(src.as_ref(), &layer).unwrap();
        layer.mask_inverted = true;
        let inverted = evaluate_layer_mask(src.as_ref(), &layer).unwrap();
        assert!(normal[0] < normal[3]);
        assert!(inverted[0] > inverted[3]);
    }

    #[test]
    fn radial_gradient_supports_ellipse_radii() {
        let src = solid(101, 101, [0, 0, 0, 255]);
        let layer = LocalAdjustmentLayer::new(
            "ellipse",
            LocalMask::RadialGradient(RadialGradientMask {
                initialized: true,
                center: [0.5, 0.5],
                inner_radius: 0.05,
                inner_radius_y: 0.05,
                outer_radius: 0.35,
                outer_radius_y: 0.12,
            }),
            LocalEffect::Tone(ToneParams::default()),
        );
        let alpha = evaluate_layer_mask(src.as_ref(), &layer).unwrap();
        let center = alpha[50 * 101 + 50];
        let right = alpha[50 * 101 + 75];
        let bottom = alpha[75 * 101 + 50];
        assert!(center > 0.9);
        assert!(right > 0.2);
        assert!(bottom < 0.05);
    }

    #[test]
    fn raster_mask_validates_dimensions() {
        let src = solid(2, 2, [0, 0, 0, 255]);
        let layer = LocalAdjustmentLayer::new(
            "bad",
            LocalMask::Raster(RasterMask {
                width: 1,
                height: 1,
                alpha: vec![1.0],
            }),
            LocalEffect::Tone(ToneParams::default()),
        );
        assert!(evaluate_layer_mask(src.as_ref(), &layer).is_err());
    }

    #[test]
    fn region_mask_uses_selected_labels() {
        let src = solid(3, 1, [0, 0, 0, 255]);
        let mask = RegionMask {
            width: 3,
            height: 1,
            labels: vec![1, 2, 0],
            selected: vec![false, true, false],
        };
        let layer = LocalAdjustmentLayer::new(
            "regions",
            LocalMask::Segmentation(mask),
            LocalEffect::Tone(ToneParams::default()),
        );
        let alpha = evaluate_layer_mask(src.as_ref(), &layer).unwrap();
        assert_eq!(alpha, vec![1.0, 0.0, 0.0]);
    }

    #[test]
    fn manual_override_adds_and_subtracts_after_base_mask() {
        let src = solid(3, 1, [0, 0, 0, 255]);
        let mut layer = LocalAdjustmentLayer::new(
            "manual override",
            LocalMask::Raster(RasterMask {
                width: 3,
                height: 1,
                alpha: vec![0.0, 0.5, 1.0],
            }),
            LocalEffect::Tone(ToneParams::default()),
        );
        layer.manual_override.add = Some(RasterVectorMask {
            width: 3,
            height: 1,
            alpha: vec![1.0, 0.0, 0.0],
            shapes: Vec::new(),
        });
        layer.manual_override.subtract = Some(RasterVectorMask {
            width: 3,
            height: 1,
            alpha: vec![0.0, 0.0, 1.0],
            shapes: Vec::new(),
        });

        let alpha = evaluate_layer_mask(src.as_ref(), &layer).unwrap();

        assert_eq!(alpha, vec![1.0, 0.5, 0.0]);
    }

    #[test]
    fn raster_vector_rect_adds_editable_shape_alpha() {
        let src = solid(5, 5, [0, 0, 0, 255]);
        let mut mask = RasterVectorMask::empty(5, 5);
        mask.shapes.push(MaskShape::Rect {
            op: ShapeOp::Add,
            center: [2.5, 2.5],
            half_w: 1.5,
            half_h: 1.5,
            rotation_rad: 0.0,
        });
        let layer = LocalAdjustmentLayer::new(
            "manual",
            LocalMask::RasterVector(mask),
            LocalEffect::Tone(ToneParams::default()),
        );
        let alpha = evaluate_layer_mask(src.as_ref(), &layer).unwrap();
        assert!(alpha[2 * 5 + 2] > 0.9);
        assert!(alpha[0] < 0.1);
    }

    #[test]
    fn polygon_hit_test_handles_descending_sloped_edges() {
        let points = [[1.0, 1.0], [7.0, 1.0], [4.0, 7.0]];
        assert!(point_in_polygon(3.0, 4.0, &points));
        assert!(!point_in_polygon(1.5, 4.0, &points));
    }

    #[test]
    fn raster_vector_subtract_shape_clears_bitmap_alpha() {
        let src = solid(5, 5, [0, 0, 0, 255]);
        let mut mask = RasterVectorMask {
            width: 5,
            height: 5,
            alpha: vec![1.0; 25],
            shapes: Vec::new(),
        };
        mask.shapes.push(MaskShape::Ellipse {
            op: ShapeOp::Subtract,
            center: [2.5, 2.5],
            rx: 1.5,
            ry: 1.5,
            rotation_rad: 0.0,
        });
        let layer = LocalAdjustmentLayer::new(
            "manual",
            LocalMask::RasterVector(mask),
            LocalEffect::Tone(ToneParams::default()),
        );
        let alpha = evaluate_layer_mask(src.as_ref(), &layer).unwrap();
        assert!(alpha[2 * 5 + 2] < 0.1);
        assert!(alpha[0] > 0.9);
    }

    #[test]
    fn zero_radius_blur_is_identity() {
        let src = RgbaImageBuf::new(2, 1, vec![0, 0, 0, 255, 255, 255, 255, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "blur",
            LocalMask::Full,
            LocalEffect::Blur(BlurParams { radius_px: 0.0 }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn motion_blur_spreads_bright_pixel_horizontally() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "motion",
            LocalMask::Full,
            LocalEffect::MotionBlur(MotionBlurParams {
                distance_px: 4.0,
                angle_degrees: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > 0);
        assert!(out.pixels[8] < 255);
        assert!(out.pixels[16] > 0);
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn tilt_shift_keeps_focus_band_and_blurs_outside() {
        let mut pixels = vec![0_u8; 3 * 3 * 4];
        for px in pixels.chunks_exact_mut(4) {
            px[3] = 11;
        }
        let center = (3 + 1) * 4;
        pixels[center..center + 4].copy_from_slice(&[255, 255, 255, 77]);
        let src = RgbaImageBuf::new(3, 3, pixels).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "tilt",
            LocalMask::Full,
            LocalEffect::TiltShift(TiltShiftParams {
                mode: TiltShiftMode::Linear,
                mode_selected: true,
                range_initialized: true,
                center: [0.5, 0.5],
                angle_degrees: 90.0,
                focus_width: 0.05,
                falloff: 0.10,
                radius: [0.3, 0.3],
                max_radius_px: 1.0,
                strength: 1.0,
                far_only: false,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let top_center = 1 * 4;
        assert!(out.pixels[top_center] > 0);
        assert_eq!(out.pixels[center], 255);
        assert_eq!(out.pixels[center + 3], 77);
        assert_eq!(out.pixels[top_center + 3], 11);
    }

    #[test]
    fn lens_blur_spreads_highlight_and_preserves_alpha() {
        let mut pixels = vec![0_u8; 5 * 5 * 4];
        for px in pixels.chunks_exact_mut(4) {
            px[3] = 123;
        }
        let center = (2 * 5 + 2) * 4;
        pixels[center..center + 4].copy_from_slice(&[255, 255, 255, 123]);
        let src = RgbaImageBuf::new(5, 5, pixels).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "lens",
            LocalMask::Full,
            LocalEffect::LensBlur(LensBlurParams {
                radius_px: 1.0,
                aperture: LensBlurAperture::Circular,
                rotation_degrees: 0.0,
                highlight_threshold: 0.5,
                highlight_boost: 1.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let neighbor = (2 * 5 + 1) * 4;
        assert!(out.pixels[neighbor] > 0);
        assert!(out.pixels[center] < 255);
        assert_eq!(out.pixels[neighbor + 3], 123);
    }

    #[test]
    fn radial_zoom_blur_spreads_from_center_and_preserves_alpha() {
        let mut pixels = vec![0_u8; 5 * 4];
        for px in pixels.chunks_exact_mut(4) {
            px[3] = 99;
        }
        pixels[0..4].copy_from_slice(&[255, 255, 255, 99]);
        let src = RgbaImageBuf::new(5, 1, pixels).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "radial zoom",
            LocalMask::Full,
            LocalEffect::RadialBlur(RadialBlurParams {
                mode: RadialBlurMode::Zoom,
                center: [0.0, 0.5],
                zoom_px: 8.0,
                spin_degrees: 0.0,
                samples: 5,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let neighbor = 4;
        assert!(out.pixels[neighbor] > 0);
        assert_eq!(out.pixels[neighbor + 3], 99);
    }

    #[test]
    fn wave_distortion_horizontal_offsets_pixels_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                10, 10, 10, 99, 40, 40, 40, 99, 80, 80, 80, 99, 120, 120, 120, 99, 160, 160, 160,
                99,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "wave",
            LocalMask::Full,
            LocalEffect::WaveDistortion(WaveDistortionParams {
                mode: WaveDistortionMode::Horizontal,
                amplitude_px: 1.0,
                wavelength_px: 64.0,
                phase_degrees: 90.0,
                center: [0.5, 0.5],
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels[0], src.pixels[4]);
        assert_eq!(out.pixels[3], 99);
    }

    #[test]
    fn wave_distortion_zero_strength_is_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "wave",
            LocalMask::Full,
            LocalEffect::WaveDistortion(WaveDistortionParams {
                mode: WaveDistortionMode::Ripple,
                amplitude_px: 8.0,
                wavelength_px: 16.0,
                phase_degrees: 45.0,
                center: [0.5, 0.5],
                strength: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn spherize_distortion_samples_toward_center() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                10, 10, 10, 77, 40, 40, 40, 77, 80, 80, 80, 77, 120, 120, 120, 77, 160, 160, 160,
                77,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "spherize",
            LocalMask::Full,
            LocalEffect::PinchSpherize(PinchSpherizeParams {
                amount: 1.0,
                radius_px: 4.0,
                center: [0.0, 0.0],
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[8] < src.pixels[8]);
        assert_eq!(out.pixels[11], 77);
    }

    #[test]
    fn pinch_distortion_samples_away_from_center() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                10, 10, 10, 77, 40, 40, 40, 77, 80, 80, 80, 77, 120, 120, 120, 77, 160, 160, 160,
                77,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "pinch",
            LocalMask::Full,
            LocalEffect::PinchSpherize(PinchSpherizeParams {
                amount: -1.0,
                radius_px: 4.0,
                center: [0.0, 0.0],
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[4] > src.pixels[4]);
        assert_eq!(out.pixels[7], 77);
    }

    #[test]
    fn median_removes_isolated_speckle_and_preserves_alpha() {
        let mut pixels = vec![0_u8; 3 * 3 * 4];
        for px in pixels.chunks_exact_mut(4) {
            px[3] = 88;
        }
        let center = (3 + 1) * 4;
        pixels[center..center + 4].copy_from_slice(&[255, 255, 255, 88]);
        let src = RgbaImageBuf::new(3, 3, pixels).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "median",
            LocalMask::Full,
            LocalEffect::Median(MedianParams {
                radius_px: 1.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels[center], 0);
        assert_eq!(out.pixels[center + 1], 0);
        assert_eq!(out.pixels[center + 2], 0);
        assert_eq!(out.pixels[center + 3], 88);
    }

    #[test]
    fn shadow_lift_brightens_dark_pixel() {
        let src = solid(1, 1, [24, 24, 24, 255]);
        let layer = LocalAdjustmentLayer::new(
            "shadows",
            LocalMask::Full,
            LocalEffect::HighlightsShadows(HighlightsShadowsParams {
                shadows: 60.0,
                highlights: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > 24);
    }

    #[test]
    fn clarity_preserves_flat_image() {
        let src = solid(4, 4, [100, 120, 140, 255]);
        let layer = LocalAdjustmentLayer::new(
            "clarity",
            LocalMask::Full,
            LocalEffect::Clarity(ClarityParams {
                amount: 0.8,
                radius_px: 3.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn texture_preserves_flat_image() {
        let src = solid(4, 4, [100, 120, 140, 255]);
        let layer = LocalAdjustmentLayer::new(
            "texture",
            LocalMask::Full,
            LocalEffect::Texture(TextureParams {
                amount: 0.8,
                radius_px: 8.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn texture_enhances_medium_detail() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                96, 96, 96, 255, 128, 128, 128, 255, 160, 160, 160, 255, 128, 128, 128, 255, 96,
                96, 96, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "texture",
            LocalMask::Full,
            LocalEffect::Texture(TextureParams {
                amount: 1.0,
                radius_px: 3.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[8] > src.pixels[8]);
    }

    #[test]
    fn high_pass_overlay_preserves_flat_image() {
        let src = solid(4, 4, [100, 120, 140, 255]);
        let layer = LocalAdjustmentLayer::new(
            "high pass",
            LocalMask::Full,
            LocalEffect::HighPass(HighPassParams {
                amount: 1.0,
                radius_px: 2.0,
                contrast: 1.0,
                detail_only: false,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn high_pass_overlay_enhances_edges() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                96, 96, 96, 255, 128, 128, 128, 255, 160, 160, 160, 255, 128, 128, 128, 255, 96,
                96, 96, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "high pass",
            LocalMask::Full,
            LocalEffect::HighPass(HighPassParams {
                amount: 1.0,
                radius_px: 1.0,
                contrast: 1.0,
                detail_only: false,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[8] > src.pixels[8]);
        assert_eq!(out.pixels[11], 255);
    }

    #[test]
    fn sharpen_threshold_suppresses_low_contrast_detail() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                100, 100, 100, 255, 102, 102, 102, 255, 100, 100, 100, 255, 102, 102, 102, 255,
                100, 100, 100, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "sharpen",
            LocalMask::Full,
            LocalEffect::Sharpen(SharpenParams {
                amount: 2.0,
                radius_px: 1.0,
                threshold: 8.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn sharpen_threshold_keeps_strong_edges() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                40, 40, 40, 255, 40, 40, 40, 255, 220, 220, 220, 255, 220, 220, 220, 255, 220, 220,
                220, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "sharpen",
            LocalMask::Full,
            LocalEffect::Sharpen(SharpenParams {
                amount: 1.0,
                radius_px: 1.0,
                threshold: 20.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[4] < src.pixels[4]);
        assert!(out.pixels[8] > src.pixels[8]);
        assert_eq!(out.pixels[11], 255);
    }

    #[test]
    fn smart_sharpen_preserves_flat_image() {
        let src = solid(4, 4, [100, 120, 140, 255]);
        let layer = LocalAdjustmentLayer::new(
            "smart sharpen",
            LocalMask::Full,
            LocalEffect::SmartSharpen(SmartSharpenParams {
                amount: 1.2,
                radius_px: 2.0,
                edge_threshold: 0.02,
                halo_suppression: 0.5,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn smart_sharpen_threshold_ignores_weak_texture() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                100, 100, 100, 255, 102, 102, 102, 255, 100, 100, 100, 255, 102, 102, 102, 255,
                100, 100, 100, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "smart sharpen",
            LocalMask::Full,
            LocalEffect::SmartSharpen(SmartSharpenParams {
                amount: 2.0,
                radius_px: 1.0,
                edge_threshold: 0.08,
                halo_suppression: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn smart_sharpen_emphasizes_strong_edges() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                40, 40, 40, 255, 40, 40, 40, 255, 180, 180, 180, 255, 180, 180, 180, 255, 180, 180,
                180, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "smart sharpen",
            LocalMask::Full,
            LocalEffect::SmartSharpen(SmartSharpenParams {
                amount: 1.2,
                radius_px: 1.0,
                edge_threshold: 0.05,
                halo_suppression: 0.2,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[4] < src.pixels[4]);
        assert!(out.pixels[8] > src.pixels[8]);
        assert_eq!(out.pixels[11], 255);
    }

    #[test]
    fn default_adjustment_effects_are_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let effects = [
            LocalEffect::Tone(ToneParams::default()),
            LocalEffect::ToneCurve(ToneCurveParams::default()),
            LocalEffect::RgbToneCurve(RgbToneCurveParams::default()),
            LocalEffect::ColorBalance(ColorBalanceParams::default()),
            LocalEffect::ThreeWayColorGrading(ThreeWayColorGradingParams::default()),
            LocalEffect::SelectiveColor(SelectiveColorParams::default()),
            LocalEffect::ChannelMixer(ChannelMixerParams::default()),
            LocalEffect::Clarity(ClarityParams::default()),
            LocalEffect::Texture(TextureParams::default()),
            LocalEffect::HighPass(HighPassParams::default()),
            LocalEffect::HighlightsShadows(HighlightsShadowsParams::default()),
            LocalEffect::Dehaze(DehazeParams::default()),
            LocalEffect::Blur(BlurParams::default()),
            LocalEffect::MotionBlur(MotionBlurParams::default()),
            LocalEffect::TiltShift(TiltShiftParams::default()),
            LocalEffect::LensBlur(LensBlurParams::default()),
            LocalEffect::RadialBlur(RadialBlurParams::default()),
            LocalEffect::WaveDistortion(WaveDistortionParams::default()),
            LocalEffect::PinchSpherize(PinchSpherizeParams::default()),
            LocalEffect::SoftFocus(SoftFocusParams::default()),
            LocalEffect::Mosaic(MosaicParams::default()),
            LocalEffect::Sharpen(SharpenParams::default()),
            LocalEffect::SmartSharpen(SmartSharpenParams::default()),
            LocalEffect::Hsl(HslParams::default()),
            LocalEffect::ColorMixer(ColorMixerParams::default()),
            LocalEffect::Look(LookParams::default()),
            LocalEffect::CubeLut(CubeLutParams::default()),
            LocalEffect::Posterize(PosterizeParams::default()),
            LocalEffect::Threshold(ThresholdParams::default()),
            LocalEffect::Invert(InvertParams::default()),
            LocalEffect::Duotone(DuotoneParams::default()),
            LocalEffect::Equalize(EqualizeParams::default()),
            LocalEffect::GradientMap(GradientMapParams::default()),
            LocalEffect::Bloom(BloomParams::default()),
            LocalEffect::Vignette(VignetteParams::default()),
            LocalEffect::FilmGrain(FilmGrainParams::default()),
            LocalEffect::ChromaticAberration(ChromaticAberrationParams::default()),
            LocalEffect::Halftone(HalftoneParams::default()),
            LocalEffect::StarGlow(StarGlowParams::default()),
            LocalEffect::EdgeSmooth(EdgeSmoothParams::default()),
            LocalEffect::Median(MedianParams::default()),
        ];
        for effect in effects {
            let layer = LocalAdjustmentLayer::new("identity", LocalMask::Full, effect);
            let out = apply_layers(src.as_ref(), &[layer]).unwrap();
            assert_eq!(out.pixels, src.pixels);
        }
    }

    #[test]
    fn gradient_map_recolors_by_luma() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![0, 0, 0, 255, 128, 128, 128, 255, 255, 255, 255, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "gradient",
            LocalMask::Full,
            LocalEffect::GradientMap(GradientMapParams {
                preset: GradientMapPreset::Fire,
                strength: 1.0,
                contrast: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] < out.pixels[8]);
        assert!(out.pixels[9] > out.pixels[1]);
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn color_mixer_adjusts_matching_hue_band() {
        let src = RgbaImageBuf::new(2, 1, vec![220, 30, 30, 255, 30, 40, 220, 255]).unwrap();
        let mut params = ColorMixerParams::default();
        params.bands[0].hue_degrees = 120.0;
        let layer =
            LocalAdjustmentLayer::new("mixer", LocalMask::Full, LocalEffect::ColorMixer(params));
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(
            out.pixels[1] > out.pixels[0],
            "red band should shift toward green"
        );
        assert_eq!(
            &out.pixels[4..8],
            &src.pixels[4..8],
            "blue pixel should stay outside the red band"
        );
    }

    #[test]
    fn rgb_tone_curve_adjusts_channels_independently() {
        let src = RgbaImageBuf::new(1, 1, vec![128, 128, 128, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "rgb curve",
            LocalMask::Full,
            LocalEffect::RgbToneCurve(RgbToneCurveParams {
                red: [0.0, 0.35, 0.65, 0.86, 1.0],
                blue: [0.0, 0.16, 0.35, 0.64, 1.0],
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > src.pixels[0]);
        assert_eq!(out.pixels[1], src.pixels[1]);
        assert!(out.pixels[2] < src.pixels[2]);
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn color_balance_targets_shadow_range() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![40, 40, 40, 255, 128, 128, 128, 255, 220, 220, 220, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "balance",
            LocalMask::Full,
            LocalEffect::ColorBalance(ColorBalanceParams {
                shadows: ColorBalanceRange {
                    cyan_red: 80.0,
                    ..Default::default()
                },
                preserve_luma: false,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > src.pixels[0]);
        assert!(out.pixels[1] < src.pixels[1]);
        assert!(out.pixels[2] < src.pixels[2]);
        assert!(out.pixels[0] - src.pixels[0] > out.pixels[8] - src.pixels[8]);
        assert_eq!(out.pixels[3], 255);
        assert_eq!(out.pixels[11], 255);
    }

    #[test]
    fn three_way_color_grading_tints_shadows_more_than_highlights() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![40, 40, 40, 255, 128, 128, 128, 255, 220, 220, 220, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "grade",
            LocalMask::Full,
            LocalEffect::ThreeWayColorGrading(ThreeWayColorGradingParams {
                shadows: ColorGradeWheel {
                    hue_degrees: 220.0,
                    saturation: 70.0,
                    luminance: 0.0,
                },
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[2] > src.pixels[2]);
        assert!(out.pixels[0] < src.pixels[0]);
        assert!(out.pixels[2] - src.pixels[2] > out.pixels[10] - src.pixels[10]);
        assert_eq!(out.pixels[3], 255);
        assert_eq!(out.pixels[11], 255);
    }

    #[test]
    fn selective_color_changes_target_hue_only() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![220, 20, 20, 255, 20, 220, 20, 255, 20, 20, 220, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "selective",
            LocalMask::Full,
            LocalEffect::SelectiveColor(SelectiveColorParams {
                target_hue_degrees: 0.0,
                range_degrees: 10.0,
                feather_degrees: 8.0,
                hue_degrees: 120.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] < src.pixels[0]);
        assert!(out.pixels[1] > src.pixels[1]);
        assert_eq!(&out.pixels[4..8], &src.pixels[4..8]);
        assert_eq!(&out.pixels[8..12], &src.pixels[8..12]);
    }

    #[test]
    fn channel_mixer_monochrome_uses_color_weights() {
        let src =
            RgbaImageBuf::new(3, 1, vec![200, 0, 0, 255, 0, 200, 0, 255, 0, 0, 200, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "mixer",
            LocalMask::Full,
            LocalEffect::ChannelMixer(ChannelMixerParams {
                monochrome: true,
                mono_output: [100.0, 0.0, 0.0],
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[200, 200, 200, 255]);
        assert_eq!(&out.pixels[4..8], &[0, 0, 0, 255]);
        assert_eq!(&out.pixels[8..12], &[0, 0, 0, 255]);
    }

    #[test]
    fn channel_mixer_can_swap_color_channels() {
        let src = RgbaImageBuf::new(1, 1, vec![10, 40, 200, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "swap",
            LocalMask::Full,
            LocalEffect::ChannelMixer(ChannelMixerParams {
                red_output: [0.0, 0.0, 100.0],
                green_output: [0.0, 100.0, 0.0],
                blue_output: [100.0, 0.0, 0.0],
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels, &[200, 40, 10, 255]);
    }

    #[test]
    fn mosaic_tile_size_supports_ratio_and_fixed_modes() {
        assert_eq!(
            compute_mosaic_tile_size(1400, MosaicTileMode::LongEdgeRatio(1.0)),
            14
        );
        assert_eq!(
            compute_mosaic_tile_size(4000, MosaicTileMode::LongEdgeRatio(2.0)),
            80
        );
        assert_eq!(
            compute_mosaic_tile_size(400, MosaicTileMode::FixedPx(16)),
            16
        );
        assert_eq!(compute_mosaic_tile_size(400, MosaicTileMode::FixedPx(2)), 4);
        assert_eq!(compute_mosaic_tile_size(400, MosaicTileMode::FixedPx(1)), 1);
    }

    #[test]
    fn mosaic_params_honor_legacy_block_px() {
        let params = MosaicParams {
            tile_mode: MosaicTileMode::LongEdgeRatio(2.0),
            boundary: MosaicBoundary::Opaque,
            block_px: 12,
        };
        assert_eq!(params.effective_tile_mode(), MosaicTileMode::FixedPx(12));
    }

    #[test]
    fn mosaic_opaque_boundary_extends_tile_outside_mask() {
        let src = RgbaImageBuf::new(
            4,
            1,
            vec![0, 0, 0, 255, 100, 0, 0, 255, 200, 0, 0, 255, 240, 0, 0, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "mosaic",
            LocalMask::Raster(RasterMask {
                width: 4,
                height: 1,
                alpha: vec![1.0, 0.0, 0.0, 0.0],
            }),
            LocalEffect::Mosaic(MosaicParams {
                tile_mode: MosaicTileMode::FixedPx(4),
                boundary: MosaicBoundary::Opaque,
                block_px: 0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels[8], 135);
    }

    #[test]
    fn mosaic_mask_shape_boundary_stays_inside_mask() {
        let src = RgbaImageBuf::new(
            4,
            1,
            vec![0, 0, 0, 255, 100, 0, 0, 255, 200, 0, 0, 255, 240, 0, 0, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "mosaic",
            LocalMask::Raster(RasterMask {
                width: 4,
                height: 1,
                alpha: vec![1.0, 0.0, 0.0, 0.0],
            }),
            LocalEffect::Mosaic(MosaicParams {
                tile_mode: MosaicTileMode::FixedPx(4),
                boundary: MosaicBoundary::MaskShape,
                block_px: 0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels[0], 135);
        assert_eq!(out.pixels[8], 200);
    }

    #[test]
    fn mosaic_translucent_boundary_uses_tile_coverage() {
        let src = RgbaImageBuf::new(
            4,
            1,
            vec![0, 0, 0, 255, 100, 0, 0, 255, 200, 0, 0, 255, 240, 0, 0, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "mosaic",
            LocalMask::Raster(RasterMask {
                width: 4,
                height: 1,
                alpha: vec![1.0, 0.0, 0.0, 0.0],
            }),
            LocalEffect::Mosaic(MosaicParams {
                tile_mode: MosaicTileMode::FixedPx(4),
                boundary: MosaicBoundary::Translucent,
                block_px: 0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels[8], 184);
    }

    #[test]
    fn vibrance_boosts_low_saturation_color() {
        let src = solid(1, 1, [120, 110, 105, 255]);
        let layer = LocalAdjustmentLayer::new(
            "vibrance",
            LocalMask::Full,
            LocalEffect::Tone(ToneParams {
                vibrance: 80.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > src.pixels[0]);
        assert!(out.pixels[2] < src.pixels[2]);
    }

    #[test]
    fn hsl_hue_shift_changes_red_to_greenish() {
        let src = solid(1, 1, [255, 0, 0, 255]);
        let layer = LocalAdjustmentLayer::new(
            "hsl",
            LocalMask::Full,
            LocalEffect::Hsl(HslParams {
                hue_degrees: 120.0,
                saturation: 0.0,
                lightness: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[1] > 200);
        assert!(out.pixels[0] < 80);
    }

    #[test]
    fn tone_curve_lifts_midtones() {
        let src = solid(1, 1, [128, 128, 128, 255]);
        let layer = LocalAdjustmentLayer::new(
            "curve",
            LocalMask::Full,
            LocalEffect::ToneCurve(ToneCurveParams {
                points: [0.0, 0.35, 0.65, 0.86, 1.0],
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > src.pixels[0]);
    }

    #[test]
    fn dehaze_darkens_hazy_dark_object() {
        let src = RgbaImageBuf::new(2, 1, vec![130, 140, 150, 255, 230, 230, 230, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "dehaze",
            LocalMask::Full,
            LocalEffect::Dehaze(DehazeParams {
                amount: 0.65,
                radius_px: 0.0,
                min_transmission: 0.30,
                saturation: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] < src.pixels[0]);
    }

    #[test]
    fn look_changes_color_when_strength_is_nonzero() {
        let src = solid(1, 1, [90, 100, 120, 255]);
        let layer = LocalAdjustmentLayer::new(
            "look",
            LocalMask::Full,
            LocalEffect::Look(LookParams {
                preset: LookPreset::Sunset,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_ne!(&out.pixels[0..3], &src.pixels[0..3]);
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn look_default_is_unselected_but_ready_to_apply() {
        let params = LookParams::default();
        assert_eq!(params.preset, LookPreset::None);
        assert_eq!(params.strength, 1.0);

        let src = solid(1, 1, [90, 100, 120, 255]);
        let layer = LocalAdjustmentLayer::new("look", LocalMask::Full, LocalEffect::Look(params));
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn cube_lut_parser_reads_three_dimensional_table() {
        let text = r#"
TITLE "Invert"
LUT_3D_SIZE 2
DOMAIN_MIN 0.0 0.0 0.0
DOMAIN_MAX 1.0 1.0 1.0
1.0 1.0 1.0
0.0 1.0 1.0
1.0 0.0 1.0
0.0 0.0 1.0
1.0 1.0 0.0
0.0 1.0 0.0
1.0 0.0 0.0
0.0 0.0 0.0
"#;
        let params = parse_cube_lut(text, "fallback").unwrap();
        assert_eq!(params.name, "Invert");
        assert_eq!(params.size, 2);
        assert_eq!(params.table.len(), 8);
    }

    #[test]
    fn cube_lut_inverts_colors() {
        let params = parse_cube_lut(
            r#"
LUT_3D_SIZE 2
1.0 1.0 1.0
0.0 1.0 1.0
1.0 0.0 1.0
0.0 0.0 1.0
1.0 1.0 0.0
0.0 1.0 0.0
1.0 0.0 0.0
0.0 0.0 0.0
"#,
            "invert",
        )
        .unwrap();
        let src = RgbaImageBuf::new(1, 1, vec![0, 128, 255, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new("lut", LocalMask::Full, LocalEffect::CubeLut(params));
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels[0], 255);
        assert!((126..=128).contains(&out.pixels[1]));
        assert_eq!(out.pixels[2], 0);
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn posterize_quantizes_each_rgb_channel() {
        let src = RgbaImageBuf::new(1, 1, vec![64, 128, 255, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "posterize",
            LocalMask::Full,
            LocalEffect::Posterize(PosterizeParams {
                levels: 4,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, vec![85, 170, 255, 255]);
    }

    #[test]
    fn threshold_binarizes_by_luma() {
        let src = RgbaImageBuf::new(2, 1, vec![40, 40, 40, 255, 220, 220, 220, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "threshold",
            LocalMask::Full,
            LocalEffect::Threshold(ThresholdParams {
                threshold: 0.5,
                invert: false,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[0, 0, 0, 255]);
        assert_eq!(&out.pixels[4..8], &[255, 255, 255, 255]);
    }

    #[test]
    fn threshold_can_invert_output() {
        let src = RgbaImageBuf::new(1, 1, vec![220, 220, 220, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "threshold",
            LocalMask::Full,
            LocalEffect::Threshold(ThresholdParams {
                threshold: 0.5,
                invert: true,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, vec![0, 0, 0, 255]);
    }

    #[test]
    fn invert_reverses_rgb_and_preserves_alpha() {
        let src = RgbaImageBuf::new(1, 1, vec![10, 120, 250, 128]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "invert",
            LocalMask::Full,
            LocalEffect::Invert(InvertParams { strength: 1.0 }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, vec![245, 135, 5, 128]);
    }

    #[test]
    fn duotone_maps_luma_to_two_colors() {
        let src = RgbaImageBuf::new(2, 1, vec![0, 0, 0, 255, 255, 255, 255, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "duotone",
            LocalMask::Full,
            LocalEffect::Duotone(DuotoneParams {
                preset: DuotonePreset::BlackRed,
                strength: 1.0,
                contrast: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[5, 0, 0, 255]);
        assert_eq!(&out.pixels[4..8], &[255, 46, 26, 255]);
    }

    #[test]
    fn equalize_spreads_luminance_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            4,
            1,
            vec![
                64, 64, 64, 10, 96, 96, 96, 20, 128, 128, 128, 30, 160, 160, 160, 40,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "equalize",
            LocalMask::Full,
            LocalEffect::Equalize(EqualizeParams {
                strength: 1.0,
                preserve_color: true,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(
            out.pixels,
            vec![
                0, 0, 0, 10, 85, 85, 85, 20, 170, 170, 170, 30, 255, 255, 255, 40
            ]
        );
    }

    #[test]
    fn bloom_lifts_neighbor_of_bright_pixel() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 20, 20, 255, 255, 255, 255, 255, 20, 20, 20, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "bloom",
            LocalMask::Full,
            LocalEffect::Bloom(BloomParams {
                threshold: 0.7,
                radius_px: 1.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > src.pixels[0]);
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn vignette_darkens_edge_more_than_center() {
        let src = solid(5, 5, [180, 180, 180, 255]);
        let layer = LocalAdjustmentLayer::new(
            "vignette",
            LocalMask::Full,
            LocalEffect::Vignette(VignetteParams {
                strength: 0.6,
                radius: 0.0,
                feather: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let center = out.pixels[(2 * 5 + 2) * 4];
        let corner = out.pixels[0];
        assert!(center > corner);
    }

    #[test]
    fn film_grain_is_deterministic() {
        let src = solid(4, 4, [128, 128, 128, 255]);
        let layer = LocalAdjustmentLayer::new(
            "grain",
            LocalMask::Full,
            LocalEffect::FilmGrain(FilmGrainParams {
                amount: 0.5,
                size_px: 1,
                seed: 42,
            }),
        );
        let out1 = apply_layers(src.as_ref(), &[layer.clone()]).unwrap();
        let out2 = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out1.pixels, out2.pixels);
        assert_ne!(out1.pixels, src.pixels);
    }

    #[test]
    fn chromatic_aberration_preserves_alpha() {
        let src =
            RgbaImageBuf::new(3, 1, vec![255, 0, 0, 201, 0, 255, 0, 202, 0, 0, 255, 203]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "aberration",
            LocalMask::Full,
            LocalEffect::ChromaticAberration(ChromaticAberrationParams { offset_px: 2.0 }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels[3], 201);
        assert_eq!(out.pixels[7], 202);
        assert_eq!(out.pixels[11], 203);
    }

    #[test]
    fn halftone_changes_flat_mid_gray() {
        let src = solid(6, 6, [128, 128, 128, 255]);
        let layer = LocalAdjustmentLayer::new(
            "halftone",
            LocalMask::Full,
            LocalEffect::Halftone(HalftoneParams {
                cell_px: 3,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_ne!(out.pixels, src.pixels);
    }

    #[test]
    fn star_glow_extends_bright_pixel_horizontally() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "star",
            LocalMask::Full,
            LocalEffect::StarGlow(StarGlowParams {
                ray_count: 4,
                rotation_degrees: 0.0,
                threshold: 0.5,
                length_px: 4.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[4] > src.pixels[4]);
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn star_glow_rotation_extends_diagonal_ray() {
        let src = RgbaImageBuf::new(
            3,
            3,
            vec![
                0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255, 0, 0,
                0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "star",
            LocalMask::Full,
            LocalEffect::StarGlow(StarGlowParams {
                ray_count: 4,
                rotation_degrees: 45.0,
                threshold: 0.5,
                length_px: 4.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[(2 * 3 + 2) * 4] > src.pixels[(2 * 3 + 2) * 4]);
    }

    #[test]
    fn edge_smooth_preserves_hard_separated_regions() {
        let src =
            RgbaImageBuf::new(3, 1, vec![0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "edge",
            LocalMask::Full,
            LocalEffect::EdgeSmooth(EdgeSmoothParams {
                radius_px: 1.0,
                strength: 1.0,
                edge_threshold: 20.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels[0], 0);
        assert_eq!(out.pixels[8], 255);
    }
}
