//! Core image operations for local adjustment layer prototypes.
//!
//! The public boundary is intentionally small: RGBA input plus an ordered list
//! of local adjustment layers returns an RGBA image with the same dimensions.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalAdjustError {
    InvalidImageBuffer { expected: usize, actual: usize },
    InvalidMaskBuffer { expected: usize, actual: usize },
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
        }
    }
}

impl std::error::Error for LocalAdjustError {}

pub type Result<T> = std::result::Result<T, LocalAdjustError>;

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

#[derive(Debug, Clone, PartialEq)]
pub struct LocalAdjustmentLayer {
    pub name: String,
    pub enabled: bool,
    pub opacity: f32,
    pub mask: LocalMask,
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
            mask_inverted: false,
            mask_expand_px: 0.0,
            mask_feather_px: 0.0,
            effect,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
pub struct RasterMask {
    pub width: usize,
    pub height: usize,
    pub alpha: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeOp {
    Add,
    Subtract,
}

impl ShapeOp {
    pub fn is_add(self) -> bool {
        matches!(self, Self::Add)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Vertical,
    Horizontal,
    Diagonal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
pub enum LocalEffect {
    None,
    Tone(ToneParams),
    ToneCurve(ToneCurveParams),
    Clarity(ClarityParams),
    HighlightsShadows(HighlightsShadowsParams),
    Dehaze(DehazeParams),
    Blur(BlurParams),
    SoftFocus(SoftFocusParams),
    Mosaic(MosaicParams),
    Sharpen(SharpenParams),
    Hsl(HslParams),
    Look(LookParams),
    Bloom(BloomParams),
    Vignette(VignetteParams),
    FilmGrain(FilmGrainParams),
    ChromaticAberration(ChromaticAberrationParams),
    Halftone(HalftoneParams),
    StarGlow(StarGlowParams),
    EdgeSmooth(EdgeSmoothParams),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneParams {
    pub brightness: f32,
    pub contrast: f32,
    pub gamma: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub temperature: f32,
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlurParams {
    pub radius_px: f32,
}

impl Default for BlurParams {
    fn default() -> Self {
        Self { radius_px: 0.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MosaicParams {
    pub block_px: u32,
}

impl Default for MosaicParams {
    fn default() -> Self {
        Self { block_px: 1 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SharpenParams {
    pub amount: f32,
    pub radius_px: f32,
}

impl Default for SharpenParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius_px: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookPreset {
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
        Self::Sunset
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LookParams {
    pub preset: LookPreset,
    pub strength: f32,
}

impl Default for LookParams {
    fn default() -> Self {
        Self {
            preset: LookPreset::Sunset,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChromaticAberrationParams {
    pub offset_px: f32,
}

impl Default for ChromaticAberrationParams {
    fn default() -> Self {
        Self { offset_px: 0.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
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

pub fn apply_layers(
    src: RgbaImageRef<'_>,
    layers: &[LocalAdjustmentLayer],
) -> Result<RgbaImageBuf> {
    let src = src.validate()?;
    let mut out = RgbaImageBuf::new(src.width, src.height, src.pixels.to_vec())?;
    for layer in layers
        .iter()
        .filter(|layer| layer.enabled && layer.opacity > 0.0)
    {
        apply_layer(&mut out, layer)?;
    }
    Ok(out)
}

pub fn evaluate_layer_mask(
    image: RgbaImageRef<'_>,
    layer: &LocalAdjustmentLayer,
) -> Result<Vec<f32>> {
    let image = image.validate()?;
    let mut alpha = evaluate_raw_mask(image, &layer.mask)?;
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

fn apply_layer(image: &mut RgbaImageBuf, layer: &LocalAdjustmentLayer) -> Result<()> {
    if matches!(layer.effect, LocalEffect::None) {
        return Ok(());
    }
    let mask = evaluate_layer_mask(image.as_ref(), layer)?;
    let effected = match layer.effect {
        LocalEffect::None => unreachable!("None is handled before mask evaluation"),
        LocalEffect::Tone(params) => apply_tone_image(&image.pixels, params),
        LocalEffect::ToneCurve(params) => apply_tone_curve(&image.pixels, params),
        LocalEffect::Clarity(params) => apply_clarity(
            &image.pixels,
            image.width,
            image.height,
            params.radius_px.round().max(0.0) as usize,
            params.amount.clamp(-1.0, 1.0),
        ),
        LocalEffect::HighlightsShadows(params) => apply_highlights_shadows(&image.pixels, params),
        LocalEffect::Dehaze(params) => {
            apply_dehaze(&image.pixels, image.width, image.height, params)
        }
        LocalEffect::Blur(params) => box_blur_rgba(
            &image.pixels,
            image.width,
            image.height,
            params.radius_px.round().max(0.0) as usize,
        ),
        LocalEffect::SoftFocus(params) => apply_soft_focus(
            &image.pixels,
            image.width,
            image.height,
            params.radius_px.round().max(0.0) as usize,
            params.strength.clamp(0.0, 1.0),
        ),
        LocalEffect::Mosaic(params) => apply_mosaic(
            &image.pixels,
            image.width,
            image.height,
            params.block_px.max(1) as usize,
        ),
        LocalEffect::Sharpen(params) => apply_sharpen(
            &image.pixels,
            image.width,
            image.height,
            params.radius_px.round().max(0.0) as usize,
            params.amount.clamp(0.0, 2.0),
        ),
        LocalEffect::Hsl(params) => apply_hsl(&image.pixels, params),
        LocalEffect::Look(params) => apply_look(&image.pixels, params),
        LocalEffect::Bloom(params) => apply_bloom(&image.pixels, image.width, image.height, params),
        LocalEffect::Vignette(params) => {
            apply_vignette(&image.pixels, image.width, image.height, params)
        }
        LocalEffect::FilmGrain(params) => {
            apply_film_grain(&image.pixels, image.width, image.height, params)
        }
        LocalEffect::ChromaticAberration(params) => apply_chromatic_aberration(
            &image.pixels,
            image.width,
            image.height,
            params.offset_px.clamp(0.0, 24.0),
        ),
        LocalEffect::Halftone(params) => {
            apply_halftone(&image.pixels, image.width, image.height, params)
        }
        LocalEffect::StarGlow(params) => {
            apply_star_glow(&image.pixels, image.width, image.height, params)
        }
        LocalEffect::EdgeSmooth(params) => {
            apply_edge_smooth(&image.pixels, image.width, image.height, params)
        }
    };
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
        LocalMask::RasterVector(mask) => {
            mask.validate(image.width, image.height)?;
            let mut alpha: Vec<f32> = mask.alpha.iter().map(|v| v.clamp(0.0, 1.0)).collect();
            rasterize_shapes_into(&mut alpha, image.width, image.height, &mask.shapes);
            Ok(alpha)
        }
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

fn tone_curve_lut(params: ToneCurveParams) -> [u8; 256] {
    let mut lut = [0_u8; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        *v = to_u8(tone_curve_value(i as f32 / 255.0, params.points));
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

fn apply_mosaic(src: &[u8], width: usize, height: usize, block: usize) -> Vec<u8> {
    if block <= 1 || width == 0 || height == 0 {
        return src.to_vec();
    }
    let mut out = src.to_vec();
    for y0 in (0..height).step_by(block) {
        for x0 in (0..width).step_by(block) {
            let x1 = (x0 + block).min(width);
            let y1 = (y0 + block).min(height);
            let mut sum = [0_u64; 4];
            let mut count = 0_u64;
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * width + x) * 4;
                    for c in 0..4 {
                        sum[c] += src[i + c] as u64;
                    }
                    count += 1;
                }
            }
            let avg = [
                (sum[0] / count) as u8,
                (sum[1] / count) as u8,
                (sum[2] / count) as u8,
                (sum[3] / count) as u8,
            ];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * width + x) * 4;
                    out[i..i + 4].copy_from_slice(&avg);
                }
            }
        }
    }
    out
}

fn apply_sharpen(src: &[u8], width: usize, height: usize, radius: usize, amount: f32) -> Vec<u8> {
    if radius == 0 || amount <= f32::EPSILON {
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

fn apply_look(src: &[u8], params: LookParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON {
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

fn look_rgb(rgb: [f32; 3], preset: LookPreset) -> [f32; 3] {
    let [mut r, mut g, mut b] = rgb;
    let luma = luma01(r, g, b);
    match preset {
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
    fn none_effect_is_identity() {
        let src = solid(2, 2, [64, 96, 128, 255]);
        let layer = LocalAdjustmentLayer::new("none", LocalMask::Full, LocalEffect::None);
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
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
            LocalEffect::Clarity(ClarityParams::default()),
            LocalEffect::HighlightsShadows(HighlightsShadowsParams::default()),
            LocalEffect::Dehaze(DehazeParams::default()),
            LocalEffect::Blur(BlurParams::default()),
            LocalEffect::SoftFocus(SoftFocusParams::default()),
            LocalEffect::Mosaic(MosaicParams::default()),
            LocalEffect::Sharpen(SharpenParams::default()),
            LocalEffect::Hsl(HslParams::default()),
            LocalEffect::Look(LookParams::default()),
            LocalEffect::Bloom(BloomParams::default()),
            LocalEffect::Vignette(VignetteParams::default()),
            LocalEffect::FilmGrain(FilmGrainParams::default()),
            LocalEffect::ChromaticAberration(ChromaticAberrationParams::default()),
            LocalEffect::Halftone(HalftoneParams::default()),
            LocalEffect::StarGlow(StarGlowParams::default()),
            LocalEffect::EdgeSmooth(EdgeSmoothParams::default()),
        ];
        for effect in effects {
            let layer = LocalAdjustmentLayer::new("identity", LocalMask::Full, effect);
            let out = apply_layers(src.as_ref(), &[layer]).unwrap();
            assert_eq!(out.pixels, src.pixels);
        }
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
