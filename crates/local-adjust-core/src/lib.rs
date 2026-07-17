//! Core image operations for local adjustment layer prototypes.
//!
//! The public boundary is intentionally small: RGBA input plus an ordered list
//! of local adjustment layers returns an RGBA image with the same dimensions.

use std::collections::VecDeque;
use std::fmt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;
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

fn default_mask_after_effect() -> bool {
    true
}

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

/// A non-destructive local adjustment layer.
///
/// Serialization contract: fields that represent lengths in source-image pixels must end in
/// `_px`, including fields in nested mask/effect parameter structs. Cross-canvas edit-bundle
/// paste discovers and scales those fields by name; normalized coordinates and ratios must not
/// use this suffix.
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
    #[serde(default)]
    pub mask_before_effect: bool,
    #[serde(default = "default_mask_after_effect")]
    pub mask_after_effect: bool,
    pub effect: LocalEffect,
}

impl LocalAdjustmentLayer {
    pub fn new(name: impl Into<String>, mask: LocalMask, effect: LocalEffect) -> Self {
        let mask_application = default_mask_application_for_effect(&effect);
        Self {
            name: name.into(),
            enabled: true,
            opacity: 1.0,
            mask,
            manual_override: ManualMaskOverride::default(),
            mask_inverted: false,
            mask_expand_px: 0.0,
            mask_feather_px: 0.0,
            mask_before_effect: mask_application.before_effect,
            mask_after_effect: mask_application.after_effect,
            effect,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaskApplication {
    pub before_effect: bool,
    pub after_effect: bool,
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

    pub fn resize_masks_to(&mut self, new_w: usize, new_h: usize) {
        let new_w = new_w.max(1);
        let new_h = new_h.max(1);
        if let Some(mask) = &mut self.add {
            mask.resize_to(new_w, new_h);
        }
        if let Some(mask) = &mut self.subtract {
            mask.resize_to(new_w, new_h);
        }
    }

    pub fn masks_match_dims(&self, width: usize, height: usize) -> bool {
        let width = width.max(1);
        let height = height.max(1);
        self.add
            .as_ref()
            .is_none_or(|mask| mask.matches_dims(width, height))
            && self
                .subtract
                .as_ref()
                .is_none_or(|mask| mask.matches_dims(width, height))
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
    Subject(SubjectMask),
    /// Region candidates that can be toggled independently.
    Segmentation(RegionMask),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RasterMask {
    pub width: usize,
    pub height: usize,
    pub alpha: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SubjectMaskRefinement {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_subject_cutout_threshold")]
    pub threshold: f32,
    #[serde(default)]
    pub expand_px: i32,
    #[serde(default = "default_subject_cutout_feather_px")]
    pub feather_px: i32,
}

impl Default for SubjectMaskRefinement {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: default_subject_cutout_threshold(),
            expand_px: 0,
            feather_px: default_subject_cutout_feather_px(),
        }
    }
}

fn default_subject_cutout_threshold() -> f32 {
    0.52
}

fn default_subject_cutout_feather_px() -> i32 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubjectMask {
    pub width: usize,
    pub height: usize,
    pub alpha: Vec<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_alpha: Option<Vec<f32>>,
    #[serde(default)]
    pub refinement: SubjectMaskRefinement,
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

pub fn resize_mask_bilinear(
    src: &[f32],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<f32> {
    if dst_w == 0 || dst_h == 0 {
        return Vec::new();
    }
    let mut dst = vec![0.0; dst_w.saturating_mul(dst_h)];
    if src_w == 0 || src_h == 0 || src.is_empty() {
        return dst;
    }
    let scale_x = if dst_w > 1 {
        (src_w.saturating_sub(1)) as f32 / (dst_w.saturating_sub(1)) as f32
    } else {
        0.0
    };
    let scale_y = if dst_h > 1 {
        (src_h.saturating_sub(1)) as f32 / (dst_h.saturating_sub(1)) as f32
    } else {
        0.0
    };
    for y in 0..dst_h {
        let sy = y as f32 * scale_y;
        let y0 = sy.floor() as usize;
        let y1 = (y0 + 1).min(src_h - 1);
        let fy = sy - y0 as f32;
        for x in 0..dst_w {
            let sx = x as f32 * scale_x;
            let x0 = sx.floor() as usize;
            let x1 = (x0 + 1).min(src_w - 1);
            let fx = sx - x0 as f32;
            let a00 = mask_sample(src, src_w, x0, y0);
            let a10 = mask_sample(src, src_w, x1, y0);
            let a01 = mask_sample(src, src_w, x0, y1);
            let a11 = mask_sample(src, src_w, x1, y1);
            let top = a00 + (a10 - a00) * fx;
            let bottom = a01 + (a11 - a01) * fx;
            dst[y * dst_w + x] = (top + (bottom - top) * fy).clamp(0.0, 1.0);
        }
    }
    dst
}

fn mask_sample(src: &[f32], width: usize, x: usize, y: usize) -> f32 {
    src.get(y.saturating_mul(width).saturating_add(x))
        .copied()
        .unwrap_or(0.0)
        .clamp(0.0, 1.0)
}

fn resize_labels_nearest(
    src: &[u32],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<u32> {
    if dst_w == 0 || dst_h == 0 {
        return Vec::new();
    }
    let mut dst = vec![0; dst_w.saturating_mul(dst_h)];
    if src_w == 0 || src_h == 0 || src.is_empty() {
        return dst;
    }
    for y in 0..dst_h {
        let sy = ((y as f32 + 0.5) * src_h as f32 / dst_h as f32)
            .floor()
            .clamp(0.0, src_h.saturating_sub(1) as f32) as usize;
        for x in 0..dst_w {
            let sx = ((x as f32 + 0.5) * src_w as f32 / dst_w as f32)
                .floor()
                .clamp(0.0, src_w.saturating_sub(1) as f32) as usize;
            dst[y * dst_w + x] = src
                .get(sy.saturating_mul(src_w).saturating_add(sx))
                .copied()
                .unwrap_or(0);
        }
    }
    dst
}

fn scale_mask_shape(shape: &mut MaskShape, sx: f32, sy: f32) {
    match shape {
        MaskShape::Line {
            p0, p1, thickness, ..
        } => {
            p0[0] *= sx;
            p0[1] *= sy;
            p1[0] *= sx;
            p1[1] *= sy;
            *thickness = (*thickness * ((sx.abs() + sy.abs()) * 0.5)).max(1.0);
        }
        MaskShape::Rect {
            center,
            half_w,
            half_h,
            ..
        } => {
            center[0] *= sx;
            center[1] *= sy;
            *half_w *= sx.abs();
            *half_h *= sy.abs();
        }
        MaskShape::Ellipse { center, rx, ry, .. } => {
            center[0] *= sx;
            center[1] *= sy;
            *rx *= sx.abs();
            *ry *= sy.abs();
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

    pub fn resize_to(&mut self, new_w: usize, new_h: usize) {
        let new_w = new_w.max(1);
        let new_h = new_h.max(1);
        if self.matches_dims(new_w, new_h) {
            return;
        }
        self.alpha = resize_mask_bilinear(&self.alpha, self.width, self.height, new_w, new_h);
        self.width = new_w;
        self.height = new_h;
    }

    pub fn matches_dims(&self, width: usize, height: usize) -> bool {
        let width = width.max(1);
        let height = height.max(1);
        self.width == width
            && self.height == height
            && self.alpha.len() == width.saturating_mul(height)
    }
}

impl SubjectMask {
    pub fn empty(width: usize, height: usize) -> Self {
        Self::from_raster(RasterMask::empty(width, height))
    }

    pub fn from_raster(mask: RasterMask) -> Self {
        Self {
            width: mask.width,
            height: mask.height,
            source_alpha: Some(mask.alpha.clone()),
            alpha: mask.alpha,
            refinement: SubjectMaskRefinement::default(),
        }
    }

    pub fn current_raster_mask(&self) -> RasterMask {
        RasterMask {
            width: self.width,
            height: self.height,
            alpha: self.alpha.clone(),
        }
    }

    pub fn source_raster_mask(&self) -> RasterMask {
        RasterMask {
            width: self.width,
            height: self.height,
            alpha: self.source_alpha.as_ref().unwrap_or(&self.alpha).clone(),
        }
    }

    pub fn set_source_from_current(&mut self) {
        self.source_alpha = Some(self.alpha.clone());
    }

    pub fn validate(&self, width: usize, height: usize) -> Result<()> {
        let expected = width.saturating_mul(height);
        if self.width != width || self.height != height || self.alpha.len() != expected {
            return Err(LocalAdjustError::InvalidMaskBuffer {
                expected,
                actual: self.alpha.len(),
            });
        }
        if let Some(source_alpha) = &self.source_alpha
            && source_alpha.len() != expected
        {
            return Err(LocalAdjustError::InvalidMaskBuffer {
                expected,
                actual: source_alpha.len(),
            });
        }
        Ok(())
    }

    pub fn resize_to(&mut self, new_w: usize, new_h: usize) {
        let new_w = new_w.max(1);
        let new_h = new_h.max(1);
        if self.matches_dims(new_w, new_h) {
            return;
        }
        self.alpha = resize_mask_bilinear(&self.alpha, self.width, self.height, new_w, new_h);
        if let Some(source_alpha) = &mut self.source_alpha {
            *source_alpha =
                resize_mask_bilinear(source_alpha, self.width, self.height, new_w, new_h);
        }
        self.width = new_w;
        self.height = new_h;
    }

    pub fn matches_dims(&self, width: usize, height: usize) -> bool {
        let width = width.max(1);
        let height = height.max(1);
        let expected = width.saturating_mul(height);
        self.width == width
            && self.height == height
            && self.alpha.len() == expected
            && self
                .source_alpha
                .as_ref()
                .is_none_or(|alpha| alpha.len() == expected)
    }
}

impl RegionMask {
    pub fn resize_to(&mut self, new_w: usize, new_h: usize) {
        let new_w = new_w.max(1);
        let new_h = new_h.max(1);
        if self.matches_dims(new_w, new_h) {
            return;
        }
        self.labels = resize_labels_nearest(&self.labels, self.width, self.height, new_w, new_h);
        self.width = new_w;
        self.height = new_h;
    }

    pub fn matches_dims(&self, width: usize, height: usize) -> bool {
        let width = width.max(1);
        let height = height.max(1);
        self.width == width
            && self.height == height
            && self.labels.len() == width.saturating_mul(height)
    }
}

impl RasterVectorMask {
    pub fn resize_to(&mut self, new_w: usize, new_h: usize) {
        let new_w = new_w.max(1);
        let new_h = new_h.max(1);
        if self.matches_dims(new_w, new_h) {
            return;
        }
        let dimensions_changed = self.width != new_w || self.height != new_h;
        if dimensions_changed {
            let sx = new_w as f32 / self.width.max(1) as f32;
            let sy = new_h as f32 / self.height.max(1) as f32;
            for shape in &mut self.shapes {
                scale_mask_shape(shape, sx, sy);
            }
        }
        self.alpha = resize_mask_bilinear(&self.alpha, self.width, self.height, new_w, new_h);
        self.width = new_w;
        self.height = new_h;
    }

    pub fn matches_dims(&self, width: usize, height: usize) -> bool {
        let width = width.max(1);
        let height = height.max(1);
        self.width == width
            && self.height == height
            && self.alpha.len() == width.saturating_mul(height)
    }
}

impl LocalMask {
    pub fn resize_to(&mut self, new_w: usize, new_h: usize) {
        let new_w = new_w.max(1);
        let new_h = new_h.max(1);
        match self {
            Self::Raster(mask) => mask.resize_to(new_w, new_h),
            Self::RasterVector(mask) => mask.resize_to(new_w, new_h),
            Self::Subject(mask) => mask.resize_to(new_w, new_h),
            Self::Segmentation(mask) => mask.resize_to(new_w, new_h),
            Self::Full
            | Self::LinearGradient(_)
            | Self::RadialGradient(_)
            | Self::LumaRange(_)
            | Self::ColorRange(_) => {}
        }
    }

    pub fn matches_dims(&self, width: usize, height: usize) -> bool {
        let width = width.max(1);
        let height = height.max(1);
        match self {
            Self::Raster(mask) => mask.matches_dims(width, height),
            Self::RasterVector(mask) => mask.matches_dims(width, height),
            Self::Subject(mask) => mask.matches_dims(width, height),
            Self::Segmentation(mask) => mask.matches_dims(width, height),
            Self::Full
            | Self::LinearGradient(_)
            | Self::RadialGradient(_)
            | Self::LumaRange(_)
            | Self::ColorRange(_) => true,
        }
    }
}

impl LocalAdjustmentLayer {
    pub fn resize_masks_to(&mut self, new_w: usize, new_h: usize) {
        let new_w = new_w.max(1);
        let new_h = new_h.max(1);
        self.mask.resize_to(new_w, new_h);
        self.manual_override.resize_masks_to(new_w, new_h);
    }

    pub fn masks_match_dims(&self, width: usize, height: usize) -> bool {
        let width = width.max(1);
        let height = height.max(1);
        self.mask.matches_dims(width, height)
            && self.manual_override.masks_match_dims(width, height)
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
    PhotoFilter(PhotoFilterParams),
    ThreeWayColorGrading(ThreeWayColorGradingParams),
    SelectiveColor(SelectiveColorParams),
    PartColor(PartColorParams),
    ChannelMixer(ChannelMixerParams),
    MonochromeMixer(MonochromeMixerParams),
    Clarity(ClarityParams),
    Texture(TextureParams),
    HighPass(HighPassParams),
    FrequencySeparation(FrequencySeparationParams),
    HighlightsShadows(HighlightsShadowsParams),
    Dehaze(DehazeParams),
    Blur(BlurParams),
    MotionBlur(MotionBlurParams),
    Wind(WindParams),
    TiltShift(TiltShiftParams),
    LensBlur(LensBlurParams),
    BokehSprite(BokehSpriteParams),
    LensDirt(LensDirtParams),
    RadialBlur(RadialBlurParams),
    WaveDistortion(WaveDistortionParams),
    HeatHaze(HeatHazeParams),
    PinchSpherize(PinchSpherizeParams),
    Twirl(TwirlParams),
    PolarCoordinates(PolarCoordinatesParams),
    GlassDisplacement(GlassDisplacementParams),
    LensCorrection(LensCorrectionParams),
    LineExtract(LineExtractParams),
    ArtisticMedia(ArtisticMediaParams),
    BrushStroke(BrushStrokeParams),
    Cutout(CutoutParams),
    ToonShade(ToonShadeParams),
    Emboss(EmbossParams),
    PixelStylize(PixelStylizeParams),
    Solarize(SolarizeParams),
    GlowingEdges(GlowingEdgesParams),
    OilPaint(OilPaintParams),
    SoftFocus(SoftFocusParams),
    Orton(OrtonParams),
    Mosaic(MosaicParams),
    Sharpen(SharpenParams),
    SmartSharpen(SmartSharpenParams),
    Hsl(HslParams),
    ColorMixer(ColorMixerParams),
    Look(LookParams),
    CubeLut(CubeLutParams),
    Posterize(PosterizeParams),
    RetroPalette(RetroPaletteParams),
    CrtDisplay(CrtDisplayParams),
    Threshold(ThresholdParams),
    Invert(InvertParams),
    Duotone(DuotoneParams),
    Equalize(EqualizeParams),
    GradientMap(GradientMapParams),
    Repair(RepairParams),
    ColorFill(ColorFillParams),
    Frame(FrameParams),
    OutlineStroke(OutlineStrokeParams),
    RimLight(RimLightParams),
    ContactShadow(ContactShadowParams),
    ColorTrace(ColorTraceParams),
    ColorOverlay(ColorOverlayParams),
    NeonGlow(NeonGlowParams),
    DiffuseGlow(DiffuseGlowParams),
    Bloom(BloomParams),
    Halation(HalationParams),
    ColorDodgeGlow(ColorDodgeGlowParams),
    GodRays(GodRaysParams),
    LensFlare(LensFlareParams),
    AnamorphicFlare(AnamorphicFlareParams),
    LightLeak(LightLeakParams),
    BacklightHaze(BacklightHazeParams),
    SpeedLines(SpeedLinesParams),
    RadialFlash(RadialFlashParams),
    CloudFog(CloudFogParams),
    Spotlight(SpotlightParams),
    Vignette(VignetteParams),
    FilmGrain(FilmGrainParams),
    Noise(NoiseParams),
    ChromaticAberration(ChromaticAberrationParams),
    Anaglyph3d(AnaglyphParams),
    Defringe(DefringeParams),
    ScanlineGlitch(ScanlineGlitchParams),
    Vhs(VhsParams),
    DataMosh(DataMoshParams),
    PixelSort(PixelSortParams),
    OldFilm(OldFilmParams),
    WaterCaustics(WaterCausticsParams),
    ParticleOverlay(ParticleOverlayParams),
    Aurora(AuroraParams),
    Halftone(HalftoneParams),
    ScreenTone(ScreenToneParams),
    ColorHalftone(ColorHalftoneParams),
    CmykPlateShift(CmykPlateShiftParams),
    Lithograph(LithographParams),
    Engraving(EngravingParams),
    NewspaperPrint(NewspaperPrintParams),
    Textureizer(TextureizerParams),
    StarGlow(StarGlowParams),
    DiffractionStarburst(DiffractionStarburstParams),
    EdgeSmooth(EdgeSmoothParams),
    Despeckle(DespeckleParams),
    Median(MedianParams),
}

impl LocalEffect {
    pub fn display_label(&self) -> &'static str {
        self.progress_label()
    }

    fn progress_label(&self) -> &'static str {
        match self {
            Self::None => "効果なし",
            Self::Tone(_) => "色調補正",
            Self::ToneCurve(_) => "トーンカーブ",
            Self::RgbToneCurve(_) => "RGBトーンカーブ",
            Self::ColorBalance(_) => "カラーバランス",
            Self::PhotoFilter(_) => "フォトフィルター",
            Self::ThreeWayColorGrading(_) => "3ウェイカラー",
            Self::SelectiveColor(_) => "セレクティブカラー",
            Self::PartColor(_) => "パートカラー",
            Self::ChannelMixer(_) => "チャンネルミキサー",
            Self::MonochromeMixer(_) => "モノクロミキサー",
            Self::Clarity(_) => "明瞭度",
            Self::Texture(_) => "テクスチャ",
            Self::HighPass(_) => "ハイパス",
            Self::FrequencySeparation(_) => "周波数分離",
            Self::HighlightsShadows(_) => "ハイライト/シャドウ",
            Self::Dehaze(_) => "かすみ除去",
            Self::Blur(_) => "ぼかし",
            Self::MotionBlur(_) => "モーションぼかし",
            Self::Wind(_) => "風/スピード",
            Self::TiltShift(_) => "チルトぼかし",
            Self::LensBlur(_) => "レンズぼかし",
            Self::BokehSprite(_) => "玉ボケスプライト",
            Self::LensDirt(_) => "レンズ汚れ/水滴",
            Self::RadialBlur(_) => "放射ぼかし",
            Self::WaveDistortion(_) => "波形ゆがみ",
            Self::HeatHaze(_) => "陽炎/熱揺らぎ",
            Self::PinchSpherize(_) => "つまむ/魚眼",
            Self::Twirl(_) => "渦巻き",
            Self::PolarCoordinates(_) => "極座標",
            Self::GlassDisplacement(_) => "ガラス変位",
            Self::LensCorrection(_) => "レンズ補正",
            Self::LineExtract(_) => "線画抽出",
            Self::ArtisticMedia(_) => "絵画調",
            Self::BrushStroke(_) => "筆致",
            Self::Cutout(_) => "切り絵",
            Self::ToonShade(_) => "トゥーンシェード",
            Self::Emboss(_) => "エンボス",
            Self::PixelStylize(_) => "粒状スタイル",
            Self::Solarize(_) => "ソラリゼーション",
            Self::GlowingEdges(_) => "エッジ光彩",
            Self::OilPaint(_) => "油彩",
            Self::SoftFocus(_) => "ソフトフォーカス",
            Self::Orton(_) => "オートン効果",
            Self::Mosaic(_) => "モザイク",
            Self::Sharpen(_) => "シャープ",
            Self::SmartSharpen(_) => "スマートシャープ",
            Self::Hsl(_) => "色相/HSL",
            Self::ColorMixer(_) => "カラーミキサー",
            Self::Look(_) => "ルック",
            Self::CubeLut(_) => "LUT",
            Self::Posterize(_) => "ポスタライズ",
            Self::RetroPalette(_) => "レトロ減色",
            Self::CrtDisplay(_) => "CRT表示",
            Self::Threshold(_) => "しきい値",
            Self::Invert(_) => "ネガ",
            Self::Duotone(_) => "デュオトーン",
            Self::Equalize(_) => "ヒストグラム均等化",
            Self::GradientMap(_) => "グラデーションマップ",
            Self::Repair(_) => "修復／塗り",
            Self::ColorFill(_) => "塗りつぶし",
            Self::Frame(_) => "フレーム",
            Self::OutlineStroke(_) => "縁取り",
            Self::RimLight(_) => "リムライト",
            Self::ContactShadow(_) => "接触影/AO",
            Self::ColorTrace(_) => "色トレス",
            Self::ColorOverlay(_) => "塗り/グラデーション",
            Self::NeonGlow(_) => "ネオングロー",
            Self::DiffuseGlow(_) => "拡散光彩",
            Self::Bloom(_) => "ブルーム",
            Self::Halation(_) => "ハレーション",
            Self::ColorDodgeGlow(_) => "覆い焼き発光",
            Self::GodRays(_) => "光芒",
            Self::LensFlare(_) => "レンズフレア",
            Self::AnamorphicFlare(_) => "アナモルフィックフレア",
            Self::LightLeak(_) => "ライトリーク",
            Self::BacklightHaze(_) => "逆光ヘイズ",
            Self::SpeedLines(_) => "集中線/スピード線",
            Self::RadialFlash(_) => "集中線フラッシュ",
            Self::CloudFog(_) => "雲/霧",
            Self::Spotlight(_) => "スポットライト",
            Self::Vignette(_) => "ビネット",
            Self::FilmGrain(_) => "フィルム粒子",
            Self::Noise(_) => "ノイズ",
            Self::ChromaticAberration(_) => "色収差",
            Self::Anaglyph3d(_) => "アナグリフ3D",
            Self::Defringe(_) => "色フチ除去",
            Self::ScanlineGlitch(_) => "走査線グリッチ",
            Self::Vhs(_) => "VHS/アナログビデオ",
            Self::DataMosh(_) => "データモッシュ",
            Self::PixelSort(_) => "ピクセルソート",
            Self::OldFilm(_) => "オールドフィルム",
            Self::WaterCaustics(_) => "水中コースティクス",
            Self::ParticleOverlay(_) => "雨/雪/花びら",
            Self::Aurora(_) => "オーロラ",
            Self::Halftone(_) => "ハーフトーン",
            Self::ScreenTone(_) => "スクリーントーン",
            Self::ColorHalftone(_) => "カラーハーフトーン",
            Self::CmykPlateShift(_) => "CMYK版ズレ",
            Self::Lithograph(_) => "リソグラフ",
            Self::Engraving(_) => "銅版画",
            Self::NewspaperPrint(_) => "新聞印刷",
            Self::Textureizer(_) => "テクスチャライザ",
            Self::StarGlow(_) => "クロス光",
            Self::DiffractionStarburst(_) => "回折スターバースト",
            Self::EdgeSmooth(_) => "エッジ保持ぼかし",
            Self::Despeckle(_) => "ディスペックル",
            Self::Median(_) => "メディアンフィルタ",
        }
    }
}

pub fn default_mask_application_for_effect(effect: &LocalEffect) -> MaskApplication {
    match effect {
        LocalEffect::Wind(_)
        | LocalEffect::GlowingEdges(_)
        | LocalEffect::NeonGlow(_)
        | LocalEffect::DiffuseGlow(_)
        | LocalEffect::Bloom(_)
        | LocalEffect::Halation(_)
        | LocalEffect::ColorDodgeGlow(_)
        | LocalEffect::BokehSprite(_)
        | LocalEffect::GodRays(_)
        | LocalEffect::AnamorphicFlare(_)
        | LocalEffect::StarGlow(_)
        | LocalEffect::DiffractionStarburst(_)
        | LocalEffect::OutlineStroke(_)
        | LocalEffect::RimLight(_) => MaskApplication {
            before_effect: true,
            after_effect: false,
        },
        LocalEffect::ContactShadow(_) => MaskApplication {
            before_effect: true,
            after_effect: true,
        },
        _ => MaskApplication {
            before_effect: false,
            after_effect: true,
        },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PhotoFilterPreset {
    #[default]
    Custom,
    Warm85,
    Warm81,
    Cool80,
    Cool82,
    Sepia,
    Sunset,
    Underwater,
    Magenta,
    Green,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PhotoFilterParams {
    #[serde(default)]
    pub preset: PhotoFilterPreset,
    #[serde(default = "default_photo_filter_color_rgb")]
    pub color_rgb: [u8; 3],
    #[serde(default = "default_photo_filter_density")]
    pub density: f32,
    #[serde(default = "default_photo_filter_preserve_luminosity")]
    pub preserve_luminosity: bool,
    #[serde(default)]
    pub strength: f32,
}

impl Default for PhotoFilterParams {
    fn default() -> Self {
        Self {
            preset: PhotoFilterPreset::Custom,
            color_rgb: default_photo_filter_color_rgb(),
            density: default_photo_filter_density(),
            preserve_luminosity: default_photo_filter_preserve_luminosity(),
            strength: 0.0,
        }
    }
}

fn default_photo_filter_color_rgb() -> [u8; 3] {
    [255, 176, 80]
}

fn default_photo_filter_density() -> f32 {
    0.35
}

fn default_photo_filter_preserve_luminosity() -> bool {
    true
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
pub struct PartColorParams {
    pub target_rgb: [u8; 3],
    pub range_degrees: f32,
    pub feather_degrees: f32,
    pub gray_strength: f32,
    pub selected_saturation: f32,
    pub selected_lightness: f32,
}

impl Default for PartColorParams {
    fn default() -> Self {
        Self {
            target_rgb: [220, 40, 40],
            range_degrees: 24.0,
            feather_degrees: 20.0,
            gray_strength: 0.0,
            selected_saturation: 0.0,
            selected_lightness: 0.0,
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
pub struct MonochromeMixerParams {
    pub red: f32,
    pub yellow: f32,
    pub green: f32,
    pub cyan: f32,
    pub blue: f32,
    pub magenta: f32,
    pub contrast: f32,
    pub tint_rgb: [u8; 3],
    pub tint_strength: f32,
    pub strength: f32,
}

impl Default for MonochromeMixerParams {
    fn default() -> Self {
        Self {
            red: 0.0,
            yellow: 0.0,
            green: 0.0,
            cyan: 0.0,
            blue: 0.0,
            magenta: 0.0,
            contrast: 0.0,
            tint_rgb: [196, 132, 68],
            tint_strength: 0.0,
            strength: 0.0,
        }
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
pub struct FrequencySeparationParams {
    /// Blur radius used to split the low-frequency color/tone layer from detail.
    pub radius_px: f32,
    /// Extra smoothing applied to the low-frequency layer to reduce broader blotches.
    pub low_smoothing: f32,
    /// Scale for the extracted high-frequency detail. 1.0 keeps the original detail.
    pub detail_amount: f32,
    /// Additional contrast applied to the detail layer before recomposition.
    pub detail_contrast: f32,
    /// Final blend amount of the recomposed image.
    pub strength: f32,
}

impl Default for FrequencySeparationParams {
    fn default() -> Self {
        Self {
            radius_px: 12.0,
            low_smoothing: 0.0,
            detail_amount: 1.0,
            detail_contrast: 1.0,
            strength: 1.0,
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
pub enum WindDirection {
    #[default]
    Right,
    Left,
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WindSource {
    #[default]
    Bright,
    Dark,
    Edge,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindParams {
    pub direction: WindDirection,
    pub source: WindSource,
    pub distance_px: f32,
    pub threshold: f32,
    pub softness: f32,
    pub turbulence: f32,
    pub strength: f32,
    pub seed: u32,
}

impl Default for WindParams {
    fn default() -> Self {
        Self {
            direction: WindDirection::Right,
            source: WindSource::Bright,
            distance_px: 0.0,
            threshold: 0.45,
            softness: 0.16,
            turbulence: 0.0,
            strength: 0.0,
            seed: 1,
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
pub enum BokehSpriteShape {
    #[default]
    Circle,
    Star,
    Heart,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BokehSpriteParams {
    pub shape: BokehSpriteShape,
    pub threshold: f32,
    pub density: f32,
    pub size_px: f32,
    pub softness: f32,
    pub brightness: f32,
    pub color_strength: f32,
    pub seed: u32,
    pub strength: f32,
}

impl Default for BokehSpriteParams {
    fn default() -> Self {
        Self {
            shape: BokehSpriteShape::Circle,
            threshold: 0.96,
            density: 0.35,
            size_px: 18.0,
            softness: 0.45,
            brightness: 1.0,
            color_strength: 0.35,
            seed: 1,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LensDirtMode {
    #[default]
    Dust,
    WaterDrops,
    Smudges,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LensDirtParams {
    pub mode: LensDirtMode,
    pub density: f32,
    pub size_px: f32,
    pub opacity: f32,
    pub softness: f32,
    pub highlight_response: f32,
    pub distortion_px: f32,
    pub seed: u32,
    pub strength: f32,
}

impl Default for LensDirtParams {
    fn default() -> Self {
        Self {
            mode: LensDirtMode::Dust,
            density: 0.45,
            size_px: 14.0,
            opacity: 0.45,
            softness: 0.45,
            highlight_response: 0.60,
            distortion_px: 6.0,
            seed: 1,
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
pub struct HeatHazeParams {
    pub amplitude_px: f32,
    pub wavelength_px: f32,
    pub rise_px: f32,
    pub turbulence: f32,
    pub blur_px: f32,
    pub phase_degrees: f32,
    pub strength: f32,
}

impl Default for HeatHazeParams {
    fn default() -> Self {
        Self {
            amplitude_px: 0.0,
            wavelength_px: 72.0,
            rise_px: 0.0,
            turbulence: 0.35,
            blur_px: 0.0,
            phase_degrees: 0.0,
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
pub struct TwirlParams {
    pub angle_degrees: f32,
    pub radius_px: f32,
    pub center: [f32; 2],
    pub strength: f32,
}

impl Default for TwirlParams {
    fn default() -> Self {
        Self {
            angle_degrees: 0.0,
            radius_px: 0.0,
            center: [0.5, 0.5],
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PolarCoordinatesMode {
    #[default]
    RectToPolar,
    PolarToRect,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PolarCoordinatesParams {
    pub mode: PolarCoordinatesMode,
    pub center: [f32; 2],
    pub radius_px: f32,
    pub angle_offset_degrees: f32,
    pub invert_radius: bool,
    pub strength: f32,
}

impl Default for PolarCoordinatesParams {
    fn default() -> Self {
        Self {
            mode: PolarCoordinatesMode::RectToPolar,
            center: [0.5, 0.5],
            radius_px: 0.0,
            angle_offset_degrees: 0.0,
            invert_radius: false,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GlassDisplacementMode {
    #[default]
    Frosted,
    Ripple,
    Faceted,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GlassDisplacementParams {
    pub mode: GlassDisplacementMode,
    pub displacement_px: f32,
    pub scale_px: f32,
    pub detail: f32,
    pub angle_degrees: f32,
    pub seed: u32,
    pub strength: f32,
}

impl Default for GlassDisplacementParams {
    fn default() -> Self {
        Self {
            mode: GlassDisplacementMode::Frosted,
            displacement_px: 0.0,
            scale_px: 48.0,
            detail: 0.5,
            angle_degrees: 0.0,
            seed: 1,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LensCorrectionParams {
    /// Positive values correct barrel distortion; negative values correct pincushion distortion.
    pub distortion: f32,
    pub zoom: f32,
    pub center: [f32; 2],
    pub vignette_correction: f32,
    pub strength: f32,
}

impl Default for LensCorrectionParams {
    fn default() -> Self {
        Self {
            distortion: 0.0,
            zoom: 0.0,
            center: [0.5, 0.5],
            vignette_correction: 0.0,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LineExtractMode {
    #[default]
    BlackOnWhite,
    WhiteOnBlack,
    DarkenOriginal,
    LightenOriginal,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LineExtractParams {
    pub mode: LineExtractMode,
    pub threshold: f32,
    pub softness: f32,
    pub thickness_px: f32,
    pub strength: f32,
}

impl Default for LineExtractParams {
    fn default() -> Self {
        Self {
            mode: LineExtractMode::BlackOnWhite,
            threshold: 0.18,
            softness: 0.1,
            thickness_px: 1.0,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ArtisticMediaMode {
    #[default]
    Watercolor,
    ColoredPencil,
    PencilSketch,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ArtisticMediaParams {
    pub mode: ArtisticMediaMode,
    pub radius_px: f32,
    pub edge_strength: f32,
    pub texture: f32,
    pub color_amount: f32,
    pub strength: f32,
    pub seed: u32,
}

impl Default for ArtisticMediaParams {
    fn default() -> Self {
        Self {
            mode: ArtisticMediaMode::Watercolor,
            radius_px: 5.0,
            edge_strength: 0.35,
            texture: 0.25,
            color_amount: 0.85,
            strength: 0.0,
            seed: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BrushStrokeMode {
    #[default]
    DryBrush,
    PaintDaubs,
    PaletteKnife,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BrushStrokeParams {
    pub mode: BrushStrokeMode,
    pub length_px: f32,
    pub radius_px: f32,
    pub angle_degrees: f32,
    pub texture: f32,
    pub edge_strength: f32,
    pub color_amount: f32,
    pub strength: f32,
    pub seed: u32,
}

impl Default for BrushStrokeParams {
    fn default() -> Self {
        Self {
            mode: BrushStrokeMode::DryBrush,
            length_px: 12.0,
            radius_px: 1.0,
            angle_degrees: 0.0,
            texture: 0.5,
            edge_strength: 0.35,
            color_amount: 0.85,
            strength: 0.0,
            seed: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CutoutParams {
    pub levels: u8,
    pub radius_px: f32,
    pub edge_strength: f32,
    pub color_amount: f32,
    pub strength: f32,
}

impl Default for CutoutParams {
    fn default() -> Self {
        Self {
            levels: 5,
            radius_px: 6.0,
            edge_strength: 0.25,
            color_amount: 0.85,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ToonShadeParams {
    pub bands: u8,
    pub softness: f32,
    pub preserve_hue: bool,
    pub shadow_tint_rgb: [u8; 3],
    pub shadow_tint_strength: f32,
    pub light_tint_rgb: [u8; 3],
    pub light_tint_strength: f32,
    pub outline_strength: f32,
    pub strength: f32,
}

impl Default for ToonShadeParams {
    fn default() -> Self {
        Self {
            bands: 4,
            softness: 0.08,
            preserve_hue: true,
            shadow_tint_rgb: [92, 116, 210],
            shadow_tint_strength: 0.0,
            light_tint_rgb: [255, 226, 176],
            light_tint_strength: 0.0,
            outline_strength: 0.0,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EmbossParams {
    pub angle_degrees: f32,
    pub depth: f32,
    pub contrast: f32,
    pub color_amount: f32,
    pub strength: f32,
}

impl Default for EmbossParams {
    fn default() -> Self {
        Self {
            angle_degrees: 135.0,
            depth: 1.0,
            contrast: 0.25,
            color_amount: 0.0,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PixelStylizeMode {
    #[default]
    Crystallize,
    Pointillize,
    Facet,
    Mezzotint,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PixelStylizeParams {
    pub mode: PixelStylizeMode,
    pub cell_px: f32,
    pub edge_strength: f32,
    pub color_amount: f32,
    pub randomness: f32,
    pub strength: f32,
    pub seed: u32,
}

impl Default for PixelStylizeParams {
    fn default() -> Self {
        Self {
            mode: PixelStylizeMode::Crystallize,
            cell_px: 12.0,
            edge_strength: 0.25,
            color_amount: 0.9,
            randomness: 0.55,
            strength: 0.0,
            seed: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SolarizeParams {
    pub threshold: f32,
    pub softness: f32,
    pub inversion: f32,
    pub contrast: f32,
    pub color_amount: f32,
    pub strength: f32,
}

impl Default for SolarizeParams {
    fn default() -> Self {
        Self {
            threshold: 0.55,
            softness: 0.08,
            inversion: 1.0,
            contrast: 0.0,
            color_amount: 1.0,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GlowingEdgesParams {
    pub threshold: f32,
    pub softness: f32,
    pub edge_width_px: f32,
    pub glow_radius_px: f32,
    pub edge_brightness: f32,
    pub glow_strength: f32,
    pub hue_degrees: f32,
    pub color_amount: f32,
    pub background_amount: f32,
    pub strength: f32,
}

impl Default for GlowingEdgesParams {
    fn default() -> Self {
        Self {
            threshold: 0.18,
            softness: 0.10,
            edge_width_px: 1.0,
            glow_radius_px: 8.0,
            edge_brightness: 1.15,
            glow_strength: 0.85,
            hue_degrees: 190.0,
            color_amount: 0.85,
            background_amount: 0.0,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OilPaintParams {
    pub radius_px: f32,
    pub saturation: f32,
    pub contrast: f32,
    pub strength: f32,
}

impl Default for OilPaintParams {
    fn default() -> Self {
        Self {
            radius_px: 5.0,
            saturation: 0.0,
            contrast: 0.0,
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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OrtonParams {
    pub radius_px: f32,
    pub strength: f32,
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
}

impl Default for OrtonParams {
    fn default() -> Self {
        Self {
            radius_px: 28.0,
            strength: 0.0,
            brightness: 0.35,
            contrast: 0.20,
            saturation: 0.15,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RetroPaletteMode {
    Dither1Bit,
    #[default]
    GameBoy,
    Famicom,
    Msx2Plus,
    Pc98,
    GameGear,
    MegaDrive,
    Sfc,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RetroPaletteParams {
    pub mode: RetroPaletteMode,
    pub dither: f32,
    pub strength: f32,
}

impl Default for RetroPaletteParams {
    fn default() -> Self {
        Self {
            mode: RetroPaletteMode::GameBoy,
            dither: 0.14,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrtDisplayMode {
    Simple,
    Full,
    Arcade,
}

impl Default for CrtDisplayMode {
    fn default() -> Self {
        Self::Simple
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CrtDisplayParams {
    pub mode: CrtDisplayMode,
    pub scanline_spacing_px: f32,
    pub scanline_depth: f32,
    pub mask_strength: f32,
    pub curvature: f32,
    pub bloom: f32,
    pub horizontal_blur: f32,
    pub brightness: f32,
    pub strength: f32,
}

impl CrtDisplayParams {
    pub fn preset(mode: CrtDisplayMode) -> Self {
        match mode {
            CrtDisplayMode::Simple => Self {
                mode,
                scanline_spacing_px: 4.0,
                scanline_depth: 0.32,
                mask_strength: 0.12,
                curvature: 0.0,
                bloom: 0.08,
                horizontal_blur: 0.30,
                brightness: 1.22,
                strength: 0.80,
            },
            CrtDisplayMode::Full => Self {
                mode,
                scanline_spacing_px: 4.0,
                scanline_depth: 0.36,
                mask_strength: 0.16,
                curvature: 0.07,
                bloom: 0.25,
                horizontal_blur: 0.40,
                brightness: 1.25,
                strength: 0.88,
            },
            CrtDisplayMode::Arcade => Self {
                mode,
                scanline_spacing_px: 3.0,
                scanline_depth: 0.55,
                mask_strength: 0.26,
                curvature: 0.0,
                bloom: 0.18,
                horizontal_blur: 0.45,
                brightness: 1.55,
                strength: 0.92,
            },
        }
    }
}

impl Default for CrtDisplayParams {
    fn default() -> Self {
        Self {
            strength: 0.0,
            ..Self::preset(CrtDisplayMode::Simple)
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepairMode {
    Solid,
    PreserveLuminance,
    #[default]
    Surrounding,
    Clone,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepairColorSource {
    #[default]
    Surrounding,
    Sampled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepairQuality {
    Fast,
    #[default]
    Standard,
    High,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepairPatchSize {
    /// 品質設定に応じた従来値 (10〜14px)。既存レイヤーとの互換もこの値に寄せる。
    #[default]
    Auto,
    /// 広めのテクスチャ単位を扱う 24px パッチ。
    Standard,
    /// 大きな色面や模様を扱う 48px パッチ。
    Large,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RepairParams {
    #[serde(default)]
    pub mode: RepairMode,
    #[serde(default)]
    pub color_source: RepairColorSource,
    #[serde(default = "default_repair_sampled_rgb")]
    pub sampled_rgb: [u8; 3],
    #[serde(default = "default_repair_sample_radius_px")]
    pub sample_radius_px: f32,
    #[serde(default = "default_repair_search_radius_px")]
    pub search_radius_px: f32,
    #[serde(default = "default_repair_texture_strength")]
    pub texture_strength: f32,
    #[serde(default = "default_repair_color_match_strength")]
    pub color_match_strength: f32,
    #[serde(default)]
    pub quality: RepairQuality,
    #[serde(default)]
    pub patch_size: RepairPatchSize,
    #[serde(default)]
    pub seed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clone_source_uv: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clone_destination_uv: Option<[f32; 2]>,
}

impl Default for RepairParams {
    fn default() -> Self {
        Self {
            mode: RepairMode::Surrounding,
            color_source: RepairColorSource::Surrounding,
            sampled_rgb: default_repair_sampled_rgb(),
            sample_radius_px: default_repair_sample_radius_px(),
            search_radius_px: default_repair_search_radius_px(),
            texture_strength: default_repair_texture_strength(),
            color_match_strength: default_repair_color_match_strength(),
            quality: RepairQuality::Standard,
            patch_size: RepairPatchSize::Auto,
            seed: 0,
            clone_source_uv: None,
            clone_destination_uv: None,
        }
    }
}

fn default_repair_sampled_rgb() -> [u8; 3] {
    [128, 128, 128]
}

fn default_repair_sample_radius_px() -> f32 {
    3.0
}

fn default_repair_search_radius_px() -> f32 {
    96.0
}

fn default_repair_texture_strength() -> f32 {
    0.85
}

fn default_repair_color_match_strength() -> f32 {
    0.75
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorFillParams {
    #[serde(default = "default_color_fill_shape")]
    pub shape: ColorOverlayShape,
    #[serde(default = "default_color_fill_start_rgb")]
    pub start_rgb: [u8; 3],
    #[serde(default = "default_color_fill_middle_rgb")]
    pub middle_rgb: [u8; 3],
    #[serde(default = "default_color_fill_end_rgb")]
    pub end_rgb: [u8; 3],
    #[serde(default)]
    pub middle_enabled: bool,
    #[serde(default = "default_color_fill_midpoint")]
    pub midpoint: f32,
    #[serde(default = "default_color_fill_angle_degrees")]
    pub angle_degrees: f32,
    #[serde(default)]
    pub linear_points_enabled: bool,
    #[serde(default = "default_color_fill_linear_start")]
    pub linear_start: [f32; 2],
    #[serde(default = "default_color_fill_linear_end")]
    pub linear_end: [f32; 2],
    #[serde(default = "default_color_fill_center")]
    pub center: [f32; 2],
    #[serde(default = "default_color_fill_radius")]
    pub radius: f32,
    #[serde(default = "default_color_fill_softness")]
    pub softness: f32,
    #[serde(default = "default_color_fill_opacity")]
    pub opacity: f32,
}

impl Default for ColorFillParams {
    fn default() -> Self {
        Self {
            shape: default_color_fill_shape(),
            start_rgb: default_color_fill_start_rgb(),
            middle_rgb: default_color_fill_middle_rgb(),
            end_rgb: default_color_fill_end_rgb(),
            middle_enabled: false,
            midpoint: default_color_fill_midpoint(),
            angle_degrees: default_color_fill_angle_degrees(),
            linear_points_enabled: false,
            linear_start: default_color_fill_linear_start(),
            linear_end: default_color_fill_linear_end(),
            center: default_color_fill_center(),
            radius: default_color_fill_radius(),
            softness: default_color_fill_softness(),
            opacity: default_color_fill_opacity(),
        }
    }
}

fn default_color_fill_shape() -> ColorOverlayShape {
    ColorOverlayShape::Unselected
}

fn default_color_fill_start_rgb() -> [u8; 3] {
    [245, 247, 252]
}

fn default_color_fill_middle_rgb() -> [u8; 3] {
    [255, 236, 206]
}

fn default_color_fill_end_rgb() -> [u8; 3] {
    [180, 205, 255]
}

fn default_color_fill_midpoint() -> f32 {
    0.5
}

fn default_color_fill_angle_degrees() -> f32 {
    -20.0
}

fn default_color_fill_linear_start() -> [f32; 2] {
    [0.0, 0.5]
}

fn default_color_fill_linear_end() -> [f32; 2] {
    [1.0, 0.5]
}

fn default_color_fill_center() -> [f32; 2] {
    [0.5, 0.5]
}

fn default_color_fill_radius() -> f32 {
    0.85
}

fn default_color_fill_softness() -> f32 {
    0.45
}

fn default_color_fill_opacity() -> f32 {
    1.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FrameMode {
    #[default]
    Border,
    Letterbox,
    RoundedMatte,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FrameParams {
    #[serde(default)]
    pub mode: FrameMode,
    #[serde(default = "default_frame_color_rgb")]
    pub color_rgb: [u8; 3],
    #[serde(default = "default_frame_line_rgb")]
    pub line_rgb: [u8; 3],
    #[serde(default = "default_frame_opacity")]
    pub opacity: f32,
    #[serde(default)]
    pub width_px: f32,
    #[serde(default)]
    pub use_individual_widths: bool,
    #[serde(default)]
    pub top_px: f32,
    #[serde(default)]
    pub right_px: f32,
    #[serde(default)]
    pub bottom_px: f32,
    #[serde(default)]
    pub left_px: f32,
    #[serde(default)]
    pub softness_px: f32,
    #[serde(default)]
    pub line_width_px: f32,
    #[serde(default)]
    pub line_opacity: f32,
    #[serde(default = "default_frame_aspect_ratio")]
    pub aspect_ratio: f32,
    #[serde(default)]
    pub corner_radius_px: f32,
}

impl Default for FrameParams {
    fn default() -> Self {
        Self {
            mode: FrameMode::Border,
            color_rgb: default_frame_color_rgb(),
            line_rgb: default_frame_line_rgb(),
            opacity: default_frame_opacity(),
            width_px: 0.0,
            use_individual_widths: false,
            top_px: 0.0,
            right_px: 0.0,
            bottom_px: 0.0,
            left_px: 0.0,
            softness_px: 0.0,
            line_width_px: 0.0,
            line_opacity: 0.0,
            aspect_ratio: default_frame_aspect_ratio(),
            corner_radius_px: 0.0,
        }
    }
}

fn default_frame_color_rgb() -> [u8; 3] {
    [0, 0, 0]
}

fn default_frame_line_rgb() -> [u8; 3] {
    [255, 255, 255]
}

fn default_frame_opacity() -> f32 {
    1.0
}

fn default_frame_aspect_ratio() -> f32 {
    2.35
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutlineStrokePlacement {
    Outside,
    Inside,
    Center,
}

impl Default for OutlineStrokePlacement {
    fn default() -> Self {
        Self::Outside
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OutlineStrokeParams {
    #[serde(default)]
    pub placement: OutlineStrokePlacement,
    pub width_px: f32,
    pub softness_px: f32,
    pub opacity: f32,
    pub color_rgb: [u8; 3],
}

impl Default for OutlineStrokeParams {
    fn default() -> Self {
        Self {
            placement: OutlineStrokePlacement::Outside,
            width_px: 0.0,
            softness_px: 1.0,
            opacity: 0.0,
            color_rgb: [0, 0, 0],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RimLightParams {
    pub light_angle_degrees: f32,
    pub width_px: f32,
    pub falloff: f32,
    pub strength: f32,
    pub color_rgb: [u8; 3],
    pub wrap: f32,
}

impl Default for RimLightParams {
    fn default() -> Self {
        Self {
            light_angle_degrees: 0.0,
            width_px: 0.0,
            falloff: 0.45,
            strength: 0.0,
            color_rgb: [220, 240, 255],
            wrap: 0.15,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ContactShadowParams {
    pub radius_px: f32,
    pub softness_px: f32,
    pub strength: f32,
    pub color_rgb: [u8; 3],
    pub direction_degrees: f32,
    pub directionality: f32,
}

impl Default for ContactShadowParams {
    fn default() -> Self {
        Self {
            radius_px: 0.0,
            softness_px: 4.0,
            strength: 0.0,
            color_rgb: [18, 16, 20],
            direction_degrees: 90.0,
            directionality: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorTraceParams {
    pub strength: f32,
    pub line_threshold: f32,
    pub softness: f32,
    pub sample_radius_px: f32,
    pub darkness: f32,
    pub saturation: f32,
}

impl Default for ColorTraceParams {
    fn default() -> Self {
        Self {
            strength: 0.0,
            line_threshold: 0.34,
            softness: 0.14,
            sample_radius_px: 6.0,
            darkness: 0.55,
            saturation: 0.12,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorOverlayShape {
    Unselected,
    Solid,
    #[default]
    Linear,
    Radial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorOverlayBlendMode {
    Normal,
    Multiply,
    Screen,
    Overlay,
    #[default]
    SoftLight,
    Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorOverlayParams {
    #[serde(default)]
    pub shape: ColorOverlayShape,
    #[serde(default)]
    pub blend_mode: ColorOverlayBlendMode,
    #[serde(default = "default_color_overlay_start_rgb")]
    pub start_rgb: [u8; 3],
    #[serde(default = "default_color_overlay_end_rgb")]
    pub end_rgb: [u8; 3],
    #[serde(default = "default_color_overlay_angle_degrees")]
    pub angle_degrees: f32,
    #[serde(default)]
    pub linear_points_enabled: bool,
    #[serde(default = "default_color_overlay_linear_start")]
    pub linear_start: [f32; 2],
    #[serde(default = "default_color_overlay_linear_end")]
    pub linear_end: [f32; 2],
    #[serde(default = "default_color_overlay_center")]
    pub center: [f32; 2],
    #[serde(default = "default_color_overlay_radius")]
    pub radius: f32,
    #[serde(default = "default_color_overlay_softness")]
    pub softness: f32,
    #[serde(default)]
    pub opacity: f32,
}

impl Default for ColorOverlayParams {
    fn default() -> Self {
        Self {
            shape: ColorOverlayShape::Linear,
            blend_mode: ColorOverlayBlendMode::SoftLight,
            start_rgb: default_color_overlay_start_rgb(),
            end_rgb: default_color_overlay_end_rgb(),
            angle_degrees: default_color_overlay_angle_degrees(),
            linear_points_enabled: false,
            linear_start: default_color_overlay_linear_start(),
            linear_end: default_color_overlay_linear_end(),
            center: default_color_overlay_center(),
            radius: default_color_overlay_radius(),
            softness: default_color_overlay_softness(),
            opacity: 0.0,
        }
    }
}

fn default_color_overlay_start_rgb() -> [u8; 3] {
    [255, 150, 64]
}

fn default_color_overlay_end_rgb() -> [u8; 3] {
    [80, 135, 255]
}

fn default_color_overlay_angle_degrees() -> f32 {
    -25.0
}

fn default_color_overlay_linear_start() -> [f32; 2] {
    [0.0, 0.5]
}

fn default_color_overlay_linear_end() -> [f32; 2] {
    [1.0, 0.5]
}

fn default_color_overlay_center() -> [f32; 2] {
    [0.5, 0.5]
}

fn default_color_overlay_radius() -> f32 {
    0.85
}

fn default_color_overlay_softness() -> f32 {
    0.55
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NeonGlowParams {
    #[serde(default = "default_neon_glow_threshold")]
    pub threshold: f32,
    #[serde(default = "default_true")]
    pub by_saturation: bool,
    #[serde(default = "default_neon_glow_inner_radius")]
    pub inner_radius_px: f32,
    #[serde(default = "default_neon_glow_outer_radius")]
    pub outer_radius_px: f32,
    #[serde(default)]
    pub strength: f32,
    #[serde(default = "default_neon_glow_inner_amount")]
    pub inner_amount: f32,
    #[serde(default = "default_neon_glow_outer_amount")]
    pub outer_amount: f32,
    #[serde(default = "default_neon_glow_saturation")]
    pub glow_saturation: f32,
    #[serde(default = "default_neon_glow_tint_rgb")]
    pub tint_rgb: [u8; 3],
    #[serde(default)]
    pub tint_strength: f32,
    #[serde(default = "default_true")]
    pub screen_blend: bool,
    #[serde(default)]
    pub source_color_enabled: bool,
    #[serde(default = "default_neon_glow_source_rgb")]
    pub source_rgb: [u8; 3],
    #[serde(default = "default_neon_glow_source_tolerance")]
    pub source_tolerance: f32,
    #[serde(default = "default_neon_glow_source_feather")]
    pub source_feather: f32,
}

impl Default for NeonGlowParams {
    fn default() -> Self {
        Self {
            threshold: default_neon_glow_threshold(),
            by_saturation: true,
            inner_radius_px: default_neon_glow_inner_radius(),
            outer_radius_px: default_neon_glow_outer_radius(),
            strength: 0.0,
            inner_amount: default_neon_glow_inner_amount(),
            outer_amount: default_neon_glow_outer_amount(),
            glow_saturation: default_neon_glow_saturation(),
            tint_rgb: default_neon_glow_tint_rgb(),
            tint_strength: 0.0,
            screen_blend: true,
            source_color_enabled: false,
            source_rgb: default_neon_glow_source_rgb(),
            source_tolerance: default_neon_glow_source_tolerance(),
            source_feather: default_neon_glow_source_feather(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_neon_glow_threshold() -> f32 {
    0.55
}

fn default_neon_glow_inner_radius() -> f32 {
    5.0
}

fn default_neon_glow_outer_radius() -> f32 {
    28.0
}

fn default_neon_glow_inner_amount() -> f32 {
    0.85
}

fn default_neon_glow_outer_amount() -> f32 {
    0.75
}

fn default_neon_glow_saturation() -> f32 {
    0.55
}

fn default_neon_glow_tint_rgb() -> [u8; 3] {
    [0, 220, 255]
}

fn default_neon_glow_source_rgb() -> [u8; 3] {
    [0, 220, 255]
}

fn default_neon_glow_source_tolerance() -> f32 {
    0.28
}

fn default_neon_glow_source_feather() -> f32 {
    0.12
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorDodgeGlowParams {
    pub threshold: f32,
    pub radius_px: f32,
    pub strength: f32,
    pub dodge_amount: f32,
    pub color_rgb: [u8; 3],
    pub color_strength: f32,
}

impl Default for ColorDodgeGlowParams {
    fn default() -> Self {
        Self {
            threshold: 0.55,
            radius_px: 0.0,
            strength: 0.0,
            dodge_amount: 0.65,
            color_rgb: [255, 220, 128],
            color_strength: 0.35,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DiffuseGlowParams {
    pub threshold: f32,
    pub radius_px: f32,
    pub strength: f32,
    pub white_mix: f32,
    pub grain: f32,
    pub seed: u32,
}

impl Default for DiffuseGlowParams {
    fn default() -> Self {
        Self {
            threshold: 0.55,
            radius_px: 24.0,
            strength: 0.0,
            white_mix: 0.4,
            grain: 0.25,
            seed: 1,
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
pub struct HalationParams {
    pub threshold: f32,
    pub radius_px: f32,
    pub strength: f32,
    pub warmth: f32,
    pub tint_rgb: [u8; 3],
    pub edge_bias: f32,
    pub screen_blend: bool,
}

impl Default for HalationParams {
    fn default() -> Self {
        Self {
            threshold: 0.62,
            radius_px: 28.0,
            strength: 0.0,
            warmth: 0.55,
            tint_rgb: [255, 232, 196],
            edge_bias: 0.35,
            screen_blend: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GodRaysParams {
    pub center: [f32; 2],
    pub threshold: f32,
    pub length_px: f32,
    pub decay: f32,
    pub strength: f32,
    pub warm_tint: f32,
}

impl Default for GodRaysParams {
    fn default() -> Self {
        Self {
            center: [0.50, 0.18],
            threshold: 0.82,
            length_px: 120.0,
            decay: 0.86,
            strength: 0.0,
            warm_tint: 0.18,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LensFlareParams {
    pub center: [f32; 2],
    pub radius_px: f32,
    pub strength: f32,
    pub core_strength: f32,
    pub halo_strength: f32,
    pub ghost_strength: f32,
    pub streak_strength: f32,
    pub warm_tint: f32,
}

impl Default for LensFlareParams {
    fn default() -> Self {
        Self {
            center: [0.72, 0.26],
            radius_px: 96.0,
            strength: 0.0,
            core_strength: 1.0,
            halo_strength: 0.7,
            ghost_strength: 0.65,
            streak_strength: 0.45,
            warm_tint: 0.2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AnamorphicFlareParams {
    pub threshold: f32,
    pub length_px: f32,
    pub thickness_px: f32,
    pub strength: f32,
    pub color_rgb: [u8; 3],
    pub color_strength: f32,
}

impl Default for AnamorphicFlareParams {
    fn default() -> Self {
        Self {
            threshold: 0.82,
            length_px: 180.0,
            thickness_px: 3.0,
            strength: 0.0,
            color_rgb: [80, 150, 255],
            color_strength: 0.85,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LightLeakParams {
    pub center: [f32; 2],
    pub color_rgb: [u8; 3],
    pub radius: f32,
    pub intensity: f32,
    pub falloff: f32,
    pub haze: f32,
    pub streak_strength: f32,
    pub streak_angle_degrees: f32,
    pub strength: f32,
    pub seed: u32,
}

impl Default for LightLeakParams {
    fn default() -> Self {
        Self {
            center: [0.08, 0.10],
            color_rgb: [255, 146, 72],
            radius: 0.70,
            intensity: 0.85,
            falloff: 2.6,
            haze: 0.28,
            streak_strength: 0.30,
            streak_angle_degrees: -28.0,
            strength: 0.0,
            seed: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BacklightHazeParams {
    pub center: [f32; 2],
    pub color_rgb: [u8; 3],
    pub radius: f32,
    pub falloff: f32,
    pub haze: f32,
    pub glow: f32,
    pub shadow_lift: f32,
    pub contrast_fade: f32,
    pub saturation_fade: f32,
    pub strength: f32,
}

impl Default for BacklightHazeParams {
    fn default() -> Self {
        Self {
            center: [0.50, 0.12],
            color_rgb: [255, 224, 174],
            radius: 0.90,
            falloff: 1.65,
            haze: 0.35,
            glow: 0.28,
            shadow_lift: 0.24,
            contrast_fade: 0.18,
            saturation_fade: 0.10,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SpeedLinesMode {
    #[default]
    Radial,
    Parallel,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpeedLinesParams {
    pub mode: SpeedLinesMode,
    pub center: [f32; 2],
    pub angle_degrees: f32,
    pub line_count: u32,
    pub line_width_px: f32,
    pub length: f32,
    pub inner_radius: f32,
    pub outer_radius: f32,
    pub softness: f32,
    pub strength: f32,
    pub color_rgb: [u8; 3],
    pub seed: u32,
}

impl Default for SpeedLinesParams {
    fn default() -> Self {
        Self {
            mode: SpeedLinesMode::Radial,
            center: [0.5, 0.5],
            angle_degrees: 0.0,
            line_count: 72,
            line_width_px: 2.0,
            length: 0.82,
            inner_radius: 0.16,
            outer_radius: 1.0,
            softness: 0.25,
            strength: 0.0,
            color_rgb: [255, 255, 255],
            seed: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RadialFlashParams {
    pub center: [f32; 2],
    pub ray_count: u32,
    pub rotation_degrees: f32,
    pub inner_radius: f32,
    pub outer_radius: f32,
    pub softness: f32,
    pub white_amount: f32,
    pub black_amount: f32,
    pub invert: bool,
    pub strength: f32,
}

impl Default for RadialFlashParams {
    fn default() -> Self {
        Self {
            center: [0.5, 0.5],
            ray_count: 36,
            rotation_degrees: 0.0,
            inner_radius: 0.05,
            outer_radius: 1.0,
            softness: 0.18,
            white_amount: 0.85,
            black_amount: 0.65,
            invert: false,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CloudFogMode {
    #[default]
    Fog,
    Clouds,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CloudFogParams {
    pub mode: CloudFogMode,
    pub scale_px: f32,
    pub detail: f32,
    pub density: f32,
    pub contrast: f32,
    pub height_fade: f32,
    pub opacity: f32,
    pub color_rgb: [u8; 3],
    pub seed: u32,
}

impl Default for CloudFogParams {
    fn default() -> Self {
        Self {
            mode: CloudFogMode::Fog,
            scale_px: 180.0,
            detail: 0.45,
            density: 0.45,
            contrast: 0.25,
            height_fade: 0.0,
            opacity: 0.0,
            color_rgb: [235, 242, 255],
            seed: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpotlightParams {
    pub center: [f32; 2],
    pub radius: f32,
    pub feather: f32,
    pub light_strength: f32,
    pub shadow_strength: f32,
    pub tint_rgb: [u8; 3],
    pub tint_strength: f32,
}

impl Default for SpotlightParams {
    fn default() -> Self {
        Self {
            center: [0.5, 0.45],
            radius: 0.34,
            feather: 0.36,
            light_strength: 0.0,
            shadow_strength: 0.0,
            tint_rgb: [255, 236, 190],
            tint_strength: 0.0,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseDistribution {
    Uniform,
    Gaussian,
}

impl Default for NoiseDistribution {
    fn default() -> Self {
        Self::Uniform
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NoiseParams {
    pub amount: f32,
    pub distribution: NoiseDistribution,
    pub monochrome: bool,
    pub seed: u32,
}

impl Default for NoiseParams {
    fn default() -> Self {
        Self {
            amount: 0.0,
            distribution: NoiseDistribution::Uniform,
            monochrome: true,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnaglyphMode {
    RedCyan,
    GreenMagenta,
    AmberBlue,
    RgbSplit,
}

impl Default for AnaglyphMode {
    fn default() -> Self {
        Self::RedCyan
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AnaglyphParams {
    pub mode: AnaglyphMode,
    pub disparity_px: f32,
    pub angle_degrees: f32,
    pub luma_mix: f32,
    pub strength: f32,
}

impl Default for AnaglyphParams {
    fn default() -> Self {
        Self {
            mode: AnaglyphMode::RedCyan,
            disparity_px: 6.0,
            angle_degrees: 0.0,
            luma_mix: 0.45,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DefringeParams {
    pub radius_px: f32,
    pub edge_threshold: f32,
    pub color_threshold: f32,
    pub neutralize: f32,
    pub strength: f32,
}

impl Default for DefringeParams {
    fn default() -> Self {
        Self {
            radius_px: 1.0,
            edge_threshold: 0.08,
            color_threshold: 0.18,
            neutralize: 0.75,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScanlineGlitchParams {
    pub line_spacing_px: f32,
    pub line_strength: f32,
    pub jitter_px: f32,
    pub rgb_shift_px: f32,
    pub block_strength: f32,
    pub noise: f32,
    pub seed: u32,
    pub strength: f32,
}

impl Default for ScanlineGlitchParams {
    fn default() -> Self {
        Self {
            line_spacing_px: 4.0,
            line_strength: 0.35,
            jitter_px: 0.0,
            rgb_shift_px: 0.0,
            block_strength: 0.0,
            noise: 0.0,
            seed: 1,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VhsParams {
    pub chroma_bleed_px: f32,
    pub chroma_shift_px: f32,
    pub ghost_offset_px: f32,
    pub ghost_strength: f32,
    pub tracking_strength: f32,
    pub scanline_strength: f32,
    pub noise: f32,
    pub desaturation: f32,
    pub seed: u32,
    pub strength: f32,
}

impl Default for VhsParams {
    fn default() -> Self {
        Self {
            chroma_bleed_px: 4.0,
            chroma_shift_px: 0.0,
            ghost_offset_px: 0.0,
            ghost_strength: 0.0,
            tracking_strength: 0.0,
            scanline_strength: 0.0,
            noise: 0.0,
            desaturation: 0.0,
            seed: 1,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DataMoshParams {
    pub block_size_px: f32,
    pub displacement_px: f32,
    pub direction_degrees: f32,
    pub low_threshold: f32,
    pub high_threshold: f32,
    pub freeze: f32,
    pub smear: f32,
    pub rgb_shift_px: f32,
    pub noise: f32,
    pub seed: u32,
    pub strength: f32,
}

impl Default for DataMoshParams {
    fn default() -> Self {
        Self {
            block_size_px: 16.0,
            displacement_px: 10.0,
            direction_degrees: 0.0,
            low_threshold: 0.08,
            high_threshold: 0.96,
            freeze: 0.35,
            smear: 0.25,
            rgb_shift_px: 2.0,
            noise: 0.10,
            seed: 1,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelSortDirection {
    Horizontal,
    Vertical,
}

impl Default for PixelSortDirection {
    fn default() -> Self {
        Self::Horizontal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelSortOrder {
    DarkToLight,
    LightToDark,
}

impl Default for PixelSortOrder {
    fn default() -> Self {
        Self::LightToDark
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PixelSortParams {
    pub direction: PixelSortDirection,
    pub order: PixelSortOrder,
    pub low_threshold: f32,
    pub high_threshold: f32,
    pub max_segment_px: u32,
    pub strength: f32,
}

impl Default for PixelSortParams {
    fn default() -> Self {
        Self {
            direction: PixelSortDirection::Horizontal,
            order: PixelSortOrder::LightToDark,
            low_threshold: 0.35,
            high_threshold: 0.95,
            max_segment_px: 160,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OldFilmParams {
    pub sepia: f32,
    pub fade: f32,
    pub vignette: f32,
    pub grain: f32,
    pub dust: f32,
    pub scratches: f32,
    pub seed: u32,
    pub strength: f32,
}

impl Default for OldFilmParams {
    fn default() -> Self {
        Self {
            sepia: 0.35,
            fade: 0.35,
            vignette: 0.25,
            grain: 0.0,
            dust: 0.0,
            scratches: 0.0,
            seed: 1,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WaterCausticsParams {
    pub scale_px: f32,
    pub intensity: f32,
    pub contrast: f32,
    pub tint: f32,
    pub depth: f32,
    pub phase: f32,
    pub seed: u32,
    pub strength: f32,
}

impl Default for WaterCausticsParams {
    fn default() -> Self {
        Self {
            scale_px: 52.0,
            intensity: 0.55,
            contrast: 0.65,
            tint: 0.35,
            depth: 0.18,
            phase: 0.0,
            seed: 1,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticleOverlayMode {
    Rain,
    Snow,
    Petals,
}

impl Default for ParticleOverlayMode {
    fn default() -> Self {
        Self::Rain
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ParticleOverlayParams {
    pub mode: ParticleOverlayMode,
    pub density: f32,
    pub size_px: f32,
    pub length_px: f32,
    pub angle_degrees: f32,
    pub opacity: f32,
    pub color_rgb: [u8; 3],
    pub seed: u32,
    pub strength: f32,
}

impl Default for ParticleOverlayParams {
    fn default() -> Self {
        Self {
            mode: ParticleOverlayMode::Rain,
            density: 0.45,
            size_px: 1.4,
            length_px: 34.0,
            angle_degrees: 105.0,
            opacity: 0.45,
            color_rgb: [210, 230, 255],
            seed: 1,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AuroraParams {
    pub band_count: f32,
    pub scale_px: f32,
    pub height: f32,
    pub waviness: f32,
    pub softness: f32,
    pub brightness: f32,
    pub color_rgb: [u8; 3],
    pub secondary_rgb: [u8; 3],
    pub phase: f32,
    pub seed: u32,
    pub strength: f32,
}

impl Default for AuroraParams {
    fn default() -> Self {
        Self {
            band_count: 5.0,
            scale_px: 120.0,
            height: 0.68,
            waviness: 0.55,
            softness: 0.45,
            brightness: 0.85,
            color_rgb: [80, 255, 170],
            secondary_rgb: [150, 105, 255],
            phase: 0.0,
            seed: 1,
            strength: 0.0,
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenToneMode {
    Dots,
    Lines,
    CrossHatch,
}

impl Default for ScreenToneMode {
    fn default() -> Self {
        Self::Dots
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScreenToneParams {
    pub mode: ScreenToneMode,
    pub cell_px: f32,
    pub angle_degrees: f32,
    pub density: f32,
    pub gradation: f32,
    pub softness: f32,
    pub strength: f32,
}

impl Default for ScreenToneParams {
    fn default() -> Self {
        Self {
            mode: ScreenToneMode::Dots,
            cell_px: 8.0,
            angle_degrees: 45.0,
            density: 0.45,
            gradation: 0.65,
            softness: 0.08,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorHalftoneParams {
    pub cell_px: f32,
    pub angle_offset_degrees: f32,
    pub dot_gain: f32,
    pub black_generation: f32,
    pub softness: f32,
    pub strength: f32,
}

impl Default for ColorHalftoneParams {
    fn default() -> Self {
        Self {
            cell_px: 10.0,
            angle_offset_degrees: 0.0,
            dot_gain: 0.0,
            black_generation: 0.70,
            softness: 0.04,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CmykPlateShiftParams {
    pub offset_px: f32,
    pub angle_degrees: f32,
    pub black_offset_px: f32,
    pub black_generation: f32,
    pub ink_gain: f32,
    pub strength: f32,
}

impl Default for CmykPlateShiftParams {
    fn default() -> Self {
        Self {
            offset_px: 0.0,
            angle_degrees: 0.0,
            black_offset_px: 0.0,
            black_generation: 0.70,
            ink_gain: 0.0,
            strength: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LithographParams {
    pub ink_a_rgb: [u8; 3],
    pub ink_b_rgb: [u8; 3],
    pub paper_rgb: [u8; 3],
    pub ink_density: f32,
    pub posterization: f32,
    pub grain: f32,
    pub misregistration_px: f32,
    pub angle_degrees: f32,
    pub paper_texture: f32,
    pub strength: f32,
    pub seed: u32,
}

impl Default for LithographParams {
    fn default() -> Self {
        Self {
            ink_a_rgb: [238, 64, 95],
            ink_b_rgb: [32, 163, 197],
            paper_rgb: [248, 238, 210],
            ink_density: 0.88,
            posterization: 0.45,
            grain: 0.35,
            misregistration_px: 2.0,
            angle_degrees: 0.0,
            paper_texture: 0.25,
            strength: 0.0,
            seed: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EngravingParams {
    pub ink_rgb: [u8; 3],
    pub paper_rgb: [u8; 3],
    pub line_spacing_px: f32,
    pub line_width: f32,
    pub angle_degrees: f32,
    pub crosshatch: f32,
    pub contour_strength: f32,
    pub tone_levels: f32,
    pub ink_density: f32,
    pub paper_texture: f32,
    pub strength: f32,
    pub seed: u32,
}

impl Default for EngravingParams {
    fn default() -> Self {
        Self {
            ink_rgb: [42, 35, 28],
            paper_rgb: [247, 238, 216],
            line_spacing_px: 7.0,
            line_width: 0.60,
            angle_degrees: -18.0,
            crosshatch: 0.35,
            contour_strength: 0.30,
            tone_levels: 7.0,
            ink_density: 0.90,
            paper_texture: 0.28,
            strength: 0.0,
            seed: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NewspaperPrintParams {
    pub cell_px: f32,
    pub dot_gain: f32,
    pub ink_bleed: f32,
    pub paper_age: f32,
    pub paper_texture: f32,
    pub contrast: f32,
    pub fade: f32,
    pub strength: f32,
    pub seed: u32,
}

impl Default for NewspaperPrintParams {
    fn default() -> Self {
        Self {
            cell_px: 9.0,
            dot_gain: 0.05,
            ink_bleed: 0.20,
            paper_age: 0.45,
            paper_texture: 0.35,
            contrast: 0.20,
            fade: 0.18,
            strength: 0.0,
            seed: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextureizerMode {
    Paper,
    Canvas,
    Linen,
}

impl Default for TextureizerMode {
    fn default() -> Self {
        Self::Paper
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TextureizerParams {
    pub mode: TextureizerMode,
    pub scale_px: f32,
    pub depth: f32,
    pub contrast: f32,
    pub warmth: f32,
    pub strength: f32,
    pub seed: u32,
}

impl Default for TextureizerParams {
    fn default() -> Self {
        Self {
            mode: TextureizerMode::Paper,
            scale_px: 10.0,
            depth: 0.45,
            contrast: 1.0,
            warmth: 0.15,
            strength: 0.0,
            seed: 1,
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
pub struct DiffractionStarburstParams {
    pub blade_count: u32,
    pub rotation_degrees: f32,
    pub threshold: f32,
    pub length_px: f32,
    pub width_px: f32,
    pub halo_radius_px: f32,
    pub chromatic_shift: f32,
    pub strength: f32,
}

impl Default for DiffractionStarburstParams {
    fn default() -> Self {
        Self {
            blade_count: 6,
            rotation_degrees: 0.0,
            threshold: 0.995,
            length_px: 96.0,
            width_px: 1.6,
            halo_radius_px: 14.0,
            chromatic_shift: 0.25,
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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DespeckleParams {
    pub radius_px: f32,
    pub threshold: f32,
    pub strength: f32,
}

impl Default for DespeckleParams {
    fn default() -> Self {
        Self {
            radius_px: 1.0,
            threshold: 48.0,
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
    let mut alpha = evaluate_layer_mask_without_opacity(image, layer)?;
    apply_mask_opacity(&mut alpha, layer.opacity);
    Ok(alpha)
}

fn evaluate_layer_mask_without_opacity(
    image: RgbaImageRef<'_>,
    layer: &LocalAdjustmentLayer,
) -> Result<Vec<f32>> {
    let image = image.validate()?;
    let alpha = evaluate_layer_mask_before_feather(image, layer)?;
    Ok(apply_layer_mask_feather(
        alpha,
        image.width,
        image.height,
        layer.mask_feather_px,
    ))
}

fn evaluate_layer_mask_before_feather(
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
        alpha.par_iter_mut().for_each(|a| {
            *a = 1.0 - *a;
        });
    }
    if layer.mask_expand_px.abs() >= 0.5 {
        alpha = morph_alpha(
            &alpha,
            image.width,
            image.height,
            layer.mask_expand_px.round() as i32,
        );
    }
    Ok(alpha)
}

fn apply_layer_mask_feather(
    alpha: Vec<f32>,
    width: usize,
    height: usize,
    feather_px: f32,
) -> Vec<f32> {
    if feather_px >= 0.5 {
        box_blur_alpha(&alpha, width, height, feather_px.round() as usize)
    } else {
        alpha
    }
}

fn apply_mask_opacity(alpha: &mut [f32], opacity: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    alpha.par_iter_mut().for_each(|a| {
        *a = (*a * opacity).clamp(0.0, 1.0);
    });
}

fn apply_manual_override(
    alpha: &mut [f32],
    width: usize,
    height: usize,
    manual_override: &ManualMaskOverride,
) -> Result<()> {
    if let Some(add) = &manual_override.add {
        apply_raster_vector_override(alpha, width, height, add, 1.0)?;
    }
    if let Some(subtract) = &manual_override.subtract {
        apply_raster_vector_override(alpha, width, height, subtract, 0.0)?;
    }
    Ok(())
}

fn apply_raster_vector_override(
    alpha: &mut [f32],
    width: usize,
    height: usize,
    mask: &RasterVectorMask,
    value: f32,
) -> Result<()> {
    mask.validate(width, height)?;
    if mask.shapes.is_empty() {
        alpha
            .par_iter_mut()
            .zip(mask.alpha.par_iter())
            .for_each(|(a, &mask_alpha)| {
                if mask_alpha.clamp(0.0, 1.0) >= 0.5 {
                    *a = value;
                }
            });
        return Ok(());
    }

    let mask_alpha = eval_raster_vector_mask(mask, width, height)?;
    alpha
        .par_iter_mut()
        .zip(mask_alpha.into_par_iter())
        .for_each(|(a, mask_alpha)| {
            if mask_alpha >= 0.5 {
                *a = value;
            }
        });
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
    let mask_before_feather = evaluate_layer_mask_before_feather(image.as_ref(), layer)?;
    // Repair の生成範囲 / 周囲パッチ探索には hard mask を使う。汎用の feathered mask を
    // 渡すと、わずかな alpha の halo まで欠損扱いになり、参照元とタイル配置が変わって
    // テクスチャそのものがぼけたり粗くなったりする。feather は最後の合成だけに使う。
    let repair_mask =
        matches!(&layer.effect, LocalEffect::Repair(_)).then(|| mask_before_feather.clone());
    let base_mask = apply_layer_mask_feather(
        mask_before_feather,
        image.width,
        image.height,
        layer.mask_feather_px,
    );
    let opacity = layer.opacity.clamp(0.0, 1.0);
    let output_mask = if layer.mask_after_effect {
        let mut output_mask = base_mask.clone();
        apply_mask_opacity(&mut output_mask, opacity);
        output_mask
    } else {
        vec![opacity; base_mask.len()]
    };
    if let LocalEffect::Mosaic(params) = &layer.effect
        && !layer.mask_before_effect
        && layer.mask_after_effect
    {
        image.pixels = apply_mosaic_with_mask(
            &image.pixels,
            image.width,
            image.height,
            &output_mask,
            *params,
        );
        return Ok(());
    }
    if let LocalEffect::Repair(params) = &layer.effect {
        let repair_mask = repair_mask
            .as_deref()
            .expect("Repair layers always retain their pre-feather mask");
        let effected = apply_repair(
            &image.pixels,
            image.width,
            image.height,
            repair_mask,
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
        )?;
        let mut repair_mask = base_mask;
        apply_mask_opacity(&mut repair_mask, opacity);
        blend_rgb_with_mask(&mut image.pixels, &effected, &repair_mask);
        return Ok(());
    }

    let masked_input = if layer.mask_before_effect {
        Some(mask_rgba_input(&image.pixels, &base_mask))
    } else {
        None
    };
    let effect_image = RgbaImageRef {
        width: image.width,
        height: image.height,
        pixels: masked_input.as_deref().unwrap_or(&image.pixels),
    };
    let original_pixels = image.pixels.as_slice();

    let effected = {
        let image = effect_image;
        match &layer.effect {
            LocalEffect::None => unreachable!("None is handled before mask evaluation"),
            LocalEffect::Tone(params) => apply_tone_image(&image.pixels, *params),
            LocalEffect::ToneCurve(params) => apply_tone_curve(&image.pixels, *params),
            LocalEffect::RgbToneCurve(params) => apply_rgb_tone_curve(&image.pixels, *params),
            LocalEffect::ColorBalance(params) => apply_color_balance(&image.pixels, *params),
            LocalEffect::PhotoFilter(params) => apply_photo_filter(&image.pixels, *params),
            LocalEffect::ThreeWayColorGrading(params) => {
                apply_three_way_color_grading(&image.pixels, *params)
            }
            LocalEffect::SelectiveColor(params) => apply_selective_color(&image.pixels, *params),
            LocalEffect::PartColor(params) => apply_part_color(&image.pixels, *params),
            LocalEffect::ChannelMixer(params) => apply_channel_mixer(&image.pixels, *params),
            LocalEffect::MonochromeMixer(params) => apply_monochrome_mixer(&image.pixels, *params),
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
            LocalEffect::FrequencySeparation(params) => {
                apply_frequency_separation(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::HighlightsShadows(params) => {
                apply_highlights_shadows(&image.pixels, *params)
            }
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
            LocalEffect::Wind(params) => {
                apply_wind(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::TiltShift(params) => {
                apply_tilt_shift(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::LensBlur(params) => {
                apply_lens_blur(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::BokehSprite(params) => {
                apply_bokeh_sprite(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::LensDirt(params) => {
                apply_lens_dirt(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::RadialBlur(params) => {
                apply_radial_blur(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::WaveDistortion(params) => {
                apply_wave_distortion(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::HeatHaze(params) => {
                apply_heat_haze(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::PinchSpherize(params) => {
                apply_pinch_spherize(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Twirl(params) => {
                apply_twirl(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::PolarCoordinates(params) => {
                apply_polar_coordinates(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::GlassDisplacement(params) => {
                apply_glass_displacement(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::LensCorrection(params) => {
                apply_lens_correction(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::LineExtract(params) => {
                apply_line_extract(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::ArtisticMedia(params) => {
                apply_artistic_media(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::BrushStroke(params) => {
                apply_brush_stroke(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Cutout(params) => {
                apply_cutout(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::ToonShade(params) => {
                apply_toon_shade(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Emboss(params) => {
                apply_emboss(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::PixelStylize(params) => {
                apply_pixel_stylize(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Solarize(params) => apply_solarize(&image.pixels, *params),
            LocalEffect::GlowingEdges(params) => {
                apply_glowing_edges(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::OilPaint(params) => {
                apply_oil_paint(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::SoftFocus(params) => apply_soft_focus(
                &image.pixels,
                image.width,
                image.height,
                params.radius_px.round().max(0.0) as usize,
                params.strength.clamp(0.0, 1.0),
            ),
            LocalEffect::Orton(params) => {
                apply_orton(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Mosaic(params) => {
                let full_mask = vec![1.0; image.width.saturating_mul(image.height)];
                apply_mosaic_with_mask(
                    &image.pixels,
                    image.width,
                    image.height,
                    &full_mask,
                    *params,
                )
            }
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
            LocalEffect::RetroPalette(params) => {
                apply_retro_palette(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::CrtDisplay(params) => {
                apply_crt_display(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Threshold(params) => apply_threshold(&image.pixels, *params),
            LocalEffect::Invert(params) => apply_invert(&image.pixels, *params),
            LocalEffect::Duotone(params) => apply_duotone(&image.pixels, *params),
            LocalEffect::Equalize(params) => apply_equalize(&image.pixels, *params),
            LocalEffect::GradientMap(params) => apply_gradient_map(&image.pixels, *params),
            LocalEffect::Repair(_) => unreachable!("Repair is handled with its resolved mask"),
            LocalEffect::ColorFill(params) => {
                apply_color_fill(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Frame(params) => {
                apply_frame(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::OutlineStroke(params) => apply_outline_stroke(
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
            LocalEffect::RimLight(params) => apply_rim_light(
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
            LocalEffect::ContactShadow(params) => apply_contact_shadow(
                original_pixels,
                image.width,
                image.height,
                &base_mask,
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
            LocalEffect::ColorTrace(params) => {
                apply_color_trace(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::ColorOverlay(params) => {
                apply_color_overlay(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::NeonGlow(params) => {
                apply_neon_glow(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::DiffuseGlow(params) => {
                apply_diffuse_glow(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Bloom(params) => {
                apply_bloom(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Halation(params) => {
                apply_halation(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::ColorDodgeGlow(params) => {
                apply_color_dodge_glow(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::GodRays(params) => {
                apply_god_rays(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::LensFlare(params) => {
                apply_lens_flare(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::AnamorphicFlare(params) => {
                apply_anamorphic_flare(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::LightLeak(params) => {
                apply_light_leak(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::BacklightHaze(params) => {
                apply_backlight_haze(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::SpeedLines(params) => {
                apply_speed_lines(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::RadialFlash(params) => {
                apply_radial_flash(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::CloudFog(params) => {
                apply_cloud_fog(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Spotlight(params) => {
                apply_spotlight(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Vignette(params) => {
                apply_vignette(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::FilmGrain(params) => {
                apply_film_grain(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Noise(params) => {
                apply_noise(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::ChromaticAberration(params) => apply_chromatic_aberration(
                &image.pixels,
                image.width,
                image.height,
                params.offset_px.clamp(0.0, 24.0),
            ),
            LocalEffect::Anaglyph3d(params) => {
                apply_anaglyph_3d(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Defringe(params) => {
                apply_defringe(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::ScanlineGlitch(params) => {
                apply_scanline_glitch(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Vhs(params) => {
                apply_vhs(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::DataMosh(params) => {
                apply_data_mosh(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::PixelSort(params) => {
                apply_pixel_sort(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::OldFilm(params) => {
                apply_old_film(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::WaterCaustics(params) => {
                apply_water_caustics(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::ParticleOverlay(params) => {
                apply_particle_overlay(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Aurora(params) => {
                apply_aurora(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Halftone(params) => {
                apply_halftone(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::ScreenTone(params) => {
                apply_screen_tone(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::ColorHalftone(params) => {
                apply_color_halftone(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::CmykPlateShift(params) => {
                apply_cmyk_plate_shift(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Lithograph(params) => {
                apply_lithograph(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Engraving(params) => {
                apply_engraving(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::NewspaperPrint(params) => {
                apply_newspaper_print(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Textureizer(params) => {
                apply_textureizer(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::StarGlow(params) => {
                apply_star_glow(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::DiffractionStarburst(params) => {
                apply_diffraction_starburst(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::EdgeSmooth(params) => {
                apply_edge_smooth(&image.pixels, image.width, image.height, *params)
            }
            LocalEffect::Despeckle(params) => {
                apply_despeckle(&image.pixels, image.width, image.height, *params)
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
        }
    };
    check_cancel(cancel)?;
    if matches!(
        &layer.effect,
        LocalEffect::OutlineStroke(_) | LocalEffect::RimLight(_)
    ) {
        blend_rgb_with_effect_alpha_mask(&mut image.pixels, &effected, &output_mask);
    } else if matches!(&layer.effect, LocalEffect::ContactShadow(_)) {
        blend_rgb_with_mask(&mut image.pixels, &effected, &output_mask);
    } else if layer.mask_before_effect && !layer.mask_after_effect {
        let input = masked_input
            .as_deref()
            .expect("masked input is present when mask_before_effect is true");
        add_rgb_effect_delta(&mut image.pixels, &effected, input, opacity);
    } else {
        blend_rgb_with_mask(&mut image.pixels, &effected, &output_mask);
    }
    Ok(())
}

fn mask_rgba_input(src: &[u8], mask: &[f32]) -> Vec<u8> {
    let mut out = src.to_vec();
    out.par_chunks_exact_mut(4)
        .zip(mask.par_iter())
        .for_each(|(px, amount)| {
            let amount = amount.clamp(0.0, 1.0);
            for c in 0..4 {
                px[c] = (px[c] as f32 * amount).round().clamp(0.0, 255.0) as u8;
            }
        });
    out
}

fn add_rgb_effect_delta(base: &mut [u8], effected: &[u8], input: &[u8], opacity: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    base.par_chunks_exact_mut(4)
        .zip(effected.par_chunks_exact(4))
        .zip(input.par_chunks_exact(4))
        .for_each(|((base, effected), input)| {
            for c in 0..3 {
                let delta = effected[c] as f32 - input[c] as f32;
                base[c] = (base[c] as f32 + delta * opacity).round().clamp(0.0, 255.0) as u8;
            }
            // Keep source alpha stable; alpha expansion is intentionally out of scope.
        });
}

fn evaluate_raw_mask(image: RgbaImageRef<'_>, mask: &LocalMask) -> Result<Vec<f32>> {
    let len = image.width * image.height;
    match mask {
        LocalMask::Full => Ok(vec![1.0; len]),
        LocalMask::Raster(mask) => {
            mask.validate(image.width, image.height)?;
            Ok(mask.alpha.par_iter().map(|v| v.clamp(0.0, 1.0)).collect())
        }
        LocalMask::Subject(mask) => {
            mask.validate(image.width, image.height)?;
            Ok(mask.alpha.par_iter().map(|v| v.clamp(0.0, 1.0)).collect())
        }
        LocalMask::Segmentation(mask) => {
            mask.validate(image.width, image.height)?;
            Ok(mask
                .labels
                .par_iter()
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
    out.par_iter_mut().enumerate().for_each(|(i, alpha)| {
        let x = i % width;
        let y = i / width;
        let nx = (x as f32 + 0.5) / wf;
        let ny = (y as f32 + 0.5) / hf;
        *alpha = (((nx - sx) * dx + (ny - sy) * dy) / denom).clamp(0.0, 1.0);
    });
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
    out.par_iter_mut().enumerate().for_each(|(i, alpha)| {
        let x = i % width;
        let y = i / width;
        let nx = (x as f32 + 0.5) / wf;
        let ny = (y as f32 + 0.5) / hf;
        let dx = nx - mask.center[0];
        let dy = ny - mask.center[1];
        let dist = (dx * dx + dy * dy).sqrt();
        if dist <= f32::EPSILON {
            *alpha = 1.0;
            return;
        }
        let ux = dx / dist;
        let uy = dy / dist;
        let inner = ellipse_radius_for_direction(inner_x, inner_y, ux, uy);
        let outer = ellipse_radius_for_direction(outer_x, outer_y, ux, uy).max(inner + 0.0001);
        *alpha = (1.0 - ((dist - inner) / (outer - inner))).clamp(0.0, 1.0);
    });
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
    out.par_iter_mut()
        .zip(image.pixels.par_chunks_exact(4))
        .for_each(|(alpha, px)| {
            let luma =
                (0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32) / 255.0;
            *alpha = range_alpha(luma, min, max, mask.feather);
        });
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
    out.par_iter_mut()
        .zip(image.pixels.par_chunks_exact(4))
        .for_each(|(alpha, px)| {
            let dr = px[0] as f32 / 255.0 - tr;
            let dg = px[1] as f32 / 255.0 - tg;
            let db = px[2] as f32 / 255.0 - tb;
            let dist = ((dr * dr + dg * dg + db * db) / 3.0).sqrt();
            *alpha = if dist <= tol {
                1.0
            } else {
                (1.0 - (dist - tol) / feather).clamp(0.0, 1.0)
            };
        });
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
    morph_alpha_disk(src, width, height, r, radius > 0, None, |_| {})
        .expect("morph_alpha without cancellation cannot fail")
}

fn morph_alpha_disk<F>(
    src: &[f32],
    width: usize,
    height: usize,
    radius: i32,
    dilate: bool,
    cancel: Option<&AtomicBool>,
    mut progress: F,
) -> Result<Vec<f32>>
where
    F: FnMut(f32),
{
    let r = radius.max(0);
    if r == 0 || width == 0 || height == 0 {
        progress(1.0);
        return Ok(src.to_vec());
    }

    let mut out: Vec<f32> = vec![if dilate { 0.0_f32 } else { 1.0_f32 }; src.len()];
    let mut row: Vec<f32> = vec![0.0_f32; width];
    let r2 = r * r;
    let dy_count = (r * 2 + 1).max(1) as f32;
    for (dy_index, dy) in (-r..=r).enumerate() {
        if dy_index % 4 == 0 {
            check_cancel(cancel)?;
            progress((dy_index as f32 / dy_count).clamp(0.0, 1.0));
        }
        let hx = ((r2 - dy * dy) as f32).sqrt().floor() as usize;
        let target_start = if dy < 0 { (-dy) as usize } else { 0 };
        let target_end = if dy > 0 {
            height.saturating_sub(dy as usize)
        } else {
            height
        };
        for y in target_start..target_end {
            let sy = (y as i32 + dy) as usize;
            let src_row = &src[sy * width..(sy + 1) * width];
            sliding_row_extreme(src_row, hx, dilate, &mut row);
            let out_row = &mut out[y * width..(y + 1) * width];
            if dilate {
                for (dst, sample) in out_row.iter_mut().zip(row.iter()) {
                    *dst = (*dst).max(*sample);
                }
            } else {
                for (dst, sample) in out_row.iter_mut().zip(row.iter()) {
                    *dst = (*dst).min(*sample);
                }
            }
        }
    }
    check_cancel(cancel)?;
    progress(1.0);
    Ok(out)
}

fn morph_alpha_disk_with_outside<F>(
    src: &[f32],
    width: usize,
    height: usize,
    radius: i32,
    dilate: bool,
    outside_value: f32,
    cancel: Option<&AtomicBool>,
    progress: F,
) -> Result<Vec<f32>>
where
    F: FnMut(f32),
{
    let r = radius.max(0) as usize;
    if r == 0 || width == 0 || height == 0 {
        return morph_alpha_disk(src, width, height, radius, dilate, cancel, progress);
    }

    let padded_width = width + r * 2;
    let padded_height = height + r * 2;
    let mut padded = vec![outside_value.clamp(0.0, 1.0); padded_width * padded_height];
    for y in 0..height {
        let src_start = y * width;
        let dst_start = (y + r) * padded_width + r;
        padded[dst_start..dst_start + width].copy_from_slice(&src[src_start..src_start + width]);
    }

    let padded_out = morph_alpha_disk(
        &padded,
        padded_width,
        padded_height,
        radius,
        dilate,
        cancel,
        progress,
    )?;
    let mut out = vec![0.0; src.len()];
    for y in 0..height {
        let src_start = (y + r) * padded_width + r;
        let dst_start = y * width;
        out[dst_start..dst_start + width]
            .copy_from_slice(&padded_out[src_start..src_start + width]);
    }
    Ok(out)
}

fn sliding_row_extreme(src: &[f32], radius: usize, dilate: bool, out: &mut [f32]) {
    debug_assert_eq!(src.len(), out.len());
    if src.is_empty() {
        return;
    }
    let mut deque: VecDeque<usize> = VecDeque::new();
    let mut right = 0usize;
    for x in 0..src.len() {
        let right_limit = (x + radius).min(src.len() - 1);
        while right <= right_limit {
            while let Some(&back) = deque.back() {
                let remove = if dilate {
                    src[back] <= src[right]
                } else {
                    src[back] >= src[right]
                };
                if !remove {
                    break;
                }
                deque.pop_back();
            }
            deque.push_back(right);
            right += 1;
        }
        let left_limit = x.saturating_sub(radius);
        while deque.front().is_some_and(|&front| front < left_limit) {
            deque.pop_front();
        }
        out[x] = src[*deque.front().expect("sliding window is never empty")];
    }
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

fn alpha_at_with_outside(
    field: &[f32],
    width: usize,
    height: usize,
    x: isize,
    y: isize,
    outside_value: f32,
) -> f32 {
    if x < 0 || y < 0 || x >= width as isize || y >= height as isize {
        outside_value.clamp(0.0, 1.0)
    } else {
        field[y as usize * width + x as usize]
    }
}

fn alpha_gradient_with_outside(
    field: &[f32],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    outside_value: f32,
) -> (f32, f32) {
    let x = x as isize;
    let y = y as isize;
    let gx = alpha_at_with_outside(field, width, height, x + 1, y, outside_value)
        - alpha_at_with_outside(field, width, height, x - 1, y, outside_value);
    let gy = alpha_at_with_outside(field, width, height, x, y + 1, outside_value)
        - alpha_at_with_outside(field, width, height, x, y - 1, outside_value);
    (gx, gy)
}

fn apply_tone_image(src: &[u8], params: ToneParams) -> Vec<u8> {
    let mut out = src.to_vec();
    out.par_chunks_exact_mut(4).for_each(|px| {
        let adjusted = tone_rgb([px[0], px[1], px[2]], params);
        px[0] = adjusted[0];
        px[1] = adjusted[1];
        px[2] = adjusted[2];
    });
    out
}

fn apply_tone_curve(src: &[u8], params: ToneCurveParams) -> Vec<u8> {
    let lut = tone_curve_lut(params);
    let mut out = src.to_vec();
    out.par_chunks_exact_mut(4).for_each(|px| {
        px[0] = lut[px[0] as usize];
        px[1] = lut[px[1] as usize];
        px[2] = lut[px[2] as usize];
    });
    out
}

fn apply_rgb_tone_curve(src: &[u8], params: RgbToneCurveParams) -> Vec<u8> {
    let red_lut = rgb_tone_curve_lut(params.master, params.red);
    let green_lut = rgb_tone_curve_lut(params.master, params.green);
    let blue_lut = rgb_tone_curve_lut(params.master, params.blue);
    let mut out = src.to_vec();
    out.par_chunks_exact_mut(4).for_each(|px| {
        px[0] = red_lut[px[0] as usize];
        px[1] = green_lut[px[1] as usize];
        px[2] = blue_lut[px[2] as usize];
    });
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
    out.par_chunks_exact_mut(4).for_each(|px| {
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
    });
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

fn apply_photo_filter(src: &[u8], params: PhotoFilterParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let density = params.density.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || density <= f32::EPSILON {
        return src.to_vec();
    }
    let filter = rgb_u8_to_f32(photo_filter_rgb(params));
    let mut out = src.to_vec();
    out.par_chunks_exact_mut(4).for_each(|px| {
        if px[3] == 0 {
            return;
        }
        let base = [
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        ];
        let mut filtered = [
            lerp_f32(base[0], filter[0], density),
            lerp_f32(base[1], filter[1], density),
            lerp_f32(base[2], filter[2], density),
        ];
        if params.preserve_luminosity {
            let base_luma = luma01(base[0], base[1], base[2]);
            let filtered_luma = luma01(filtered[0], filtered[1], filtered[2]);
            let delta = base_luma - filtered_luma;
            for channel in &mut filtered {
                *channel = (*channel + delta).clamp(0.0, 1.0);
            }
        }
        for c in 0..3 {
            px[c] = to_u8(lerp_f32(base[c], filtered[c], strength));
        }
    });
    out
}

fn photo_filter_rgb(params: PhotoFilterParams) -> [u8; 3] {
    match params.preset {
        PhotoFilterPreset::Custom => params.color_rgb,
        PhotoFilterPreset::Warm85 => [255, 174, 74],
        PhotoFilterPreset::Warm81 => [255, 202, 124],
        PhotoFilterPreset::Cool80 => [92, 165, 255],
        PhotoFilterPreset::Cool82 => [150, 205, 255],
        PhotoFilterPreset::Sepia => [196, 132, 68],
        PhotoFilterPreset::Sunset => [255, 112, 58],
        PhotoFilterPreset::Underwater => [45, 180, 205],
        PhotoFilterPreset::Magenta => [230, 88, 200],
        PhotoFilterPreset::Green => [98, 205, 105],
    }
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
    out.par_chunks_exact_mut(4).for_each(|px| {
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
    });
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
    out.par_chunks_exact_mut(4).for_each(|px| {
        let (mut h, mut s, mut l) = rgb_to_hsl(
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        );
        let hue_degrees = h * 360.0;
        let weight =
            selective_hue_weight(hue_degrees, target, range, feather) * smoothstep(0.03, 0.16, s);
        if weight <= f32::EPSILON {
            return;
        }
        h = wrap01(h + hue_shift * weight);
        s = (s * (1.0 + sat_delta * weight)).clamp(0.0, 1.0);
        l = (l + light_delta * 0.5 * weight).clamp(0.0, 1.0);
        let [r, g, b] = hsl_to_rgb(h, s, l);
        px[0] = to_u8(r);
        px[1] = to_u8(g);
        px[2] = to_u8(b);
    });
    out
}

fn apply_part_color(src: &[u8], params: PartColorParams) -> Vec<u8> {
    let gray_strength = params.gray_strength.clamp(0.0, 1.0);
    let sat_delta = (params.selected_saturation / 100.0).clamp(-1.0, 1.0);
    let light_delta = (params.selected_lightness / 100.0).clamp(-1.0, 1.0);
    if gray_strength <= f32::EPSILON
        && sat_delta.abs() <= f32::EPSILON
        && light_delta.abs() <= f32::EPSILON
    {
        return src.to_vec();
    }

    let target = part_color_target_hue_degrees(params.target_rgb);
    let range = params.range_degrees.clamp(1.0, 180.0);
    let feather = params.feather_degrees.clamp(0.0, 180.0);
    let mut out = src.to_vec();
    out.par_chunks_exact_mut(4).for_each(|px| {
        let r = px[0] as f32 / 255.0;
        let g = px[1] as f32 / 255.0;
        let b = px[2] as f32 / 255.0;
        let (h, s, mut l) = rgb_to_hsl(r, g, b);
        let hue_degrees = h * 360.0;
        let color_weight =
            selective_hue_weight(hue_degrees, target, range, feather) * smoothstep(0.03, 0.16, s);

        let selected_saturation = (s * (1.0 + sat_delta * color_weight)).clamp(0.0, 1.0);
        l = (l + light_delta * 0.5 * color_weight).clamp(0.0, 1.0);
        let selected = hsl_to_rgb(h, selected_saturation, l);
        let gray = luma01(r, g, b);
        let gray_mix = gray_strength * (1.0 - color_weight);

        px[0] = to_u8(lerp_f32(selected[0], gray, gray_mix));
        px[1] = to_u8(lerp_f32(selected[1], gray, gray_mix));
        px[2] = to_u8(lerp_f32(selected[2], gray, gray_mix));
    });
    out
}

fn part_color_target_hue_degrees(rgb: [u8; 3]) -> f32 {
    let (h, _, _) = rgb_to_hsl(
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
    );
    h * 360.0
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
    out.par_chunks_exact_mut(4).for_each(|px| {
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
    });
    out
}

fn mix_channels(rgb: [f32; 3], coeffs: [f32; 3]) -> f32 {
    (rgb[0] * coeffs[0] + rgb[1] * coeffs[1] + rgb[2] * coeffs[2]) / 100.0
}

fn apply_monochrome_mixer(src: &[u8], params: MonochromeMixerParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON {
        return src.to_vec();
    }
    let tint_strength = params.tint_strength.clamp(0.0, 1.0);
    let band_values = [
        params.red,
        params.yellow,
        params.green,
        params.cyan,
        params.blue,
        params.magenta,
    ];
    let band_centers = [0.0, 60.0, 120.0, 180.0, 240.0, 300.0];
    let tint = rgb_u8_to_f32(params.tint_rgb);
    let tint_luma = luma01(tint[0], tint[1], tint[2]).max(0.001);
    let contrast = (1.0 + params.contrast.clamp(-100.0, 100.0) / 100.0).max(0.0);
    let mut out = src.to_vec();
    out.par_chunks_exact_mut(4).for_each(|px| {
        if px[3] == 0 {
            return;
        }
        let base = [
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        ];
        let (h, s, _) = rgb_to_hsl(base[0], base[1], base[2]);
        let hue = h * 360.0;
        let mut weighted_delta = 0.0;
        let mut weight_sum = 0.0;
        for (center, value) in band_centers
            .iter()
            .copied()
            .zip(band_values.iter().copied())
        {
            let weight = monochrome_mixer_hue_weight(hue, center);
            weighted_delta += weight * value.clamp(-100.0, 100.0);
            weight_sum += weight;
        }
        let sat_weight = smoothstep(0.02, 0.32, s);
        let band_delta = if weight_sum > f32::EPSILON {
            weighted_delta / weight_sum / 100.0 * 0.45 * sat_weight
        } else {
            0.0
        };
        let mut gray = (luma01(base[0], base[1], base[2]) + band_delta).clamp(0.0, 1.0);
        gray = ((gray - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
        let mut mono = [gray, gray, gray];
        if tint_strength > f32::EPSILON {
            let toned = [
                (gray * tint[0] / tint_luma).clamp(0.0, 1.0),
                (gray * tint[1] / tint_luma).clamp(0.0, 1.0),
                (gray * tint[2] / tint_luma).clamp(0.0, 1.0),
            ];
            for c in 0..3 {
                mono[c] = lerp_f32(mono[c], toned[c], tint_strength);
            }
        }
        for c in 0..3 {
            px[c] = to_u8(lerp_f32(base[c], mono[c], strength));
        }
    });
    out
}

fn monochrome_mixer_hue_weight(hue_degrees: f32, center_degrees: f32) -> f32 {
    let delta = (hue_degrees - center_degrees).rem_euclid(360.0);
    let distance = delta.min(360.0 - delta);
    let t = (1.0 - distance / 90.0).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn apply_clarity(src: &[u8], width: usize, height: usize, radius: usize, amount: f32) -> Vec<u8> {
    if radius == 0 || amount.abs() <= f32::EPSILON {
        return src.to_vec();
    }
    let blur = box_blur_rgba(src, width, height, radius);
    let mut out = src.to_vec();
    out.par_chunks_exact_mut(4)
        .zip(src.par_chunks_exact(4))
        .zip(blur.par_chunks_exact(4))
        .for_each(|((out, src), blur)| {
            for c in 0..3 {
                let base = src[c] as f32;
                let low = blur[c] as f32;
                out[c] = (base + (base - low) * amount).round().clamp(0.0, 255.0) as u8;
            }
        });
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
    out.par_chunks_exact_mut(4)
        .zip(src.par_chunks_exact(4))
        .zip(fine.par_chunks_exact(4))
        .zip(coarse.par_chunks_exact(4))
        .for_each(|(((out, src), fine), coarse)| {
            for c in 0..3 {
                let base = src[c] as f32;
                let detail = fine[c] as f32 - coarse[c] as f32;
                out[c] = (base + detail * amount).round().clamp(0.0, 255.0) as u8;
            }
        });
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
    out.par_chunks_exact_mut(4)
        .zip(src.par_chunks_exact(4))
        .zip(blur.par_chunks_exact(4))
        .for_each(|((out, src), blur)| {
            for c in 0..3 {
                let base = src[c] as f32;
                let low = blur[c] as f32;
                let high_pass = (128.0 + (base - low) * contrast).round().clamp(0.0, 255.0) as u8;
                if detail_only {
                    out[c] = high_pass;
                } else {
                    let overlay = overlay_channel(src[c], high_pass);
                    out[c] = (base + (overlay as f32 - base) * amount)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
            }
        });
    out
}

fn apply_frequency_separation(
    src: &[u8],
    width: usize,
    height: usize,
    params: FrequencySeparationParams,
) -> Vec<u8> {
    let radius = params.radius_px.round().clamp(1.0, 128.0) as usize;
    let low_smoothing = params.low_smoothing.clamp(0.0, 1.0);
    let detail_amount = params.detail_amount.clamp(0.0, 2.0);
    let detail_contrast = params.detail_contrast.clamp(0.25, 2.0);
    let strength = params.strength.clamp(0.0, 1.0);
    let detail_scale = detail_amount * detail_contrast;
    if width == 0
        || height == 0
        || strength <= f32::EPSILON
        || (low_smoothing <= f32::EPSILON && (detail_scale - 1.0).abs() <= f32::EPSILON)
    {
        return src.to_vec();
    }

    let premultiplied = premultiply_rgba(src);
    let low = box_blur_rgba(&premultiplied, width, height, radius);
    let smoothed_low = if low_smoothing > f32::EPSILON {
        let smooth_radius = ((radius as f32 * 1.75).round() as usize).clamp(1, 192);
        Some(box_blur_rgba(&low, width, height, smooth_radius))
    } else {
        None
    };
    let mut out = src.to_vec();

    for i in (0..src.len()).step_by(4) {
        let alpha = src[i + 3];
        if alpha == 0 {
            continue;
        }
        let base = [
            src[i] as f32 / 255.0,
            src[i + 1] as f32 / 255.0,
            src[i + 2] as f32 / 255.0,
        ];
        let low_rgb = unpremultiply_rgb(&low, i).unwrap_or(base);
        let smooth_rgb = smoothed_low
            .as_ref()
            .and_then(|smooth| unpremultiply_rgb(smooth, i))
            .unwrap_or(low_rgb);
        for c in 0..3 {
            let low_adjusted = lerp_f32(low_rgb[c], smooth_rgb[c], low_smoothing);
            let detail = (base[c] - low_rgb[c]) * detail_scale;
            let target = (low_adjusted + detail).clamp(0.0, 1.0);
            out[i + c] = to_u8(lerp_f32(base[c], target, strength));
        }
        out[i + 3] = alpha;
    }

    out
}

fn premultiply_rgba(src: &[u8]) -> Vec<u8> {
    let mut out = vec![0_u8; src.len()];
    for i in (0..src.len()).step_by(4) {
        let alpha = src[i + 3] as f32 / 255.0;
        out[i] = to_u8(src[i] as f32 / 255.0 * alpha);
        out[i + 1] = to_u8(src[i + 1] as f32 / 255.0 * alpha);
        out[i + 2] = to_u8(src[i + 2] as f32 / 255.0 * alpha);
        out[i + 3] = src[i + 3];
    }
    out
}

fn unpremultiply_rgb(src: &[u8], i: usize) -> Option<[f32; 3]> {
    let alpha = src[i + 3] as f32 / 255.0;
    if alpha <= 1.0 / 255.0 {
        return None;
    }
    let inv_alpha = 1.0 / alpha;
    Some([
        (src[i] as f32 / 255.0 * inv_alpha).clamp(0.0, 1.0),
        (src[i + 1] as f32 / 255.0 * inv_alpha).clamp(0.0, 1.0),
        (src[i + 2] as f32 / 255.0 * inv_alpha).clamp(0.0, 1.0),
    ])
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
    // 水平 / 垂直とも出力行単位で独立しているため rayon で行並列化する
    // (整数加算 + 除算のみなので逐次版と bit 一致の結果になる)。
    let row_bytes = width * 4;
    let mut tmp = vec![0_u8; src.len()];
    tmp.par_chunks_exact_mut(row_bytes)
        .enumerate()
        .for_each(|(y, row)| {
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
                let o = x * 4;
                for c in 0..4 {
                    row[o + c] = (sum[c] / count) as u8;
                }
            }
        });
    let mut out = vec![0_u8; src.len()];
    out.par_chunks_exact_mut(row_bytes)
        .enumerate()
        .for_each(|(y, row)| {
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
                let o = x * 4;
                for c in 0..4 {
                    row[o + c] = (sum[c] / count) as u8;
                }
            }
        });
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

fn apply_wind(src: &[u8], width: usize, height: usize, params: WindParams) -> Vec<u8> {
    let distance = params.distance_px.clamp(0.0, 240.0);
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || distance <= 0.5 || strength <= f32::EPSILON {
        return src.to_vec();
    }

    let threshold = params.threshold.clamp(0.0, 1.0);
    let softness = params.softness.clamp(0.001, 1.0);
    let turbulence = params.turbulence.clamp(0.0, 1.0);
    let (dir_x, dir_y) = wind_direction_vector(params.direction);
    let (perp_x, perp_y) = (-dir_y, dir_x);
    let steps = (distance.ceil() as usize).clamp(1, 240);
    let mut signal = vec![0.0_f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let alpha = src[i + 3] as f32 / 255.0;
            let luma = luma01(
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            );
            signal[y * width + x] = match params.source {
                WindSource::Bright => luma * alpha,
                WindSource::Dark => (1.0 - luma) * alpha,
                WindSource::Edge => luma_edge_strength(src, width, height, x, y) * alpha,
            };
        }
    }

    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let mut best_weight = 0.0_f32;
            let mut best_rgb = [0_u8; 3];
            for step in 1..=steps {
                let step_f = step as f32;
                let gust = if turbulence <= f32::EPSILON {
                    0.0
                } else {
                    let gust_scale = (step_f / steps as f32) * turbulence * 3.0;
                    signed_noise(
                        (x as u32).wrapping_add(step as u32),
                        (y as u32).wrapping_add((step as u32).rotate_left(7)),
                        params.seed,
                    ) * gust_scale
                };
                let sx = x as f32 - dir_x * step_f + perp_x * gust;
                let sy = y as f32 - dir_y * step_f + perp_y * gust;
                let Some(si) = wind_sample_index(width, height, sx, sy) else {
                    continue;
                };
                let gate = smoothstep(threshold, (threshold + softness).min(1.0), signal[si / 4]);
                if gate <= f32::EPSILON {
                    continue;
                }
                let decay = (1.0 - step_f / (steps as f32 + 1.0)).powf(1.15);
                let weight = gate * decay;
                if weight > best_weight {
                    best_weight = weight;
                    best_rgb = [src[si], src[si + 1], src[si + 2]];
                }
            }
            if best_weight <= f32::EPSILON {
                continue;
            }
            let i = (y * width + x) * 4;
            let amount = best_weight * strength;
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], best_rgb[c], amount);
            }
        }
    }
    out
}

fn wind_direction_vector(direction: WindDirection) -> (f32, f32) {
    match direction {
        WindDirection::Right => (1.0, 0.0),
        WindDirection::Left => (-1.0, 0.0),
        WindDirection::Down => (0.0, 1.0),
        WindDirection::Up => (0.0, -1.0),
    }
}

fn wind_sample_index(width: usize, height: usize, x: f32, y: f32) -> Option<usize> {
    let x = x.round() as isize;
    let y = y.round() as isize;
    if x < 0 || y < 0 || x >= width as isize || y >= height as isize {
        return None;
    }
    Some((y as usize * width + x as usize) * 4)
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

fn apply_bokeh_sprite(
    src: &[u8],
    width: usize,
    height: usize,
    params: BokehSpriteParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let density = params.density.clamp(0.0, 1.0);
    let size = params.size_px.clamp(2.0, 96.0);
    let brightness = params.brightness.clamp(0.0, 2.0);
    if width == 0
        || height == 0
        || strength <= f32::EPSILON
        || density <= f32::EPSILON
        || brightness <= f32::EPSILON
    {
        return src.to_vec();
    }

    let threshold = params.threshold.clamp(0.0, 0.9999);
    let inv_range = 1.0 / (1.0 - threshold).max(0.001);
    let softness = params.softness.clamp(0.0, 1.0);
    let color_strength = params.color_strength.clamp(0.0, 1.0);
    let spacing = (size * lerp_f32(2.6, 0.65, density))
        .round()
        .clamp(2.0, 128.0) as usize;
    let mut sprites = vec![0.0_f32; width * height * 3];

    for cell_y in (0..height).step_by(spacing) {
        let y_end = (cell_y + spacing).min(height);
        for cell_x in (0..width).step_by(spacing) {
            let x_end = (cell_x + spacing).min(width);
            let mut best: Option<(usize, usize, usize, f32)> = None;
            for y in cell_y..y_end {
                for x in cell_x..x_end {
                    let i = (y * width + x) * 4;
                    let alpha = src[i + 3] as f32 / 255.0;
                    if alpha <= f32::EPSILON {
                        continue;
                    }
                    let r = src[i] as f32 / 255.0;
                    let g = src[i + 1] as f32 / 255.0;
                    let b = src[i + 2] as f32 / 255.0;
                    let max_channel = r.max(g).max(b);
                    let signal = luma01(r, g, b).max(max_channel * 0.90) * alpha;
                    if signal
                        > best
                            .map(|(_, _, _, best_signal)| best_signal)
                            .unwrap_or(threshold)
                    {
                        best = Some((x, y, i, signal));
                    }
                }
            }

            let Some((x, y, i, signal)) = best else {
                continue;
            };
            let gate = ((signal - threshold) * inv_range).clamp(0.0, 1.0);
            if gate <= 0.001 {
                continue;
            }
            let cell_ix = (cell_x / spacing) as u32;
            let cell_iy = (cell_y / spacing) as u32;
            let radius_noise = signed_noise(cell_ix, cell_iy, params.seed ^ 0xB04E_1105);
            let jitter_x = signed_noise(cell_ix, cell_iy, params.seed ^ 0x51A7_C1E5) * size * 0.10;
            let jitter_y = signed_noise(cell_ix, cell_iy, params.seed ^ 0xA11E_5EED) * size * 0.10;
            let radius = (size * (0.50 + radius_noise * 0.12)).clamp(1.0, 96.0);
            let source = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let color = [
                lerp_f32(1.0, source[0], color_strength),
                lerp_f32(1.0, source[1], color_strength),
                lerp_f32(1.0, source[2], color_strength),
            ];
            draw_bokeh_sprite(
                &mut sprites,
                width,
                height,
                x as f32 + 0.5 + jitter_x,
                y as f32 + 0.5 + jitter_y,
                radius,
                params.shape,
                softness,
                color,
                gate * brightness,
            );
        }
    }

    let mut out = src.to_vec();
    for i in 0..width * height {
        let si = i * 3;
        let oi = i * 4;
        for c in 0..3 {
            let base = src[oi + c] as f32 / 255.0;
            let overlay = (sprites[si + c] * strength).clamp(0.0, 1.0);
            out[oi + c] = to_u8(screen_channel(base, overlay));
        }
    }
    out
}

fn draw_bokeh_sprite(
    dst: &mut [f32],
    width: usize,
    height: usize,
    center_x: f32,
    center_y: f32,
    radius: f32,
    shape: BokehSpriteShape,
    softness: f32,
    color: [f32; 3],
    weight: f32,
) {
    if radius <= f32::EPSILON || weight <= f32::EPSILON {
        return;
    }
    let pad = radius + 1.0;
    let min_x = (center_x - pad).floor().max(0.0) as usize;
    let min_y = (center_y - pad).floor().max(0.0) as usize;
    let max_x = (center_x + pad).ceil().min(width.saturating_sub(1) as f32) as usize;
    let max_y = (center_y + pad).ceil().min(height.saturating_sub(1) as f32) as usize;
    let edge = (0.035 + softness * 0.25).clamp(0.035, 0.32);
    let gamma = lerp_f32(0.68, 1.35, softness);

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let nx = (x as f32 + 0.5 - center_x) / radius;
            let ny = (y as f32 + 0.5 - center_y) / radius;
            let shape_alpha = bokeh_sprite_shape_alpha(shape, nx, ny, edge);
            if shape_alpha <= 0.001 {
                continue;
            }
            let distance = (nx * nx + ny * ny).sqrt().clamp(0.0, 1.2);
            let rim = smoothstep(0.58, 0.98, distance) * 0.28;
            let falloff = (1.0 - distance * (0.18 + softness * 0.12)).clamp(0.60, 1.0);
            let amount = shape_alpha.powf(gamma) * (1.0 + rim) * falloff * weight;
            if amount <= 0.001 {
                continue;
            }
            let i = (y * width + x) * 3;
            for c in 0..3 {
                dst[i + c] += color[c] * amount;
            }
        }
    }
}

fn bokeh_sprite_shape_alpha(shape: BokehSpriteShape, nx: f32, ny: f32, edge: f32) -> f32 {
    match shape {
        BokehSpriteShape::Circle => {
            let r = (nx * nx + ny * ny).sqrt();
            1.0 - smoothstep(1.0 - edge, 1.0 + edge, r)
        }
        BokehSpriteShape::Star => {
            let r = (nx * nx + ny * ny).sqrt();
            let angle = ny.atan2(nx);
            let point = (0.5 + 0.5 * (angle * 5.0).cos()).powf(1.35);
            let boundary = 0.56 + point * 0.42;
            1.0 - smoothstep(boundary - edge, boundary + edge, r)
        }
        BokehSpriteShape::Heart => {
            let x = nx * 1.18;
            let y = -ny * 1.18 + 0.18;
            let f = (x * x + y * y - 1.0).powi(3) - x * x * y.powi(3);
            let band = (0.10 + edge * 1.8).max(0.05);
            let t = ((band - f) / (band * 2.0)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        }
    }
    .clamp(0.0, 1.0)
}

fn apply_lens_dirt(src: &[u8], width: usize, height: usize, params: LensDirtParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let opacity = params.opacity.clamp(0.0, 1.0);
    let density = params.density.clamp(0.0, 1.0);
    if width == 0
        || height == 0
        || strength <= f32::EPSILON
        || opacity <= f32::EPSILON
        || density <= f32::EPSILON
    {
        return src.to_vec();
    }

    let size = params.size_px.clamp(2.0, 128.0);
    let softness = params.softness.clamp(0.0, 1.0);
    let highlight_response = params.highlight_response.clamp(0.0, 1.0);
    let distortion = params.distortion_px.clamp(0.0, 32.0);
    let len = width.saturating_mul(height);
    let mut grime = vec![0.0_f32; len];
    let mut shine = vec![0.0_f32; len];
    let mut refract_x = vec![0.0_f32; len];
    let mut refract_y = vec![0.0_f32; len];

    match params.mode {
        LensDirtMode::Dust => paint_lens_dust(
            &mut grime,
            &mut shine,
            width,
            height,
            size,
            density,
            softness,
            params.seed,
        ),
        LensDirtMode::WaterDrops => paint_lens_water_drops(
            &mut grime,
            &mut shine,
            &mut refract_x,
            &mut refract_y,
            width,
            height,
            size,
            density,
            softness,
            params.seed,
        ),
        LensDirtMode::Smudges => paint_lens_smudges(
            &mut grime,
            &mut shine,
            width,
            height,
            size,
            density,
            softness,
            params.seed,
        ),
    }

    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let oi = idx * 4;
            let alpha = src[oi + 3] as f32 / 255.0;
            if alpha <= f32::EPSILON {
                continue;
            }
            let base = [
                src[oi] as f32 / 255.0,
                src[oi + 1] as f32 / 255.0,
                src[oi + 2] as f32 / 255.0,
            ];
            let luma = luma01(base[0], base[1], base[2]);
            let visibility =
                opacity * strength * lerp_f32(1.0, 0.22 + luma * 1.05, highlight_response);
            let grime_amount = grime[idx].clamp(0.0, 1.0) * visibility;
            let shine_amount =
                shine[idx].clamp(0.0, 1.0) * visibility * (0.45 + highlight_response * luma * 1.35);
            if grime_amount <= f32::EPSILON && shine_amount <= f32::EPSILON {
                continue;
            }

            let mut sampled = base;
            if distortion > f32::EPSILON {
                let sx = x as f32 + refract_x[idx] * distortion * strength;
                let sy = y as f32 + refract_y[idx] * distortion * strength;
                let (rgb, sample_alpha) =
                    sample_rgb_bilinear_alpha_aware(src, width, height, sx, sy);
                if sample_alpha > f32::EPSILON {
                    sampled = rgb;
                }
            }

            let target = match params.mode {
                LensDirtMode::Dust => lens_dust_rgb(sampled, grime_amount, shine_amount),
                LensDirtMode::WaterDrops => {
                    lens_water_drop_rgb(sampled, grime_amount, shine_amount)
                }
                LensDirtMode::Smudges => lens_smudge_rgb(sampled, grime_amount, shine_amount),
            };
            for c in 0..3 {
                out[oi + c] = to_u8(target[c]);
            }
        }
    }

    out
}

#[allow(clippy::too_many_arguments)]
fn paint_lens_dust(
    grime: &mut [f32],
    shine: &mut [f32],
    width: usize,
    height: usize,
    size: f32,
    density: f32,
    softness: f32,
    seed: u32,
) {
    let spacing = (size * lerp_f32(3.2, 0.8, density))
        .round()
        .clamp(2.0, 128.0) as usize;
    for cell_y in (0..height).step_by(spacing) {
        for cell_x in (0..width).step_by(spacing) {
            let gx = (cell_x / spacing) as u32;
            let gy = (cell_y / spacing) as u32;
            if lens_noise01(gx, gy, seed) > density {
                continue;
            }
            let jitter_x = lens_noise01(gx, gy, seed ^ 0xD15A_11CE);
            let jitter_y = lens_noise01(gx, gy, seed ^ 0xA53D_0011);
            let cx = cell_x as f32 + jitter_x * spacing as f32;
            let cy = cell_y as f32 + jitter_y * spacing as f32;
            let radius = (size * lerp_f32(0.08, 0.34, lens_noise01(gx, gy, seed ^ 0x51E6_5EED)))
                .clamp(0.6, 18.0);
            let scratch = lens_noise01(gx, gy, seed ^ 0x5C8A_7C4D) > 0.86;
            let angle = signed_noise(gx, gy, seed ^ 0xA119_1E55) * std::f32::consts::PI;
            let rx = if scratch { radius * 5.2 } else { radius * 1.35 };
            let ry = if scratch { radius * 0.45 } else { radius };
            paint_lens_ellipse(
                grime,
                shine,
                width,
                height,
                cx,
                cy,
                rx,
                ry,
                angle,
                0.50 + density * 0.35,
                if scratch { 0.12 } else { 0.04 },
                0.10 + softness * 0.26,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_lens_water_drops(
    grime: &mut [f32],
    shine: &mut [f32],
    refract_x: &mut [f32],
    refract_y: &mut [f32],
    width: usize,
    height: usize,
    size: f32,
    density: f32,
    softness: f32,
    seed: u32,
) {
    let spacing = (size * lerp_f32(2.4, 0.72, density))
        .round()
        .clamp(3.0, 160.0) as usize;
    for cell_y in (0..height).step_by(spacing) {
        for cell_x in (0..width).step_by(spacing) {
            let gx = (cell_x / spacing) as u32;
            let gy = (cell_y / spacing) as u32;
            if lens_noise01(gx, gy, seed ^ 0xA7E2_0615) > density {
                continue;
            }
            let cx = cell_x as f32 + lens_noise01(gx, gy, seed ^ 0xC0DE_0011) * spacing as f32;
            let cy = cell_y as f32 + lens_noise01(gx, gy, seed ^ 0xC0DE_0021) * spacing as f32;
            let radius = (size * lerp_f32(0.28, 0.62, lens_noise01(gx, gy, seed ^ 0xC0DE_0031)))
                .clamp(1.5, 96.0);
            let min_x = (cx - radius - 2.0).floor().max(0.0) as usize;
            let min_y = (cy - radius - 2.0).floor().max(0.0) as usize;
            let max_x = (cx + radius + 2.0)
                .ceil()
                .min(width.saturating_sub(1) as f32) as usize;
            let max_y = (cy + radius + 2.0)
                .ceil()
                .min(height.saturating_sub(1) as f32) as usize;
            let edge = 0.05 + softness * 0.25;
            for y in min_y..=max_y {
                for x in min_x..=max_x {
                    let nx = (x as f32 + 0.5 - cx) / radius;
                    let ny = (y as f32 + 0.5 - cy) / radius;
                    let r = (nx * nx + ny * ny).sqrt();
                    if r > 1.16 {
                        continue;
                    }
                    let body = 1.0 - smoothstep(0.82, 1.05 + edge, r);
                    let ring = smoothstep(0.48, 0.82, r) * (1.0 - smoothstep(0.95, 1.12, r));
                    let highlight = smoothstep(0.68, 0.95, r) * (1.0 - smoothstep(0.98, 1.15, r));
                    let idx = y * width + x;
                    grime[idx] += (body * 0.22 + ring * 0.50).clamp(0.0, 1.0);
                    shine[idx] += highlight * (0.65 + softness * 0.25);
                    let normal_gain = (body * (1.0 - r).max(0.0) + ring * 0.45).clamp(0.0, 1.0);
                    refract_x[idx] += nx * normal_gain;
                    refract_y[idx] += ny * normal_gain;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_lens_smudges(
    grime: &mut [f32],
    shine: &mut [f32],
    width: usize,
    height: usize,
    size: f32,
    density: f32,
    softness: f32,
    seed: u32,
) {
    let scale = size.clamp(8.0, 160.0);
    let angle = signed_noise(7, 11, seed ^ 0x5EA1_5EED) * std::f32::consts::PI;
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let threshold = lerp_f32(0.88, 0.28, density);
    for y in 0..height {
        for x in 0..width {
            let u = (x as f32 * cos_a + y as f32 * sin_a) / scale;
            let v = (-(x as f32) * sin_a + y as f32 * cos_a) / (scale * 0.36).max(1.0);
            let coarse = glass_value_noise(u, v, seed);
            let fine = glass_value_noise(u * 2.7 + 13.1, v * 1.9 - 5.7, seed ^ 0x91E7_5A1D);
            let streak = ((v * std::f32::consts::TAU).sin() * 0.5 + 0.5) * 2.0 - 1.0;
            let raw = 0.5 + (coarse * 0.52 + fine * 0.28 + streak * 0.20) * 0.5;
            let alpha = smoothstep(threshold, 1.0, raw) * (0.62 + softness * 0.34);
            if alpha <= 0.001 {
                continue;
            }
            let idx = y * width + x;
            grime[idx] += alpha;
            shine[idx] += alpha * (0.16 + softness * 0.18);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_lens_ellipse(
    grime: &mut [f32],
    shine: &mut [f32],
    width: usize,
    height: usize,
    center_x: f32,
    center_y: f32,
    radius_x: f32,
    radius_y: f32,
    angle: f32,
    amount: f32,
    shine_amount: f32,
    edge: f32,
) {
    let pad = radius_x.max(radius_y) + 2.0;
    let min_x = (center_x - pad).floor().max(0.0) as usize;
    let min_y = (center_y - pad).floor().max(0.0) as usize;
    let max_x = (center_x + pad).ceil().min(width.saturating_sub(1) as f32) as usize;
    let max_y = (center_y + pad).ceil().min(height.saturating_sub(1) as f32) as usize;
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let rx = radius_x.max(0.001);
    let ry = radius_y.max(0.001);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f32 + 0.5 - center_x;
            let dy = y as f32 + 0.5 - center_y;
            let u = (dx * cos_a + dy * sin_a) / rx;
            let v = (-dx * sin_a + dy * cos_a) / ry;
            let r = (u * u + v * v).sqrt();
            if r > 1.12 {
                continue;
            }
            let alpha = 1.0 - smoothstep(1.0 - edge, 1.0 + edge, r);
            if alpha <= 0.001 {
                continue;
            }
            let idx = y * width + x;
            grime[idx] += alpha * amount;
            shine[idx] += alpha * shine_amount;
        }
    }
}

fn lens_dust_rgb(rgb: [f32; 3], grime: f32, shine: f32) -> [f32; 3] {
    let mut out = rgb;
    for c in 0..3 {
        out[c] = (out[c] * (1.0 - grime * 0.62)).clamp(0.0, 1.0);
        out[c] = screen_channel(out[c], shine * 0.30);
    }
    out
}

fn lens_water_drop_rgb(rgb: [f32; 3], grime: f32, shine: f32) -> [f32; 3] {
    let mut out = rgb;
    for c in 0..3 {
        out[c] = (out[c] * (1.0 - grime * 0.12)).clamp(0.0, 1.0);
        out[c] = screen_channel(out[c], shine * 0.72);
    }
    out
}

fn lens_smudge_rgb(rgb: [f32; 3], grime: f32, shine: f32) -> [f32; 3] {
    let luma = luma01(rgb[0], rgb[1], rgb[2]);
    let mut out = rgb;
    for c in 0..3 {
        let hazed = screen_channel(out[c], grime * 0.58 + shine * 0.24);
        out[c] = lerp_f32(hazed, luma + (hazed - luma) * 0.72, grime * 0.38);
    }
    out
}

fn lens_noise01(x: u32, y: u32, seed: u32) -> f32 {
    (signed_noise(x, y, seed) * 0.5 + 0.5).clamp(0.0, 1.0)
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

fn apply_heat_haze(src: &[u8], width: usize, height: usize, params: HeatHazeParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let amplitude = params.amplitude_px.clamp(-160.0, 160.0);
    let rise = params.rise_px.clamp(-160.0, 160.0);
    let blur = params.blur_px.clamp(0.0, 12.0);
    if width == 0
        || height == 0
        || strength <= f32::EPSILON
        || (amplitude.abs() <= f32::EPSILON && rise.abs() <= f32::EPSILON && blur <= f32::EPSILON)
    {
        return src.to_vec();
    }

    let wavelength = params.wavelength_px.clamp(4.0, 360.0);
    let turbulence = params.turbulence.clamp(0.0, 1.0);
    let phase = params.phase_degrees.to_radians();
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let xf = x as f32;
            let yf = y as f32;
            let wave = (yf / wavelength * std::f32::consts::TAU + phase).sin();
            let cross = ((yf * 1.73 + xf * 0.41) / (wavelength * 0.58).max(4.0)
                * std::f32::consts::TAU
                + phase * 1.31)
                .sin();
            let shimmer = lerp_f32(
                wave,
                (wave * 0.58 + cross * 0.42).clamp(-1.0, 1.0),
                turbulence,
            );
            let vertical_wobble = (cross * 0.5 + 0.5) * turbulence;
            let sx = xf + amplitude * shimmer;
            let sy = yf + rise * (0.55 + vertical_wobble * 0.45);
            let i = (y * width + x) * 4;
            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let sampled = sample_heat_haze_rgb(src, width, height, sx, sy, blur, base);
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], to_u8(sampled[c]), strength);
            }
        }
    }
    out
}

fn sample_heat_haze_rgb(
    src: &[u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    blur: f32,
    fallback: [f32; 3],
) -> [f32; 3] {
    if blur <= 0.05 {
        let (rgb, alpha) = sample_rgb_bilinear_alpha_aware(src, width, height, x, y);
        return if alpha > f32::EPSILON { rgb } else { fallback };
    }

    let taps = [
        (0.0_f32, 0.0_f32, 0.44_f32),
        (blur, 0.0, 0.14),
        (-blur, 0.0, 0.14),
        (0.0, blur, 0.14),
        (0.0, -blur, 0.14),
    ];
    let mut sum = [0.0_f32; 3];
    let mut weight_sum = 0.0_f32;
    for (ox, oy, weight) in taps {
        let (rgb, alpha) = sample_rgb_bilinear_alpha_aware(src, width, height, x + ox, y + oy);
        let weighted_alpha = weight * alpha;
        if weighted_alpha <= f32::EPSILON {
            continue;
        }
        for c in 0..3 {
            sum[c] += rgb[c] * weighted_alpha;
        }
        weight_sum += weighted_alpha;
    }
    if weight_sum <= f32::EPSILON {
        return fallback;
    }
    [
        sum[0] / weight_sum,
        sum[1] / weight_sum,
        sum[2] / weight_sum,
    ]
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

fn apply_twirl(src: &[u8], width: usize, height: usize, params: TwirlParams) -> Vec<u8> {
    let angle = params.angle_degrees.clamp(-1080.0, 1080.0).to_radians();
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || angle.abs() <= f32::EPSILON || strength <= f32::EPSILON {
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
            let theta = angle * (1.0 - t) * (1.0 - t);
            let cos_t = theta.cos();
            let sin_t = theta.sin();
            let sx = cx + dx * cos_t - dy * sin_t;
            let sy = cy + dx * sin_t + dy * cos_t;
            let sampled = sample_rgb_bilinear(src, width, height, sx, sy);
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], to_u8(sampled[c]), strength);
            }
        }
    }
    out
}

fn apply_polar_coordinates(
    src: &[u8],
    width: usize,
    height: usize,
    params: PolarCoordinatesParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || strength <= f32::EPSILON {
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
    let max_x = width.saturating_sub(1) as f32;
    let max_y = height.saturating_sub(1) as f32;
    let denom_x = max_x.max(1.0);
    let denom_y = max_y.max(1.0);
    let angle_offset = params.angle_offset_degrees.to_radians();
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let (sample_pos, inside_effect) = match params.mode {
                PolarCoordinatesMode::RectToPolar => {
                    let dx = x as f32 - cx;
                    let dy = y as f32 - cy;
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist > radius {
                        ((x as f32, y as f32), false)
                    } else {
                        let angle_t = (dy.atan2(dx) - angle_offset)
                            .rem_euclid(std::f32::consts::TAU)
                            / std::f32::consts::TAU;
                        let mut radius_t = (dist / radius).clamp(0.0, 1.0);
                        if params.invert_radius {
                            radius_t = 1.0 - radius_t;
                        }
                        ((angle_t * max_x, radius_t * max_y), true)
                    }
                }
                PolarCoordinatesMode::PolarToRect => {
                    let angle_t = x as f32 / denom_x;
                    let mut radius_t = y as f32 / denom_y;
                    if params.invert_radius {
                        radius_t = 1.0 - radius_t;
                    }
                    let angle = angle_t * std::f32::consts::TAU + angle_offset;
                    let dist = radius * radius_t.clamp(0.0, 1.0);
                    ((cx + angle.cos() * dist, cy + angle.sin() * dist), true)
                }
            };
            if !inside_effect {
                continue;
            }
            let sampled = sample_rgb_bilinear(src, width, height, sample_pos.0, sample_pos.1);
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], to_u8(sampled[c]), strength);
            }
        }
    }
    out
}

fn apply_glass_displacement(
    src: &[u8],
    width: usize,
    height: usize,
    params: GlassDisplacementParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let displacement = params.displacement_px.clamp(0.0, 128.0);
    if width == 0 || height == 0 || strength <= f32::EPSILON || displacement <= f32::EPSILON {
        return src.to_vec();
    }
    let scale = params.scale_px.max(2.0);
    let detail = params.detail.clamp(0.0, 1.0);
    let angle = params.angle_degrees.to_radians();
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let (dx, dy) = glass_displacement_vector(
                x as f32,
                y as f32,
                params.mode,
                scale,
                detail,
                cos_a,
                sin_a,
                params.seed,
            );
            let sx = x as f32 + dx * displacement;
            let sy = y as f32 + dy * displacement;
            let sampled = sample_rgb_bilinear(src, width, height, sx, sy);
            let i = (y * width + x) * 4;
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], to_u8(sampled[c]), strength);
            }
        }
    }
    out
}

fn glass_displacement_vector(
    x: f32,
    y: f32,
    mode: GlassDisplacementMode,
    scale: f32,
    detail: f32,
    cos_a: f32,
    sin_a: f32,
    seed: u32,
) -> (f32, f32) {
    let u = (x * cos_a + y * sin_a) / scale;
    let v = (-x * sin_a + y * cos_a) / scale;
    let (local_x, local_y) = match mode {
        GlassDisplacementMode::Frosted => {
            let fine_scale = 2.0 + detail * 3.0;
            let fine_weight = detail * 0.45;
            let coarse_weight = 1.0 - fine_weight * 0.5;
            let dx = glass_value_noise(u, v, seed) * coarse_weight
                + glass_value_noise(
                    u * fine_scale + 13.7,
                    v * fine_scale - 5.3,
                    seed ^ 0xA511_E9B3,
                ) * fine_weight;
            let dy = glass_value_noise(u + 31.2, v - 17.9, seed ^ 0x63D8_3511) * coarse_weight
                + glass_value_noise(
                    u * fine_scale - 19.1,
                    v * fine_scale + 7.4,
                    seed ^ 0xB529_7A4D,
                ) * fine_weight;
            (dx, dy)
        }
        GlassDisplacementMode::Ripple => {
            let primary = (u * std::f32::consts::TAU).sin();
            let cross = ((v + u * 0.35) * std::f32::consts::TAU).sin() * detail;
            (primary, cross)
        }
        GlassDisplacementMode::Faceted => {
            let cell_x = u.floor() as i32;
            let cell_y = v.floor() as i32;
            (
                signed_noise(cell_x as u32, cell_y as u32, seed),
                signed_noise(cell_x as u32, cell_y as u32, seed ^ 0x9E37_79B9),
            )
        }
    };
    let x = local_x * cos_a - local_y * sin_a;
    let y = local_x * sin_a + local_y * cos_a;
    let len = (x * x + y * y).sqrt();
    if len > 1.0 {
        (x / len, y / len)
    } else {
        (x, y)
    }
}

fn glass_value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = x0.wrapping_add(1);
    let y1 = y0.wrapping_add(1);
    let tx = smoothstep(0.0, 1.0, x - x0 as f32);
    let ty = smoothstep(0.0, 1.0, y - y0 as f32);
    let n00 = signed_noise(x0 as u32, y0 as u32, seed);
    let n10 = signed_noise(x1 as u32, y0 as u32, seed);
    let n01 = signed_noise(x0 as u32, y1 as u32, seed);
    let n11 = signed_noise(x1 as u32, y1 as u32, seed);
    let top = lerp_f32(n00, n10, tx);
    let bottom = lerp_f32(n01, n11, tx);
    lerp_f32(top, bottom, ty)
}

fn apply_lens_correction(
    src: &[u8],
    width: usize,
    height: usize,
    params: LensCorrectionParams,
) -> Vec<u8> {
    let distortion = params.distortion.clamp(-1.0, 1.0);
    let zoom = params.zoom.clamp(0.0, 0.5);
    let vignette = params.vignette_correction.clamp(-1.0, 1.0);
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0
        || height == 0
        || strength <= f32::EPSILON
        || (distortion.abs() <= f32::EPSILON
            && zoom <= f32::EPSILON
            && vignette.abs() <= f32::EPSILON)
    {
        return src.to_vec();
    }
    let cx = (width.saturating_sub(1)) as f32 * params.center[0].clamp(0.0, 1.0);
    let cy = (height.saturating_sub(1)) as f32 * params.center[1].clamp(0.0, 1.0);
    let radius = farthest_corner_distance(width, height, cx, cy).max(1.0);
    let zoom_scale = 1.0 / (1.0 + zoom);
    let k1 = distortion * 0.72;
    let k2 = distortion.abs() * distortion * 0.18;
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let dx = (x as f32 - cx) * zoom_scale;
            let dy = (y as f32 - cy) * zoom_scale;
            let r = ((dx * dx + dy * dy).sqrt() / radius).clamp(0.0, 1.0);
            let r2 = r * r;
            let radial_scale = (1.0 + k1 * r2 + k2 * r2 * r2).max(0.05);
            let sx = cx + dx * radial_scale;
            let sy = cy + dy * radial_scale;
            let mut sampled = sample_rgb_bilinear(src, width, height, sx, sy);
            if vignette.abs() > f32::EPSILON {
                let edge = smoothstep(0.18, 1.0, r);
                for c in &mut sampled {
                    if vignette >= 0.0 {
                        *c += (1.0 - *c) * vignette * edge * 0.75;
                    } else {
                        *c *= 1.0 + vignette * edge * 0.75;
                    }
                    *c = (*c).clamp(0.0, 1.0);
                }
            }
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], to_u8(sampled[c]), strength);
            }
        }
    }
    out
}

fn apply_line_extract(
    src: &[u8],
    width: usize,
    height: usize,
    params: LineExtractParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || strength <= f32::EPSILON {
        return src.to_vec();
    }
    let threshold = params.threshold.clamp(0.0, 1.0);
    let softness = params.softness.clamp(0.001, 1.0);
    let radius = params.thickness_px.round().clamp(1.0, 8.0) as usize - 1;

    let luma = src
        .chunks_exact(4)
        .map(|px| {
            let alpha = px[3] as f32 / 255.0;
            luma01(
                px[0] as f32 / 255.0,
                px[1] as f32 / 255.0,
                px[2] as f32 / 255.0,
            ) * alpha
        })
        .collect::<Vec<_>>();

    let mut edges = vec![0.0_f32; width * height];
    for y in 0..height {
        for x in 0..width {
            edges[y * width + x] = line_extract_sobel_edge(&luma, width, height, x, y);
        }
    }

    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let edge = if radius == 0 {
                edges[y * width + x]
            } else {
                line_extract_max_edge(&edges, width, height, x, y, radius)
            };
            let line = smoothstep(threshold, (threshold + softness).min(1.0), edge);
            if line <= f32::EPSILON
                && matches!(
                    params.mode,
                    LineExtractMode::DarkenOriginal | LineExtractMode::LightenOriginal
                )
            {
                continue;
            }

            let i = (y * width + x) * 4;
            let target = match params.mode {
                LineExtractMode::BlackOnWhite => {
                    let v = 1.0 - line;
                    [v, v, v]
                }
                LineExtractMode::WhiteOnBlack => [line, line, line],
                LineExtractMode::DarkenOriginal => [
                    src[i] as f32 / 255.0 * (1.0 - line),
                    src[i + 1] as f32 / 255.0 * (1.0 - line),
                    src[i + 2] as f32 / 255.0 * (1.0 - line),
                ],
                LineExtractMode::LightenOriginal => [
                    src[i] as f32 / 255.0 + (1.0 - src[i] as f32 / 255.0) * line,
                    src[i + 1] as f32 / 255.0 + (1.0 - src[i + 1] as f32 / 255.0) * line,
                    src[i + 2] as f32 / 255.0 + (1.0 - src[i + 2] as f32 / 255.0) * line,
                ],
            };
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], to_u8(target[c]), strength);
            }
        }
    }
    out
}

fn line_extract_sobel_edge(luma: &[f32], width: usize, height: usize, x: usize, y: usize) -> f32 {
    let max_x = width.saturating_sub(1) as isize;
    let max_y = height.saturating_sub(1) as isize;
    let sample = |x: isize, y: isize| -> f32 {
        let x = x.clamp(0, max_x) as usize;
        let y = y.clamp(0, max_y) as usize;
        luma[y * width + x]
    };
    let x = x as isize;
    let y = y as isize;
    let gx = -sample(x - 1, y - 1) + sample(x + 1, y - 1) - 2.0 * sample(x - 1, y)
        + 2.0 * sample(x + 1, y)
        - sample(x - 1, y + 1)
        + sample(x + 1, y + 1);
    let gy = -sample(x - 1, y - 1) - 2.0 * sample(x, y - 1) - sample(x + 1, y - 1)
        + sample(x - 1, y + 1)
        + 2.0 * sample(x, y + 1)
        + sample(x + 1, y + 1);
    ((gx * gx + gy * gy).sqrt() * 0.25).min(1.0)
}

fn line_extract_max_edge(
    edges: &[f32],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    radius: usize,
) -> f32 {
    let y0 = y.saturating_sub(radius);
    let y1 = (y + radius).min(height - 1);
    let x0 = x.saturating_sub(radius);
    let x1 = (x + radius).min(width - 1);
    let mut best = 0.0_f32;
    for yy in y0..=y1 {
        for xx in x0..=x1 {
            best = best.max(edges[yy * width + xx]);
        }
    }
    best
}

fn apply_glowing_edges(
    src: &[u8],
    width: usize,
    height: usize,
    params: GlowingEdgesParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || strength <= f32::EPSILON {
        return src.to_vec();
    }
    let threshold = params.threshold.clamp(0.0, 1.0);
    let softness = params.softness.clamp(0.0, 1.0);
    let edge_radius = params.edge_width_px.round().clamp(1.0, 12.0) as usize - 1;
    let glow_radius = params.glow_radius_px.round().clamp(0.0, 120.0) as usize;
    let edge_brightness = params.edge_brightness.clamp(0.0, 3.0);
    let glow_strength = params.glow_strength.clamp(0.0, 3.0);
    let color_amount = params.color_amount.clamp(0.0, 1.0);
    let background_amount = params.background_amount.clamp(0.0, 1.0);
    let neon = hsl_to_rgb((params.hue_degrees / 360.0).rem_euclid(1.0), 1.0, 0.55);

    let luma = src
        .chunks_exact(4)
        .map(|px| {
            let alpha = px[3] as f32 / 255.0;
            luma01(
                px[0] as f32 / 255.0,
                px[1] as f32 / 255.0,
                px[2] as f32 / 255.0,
            ) * alpha
        })
        .collect::<Vec<_>>();

    let mut edges = vec![0.0_f32; width * height];
    for y in 0..height {
        for x in 0..width {
            edges[y * width + x] = line_extract_sobel_edge(&luma, width, height, x, y);
        }
    }

    let mut edge_plate = vec![0_u8; src.len()];
    for y in 0..height {
        for x in 0..width {
            let edge = if edge_radius == 0 {
                edges[y * width + x]
            } else {
                line_extract_max_edge(&edges, width, height, x, y, edge_radius)
            };
            let edge_alpha = glowing_edges_gate(edge, threshold, softness);
            let i = (y * width + x) * 4;
            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let source_color = adjust_saturation(base, 1.25);
            let edge_color = [
                lerp_f32(source_color[0], neon[0], color_amount),
                lerp_f32(source_color[1], neon[1], color_amount),
                lerp_f32(source_color[2], neon[2], color_amount),
            ];
            for c in 0..3 {
                edge_plate[i + c] = to_u8(edge_color[c] * edge_alpha * edge_brightness);
            }
            edge_plate[i + 3] = src[i + 3];
        }
    }

    let glow = if glow_radius > 0 && glow_strength > f32::EPSILON {
        Some(box_blur_rgba(&edge_plate, width, height, glow_radius))
    } else {
        None
    };

    let mut out = src.to_vec();
    for i in (0..src.len()).step_by(4) {
        let base = [
            src[i] as f32 / 255.0,
            src[i + 1] as f32 / 255.0,
            src[i + 2] as f32 / 255.0,
        ];
        for c in 0..3 {
            let background = base[c] * background_amount;
            let core = edge_plate[i + c] as f32 / 255.0;
            let glow_add = glow
                .as_ref()
                .map(|glow| glow[i + c] as f32 / 255.0 * glow_strength)
                .unwrap_or(0.0);
            let light = (core + glow_add).clamp(0.0, 1.0);
            let target = 1.0 - (1.0 - background) * (1.0 - light);
            out[i + c] = to_u8(lerp_f32(base[c], target, strength));
        }
    }
    out
}

fn glowing_edges_gate(edge: f32, threshold: f32, softness: f32) -> f32 {
    if softness <= f32::EPSILON {
        if edge >= threshold { 1.0 } else { 0.0 }
    } else {
        smoothstep(threshold, (threshold + softness).min(1.0), edge)
    }
}

fn apply_oil_paint(src: &[u8], width: usize, height: usize, params: OilPaintParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || strength <= f32::EPSILON {
        return src.to_vec();
    }
    let radius = params.radius_px.round().clamp(1.0, 12.0) as usize;
    let saturation = params.saturation.clamp(-1.0, 1.0);
    let contrast = params.contrast.clamp(-1.0, 1.0);
    let integrals = build_oil_paint_integrals(src, width, height);

    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let mut target = oil_paint_best_region_color(&integrals, width, height, x, y, radius)
                .unwrap_or(base);
            target = adjust_saturation(target, 1.0 + saturation);
            for c in &mut target {
                *c = ((*c - 0.5) * (1.0 + contrast * 1.25) + 0.5).clamp(0.0, 1.0);
            }
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], to_u8(target[c]), strength);
            }
        }
    }
    out
}

struct OilPaintIntegrals {
    stride: usize,
    count: Vec<f32>,
    r: Vec<f32>,
    g: Vec<f32>,
    b: Vec<f32>,
    luma: Vec<f32>,
    luma_sq: Vec<f32>,
}

fn build_oil_paint_integrals(src: &[u8], width: usize, height: usize) -> OilPaintIntegrals {
    let stride = width + 1;
    let len = stride * (height + 1);
    let mut integrals = OilPaintIntegrals {
        stride,
        count: vec![0.0; len],
        r: vec![0.0; len],
        g: vec![0.0; len],
        b: vec![0.0; len],
        luma: vec![0.0; len],
        luma_sq: vec![0.0; len],
    };
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let alpha = src[i + 3];
            let (count, r, g, b, luma, luma_sq) = if alpha == 0 {
                (0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
            } else {
                let r = src[i] as f32 / 255.0;
                let g = src[i + 1] as f32 / 255.0;
                let b = src[i + 2] as f32 / 255.0;
                let luma = luma01(r, g, b);
                (1.0, r, g, b, luma, luma * luma)
            };
            let dst = (y + 1) * stride + x + 1;
            let left = (y + 1) * stride + x;
            let top = y * stride + x + 1;
            let top_left = y * stride + x;
            integrals.count[dst] =
                count + integrals.count[left] + integrals.count[top] - integrals.count[top_left];
            integrals.r[dst] = r + integrals.r[left] + integrals.r[top] - integrals.r[top_left];
            integrals.g[dst] = g + integrals.g[left] + integrals.g[top] - integrals.g[top_left];
            integrals.b[dst] = b + integrals.b[left] + integrals.b[top] - integrals.b[top_left];
            integrals.luma[dst] =
                luma + integrals.luma[left] + integrals.luma[top] - integrals.luma[top_left];
            integrals.luma_sq[dst] = luma_sq + integrals.luma_sq[left] + integrals.luma_sq[top]
                - integrals.luma_sq[top_left];
        }
    }
    integrals
}

fn oil_paint_best_region_color(
    integrals: &OilPaintIntegrals,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    radius: usize,
) -> Option<[f32; 3]> {
    let r = radius as isize;
    let x = x as isize;
    let y = y as isize;
    let regions = [
        (x - r, x, y - r, y),
        (x, x + r, y - r, y),
        (x - r, x, y, y + r),
        (x, x + r, y, y + r),
    ];
    let mut best_color = None;
    let mut best_var = f32::MAX;
    for &(x0, x1, y0, y1) in &regions {
        let stats = oil_paint_region_stats(integrals, width, height, x0, x1, y0, y1);
        let Some((mean_rgb, variance)) = stats else {
            continue;
        };
        if variance < best_var {
            best_var = variance;
            best_color = Some(mean_rgb);
        }
    }
    best_color
}

fn oil_paint_region_stats(
    integrals: &OilPaintIntegrals,
    width: usize,
    height: usize,
    x0: isize,
    x1: isize,
    y0: isize,
    y1: isize,
) -> Option<([f32; 3], f32)> {
    let x0 = x0.clamp(0, width.saturating_sub(1) as isize) as usize;
    let x1 = x1.clamp(0, width.saturating_sub(1) as isize) as usize;
    let y0 = y0.clamp(0, height.saturating_sub(1) as isize) as usize;
    let y1 = y1.clamp(0, height.saturating_sub(1) as isize) as usize;
    if x1 < x0 || y1 < y0 {
        return None;
    }
    let count = oil_paint_integral_sum(&integrals.count, integrals.stride, x0, x1, y0, y1);
    if count <= f32::EPSILON {
        return None;
    }
    let sum_luma = oil_paint_integral_sum(&integrals.luma, integrals.stride, x0, x1, y0, y1);
    let sum_luma_sq = oil_paint_integral_sum(&integrals.luma_sq, integrals.stride, x0, x1, y0, y1);
    let mean_luma = sum_luma / count;
    let variance = (sum_luma_sq / count - mean_luma * mean_luma).max(0.0);
    Some((
        [
            oil_paint_integral_sum(&integrals.r, integrals.stride, x0, x1, y0, y1) / count,
            oil_paint_integral_sum(&integrals.g, integrals.stride, x0, x1, y0, y1) / count,
            oil_paint_integral_sum(&integrals.b, integrals.stride, x0, x1, y0, y1) / count,
        ],
        variance,
    ))
}

fn oil_paint_integral_sum(
    integral: &[f32],
    stride: usize,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
) -> f32 {
    let ax = x0;
    let ay = y0;
    let bx = x1 + 1;
    let by = y1 + 1;
    integral[by * stride + bx] - integral[ay * stride + bx] - integral[by * stride + ax]
        + integral[ay * stride + ax]
}

fn apply_artistic_media(
    src: &[u8],
    width: usize,
    height: usize,
    params: ArtisticMediaParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || strength <= f32::EPSILON {
        return src.to_vec();
    }
    let radius = params.radius_px.round().clamp(0.0, 48.0) as usize;
    let edge_strength = params.edge_strength.clamp(0.0, 1.0);
    let texture = params.texture.clamp(0.0, 1.0);
    let color_amount = params.color_amount.clamp(0.0, 1.0);
    let smooth = if radius == 0 {
        src.to_vec()
    } else {
        box_blur_rgba(src, width, height, radius)
    };

    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let soft = [
                smooth[i] as f32 / 255.0,
                smooth[i + 1] as f32 / 255.0,
                smooth[i + 2] as f32 / 255.0,
            ];
            let luma = luma01(base[0], base[1], base[2]);
            let edge = luma_edge_strength(src, width, height, x, y);
            let paper = signed_noise((x / 2) as u32, (y / 2) as u32, params.seed);
            let target = match params.mode {
                ArtisticMediaMode::Watercolor => artistic_watercolor(
                    base,
                    soft,
                    edge,
                    paper,
                    edge_strength,
                    texture,
                    color_amount,
                ),
                ArtisticMediaMode::ColoredPencil => artistic_colored_pencil(
                    base,
                    soft,
                    luma,
                    edge,
                    x,
                    y,
                    params.seed,
                    edge_strength,
                    texture,
                    color_amount,
                ),
                ArtisticMediaMode::PencilSketch => artistic_pencil_sketch(
                    base,
                    luma,
                    edge,
                    x,
                    y,
                    params.seed,
                    edge_strength,
                    texture,
                    color_amount,
                ),
            };
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], to_u8(target[c]), strength);
            }
        }
    }
    out
}

fn artistic_watercolor(
    base: [f32; 3],
    soft: [f32; 3],
    edge: f32,
    paper: f32,
    edge_strength: f32,
    texture: f32,
    color_amount: f32,
) -> [f32; 3] {
    let mut wash = [
        lerp_f32(soft[0], base[0], 0.22),
        lerp_f32(soft[1], base[1], 0.22),
        lerp_f32(soft[2], base[2], 0.22),
    ];
    wash = adjust_saturation(wash, 0.78 + color_amount * 0.52);
    let edge_darken = edge * edge_strength * 0.42;
    let paper_delta = paper * texture * 0.055;
    for c in &mut wash {
        let lifted = *c + (1.0 - *c) * 0.08;
        *c = (lifted * (1.0 - edge_darken) + paper_delta).clamp(0.0, 1.0);
        *c = quantize_unit(*c, 18.0);
    }
    wash
}

fn artistic_colored_pencil(
    base: [f32; 3],
    soft: [f32; 3],
    luma: f32,
    edge: f32,
    x: usize,
    y: usize,
    seed: u32,
    edge_strength: f32,
    texture: f32,
    color_amount: f32,
) -> [f32; 3] {
    let mut color = [
        lerp_f32(base[0], soft[0], 0.35),
        lerp_f32(base[1], soft[1], 0.35),
        lerp_f32(base[2], soft[2], 0.35),
    ];
    color = adjust_saturation(color, 0.85 + color_amount * 0.55);
    let hatch = pencil_hatch(x, y, seed);
    let shade = ((1.0 - luma) * 0.35 + edge * edge_strength).clamp(0.0, 1.0);
    let paper = signed_noise(x as u32, y as u32, seed ^ 0xC0A7_EA11) * texture * 0.045;
    for c in &mut color {
        let lifted = *c + (1.0 - *c) * 0.10;
        *c = (lifted * (1.0 - hatch * shade * texture * 0.65) + paper).clamp(0.0, 1.0);
        *c = quantize_unit(*c, 24.0);
    }
    color
}

fn artistic_pencil_sketch(
    base: [f32; 3],
    luma: f32,
    edge: f32,
    x: usize,
    y: usize,
    seed: u32,
    edge_strength: f32,
    texture: f32,
    color_amount: f32,
) -> [f32; 3] {
    let hatch = pencil_hatch(x, y, seed);
    let paper = 0.94 + signed_noise((x / 2) as u32, (y / 2) as u32, seed) * texture * 0.055;
    let line =
        (edge * edge_strength * 1.7 + hatch * (1.0 - luma) * texture * 0.72).clamp(0.0, 0.92);
    let gray = (paper * (1.0 - line)).clamp(0.0, 1.0);
    let color = adjust_saturation(base, 0.25);
    [
        lerp_f32(gray, color[0], color_amount * 0.35),
        lerp_f32(gray, color[1], color_amount * 0.35),
        lerp_f32(gray, color[2], color_amount * 0.35),
    ]
}

fn pencil_hatch(x: usize, y: usize, seed: u32) -> f32 {
    let phase = (seed % 11) as usize;
    let a = ((x + y * 2 + phase) % 9) as f32 / 8.0;
    let b = ((x * 2 + y + phase * 3) % 13) as f32 / 12.0;
    (smoothstep(0.0, 0.32, 1.0 - a) * 0.65 + smoothstep(0.0, 0.22, 1.0 - b) * 0.35).clamp(0.0, 1.0)
}

fn quantize_unit(v: f32, levels: f32) -> f32 {
    let steps = (levels.max(2.0) - 1.0).max(1.0);
    (v.clamp(0.0, 1.0) * steps).round() / steps
}

fn soft_quantize_unit(v: f32, levels: f32, softness: f32) -> f32 {
    let steps = (levels.max(2.0) - 1.0).max(1.0);
    let scaled = v.clamp(0.0, 1.0) * steps;
    if softness <= f32::EPSILON {
        return scaled.round() / steps;
    }
    let lower = scaled.floor().min(steps);
    if lower >= steps {
        return 1.0;
    }
    let upper = (lower + 1.0).min(steps);
    let frac = scaled - lower;
    let half_width = (softness.clamp(0.0, 1.0) * 0.5).clamp(0.001, 0.49);
    let t = smoothstep(0.5 - half_width, 0.5 + half_width, frac);
    (lower + (upper - lower) * t) / steps
}

fn apply_brush_stroke(
    src: &[u8],
    width: usize,
    height: usize,
    params: BrushStrokeParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || strength <= f32::EPSILON {
        return src.to_vec();
    }
    let length = params.length_px.clamp(0.0, 96.0);
    let radius = params.radius_px.clamp(0.0, 16.0);
    if length <= 0.5 && radius <= 0.5 {
        return src.to_vec();
    }
    let angle = params.angle_degrees.to_radians();
    let dir = (angle.cos(), angle.sin());
    let perp = (-dir.1, dir.0);
    let texture = params.texture.clamp(0.0, 1.0);
    let edge_strength = params.edge_strength.clamp(0.0, 1.0);
    let color_amount = params.color_amount.clamp(0.0, 1.0);
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let stroke = brush_stroke_average(
                src,
                width,
                height,
                x,
                y,
                dir,
                perp,
                length,
                radius,
                params.seed,
            );
            let edge = luma_edge_strength(src, width, height, x, y);
            let luma = luma01(base[0], base[1], base[2]);
            let target = match params.mode {
                BrushStrokeMode::DryBrush => brush_stroke_dry(
                    base,
                    stroke,
                    x,
                    y,
                    params.seed,
                    texture,
                    edge,
                    edge_strength,
                    color_amount,
                ),
                BrushStrokeMode::PaintDaubs => brush_stroke_paint(
                    base,
                    stroke,
                    x,
                    y,
                    params.seed,
                    texture,
                    edge,
                    edge_strength,
                    color_amount,
                ),
                BrushStrokeMode::PaletteKnife => brush_stroke_palette_knife(
                    base,
                    stroke,
                    luma,
                    x,
                    y,
                    params.seed,
                    texture,
                    edge,
                    edge_strength,
                    color_amount,
                ),
            };
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], to_u8(target[c]), strength);
            }
        }
    }
    out
}

fn brush_stroke_average(
    src: &[u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    dir: (f32, f32),
    perp: (f32, f32),
    length: f32,
    radius: f32,
    seed: u32,
) -> [f32; 3] {
    let samples = ((length / 2.0).ceil() as usize + 1).clamp(3, 49);
    let mut sum = [0.0_f32; 3];
    let mut weight_sum = 0.0_f32;
    for sample in 0..samples {
        let t = if samples <= 1 {
            0.0
        } else {
            sample as f32 / (samples - 1) as f32 - 0.5
        };
        let taper = 1.0 - (t.abs() * 2.0).powf(1.35) * 0.45;
        let jitter = signed_noise(
            (x as u32).wrapping_add(sample as u32),
            (y as u32).wrapping_add((sample as u32).rotate_left(5)),
            seed ^ 0x7A13_D91D,
        ) * radius;
        let sx = x as f32 + dir.0 * t * length + perp.0 * jitter;
        let sy = y as f32 + dir.1 * t * length + perp.1 * jitter;
        let rgb = sample_rgb_bilinear(src, width, height, sx, sy);
        for c in 0..3 {
            sum[c] += rgb[c] * taper;
        }
        weight_sum += taper;
    }
    if weight_sum <= f32::EPSILON {
        return sample_rgb_bilinear(src, width, height, x as f32, y as f32);
    }
    [
        sum[0] / weight_sum,
        sum[1] / weight_sum,
        sum[2] / weight_sum,
    ]
}

fn brush_stroke_dry(
    base: [f32; 3],
    stroke: [f32; 3],
    x: usize,
    y: usize,
    seed: u32,
    texture: f32,
    edge: f32,
    edge_strength: f32,
    color_amount: f32,
) -> [f32; 3] {
    let grain = signed_noise(x as u32, y as u32, seed ^ 0xD20B_1A55);
    let skip = smoothstep(-0.45, 0.75, grain) * texture;
    let mut color = [
        lerp_f32(stroke[0], base[0], 0.35 + skip * 0.25),
        lerp_f32(stroke[1], base[1], 0.35 + skip * 0.25),
        lerp_f32(stroke[2], base[2], 0.35 + skip * 0.25),
    ];
    color = adjust_saturation(color, 0.78 + color_amount * 0.52);
    let dry = (grain * texture * 0.10 - edge * edge_strength * 0.36).clamp(-0.2, 0.2);
    for c in &mut color {
        *c = quantize_unit((*c + dry).clamp(0.0, 1.0), 20.0);
    }
    color
}

fn brush_stroke_paint(
    base: [f32; 3],
    stroke: [f32; 3],
    x: usize,
    y: usize,
    seed: u32,
    texture: f32,
    edge: f32,
    edge_strength: f32,
    color_amount: f32,
) -> [f32; 3] {
    let daub = signed_noise((x / 2) as u32, (y / 2) as u32, seed ^ 0xA1CE_B00C);
    let mut color = [
        lerp_f32(base[0], stroke[0], 0.72),
        lerp_f32(base[1], stroke[1], 0.72),
        lerp_f32(base[2], stroke[2], 0.72),
    ];
    color = adjust_saturation(color, 0.92 + color_amount * 0.55);
    let impasto = (daub * texture * 0.08 + edge * edge_strength * 0.10).clamp(-0.16, 0.18);
    for c in &mut color {
        *c = quantize_unit((*c + impasto).clamp(0.0, 1.0), 16.0);
    }
    color
}

fn brush_stroke_palette_knife(
    base: [f32; 3],
    stroke: [f32; 3],
    luma: f32,
    x: usize,
    y: usize,
    seed: u32,
    texture: f32,
    edge: f32,
    edge_strength: f32,
    color_amount: f32,
) -> [f32; 3] {
    let scrape = signed_noise((x / 3) as u32, y as u32, seed ^ 0x51AB_1E7D);
    let mut color = [
        lerp_f32(base[0], stroke[0], 0.86),
        lerp_f32(base[1], stroke[1], 0.86),
        lerp_f32(base[2], stroke[2], 0.86),
    ];
    color = adjust_saturation(color, 0.75 + color_amount * 0.65);
    let ridge = (scrape.signum() * scrape.abs().powf(0.65) * texture * 0.11)
        + edge * edge_strength * (0.14 - luma * 0.18);
    for c in &mut color {
        let contrast = (*c - 0.5) * (1.0 + edge_strength * 0.35) + 0.5;
        *c = quantize_unit((contrast + ridge).clamp(0.0, 1.0), 10.0);
    }
    color
}

fn apply_cutout(src: &[u8], width: usize, height: usize, params: CutoutParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || strength <= f32::EPSILON {
        return src.to_vec();
    }
    let levels = params.levels.clamp(2, 12) as f32;
    let radius = params.radius_px.round().clamp(0.0, 32.0) as usize;
    let edge_strength = params.edge_strength.clamp(0.0, 1.0);
    let color_amount = params.color_amount.clamp(0.0, 1.0);
    let smooth = if radius == 0 {
        src.to_vec()
    } else {
        box_blur_rgba(src, width, height, radius)
    };

    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let soft = [
                smooth[i] as f32 / 255.0,
                smooth[i + 1] as f32 / 255.0,
                smooth[i + 2] as f32 / 255.0,
            ];
            let (h, s, l) = rgb_to_hsl(soft[0], soft[1], soft[2]);
            let l = quantize_unit(l, levels);
            let sat_levels = (levels * 0.7 + 2.0).clamp(3.0, 10.0);
            let s = quantize_unit(s * (0.65 + color_amount * 0.55), sat_levels).clamp(0.0, 1.0);
            let hue_steps = (levels * 4.0).clamp(8.0, 36.0);
            let h = if s <= 0.035 {
                h
            } else {
                ((h * hue_steps).round() / hue_steps).rem_euclid(1.0)
            };
            let mut target = hsl_to_rgb(h, s, l);
            target = adjust_saturation(target, 0.8 + color_amount * 0.35);

            let edge = luma_edge_strength(src, width, height, x, y);
            let edge_darken = edge * edge_strength * 0.58;
            for c in 0..3 {
                target[c] = (target[c] * (1.0 - edge_darken)).clamp(0.0, 1.0);
                out[i + c] = lerp_u8(src[i + c], to_u8(target[c]), strength);
            }
        }
    }
    out
}

fn apply_toon_shade(src: &[u8], width: usize, height: usize, params: ToonShadeParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || strength <= f32::EPSILON {
        return src.to_vec();
    }

    let bands = params.bands.clamp(2, 8) as f32;
    let softness = params.softness.clamp(0.0, 1.0);
    let shadow_tint = rgb_u8_to_f32(params.shadow_tint_rgb);
    let shadow_strength = params.shadow_tint_strength.clamp(0.0, 1.0);
    let light_tint = rgb_u8_to_f32(params.light_tint_rgb);
    let light_strength = params.light_tint_strength.clamp(0.0, 1.0);
    let outline_strength = params.outline_strength.clamp(0.0, 1.0);
    let mut band_lightness = vec![0.0; width.saturating_mul(height)];

    for (idx, px) in src.chunks_exact(4).enumerate() {
        if px[3] == 0 {
            continue;
        }
        let (_, _, lightness) = rgb_to_hsl(
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        );
        band_lightness[idx] = soft_quantize_unit(lightness, bands, softness);
    }

    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let i = idx * 4;
            if src[i + 3] == 0 {
                continue;
            }
            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let band = band_lightness[idx];
            let mut target = if params.preserve_hue {
                let (h, s, _) = rgb_to_hsl(base[0], base[1], base[2]);
                hsl_to_rgb(h, s, band)
            } else {
                [
                    soft_quantize_unit(base[0], bands, softness),
                    soft_quantize_unit(base[1], bands, softness),
                    soft_quantize_unit(base[2], bands, softness),
                ]
            };

            let shadow_mix = shadow_strength * (1.0 - band).powf(1.25);
            let light_mix = light_strength * band.powf(1.35);
            for c in 0..3 {
                target[c] = lerp_f32(target[c], shadow_tint[c], shadow_mix);
                target[c] = lerp_f32(target[c], light_tint[c], light_mix);
            }

            let edge = toon_band_edge_signal(&band_lightness, width, height, x, y, bands);
            let darken = edge * outline_strength * 0.65;
            for c in 0..3 {
                target[c] = (target[c] * (1.0 - darken)).clamp(0.0, 1.0);
                out[i + c] = lerp_u8(src[i + c], to_u8(target[c]), strength);
            }
        }
    }
    out
}

fn toon_band_edge_signal(
    band_lightness: &[f32],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    bands: f32,
) -> f32 {
    let idx = y * width + x;
    let center = band_lightness[idx];
    let left = band_lightness[y * width + x.saturating_sub(1)];
    let right = band_lightness[y * width + (x + 1).min(width - 1)];
    let top = band_lightness[y.saturating_sub(1) * width + x];
    let bottom = band_lightness[(y + 1).min(height - 1) * width + x];
    let delta = (center - left)
        .abs()
        .max((center - right).abs())
        .max((center - top).abs())
        .max((center - bottom).abs());
    (delta * (bands - 1.0)).clamp(0.0, 1.0)
}

fn apply_emboss(src: &[u8], width: usize, height: usize, params: EmbossParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let depth = params.depth.clamp(0.0, 4.0);
    if width == 0 || height == 0 || strength <= f32::EPSILON || depth <= f32::EPSILON {
        return src.to_vec();
    }
    let angle = params.angle_degrees.to_radians();
    let dir = (angle.cos(), angle.sin());
    let contrast = params.contrast.clamp(-1.0, 1.0);
    let color_amount = params.color_amount.clamp(0.0, 1.0);

    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let luma = luma01(base[0], base[1], base[2]);
            let (gx, gy) = emboss_luma_gradient(src, width, height, x, y);
            let slope = (gx * dir.0 + gy * dir.1) * depth;
            let mut relief = (0.5 + slope * 0.58).clamp(0.0, 1.0);
            relief = ((relief - 0.5) * (1.0 + contrast * 1.25) + 0.5).clamp(0.0, 1.0);
            let gray = [relief, relief, relief];
            let scale = relief / luma.max(0.05);
            let tinted = [
                (base[0] * scale).clamp(0.0, 1.0),
                (base[1] * scale).clamp(0.0, 1.0),
                (base[2] * scale).clamp(0.0, 1.0),
            ];
            let target = [
                lerp_f32(gray[0], tinted[0], color_amount),
                lerp_f32(gray[1], tinted[1], color_amount),
                lerp_f32(gray[2], tinted[2], color_amount),
            ];
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], to_u8(target[c]), strength);
            }
        }
    }
    out
}

fn emboss_luma_gradient(src: &[u8], width: usize, height: usize, x: usize, y: usize) -> (f32, f32) {
    let x = x as isize;
    let y = y as isize;
    let sample = |xx: isize, yy: isize| emboss_luma_at(src, width, height, xx, yy);
    let gx = -sample(x - 1, y - 1) + sample(x + 1, y - 1) - 2.0 * sample(x - 1, y)
        + 2.0 * sample(x + 1, y)
        - sample(x - 1, y + 1)
        + sample(x + 1, y + 1);
    let gy = -sample(x - 1, y - 1) - 2.0 * sample(x, y - 1) - sample(x + 1, y - 1)
        + sample(x - 1, y + 1)
        + 2.0 * sample(x, y + 1)
        + sample(x + 1, y + 1);
    (gx * 0.25, gy * 0.25)
}

fn emboss_luma_at(src: &[u8], width: usize, height: usize, x: isize, y: isize) -> f32 {
    let x = x.clamp(0, width.saturating_sub(1) as isize) as usize;
    let y = y.clamp(0, height.saturating_sub(1) as isize) as usize;
    let i = (y * width + x) * 4;
    luma01(
        src[i] as f32 / 255.0,
        src[i + 1] as f32 / 255.0,
        src[i + 2] as f32 / 255.0,
    )
}

fn apply_pixel_stylize(
    src: &[u8],
    width: usize,
    height: usize,
    params: PixelStylizeParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || strength <= f32::EPSILON {
        return src.to_vec();
    }
    let cell = params.cell_px.clamp(1.0, 80.0);
    let edge_strength = params.edge_strength.clamp(0.0, 1.0);
    let color_amount = params.color_amount.clamp(0.0, 1.0);
    let randomness = params.randomness.clamp(0.0, 1.0);

    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let target = match params.mode {
                PixelStylizeMode::Crystallize => pixel_stylize_crystallize(
                    src,
                    width,
                    height,
                    x,
                    y,
                    cell.max(2.0),
                    edge_strength,
                    color_amount,
                    randomness,
                    params.seed,
                    base,
                ),
                PixelStylizeMode::Pointillize => pixel_stylize_pointillize(
                    src,
                    width,
                    height,
                    x,
                    y,
                    cell.max(2.0),
                    edge_strength,
                    color_amount,
                    randomness,
                    params.seed,
                    base,
                ),
                PixelStylizeMode::Facet => pixel_stylize_facet(
                    src,
                    width,
                    height,
                    x,
                    y,
                    cell.max(2.0),
                    edge_strength,
                    color_amount,
                    randomness,
                    params.seed,
                    base,
                ),
                PixelStylizeMode::Mezzotint => pixel_stylize_mezzotint(
                    src,
                    width,
                    height,
                    x,
                    y,
                    cell,
                    edge_strength,
                    color_amount,
                    randomness,
                    params.seed,
                    base,
                ),
            };
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], to_u8(target[c]), strength);
            }
        }
    }
    out
}

fn pixel_stylize_crystallize(
    src: &[u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    cell: f32,
    edge_strength: f32,
    color_amount: f32,
    randomness: f32,
    seed: u32,
    base: [f32; 3],
) -> [f32; 3] {
    let (cx, cy, best_dist, second_dist) =
        pixel_nearest_cell_center(x, y, cell, width, height, randomness, seed);
    let sampled = sample_rgb_bilinear(src, width, height, cx, cy);
    let mut target = adjust_saturation(
        [
            lerp_f32(base[0], sampled[0], 0.70 + color_amount * 0.30),
            lerp_f32(base[1], sampled[1], 0.70 + color_amount * 0.30),
            lerp_f32(base[2], sampled[2], 0.70 + color_amount * 0.30),
        ],
        0.72 + color_amount * 0.58,
    );
    let gap = (second_dist.sqrt() - best_dist.sqrt()).max(0.0);
    let boundary = 1.0 - smoothstep(0.0, cell * 0.18, gap);
    let source_edge = luma_edge_strength(src, width, height, x, y);
    let darken = (boundary * 0.34 + source_edge * 0.10) * edge_strength;
    for c in &mut target {
        *c = (*c * (1.0 - darken)).clamp(0.0, 1.0);
    }
    target
}

fn pixel_stylize_pointillize(
    src: &[u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    cell: f32,
    edge_strength: f32,
    color_amount: f32,
    randomness: f32,
    seed: u32,
    base: [f32; 3],
) -> [f32; 3] {
    let (cx, cy, best_dist, _) =
        pixel_nearest_cell_center(x, y, cell, width, height, randomness, seed ^ 0x0D07_51A7);
    let mut ink = sample_rgb_bilinear(src, width, height, cx, cy);
    ink = adjust_saturation(ink, 0.86 + color_amount * 0.64);
    let radius = (cell * (0.30 + randomness * 0.20)).max(0.5);
    let dot = 1.0 - smoothstep(0.72, 1.08, best_dist.sqrt() / radius);
    let paper_noise = signed_noise((x / 2) as u32, (y / 2) as u32, seed ^ 0xFADE_1201)
        * (0.012 + randomness * 0.026);
    let paper = [
        lerp_f32(0.93 + paper_noise, base[0], 0.18 + color_amount * 0.20),
        lerp_f32(0.93 + paper_noise, base[1], 0.18 + color_amount * 0.20),
        lerp_f32(0.93 + paper_noise, base[2], 0.18 + color_amount * 0.20),
    ];
    let edge = luma_edge_strength(src, width, height, x, y) * edge_strength * 0.18;
    [
        lerp_f32(paper[0], ink[0] * (1.0 - edge), dot).clamp(0.0, 1.0),
        lerp_f32(paper[1], ink[1] * (1.0 - edge), dot).clamp(0.0, 1.0),
        lerp_f32(paper[2], ink[2] * (1.0 - edge), dot).clamp(0.0, 1.0),
    ]
}

fn pixel_stylize_facet(
    src: &[u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    cell: f32,
    edge_strength: f32,
    color_amount: f32,
    randomness: f32,
    seed: u32,
    base: [f32; 3],
) -> [f32; 3] {
    let fx = x as f32 + 0.5;
    let fy = y as f32 + 0.5;
    let cell_x = (fx / cell).floor() as i32;
    let cell_y = (fy / cell).floor() as i32;
    let local_x = (fx / cell - cell_x as f32).clamp(0.0, 1.0);
    let local_y = (fy / cell - cell_y as f32).clamp(0.0, 1.0);
    let flip = signed_noise(cell_x as u32, cell_y as u32, seed ^ 0xFACE_7A1E) > 0.0;
    let upper = if flip {
        local_x + local_y < 1.0
    } else {
        local_x > local_y
    };
    let (tri_x, tri_y) = match (flip, upper) {
        (true, true) => (0.34, 0.34),
        (true, false) => (0.66, 0.66),
        (false, true) => (0.68, 0.32),
        (false, false) => (0.32, 0.68),
    };
    let jitter_x =
        signed_noise(cell_x as u32, cell_y as u32, seed ^ 0xA11C_E551) * randomness * 0.11;
    let jitter_y =
        signed_noise(cell_x as u32, cell_y as u32, seed ^ 0x51C0_1A7E) * randomness * 0.11;
    let sx = (cell_x as f32 * cell + (tri_x + jitter_x) * cell)
        .clamp(0.0, width.saturating_sub(1) as f32);
    let sy = (cell_y as f32 * cell + (tri_y + jitter_y) * cell)
        .clamp(0.0, height.saturating_sub(1) as f32);
    let sampled = sample_rgb_bilinear(src, width, height, sx, sy);
    let mut target = adjust_saturation(
        [
            lerp_f32(base[0], sampled[0], 0.74 + color_amount * 0.18),
            lerp_f32(base[1], sampled[1], 0.74 + color_amount * 0.18),
            lerp_f32(base[2], sampled[2], 0.74 + color_amount * 0.18),
        ],
        0.78 + color_amount * 0.46,
    );

    let diagonal = if flip {
        (local_x + local_y - 1.0).abs() / 2.0_f32.sqrt()
    } else {
        (local_x - local_y).abs() / 2.0_f32.sqrt()
    };
    let border = local_x.min(1.0 - local_x).min(local_y).min(1.0 - local_y);
    let line = (1.0 - smoothstep(0.0, 0.055, diagonal)).max(1.0 - smoothstep(0.0, 0.045, border));
    let shade = signed_noise(cell_x as u32, cell_y as u32, seed ^ 0xC011_A9E5) * 0.035;
    for c in &mut target {
        *c = (*c + shade - line * edge_strength * 0.24).clamp(0.0, 1.0);
    }
    target
}

fn pixel_stylize_mezzotint(
    src: &[u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    cell: f32,
    edge_strength: f32,
    color_amount: f32,
    randomness: f32,
    seed: u32,
    base: [f32; 3],
) -> [f32; 3] {
    let grain = cell.round().clamp(1.0, 12.0) as usize;
    let gx = (x / grain) as u32;
    let gy = (y / grain) as u32;
    let coarse = signed_noise(gx, gy, seed ^ 0x0E22_071D);
    let fine = signed_noise(x as u32, y as u32, seed ^ 0x7111_9A1D);
    let noise = ((coarse * (0.54 + randomness * 0.20) + fine * (0.26 + randomness * 0.24))
        / (0.80 + randomness * 0.44)
        * 0.5
        + 0.5)
        .clamp(0.0, 1.0);
    let luma = luma01(base[0], base[1], base[2]).clamp(0.0, 1.0);
    let softness = (0.018 + (1.0 - randomness) * 0.085).clamp(0.01, 0.12);
    let white = 1.0 - smoothstep(luma - softness, luma + softness, noise);
    let gray = lerp_f32(0.07, 0.94, white);
    let scale = gray / luma.max(0.06);
    let tinted = [
        (base[0] * scale).clamp(0.0, 1.0),
        (base[1] * scale).clamp(0.0, 1.0),
        (base[2] * scale).clamp(0.0, 1.0),
    ];
    let edge = luma_edge_strength(src, width, height, x, y) * edge_strength * 0.26;
    [
        lerp_f32(gray, tinted[0], color_amount) * (1.0 - edge),
        lerp_f32(gray, tinted[1], color_amount) * (1.0 - edge),
        lerp_f32(gray, tinted[2], color_amount) * (1.0 - edge),
    ]
}

fn pixel_nearest_cell_center(
    x: usize,
    y: usize,
    cell: f32,
    width: usize,
    height: usize,
    randomness: f32,
    seed: u32,
) -> (f32, f32, f32, f32) {
    let fx = x as f32 + 0.5;
    let fy = y as f32 + 0.5;
    let cell_x = (fx / cell).floor() as i32;
    let cell_y = (fy / cell).floor() as i32;
    let max_cell_x = (width.saturating_sub(1) as f32 / cell).floor() as i32;
    let max_cell_y = (height.saturating_sub(1) as f32 / cell).floor() as i32;
    let mut best = (0.0, 0.0, f32::MAX);
    let mut second_dist = f32::MAX;
    for cy in cell_y - 1..=cell_y + 1 {
        if cy < 0 || cy > max_cell_y {
            continue;
        }
        for cx in cell_x - 1..=cell_x + 1 {
            if cx < 0 || cx > max_cell_x {
                continue;
            }
            let (center_x, center_y) =
                pixel_cell_center(cx, cy, cell, width, height, randomness, seed);
            let dx = fx - center_x;
            let dy = fy - center_y;
            let dist = dx * dx + dy * dy;
            if dist < best.2 {
                second_dist = best.2;
                best = (center_x, center_y, dist);
            } else {
                second_dist = second_dist.min(dist);
            }
        }
    }
    if second_dist == f32::MAX {
        second_dist = best.2 + cell * cell;
    }
    (best.0, best.1, best.2, second_dist)
}

fn pixel_cell_center(
    cell_x: i32,
    cell_y: i32,
    cell: f32,
    width: usize,
    height: usize,
    randomness: f32,
    seed: u32,
) -> (f32, f32) {
    let jitter = (randomness * 0.42).clamp(0.0, 0.42);
    let jx = signed_noise(cell_x as u32, cell_y as u32, seed) * jitter;
    let jy = signed_noise(cell_x as u32, cell_y as u32, seed ^ 0x9E37_79B9) * jitter;
    let x = (cell_x as f32 + 0.5 + jx) * cell;
    let y = (cell_y as f32 + 0.5 + jy) * cell;
    (
        x.clamp(0.0, width.saturating_sub(1) as f32),
        y.clamp(0.0, height.saturating_sub(1) as f32),
    )
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

fn apply_orton(src: &[u8], width: usize, height: usize, params: OrtonParams) -> Vec<u8> {
    let radius = params.radius_px.round().clamp(0.0, 160.0) as usize;
    let strength = params.strength.clamp(0.0, 1.0);
    if radius == 0 || strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let brightness = params.brightness.clamp(0.0, 1.0);
    let contrast = params.contrast.clamp(-1.0, 1.0);
    let saturation = params.saturation.clamp(-1.0, 1.0);
    let mut premultiplied = vec![0_u8; src.len()];
    for i in (0..src.len()).step_by(4) {
        let alpha = src[i + 3] as f32 / 255.0;
        premultiplied[i] = to_u8(src[i] as f32 / 255.0 * alpha);
        premultiplied[i + 1] = to_u8(src[i + 1] as f32 / 255.0 * alpha);
        premultiplied[i + 2] = to_u8(src[i + 2] as f32 / 255.0 * alpha);
        premultiplied[i + 3] = src[i + 3];
    }

    let blur = box_blur_rgba(&premultiplied, width, height, radius);
    let contrast_scale = if contrast >= 0.0 {
        1.0 + contrast * 1.35
    } else {
        1.0 + contrast * 0.75
    }
    .max(0.05);
    let saturation_scale = (1.0 + saturation).max(0.0);
    let glow_scale = (0.45 + brightness * 0.70).clamp(0.0, 1.25);
    let mut out = src.to_vec();

    for i in (0..src.len()).step_by(4) {
        let base = [
            src[i] as f32 / 255.0,
            src[i + 1] as f32 / 255.0,
            src[i + 2] as f32 / 255.0,
        ];
        let blur_alpha = blur[i + 3] as f32 / 255.0;
        if blur_alpha <= 1.0 / 255.0 {
            continue;
        }
        let inv_alpha = 1.0 / blur_alpha;
        let mut glow = [
            (blur[i] as f32 / 255.0 * inv_alpha).clamp(0.0, 1.0),
            (blur[i + 1] as f32 / 255.0 * inv_alpha).clamp(0.0, 1.0),
            (blur[i + 2] as f32 / 255.0 * inv_alpha).clamp(0.0, 1.0),
        ];

        glow = adjust_saturation(glow, saturation_scale);
        for channel in &mut glow {
            *channel = ((*channel - 0.5) * contrast_scale + 0.5).clamp(0.0, 1.0);
            *channel = screen_channel(*channel, brightness * 0.45);
            *channel = (*channel * glow_scale).clamp(0.0, 1.0);
        }

        for c in 0..3 {
            let screened = screen_channel(base[c], glow[c]);
            out[i + c] = to_u8(lerp_f32(base[c], screened, strength));
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
    // 出力合成は行単位で独立しているため rayon で並列化する (final pipeline では
    // AI アップスケール後の大判画像にも UI スレッド同期で掛かるため)。
    out.par_chunks_exact_mut(width * 4)
        .enumerate()
        .for_each(|(y, row)| {
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
                    let halo_gate =
                        lerp_f32(1.0, smoothstep(0.02, 0.42, headroom), halo_suppression);
                    row[x * 4 + c] = (base + detail * amount * edge_weight * halo_gate)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
            }
        });
    out
}

/// 最終表示段 (mIV final pipeline) 用スマートシャープの公開 API。
///
/// 入力は unmultiplied RGBA8。alpha チャンネルは変更しない (RGB のみ強調)。
/// `radius_px` は内部で 0..=3.0 に clamp する — final pipeline は AI アップスケール後の
/// 大判画像にも同期で掛かるため、巨大半径による UI ブロックを構造的に防ぐ。
/// 各パラメータの clamp 値は `LocalEffect::SmartSharpen` 経路と同一。
pub fn apply_smart_sharpen_rgba(
    src: &[u8],
    width: usize,
    height: usize,
    params: &SmartSharpenParams,
) -> Vec<u8> {
    apply_smart_sharpen(
        src,
        width,
        height,
        params.radius_px.clamp(0.0, 3.0).round() as usize,
        params.amount.clamp(0.0, 2.0),
        params.edge_threshold.clamp(0.0, 1.0),
        params.halo_suppression.clamp(0.0, 1.0),
    )
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

fn apply_retro_palette(
    src: &[u8],
    width: usize,
    height: usize,
    params: RetroPaletteParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if width == 0 || height == 0 || strength <= f32::EPSILON {
        return src.to_vec();
    }
    let dither = params.dither.clamp(0.0, 1.0);
    let adaptive_palette = retro_adaptive_palette_spec(params.mode)
        .map(|(colors, bit_depth)| generate_retro_adaptive_palette(src, colors, bit_depth));
    let adaptive_lut = adaptive_palette
        .as_ref()
        .map(|palette| build_retro_palette_lut(palette));
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }
            let offset = bayer4_offset(x, y);
            let rgb = [src[i], src[i + 1], src[i + 2]];
            let quantized = match (adaptive_palette.as_deref(), adaptive_lut.as_deref()) {
                (Some(palette), Some(lut)) => {
                    retro_palette_lut_rgb(rgb, palette, lut, offset, dither)
                }
                _ => retro_palette_rgb(rgb, params.mode, offset, dither),
            };
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], quantized[c], strength);
            }
        }
    }
    out
}

fn apply_crt_display(src: &[u8], width: usize, height: usize, params: CrtDisplayParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let scanline_spacing = params.scanline_spacing_px.clamp(2.0, 24.0);
    let scanline_depth = params.scanline_depth.clamp(0.0, 1.0);
    let mask_strength = params.mask_strength.clamp(0.0, 1.0);
    let curvature = params.curvature.clamp(0.0, 0.25);
    let bloom = params.bloom.clamp(0.0, 1.0);
    let horizontal_blur = params.horizontal_blur.clamp(0.0, 1.0);
    let brightness = params.brightness.clamp(0.25, 2.5);
    if scanline_depth <= f32::EPSILON
        && mask_strength <= f32::EPSILON
        && curvature <= f32::EPSILON
        && bloom <= f32::EPSILON
        && horizontal_blur <= f32::EPSILON
        && (brightness - 1.0).abs() <= f32::EPSILON
    {
        return src.to_vec();
    }

    let bloom_map = (bloom > f32::EPSILON).then(|| build_crt_bloom_map(src, width, height));
    let mut out = src.to_vec();
    let width_f = width.max(1) as f32;
    let height_f = height.max(1) as f32;
    let sample_blur_offset = (0.35 + horizontal_blur * 1.15).clamp(0.35, 1.5);
    let side_weight = horizontal_blur * 0.42;
    let center_weight = 1.0 - side_weight * 2.0;

    for y in 0..height {
        let scan_mult = crt_scanline_multiplier(y as f32 + 0.5, scanline_spacing, scanline_depth);
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }
            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let Some((sx, sy)) = crt_source_position(x, y, width_f, height_f, curvature) else {
                for c in 0..3 {
                    out[i + c] = to_u8(lerp_f32(base[c], 0.0, strength));
                }
                continue;
            };

            let mut rgb = sample_rgb_bilinear_alpha_fallback(src, width, height, sx, sy, base);
            if horizontal_blur > f32::EPSILON {
                let left = sample_rgb_bilinear_alpha_fallback(
                    src,
                    width,
                    height,
                    sx - sample_blur_offset,
                    sy,
                    rgb,
                );
                let right = sample_rgb_bilinear_alpha_fallback(
                    src,
                    width,
                    height,
                    sx + sample_blur_offset,
                    sy,
                    rgb,
                );
                for c in 0..3 {
                    rgb[c] = rgb[c] * center_weight + (left[c] + right[c]) * side_weight;
                }
            }

            let mask = crt_aperture_mask(x as f32 + 0.5, mask_strength);
            for c in 0..3 {
                rgb[c] *= mask[c] * scan_mult * brightness;
            }
            if let Some(map) = &bloom_map {
                let bx = sx.round().clamp(0.0, width.saturating_sub(1) as f32) as usize;
                let by = sy.round().clamp(0.0, height.saturating_sub(1) as f32) as usize;
                let glow = map[by * width + bx] * bloom;
                for channel in &mut rgb {
                    *channel += glow;
                }
            }

            for c in 0..3 {
                out[i + c] = to_u8(lerp_f32(base[c], rgb[c], strength));
            }
        }
    }
    out
}

fn build_crt_bloom_map(src: &[u8], width: usize, height: usize) -> Vec<f32> {
    let mut bright = vec![0.0_f32; width.saturating_mul(height)];
    for (idx, px) in src.chunks_exact(4).enumerate() {
        let alpha = px[3] as f32 / 255.0;
        if alpha <= f32::EPSILON {
            continue;
        }
        let luma = luma01(
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        );
        bright[idx] = smoothstep(0.55, 0.92, luma) * alpha;
    }
    let radius = ((width.min(height) as f32 / 180.0).round() as usize).clamp(1, 8);
    box_blur_alpha(&bright, width, height, radius)
}

fn crt_source_position(
    x: usize,
    y: usize,
    width: f32,
    height: f32,
    curvature: f32,
) -> Option<(f32, f32)> {
    if curvature <= f32::EPSILON {
        return Some((x as f32, y as f32));
    }
    let nx = ((x as f32 + 0.5) / width) * 2.0 - 1.0;
    let ny = ((y as f32 + 0.5) / height) * 2.0 - 1.0;
    let r2 = nx * nx + ny * ny;
    let k = 1.0 + r2 * curvature;
    let dx = nx * k;
    let dy = ny * k;
    if dx.abs() > 1.0 || dy.abs() > 1.0 {
        return None;
    }
    Some((
        (dx * 0.5 + 0.5) * width - 0.5,
        (dy * 0.5 + 0.5) * height - 0.5,
    ))
}

fn crt_scanline_multiplier(y: f32, spacing: f32, depth: f32) -> f32 {
    if depth <= f32::EPSILON {
        return 1.0;
    }
    let phase = (y / spacing).fract();
    let curve = (phase * std::f32::consts::PI).sin();
    1.0 - depth * (1.0 - curve * curve)
}

fn crt_aperture_mask(x: f32, strength: f32) -> [f32; 3] {
    if strength <= f32::EPSILON {
        return [1.0, 1.0, 1.0];
    }
    let phase = x / 3.0;
    let two_pi = std::f32::consts::TAU;
    [
        1.0 - strength + strength * 3.0 * (phase * two_pi).sin().max(0.0).powi(2),
        1.0 - strength + strength * 3.0 * ((phase + 1.0 / 3.0) * two_pi).sin().max(0.0).powi(2),
        1.0 - strength + strength * 3.0 * ((phase + 2.0 / 3.0) * two_pi).sin().max(0.0).powi(2),
    ]
}

const RETRO_GAMEBOY_PALETTE: [[u8; 3]; 4] = [
    [0x0F, 0x38, 0x0F],
    [0x30, 0x62, 0x30],
    [0x8B, 0xAC, 0x0F],
    [0x9B, 0xBC, 0x0F],
];

const RETRO_FAMICOM_PALETTE: &[[u8; 3]] = &[
    [0x7C, 0x7C, 0x7C],
    [0x00, 0x00, 0xFC],
    [0x00, 0x00, 0xBC],
    [0x44, 0x28, 0xBC],
    [0x94, 0x00, 0x84],
    [0xA8, 0x00, 0x20],
    [0xA8, 0x10, 0x00],
    [0x88, 0x14, 0x00],
    [0x50, 0x30, 0x00],
    [0x00, 0x78, 0x00],
    [0x00, 0x68, 0x00],
    [0x00, 0x58, 0x00],
    [0x00, 0x40, 0x58],
    [0x00, 0x00, 0x00],
    [0xBC, 0xBC, 0xBC],
    [0x00, 0x78, 0xF8],
    [0x00, 0x58, 0xF8],
    [0x68, 0x44, 0xFC],
    [0xD8, 0x00, 0xCC],
    [0xE4, 0x00, 0x58],
    [0xF8, 0x38, 0x00],
    [0xE4, 0x5C, 0x10],
    [0xAC, 0x7C, 0x00],
    [0x00, 0xB8, 0x00],
    [0x00, 0xA8, 0x00],
    [0x00, 0xA8, 0x44],
    [0x00, 0x88, 0x88],
    [0xF8, 0xF8, 0xF8],
    [0x3C, 0xBC, 0xFC],
    [0x68, 0x88, 0xFC],
    [0x98, 0x78, 0xF8],
    [0xF8, 0x78, 0xF8],
    [0xF8, 0x58, 0x98],
    [0xF8, 0x78, 0x58],
    [0xFC, 0xA0, 0x44],
    [0xF8, 0xB8, 0x00],
    [0xB8, 0xF8, 0x18],
    [0x58, 0xD8, 0x54],
    [0x58, 0xF8, 0x98],
    [0x00, 0xE8, 0xD8],
    [0x78, 0x78, 0x78],
    [0xFC, 0xFC, 0xFC],
    [0xA4, 0xE4, 0xFC],
    [0xB8, 0xB8, 0xF8],
    [0xD8, 0xB8, 0xF8],
    [0xF8, 0xB8, 0xF8],
    [0xF8, 0xA4, 0xC0],
    [0xF0, 0xD0, 0xB0],
    [0xFC, 0xE0, 0xA8],
    [0xF8, 0xD8, 0x78],
    [0xD8, 0xF8, 0x78],
    [0xB8, 0xF8, 0xB8],
    [0xB8, 0xF8, 0xD8],
];

const RETRO_BAYER4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
const RETRO_LUT_BITS: u32 = 5;
const RETRO_LUT_DIM: u32 = 1 << RETRO_LUT_BITS;
const RETRO_LUT_MAX: u32 = RETRO_LUT_DIM - 1;
const RETRO_LUT_SIZE: usize = (RETRO_LUT_DIM * RETRO_LUT_DIM * RETRO_LUT_DIM) as usize;
const RETRO_ADAPTIVE_SAMPLE_LIMIT: usize = 50_000;

static RETRO_FAMICOM_LUT: OnceLock<Vec<u8>> = OnceLock::new();

fn retro_palette_rgb(
    rgb: [u8; 3],
    mode: RetroPaletteMode,
    dither_offset: f32,
    dither: f32,
) -> [u8; 3] {
    match mode {
        RetroPaletteMode::Dither1Bit => {
            let luma = luma01(
                rgb[0] as f32 / 255.0,
                rgb[1] as f32 / 255.0,
                rgb[2] as f32 / 255.0,
            );
            let v = if luma + dither_offset * dither * 0.75 >= 0.5 {
                255
            } else {
                0
            };
            [v, v, v]
        }
        RetroPaletteMode::GameBoy => {
            let luma = luma01(
                rgb[0] as f32 / 255.0,
                rgb[1] as f32 / 255.0,
                rgb[2] as f32 / 255.0,
            );
            let idx =
                ((luma + dither_offset * dither * 0.45).clamp(0.0, 1.0) * 3.0).round() as usize;
            RETRO_GAMEBOY_PALETTE[idx.min(3)]
        }
        RetroPaletteMode::Famicom => {
            let offset = dither_offset * dither * 255.0;
            let r = (rgb[0] as f32 + offset).round().clamp(0.0, 255.0) as u32;
            let g = (rgb[1] as f32 + offset).round().clamp(0.0, 255.0) as u32;
            let b = (rgb[2] as f32 + offset).round().clamp(0.0, 255.0) as u32;
            let lut =
                RETRO_FAMICOM_LUT.get_or_init(|| build_retro_palette_lut(RETRO_FAMICOM_PALETTE));
            let idx = lut[retro_lut_index(r, g, b)] as usize;
            RETRO_FAMICOM_PALETTE[idx]
        }
        RetroPaletteMode::Msx2Plus => {
            let offset = dither_offset * dither * 255.0;
            [
                quantize_retro_channel(rgb[0], 8, offset),
                quantize_retro_channel(rgb[1], 8, offset),
                quantize_retro_channel(rgb[2], 4, offset),
            ]
        }
        RetroPaletteMode::Pc98
        | RetroPaletteMode::GameGear
        | RetroPaletteMode::MegaDrive
        | RetroPaletteMode::Sfc => rgb,
    }
}

fn quantize_retro_channel(channel: u8, levels: u32, offset: f32) -> u8 {
    let max_level = (levels - 1) as f32;
    let value = ((channel as f32 + offset).clamp(0.0, 255.0) / 255.0 * max_level).round();
    ((value / max_level) * 255.0).round().clamp(0.0, 255.0) as u8
}

fn quantize_retro_channel_bits(channel: u8, bits: u8) -> u8 {
    let levels = ((1_u32 << bits) - 1).max(1);
    let level = ((channel as u32 * levels + 127) / 255).min(levels);
    ((level * 255 + levels / 2) / levels) as u8
}

fn quantize_retro_color_bits(rgb: [u8; 3], bits: u8) -> [u8; 3] {
    [
        quantize_retro_channel_bits(rgb[0], bits),
        quantize_retro_channel_bits(rgb[1], bits),
        quantize_retro_channel_bits(rgb[2], bits),
    ]
}

fn retro_adaptive_palette_spec(mode: RetroPaletteMode) -> Option<(usize, Option<u8>)> {
    match mode {
        RetroPaletteMode::Pc98 => Some((16, None)),
        RetroPaletteMode::GameGear => Some((32, Some(4))),
        RetroPaletteMode::MegaDrive => Some((61, Some(3))),
        RetroPaletteMode::Sfc => Some((256, Some(5))),
        RetroPaletteMode::Dither1Bit
        | RetroPaletteMode::GameBoy
        | RetroPaletteMode::Famicom
        | RetroPaletteMode::Msx2Plus => None,
    }
}

fn retro_palette_lut_rgb(
    rgb: [u8; 3],
    palette: &[[u8; 3]],
    lut: &[u8],
    dither_offset: f32,
    dither: f32,
) -> [u8; 3] {
    if palette.is_empty() {
        return rgb;
    }
    let offset = dither_offset * dither * 255.0;
    let r = (rgb[0] as f32 + offset).round().clamp(0.0, 255.0) as u32;
    let g = (rgb[1] as f32 + offset).round().clamp(0.0, 255.0) as u32;
    let b = (rgb[2] as f32 + offset).round().clamp(0.0, 255.0) as u32;
    let idx = lut[retro_lut_index(r, g, b)] as usize;
    palette[idx.min(palette.len() - 1)]
}

fn generate_retro_adaptive_palette(
    src: &[u8],
    target_colors: usize,
    bit_depth: Option<u8>,
) -> Vec<[u8; 3]> {
    if target_colors <= 1 || src.len() < 4 {
        return vec![[0, 0, 0]];
    }
    let visible_count = src.chunks_exact(4).filter(|px| px[3] > 0).count();
    if visible_count == 0 {
        return vec![[0, 0, 0]];
    }
    let stride = visible_count.div_ceil(RETRO_ADAPTIVE_SAMPLE_LIMIT).max(1);
    let mut samples = Vec::with_capacity(visible_count.min(RETRO_ADAPTIVE_SAMPLE_LIMIT));
    let mut visible_seen = 0_usize;
    for px in src.chunks_exact(4) {
        if px[3] == 0 {
            continue;
        }
        if visible_seen % stride == 0 {
            let rgb = [px[0], px[1], px[2]];
            samples.push(match bit_depth {
                Some(bits) => quantize_retro_color_bits(rgb, bits),
                None => rgb,
            });
        }
        visible_seen += 1;
    }
    if samples.is_empty() {
        return vec![[0, 0, 0]];
    }

    let mut boxes = vec![samples];
    while boxes.len() < target_colors {
        let Some(idx) = boxes
            .iter()
            .enumerate()
            .filter(|(_, colors)| colors.len() >= 2)
            .max_by_key(|(_, colors)| retro_palette_box_range(colors))
            .map(|(idx, _)| idx)
        else {
            break;
        };
        let colors = boxes.swap_remove(idx);
        let (left, right) = split_retro_palette_box(colors);
        boxes.push(left);
        boxes.push(right);
    }

    let mut palette: Vec<[u8; 3]> = boxes
        .iter()
        .map(|colors| {
            let avg = average_retro_palette_color(colors);
            match bit_depth {
                Some(bits) => quantize_retro_color_bits(avg, bits),
                None => avg,
            }
        })
        .collect();
    palette.sort_unstable();
    palette.dedup();
    if palette.is_empty() {
        vec![[0, 0, 0]]
    } else {
        palette
    }
}

fn retro_palette_box_range(colors: &[[u8; 3]]) -> u32 {
    let (min, max) = retro_palette_box_bounds(colors);
    (max[0] - min[0]).max(max[1] - min[1]).max(max[2] - min[2]) as u32
}

fn split_retro_palette_box(mut colors: Vec<[u8; 3]>) -> (Vec<[u8; 3]>, Vec<[u8; 3]>) {
    let (min, max) = retro_palette_box_bounds(&colors);
    let ranges = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    let channel = (0..3).max_by_key(|&idx| ranges[idx]).unwrap_or(0);
    colors.sort_unstable_by_key(|rgb| rgb[channel]);
    let mid = colors.len() / 2;
    let right = colors.split_off(mid);
    (colors, right)
}

fn retro_palette_box_bounds(colors: &[[u8; 3]]) -> ([u8; 3], [u8; 3]) {
    let mut min = [255_u8; 3];
    let mut max = [0_u8; 3];
    for rgb in colors {
        for channel in 0..3 {
            min[channel] = min[channel].min(rgb[channel]);
            max[channel] = max[channel].max(rgb[channel]);
        }
    }
    (min, max)
}

fn average_retro_palette_color(colors: &[[u8; 3]]) -> [u8; 3] {
    let len = colors.len().max(1) as u64;
    let mut sum = [0_u64; 3];
    for rgb in colors {
        sum[0] += rgb[0] as u64;
        sum[1] += rgb[1] as u64;
        sum[2] += rgb[2] as u64;
    }
    [
        (sum[0] / len) as u8,
        (sum[1] / len) as u8,
        (sum[2] / len) as u8,
    ]
}

fn bayer4_offset(x: usize, y: usize) -> f32 {
    (RETRO_BAYER4[y & 3][x & 3] as f32 + 0.5) / 16.0 - 0.5
}

fn retro_lut_index(r: u32, g: u32, b: u32) -> usize {
    let ri = (r * RETRO_LUT_MAX + 127) / 255;
    let gi = (g * RETRO_LUT_MAX + 127) / 255;
    let bi = (b * RETRO_LUT_MAX + 127) / 255;
    ((ri * RETRO_LUT_DIM + gi) * RETRO_LUT_DIM + bi) as usize
}

fn build_retro_palette_lut(palette: &[[u8; 3]]) -> Vec<u8> {
    let mut lut = vec![0_u8; RETRO_LUT_SIZE];
    for (bin, slot) in lut.iter_mut().enumerate() {
        let bi = bin as u32 % RETRO_LUT_DIM;
        let gi = (bin as u32 / RETRO_LUT_DIM) % RETRO_LUT_DIM;
        let ri = bin as u32 / (RETRO_LUT_DIM * RETRO_LUT_DIM);
        let r = (ri * 255 / RETRO_LUT_MAX) as f32;
        let g = (gi * 255 / RETRO_LUT_MAX) as f32;
        let b = (bi * 255 / RETRO_LUT_MAX) as f32;
        *slot = nearest_retro_palette_idx(palette, r, g, b) as u8;
    }
    lut
}

fn nearest_retro_palette_idx(palette: &[[u8; 3]], r: f32, g: f32, b: f32) -> usize {
    let mut best = 0;
    let mut best_distance = f32::MAX;
    for (idx, color) in palette.iter().enumerate() {
        let dr = r - color[0] as f32;
        let dg = g - color[1] as f32;
        let db = b - color[2] as f32;
        let distance = dr * dr + dg * dg + db * db;
        if distance < best_distance {
            best_distance = distance;
            best = idx;
        }
    }
    best
}

fn apply_threshold(src: &[u8], params: ThresholdParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON {
        return src.to_vec();
    }
    let threshold = params.threshold.clamp(0.0, 1.0);
    let mut out = src.to_vec();
    out.par_chunks_exact_mut(4).for_each(|px| {
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
    });
    out
}

fn apply_invert(src: &[u8], params: InvertParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON {
        return src.to_vec();
    }
    let mut out = src.to_vec();
    out.par_chunks_exact_mut(4).for_each(|px| {
        for channel in &mut px[0..3] {
            let original = *channel as f32 / 255.0;
            *channel = to_u8(lerp_f32(original, 1.0 - original, strength));
        }
    });
    out
}

fn apply_solarize(src: &[u8], params: SolarizeParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let inversion = params.inversion.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || inversion <= f32::EPSILON {
        return src.to_vec();
    }
    let threshold = params.threshold.clamp(0.0, 1.0);
    let softness = params.softness.clamp(0.0, 0.5);
    let contrast = params.contrast.clamp(-1.0, 1.0);
    let color_amount = params.color_amount.clamp(0.0, 1.0);

    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        let base = [
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        ];
        let luma = luma01(base[0], base[1], base[2]);
        let luma_gate = solarize_gate(luma, threshold, softness);
        let solar_luma = lerp_f32(luma, 1.0 - luma, luma_gate * inversion);
        let mut color_target = [0.0; 3];
        for c in 0..3 {
            let channel_gate = solarize_gate(base[c], threshold, softness);
            let gate = lerp_f32(luma_gate, channel_gate, color_amount * 0.7);
            color_target[c] = lerp_f32(base[c], 1.0 - base[c], gate * inversion);
        }
        let mut target = [
            lerp_f32(solar_luma, color_target[0], color_amount),
            lerp_f32(solar_luma, color_target[1], color_amount),
            lerp_f32(solar_luma, color_target[2], color_amount),
        ];
        for c in &mut target {
            *c = ((*c - 0.5) * (1.0 + contrast * 1.25) + 0.5).clamp(0.0, 1.0);
        }
        px[0] = to_u8(lerp_f32(base[0], target[0], strength));
        px[1] = to_u8(lerp_f32(base[1], target[1], strength));
        px[2] = to_u8(lerp_f32(base[2], target[2], strength));
    }
    out
}

fn solarize_gate(value: f32, threshold: f32, softness: f32) -> f32 {
    if softness <= f32::EPSILON {
        if value >= threshold { 1.0 } else { 0.0 }
    } else {
        smoothstep(
            (threshold - softness).clamp(0.0, 1.0),
            (threshold + softness).clamp(0.0, 1.0),
            value,
        )
    }
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

fn apply_color_overlay(
    src: &[u8],
    width: usize,
    height: usize,
    params: ColorOverlayParams,
) -> Vec<u8> {
    let opacity = params.opacity.clamp(0.0, 1.0);
    if width == 0
        || height == 0
        || opacity <= f32::EPSILON
        || params.shape == ColorOverlayShape::Unselected
    {
        return src.to_vec();
    }
    let start = rgb_u8_to_f32(params.start_rgb);
    let end = rgb_u8_to_f32(params.end_rgb);
    let mut out = src.to_vec();
    for y in 0..height {
        let ny = normalized_pixel_coord(y, height);
        for x in 0..width {
            let nx = normalized_pixel_coord(x, width);
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }
            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let t = color_shape_gradient_t(
                nx,
                ny,
                params.shape,
                params.angle_degrees,
                params.linear_points_enabled,
                params.linear_start,
                params.linear_end,
                params.center,
                params.radius,
                params.softness,
            );
            let overlay = [
                lerp_f32(start[0], end[0], t),
                lerp_f32(start[1], end[1], t),
                lerp_f32(start[2], end[2], t),
            ];
            let blended = color_overlay_blend_rgb(base, overlay, params.blend_mode);
            out[i] = to_u8(lerp_f32(base[0], blended[0], opacity));
            out[i + 1] = to_u8(lerp_f32(base[1], blended[1], opacity));
            out[i + 2] = to_u8(lerp_f32(base[2], blended[2], opacity));
        }
    }
    out
}

#[derive(Debug, Clone, Copy)]
struct RepairTile {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    anchor: usize,
    distance: u32,
}

#[derive(Debug, Clone, Copy)]
struct RepairRgbStats {
    mean: [f32; 3],
    std_dev: [f32; 3],
}

struct RepairBlendState {
    x0: usize,
    y0: usize,
    width: usize,
    weights: Vec<f32>,
}

impl RepairBlendState {
    fn new(image_width: usize, image_height: usize, hole: &[bool]) -> Self {
        let mut x0 = image_width;
        let mut y0 = image_height;
        let mut x1 = 0;
        let mut y1 = 0;
        for (index, inside) in hole.iter().copied().enumerate() {
            if !inside {
                continue;
            }
            let x = index % image_width;
            let y = index / image_width;
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
        let width = x1.saturating_sub(x0);
        let height = y1.saturating_sub(y0);
        Self {
            x0,
            y0,
            width,
            weights: vec![0.0; width.saturating_mul(height)],
        }
    }

    fn local_index(&self, x: usize, y: usize) -> usize {
        (y - self.y0) * self.width + (x - self.x0)
    }

    fn has_sample(&self, x: usize, y: usize) -> bool {
        self.weights[self.local_index(x, y)] > f32::EPSILON
    }

    fn blend_rgb(
        &mut self,
        out: &mut [u8],
        target: usize,
        source_rgb: &[u8],
        x: usize,
        y: usize,
        weight: f32,
    ) {
        let local = self.local_index(x, y);
        let previous_weight = self.weights[local];
        let total_weight = previous_weight + weight;
        let pixel = target * 4;
        if previous_weight <= f32::EPSILON {
            out[pixel..pixel + 3].copy_from_slice(source_rgb);
        } else {
            for channel in 0..3 {
                let value = (out[pixel + channel] as f32 * previous_weight
                    + source_rgb[channel] as f32 * weight)
                    / total_weight;
                out[pixel + channel] = value.round().clamp(0.0, 255.0) as u8;
            }
        }
        self.weights[local] = total_weight;
    }
}

fn apply_repair<F>(
    src: &[u8],
    width: usize,
    height: usize,
    mask: &[f32],
    params: RepairParams,
    cancel: Option<&AtomicBool>,
    mut progress: F,
) -> Result<Vec<u8>>
where
    F: FnMut(f32),
{
    if width == 0 || height == 0 {
        return Ok(src.to_vec());
    }
    let hole: Vec<bool> = mask.iter().map(|value| *value > 0.001).collect();
    if !hole.iter().any(|value| *value) {
        return Ok(src.to_vec());
    }
    check_cancel(cancel)?;
    progress(0.05);

    match params.mode {
        RepairMode::Solid | RepairMode::PreserveLuminance => {
            let overlay = rgb_u8_to_f32(params.sampled_rgb);
            let mut out = src.to_vec();
            out.par_chunks_exact_mut(4)
                .zip(src.par_chunks_exact(4))
                .zip(hole.par_iter())
                .for_each(|((dst, base), inside)| {
                    if !inside {
                        return;
                    }
                    let replacement = if params.mode == RepairMode::PreserveLuminance {
                        color_overlay_blend_rgb(
                            [
                                base[0] as f32 / 255.0,
                                base[1] as f32 / 255.0,
                                base[2] as f32 / 255.0,
                            ],
                            overlay,
                            ColorOverlayBlendMode::Color,
                        )
                    } else {
                        overlay
                    };
                    for channel in 0..3 {
                        dst[channel] = to_u8(replacement[channel]);
                    }
                });
            progress(0.95);
            Ok(out)
        }
        RepairMode::Clone => {
            let (Some(source_uv), Some(destination_uv)) =
                (params.clone_source_uv, params.clone_destination_uv)
            else {
                return Ok(src.to_vec());
            };
            let source = repair_uv_to_pixel(source_uv, width, height);
            let destination = repair_uv_to_pixel(destination_uv, width, height);
            let offset = [source[0] - destination[0], source[1] - destination[1]];
            let mut out = src.to_vec();
            let mut cloned = vec![false; hole.len()];
            for y in 0..height {
                if y % 64 == 0 {
                    check_cancel(cancel)?;
                    progress(0.1 + 0.62 * y as f32 / height.max(1) as f32);
                }
                for x in 0..width {
                    let index = y * width + x;
                    if !hole[index] {
                        continue;
                    }
                    let source_x = x as f32 + offset[0];
                    let source_y = y as f32 + offset[1];
                    if source_x < 0.0
                        || source_y < 0.0
                        || source_x > width.saturating_sub(1) as f32
                        || source_y > height.saturating_sub(1) as f32
                    {
                        continue;
                    }
                    let (sampled, sampled_alpha) =
                        sample_rgb_bilinear_alpha_aware(src, width, height, source_x, source_y);
                    if sampled_alpha <= f32::EPSILON {
                        continue;
                    }
                    let pixel = index * 4;
                    for channel in 0..3 {
                        out[pixel + channel] = to_u8(sampled[channel]);
                    }
                    cloned[index] = true;
                }
            }
            harmonize_repair(src, &mut out, width, height, &cloned, params);
            progress(0.95);
            Ok(out)
        }
        RepairMode::Surrounding => {
            apply_surrounding_repair(src, width, height, &hole, params, cancel, progress)
        }
    }
}

fn repair_uv_to_pixel(uv: [f32; 2], width: usize, height: usize) -> [f32; 2] {
    [
        uv[0].clamp(0.0, 1.0) * width.saturating_sub(1) as f32,
        uv[1].clamp(0.0, 1.0) * height.saturating_sub(1) as f32,
    ]
}

fn apply_surrounding_repair<F>(
    src: &[u8],
    width: usize,
    height: usize,
    hole: &[bool],
    params: RepairParams,
    cancel: Option<&AtomicBool>,
    mut progress: F,
) -> Result<Vec<u8>>
where
    F: FnMut(f32),
{
    let source_available: Vec<bool> = hole
        .iter()
        .copied()
        .zip(src.chunks_exact(4))
        .map(|(inside, pixel)| !inside && pixel[3] > 0)
        .collect();
    let (distance, nearest_source) =
        repair_distance_map(width, height, hole, &source_available, cancel)?;
    if nearest_source.iter().all(|source| *source == usize::MAX) {
        return Ok(src.to_vec());
    }
    check_cancel(cancel)?;
    progress(0.12);

    let (patch_size, patch_step, context, candidate_count) =
        repair_patch_geometry(params.quality, params.patch_size);
    let mut tiles = repair_tiles(width, height, hole, &distance, patch_size, patch_step);
    tiles.sort_unstable_by_key(|tile| tile.distance);
    let mut out = src.to_vec();
    let mut blend_state = RepairBlendState::new(width, height, hole);
    let radius = params.search_radius_px.round().clamp(8.0, 512.0) as i32;
    let target_hint = if params.color_source == RepairColorSource::Sampled {
        rgb_u8_to_f32(params.sampled_rgb)
    } else {
        repair_surrounding_stats(src, width, height, hole)
            .map(|stats| stats.mean)
            .unwrap_or_else(|| rgb_u8_to_f32(params.sampled_rgb))
    };

    for (tile_index, tile) in tiles.iter().copied().enumerate() {
        if tile_index % 16 == 0 {
            check_cancel(cancel)?;
            progress(0.12 + 0.68 * tile_index as f32 / tiles.len().max(1) as f32);
        }
        let anchor_x = tile.anchor % width;
        let anchor_y = tile.anchor / width;
        let mut offsets = Vec::with_capacity(candidate_count + 1);
        let nearest = nearest_source[tile.anchor];
        let nearest_offset = if nearest != usize::MAX {
            Some([
                nearest as i32 % width as i32 - anchor_x as i32,
                nearest as i32 / width as i32 - anchor_y as i32,
            ])
        } else {
            None
        };
        if let Some(nearest_offset) = nearest_offset {
            offsets.push(nearest_offset);
        }
        let max_attempts = candidate_count * 16;
        for attempt in 0..max_attempts {
            if offsets.len() >= candidate_count {
                break;
            }
            let base_seed = params.seed
                ^ (tile_index as u32).wrapping_mul(0x9E37_79B1)
                ^ (attempt as u32).wrapping_mul(0x85EB_CA77);
            let jitter_x = repair_hash_offset(base_seed, radius);
            let jitter_y = repair_hash_offset(base_seed ^ 0xA511_E9B3, radius);
            if jitter_x * jitter_x + jitter_y * jitter_y > radius * radius {
                continue;
            }
            let base = nearest_offset.unwrap_or([0, 0]);
            let offset = [base[0] + jitter_x, base[1] + jitter_y];
            if !offsets.contains(&offset)
                && repair_tile_source_is_valid(tile, offset, width, height, hole, &source_available)
            {
                offsets.push(offset);
            }
        }

        let mut best_offset = None;
        let mut best_score = f32::INFINITY;
        for offset in offsets {
            if !repair_tile_source_is_valid(tile, offset, width, height, hole, &source_available) {
                continue;
            }
            let score = repair_patch_score(
                src,
                &out,
                width,
                height,
                &source_available,
                hole,
                &blend_state,
                tile,
                offset,
                context,
                target_hint,
            );
            if score < best_score {
                best_score = score;
                best_offset = Some(offset);
            }
        }

        for y in tile.y0..tile.y1 {
            for x in tile.x0..tile.x1 {
                let target = y * width + x;
                if !hole[target] {
                    continue;
                }
                let source = best_offset
                    .and_then(|offset| repair_offset_index(x, y, offset, width, height))
                    .filter(|source| source_available[*source])
                    .or_else(|| {
                        (nearest_source[target] != usize::MAX).then_some(nearest_source[target])
                    });
                let Some(source) = source else {
                    continue;
                };
                let source_pixel = source * 4;
                blend_state.blend_rgb(
                    &mut out,
                    target,
                    &src[source_pixel..source_pixel + 3],
                    x,
                    y,
                    repair_tile_blend_weight(tile, x, y),
                );
            }
        }
    }
    check_cancel(cancel)?;
    progress(0.84);
    harmonize_repair(src, &mut out, width, height, hole, params);
    progress(0.95);
    Ok(out)
}

fn repair_patch_geometry(
    quality: RepairQuality,
    patch_size: RepairPatchSize,
) -> (usize, usize, usize, usize) {
    let (auto_patch_size, auto_patch_step, auto_context, candidate_count) = match quality {
        RepairQuality::Fast => (14, 10, 2, 10),
        RepairQuality::Standard => (12, 6, 3, 18),
        RepairQuality::High => (10, 5, 4, 32),
    };
    let (patch_size, patch_step, context) = match patch_size {
        RepairPatchSize::Auto => (auto_patch_size, auto_patch_step, auto_context),
        RepairPatchSize::Standard => (24, 12, 6),
        RepairPatchSize::Large => (48, 24, 10),
    };
    (patch_size, patch_step, context, candidate_count)
}

fn repair_hash_offset(seed: u32, radius: i32) -> i32 {
    let unit = hash_u32(seed) as f32 / u32::MAX as f32;
    ((unit * 2.0 - 1.0) * radius as f32).round() as i32
}

fn repair_distance_map(
    width: usize,
    height: usize,
    hole: &[bool],
    source_available: &[bool],
    cancel: Option<&AtomicBool>,
) -> Result<(Vec<u32>, Vec<usize>)> {
    let len = width.saturating_mul(height);
    let mut distance = vec![u32::MAX; len];
    let mut nearest_source = vec![usize::MAX; len];
    let mut queue = VecDeque::new();
    for y in 0..height {
        if y % 128 == 0 {
            check_cancel(cancel)?;
        }
        for x in 0..width {
            let index = y * width + x;
            if !hole[index] {
                continue;
            }
            for neighbor in repair_neighbors(x, y, width, height) {
                if source_available[neighbor] {
                    distance[index] = 1;
                    nearest_source[index] = neighbor;
                    queue.push_back(index);
                    break;
                }
            }
        }
    }
    let mut visited = 0_usize;
    while let Some(index) = queue.pop_front() {
        visited += 1;
        if visited.is_multiple_of(65_536) {
            check_cancel(cancel)?;
        }
        let x = index % width;
        let y = index / width;
        for neighbor in repair_neighbors(x, y, width, height) {
            if hole[neighbor] && distance[neighbor] == u32::MAX {
                distance[neighbor] = distance[index].saturating_add(1);
                nearest_source[neighbor] = nearest_source[index];
                queue.push_back(neighbor);
            }
        }
    }
    Ok((distance, nearest_source))
}

fn repair_neighbors(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> impl Iterator<Item = usize> {
    let mut neighbors = [usize::MAX; 4];
    let mut count = 0;
    if x > 0 {
        neighbors[count] = y * width + x - 1;
        count += 1;
    }
    if x + 1 < width {
        neighbors[count] = y * width + x + 1;
        count += 1;
    }
    if y > 0 {
        neighbors[count] = (y - 1) * width + x;
        count += 1;
    }
    if y + 1 < height {
        neighbors[count] = (y + 1) * width + x;
        count += 1;
    }
    neighbors.into_iter().take(count)
}

fn repair_tiles(
    width: usize,
    height: usize,
    hole: &[bool],
    distance: &[u32],
    patch_size: usize,
    patch_step: usize,
) -> Vec<RepairTile> {
    let mut tiles = Vec::new();
    for y0 in (0..height).step_by(patch_step.max(1)) {
        for x0 in (0..width).step_by(patch_step.max(1)) {
            let y1 = (y0 + patch_size).min(height);
            let x1 = (x0 + patch_size).min(width);
            let mut anchor = usize::MAX;
            let mut min_distance = u32::MAX;
            for y in y0..y1 {
                for x in x0..x1 {
                    let index = y * width + x;
                    if hole[index] && distance[index] < min_distance {
                        anchor = index;
                        min_distance = distance[index];
                    }
                }
            }
            if anchor != usize::MAX {
                tiles.push(RepairTile {
                    x0,
                    y0,
                    x1,
                    y1,
                    anchor,
                    distance: min_distance,
                });
            }
        }
    }
    tiles
}

fn repair_tile_blend_weight(tile: RepairTile, x: usize, y: usize) -> f32 {
    let width = tile.x1.saturating_sub(tile.x0).max(1) as f32;
    let height = tile.y1.saturating_sub(tile.y0).max(1) as f32;
    let x_norm = (x.saturating_sub(tile.x0) as f32 + 0.5) / width;
    let y_norm = (y.saturating_sub(tile.y0) as f32 + 0.5) / height;
    let x_weight = (1.0 - (x_norm * 2.0 - 1.0).abs()).clamp(0.08, 1.0);
    let y_weight = (1.0 - (y_norm * 2.0 - 1.0).abs()).clamp(0.08, 1.0);
    x_weight * y_weight
}

fn repair_tile_source_is_valid(
    tile: RepairTile,
    offset: [i32; 2],
    width: usize,
    height: usize,
    hole: &[bool],
    source_available: &[bool],
) -> bool {
    for y in tile.y0..tile.y1 {
        for x in tile.x0..tile.x1 {
            let target = y * width + x;
            if !hole[target] {
                continue;
            }
            let Some(source) = repair_offset_index(x, y, offset, width, height) else {
                return false;
            };
            if !source_available[source] {
                return false;
            }
        }
    }
    true
}

fn repair_offset_index(
    x: usize,
    y: usize,
    offset: [i32; 2],
    width: usize,
    height: usize,
) -> Option<usize> {
    let source_x = x as i32 + offset[0];
    let source_y = y as i32 + offset[1];
    if source_x < 0 || source_y < 0 || source_x >= width as i32 || source_y >= height as i32 {
        None
    } else {
        Some(source_y as usize * width + source_x as usize)
    }
}

#[allow(clippy::too_many_arguments)]
fn repair_patch_score(
    src: &[u8],
    out: &[u8],
    width: usize,
    height: usize,
    source_available: &[bool],
    hole: &[bool],
    blend_state: &RepairBlendState,
    tile: RepairTile,
    offset: [i32; 2],
    context: usize,
    target_hint: [f32; 3],
) -> f32 {
    let min_x = tile.x0.saturating_sub(context);
    let min_y = tile.y0.saturating_sub(context);
    let max_x = (tile.x1 + context).min(width);
    let max_y = (tile.y1 + context).min(height);
    let mut error = 0.0_f32;
    let mut samples = 0_u32;
    for y in min_y..max_y {
        for x in min_x..max_x {
            let target = y * width + x;
            if hole[target] && !blend_state.has_sample(x, y) {
                continue;
            }
            let Some(source) = repair_offset_index(x, y, offset, width, height) else {
                continue;
            };
            if !source_available[source] {
                continue;
            }
            let target_pixel = target * 4;
            let source_pixel = source * 4;
            for channel in 0..3 {
                let delta = out[target_pixel + channel] as f32 - src[source_pixel + channel] as f32;
                error += delta * delta;
            }
            samples += 1;
        }
    }
    let anchor_x = tile.anchor % width;
    let anchor_y = tile.anchor / width;
    if let Some(source) = repair_offset_index(anchor_x, anchor_y, offset, width, height) {
        let source_pixel = source * 4;
        for channel in 0..3 {
            let delta = src[source_pixel + channel] as f32 / 255.0 - target_hint[channel];
            error += delta * delta * 255.0 * 255.0 * 0.15;
        }
    }
    error / samples.max(1) as f32
}

fn harmonize_repair(
    src: &[u8],
    out: &mut [u8],
    width: usize,
    height: usize,
    hole: &[bool],
    params: RepairParams,
) {
    let texture_strength = params.texture_strength.clamp(0.0, 1.0);
    if texture_strength < 0.999 {
        let softened = box_blur_rgba(out, width, height, 2);
        for (index, inside) in hole.iter().copied().enumerate() {
            if !inside {
                continue;
            }
            let pixel = index * 4;
            for channel in 0..3 {
                out[pixel + channel] = lerp_u8(
                    softened[pixel + channel],
                    out[pixel + channel],
                    texture_strength,
                );
            }
        }
    }
    let match_strength = params.color_match_strength.clamp(0.0, 1.0);
    if match_strength <= f32::EPSILON {
        return;
    }
    let Some(mut target) = repair_surrounding_stats(src, width, height, hole) else {
        return;
    };
    if params.color_source == RepairColorSource::Sampled {
        target.mean = rgb_u8_to_f32(params.sampled_rgb);
    }
    let Some(current) = repair_masked_stats(out, hole) else {
        return;
    };
    for (index, inside) in hole.iter().copied().enumerate() {
        if !inside {
            continue;
        }
        let pixel = index * 4;
        for channel in 0..3 {
            let value = out[pixel + channel] as f32 / 255.0;
            let scale = if current.std_dev[channel] > 0.003 {
                (target.std_dev[channel] / current.std_dev[channel]).clamp(0.35, 2.8)
            } else {
                1.0
            };
            let matched =
                ((value - current.mean[channel]) * scale + target.mean[channel]).clamp(0.0, 1.0);
            out[pixel + channel] = to_u8(lerp_f32(value, matched, match_strength));
        }
    }
}

fn repair_surrounding_stats(
    src: &[u8],
    width: usize,
    height: usize,
    hole: &[bool],
) -> Option<RepairRgbStats> {
    let mut boundary = vec![false; hole.len()];
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if hole[index] {
                continue;
            }
            boundary[index] = repair_neighbors(x, y, width, height).any(|neighbor| hole[neighbor]);
        }
    }
    repair_stats(src, boundary)
}

fn repair_masked_stats(src: &[u8], hole: &[bool]) -> Option<RepairRgbStats> {
    repair_stats(src, hole.iter().copied())
}

fn repair_stats(src: &[u8], selected: impl IntoIterator<Item = bool>) -> Option<RepairRgbStats> {
    let mut sum = [0.0_f64; 3];
    let mut sum_squared = [0.0_f64; 3];
    let mut count = 0_u64;
    for (pixel, selected) in src.chunks_exact(4).zip(selected) {
        if !selected || pixel[3] == 0 {
            continue;
        }
        for channel in 0..3 {
            let value = pixel[channel] as f64 / 255.0;
            sum[channel] += value;
            sum_squared[channel] += value * value;
        }
        count += 1;
    }
    if count == 0 {
        return None;
    }
    let mut mean = [0.0_f32; 3];
    let mut std_dev = [0.0_f32; 3];
    for channel in 0..3 {
        let channel_mean = sum[channel] / count as f64;
        mean[channel] = channel_mean as f32;
        std_dev[channel] = (sum_squared[channel] / count as f64 - channel_mean * channel_mean)
            .max(0.0)
            .sqrt() as f32;
    }
    Some(RepairRgbStats { mean, std_dev })
}

fn apply_color_fill(src: &[u8], width: usize, height: usize, params: ColorFillParams) -> Vec<u8> {
    let opacity = params.opacity.clamp(0.0, 1.0);
    if width == 0
        || height == 0
        || opacity <= f32::EPSILON
        || params.shape == ColorOverlayShape::Unselected
    {
        return src.to_vec();
    }
    let mut out = src.to_vec();
    for y in 0..height {
        let ny = normalized_pixel_coord(y, height);
        for x in 0..width {
            let nx = normalized_pixel_coord(x, width);
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }
            let t = color_shape_gradient_t(
                nx,
                ny,
                params.shape,
                params.angle_degrees,
                params.linear_points_enabled,
                params.linear_start,
                params.linear_end,
                params.center,
                params.radius,
                params.softness,
            );
            let fill = color_fill_rgb(t, params);
            for c in 0..3 {
                let base = src[i + c] as f32 / 255.0;
                out[i + c] = to_u8(lerp_f32(base, fill[c], opacity));
            }
        }
    }
    out
}

fn apply_frame(src: &[u8], width: usize, height: usize, params: FrameParams) -> Vec<u8> {
    let opacity = params.opacity.clamp(0.0, 1.0);
    if width == 0 || height == 0 || opacity <= f32::EPSILON {
        return src.to_vec();
    }
    let mut out = src.to_vec();
    let color = params.color_rgb;
    let line_color = params.line_rgb;
    let line_opacity = params.line_opacity.clamp(0.0, 1.0);
    for y in 0..height {
        let py = y as f32 + 0.5;
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }
            let px = x as f32 + 0.5;
            let (matte_amount, line_amount) = match params.mode {
                FrameMode::Border => frame_border_amounts(px, py, width, height, params),
                FrameMode::Letterbox => frame_letterbox_amounts(px, py, width, height, params),
                FrameMode::RoundedMatte => {
                    frame_rounded_matte_amounts(px, py, width, height, params)
                }
            };
            let matte_amount = matte_amount * opacity;
            if matte_amount > f32::EPSILON {
                for c in 0..3 {
                    out[i + c] = lerp_u8(out[i + c], color[c], matte_amount);
                }
            }
            let line_amount = line_amount * line_opacity * opacity;
            if line_amount > f32::EPSILON {
                for c in 0..3 {
                    out[i + c] = lerp_u8(out[i + c], line_color[c], line_amount);
                }
            }
        }
    }
    out
}

fn frame_border_amounts(
    px: f32,
    py: f32,
    width: usize,
    height: usize,
    params: FrameParams,
) -> (f32, f32) {
    let (top, right, bottom, left) = frame_effective_widths(params);
    let width = width as f32;
    let height = height as f32;
    let softness = params.softness_px.max(0.0);
    let line_width = params.line_width_px.max(0.0);
    let left_distance = px;
    let right_distance = width - px;
    let top_distance = py;
    let bottom_distance = height - py;
    let matte = [
        frame_edge_amount(left_distance, left, softness),
        frame_edge_amount(right_distance, right, softness),
        frame_edge_amount(top_distance, top, softness),
        frame_edge_amount(bottom_distance, bottom, softness),
    ]
    .into_iter()
    .fold(0.0_f32, f32::max);
    let line = [
        frame_inner_line_amount(left_distance, left, line_width, softness),
        frame_inner_line_amount(right_distance, right, line_width, softness),
        frame_inner_line_amount(top_distance, top, line_width, softness),
        frame_inner_line_amount(bottom_distance, bottom, line_width, softness),
    ]
    .into_iter()
    .fold(0.0_f32, f32::max);
    (matte, line)
}

fn frame_letterbox_amounts(
    px: f32,
    py: f32,
    width: usize,
    height: usize,
    params: FrameParams,
) -> (f32, f32) {
    let w = width as f32;
    let h = height as f32;
    let aspect = params.aspect_ratio.clamp(0.1, 10.0);
    let current_aspect = if h > 0.0 { w / h } else { aspect };
    let softness = params.softness_px.max(0.0);
    let line_width = params.line_width_px.max(0.0);
    if current_aspect < aspect {
        let content_h = (w / aspect).min(h).max(0.0);
        let bar = ((h - content_h) * 0.5).max(0.0);
        let top_distance = py;
        let bottom_distance = h - py;
        let matte = frame_edge_amount(top_distance, bar, softness).max(frame_edge_amount(
            bottom_distance,
            bar,
            softness,
        ));
        let line = frame_inner_line_amount(top_distance, bar, line_width, softness).max(
            frame_inner_line_amount(bottom_distance, bar, line_width, softness),
        );
        (matte, line)
    } else {
        let content_w = (h * aspect).min(w).max(0.0);
        let bar = ((w - content_w) * 0.5).max(0.0);
        let left_distance = px;
        let right_distance = w - px;
        let matte = frame_edge_amount(left_distance, bar, softness).max(frame_edge_amount(
            right_distance,
            bar,
            softness,
        ));
        let line = frame_inner_line_amount(left_distance, bar, line_width, softness).max(
            frame_inner_line_amount(right_distance, bar, line_width, softness),
        );
        (matte, line)
    }
}

fn frame_rounded_matte_amounts(
    px: f32,
    py: f32,
    width: usize,
    height: usize,
    params: FrameParams,
) -> (f32, f32) {
    let inset = params.width_px.max(0.0);
    let radius = params.corner_radius_px.max(0.0);
    let softness = params.softness_px.max(0.0);
    let line_width = params.line_width_px.max(0.0);
    let signed_distance = rounded_rect_signed_distance(px, py, width, height, inset, radius);
    let matte = if softness <= f32::EPSILON {
        if signed_distance >= 0.0 { 1.0 } else { 0.0 }
    } else {
        smoothstep(-softness, 0.0, signed_distance)
    };
    let line = if signed_distance <= 0.0 {
        frame_inner_line_amount(-signed_distance, 0.0, line_width, softness)
    } else {
        0.0
    };
    (matte, line)
}

fn frame_effective_widths(params: FrameParams) -> (f32, f32, f32, f32) {
    if params.use_individual_widths {
        (
            params.top_px.max(0.0),
            params.right_px.max(0.0),
            params.bottom_px.max(0.0),
            params.left_px.max(0.0),
        )
    } else {
        let width = params.width_px.max(0.0);
        (width, width, width, width)
    }
}

fn frame_edge_amount(distance: f32, width: f32, softness: f32) -> f32 {
    if width <= f32::EPSILON {
        return 0.0;
    }
    if softness <= f32::EPSILON {
        return if distance < width { 1.0 } else { 0.0 };
    }
    let hard_width = (width - softness).max(0.0);
    if distance <= hard_width {
        1.0
    } else if distance >= width {
        0.0
    } else {
        smoothstep(0.0, 1.0, (width - distance) / softness)
    }
}

fn frame_inner_line_amount(distance: f32, edge_width: f32, line_width: f32, softness: f32) -> f32 {
    if line_width <= f32::EPSILON {
        return 0.0;
    }
    frame_band_amount(
        distance,
        edge_width.max(0.0),
        edge_width.max(0.0) + line_width,
        softness,
    )
}

fn frame_band_amount(distance: f32, start: f32, end: f32, softness: f32) -> f32 {
    if end <= start {
        return 0.0;
    }
    if softness <= f32::EPSILON {
        return if distance >= start && distance < end {
            1.0
        } else {
            0.0
        };
    }
    let enter = smoothstep(start - softness, start, distance);
    let leave = 1.0 - smoothstep(end, end + softness, distance);
    enter.min(leave).clamp(0.0, 1.0)
}

fn rounded_rect_signed_distance(
    px: f32,
    py: f32,
    width: usize,
    height: usize,
    inset: f32,
    radius: f32,
) -> f32 {
    let w = width as f32;
    let h = height as f32;
    let inner_w = (w - inset * 2.0).max(0.0);
    let inner_h = (h - inset * 2.0).max(0.0);
    if inner_w <= f32::EPSILON || inner_h <= f32::EPSILON {
        return 1.0;
    }
    let half_w = inner_w * 0.5;
    let half_h = inner_h * 0.5;
    let radius = radius.clamp(0.0, half_w.min(half_h));
    let cx = w * 0.5;
    let cy = h * 0.5;
    let qx = (px - cx).abs() - (half_w - radius);
    let qy = (py - cy).abs() - (half_h - radius);
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - radius
}

fn apply_outline_stroke(
    src: &[u8],
    width: usize,
    height: usize,
    params: OutlineStrokeParams,
    cancel: Option<&AtomicBool>,
    mut progress: impl FnMut(f32),
) -> Result<Vec<u8>> {
    let mut out = vec![0; src.len()];
    let opacity = params.opacity.clamp(0.0, 1.0);
    let radius = params.width_px.round().clamp(0.0, 96.0) as i32;
    if width == 0 || height == 0 || radius == 0 || opacity <= f32::EPSILON {
        progress(1.0);
        return Ok(out);
    }

    check_cancel(cancel)?;
    let alpha: Vec<f32> = src.chunks_exact(4).map(|px| px[3] as f32 / 255.0).collect();
    progress(0.02);
    let needs_dilate = matches!(
        params.placement,
        OutlineStrokePlacement::Outside | OutlineStrokePlacement::Center
    );
    let needs_erode = matches!(
        params.placement,
        OutlineStrokePlacement::Inside | OutlineStrokePlacement::Center
    );
    let (dilate_start, dilate_span, erode_start, erode_span) = if needs_dilate && needs_erode {
        (0.02, 0.40, 0.42, 0.40)
    } else {
        (0.02, 0.72, 0.02, 0.72)
    };
    let dilated = if needs_dilate {
        Some(morph_alpha_disk_with_outside(
            &alpha,
            width,
            height,
            radius,
            true,
            0.0,
            cancel,
            |p| progress(dilate_start + p * dilate_span),
        )?)
    } else {
        None
    };
    let eroded = if needs_erode {
        Some(morph_alpha_disk_with_outside(
            &alpha,
            width,
            height,
            radius,
            false,
            0.0,
            cancel,
            |p| progress(erode_start + p * erode_span),
        )?)
    } else {
        None
    };
    check_cancel(cancel)?;
    progress(0.84);

    let mut stroke = vec![0.0; alpha.len()];
    for idx in 0..alpha.len() {
        stroke[idx] = match params.placement {
            OutlineStrokePlacement::Outside => {
                (dilated.as_ref().expect("outside stroke has dilation")[idx] - alpha[idx])
                    .clamp(0.0, 1.0)
            }
            OutlineStrokePlacement::Inside => (alpha[idx]
                - eroded.as_ref().expect("inside stroke has erosion")[idx])
                .clamp(0.0, 1.0),
            OutlineStrokePlacement::Center => {
                (dilated.as_ref().expect("center stroke has dilation")[idx]
                    - eroded.as_ref().expect("center stroke has erosion")[idx])
                    .clamp(0.0, 1.0)
            }
        };
    }

    let softness = params.softness_px.round().clamp(0.0, 32.0) as usize;
    if softness > 0 {
        stroke = box_blur_alpha(&stroke, width, height, softness);
    }
    check_cancel(cancel)?;
    progress(0.92);

    for (idx, amount) in stroke.iter().enumerate() {
        if idx % 8192 == 0 {
            check_cancel(cancel)?;
        }
        let amount = (amount * opacity).clamp(0.0, 1.0);
        if amount <= f32::EPSILON {
            continue;
        }
        let o = idx * 4;
        out[o] = params.color_rgb[0];
        out[o + 1] = params.color_rgb[1];
        out[o + 2] = params.color_rgb[2];
        out[o + 3] = to_u8(amount);
    }
    progress(1.0);
    Ok(out)
}

fn apply_rim_light(
    src: &[u8],
    width: usize,
    height: usize,
    params: RimLightParams,
    cancel: Option<&AtomicBool>,
    mut progress: impl FnMut(f32),
) -> Result<Vec<u8>> {
    let mut out = vec![0; src.len()];
    let strength = params.strength.clamp(0.0, 2.0);
    let radius = params.width_px.round().clamp(0.0, 96.0) as i32;
    if width == 0 || height == 0 || radius == 0 || strength <= f32::EPSILON {
        progress(1.0);
        return Ok(out);
    }

    check_cancel(cancel)?;
    let alpha: Vec<f32> = src.chunks_exact(4).map(|px| px[3] as f32 / 255.0).collect();
    progress(0.02);

    let dilated =
        morph_alpha_disk_with_outside(&alpha, width, height, radius, true, 0.0, cancel, |p| {
            progress(0.02 + p * 0.34);
        })?;
    let inside_radius = (radius / 2).max(1);
    let eroded = morph_alpha_disk_with_outside(
        &alpha,
        width,
        height,
        inside_radius,
        false,
        0.0,
        cancel,
        |p| {
            progress(0.36 + p * 0.24);
        },
    )?;
    check_cancel(cancel)?;

    let mut band = vec![0.0; alpha.len()];
    for idx in 0..alpha.len() {
        band[idx] = (dilated[idx] - eroded[idx]).clamp(0.0, 1.0);
    }
    let falloff = params.falloff.clamp(0.0, 1.0);
    let softness = (radius as f32 * falloff * 0.75).round().clamp(0.0, 32.0) as usize;
    if softness > 0 {
        band = box_blur_alpha(&band, width, height, softness);
    }
    progress(0.68);

    let direction_field = if radius <= 1 {
        alpha
    } else {
        let normal_radius = (radius / 2).clamp(1, 16) as usize;
        box_blur_alpha(&alpha, width, height, normal_radius)
    };
    let angle = params.light_angle_degrees.to_radians();
    let light_x = angle.cos();
    let light_y = angle.sin();
    let wrap = params.wrap.clamp(0.0, 1.0);
    let color = params.color_rgb;

    for y in 0..height {
        if y % 32 == 0 {
            check_cancel(cancel)?;
        }
        for x in 0..width {
            let idx = y * width + x;
            let band_amount = band[idx].clamp(0.0, 1.0);
            if band_amount <= f32::EPSILON {
                continue;
            }
            let (gx, gy) = alpha_gradient_with_outside(&direction_field, width, height, x, y, 0.0);
            let len = (gx * gx + gy * gy).sqrt();
            if len <= f32::EPSILON {
                continue;
            }
            let outward_x = -gx / len;
            let outward_y = -gy / len;
            let facing = (outward_x * light_x + outward_y * light_y).clamp(-1.0, 1.0);
            let wrapped = ((facing + wrap) / (1.0 + wrap)).clamp(0.0, 1.0);
            let direction_amount = smoothstep(0.0, 1.0, wrapped);
            let amount = (band_amount * direction_amount * strength).clamp(0.0, 1.0);
            if amount <= f32::EPSILON {
                continue;
            }
            let o = idx * 4;
            out[o] = color[0];
            out[o + 1] = color[1];
            out[o + 2] = color[2];
            out[o + 3] = to_u8(amount);
        }
    }
    progress(1.0);
    Ok(out)
}

fn apply_contact_shadow(
    src: &[u8],
    width: usize,
    height: usize,
    mask: &[f32],
    params: ContactShadowParams,
    cancel: Option<&AtomicBool>,
    mut progress: impl FnMut(f32),
) -> Result<Vec<u8>> {
    let mut out = src.to_vec();
    let strength = params.strength.clamp(0.0, 1.0);
    let radius = params.radius_px.round().clamp(0.0, 96.0) as i32;
    if width == 0
        || height == 0
        || radius == 0
        || strength <= f32::EPSILON
        || mask.len() != width.saturating_mul(height)
    {
        progress(1.0);
        return Ok(out);
    }

    check_cancel(cancel)?;
    let alpha: Vec<f32> = src
        .chunks_exact(4)
        .zip(mask.iter())
        .map(|(px, mask_amount)| (px[3] as f32 / 255.0) * mask_amount.clamp(0.0, 1.0))
        .collect();
    progress(0.02);

    let eroded =
        morph_alpha_disk_with_outside(&alpha, width, height, radius, false, 0.0, cancel, |p| {
            progress(0.02 + p * 0.50);
        })?;
    check_cancel(cancel)?;

    let mut band = vec![0.0; alpha.len()];
    for idx in 0..alpha.len() {
        band[idx] = (alpha[idx] - eroded[idx]).clamp(0.0, 1.0);
    }
    let softness = params.softness_px.round().clamp(0.0, 32.0) as usize;
    if softness > 0 {
        band = box_blur_alpha(&band, width, height, softness);
    }
    progress(0.68);

    let directionality = params.directionality.clamp(0.0, 1.0);
    let direction_field = if directionality <= f32::EPSILON || radius <= 1 {
        alpha
    } else {
        let normal_radius = (radius / 2).clamp(1, 16) as usize;
        box_blur_alpha(&alpha, width, height, normal_radius)
    };
    let angle = params.direction_degrees.to_radians();
    let shadow_x = angle.cos();
    let shadow_y = angle.sin();
    let color = params.color_rgb;

    for y in 0..height {
        if y % 32 == 0 {
            check_cancel(cancel)?;
        }
        for x in 0..width {
            let idx = y * width + x;
            let mut amount = band[idx].clamp(0.0, 1.0) * strength;
            if amount <= f32::EPSILON {
                continue;
            }
            if directionality > f32::EPSILON {
                let (gx, gy) =
                    alpha_gradient_with_outside(&direction_field, width, height, x, y, 0.0);
                let len = (gx * gx + gy * gy).sqrt();
                if len <= f32::EPSILON {
                    continue;
                }
                let outward_x = -gx / len;
                let outward_y = -gy / len;
                let facing = (outward_x * shadow_x + outward_y * shadow_y).clamp(0.0, 1.0);
                let directional_amount = smoothstep(0.0, 1.0, facing);
                amount *= lerp_f32(1.0, directional_amount, directionality);
            }
            if amount <= f32::EPSILON {
                continue;
            }
            let o = idx * 4;
            for c in 0..3 {
                out[o + c] = lerp_u8(src[o + c], color[c], amount);
            }
        }
    }
    progress(1.0);
    Ok(out)
}

fn apply_color_trace(src: &[u8], width: usize, height: usize, params: ColorTraceParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let radius = params.sample_radius_px.round().clamp(1.0, 64.0) as usize;
    if width == 0 || height == 0 || strength <= f32::EPSILON {
        return src.to_vec();
    }

    let len = width.saturating_mul(height);
    let threshold = params.line_threshold.clamp(0.02, 0.95);
    let softness = params.softness.clamp(0.001, 0.60);
    let darkness = params.darkness.clamp(0.0, 1.0);
    let saturation_scale = 1.0 + params.saturation.clamp(-1.0, 2.0);
    let mut line = vec![0.0; len];
    let mut weight = vec![0.0; len];
    let mut weighted_r = vec![0.0; len];
    let mut weighted_g = vec![0.0; len];
    let mut weighted_b = vec![0.0; len];

    for (idx, px) in src.chunks_exact(4).enumerate() {
        let alpha = px[3] as f32 / 255.0;
        if alpha <= f32::EPSILON {
            continue;
        }
        let r = px[0] as f32 / 255.0;
        let g = px[1] as f32 / 255.0;
        let b = px[2] as f32 / 255.0;
        let luma = luma01(r, g, b);
        let line_weight = (1.0 - smoothstep(threshold, threshold + softness, luma)) * alpha;
        let sample_weight = (1.0 - line_weight).max(0.04) * alpha;
        line[idx] = line_weight.clamp(0.0, 1.0);
        weight[idx] = sample_weight;
        weighted_r[idx] = r * sample_weight;
        weighted_g[idx] = g * sample_weight;
        weighted_b[idx] = b * sample_weight;
    }

    let blur_weight = box_blur_alpha(&weight, width, height, radius);
    let blur_r = box_blur_alpha(&weighted_r, width, height, radius);
    let blur_g = box_blur_alpha(&weighted_g, width, height, radius);
    let blur_b = box_blur_alpha(&weighted_b, width, height, radius);
    let mut out = src.to_vec();

    for idx in 0..len {
        let amount = (line[idx] * strength).clamp(0.0, 1.0);
        if amount <= f32::EPSILON {
            continue;
        }
        let o = idx * 4;
        let denom = blur_weight[idx];
        let mut sampled = if denom > 0.0001 {
            [
                (blur_r[idx] / denom).clamp(0.0, 1.0),
                (blur_g[idx] / denom).clamp(0.0, 1.0),
                (blur_b[idx] / denom).clamp(0.0, 1.0),
            ]
        } else {
            [
                src[o] as f32 / 255.0,
                src[o + 1] as f32 / 255.0,
                src[o + 2] as f32 / 255.0,
            ]
        };
        sampled = adjust_saturation(sampled, saturation_scale);
        let darken = 1.0 - darkness;
        for c in 0..3 {
            let target = sampled[c] * darken;
            out[o + c] = to_u8(lerp_f32(src[o + c] as f32 / 255.0, target, amount));
        }
    }
    out
}

fn normalized_pixel_coord(index: usize, size: usize) -> f32 {
    if size <= 1 {
        0.5
    } else {
        index as f32 / (size - 1) as f32
    }
}

fn rgb_u8_to_f32(rgb: [u8; 3]) -> [f32; 3] {
    [
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
    ]
}

fn color_shape_gradient_t(
    nx: f32,
    ny: f32,
    shape: ColorOverlayShape,
    angle_degrees: f32,
    linear_points_enabled: bool,
    linear_start: [f32; 2],
    linear_end: [f32; 2],
    center: [f32; 2],
    radius: f32,
    softness: f32,
) -> f32 {
    let raw = match shape {
        ColorOverlayShape::Unselected => 0.0,
        ColorOverlayShape::Solid => 0.0,
        ColorOverlayShape::Linear => {
            if linear_points_enabled {
                let sx = linear_start[0];
                let sy = linear_start[1];
                let dx = linear_end[0] - sx;
                let dy = linear_end[1] - sy;
                let denom = dx * dx + dy * dy;
                if denom <= f32::EPSILON {
                    1.0
                } else {
                    (((nx - sx) * dx + (ny - sy) * dy) / denom).clamp(0.0, 1.0)
                }
            } else {
                let angle = angle_degrees.to_radians();
                let dx = angle.cos();
                let dy = angle.sin();
                let span = 0.5 * (dx.abs() + dy.abs()).max(f32::EPSILON);
                let projected = (nx - 0.5) * dx + (ny - 0.5) * dy;
                (projected / (span * 2.0) + 0.5).clamp(0.0, 1.0)
            }
        }
        ColorOverlayShape::Radial => {
            let cx = center[0].clamp(0.0, 1.0);
            let cy = center[1].clamp(0.0, 1.0);
            let radius = radius.clamp(0.02, 2.0);
            let dx = nx - cx;
            let dy = ny - cy;
            (dx.hypot(dy) / radius).clamp(0.0, 1.0)
        }
    };
    let smooth = smoothstep(0.0, 1.0, raw);
    lerp_f32(raw, smooth, softness.clamp(0.0, 1.0))
}

fn color_fill_rgb(t: f32, params: ColorFillParams) -> [f32; 3] {
    let start = rgb_u8_to_f32(params.start_rgb);
    if matches!(
        params.shape,
        ColorOverlayShape::Unselected | ColorOverlayShape::Solid
    ) {
        return start;
    }
    let end = rgb_u8_to_f32(params.end_rgb);
    if params.middle_enabled {
        let middle = rgb_u8_to_f32(params.middle_rgb);
        sample_gradient_stops(
            t,
            &[
                (0.0, start),
                (params.midpoint.clamp(0.01, 0.99), middle),
                (1.0, end),
            ],
        )
    } else {
        [
            lerp_f32(start[0], end[0], t),
            lerp_f32(start[1], end[1], t),
            lerp_f32(start[2], end[2], t),
        ]
    }
}

fn color_overlay_blend_rgb(
    base: [f32; 3],
    overlay: [f32; 3],
    mode: ColorOverlayBlendMode,
) -> [f32; 3] {
    match mode {
        ColorOverlayBlendMode::Normal => overlay,
        ColorOverlayBlendMode::Multiply => [
            base[0] * overlay[0],
            base[1] * overlay[1],
            base[2] * overlay[2],
        ],
        ColorOverlayBlendMode::Screen => [
            screen_channel(base[0], overlay[0]),
            screen_channel(base[1], overlay[1]),
            screen_channel(base[2], overlay[2]),
        ],
        ColorOverlayBlendMode::Overlay => [
            overlay_blend_channel(base[0], overlay[0]),
            overlay_blend_channel(base[1], overlay[1]),
            overlay_blend_channel(base[2], overlay[2]),
        ],
        ColorOverlayBlendMode::SoftLight => [
            soft_light_channel(base[0], overlay[0]),
            soft_light_channel(base[1], overlay[1]),
            soft_light_channel(base[2], overlay[2]),
        ],
        ColorOverlayBlendMode::Color => {
            let (h, s, _) = rgb_to_hsl(overlay[0], overlay[1], overlay[2]);
            let (_, _, l) = rgb_to_hsl(base[0], base[1], base[2]);
            hsl_to_rgb(h, s, l)
        }
    }
}

fn screen_channel(base: f32, overlay: f32) -> f32 {
    (1.0 - (1.0 - base) * (1.0 - overlay)).clamp(0.0, 1.0)
}

fn color_dodge_channel(base: f32, blend: f32) -> f32 {
    let blend = blend.clamp(0.0, 0.98);
    if blend >= 0.98 {
        1.0
    } else {
        (base / (1.0 - blend)).clamp(0.0, 1.0)
    }
}

fn overlay_blend_channel(base: f32, overlay: f32) -> f32 {
    if base < 0.5 {
        2.0 * base * overlay
    } else {
        1.0 - 2.0 * (1.0 - base) * (1.0 - overlay)
    }
    .clamp(0.0, 1.0)
}

fn soft_light_channel(base: f32, overlay: f32) -> f32 {
    if overlay < 0.5 {
        base - (1.0 - 2.0 * overlay) * base * (1.0 - base)
    } else {
        base + (2.0 * overlay - 1.0) * (base.sqrt() - base)
    }
    .clamp(0.0, 1.0)
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

fn apply_neon_glow(src: &[u8], width: usize, height: usize, params: NeonGlowParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 2.0);
    let inner_radius = params.inner_radius_px.round().clamp(0.0, 96.0) as usize;
    let outer_radius = params.outer_radius_px.round().clamp(0.0, 180.0) as usize;
    if width == 0
        || height == 0
        || strength <= f32::EPSILON
        || (inner_radius == 0 && outer_radius == 0)
    {
        return src.to_vec();
    }

    let threshold = params.threshold.clamp(0.05, 0.999);
    let inner_amount = params.inner_amount.clamp(0.0, 2.0);
    let outer_amount = params.outer_amount.clamp(0.0, 2.0);
    let saturation_scale = 1.0 + params.glow_saturation.clamp(-1.0, 2.0);
    let tint = rgb_u8_to_f32(params.tint_rgb);
    let tint_strength = params.tint_strength.clamp(0.0, 1.0);
    let source_rgb = rgb_u8_to_f32(params.source_rgb);
    let source_tolerance = params.source_tolerance.clamp(0.0, 1.0);
    let source_feather = params.source_feather.clamp(0.001, 1.0);
    let mut bright = vec![0_u8; src.len()];

    for i in (0..src.len()).step_by(4) {
        let alpha = src[i + 3] as f32 / 255.0;
        if alpha <= f32::EPSILON {
            continue;
        }
        let r = src[i] as f32 / 255.0;
        let g = src[i + 1] as f32 / 255.0;
        let b = src[i + 2] as f32 / 255.0;
        let base_rgb = [r, g, b];
        let signal = neon_source_signal(base_rgb, params.by_saturation);
        let mut gate = smoothstep(threshold, (threshold + 0.32).min(1.0), signal);
        if params.source_color_enabled {
            gate *= color_range_gate(base_rgb, source_rgb, source_tolerance, source_feather);
        }
        if gate <= f32::EPSILON {
            continue;
        }
        let mut glow_rgb = adjust_saturation(base_rgb, saturation_scale);
        glow_rgb = [
            lerp_f32(glow_rgb[0], tint[0], tint_strength),
            lerp_f32(glow_rgb[1], tint[1], tint_strength),
            lerp_f32(glow_rgb[2], tint[2], tint_strength),
        ];
        for c in 0..3 {
            bright[i + c] = to_u8(glow_rgb[c] * gate * alpha);
        }
        bright[i + 3] = src[i + 3];
    }

    let inner = box_blur_rgba(&bright, width, height, inner_radius);
    let outer = box_blur_rgba(&bright, width, height, outer_radius);
    let mut out = src.to_vec();
    for i in (0..src.len()).step_by(4) {
        if src[i + 3] == 0 {
            continue;
        }
        for c in 0..3 {
            let base = src[i + c] as f32 / 255.0;
            let glow = ((inner[i + c] as f32 / 255.0) * inner_amount
                + (outer[i + c] as f32 / 255.0) * outer_amount)
                * strength;
            let target = if params.screen_blend {
                screen_channel(base, glow.clamp(0.0, 1.0))
            } else {
                (base + glow).clamp(0.0, 1.0)
            };
            out[i + c] = to_u8(target);
        }
    }
    out
}

fn neon_source_signal(rgb: [f32; 3], by_saturation: bool) -> f32 {
    let luma = luma01(rgb[0], rgb[1], rgb[2]);
    if !by_saturation {
        return luma;
    }
    let max_channel = rgb[0].max(rgb[1]).max(rgb[2]);
    let min_channel = rgb[0].min(rgb[1]).min(rgb[2]);
    let chroma = max_channel - min_channel;
    luma.max(max_channel * 0.72 + chroma * 0.28).clamp(0.0, 1.0)
}

fn color_range_gate(rgb: [f32; 3], target: [f32; 3], tolerance: f32, feather: f32) -> f32 {
    let dr = rgb[0] - target[0];
    let dg = rgb[1] - target[1];
    let db = rgb[2] - target[2];
    let dist = (dr * dr + dg * dg + db * db).sqrt() / 3.0_f32.sqrt();
    (1.0 - smoothstep(tolerance, (tolerance + feather).min(1.0), dist)).clamp(0.0, 1.0)
}

fn apply_diffuse_glow(
    src: &[u8],
    width: usize,
    height: usize,
    params: DiffuseGlowParams,
) -> Vec<u8> {
    let radius = params.radius_px.round().clamp(0.0, 120.0) as usize;
    let strength = params.strength.clamp(0.0, 2.0);
    if radius == 0 || strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }
    let threshold = params.threshold.clamp(0.0, 0.999);
    let white_mix = params.white_mix.clamp(0.0, 1.0);
    let grain = params.grain.clamp(0.0, 1.0);
    let mut bright = vec![0_u8; src.len()];
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let r = src[i] as f32 / 255.0;
            let g = src[i + 1] as f32 / 255.0;
            let b = src[i + 2] as f32 / 255.0;
            let luma = luma01(r, g, b);
            let gate = smoothstep(threshold, (threshold + 0.35).min(1.0), luma);
            let noise = signed_noise(x as u32, y as u32, params.seed);
            let grain_weight = (1.0 + noise * grain * 0.55).clamp(0.0, 1.75);
            let source = [
                r + (1.0 - r) * white_mix,
                g + (1.0 - g) * white_mix,
                b + (1.0 - b) * white_mix,
            ];
            for c in 0..3 {
                bright[i + c] = to_u8(source[c] * gate * grain_weight);
            }
            bright[i + 3] = src[i + 3];
        }
    }

    let glow = box_blur_rgba(&bright, width, height, radius);
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let r = src[i] as f32 / 255.0;
            let g = src[i + 1] as f32 / 255.0;
            let b = src[i + 2] as f32 / 255.0;
            let luma = luma01(r, g, b);
            let highlight = smoothstep(
                (threshold * 0.75).clamp(0.0, 0.98),
                (threshold + 0.18).min(1.0),
                luma,
            );
            let noise = signed_noise(x as u32, y as u32, params.seed ^ 0xA53A_9E37);
            let grain_delta = noise * grain * highlight * strength * 0.06;
            let veil = white_mix * highlight * strength * 0.18;
            for c in 0..3 {
                let base = src[i + c] as f32 / 255.0;
                let glow_add = (glow[i + c] as f32 / 255.0 * strength).clamp(0.0, 1.0);
                let screened = 1.0 - (1.0 - base) * (1.0 - glow_add);
                let target = screened + (1.0 - screened) * veil + grain_delta;
                out[i + c] = to_u8(target);
            }
        }
    }
    out
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

fn apply_halation(src: &[u8], width: usize, height: usize, params: HalationParams) -> Vec<u8> {
    let radius = params.radius_px.round().clamp(0.0, 180.0) as usize;
    let strength = params.strength.clamp(0.0, 2.0);
    if radius == 0 || strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let threshold = params.threshold.clamp(0.05, 0.995);
    let warmth = params.warmth.clamp(0.0, 1.0);
    let edge_bias = params.edge_bias.clamp(0.0, 1.0);
    let tint = rgb_u8_to_f32(params.tint_rgb);
    let len = width.saturating_mul(height);
    let mut luma = vec![0.0; len];
    for (idx, px) in src.chunks_exact(4).enumerate() {
        luma[idx] = luma01(
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        );
    }

    let mut bright = vec![0_u8; src.len()];
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let i = idx * 4;
            let alpha = src[i + 3] as f32 / 255.0;
            if alpha <= f32::EPSILON {
                continue;
            }
            let gate = smoothstep(threshold, (threshold + 0.30).min(1.0), luma[idx]);
            if gate <= f32::EPSILON {
                continue;
            }
            let edge = halation_edge_signal(&luma, width, height, x, y);
            let edge_weight = lerp_f32(1.0, edge, edge_bias);
            let weight = gate * edge_weight * alpha;
            if weight <= f32::EPSILON {
                continue;
            }
            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let white_mix = 0.24 + warmth * 0.38;
            let warmed = [
                base[0] + (1.0 - base[0]) * white_mix,
                base[1] + (1.0 - base[1]) * white_mix,
                base[2] + (1.0 - base[2]) * white_mix,
            ];
            for c in 0..3 {
                let source = lerp_f32(warmed[c], tint[c], warmth * 0.75);
                bright[i + c] = to_u8(source * weight);
            }
            bright[i + 3] = src[i + 3];
        }
    }

    let glow = box_blur_rgba(&bright, width, height, radius);
    let mut out = src.to_vec();
    for i in (0..src.len()).step_by(4) {
        if src[i + 3] == 0 {
            continue;
        }
        for c in 0..3 {
            let base = src[i + c] as f32 / 255.0;
            let add = (glow[i + c] as f32 / 255.0 * strength).clamp(0.0, 1.0);
            let target = if params.screen_blend {
                screen_channel(base, add)
            } else {
                (base + add).clamp(0.0, 1.0)
            };
            out[i + c] = to_u8(target);
        }
    }
    out
}

fn apply_color_dodge_glow(
    src: &[u8],
    width: usize,
    height: usize,
    params: ColorDodgeGlowParams,
) -> Vec<u8> {
    let radius = params.radius_px.round().clamp(0.0, 180.0) as usize;
    let strength = params.strength.clamp(0.0, 2.0);
    if radius == 0 || strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let threshold = params.threshold.clamp(0.0, 0.995);
    let tint = rgb_u8_to_f32(params.color_rgb);
    let color_strength = params.color_strength.clamp(0.0, 1.0);
    let mut bright = vec![0_u8; src.len()];
    for i in (0..src.len()).step_by(4) {
        let alpha = src[i + 3] as f32 / 255.0;
        if alpha <= f32::EPSILON {
            continue;
        }
        let base = [
            src[i] as f32 / 255.0,
            src[i + 1] as f32 / 255.0,
            src[i + 2] as f32 / 255.0,
        ];
        let luma = luma01(base[0], base[1], base[2]);
        let gate = smoothstep(threshold, (threshold + 0.32).min(1.0), luma) * alpha;
        if gate <= f32::EPSILON {
            continue;
        }
        let glow_rgb = [
            lerp_f32(base[0], tint[0], color_strength),
            lerp_f32(base[1], tint[1], color_strength),
            lerp_f32(base[2], tint[2], color_strength),
        ];
        for c in 0..3 {
            bright[i + c] = to_u8(glow_rgb[c] * gate);
        }
        bright[i + 3] = src[i + 3];
    }

    let glow = box_blur_rgba(&bright, width, height, radius);
    let dodge_amount = params.dodge_amount.clamp(0.0, 1.0);
    let mut out = src.to_vec();
    for i in (0..src.len()).step_by(4) {
        for c in 0..3 {
            let base = src[i + c] as f32 / 255.0;
            let glow_signal = (glow[i + c] as f32 / 255.0 * strength).clamp(0.0, 1.0);
            if glow_signal <= f32::EPSILON {
                continue;
            }
            let screened = screen_channel(base, glow_signal);
            let dodged = color_dodge_channel(base, glow_signal * dodge_amount);
            let target = (screened + (dodged - base).max(0.0) * dodge_amount).clamp(0.0, 1.0);
            out[i + c] = to_u8(target);
        }
    }
    out
}

fn halation_edge_signal(luma: &[f32], width: usize, height: usize, x: usize, y: usize) -> f32 {
    let idx = y * width + x;
    let center = luma[idx];
    let left = luma[y * width + x.saturating_sub(1)];
    let right = luma[y * width + (x + 1).min(width - 1)];
    let top = luma[y.saturating_sub(1) * width + x];
    let bottom = luma[(y + 1).min(height - 1) * width + x];
    let delta = (center - left)
        .abs()
        .max((center - right).abs())
        .max((center - top).abs())
        .max((center - bottom).abs());
    smoothstep(0.02, 0.22, delta)
}

fn apply_god_rays(src: &[u8], width: usize, height: usize, params: GodRaysParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 3.0);
    let length = params.length_px.clamp(1.0, 360.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }
    let threshold = params.threshold.clamp(0.0, 0.999);
    let inv_range = 1.0 / (1.0 - threshold).max(0.001);
    let warm_tint = params.warm_tint.clamp(0.0, 1.0);
    let warm = [1.0, 0.84, 0.54];
    let mut bright = vec![0_u8; src.len()];
    for i in (0..src.len()).step_by(4) {
        let alpha = src[i + 3] as f32 / 255.0;
        if alpha <= f32::EPSILON {
            continue;
        }
        let base = [
            src[i] as f32 / 255.0,
            src[i + 1] as f32 / 255.0,
            src[i + 2] as f32 / 255.0,
        ];
        let gate = ((luma01(base[0], base[1], base[2]) - threshold) * inv_range)
            .clamp(0.0, 1.0)
            .powf(1.35);
        if gate <= 0.001 {
            continue;
        }
        let source = [
            lerp_f32(base[0], warm[0], warm_tint),
            lerp_f32(base[1], warm[1], warm_tint),
            lerp_f32(base[2], warm[2], warm_tint),
        ];
        for c in 0..3 {
            bright[i + c] = to_u8(source[c] * gate * alpha);
        }
        bright[i + 3] = src[i + 3];
    }

    let bright = box_blur_rgba(&bright, width, height, 2);
    let cx = params.center[0].clamp(0.0, 1.0) * width.saturating_sub(1) as f32;
    let cy = params.center[1].clamp(0.0, 1.0) * height.saturating_sub(1) as f32;
    let max_steps = length.round().clamp(1.0, 180.0) as usize;
    let step_px = length / max_steps as f32;
    let decay = params.decay.clamp(0.0, 1.0);
    let mut rays = vec![0.0_f32; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if bright[i] == 0 && bright[i + 1] == 0 && bright[i + 2] == 0 {
                continue;
            }
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = dx.hypot(dy);
            if dist <= 0.5 {
                continue;
            }
            let dir_x = dx / dist;
            let dir_y = dy / dist;
            let color = [
                bright[i] as f32 / 255.0,
                bright[i + 1] as f32 / 255.0,
                bright[i + 2] as f32 / 255.0,
            ];
            for step in 1..=max_steps {
                let distance = step as f32 * step_px;
                let sx = x as f32 + dir_x * distance;
                let sy = y as f32 + dir_y * distance;
                if sx < -0.001
                    || sy < -0.001
                    || sx > width as f32 - 1.0 + 0.001
                    || sy > height as f32 - 1.0 + 0.001
                {
                    break;
                }
                let linear = (1.0 - distance / length).max(0.0);
                let falloff = linear * linear * decay.powf(distance / 18.0);
                add_bilinear_rgb(&mut rays, width, height, sx, sy, color, falloff);
            }
        }
    }

    let mut out = src.to_vec();
    let scale = strength * 0.22;
    for i in 0..width * height {
        let si = i * 3;
        let oi = i * 4;
        if src[oi + 3] == 0 {
            continue;
        }
        for c in 0..3 {
            let base = src[oi + c] as f32 / 255.0;
            let ray = (rays[si + c] * scale).clamp(0.0, 1.0);
            out[oi + c] = to_u8(screen_channel(base, ray));
        }
    }
    out
}

fn apply_lens_flare(src: &[u8], width: usize, height: usize, params: LensFlareParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 3.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }
    let radius = params.radius_px.clamp(4.0, 420.0);
    let core_strength = params.core_strength.clamp(0.0, 2.0);
    let halo_strength = params.halo_strength.clamp(0.0, 2.0);
    let ghost_strength = params.ghost_strength.clamp(0.0, 2.0);
    let streak_strength = params.streak_strength.clamp(0.0, 2.0);
    let warm_tint = params.warm_tint.clamp(0.0, 1.0);
    let warm = [1.0, 0.84, 0.54];
    let cool = [0.50, 0.78, 1.0];
    let light_rgb = [
        lerp_f32(0.90, warm[0], warm_tint),
        lerp_f32(0.94, warm[1], warm_tint),
        lerp_f32(1.00, warm[2], warm_tint),
    ];
    let cx = params.center[0].clamp(0.0, 1.0) * width.saturating_sub(1) as f32;
    let cy = params.center[1].clamp(0.0, 1.0) * height.saturating_sub(1) as f32;
    let mx = width.saturating_sub(1) as f32 * 0.5;
    let my = height.saturating_sub(1) as f32 * 0.5;
    let axis_x = mx - cx;
    let axis_y = my - cy;
    let axis_len = axis_x.hypot(axis_y);
    let (dir_x, dir_y) = if axis_len > 0.001 {
        (axis_x / axis_len, axis_y / axis_len)
    } else {
        (1.0, 0.0)
    };
    let diag = (width.saturating_sub(1) as f32).hypot(height.saturating_sub(1) as f32);
    let ghosts = [
        (0.38_f32, 0.20_f32, [0.55_f32, 0.82_f32, 1.00_f32], 0.42_f32),
        (0.66_f32, 0.11_f32, [1.00_f32, 0.54_f32, 0.86_f32], 0.30_f32),
        (0.94_f32, 0.16_f32, [0.72_f32, 1.00_f32, 0.62_f32], 0.34_f32),
        (1.18_f32, 0.24_f32, [1.00_f32, 0.76_f32, 0.42_f32], 0.24_f32),
    ];
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }
            let px = x as f32;
            let py = y as f32;
            let dx = px - cx;
            let dy = py - cy;
            let d = dx.hypot(dy);
            let mut add = [0.0_f32; 3];

            if core_strength > 0.0 {
                let core_radius = (radius * 0.22).max(2.0);
                let core = radial_falloff(d, core_radius).powf(2.4) * core_strength * 1.15;
                let glow = radial_falloff(d, radius).powf(2.0) * core_strength * 0.36;
                let amount = (core + glow) * strength;
                for c in 0..3 {
                    add[c] += light_rgb[c] * amount;
                }
            }

            if halo_strength > 0.0 {
                let halo_radius = radius * 0.58;
                let ring_width = (radius * 0.16).max(2.0);
                let ring = radial_ring(d, halo_radius, ring_width).powf(1.7);
                let soft_halo = radial_falloff(d, radius * 1.12).powf(2.6) * 0.18;
                let amount = (ring * 0.52 + soft_halo) * halo_strength * strength;
                for c in 0..3 {
                    let color = lerp_f32(cool[c], warm[c], warm_tint * 0.55);
                    add[c] += color * amount;
                }
            }

            if streak_strength > 0.0 {
                let along = dx.abs() / (radius * 1.7).max(1.0);
                let across = dy.abs() / (radius * 0.035).max(1.2);
                let streak = (1.0 - along).clamp(0.0, 1.0).powf(1.35) * (-across * across).exp();
                let amount = streak * streak_strength * strength * 0.42;
                for c in 0..3 {
                    add[c] += light_rgb[c] * amount;
                }
            }

            if ghost_strength > 0.0 {
                for &(offset, size_scale, ghost_rgb, amount_scale) in &ghosts {
                    let gx = cx + dir_x * diag * offset;
                    let gy = cy + dir_y * diag * offset;
                    let gd = (px - gx).hypot(py - gy);
                    let ghost_radius = (radius * size_scale).max(2.0);
                    let disc = radial_falloff(gd, ghost_radius).powf(2.2);
                    let ring =
                        radial_ring(gd, ghost_radius * 0.72, ghost_radius * 0.22).powf(1.5) * 0.55;
                    let amount = (disc * 0.40 + ring) * ghost_strength * strength * amount_scale;
                    for c in 0..3 {
                        let color = lerp_f32(ghost_rgb[c], warm[c], warm_tint * 0.35);
                        add[c] += color * amount;
                    }
                }
            }

            for c in 0..3 {
                let base = src[i + c] as f32 / 255.0;
                out[i + c] = to_u8(screen_channel(base, add[c].clamp(0.0, 1.0)));
            }
        }
    }
    out
}

fn apply_anamorphic_flare(
    src: &[u8],
    width: usize,
    height: usize,
    params: AnamorphicFlareParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 3.0);
    let length = params.length_px.round().clamp(1.0, 480.0) as usize;
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let threshold = params.threshold.clamp(0.0, 0.995);
    let thickness = params.thickness_px.round().clamp(0.0, 48.0) as usize;
    let tint = rgb_u8_to_f32(params.color_rgb);
    let color_strength = params.color_strength.clamp(0.0, 1.0);
    let mut bright = vec![0.0_f32; width.saturating_mul(height).saturating_mul(3)];
    for idx in 0..width * height {
        let si = idx * 4;
        let di = idx * 3;
        let alpha = src[si + 3] as f32 / 255.0;
        if alpha <= f32::EPSILON {
            continue;
        }
        let rgb = [
            src[si] as f32 / 255.0,
            src[si + 1] as f32 / 255.0,
            src[si + 2] as f32 / 255.0,
        ];
        let luma = luma01(rgb[0], rgb[1], rgb[2]);
        let gate = smoothstep(threshold, (threshold + 0.28).min(1.0), luma).powf(1.25) * alpha;
        if gate <= 0.001 {
            continue;
        }
        for c in 0..3 {
            let color = lerp_f32(rgb[c], tint[c], color_strength);
            bright[di + c] = color * gate;
        }
    }

    let streak = horizontal_streak_rgb_f32(&bright, width, height, length);
    let streak = if thickness > 0 {
        vertical_blur_rgb_f32(&streak, width, height, thickness)
    } else {
        streak
    };
    let mut out = src.to_vec();
    let scale = strength * 0.85;
    for idx in 0..width * height {
        let si = idx * 3;
        let oi = idx * 4;
        for c in 0..3 {
            let base = src[oi + c] as f32 / 255.0;
            let flare = (streak[si + c] * scale).clamp(0.0, 1.0);
            out[oi + c] = to_u8(screen_channel(base, flare));
        }
    }
    out
}

fn apply_light_leak(src: &[u8], width: usize, height: usize, params: LightLeakParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let intensity = params.intensity.clamp(0.0, 2.0);
    if strength <= f32::EPSILON || intensity <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let color = rgb_u8_to_f32(params.color_rgb);
    let radius = params.radius.clamp(0.05, 1.6);
    let falloff = params.falloff.clamp(0.35, 6.0);
    let haze = params.haze.clamp(0.0, 1.0);
    let streak_strength = params.streak_strength.clamp(0.0, 1.0);
    let angle = params.streak_angle_degrees.to_radians();
    let dir = (angle.cos(), angle.sin());
    let perp = (-dir.1, dir.0);
    let cx = params.center[0].clamp(-0.5, 1.5) * width.saturating_sub(1).max(1) as f32;
    let cy = params.center[1].clamp(-0.5, 1.5) * height.saturating_sub(1).max(1) as f32;
    let diag = (width as f32).hypot(height as f32).max(1.0);
    let radius_px = radius * diag;
    let mut out = src.to_vec();

    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }

            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = dx.hypot(dy);
            let radial = (1.0 - dist / radius_px).clamp(0.0, 1.0).powf(falloff);
            let broad = (1.0 - dist / (radius_px * 1.85)).clamp(0.0, 1.0).powf(1.25) * haze;
            let along = dx * dir.0 + dy * dir.1;
            let across = dx * perp.0 + dy * perp.1;
            let streak_decay = (1.0 - along.abs() / (radius_px * 1.28).max(1.0)).clamp(0.0, 1.0);
            let stripe = ((across / 19.0)
                + glass_value_noise(x as f32 / 42.0, y as f32 / 42.0, params.seed) * 1.2)
                .sin()
                .abs();
            let stripe = 1.0 - smoothstep(0.20, 0.72, stripe);
            let streak = stripe * streak_decay.powf(1.6) * streak_strength;
            let noise = glass_value_noise(
                x as f32 / 24.0 + 11.0,
                y as f32 / 24.0 - 7.0,
                params.seed ^ 0x1EAF_5EED,
            );
            let leak = ((radial + broad + streak) * intensity * strength)
                * (1.0 + noise * 0.10 * (haze + streak_strength).clamp(0.0, 1.0));
            let leak = leak.clamp(0.0, 1.0);
            if leak <= f32::EPSILON {
                continue;
            }

            let warm_edge = radial.powf(0.55) * 0.22 + streak * 0.12;
            let overlay = [
                (color[0] * leak + warm_edge).clamp(0.0, 1.0),
                (color[1] * leak + warm_edge * 0.32).clamp(0.0, 1.0),
                (color[2] * leak).clamp(0.0, 1.0),
            ];
            for c in 0..3 {
                let base = src[i + c] as f32 / 255.0;
                out[i + c] = to_u8(screen_channel(base, overlay[c]));
            }
        }
    }

    out
}

fn apply_backlight_haze(
    src: &[u8],
    width: usize,
    height: usize,
    params: BacklightHazeParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let color = rgb_u8_to_f32(params.color_rgb);
    let radius = params.radius.clamp(0.05, 1.6);
    let falloff = params.falloff.clamp(0.35, 5.0);
    let haze = params.haze.clamp(0.0, 1.0);
    let glow = params.glow.clamp(0.0, 2.0);
    let shadow_lift = params.shadow_lift.clamp(0.0, 1.0);
    let contrast_fade = params.contrast_fade.clamp(0.0, 1.0);
    let saturation_fade = params.saturation_fade.clamp(0.0, 1.0);
    if haze <= f32::EPSILON
        && glow <= f32::EPSILON
        && shadow_lift <= f32::EPSILON
        && contrast_fade <= f32::EPSILON
        && saturation_fade <= f32::EPSILON
    {
        return src.to_vec();
    }

    let cx = params.center[0].clamp(0.0, 1.0) * width.saturating_sub(1).max(1) as f32;
    let cy = params.center[1].clamp(0.0, 1.0) * height.saturating_sub(1).max(1) as f32;
    let diag = (width as f32).hypot(height as f32).max(1.0);
    let radius_px = radius * diag;
    let mut out = src.to_vec();

    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }

            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let luma = luma01(base[0], base[1], base[2]);
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = dx.hypot(dy);
            let radial = (1.0 - dist / radius_px).clamp(0.0, 1.0).powf(falloff);
            let broad = (1.0 - dist / (radius_px * 1.55))
                .clamp(0.0, 1.0)
                .powf((falloff * 0.55).max(0.35));
            let air = (radial * 0.68 + broad * 0.32).clamp(0.0, 1.0);
            if air <= f32::EPSILON {
                continue;
            }

            let mut target = base;
            let contrast_amount = contrast_fade * air;
            for channel in &mut target {
                *channel = lerp_f32(0.5, *channel, 1.0 - contrast_amount * 0.72);
            }
            let faded_luma = luma01(target[0], target[1], target[2]);
            for channel in &mut target {
                *channel = lerp_f32(*channel, faded_luma, saturation_fade * air * 0.85);
            }

            let shadow = (1.0 - luma).powf(0.72) * shadow_lift * air;
            let haze_amount = haze * air * (0.42 + (1.0 - luma) * 0.28);
            let glow_amount = glow * radial.powf(0.62) * (0.28 + luma * 0.72);
            for c in 0..3 {
                target[c] = screen_channel(target[c], color[c] * shadow * 0.70);
                target[c] = screen_channel(target[c], color[c] * haze_amount * 0.78);
                target[c] = screen_channel(target[c], color[c] * glow_amount * 0.60);
                out[i + c] = to_u8(lerp_f32(base[c], target[c], strength));
            }
        }
    }

    out
}

fn horizontal_streak_rgb_f32(src: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    if radius == 0 || width == 0 || height == 0 {
        return src.to_vec();
    }
    let mut out = vec![0.0_f32; src.len()];
    for y in 0..height {
        let mut prefix = vec![[0.0_f32; 3]; width + 1];
        for x in 0..width {
            let si = (y * width + x) * 3;
            for c in 0..3 {
                prefix[x + 1][c] = prefix[x][c] + src[si + c];
            }
        }
        for x in 0..width {
            let x0 = x.saturating_sub(radius);
            let x1 = (x + radius).min(width - 1);
            let count = (x1 - x0 + 1) as f32;
            let normalizer = count.powf(0.55).max(1.0);
            let oi = (y * width + x) * 3;
            for c in 0..3 {
                out[oi + c] = (prefix[x1 + 1][c] - prefix[x0][c]) / normalizer;
            }
        }
    }
    out
}

fn vertical_blur_rgb_f32(src: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    if radius == 0 || width == 0 || height == 0 {
        return src.to_vec();
    }
    let mut out = vec![0.0_f32; src.len()];
    for x in 0..width {
        let mut prefix = vec![[0.0_f32; 3]; height + 1];
        for y in 0..height {
            let si = (y * width + x) * 3;
            for c in 0..3 {
                prefix[y + 1][c] = prefix[y][c] + src[si + c];
            }
        }
        for y in 0..height {
            let y0 = y.saturating_sub(radius);
            let y1 = (y + radius).min(height - 1);
            let count = (y1 - y0 + 1) as f32;
            let oi = (y * width + x) * 3;
            for c in 0..3 {
                out[oi + c] = (prefix[y1 + 1][c] - prefix[y0][c]) / count;
            }
        }
    }
    out
}

fn radial_falloff(distance: f32, radius: f32) -> f32 {
    (1.0 - distance / radius.max(0.001)).clamp(0.0, 1.0)
}

fn radial_ring(distance: f32, radius: f32, width: f32) -> f32 {
    (1.0 - (distance - radius).abs() / width.max(0.001)).clamp(0.0, 1.0)
}

fn apply_speed_lines(src: &[u8], width: usize, height: usize, params: SpeedLinesParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }
    let color = [
        params.color_rgb[0] as f32 / 255.0,
        params.color_rgb[1] as f32 / 255.0,
        params.color_rgb[2] as f32 / 255.0,
    ];
    let line_count = params.line_count.clamp(4, 360);
    let line_width = params.line_width_px.clamp(0.25, 32.0);
    let softness_px = (line_width * params.softness.clamp(0.0, 1.0) * 2.5).max(0.35);
    let mut inner = params.inner_radius.clamp(0.0, 1.0);
    let mut outer = params.outer_radius.clamp(0.0, 1.0);
    if outer < inner {
        std::mem::swap(&mut inner, &mut outer);
    }
    outer = outer.max(inner + 0.001).min(1.0);
    let length = params.length.clamp(0.05, 1.0);
    let cx = params.center[0].clamp(0.0, 1.0) * width.saturating_sub(1) as f32;
    let cy = params.center[1].clamp(0.0, 1.0) * height.saturating_sub(1) as f32;
    let max_dist = farthest_corner_distance(width, height, cx, cy).max(1.0);
    let mut out = src.to_vec();
    match params.mode {
        SpeedLinesMode::Radial => {
            for y in 0..height {
                for x in 0..width {
                    let px = x as f32 + 0.5;
                    let py = y as f32 + 0.5;
                    let dx = px - cx;
                    let dy = py - cy;
                    let dist = dx.hypot(dy);
                    if dist <= 0.25 {
                        continue;
                    }
                    let dist_norm = (dist / max_dist).clamp(0.0, 1.0);
                    let angle = dy.atan2(dx);
                    let turns = (angle / std::f32::consts::TAU).rem_euclid(1.0);
                    let line_pos = turns * line_count as f32;
                    let nearest_line = line_pos.round();
                    let line_index = (nearest_line as i32).rem_euclid(line_count as i32) as u32;
                    let line_center_noise =
                        signed_noise(line_index, 0, params.seed ^ 0x5FEE_D1A5) * 0.20;
                    let centered = (line_pos - nearest_line - line_center_noise).abs();
                    let angular_gap = centered / line_count as f32 * std::f32::consts::TAU;
                    let distance_to_line = angular_gap * dist;
                    let line =
                        1.0 - smoothstep(line_width, line_width + softness_px, distance_to_line);
                    if line <= f32::EPSILON {
                        continue;
                    }
                    let length_noise =
                        (0.72 + signed_noise(line_index, 1, params.seed) * 0.28).clamp(0.35, 1.0);
                    let span = (outer - inner) * length * length_noise;
                    let start = (outer - span).max(inner);
                    let radial = smoothstep(start, (start + 0.04).min(outer), dist_norm)
                        * (1.0 - smoothstep((outer - 0.05).max(start), outer, dist_norm));
                    let intensity_noise = (0.76
                        + signed_noise(line_index, 2, params.seed ^ 0xB1A5_E111) * 0.24)
                        .clamp(0.35, 1.0);
                    let amount = (line * radial * intensity_noise * strength).clamp(0.0, 1.0);
                    if amount > f32::EPSILON {
                        blend_speed_line_pixel(&mut out, src, (y * width + x) * 4, color, amount);
                    }
                }
            }
        }
        SpeedLinesMode::Parallel => {
            let angle = params.angle_degrees.to_radians();
            let dir_x = angle.cos();
            let dir_y = angle.sin();
            let perp_x = -dir_y;
            let perp_y = dir_x;
            let diag = (width as f32).hypot(height as f32).max(1.0);
            let period = (diag / line_count as f32).max(line_width + softness_px + 0.5);
            let origin = diag * 0.5;
            for y in 0..height {
                for x in 0..width {
                    let px = x as f32 + 0.5 - cx;
                    let py = y as f32 + 0.5 - cy;
                    let across = px * perp_x + py * perp_y + origin;
                    let line_pos = across / period;
                    let line_index = line_pos.round() as i32;
                    let jitter = signed_noise(line_index as u32, 0, params.seed) * 0.18;
                    let centered = (line_pos - line_index as f32 - jitter).abs();
                    let distance_to_line = centered * period;
                    let line =
                        1.0 - smoothstep(line_width, line_width + softness_px, distance_to_line);
                    if line <= f32::EPSILON {
                        continue;
                    }
                    let along = px * dir_x + py * dir_y;
                    let length_noise = (0.70
                        + signed_noise(line_index as u32, 1, params.seed) * 0.30)
                        .clamp(0.30, 1.0);
                    let half = diag * 0.5 * length * length_noise;
                    let offset = signed_noise(line_index as u32, 2, params.seed ^ 0x51E2_D71A)
                        * diag
                        * (1.0 - length)
                        * 0.45;
                    let longitudinal =
                        1.0 - smoothstep(half * 0.78, half.max(1.0), (along - offset).abs());
                    let edge = {
                        let edge_x =
                            (x as f32 / width.saturating_sub(1).max(1) as f32 - 0.5).abs() * 2.0;
                        let edge_y =
                            (y as f32 / height.saturating_sub(1).max(1) as f32 - 0.5).abs() * 2.0;
                        smoothstep(inner, outer, edge_x.max(edge_y).clamp(0.0, 1.0))
                    };
                    let intensity_noise = (0.72
                        + signed_noise(line_index as u32, 3, params.seed ^ 0xA71C_2027) * 0.28)
                        .clamp(0.35, 1.0);
                    let amount =
                        (line * longitudinal * edge * intensity_noise * strength).clamp(0.0, 1.0);
                    if amount > f32::EPSILON {
                        blend_speed_line_pixel(&mut out, src, (y * width + x) * 4, color, amount);
                    }
                }
            }
        }
    }
    out
}

fn blend_speed_line_pixel(out: &mut [u8], src: &[u8], i: usize, color: [f32; 3], amount: f32) {
    for c in 0..3 {
        let base = src[i + c] as f32 / 255.0;
        out[i + c] = to_u8(lerp_f32(base, color[c], amount));
    }
}

fn apply_radial_flash(
    src: &[u8],
    width: usize,
    height: usize,
    params: RadialFlashParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let ray_count = params.ray_count.clamp(4, 240);
    let cx = params.center[0].clamp(0.0, 1.0) * width.saturating_sub(1) as f32;
    let cy = params.center[1].clamp(0.0, 1.0) * height.saturating_sub(1) as f32;
    let max_dist = farthest_corner_distance(width, height, cx, cy).max(1.0);
    let mut inner = params.inner_radius.clamp(0.0, 1.0);
    let mut outer = params.outer_radius.clamp(0.0, 1.0);
    if outer < inner {
        std::mem::swap(&mut inner, &mut outer);
    }
    outer = outer.max(inner + 0.001).min(1.0);
    let softness = params.softness.clamp(0.0, 1.0);
    let angular_softness = (0.006 + softness * 0.18).min(0.45);
    let radial_softness = (0.015 + softness * 0.18).min(0.35);
    let white_amount = params.white_amount.clamp(0.0, 1.0);
    let black_amount = params.black_amount.clamp(0.0, 1.0);
    let rotation = params.rotation_degrees.to_radians();
    let mut out = src.to_vec();

    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let alpha = src[i + 3] as f32 / 255.0;
            if alpha <= f32::EPSILON {
                continue;
            }
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let dx = px - cx;
            let dy = py - cy;
            let dist_norm = (dx.hypot(dy) / max_dist).clamp(0.0, 1.0);
            let radial = smoothstep(inner, (inner + radial_softness).min(outer), dist_norm)
                * (1.0 - smoothstep((outer - radial_softness).max(inner), outer, dist_norm));
            if radial <= f32::EPSILON {
                continue;
            }

            let angle = (dy.atan2(dx) - rotation) / std::f32::consts::TAU;
            let phase = angle.rem_euclid(1.0) * ray_count as f32;
            let sector = phase.floor() as u32;
            let in_sector = phase - sector as f32;
            let edge_distance = in_sector.min(1.0 - in_sector);
            let sector_fill = smoothstep(0.0, angular_softness, edge_distance);
            if sector_fill <= f32::EPSILON {
                continue;
            }

            let mut white_sector = sector % 2 == 0;
            if params.invert {
                white_sector = !white_sector;
            }
            let amount = radial * sector_fill * strength * alpha;
            if white_sector {
                for c in 0..3 {
                    let base = src[i + c] as f32 / 255.0;
                    out[i + c] = to_u8(lerp_f32(base, 1.0, amount * white_amount));
                }
            } else {
                for c in 0..3 {
                    let base = src[i + c] as f32 / 255.0;
                    out[i + c] = to_u8(lerp_f32(base, 0.0, amount * black_amount));
                }
            }
        }
    }
    out
}

fn apply_cloud_fog(src: &[u8], width: usize, height: usize, params: CloudFogParams) -> Vec<u8> {
    let opacity = params.opacity.clamp(0.0, 1.0);
    if opacity <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }
    let scale = params.scale_px.clamp(8.0, 640.0);
    let detail = params.detail.clamp(0.0, 1.0);
    let density = params.density.clamp(0.0, 1.0);
    let contrast = params.contrast.clamp(0.0, 1.0);
    let height_fade = params.height_fade.clamp(-1.0, 1.0);
    let color = [
        params.color_rgb[0] as f32 / 255.0,
        params.color_rgb[1] as f32 / 255.0,
        params.color_rgb[2] as f32 / 255.0,
    ];
    let mut out = src.to_vec();
    for y in 0..height {
        let vertical = cloud_fog_vertical_weight(y, height, height_fade);
        if vertical <= f32::EPSILON {
            continue;
        }
        for x in 0..width {
            let i = (y * width + x) * 4;
            let alpha = src[i + 3] as f32 / 255.0;
            if alpha <= f32::EPSILON {
                continue;
            }
            let noise = cloud_fog_noise(x as f32, y as f32, scale, detail, params.seed);
            let shaped = ((noise - 0.5) * (1.0 + contrast * 3.0) + 0.5 + (density - 0.5) * 0.55)
                .clamp(0.0, 1.0);
            let coverage = match params.mode {
                CloudFogMode::Fog => (0.35 + shaped * 0.65) * (0.25 + density * 0.75),
                CloudFogMode::Clouds => {
                    let threshold = (0.92 - density * 0.70).clamp(0.12, 0.94);
                    let billow = (1.0 - (shaped * 2.0 - 1.0).abs()).powf(0.65);
                    let cloud = smoothstep(threshold, 1.0, shaped);
                    (cloud * 0.72 + billow * cloud * 0.28).clamp(0.0, 1.0)
                }
            };
            let amount = (coverage * vertical * opacity * alpha).clamp(0.0, 1.0);
            if amount <= f32::EPSILON {
                continue;
            }
            for c in 0..3 {
                let base = src[i + c] as f32 / 255.0;
                out[i + c] = to_u8(lerp_f32(base, color[c], amount));
            }
        }
    }
    out
}

fn cloud_fog_noise(x: f32, y: f32, scale: f32, detail: f32, seed: u32) -> f32 {
    let u = x / scale;
    let v = y / scale;
    let fine = detail.clamp(0.0, 1.0);
    let coarse = glass_value_noise(u, v, seed);
    let mid = glass_value_noise(u * 2.17 + 13.4, v * 2.17 - 7.1, seed ^ 0x7F4A_7C15);
    let high = glass_value_noise(u * 4.63 - 5.7, v * 4.63 + 19.2, seed ^ 0xC10D_F06A);
    let weight_mid = fine * 0.58;
    let weight_high = fine * fine * 0.28;
    let sum = coarse + mid * weight_mid + high * weight_high;
    let denom = 1.0 + weight_mid + weight_high;
    (sum / denom * 0.5 + 0.5).clamp(0.0, 1.0)
}

fn cloud_fog_vertical_weight(y: usize, height: usize, fade: f32) -> f32 {
    let amount = fade.abs().clamp(0.0, 1.0);
    if amount <= f32::EPSILON {
        return 1.0;
    }
    let denom = height.saturating_sub(1).max(1) as f32;
    let y_norm = y as f32 / denom;
    let gradient = if fade >= 0.0 { 1.0 - y_norm } else { y_norm };
    lerp_f32(1.0, gradient.clamp(0.0, 1.0), amount)
}

fn apply_spotlight(src: &[u8], width: usize, height: usize, params: SpotlightParams) -> Vec<u8> {
    let light_strength = params.light_strength.clamp(-1.0, 2.0);
    let shadow_strength = params.shadow_strength.clamp(0.0, 1.0);
    let tint_strength = params.tint_strength.clamp(0.0, 1.0);
    if width == 0
        || height == 0
        || (light_strength.abs() <= f32::EPSILON
            && shadow_strength <= f32::EPSILON
            && tint_strength <= f32::EPSILON)
    {
        return src.to_vec();
    }
    let radius = params.radius.clamp(0.0, 1.0);
    let feather = params.feather.clamp(0.001, 1.0);
    let cx = params.center[0].clamp(0.0, 1.0) * width.saturating_sub(1) as f32;
    let cy = params.center[1].clamp(0.0, 1.0) * height.saturating_sub(1) as f32;
    let max_dist = farthest_corner_distance(width, height, cx, cy).max(1.0);
    let tint = [
        params.tint_rgb[0] as f32 / 255.0,
        params.tint_rgb[1] as f32 / 255.0,
        params.tint_rgb[2] as f32 / 255.0,
    ];
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let d = dx.hypot(dy) / max_dist;
            let spot = 1.0 - smoothstep(radius, (radius + feather).min(1.5), d);
            let edge = 1.0 - spot;
            for c in 0..3 {
                let base = src[i + c] as f32 / 255.0;
                let lit = if light_strength >= 0.0 {
                    base + (1.0 - base) * light_strength * spot * 0.75
                } else {
                    base * (1.0 + light_strength * spot).clamp(0.0, 1.0)
                };
                let shaded = lit * (1.0 - shadow_strength * edge * 0.85);
                let tinted = lerp_f32(
                    shaded,
                    screen_channel(shaded, tint[c] * tint_strength),
                    tint_strength * spot,
                );
                out[i + c] = to_u8(tinted);
            }
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

fn apply_noise(src: &[u8], width: usize, height: usize, params: NoiseParams) -> Vec<u8> {
    let amount = params.amount.clamp(0.0, 1.0);
    if amount <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let mut out = src.to_vec();
    let amplitude = amount * 0.45;
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let mono_noise = noise_sample(x as u32, y as u32, 0, params);
            for c in 0..3 {
                let noise = if params.monochrome {
                    mono_noise
                } else {
                    noise_sample(x as u32, y as u32, c as u32, params)
                };
                out[i + c] = to_u8(src[i + c] as f32 / 255.0 + noise * amplitude);
            }
        }
    }
    out
}

fn noise_sample(x: u32, y: u32, channel: u32, params: NoiseParams) -> f32 {
    let seed = params.seed ^ channel.wrapping_mul(0x9E37_79B9);
    match params.distribution {
        NoiseDistribution::Uniform => signed_noise(x, y, seed),
        NoiseDistribution::Gaussian => {
            let mut sum = 0.0;
            for idx in 0..6 {
                sum += signed_noise(
                    x,
                    y,
                    seed ^ (idx as u32).wrapping_mul(0xA511_E9B3) ^ 0x632B_E59B,
                );
            }
            (sum / 6.0).clamp(-1.0, 1.0)
        }
    }
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

fn apply_anaglyph_3d(src: &[u8], width: usize, height: usize, params: AnaglyphParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let disparity = params.disparity_px.clamp(0.0, 96.0);
    if strength <= f32::EPSILON || disparity <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let half_disparity = disparity * 0.5;
    let angle = params.angle_degrees.to_radians();
    let (sin, cos) = angle.sin_cos();
    let dx = cos * half_disparity;
    let dy = sin * half_disparity;
    let luma_mix = params.luma_mix.clamp(0.0, 1.0);
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }
            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let left = anaglyph_prepare_sample(
                sample_rgb_bilinear_alpha_fallback(
                    src,
                    width,
                    height,
                    x as f32 - dx,
                    y as f32 - dy,
                    base,
                ),
                luma_mix,
            );
            let right = anaglyph_prepare_sample(
                sample_rgb_bilinear_alpha_fallback(
                    src,
                    width,
                    height,
                    x as f32 + dx,
                    y as f32 + dy,
                    base,
                ),
                luma_mix,
            );
            let shifted = match params.mode {
                AnaglyphMode::RedCyan => [left[0], right[1], right[2]],
                AnaglyphMode::GreenMagenta => [right[0], left[1], right[2]],
                AnaglyphMode::AmberBlue => [left[0], left[1] * 0.86 + right[1] * 0.14, right[2]],
                AnaglyphMode::RgbSplit => [left[0], base[1], right[2]],
            };
            for c in 0..3 {
                out[i + c] = to_u8(lerp_f32(base[c], shifted[c], strength));
            }
        }
    }
    out
}

fn anaglyph_prepare_sample(rgb: [f32; 3], luma_mix: f32) -> [f32; 3] {
    if luma_mix <= f32::EPSILON {
        return rgb;
    }
    let luma = luma01(rgb[0], rgb[1], rgb[2]);
    [
        lerp_f32(rgb[0], luma, luma_mix),
        lerp_f32(rgb[1], luma, luma_mix),
        lerp_f32(rgb[2], luma, luma_mix),
    ]
}

fn sample_rgb_bilinear_alpha_fallback(
    src: &[u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    fallback: [f32; 3],
) -> [f32; 3] {
    let (rgb, alpha) = sample_rgb_bilinear_alpha_aware(src, width, height, x, y);
    if alpha <= f32::EPSILON {
        fallback
    } else {
        [
            lerp_f32(fallback[0], rgb[0], alpha.clamp(0.0, 1.0)),
            lerp_f32(fallback[1], rgb[1], alpha.clamp(0.0, 1.0)),
            lerp_f32(fallback[2], rgb[2], alpha.clamp(0.0, 1.0)),
        ]
    }
}

fn apply_defringe(src: &[u8], width: usize, height: usize, params: DefringeParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let neutralize = params.neutralize.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || neutralize <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let radius = params.radius_px.clamp(1.0, 8.0).round() as isize;
    let edge_threshold = params.edge_threshold.clamp(0.0, 1.0);
    let color_threshold = params.color_threshold.clamp(0.0, 1.0);
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }
            let rgb = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let (_, saturation, _) = rgb_to_hsl(rgb[0], rgb[1], rgb[2]);
            let luma = luma01(rgb[0], rgb[1], rgb[2]);
            if saturation <= color_threshold {
                continue;
            }
            let Some((edge, max_neighbor_saturation)) =
                defringe_neighbor_stats(src, width, height, x, y, radius, luma)
            else {
                continue;
            };
            let saturation_excess = (saturation - max_neighbor_saturation).max(0.0);
            let edge_weight = smoothstep(edge_threshold, (edge_threshold + 0.22).min(1.0), edge);
            let color_weight = smoothstep(
                color_threshold,
                (color_threshold + 0.28).min(1.0),
                saturation_excess,
            );
            let amount = (edge_weight * color_weight * neutralize * strength).clamp(0.0, 1.0);
            if amount <= f32::EPSILON {
                continue;
            }
            for c in 0..3 {
                out[i + c] = to_u8(lerp_f32(rgb[c], luma, amount));
            }
        }
    }
    out
}

fn defringe_neighbor_stats(
    src: &[u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    radius: isize,
    luma: f32,
) -> Option<(f32, f32)> {
    let mut max_edge: f32 = 0.0;
    let mut max_saturation: f32 = 0.0;
    let mut count = 0;
    for (dx, dy) in [(radius, 0), (-radius, 0), (0, radius), (0, -radius)] {
        let sx = x as isize + dx;
        let sy = y as isize + dy;
        if sx < 0 || sy < 0 || sx >= width as isize || sy >= height as isize {
            continue;
        }
        let ni = (sy as usize * width + sx as usize) * 4;
        if src[ni + 3] == 0 {
            continue;
        }
        let rgb = [
            src[ni] as f32 / 255.0,
            src[ni + 1] as f32 / 255.0,
            src[ni + 2] as f32 / 255.0,
        ];
        let (_, saturation, _) = rgb_to_hsl(rgb[0], rgb[1], rgb[2]);
        let neighbor_luma = luma01(rgb[0], rgb[1], rgb[2]);
        max_edge = max_edge.max((luma - neighbor_luma).abs());
        max_saturation = max_saturation.max(saturation);
        count += 1;
    }
    (count > 0).then_some((max_edge, max_saturation))
}

fn apply_scanline_glitch(
    src: &[u8],
    width: usize,
    height: usize,
    params: ScanlineGlitchParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let spacing = params.line_spacing_px.clamp(2.0, 64.0);
    let line_strength = params.line_strength.clamp(0.0, 1.0);
    let jitter_px = params.jitter_px.clamp(0.0, 48.0);
    let rgb_shift_px = params.rgb_shift_px.clamp(0.0, 24.0);
    let block_strength = params.block_strength.clamp(0.0, 1.0);
    let noise = params.noise.clamp(0.0, 1.0);
    if line_strength <= f32::EPSILON
        && jitter_px <= f32::EPSILON
        && rgb_shift_px <= f32::EPSILON
        && block_strength <= f32::EPSILON
        && noise <= f32::EPSILON
    {
        return src.to_vec();
    }

    let mut out = src.to_vec();
    for y in 0..height {
        let row_seed = (y as u32).wrapping_mul(0x9E37_79B9) ^ params.seed;
        let row_noise = signed_noise(y as u32, 0, row_seed);
        let gate_noise = signed_noise(0, y as u32, params.seed ^ 0x51A7_9E21).abs();
        let row_gate = smoothstep(1.0 - block_strength, 1.0, gate_noise);
        let row_offset = (row_noise * jitter_px * (0.25 + row_gate * 0.75)).round();
        let line_mask = scanline_glitch_line_mask(y, spacing);
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }

            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let xf = x as f32 + row_offset;
            let yf = y as f32;
            let mut target = [
                sample_scanline_glitch_channel(
                    src,
                    width,
                    height,
                    xf + rgb_shift_px,
                    yf,
                    0,
                    base[0],
                ),
                sample_scanline_glitch_channel(src, width, height, xf, yf, 1, base[1]),
                sample_scanline_glitch_channel(
                    src,
                    width,
                    height,
                    xf - rgb_shift_px,
                    yf,
                    2,
                    base[2],
                ),
            ];

            let scan_dark = 1.0 - line_mask * line_strength * 0.58;
            target[0] *= scan_dark;
            target[1] = (target[1] * scan_dark + line_mask * line_strength * 0.035).clamp(0.0, 1.0);
            target[2] = (target[2] * scan_dark + line_mask * line_strength * 0.070).clamp(0.0, 1.0);
            for c in 0..3 {
                let n = signed_noise(
                    x as u32,
                    y as u32,
                    params.seed ^ (c as u32).wrapping_mul(0xA511_E9B3),
                ) * noise
                    * 0.20;
                target[c] = (target[c] + n).clamp(0.0, 1.0);
                out[i + c] = to_u8(lerp_f32(base[c], target[c], strength));
            }
        }
    }
    out
}

fn scanline_glitch_line_mask(y: usize, spacing: f32) -> f32 {
    let phase = ((y as f32 + 0.5) / spacing).fract();
    1.0 - smoothstep(0.32, 0.52, phase)
}

fn sample_scanline_glitch_channel(
    src: &[u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    channel: usize,
    fallback: f32,
) -> f32 {
    let (rgb, alpha) = sample_rgb_bilinear_alpha_aware(src, width, height, x, y);
    if alpha > f32::EPSILON {
        rgb[channel.min(2)]
    } else {
        fallback
    }
}

fn apply_vhs(src: &[u8], width: usize, height: usize, params: VhsParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let chroma_bleed_px = params.chroma_bleed_px.clamp(0.0, 32.0);
    let chroma_shift_px = params.chroma_shift_px.clamp(-24.0, 24.0);
    let ghost_offset_px = params.ghost_offset_px.clamp(0.0, 64.0);
    let ghost_strength = params.ghost_strength.clamp(0.0, 1.0);
    let tracking_strength = params.tracking_strength.clamp(0.0, 1.0);
    let scanline_strength = params.scanline_strength.clamp(0.0, 1.0);
    let noise = params.noise.clamp(0.0, 1.0);
    let desaturation = params.desaturation.clamp(0.0, 1.0);
    if chroma_bleed_px <= f32::EPSILON
        && chroma_shift_px.abs() <= f32::EPSILON
        && ghost_offset_px <= f32::EPSILON
        && ghost_strength <= f32::EPSILON
        && tracking_strength <= f32::EPSILON
        && scanline_strength <= f32::EPSILON
        && noise <= f32::EPSILON
        && desaturation <= f32::EPSILON
    {
        return src.to_vec();
    }

    let chroma_rows = if chroma_bleed_px > f32::EPSILON || chroma_shift_px.abs() > f32::EPSILON {
        Some(build_vhs_chroma_rows(
            src,
            width,
            height,
            chroma_bleed_px.round() as usize,
        ))
    } else {
        None
    };

    let mut out = src.to_vec();
    let height_scale = height.saturating_sub(1).max(1) as f32;
    for y in 0..height {
        let row_block = (y / 3) as u32;
        let row_noise = signed_noise(0, row_block, params.seed ^ 0x621D_4A33);
        let band_gate = smoothstep(0.58, 0.96, row_noise.abs());
        let y_norm = y as f32 / height_scale;
        let head_switch = smoothstep(0.82, 0.98, y_norm);
        let tracking = (band_gate * 0.75 + head_switch * 0.65).clamp(0.0, 1.0) * tracking_strength;
        let row_shift = (signed_noise(row_block, 1, params.seed ^ 0xA53B_72C1) * tracking * 8.0
            + signed_noise(2, row_block, params.seed ^ 0xC2B2_AE35)
                * head_switch
                * tracking
                * 14.0)
            .round();
        let tracking_luma = (row_noise * band_gate * 0.16 - head_switch * 0.10) * tracking_strength;

        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }

            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let shifted = sample_vhs_rgb_or_fallback(
                src,
                width,
                height,
                x as f32 + row_shift,
                y as f32,
                base,
            );
            let (mut luma, mut cb, mut cr) = vhs_rgb_to_ycbcr(shifted[0], shifted[1], shifted[2]);
            if let Some(chroma_rows) = chroma_rows.as_ref() {
                let sampled = sample_vhs_chroma(
                    chroma_rows,
                    width,
                    x as f32 + row_shift + chroma_shift_px,
                    y,
                    [cb, cr],
                );
                cb = sampled[0];
                cr = sampled[1];
            }

            let chroma_scale = 1.0 - desaturation * 0.78;
            cb *= chroma_scale;
            cr *= chroma_scale;
            luma = (luma + tracking_luma).clamp(0.0, 1.0);
            let mut target = vhs_ycbcr_to_rgb(luma, cb, cr);

            if ghost_strength > f32::EPSILON
                && ghost_offset_px > f32::EPSILON
                && let Some(ghost) =
                    sample_vhs_rgb(src, width, height, x as f32 - ghost_offset_px, y as f32)
            {
                for c in 0..3 {
                    target[c] = screen_channel(target[c], ghost[c] * ghost_strength * 0.62);
                }
            }

            let scan_mask = if y % 2 == 1 { 1.0 } else { 0.18 };
            let scan_dark = 1.0 - scan_mask * scanline_strength * 0.22;
            for c in 0..3 {
                let mono_noise =
                    signed_noise(x as u32, y as u32, params.seed ^ 0x34A6_E7B1) * noise * 0.16;
                let color_noise = signed_noise(
                    x as u32,
                    y as u32,
                    params.seed ^ (c as u32).wrapping_mul(0x9E37_79B9) ^ 0x41C6_4E6D,
                ) * noise
                    * 0.055;
                target[c] = (target[c] * scan_dark + mono_noise + color_noise).clamp(0.0, 1.0);
                out[i + c] = to_u8(lerp_f32(base[c], target[c], strength));
            }
        }
    }
    out
}

struct VhsChromaRows {
    cb: Vec<f32>,
    cr: Vec<f32>,
    coverage: Vec<f32>,
}

fn build_vhs_chroma_rows(src: &[u8], width: usize, height: usize, radius: usize) -> VhsChromaRows {
    let radius = radius.min(32);
    let mut cb = vec![0.0; width * height];
    let mut cr = vec![0.0; width * height];
    let mut coverage = vec![0.0; width * height];
    let mut prefix_cb = vec![0.0; width + 1];
    let mut prefix_cr = vec![0.0; width + 1];
    let mut prefix_alpha = vec![0.0; width + 1];

    for y in 0..height {
        prefix_cb.fill(0.0);
        prefix_cr.fill(0.0);
        prefix_alpha.fill(0.0);
        for x in 0..width {
            let i = (y * width + x) * 4;
            let alpha = src[i + 3] as f32 / 255.0;
            let (pixel_cb, pixel_cr) = if alpha > f32::EPSILON {
                let (_, pixel_cb, pixel_cr) = vhs_rgb_to_ycbcr(
                    src[i] as f32 / 255.0,
                    src[i + 1] as f32 / 255.0,
                    src[i + 2] as f32 / 255.0,
                );
                (pixel_cb * alpha, pixel_cr * alpha)
            } else {
                (0.0, 0.0)
            };
            prefix_cb[x + 1] = prefix_cb[x] + pixel_cb;
            prefix_cr[x + 1] = prefix_cr[x] + pixel_cr;
            prefix_alpha[x + 1] = prefix_alpha[x] + alpha;
        }

        for x in 0..width {
            let x0 = x.saturating_sub(radius);
            let x1 = (x + radius + 1).min(width);
            let alpha_sum = prefix_alpha[x1] - prefix_alpha[x0];
            let o = y * width + x;
            if alpha_sum > f32::EPSILON {
                cb[o] = (prefix_cb[x1] - prefix_cb[x0]) / alpha_sum;
                cr[o] = (prefix_cr[x1] - prefix_cr[x0]) / alpha_sum;
                coverage[o] = alpha_sum / (x1 - x0).max(1) as f32;
            }
        }
    }

    VhsChromaRows { cb, cr, coverage }
}

fn sample_vhs_chroma(
    rows: &VhsChromaRows,
    width: usize,
    x: f32,
    y: usize,
    fallback: [f32; 2],
) -> [f32; 2] {
    if width == 0 || x < 0.0 || x > width.saturating_sub(1) as f32 {
        return fallback;
    }
    let x0 = x.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let tx = x - x0 as f32;
    let i0 = y * width + x0;
    let i1 = y * width + x1;
    let coverage = lerp_f32(rows.coverage[i0], rows.coverage[i1], tx);
    if coverage <= f32::EPSILON {
        return fallback;
    }
    [
        lerp_f32(rows.cb[i0], rows.cb[i1], tx),
        lerp_f32(rows.cr[i0], rows.cr[i1], tx),
    ]
}

fn sample_vhs_rgb_or_fallback(
    src: &[u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    fallback: [f32; 3],
) -> [f32; 3] {
    sample_vhs_rgb(src, width, height, x, y).unwrap_or(fallback)
}

fn sample_vhs_rgb(src: &[u8], width: usize, height: usize, x: f32, y: f32) -> Option<[f32; 3]> {
    if width == 0
        || height == 0
        || x < 0.0
        || y < 0.0
        || x > width.saturating_sub(1) as f32
        || y > height.saturating_sub(1) as f32
    {
        return None;
    }
    let (rgb, alpha) = sample_rgb_bilinear_alpha_aware(src, width, height, x, y);
    if alpha > f32::EPSILON {
        Some(rgb)
    } else {
        None
    }
}

fn vhs_rgb_to_ycbcr(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let y = luma01(r, g, b);
    let cb = (b - y) * 0.565;
    let cr = (r - y) * 0.713;
    (y, cb, cr)
}

fn vhs_ycbcr_to_rgb(y: f32, cb: f32, cr: f32) -> [f32; 3] {
    let r = y + cr * 1.403;
    let b = y + cb * 1.770;
    let g = (y - 0.299 * r - 0.114 * b) / 0.587;
    [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]
}

fn apply_data_mosh(src: &[u8], width: usize, height: usize, params: DataMoshParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let block_size = params.block_size_px.clamp(2.0, 128.0).round() as usize;
    let displacement_px = params.displacement_px.clamp(0.0, 128.0);
    let base_angle = params.direction_degrees.to_radians();
    let base_dir_x = base_angle.cos();
    let base_dir_y = base_angle.sin();
    let low = params
        .low_threshold
        .clamp(0.0, 1.0)
        .min(params.high_threshold.clamp(0.0, 1.0));
    let high = params
        .low_threshold
        .clamp(0.0, 1.0)
        .max(params.high_threshold.clamp(0.0, 1.0));
    let freeze = params.freeze.clamp(0.0, 1.0);
    let smear = params.smear.clamp(0.0, 1.0);
    let rgb_shift_px = params.rgb_shift_px.clamp(0.0, 32.0);
    let noise = params.noise.clamp(0.0, 1.0);
    if displacement_px <= f32::EPSILON
        && freeze <= f32::EPSILON
        && smear <= f32::EPSILON
        && rgb_shift_px <= f32::EPSILON
        && noise <= f32::EPSILON
    {
        return src.to_vec();
    }

    let mut out = src.to_vec();
    for y in 0..height {
        let by = (y / block_size) as u32;
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }

            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let range_gate = data_mosh_luma_gate(luma01(base[0], base[1], base[2]), low, high);
            if range_gate <= f32::EPSILON {
                continue;
            }

            let bx = (x / block_size) as u32;
            let block_noise = signed_noise(bx, by, params.seed ^ 0x46D7_2B11);
            let freeze_gate = if freeze <= f32::EPSILON {
                0.0
            } else {
                (freeze * 0.28 + smoothstep(1.0 - freeze * 0.9, 1.0, block_noise.abs()) * 0.72)
                    .clamp(0.0, 1.0)
            };
            let local_gate = range_gate * freeze_gate;
            let angle = base_angle + signed_noise(bx, by, params.seed ^ 0xD153_8F5D) * 0.55;
            let dir_x = angle.cos();
            let dir_y = angle.sin();
            let offset = displacement_px
                * local_gate
                * (0.45 + signed_noise(by, bx, params.seed ^ 0xA9B4_3C17).abs() * 0.55);
            let mut sx = x as f32 - dir_x * offset;
            let mut sy = y as f32 - dir_y * offset;
            let mut target = data_mosh_sample_rgb_or_base(src, width, height, sx, sy, base);

            if smear > f32::EPSILON && offset > f32::EPSILON {
                let smear_offset = block_size as f32
                    * smear
                    * (0.45
                        + signed_noise(bx, by, params.seed ^ 0x7C31_9E93 ^ block_size as u32)
                            .abs()
                            * 0.85);
                sx -= dir_x * smear_offset;
                sy -= dir_y * smear_offset;
                let smeared = data_mosh_sample_rgb_or_base(src, width, height, sx, sy, target);
                for c in 0..3 {
                    target[c] = lerp_f32(target[c], smeared[c], smear * local_gate);
                }
            }

            if rgb_shift_px > f32::EPSILON {
                let shift = rgb_shift_px * range_gate;
                target[0] = data_mosh_sample_channel_or_base(
                    src,
                    width,
                    height,
                    x as f32 + base_dir_x * shift,
                    y as f32 + base_dir_y * shift,
                    0,
                    target[0],
                );
                target[2] = data_mosh_sample_channel_or_base(
                    src,
                    width,
                    height,
                    x as f32 - base_dir_x * shift,
                    y as f32 - base_dir_y * shift,
                    2,
                    target[2],
                );
            }

            if noise > f32::EPSILON {
                for (c, channel) in target.iter_mut().enumerate() {
                    let n = signed_noise(
                        x as u32,
                        y as u32,
                        params.seed ^ (c as u32).wrapping_mul(0x9E37_79B9) ^ 0x6B8B_4567,
                    ) * noise
                        * range_gate
                        * 0.18;
                    *channel = (*channel + n).clamp(0.0, 1.0);
                }
            }

            let amount = strength * range_gate;
            for c in 0..3 {
                out[i + c] = to_u8(lerp_f32(base[c], target[c], amount));
            }
        }
    }
    out
}

fn data_mosh_luma_gate(luma: f32, low: f32, high: f32) -> f32 {
    let feather = 0.055;
    let lower = if low <= f32::EPSILON {
        1.0
    } else {
        smoothstep((low - feather).max(0.0), low, luma)
    };
    let upper = if high >= 1.0 - f32::EPSILON {
        1.0
    } else {
        1.0 - smoothstep(high, (high + feather).min(1.0), luma)
    };
    (lower * upper).clamp(0.0, 1.0)
}

fn data_mosh_sample_rgb_or_base(
    src: &[u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    fallback: [f32; 3],
) -> [f32; 3] {
    if width == 0
        || height == 0
        || x < 0.0
        || y < 0.0
        || x > width.saturating_sub(1) as f32
        || y > height.saturating_sub(1) as f32
    {
        return fallback;
    }
    let (rgb, alpha) = sample_rgb_bilinear_alpha_aware(src, width, height, x, y);
    if alpha > f32::EPSILON { rgb } else { fallback }
}

fn data_mosh_sample_channel_or_base(
    src: &[u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    channel: usize,
    fallback: f32,
) -> f32 {
    if width == 0
        || height == 0
        || x < 0.0
        || y < 0.0
        || x > width.saturating_sub(1) as f32
        || y > height.saturating_sub(1) as f32
    {
        return fallback;
    }
    let (rgb, alpha) = sample_rgb_bilinear_alpha_aware(src, width, height, x, y);
    if alpha > f32::EPSILON {
        rgb[channel.min(2)]
    } else {
        fallback
    }
}

fn apply_pixel_sort(src: &[u8], width: usize, height: usize, params: PixelSortParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let low = params
        .low_threshold
        .clamp(0.0, 1.0)
        .min(params.high_threshold.clamp(0.0, 1.0));
    let high = params
        .low_threshold
        .clamp(0.0, 1.0)
        .max(params.high_threshold.clamp(0.0, 1.0));
    let max_segment = params.max_segment_px.clamp(2, 512) as usize;
    let mut out = src.to_vec();
    let mut indices = Vec::with_capacity(max_segment);
    let mut samples = Vec::with_capacity(max_segment);

    match params.direction {
        PixelSortDirection::Horizontal => {
            for y in 0..height {
                let mut x = 0;
                while x < width {
                    indices.clear();
                    while x < width {
                        let i = (y * width + x) * 4;
                        if pixel_sort_eligible(src, i, low, high) {
                            break;
                        }
                        x += 1;
                    }
                    while x < width {
                        let i = (y * width + x) * 4;
                        if !pixel_sort_eligible(src, i, low, high) {
                            break;
                        }
                        indices.push(i);
                        if indices.len() == max_segment {
                            apply_pixel_sort_segment(
                                src,
                                &mut out,
                                &indices,
                                params.order,
                                strength,
                                &mut samples,
                            );
                            indices.clear();
                        }
                        x += 1;
                    }
                    apply_pixel_sort_segment(
                        src,
                        &mut out,
                        &indices,
                        params.order,
                        strength,
                        &mut samples,
                    );
                }
            }
        }
        PixelSortDirection::Vertical => {
            for x in 0..width {
                let mut y = 0;
                while y < height {
                    indices.clear();
                    while y < height {
                        let i = (y * width + x) * 4;
                        if pixel_sort_eligible(src, i, low, high) {
                            break;
                        }
                        y += 1;
                    }
                    while y < height {
                        let i = (y * width + x) * 4;
                        if !pixel_sort_eligible(src, i, low, high) {
                            break;
                        }
                        indices.push(i);
                        if indices.len() == max_segment {
                            apply_pixel_sort_segment(
                                src,
                                &mut out,
                                &indices,
                                params.order,
                                strength,
                                &mut samples,
                            );
                            indices.clear();
                        }
                        y += 1;
                    }
                    apply_pixel_sort_segment(
                        src,
                        &mut out,
                        &indices,
                        params.order,
                        strength,
                        &mut samples,
                    );
                }
            }
        }
    }

    out
}

#[derive(Clone, Copy)]
struct PixelSortSample {
    rgb: [u8; 3],
    luma: f32,
}

fn pixel_sort_eligible(src: &[u8], i: usize, low: f32, high: f32) -> bool {
    if src[i + 3] == 0 {
        return false;
    }
    let luma = luma01(
        src[i] as f32 / 255.0,
        src[i + 1] as f32 / 255.0,
        src[i + 2] as f32 / 255.0,
    );
    luma >= low && luma <= high
}

fn apply_pixel_sort_segment(
    src: &[u8],
    out: &mut [u8],
    indices: &[usize],
    order: PixelSortOrder,
    strength: f32,
    samples: &mut Vec<PixelSortSample>,
) {
    if indices.len() < 2 {
        return;
    }
    samples.clear();
    samples.extend(indices.iter().map(|&i| {
        let r = src[i] as f32 / 255.0;
        let g = src[i + 1] as f32 / 255.0;
        let b = src[i + 2] as f32 / 255.0;
        PixelSortSample {
            rgb: [src[i], src[i + 1], src[i + 2]],
            luma: luma01(r, g, b),
        }
    }));
    samples.sort_by(|a, b| match order {
        PixelSortOrder::DarkToLight => a
            .luma
            .partial_cmp(&b.luma)
            .unwrap_or(std::cmp::Ordering::Equal),
        PixelSortOrder::LightToDark => b
            .luma
            .partial_cmp(&a.luma)
            .unwrap_or(std::cmp::Ordering::Equal),
    });

    for (&i, sample) in indices.iter().zip(samples.iter()) {
        for c in 0..3 {
            out[i + c] = lerp_u8(src[i + c], sample.rgb[c], strength);
        }
    }
}

fn apply_old_film(src: &[u8], width: usize, height: usize, params: OldFilmParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let sepia = params.sepia.clamp(0.0, 1.0);
    let fade = params.fade.clamp(0.0, 1.0);
    let vignette = params.vignette.clamp(0.0, 1.0);
    let grain = params.grain.clamp(0.0, 1.0);
    let dust = params.dust.clamp(0.0, 1.0);
    let scratches = params.scratches.clamp(0.0, 1.0);
    if sepia <= f32::EPSILON
        && fade <= f32::EPSILON
        && vignette <= f32::EPSILON
        && grain <= f32::EPSILON
        && dust <= f32::EPSILON
        && scratches <= f32::EPSILON
    {
        return src.to_vec();
    }

    let cx = (width.saturating_sub(1)) as f32 * 0.5;
    let cy = (height.saturating_sub(1)) as f32 * 0.5;
    let max_dist = (cx * cx + cy * cy).sqrt().max(1.0);
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }

            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let luma = luma01(base[0], base[1], base[2]);
            let sepia_rgb = [
                (luma * 1.10 + 0.035).clamp(0.0, 1.0),
                (luma * 0.91 + 0.030).clamp(0.0, 1.0),
                (luma * 0.62 + 0.018).clamp(0.0, 1.0),
            ];
            let paper = [0.86, 0.78, 0.58];
            let mut target = base;
            for c in 0..3 {
                target[c] = lerp_f32(target[c], sepia_rgb[c], sepia);
                target[c] = lerp_f32(target[c], luma, fade * 0.40);
                target[c] = lerp_f32(target[c], paper[c], fade * 0.26);
                target[c] = lerp_f32(0.5, target[c], 1.0 - fade * 0.22).clamp(0.0, 1.0);
            }

            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = ((dx * dx + dy * dy).sqrt() / max_dist).clamp(0.0, 1.0);
            let vignette_amount = smoothstep(0.42, 1.0, dist) * vignette;
            for c in 0..3 {
                target[c] *= 1.0 - vignette_amount * 0.62;
            }

            if scratches > f32::EPSILON {
                let line_noise = signed_noise(x as u32, 0, params.seed ^ 0x4C52_4B1D).abs();
                let line_gate = smoothstep(0.56, 0.96, line_noise) * scratches;
                let gap_noise =
                    signed_noise(x as u32, (y / 9) as u32, params.seed ^ 0x912F_A33B).abs();
                let gap = smoothstep(0.12, 0.62, gap_noise);
                let scratch_amount = line_gate * gap;
                if scratch_amount > f32::EPSILON {
                    let dark_line = signed_noise(x as u32, 1, params.seed ^ 0x3C6E_F372) > 0.52;
                    for channel in &mut target {
                        *channel = if dark_line {
                            *channel * (1.0 - scratch_amount * 0.55)
                        } else {
                            screen_channel(*channel, scratch_amount * 0.82)
                        };
                    }
                }
            }

            if dust > f32::EPSILON {
                let white_gate = smoothstep(
                    1.0 - dust * 0.075,
                    1.0,
                    signed_noise(x as u32, y as u32, params.seed ^ 0x7A37_95D1).abs(),
                );
                let black_gate = smoothstep(
                    1.0 - dust * 0.050,
                    1.0,
                    signed_noise(x as u32, y as u32, params.seed ^ 0xA4B1_83F5).abs(),
                );
                if white_gate > f32::EPSILON || black_gate > f32::EPSILON {
                    for channel in &mut target {
                        *channel = screen_channel(*channel, white_gate * 0.70);
                        *channel *= 1.0 - black_gate * 0.50;
                    }
                }
            }

            if grain > f32::EPSILON {
                let n = signed_noise(x as u32, y as u32, params.seed ^ 0xB529_7A4D) * grain * 0.15;
                for channel in &mut target {
                    *channel = (*channel + n).clamp(0.0, 1.0);
                }
            }

            for c in 0..3 {
                out[i + c] = to_u8(lerp_f32(base[c], target[c], strength));
            }
        }
    }
    out
}

fn apply_water_caustics(
    src: &[u8],
    width: usize,
    height: usize,
    params: WaterCausticsParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let scale = params.scale_px.clamp(8.0, 240.0);
    let intensity = params.intensity.clamp(0.0, 2.0);
    let contrast = params.contrast.clamp(0.0, 1.0);
    let tint = params.tint.clamp(0.0, 1.0);
    let depth = params.depth.clamp(0.0, 1.0);
    let phase = params.phase.rem_euclid(1.0);
    if intensity <= f32::EPSILON && depth <= f32::EPSILON {
        return src.to_vec();
    }

    let tint_rgb = [0.42, 0.88, 1.0];
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }

            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let u = x as f32 / scale;
            let v = y as f32 / scale;
            let raw = water_caustics_pattern(u, v, phase, params.seed);
            let threshold = 0.70 - contrast * 0.50;
            let caustic = smoothstep(threshold, 1.0, raw);
            let gap_shadow = (1.0 - raw).powf(1.6) * depth * 0.18;
            let luma = luma01(base[0], base[1], base[2]);
            let light_amount = caustic * intensity * (0.32 + (1.0 - luma) * 0.48);
            let mut target = base;
            for c in 0..3 {
                let light_color = lerp_f32(1.0, tint_rgb[c], tint);
                target[c] *= 1.0 - gap_shadow;
                target[c] = screen_channel(target[c], light_color * light_amount);
                out[i + c] = to_u8(lerp_f32(base[c], target[c], strength));
            }
        }
    }
    out
}

fn water_caustics_pattern(u: f32, v: f32, phase: f32, seed: u32) -> f32 {
    let phase = phase * std::f32::consts::TAU;
    let ox = signed_noise(17, 3, seed) * 4.0;
    let oy = signed_noise(5, 29, seed ^ 0x9E37_79B9) * 4.0;
    let u = u + ox;
    let v = v + oy;
    let a = (u * 1.34 + (v * 0.73 + phase).sin() * 0.78 + phase * 0.34)
        .sin()
        .abs();
    let b = (v * 1.18 + (u * 0.61 - phase * 0.7).sin() * 0.70 - phase * 0.22)
        .sin()
        .abs();
    let c = ((u + v) * 0.82 + ((u - v) * 0.54 + phase * 0.5).sin() * 0.54)
        .sin()
        .abs();
    let distance = a.min(b).min(c);
    let line = 1.0 - smoothstep(0.035, 0.22, distance);
    let intersection = 1.0 - smoothstep(0.0, 0.16, (a - b).abs().min((b - c).abs()));
    (line * (0.72 + intersection * 0.28)).clamp(0.0, 1.0)
}

#[derive(Clone, Copy)]
struct ParticleOverlaySample {
    alpha: f32,
    rgb: [f32; 3],
}

fn apply_particle_overlay(
    src: &[u8],
    width: usize,
    height: usize,
    params: ParticleOverlayParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let opacity = params.opacity.clamp(0.0, 1.0);
    let density = params.density.clamp(0.0, 1.0);
    if strength <= f32::EPSILON
        || opacity <= f32::EPSILON
        || density <= f32::EPSILON
        || width == 0
        || height == 0
    {
        return src.to_vec();
    }

    let size = params.size_px.clamp(0.5, 48.0);
    let length = params.length_px.clamp(0.0, 240.0);
    let angle = params.angle_degrees.to_radians();
    let base_rgb = [
        params.color_rgb[0] as f32 / 255.0,
        params.color_rgb[1] as f32 / 255.0,
        params.color_rgb[2] as f32 / 255.0,
    ];
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }
            let sample = particle_overlay_sample_at(
                x as f32,
                y as f32,
                params.mode,
                density,
                size,
                length,
                angle,
                base_rgb,
                params.seed,
            );
            let alpha = (sample.alpha * opacity * strength).clamp(0.0, 1.0);
            if alpha <= f32::EPSILON {
                continue;
            }
            for c in 0..3 {
                let base = src[i + c] as f32 / 255.0;
                out[i + c] = to_u8(lerp_f32(base, sample.rgb[c], alpha));
            }
        }
    }
    out
}

fn particle_overlay_sample_at(
    x: f32,
    y: f32,
    mode: ParticleOverlayMode,
    density: f32,
    size: f32,
    length: f32,
    angle: f32,
    base_rgb: [f32; 3],
    seed: u32,
) -> ParticleOverlaySample {
    let (sin, cos) = angle.sin_cos();
    let u = x * cos + y * sin;
    let v = -x * sin + y * cos;
    let spacing = match mode {
        ParticleOverlayMode::Rain => lerp_f32(52.0, 10.0, density),
        ParticleOverlayMode::Snow => lerp_f32(68.0, 13.0, density),
        ParticleOverlayMode::Petals => lerp_f32(78.0, 15.0, density),
    }
    .max(2.0);
    let long_cell = match mode {
        ParticleOverlayMode::Rain => (length.max(size * 3.0) * 1.18).max(spacing),
        ParticleOverlayMode::Snow | ParticleOverlayMode::Petals => spacing,
    };
    let cell_u = (u / long_cell).floor() as i32;
    let cell_v = (v / spacing).floor() as i32;
    let mut alpha = 0.0;
    let mut rgb = base_rgb;

    for du in -1..=1 {
        for dv in -1..=1 {
            let cu = cell_u + du;
            let cv = cell_v + dv;
            let jitter_u = cell_noise01(cu, cv, seed);
            let jitter_v = cell_noise01(cu, cv, seed ^ 0xB529_7A4D);
            let particle_u = (cu as f32 + 0.12 + jitter_u * 0.76) * long_cell;
            let particle_v = (cv as f32 + 0.12 + jitter_v * 0.76) * spacing;
            let candidate = match mode {
                ParticleOverlayMode::Rain => particle_rain_sample(
                    u,
                    v,
                    particle_u,
                    particle_v,
                    size,
                    length.max(size * 2.0),
                    base_rgb,
                    cu,
                    cv,
                    seed,
                ),
                ParticleOverlayMode::Snow => {
                    particle_snow_sample(u, v, particle_u, particle_v, size, base_rgb, cu, cv, seed)
                }
                ParticleOverlayMode::Petals => particle_petal_sample(
                    u, v, particle_u, particle_v, size, base_rgb, cu, cv, seed,
                ),
            };
            if candidate.alpha > alpha {
                alpha = candidate.alpha;
                rgb = candidate.rgb;
            }
        }
    }

    ParticleOverlaySample { alpha, rgb }
}

fn particle_rain_sample(
    u: f32,
    v: f32,
    particle_u: f32,
    particle_v: f32,
    size: f32,
    length: f32,
    base_rgb: [f32; 3],
    cell_u: i32,
    cell_v: i32,
    seed: u32,
) -> ParticleOverlaySample {
    let along = (u - particle_u).abs();
    let across = (v - particle_v).abs();
    let half_len =
        (length * (0.72 + cell_noise01(cell_u, cell_v, seed ^ 0x68BC_21EB) * 0.55)).max(size);
    let line = 1.0 - smoothstep(size * 0.18, size, across);
    let tail = 1.0 - smoothstep(half_len * 0.72, half_len, along);
    let alpha = (line * tail).clamp(0.0, 1.0);
    ParticleOverlaySample {
        alpha,
        rgb: brighten_particle_rgb(base_rgb, 0.85, 0.20),
    }
}

fn particle_snow_sample(
    u: f32,
    v: f32,
    particle_u: f32,
    particle_v: f32,
    size: f32,
    base_rgb: [f32; 3],
    cell_u: i32,
    cell_v: i32,
    seed: u32,
) -> ParticleOverlaySample {
    let radius = (size * (0.55 + cell_noise01(cell_u, cell_v, seed ^ 0xD1B5_4A32) * 0.75)).max(0.5);
    let dist = ((u - particle_u).powi(2) + (v - particle_v).powi(2)).sqrt();
    let alpha = 1.0 - smoothstep(radius * 0.45, radius, dist);
    ParticleOverlaySample {
        alpha: alpha.clamp(0.0, 1.0),
        rgb: brighten_particle_rgb(base_rgb, 0.95, 0.10),
    }
}

fn particle_petal_sample(
    u: f32,
    v: f32,
    particle_u: f32,
    particle_v: f32,
    size: f32,
    base_rgb: [f32; 3],
    cell_u: i32,
    cell_v: i32,
    seed: u32,
) -> ParticleOverlaySample {
    let dx = u - particle_u;
    let dy = v - particle_v;
    let rotation = cell_noise01(cell_u, cell_v, seed ^ 0xA3C5_9AC3) * std::f32::consts::TAU;
    let (sin, cos) = rotation.sin_cos();
    let rx = dx * cos + dy * sin;
    let ry = -dx * sin + dy * cos;
    let major = (size * (1.7 + cell_noise01(cell_u, cell_v, seed ^ 0x91E1_D0F5) * 0.9)).max(1.0);
    let minor = (size * 0.62).max(0.5);
    let shape = ((rx / major).powi(2) + (ry / minor).powi(2)).sqrt();
    let alpha = 1.0 - smoothstep(0.58, 1.0, shape);
    let warmth = cell_noise01(cell_u, cell_v, seed ^ 0xC2B2_AE35);
    let rgb = [
        (base_rgb[0] * (0.95 + warmth * 0.12)).clamp(0.0, 1.0),
        (base_rgb[1] * (0.88 + warmth * 0.12)).clamp(0.0, 1.0),
        (base_rgb[2] * (0.92 + warmth * 0.10)).clamp(0.0, 1.0),
    ];
    ParticleOverlaySample {
        alpha: alpha.clamp(0.0, 1.0),
        rgb,
    }
}

fn brighten_particle_rgb(rgb: [f32; 3], keep: f32, white: f32) -> [f32; 3] {
    [
        lerp_f32(rgb[0], 1.0, white) * keep,
        lerp_f32(rgb[1], 1.0, white) * keep,
        lerp_f32(rgb[2], 1.0, white) * keep,
    ]
}

fn cell_noise01(x: i32, y: i32, seed: u32) -> f32 {
    let h = hash_u32(
        seed ^ (x as u32).wrapping_mul(0x9E37_79B1)
            ^ (y as u32).wrapping_mul(0x85EB_CA77)
            ^ (x as u32).rotate_left(11)
            ^ (y as u32).rotate_right(9),
    );
    h as f32 / u32::MAX as f32
}

fn apply_aurora(src: &[u8], width: usize, height: usize, params: AuroraParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let brightness = params.brightness.clamp(0.0, 2.0);
    if strength <= f32::EPSILON || brightness <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let band_count = params.band_count.clamp(1.0, 12.0);
    let scale = params.scale_px.clamp(24.0, 480.0);
    let height_extent = params.height.clamp(0.08, 1.0);
    let waviness = params.waviness.clamp(0.0, 1.0);
    let softness = params.softness.clamp(0.0, 1.0);
    let phase = params.phase.rem_euclid(1.0) * std::f32::consts::TAU;
    let primary = [
        params.color_rgb[0] as f32 / 255.0,
        params.color_rgb[1] as f32 / 255.0,
        params.color_rgb[2] as f32 / 255.0,
    ];
    let secondary = [
        params.secondary_rgb[0] as f32 / 255.0,
        params.secondary_rgb[1] as f32 / 255.0,
        params.secondary_rgb[2] as f32 / 255.0,
    ];
    let mut out = src.to_vec();
    let width_denom = width.saturating_sub(1).max(1) as f32;
    let height_denom = height.saturating_sub(1).max(1) as f32;

    for y in 0..height {
        let y_norm = y as f32 / height_denom;
        let vertical = aurora_vertical_weight(y_norm, height_extent, softness);
        if vertical <= f32::EPSILON {
            continue;
        }
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }

            let x_norm = x as f32 / width_denom;
            let u = x as f32 / scale;
            let v = y as f32 / scale;
            let curtain = aurora_curtain_value(
                x_norm,
                y_norm,
                u,
                v,
                band_count,
                waviness,
                softness,
                phase,
                params.seed,
            );
            let luma = luma01(
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            );
            let visibility = 0.34 + (1.0 - luma) * 0.66;
            let amount = (curtain * vertical * brightness * visibility).clamp(0.0, 1.0);
            if amount <= f32::EPSILON {
                continue;
            }
            let mix = aurora_color_mix(x_norm, y_norm, phase, params.seed);
            for c in 0..3 {
                let color = lerp_f32(primary[c], secondary[c], mix);
                let base = src[i + c] as f32 / 255.0;
                let target = screen_channel(base, (color * amount).clamp(0.0, 1.0));
                out[i + c] = to_u8(lerp_f32(base, target, strength));
            }
        }
    }
    out
}

fn aurora_vertical_weight(y_norm: f32, height_extent: f32, softness: f32) -> f32 {
    let lower = height_extent * (0.18 + softness * 0.20);
    let upper = height_extent;
    let fade_down = 1.0 - smoothstep(lower, upper, y_norm);
    let top_lift = smoothstep(0.0, 0.03 + softness * 0.10, y_norm);
    (fade_down * (0.72 + top_lift * 0.28)).clamp(0.0, 1.0)
}

fn aurora_curtain_value(
    x_norm: f32,
    y_norm: f32,
    u: f32,
    v: f32,
    band_count: f32,
    waviness: f32,
    softness: f32,
    phase: f32,
    seed: u32,
) -> f32 {
    let drift = ((y_norm * (2.4 + waviness * 5.8) + phase * 0.18).sin() * 0.20
        + (y_norm * (6.2 + waviness * 4.5) - phase * 0.12).sin() * 0.08)
        * waviness;
    let noise = glass_value_noise(
        u * 0.64 + phase * 0.11,
        v * 1.55 - phase * 0.07,
        seed ^ 0x3F2D_A91B,
    ) * waviness
        * 0.18;
    let phase_x = x_norm * band_count + drift + noise + phase * 0.035;
    let fold = (phase_x * std::f32::consts::TAU).sin() * 0.5 + 0.5;
    let ridge = smoothstep(0.46 - softness * 0.18, 1.0, fold);
    let veil = smoothstep(0.12, 0.88, fold) * (0.20 + softness * 0.22);
    let shimmer = 0.68
        + glass_value_noise(
            u * 2.1 + 9.7 - phase * 0.17,
            v * 3.0 + 4.2 + phase * 0.13,
            seed ^ 0x87C1_59E3,
        ) * 0.32;
    ((ridge * (0.80 + softness * 0.18) + veil) * shimmer).clamp(0.0, 1.0)
}

fn aurora_color_mix(x_norm: f32, y_norm: f32, phase: f32, seed: u32) -> f32 {
    let wave =
        ((x_norm * 2.6 + y_norm * 0.9 + phase * 0.08) * std::f32::consts::TAU).sin() * 0.5 + 0.5;
    let noise = glass_value_noise(x_norm * 3.4 + 2.0, y_norm * 1.7 - phase * 0.1, seed);
    (wave * 0.62 + (noise * 0.5 + 0.5) * 0.38).clamp(0.0, 1.0)
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

fn apply_screen_tone(src: &[u8], width: usize, height: usize, params: ScreenToneParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let cell = params.cell_px.clamp(2.0, 128.0);
    let density = params.density.clamp(0.0, 1.0);
    let gradation = params.gradation.clamp(0.0, 1.0);
    let edge = 0.01 + params.softness.clamp(0.0, 1.0) * 0.35;
    let angle = params.angle_degrees.to_radians();
    let cos = angle.cos();
    let sin = angle.sin();
    let cx = width as f32 * 0.5;
    let cy = height as f32 * 0.5;
    let mut out = src.to_vec();

    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let r = src[i] as f32 / 255.0;
            let g = src[i + 1] as f32 / 255.0;
            let b = src[i + 2] as f32 / 255.0;
            let source_tone = 1.0 - luma01(r, g, b);
            let tone = density * lerp_f32(1.0, source_tone, gradation);
            let tone = tone.clamp(0.0, 1.0);
            let fx = x as f32 + 0.5 - cx;
            let fy = y as f32 + 0.5 - cy;
            let u = fx * cos + fy * sin;
            let v = -fx * sin + fy * cos;
            let ink = screen_tone_ink_mask(params.mode, u, v, cell, tone, edge);
            if ink <= 0.0 {
                continue;
            }

            let darken = (ink * strength).clamp(0.0, 1.0);
            for c in 0..3 {
                let base = src[i + c] as f32 / 255.0;
                out[i + c] = to_u8(base * (1.0 - darken));
            }
        }
    }

    out
}

fn screen_tone_ink_mask(
    mode: ScreenToneMode,
    u: f32,
    v: f32,
    cell: f32,
    tone: f32,
    edge: f32,
) -> f32 {
    if tone <= 0.001 {
        return 0.0;
    }
    match mode {
        ScreenToneMode::Dots => {
            let du = centered_periodic_distance(u, cell);
            let dv = centered_periodic_distance(v, cell);
            let dist = (du * du + dv * dv).sqrt();
            let radius = tone.sqrt() * 0.96;
            1.0 - smoothstep(radius, radius + edge, dist)
        }
        ScreenToneMode::Lines => line_tone_ink(v, cell, tone, edge),
        ScreenToneMode::CrossHatch => {
            let primary = line_tone_ink(v, cell, tone, edge);
            let secondary_tone = smoothstep(0.18, 1.0, tone);
            let secondary = line_tone_ink(u, cell, secondary_tone, edge);
            (primary + secondary * (1.0 - primary)).clamp(0.0, 1.0)
        }
    }
}

fn line_tone_ink(coord: f32, cell: f32, tone: f32, edge: f32) -> f32 {
    let width = tone.clamp(0.0, 1.0) * 0.98;
    if width <= 0.001 {
        return 0.0;
    }
    let dist = centered_periodic_distance(coord, cell);
    1.0 - smoothstep(width, width + edge, dist)
}

fn centered_periodic_distance(coord: f32, cell: f32) -> f32 {
    let phase = (coord / cell).rem_euclid(1.0);
    (phase - 0.5).abs() * 2.0
}

fn apply_color_halftone(
    src: &[u8],
    width: usize,
    height: usize,
    params: ColorHalftoneParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let cell = params.cell_px.clamp(3.0, 160.0);
    let dot_gain = params.dot_gain.clamp(-0.5, 0.5);
    let black_generation = params.black_generation.clamp(0.0, 1.0);
    let edge = 0.01 + params.softness.clamp(0.0, 1.0) * 0.35;
    let offset = params.angle_offset_degrees;
    let plate_angles = [
        offset + 15.0, // Cyan
        offset + 75.0, // Magenta
        offset + 0.0,  // Yellow
        offset + 45.0, // Black
    ];
    let mut out = src.to_vec();
    let cx = width as f32 * 0.5;
    let cy = height as f32 * 0.5;

    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let r = src[i] as f32 / 255.0;
            let g = src[i + 1] as f32 / 255.0;
            let b = src[i + 2] as f32 / 255.0;
            let (cyan, magenta, yellow, black) =
                rgb_to_cmyk_ink(r, g, b, black_generation, dot_gain);
            let fx = x as f32 + 0.5 - cx;
            let fy = y as f32 + 0.5 - cy;
            let c_mask = color_halftone_plate_mask(fx, fy, cell, cyan, edge, plate_angles[0]);
            let m_mask = color_halftone_plate_mask(fx, fy, cell, magenta, edge, plate_angles[1]);
            let y_mask = color_halftone_plate_mask(fx, fy, cell, yellow, edge, plate_angles[2]);
            let k_mask = color_halftone_plate_mask(fx, fy, cell, black, edge, plate_angles[3]);
            let target = [
                (1.0 - c_mask) * (1.0 - k_mask),
                (1.0 - m_mask) * (1.0 - k_mask),
                (1.0 - y_mask) * (1.0 - k_mask),
            ];
            for c in 0..3 {
                let base = src[i + c] as f32 / 255.0;
                out[i + c] = to_u8(lerp_f32(base, target[c], strength));
            }
        }
    }

    out
}

fn rgb_to_cmyk_ink(
    r: f32,
    g: f32,
    b: f32,
    black_generation: f32,
    dot_gain: f32,
) -> (f32, f32, f32, f32) {
    let raw_cyan = 1.0 - r.clamp(0.0, 1.0);
    let raw_magenta = 1.0 - g.clamp(0.0, 1.0);
    let raw_yellow = 1.0 - b.clamp(0.0, 1.0);
    let shared = raw_cyan.min(raw_magenta).min(raw_yellow);
    let black = shared * black_generation;
    (
        (raw_cyan - black + dot_gain).clamp(0.0, 1.0),
        (raw_magenta - black + dot_gain).clamp(0.0, 1.0),
        (raw_yellow - black + dot_gain).clamp(0.0, 1.0),
        (black + dot_gain).clamp(0.0, 1.0),
    )
}

fn color_halftone_plate_mask(
    fx: f32,
    fy: f32,
    cell: f32,
    ink: f32,
    edge: f32,
    angle_degrees: f32,
) -> f32 {
    if ink <= 0.001 {
        return 0.0;
    }
    let angle = angle_degrees.to_radians();
    let u = fx * angle.cos() + fy * angle.sin();
    let v = -fx * angle.sin() + fy * angle.cos();
    let du = centered_periodic_distance(u, cell);
    let dv = centered_periodic_distance(v, cell);
    let dist = (du * du + dv * dv).sqrt();
    let radius = ink.sqrt() * 0.98;
    1.0 - smoothstep(radius, radius + edge, dist)
}

fn apply_cmyk_plate_shift(
    src: &[u8],
    width: usize,
    height: usize,
    params: CmykPlateShiftParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let offset = params.offset_px.clamp(0.0, 64.0);
    let black_offset = params.black_offset_px.clamp(-64.0, 64.0);
    let ink_gain = params.ink_gain.clamp(-0.35, 0.35);
    if width == 0
        || height == 0
        || strength <= f32::EPSILON
        || (offset <= f32::EPSILON
            && black_offset.abs() <= f32::EPSILON
            && ink_gain.abs() <= f32::EPSILON)
    {
        return src.to_vec();
    }

    let angle = params.angle_degrees.to_radians();
    let dir = (angle.cos(), angle.sin());
    let perp = (-dir.1, dir.0);
    let black_generation = params.black_generation.clamp(0.0, 1.0);
    let mut out = src.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }
            let xf = x as f32;
            let yf = y as f32;
            let (cyan, _, _, _) = sample_cmyk_ink_alpha_aware(
                src,
                width,
                height,
                xf + dir.0 * offset,
                yf + dir.1 * offset,
                black_generation,
                ink_gain,
            );
            let (_, magenta, _, _) = sample_cmyk_ink_alpha_aware(
                src,
                width,
                height,
                xf - dir.0 * offset,
                yf - dir.1 * offset,
                black_generation,
                ink_gain,
            );
            let (_, _, yellow, _) = sample_cmyk_ink_alpha_aware(
                src,
                width,
                height,
                xf + perp.0 * offset,
                yf + perp.1 * offset,
                black_generation,
                ink_gain,
            );
            let (_, _, _, black) = sample_cmyk_ink_alpha_aware(
                src,
                width,
                height,
                xf - perp.0 * black_offset,
                yf - perp.1 * black_offset,
                black_generation,
                ink_gain,
            );
            let target = [
                1.0 - (cyan + black).clamp(0.0, 1.0),
                1.0 - (magenta + black).clamp(0.0, 1.0),
                1.0 - (yellow + black).clamp(0.0, 1.0),
            ];
            for c in 0..3 {
                let base = src[i + c] as f32 / 255.0;
                out[i + c] = to_u8(lerp_f32(base, target[c], strength));
            }
        }
    }
    out
}

fn sample_cmyk_ink_alpha_aware(
    src: &[u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    black_generation: f32,
    ink_gain: f32,
) -> (f32, f32, f32, f32) {
    let (rgb, alpha) = sample_rgb_bilinear_alpha_aware(src, width, height, x, y);
    if alpha <= f32::EPSILON {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let (cyan, magenta, yellow, black) =
        rgb_to_cmyk_ink(rgb[0], rgb[1], rgb[2], black_generation, ink_gain);
    (cyan * alpha, magenta * alpha, yellow * alpha, black * alpha)
}

fn apply_lithograph(src: &[u8], width: usize, height: usize, params: LithographParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let ink_a = rgb_u8_to_unit(params.ink_a_rgb);
    let ink_b = rgb_u8_to_unit(params.ink_b_rgb);
    let paper_base = rgb_u8_to_unit(params.paper_rgb);
    let density = params.ink_density.clamp(0.0, 1.6);
    let posterization = params.posterization.clamp(0.0, 1.0);
    let grain = params.grain.clamp(0.0, 1.0);
    let paper_texture = params.paper_texture.clamp(0.0, 1.0);
    let offset = params.misregistration_px.clamp(0.0, 32.0);
    let angle = params.angle_degrees.to_radians();
    let offset_dir = (angle.cos() * offset, angle.sin() * offset);
    let texture_scale = 12.0_f32;
    let mut out = src.to_vec();

    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }

            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let (shifted_b, shifted_alpha) = sample_rgb_bilinear_alpha_aware(
                src,
                width,
                height,
                x as f32 + offset_dir.0,
                y as f32 + offset_dir.1,
            );
            let (mut amount_a, _) = lithograph_ink_amounts(base, ink_a, ink_b);
            let (_, mut amount_b) = lithograph_ink_amounts(shifted_b, ink_a, ink_b);
            amount_b *= shifted_alpha;
            amount_a = lithograph_quantize_ink(amount_a * density, posterization);
            amount_b = lithograph_quantize_ink(amount_b * density, posterization);

            let grain_a = glass_value_noise(
                x as f32 / 2.5 + 17.0,
                y as f32 / 2.5 - 11.0,
                params.seed ^ 0xA7E5_135B,
            );
            let grain_b = glass_value_noise(
                x as f32 / 2.8 - 23.0,
                y as f32 / 2.8 + 19.0,
                params.seed ^ 0x5EED_6A1D,
            );
            amount_a = lithograph_apply_grain(amount_a, grain_a, grain);
            amount_b = lithograph_apply_grain(amount_b, grain_b, grain);

            let paper_noise = textureizer_value(
                TextureizerMode::Paper,
                x as f32,
                y as f32,
                texture_scale,
                1.15,
                params.seed ^ 0x1771_0BAD,
            );
            let paper = [
                (paper_base[0] + paper_noise * paper_texture * 0.070).clamp(0.0, 1.0),
                (paper_base[1] + paper_noise * paper_texture * 0.055).clamp(0.0, 1.0),
                (paper_base[2] + paper_noise * paper_texture * 0.035).clamp(0.0, 1.0),
            ];
            let mut target = paper;
            target = lithograph_overprint(target, ink_a, amount_a);
            target = lithograph_overprint(target, ink_b, amount_b);

            for c in 0..3 {
                out[i + c] = to_u8(lerp_f32(base[c], target[c], strength));
            }
        }
    }

    out
}

fn rgb_u8_to_unit(rgb: [u8; 3]) -> [f32; 3] {
    [
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
    ]
}

fn lithograph_ink_amounts(rgb: [f32; 3], ink_a: [f32; 3], ink_b: [f32; 3]) -> (f32, f32) {
    let luma = luma01(rgb[0], rgb[1], rgb[2]);
    let (_, saturation, _) = rgb_to_hsl(rgb[0], rgb[1], rgb[2]);
    let tone = (1.0 - luma).clamp(0.0, 1.0);
    if tone <= f32::EPSILON {
        return (0.0, 0.0);
    }

    let dist_a = rgb_distance_sq(rgb, ink_a);
    let dist_b = rgb_distance_sq(rgb, ink_b);
    let affinity_a = dist_b / (dist_a + dist_b + 0.0001);
    let affinity_b = 1.0 - affinity_a;
    let neutral = (1.0 - saturation) * tone * 0.44;
    let chroma = saturation * tone;
    (
        (neutral + chroma * affinity_a).clamp(0.0, 1.0),
        (neutral + chroma * affinity_b).clamp(0.0, 1.0),
    )
}

fn rgb_distance_sq(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dr = a[0] - b[0];
    let dg = a[1] - b[1];
    let db = a[2] - b[2];
    dr * dr + dg * dg + db * db
}

fn lithograph_quantize_ink(amount: f32, posterization: f32) -> f32 {
    let amount = amount.clamp(0.0, 1.0);
    if posterization <= f32::EPSILON {
        return amount;
    }
    let steps = lerp_f32(12.0, 3.0, posterization).round().max(2.0);
    let quantized = (amount * steps).round() / steps;
    lerp_f32(amount, quantized, posterization)
}

fn lithograph_apply_grain(amount: f32, noise: f32, grain: f32) -> f32 {
    let dropout = smoothstep(0.68, 0.96, -noise) * grain * 0.42;
    (amount * (1.0 + noise * grain * 0.28) * (1.0 - dropout)).clamp(0.0, 1.0)
}

fn lithograph_overprint(base: [f32; 3], ink: [f32; 3], amount: f32) -> [f32; 3] {
    let amount = amount.clamp(0.0, 1.0);
    [
        lerp_f32(base[0], base[0] * ink[0], amount * 0.74),
        lerp_f32(base[1], base[1] * ink[1], amount * 0.74),
        lerp_f32(base[2], base[2] * ink[2], amount * 0.74),
    ]
}

fn apply_engraving(src: &[u8], width: usize, height: usize, params: EngravingParams) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let ink = rgb_u8_to_unit(params.ink_rgb);
    let paper_base = rgb_u8_to_unit(params.paper_rgb);
    let spacing = params.line_spacing_px.clamp(2.0, 48.0);
    let line_width = params.line_width.clamp(0.05, 1.0);
    let angle = params.angle_degrees.to_radians();
    let crosshatch = params.crosshatch.clamp(0.0, 1.0);
    let contour_strength = params.contour_strength.clamp(0.0, 1.0);
    let tone_levels = params.tone_levels.clamp(2.0, 16.0).round();
    let ink_density = params.ink_density.clamp(0.0, 1.8);
    let paper_texture = params.paper_texture.clamp(0.0, 1.0);
    let mut out = src.to_vec();

    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }

            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let luma = luma01(base[0], base[1], base[2]);
            let tone = engraving_quantized_tone(1.0 - luma, tone_levels);
            let xf = x as f32;
            let yf = y as f32;
            let main_line = engraving_line_amount(
                xf,
                yf,
                spacing,
                line_width,
                angle,
                tone,
                params.seed ^ 0xE9A7_1351,
            );
            let deep_shadow = smoothstep(0.35, 0.86, tone) * crosshatch;
            let cross_line = engraving_line_amount(
                xf,
                yf,
                spacing * 0.88,
                line_width * 0.86,
                angle + 1.570_796_4,
                tone,
                params.seed ^ 0xB047_C0DE,
            ) * deep_shadow;
            let contour = engraving_contour_amount(
                src,
                width,
                height,
                x,
                y,
                luma,
                tone_levels,
                contour_strength,
            );
            let mut ink_amount = 1.0 - (1.0 - main_line) * (1.0 - cross_line) * (1.0 - contour);
            let fiber_noise =
                glass_value_noise(xf / 2.2 + 9.0, yf / 2.2 - 13.0, params.seed ^ 0xA11C_E771);
            ink_amount = (ink_amount * (1.0 + fiber_noise * paper_texture * 0.16) * ink_density)
                .clamp(0.0, 1.0);

            let paper_noise = textureizer_value(
                TextureizerMode::Paper,
                xf,
                yf,
                10.0,
                1.1,
                params.seed ^ 0x3D17_5EED,
            );
            let paper = [
                (paper_base[0] + paper_noise * paper_texture * 0.060).clamp(0.0, 1.0),
                (paper_base[1] + paper_noise * paper_texture * 0.052).clamp(0.0, 1.0),
                (paper_base[2] + paper_noise * paper_texture * 0.040).clamp(0.0, 1.0),
            ];
            let target = engraving_overprint(paper, ink, ink_amount);

            for c in 0..3 {
                out[i + c] = to_u8(lerp_f32(base[c], target[c], strength));
            }
        }
    }

    out
}

fn engraving_quantized_tone(tone: f32, levels: f32) -> f32 {
    let tone = tone.clamp(0.0, 1.0);
    let levels = levels.max(2.0);
    let stepped = (tone * levels).round() / levels;
    lerp_f32(tone, stepped, 0.55).clamp(0.0, 1.0)
}

fn engraving_line_amount(
    x: f32,
    y: f32,
    spacing: f32,
    line_width: f32,
    angle: f32,
    tone: f32,
    seed: u32,
) -> f32 {
    if tone <= f32::EPSILON {
        return 0.0;
    }
    let spacing = spacing.max(2.0);
    let wobble = glass_value_noise(x / 18.0, y / 18.0, seed) * spacing * 0.05;
    let u = x * angle.cos() + y * angle.sin() + wobble;
    let phase = (u / spacing).rem_euclid(1.0);
    let dist = phase.min(1.0 - phase) * 2.0;
    let width = lerp_f32(0.05, line_width.clamp(0.05, 1.0), tone).clamp(0.02, 0.98);
    let feather = 0.06 + (1.0 - tone) * 0.08;
    (1.0 - smoothstep(width, (width + feather).min(1.0), dist)) * tone
}

fn engraving_contour_amount(
    src: &[u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    luma: f32,
    tone_levels: f32,
    strength: f32,
) -> f32 {
    if strength <= f32::EPSILON {
        return 0.0;
    }
    let xi = x as isize;
    let yi = y as isize;
    let left = engraving_luma_at(src, width, height, xi - 1, yi, luma);
    let right = engraving_luma_at(src, width, height, xi + 1, yi, luma);
    let up = engraving_luma_at(src, width, height, xi, yi - 1, luma);
    let down = engraving_luma_at(src, width, height, xi, yi + 1, luma);
    let gradient = ((right - left).abs() + (down - up).abs()).clamp(0.0, 1.0);
    let band = (luma * tone_levels.max(2.0)).fract();
    let distance = band.min(1.0 - band);
    let isoline = 1.0 - smoothstep(0.015, 0.085, distance);
    let edge_boost = smoothstep(0.03, 0.24, gradient);
    let shadow_bias = 0.35 + (1.0 - luma).clamp(0.0, 1.0) * 0.65;
    (isoline * (0.20 + edge_boost * 0.80) * shadow_bias * strength).clamp(0.0, 1.0)
}

fn engraving_luma_at(
    src: &[u8],
    width: usize,
    height: usize,
    x: isize,
    y: isize,
    fallback: f32,
) -> f32 {
    if x < 0 || y < 0 {
        return fallback;
    }
    let x = x as usize;
    let y = y as usize;
    if x >= width || y >= height {
        return fallback;
    }
    let i = (y * width + x) * 4;
    if src[i + 3] == 0 {
        return fallback;
    }
    luma01(
        src[i] as f32 / 255.0,
        src[i + 1] as f32 / 255.0,
        src[i + 2] as f32 / 255.0,
    )
}

fn engraving_overprint(base: [f32; 3], ink: [f32; 3], amount: f32) -> [f32; 3] {
    let amount = amount.clamp(0.0, 1.0);
    [
        lerp_f32(base[0], base[0] * ink[0], amount * 0.88),
        lerp_f32(base[1], base[1] * ink[1], amount * 0.88),
        lerp_f32(base[2], base[2] * ink[2], amount * 0.88),
    ]
}

fn apply_newspaper_print(
    src: &[u8],
    width: usize,
    height: usize,
    params: NewspaperPrintParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let cell = params.cell_px.clamp(3.0, 96.0);
    let dot_gain = params.dot_gain.clamp(-0.35, 0.45);
    let ink_bleed = params.ink_bleed.clamp(0.0, 1.0);
    let paper_age = params.paper_age.clamp(0.0, 1.0);
    let paper_texture = params.paper_texture.clamp(0.0, 1.0);
    let contrast = 1.0 + params.contrast.clamp(-1.0, 1.0) * 1.35;
    let fade = params.fade.clamp(0.0, 1.0);
    let edge = 0.01 + ink_bleed * 0.34;
    let angle = 15.0_f32.to_radians();
    let cos = angle.cos();
    let sin = angle.sin();
    let cx = width as f32 * 0.5;
    let cy = height as f32 * 0.5;
    let texture_scale = (cell * 1.45).clamp(5.0, 120.0);
    let mut out = src.to_vec();

    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            if src[i + 3] == 0 {
                continue;
            }

            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let luma = luma01(base[0], base[1], base[2]);
            let print_luma = ((luma - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
            let tone = (1.0 - print_luma + dot_gain).clamp(0.0, 1.0);
            let fx = x as f32 + 0.5 - cx;
            let fy = y as f32 + 0.5 - cy;
            let u = fx * cos + fy * sin;
            let v = -fx * sin + fy * cos;
            let ink_mask = screen_tone_ink_mask(ScreenToneMode::Dots, u, v, cell, tone, edge);
            let print_noise = glass_value_noise(
                x as f32 / (cell * 0.42).max(1.0) + 17.0,
                y as f32 / (cell * 0.42).max(1.0) - 9.0,
                params.seed ^ 0xBEE5_2DAD,
            );
            let ink_noise = (0.92 + print_noise * 0.16).clamp(0.70, 1.08);
            let ink_alpha = (ink_mask * (1.0 - fade * 0.34) * ink_noise).clamp(0.0, 1.0);

            let paper_noise = textureizer_value(
                TextureizerMode::Paper,
                x as f32,
                y as f32,
                texture_scale,
                1.05,
                params.seed,
            );
            let fiber = signed_noise(
                x as u32,
                (y as u32).wrapping_mul(3),
                params.seed ^ 0x5A17_9E21,
            );
            let paper_variation = (paper_noise * 0.075 + fiber * 0.025) * paper_texture;
            let paper = [
                (0.995 - paper_age * 0.085 + paper_variation).clamp(0.0, 1.0),
                (0.972 - paper_age * 0.125 + paper_variation * 0.82).clamp(0.0, 1.0),
                (0.900 - paper_age * 0.225 + paper_variation * 0.56).clamp(0.0, 1.0),
            ];
            let ink_floor = 0.035 + fade * 0.155 + paper_age * 0.035;
            let ink = [
                (ink_floor * 1.06).clamp(0.0, 1.0),
                (ink_floor * 0.98).clamp(0.0, 1.0),
                (ink_floor * 0.82).clamp(0.0, 1.0),
            ];
            let target = [
                lerp_f32(paper[0], ink[0], ink_alpha),
                lerp_f32(paper[1], ink[1], ink_alpha),
                lerp_f32(paper[2], ink[2], ink_alpha),
            ];

            for c in 0..3 {
                out[i + c] = to_u8(lerp_f32(base[c], target[c], strength));
            }
        }
    }

    out
}

fn apply_textureizer(
    src: &[u8],
    width: usize,
    height: usize,
    params: TextureizerParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 1.0);
    let depth = params.depth.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || depth <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let scale = params.scale_px.clamp(2.0, 96.0);
    let contrast = params.contrast.clamp(0.0, 2.0);
    let warmth = params.warmth.clamp(-1.0, 1.0);
    let mut out = src.to_vec();

    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let base = [
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            ];
            let texture = textureizer_value(
                params.mode,
                x as f32,
                y as f32,
                scale,
                contrast,
                params.seed,
            );
            let plate = (0.5 + texture * depth * 0.48).clamp(0.0, 1.0);
            let mut target = [
                soft_light_channel(base[0], plate),
                soft_light_channel(base[1], plate),
                soft_light_channel(base[2], plate),
            ];
            apply_textureizer_warmth(&mut target, warmth, texture);
            for c in 0..3 {
                out[i + c] = to_u8(lerp_f32(base[c], target[c], strength));
            }
        }
    }

    out
}

fn textureizer_value(
    mode: TextureizerMode,
    x: f32,
    y: f32,
    scale: f32,
    contrast: f32,
    seed: u32,
) -> f32 {
    let raw = match mode {
        TextureizerMode::Paper => {
            let coarse = glass_value_noise(x / scale, y / scale, seed);
            let mid = glass_value_noise(
                x / (scale * 0.43) + 19.3,
                y / (scale * 0.43) - 7.1,
                seed ^ 0xCAFE_71E5,
            );
            let fiber = signed_noise((x / 2.0) as u32, y as u32, seed ^ 0xA11C_E5E1);
            coarse * 0.58 + mid * 0.30 + fiber * 0.12
        }
        TextureizerMode::Canvas => {
            let warp = (x / scale * std::f32::consts::TAU).sin().abs();
            let weft = (y / scale * std::f32::consts::TAU).sin().abs();
            let weave_phase = if ((x / scale).floor() as i32 + (y / scale).floor() as i32) & 1 == 0
            {
                1.0
            } else {
                -1.0
            };
            let weave = (warp + weft - 1.0) * 0.72 + (warp - weft) * weave_phase * 0.26;
            let rough = glass_value_noise(x / (scale * 1.7), y / (scale * 1.7), seed);
            weave + rough * 0.22
        }
        TextureizerMode::Linen => {
            let vertical = (x / (scale * 0.42) * std::f32::consts::TAU).sin().abs() * 2.0 - 1.0;
            let horizontal = (y / (scale * 1.15) * std::f32::consts::TAU).sin().abs() * 2.0 - 1.0;
            let long_fiber = glass_value_noise(x / (scale * 0.6), y / (scale * 3.2), seed);
            vertical * 0.46 + horizontal * 0.22 + long_fiber * 0.32
        }
    };
    (raw * contrast).clamp(-1.0, 1.0)
}

fn apply_textureizer_warmth(rgb: &mut [f32; 3], warmth: f32, texture: f32) {
    let amount = warmth.abs() * (0.35 + texture.abs() * 0.65);
    if amount <= f32::EPSILON {
        return;
    }
    if warmth >= 0.0 {
        rgb[0] = (rgb[0] + (1.0 - rgb[0]) * amount * 0.050).clamp(0.0, 1.0);
        rgb[1] = (rgb[1] + (1.0 - rgb[1]) * amount * 0.026).clamp(0.0, 1.0);
        rgb[2] = (rgb[2] * (1.0 - amount * 0.055)).clamp(0.0, 1.0);
    } else {
        rgb[0] = (rgb[0] * (1.0 - amount * 0.040)).clamp(0.0, 1.0);
        rgb[1] = (rgb[1] + (1.0 - rgb[1]) * amount * 0.012).clamp(0.0, 1.0);
        rgb[2] = (rgb[2] + (1.0 - rgb[2]) * amount * 0.052).clamp(0.0, 1.0);
    }
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

fn apply_diffraction_starburst(
    src: &[u8],
    width: usize,
    height: usize,
    params: DiffractionStarburstParams,
) -> Vec<u8> {
    let strength = params.strength.clamp(0.0, 3.0);
    let length = params.length_px.clamp(1.0, 360.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let blade_count = normalize_diffraction_blade_count(params.blade_count);
    let ray_count = diffraction_ray_count(blade_count);
    let max_steps = length.round().clamp(1.0, 360.0) as usize;
    let width_px = params.width_px.clamp(0.4, 12.0);
    let threshold = params.threshold.clamp(0.0, 0.9999);
    let inv_range = 1.0 / (1.0 - threshold).max(0.001);
    let rotation = params.rotation_degrees.to_radians();
    let halo_radius = params.halo_radius_px.round().clamp(0.0, 96.0) as usize;
    let chromatic_shift = params.chromatic_shift.clamp(0.0, 1.0);
    let mut dirs = Vec::with_capacity(ray_count as usize);
    for ray in 0..ray_count {
        let angle = rotation + std::f32::consts::TAU * ray as f32 / ray_count as f32;
        dirs.push((angle.cos(), angle.sin()));
    }

    let mut streak = vec![0.0_f32; width * height * 3];
    let mut bright = vec![0_u8; src.len()];
    for y in 0..height {
        for x in 0..width {
            let o = (y * width + x) * 4;
            let alpha = src[o + 3] as f32 / 255.0;
            if alpha <= f32::EPSILON {
                continue;
            }
            let r = src[o] as f32 / 255.0;
            let g = src[o + 1] as f32 / 255.0;
            let b = src[o + 2] as f32 / 255.0;
            let signal = luma01(r, g, b).max(r.max(g).max(b) * 0.92);
            let weight = ((signal - threshold) * inv_range).clamp(0.0, 1.0);
            let weight = weight * weight * alpha;
            if weight <= 0.001 {
                continue;
            }

            let color = [
                (r + (1.0 - r) * 0.20) * weight,
                (g + (1.0 - g) * 0.20) * weight,
                (b + (1.0 - b) * 0.20) * weight,
            ];
            for c in 0..3 {
                bright[o + c] = to_u8(color[c]);
            }
            bright[o + 3] = src[o + 3];

            for &(dx, dy) in &dirs {
                let px = -dy;
                let py = dx;
                for step in 1..=max_steps {
                    let distance = step as f32;
                    let linear = 1.0 - distance / (max_steps as f32 + 1.0);
                    if linear <= 0.0 {
                        break;
                    }
                    let falloff = linear.powf(1.25) * (-distance / (length * 0.62)).exp();
                    if falloff <= 0.0001 {
                        continue;
                    }
                    let base_x = x as f32 + dx * distance;
                    let base_y = y as f32 + dy * distance;
                    if base_x < -width_px
                        || base_y < -width_px
                        || base_x > width as f32 - 1.0 + width_px
                        || base_y > height as f32 - 1.0 + width_px
                    {
                        break;
                    }

                    let side_offsets = [
                        (0.0, 1.0),
                        (-0.42 * width_px, 0.46),
                        (0.42 * width_px, 0.46),
                    ];
                    for (side, side_weight) in side_offsets {
                        let sx = base_x + px * side;
                        let sy = base_y + py * side;
                        if sx < -0.001
                            || sy < -0.001
                            || sx > width as f32 - 1.0 + 0.001
                            || sy > height as f32 - 1.0 + 0.001
                        {
                            continue;
                        }
                        let mut ray_color = color;
                        if chromatic_shift > 0.0 {
                            let phase = (distance / length).clamp(0.0, 1.0);
                            let blue_bias = phase * chromatic_shift * 0.28;
                            let red_bias = (1.0 - phase * 0.4) * chromatic_shift * 0.10;
                            ray_color[0] *= 1.0 + red_bias;
                            ray_color[1] *= 1.0 - blue_bias * 0.25;
                            ray_color[2] *= 1.0 + blue_bias;
                        }
                        add_bilinear_rgb(
                            &mut streak,
                            width,
                            height,
                            sx,
                            sy,
                            ray_color,
                            falloff * side_weight,
                        );
                    }
                }
            }
        }
    }

    let halo = if halo_radius > 0 {
        Some(box_blur_rgba(&bright, width, height, halo_radius))
    } else {
        None
    };
    let mut out = src.to_vec();
    let ray_scale = strength / (ray_count as f32 * 0.42).max(1.0);
    let halo_scale = strength * 0.35;
    for i in 0..width * height {
        let si = i * 3;
        let oi = i * 4;
        if src[oi + 3] == 0 {
            continue;
        }
        for c in 0..3 {
            let base = src[oi + c] as f32 / 255.0;
            let ray = streak[si + c] * ray_scale;
            let halo_add = halo
                .as_ref()
                .map(|h| h[oi + c] as f32 / 255.0 * halo_scale)
                .unwrap_or(0.0);
            out[oi + c] = to_u8(base + ray + halo_add);
        }
    }
    out
}

fn normalize_diffraction_blade_count(blade_count: u32) -> u32 {
    blade_count.clamp(3, 12)
}

fn diffraction_ray_count(blade_count: u32) -> u32 {
    if blade_count % 2 == 0 {
        blade_count
    } else {
        blade_count * 2
    }
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

fn apply_despeckle(src: &[u8], width: usize, height: usize, params: DespeckleParams) -> Vec<u8> {
    let radius = params.radius_px.round().clamp(1.0, 4.0) as i32;
    let strength = params.strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || width == 0 || height == 0 {
        return src.to_vec();
    }

    let threshold = params.threshold.clamp(1.0, 255.0);
    let upper_threshold = (threshold * 1.7).min(255.0).max(threshold + 1.0);
    let offsets = circle_offsets(radius);
    let mut out = src.to_vec();
    let mut red = Vec::with_capacity(offsets.len().saturating_sub(1));
    let mut green = Vec::with_capacity(offsets.len().saturating_sub(1));
    let mut blue = Vec::with_capacity(offsets.len().saturating_sub(1));

    for y in 0..height {
        for x in 0..width {
            red.clear();
            green.clear();
            blue.clear();
            for (dx, dy) in &offsets {
                if *dx == 0 && *dy == 0 {
                    continue;
                }
                let xx = (x as i32 + *dx).clamp(0, width as i32 - 1) as usize;
                let yy = (y as i32 + *dy).clamp(0, height as i32 - 1) as usize;
                let i = (yy * width + xx) * 4;
                red.push(src[i]);
                green.push(src[i + 1]);
                blue.push(src[i + 2]);
            }
            if red.is_empty() {
                continue;
            }

            red.sort_unstable();
            green.sort_unstable();
            blue.sort_unstable();
            let mid = red.len() / 2;
            let target = [red[mid], green[mid], blue[mid]];
            let i = (y * width + x) * 4;
            let dr = src[i] as f32 - target[0] as f32;
            let dg = src[i + 1] as f32 - target[1] as f32;
            let db = src[i + 2] as f32 - target[2] as f32;
            let distance = ((dr * dr + dg * dg + db * db) / 3.0).sqrt();
            let amount = smoothstep(threshold, upper_threshold, distance) * strength;
            if amount <= f32::EPSILON {
                continue;
            }
            for c in 0..3 {
                out[i + c] = lerp_u8(src[i + c], target[c], amount);
            }
        }
    }
    out
}

fn blend_rgb_with_mask(base: &mut [u8], effected: &[u8], mask: &[f32]) {
    base.par_chunks_exact_mut(4)
        .zip(effected.par_chunks_exact(4))
        .zip(mask.par_iter())
        .for_each(|((base, effected), amount)| {
            let amount = amount.clamp(0.0, 1.0);
            for c in 0..3 {
                base[c] = lerp_u8(base[c], effected[c], amount);
            }
            // Keep source alpha stable; local adjustments are visual RGB operations.
        });
}

fn blend_rgb_with_effect_alpha_mask(base: &mut [u8], effected: &[u8], mask: &[f32]) {
    base.par_chunks_exact_mut(4)
        .zip(effected.par_chunks_exact(4))
        .zip(mask.par_iter())
        .for_each(|((base, effected), mask_amount)| {
            let amount = (effected[3] as f32 / 255.0 * mask_amount.clamp(0.0, 1.0)).clamp(0.0, 1.0);
            if amount <= f32::EPSILON {
                return;
            }
            for c in 0..3 {
                base[c] = lerp_u8(base[c], effected[c], amount);
            }
            // Keep source alpha stable; outline stroke is a visual RGB overlay.
        });
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

fn sample_rgb_bilinear_alpha_aware(
    src: &[u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
) -> ([f32; 3], f32) {
    if width == 0 || height == 0 {
        return ([0.0; 3], 0.0);
    }
    let x = x.clamp(0.0, width.saturating_sub(1) as f32);
    let y = y.clamp(0.0, height.saturating_sub(1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let samples = [
        ((y0 * width + x0) * 4, (1.0 - tx) * (1.0 - ty)),
        ((y0 * width + x1) * 4, tx * (1.0 - ty)),
        ((y1 * width + x0) * 4, (1.0 - tx) * ty),
        ((y1 * width + x1) * 4, tx * ty),
    ];
    let mut rgb_sum = [0.0_f32; 3];
    let mut alpha_sum = 0.0_f32;
    for (i, weight) in samples {
        let alpha = src[i + 3] as f32 / 255.0;
        let weighted_alpha = weight * alpha;
        if weighted_alpha <= f32::EPSILON {
            continue;
        }
        for c in 0..3 {
            rgb_sum[c] += src[i + c] as f32 / 255.0 * weighted_alpha;
        }
        alpha_sum += weighted_alpha;
    }
    if alpha_sum <= f32::EPSILON {
        return ([0.0; 3], 0.0);
    }
    (
        [
            rgb_sum[0] / alpha_sum,
            rgb_sum[1] / alpha_sum,
            rgb_sum[2] / alpha_sum,
        ],
        alpha_sum,
    )
}

fn nearest_pixel_index(width: usize, height: usize, x: f32, y: f32) -> usize {
    let xx = x.round().clamp(0.0, width.saturating_sub(1) as f32) as usize;
    let yy = y.round().clamp(0.0, height.saturating_sub(1) as f32) as usize;
    (yy * width + xx) * 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn solid(width: usize, height: usize, rgba: [u8; 4]) -> RgbaImageBuf {
        let mut pixels = Vec::with_capacity(width * height * 4);
        for _ in 0..width * height {
            pixels.extend_from_slice(&rgba);
        }
        RgbaImageBuf::new(width, height, pixels).unwrap()
    }

    fn patterned(width: usize, height: usize) -> RgbaImageBuf {
        let mut pixels = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                let checker = if (x / 4 + y / 4) % 2 == 0 { 48 } else { 206 };
                let r = ((x * 9 + y * 3 + checker) % 256) as u8;
                let g = ((x * 5 + y * 11 + 64) % 256) as u8;
                let b = ((x * 13 + y * 7 + 118) % 256) as u8;
                let a = if x == 0 || y == 0 || x + 1 == width || y + 1 == height {
                    0
                } else {
                    255
                };
                pixels.extend_from_slice(&[r, g, b, a]);
            }
        }
        RgbaImageBuf::new(width, height, pixels).unwrap()
    }

    fn center_rect_mask(width: usize, height: usize) -> LocalMask {
        let mut alpha = vec![0.0; width * height];
        let x0 = width / 4;
        let x1 = width - x0;
        let y0 = height / 4;
        let y1 = height - y0;
        for y in y0..y1 {
            for x in x0..x1 {
                alpha[y * width + x] = 1.0;
            }
        }
        LocalMask::Raster(RasterMask {
            width,
            height,
            alpha,
        })
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
    fn spreading_effects_default_to_mask_before_without_mask_after() {
        let wind = LocalAdjustmentLayer::new(
            "wind",
            LocalMask::Full,
            LocalEffect::Wind(WindParams::default()),
        );
        assert!(wind.mask_before_effect);
        assert!(!wind.mask_after_effect);

        let halation = LocalAdjustmentLayer::new(
            "halation",
            LocalMask::Full,
            LocalEffect::Halation(HalationParams::default()),
        );
        assert!(halation.mask_before_effect);
        assert!(!halation.mask_after_effect);

        let outline = LocalAdjustmentLayer::new(
            "outline",
            LocalMask::Full,
            LocalEffect::OutlineStroke(OutlineStrokeParams::default()),
        );
        assert!(outline.mask_before_effect);
        assert!(!outline.mask_after_effect);

        let rim_light = LocalAdjustmentLayer::new(
            "rim light",
            LocalMask::Full,
            LocalEffect::RimLight(RimLightParams::default()),
        );
        assert!(rim_light.mask_before_effect);
        assert!(!rim_light.mask_after_effect);

        let contact_shadow = LocalAdjustmentLayer::new(
            "contact shadow",
            LocalMask::Full,
            LocalEffect::ContactShadow(ContactShadowParams::default()),
        );
        assert!(contact_shadow.mask_before_effect);
        assert!(contact_shadow.mask_after_effect);

        let color_dodge_glow = LocalAdjustmentLayer::new(
            "color dodge glow",
            LocalMask::Full,
            LocalEffect::ColorDodgeGlow(ColorDodgeGlowParams::default()),
        );
        assert!(color_dodge_glow.mask_before_effect);
        assert!(!color_dodge_glow.mask_after_effect);

        let anamorphic_flare = LocalAdjustmentLayer::new(
            "anamorphic flare",
            LocalMask::Full,
            LocalEffect::AnamorphicFlare(AnamorphicFlareParams::default()),
        );
        assert!(anamorphic_flare.mask_before_effect);
        assert!(!anamorphic_flare.mask_after_effect);

        let diffraction_starburst = LocalAdjustmentLayer::new(
            "diffraction starburst",
            LocalMask::Full,
            LocalEffect::DiffractionStarburst(DiffractionStarburstParams::default()),
        );
        assert!(diffraction_starburst.mask_before_effect);
        assert!(!diffraction_starburst.mask_after_effect);

        let bokeh_sprite = LocalAdjustmentLayer::new(
            "bokeh sprite",
            LocalMask::Full,
            LocalEffect::BokehSprite(BokehSpriteParams::default()),
        );
        assert!(bokeh_sprite.mask_before_effect);
        assert!(!bokeh_sprite.mask_after_effect);

        let wave = LocalAdjustmentLayer::new(
            "wave",
            LocalMask::Full,
            LocalEffect::WaveDistortion(WaveDistortionParams::default()),
        );
        assert!(!wave.mask_before_effect);
        assert!(wave.mask_after_effect);
    }

    #[test]
    fn mask_before_without_mask_after_lets_wind_escape_mask() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255,
            ],
        )
        .unwrap();
        let mask = LocalMask::Raster(RasterMask {
            width: 5,
            height: 1,
            alpha: vec![0.0, 1.0, 0.0, 0.0, 0.0],
        });
        let effect = LocalEffect::Wind(WindParams {
            direction: WindDirection::Right,
            source: WindSource::Bright,
            distance_px: 3.0,
            threshold: 0.0,
            softness: 0.01,
            turbulence: 0.0,
            strength: 1.0,
            seed: 1,
        });
        let mut layer = LocalAdjustmentLayer::new("wind", mask, effect);
        layer.mask_before_effect = true;
        layer.mask_after_effect = false;

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        assert!(out.pixels[2 * 4] > 0, "wind should leak to the right");
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn mask_before_without_mask_after_lets_bokeh_sprite_escape_mask() {
        let mut pixels = vec![0_u8; 5 * 5 * 4];
        for px in pixels.chunks_exact_mut(4) {
            px[3] = 255;
        }
        let center = (2 * 5 + 2) * 4;
        pixels[center..center + 4].copy_from_slice(&[255, 255, 255, 255]);
        let src = RgbaImageBuf::new(5, 5, pixels).unwrap();
        let mask = LocalMask::Raster(RasterMask {
            width: 5,
            height: 5,
            alpha: vec![
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            ],
        });
        let layer = LocalAdjustmentLayer::new(
            "bokeh",
            mask,
            LocalEffect::BokehSprite(BokehSpriteParams {
                threshold: 0.30,
                density: 1.0,
                size_px: 5.0,
                strength: 1.0,
                ..BokehSpriteParams::default()
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        let outside_mask = (2 * 5 + 1) * 4;
        assert!(
            out.pixels[outside_mask] > 0,
            "bokeh sprite should leak outside the source mask"
        );
        assert_eq!(out.pixels[outside_mask + 3], 255);
    }

    #[test]
    fn mask_after_clips_wind_that_would_escape_mask() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255,
            ],
        )
        .unwrap();
        let mask = LocalMask::Raster(RasterMask {
            width: 5,
            height: 1,
            alpha: vec![0.0, 1.0, 0.0, 0.0, 0.0],
        });
        let effect = LocalEffect::Wind(WindParams {
            direction: WindDirection::Right,
            source: WindSource::Bright,
            distance_px: 3.0,
            threshold: 0.0,
            softness: 0.01,
            turbulence: 0.0,
            strength: 1.0,
            seed: 1,
        });
        let mut layer = LocalAdjustmentLayer::new("wind", mask, effect);
        layer.mask_before_effect = true;
        layer.mask_after_effect = true;

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        assert_eq!(out.pixels[2 * 4], 0, "post mask should clip escaped wind");
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn outline_stroke_paints_outside_mask_and_preserves_alpha() {
        let src = solid(3, 3, [200, 200, 200, 255]);
        let mask = LocalMask::Raster(RasterMask {
            width: 3,
            height: 3,
            alpha: vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        });
        let layer = LocalAdjustmentLayer::new(
            "outline",
            mask,
            LocalEffect::OutlineStroke(OutlineStrokeParams {
                placement: OutlineStrokePlacement::Outside,
                width_px: 1.0,
                softness_px: 0.0,
                opacity: 1.0,
                color_rgb: [0, 0, 0],
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        let top_center = 1 * 4;
        let center = (3 + 1) * 4;
        assert_eq!(&out.pixels[top_center..top_center + 3], &[0, 0, 0]);
        assert_eq!(&out.pixels[center..center + 3], &[200, 200, 200]);
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 255));
    }

    #[test]
    fn outline_stroke_inside_full_mask_paints_image_border() {
        let src = solid(5, 5, [200, 200, 200, 255]);
        let layer = LocalAdjustmentLayer::new(
            "outline",
            LocalMask::Full,
            LocalEffect::OutlineStroke(OutlineStrokeParams {
                placement: OutlineStrokePlacement::Inside,
                width_px: 1.0,
                softness_px: 0.0,
                opacity: 1.0,
                color_rgb: [0, 0, 0],
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        let top_left = 0;
        let top_center = 2 * 4;
        let center = (2 * 5 + 2) * 4;
        assert_eq!(&out.pixels[top_left..top_left + 3], &[0, 0, 0]);
        assert_eq!(&out.pixels[top_center..top_center + 3], &[0, 0, 0]);
        assert_eq!(&out.pixels[center..center + 3], &[200, 200, 200]);
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 255));
    }

    #[test]
    fn rim_light_paints_only_light_facing_mask_edge() {
        let src = solid(3, 3, [80, 80, 80, 255]);
        let mask = LocalMask::Raster(RasterMask {
            width: 3,
            height: 3,
            alpha: vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        });
        let layer = LocalAdjustmentLayer::new(
            "rim",
            mask,
            LocalEffect::RimLight(RimLightParams {
                light_angle_degrees: 0.0,
                width_px: 1.0,
                falloff: 0.0,
                strength: 1.0,
                color_rgb: [255, 255, 255],
                wrap: 0.0,
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        let left = (3 + 0) * 4;
        let right = (3 + 2) * 4;
        let center = (3 + 1) * 4;
        assert_eq!(&out.pixels[left..left + 3], &[80, 80, 80]);
        assert_eq!(&out.pixels[center..center + 3], &[80, 80, 80]);
        assert!(out.pixels[right] > 80);
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 255));
    }

    #[test]
    fn rim_light_full_mask_uses_image_border_as_edge() {
        let src = solid(5, 5, [80, 80, 80, 255]);
        let layer = LocalAdjustmentLayer::new(
            "rim",
            LocalMask::Full,
            LocalEffect::RimLight(RimLightParams {
                light_angle_degrees: 0.0,
                width_px: 1.0,
                falloff: 0.0,
                strength: 1.0,
                color_rgb: [255, 255, 255],
                wrap: 0.0,
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        let left = (2 * 5) * 4;
        let right = (2 * 5 + 4) * 4;
        let center = (2 * 5 + 2) * 4;
        assert_eq!(&out.pixels[left..left + 3], &[80, 80, 80]);
        assert_eq!(&out.pixels[center..center + 3], &[80, 80, 80]);
        assert!(out.pixels[right] > 80);
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 255));
    }

    #[test]
    fn contact_shadow_darkens_inner_mask_edge_and_preserves_center() {
        let src = solid(5, 5, [120, 120, 120, 255]);
        let mut alpha = vec![0.0; 25];
        for y in 1..4 {
            for x in 1..4 {
                alpha[y * 5 + x] = 1.0;
            }
        }
        let mask = LocalMask::Raster(RasterMask {
            width: 5,
            height: 5,
            alpha,
        });
        let layer = LocalAdjustmentLayer::new(
            "contact",
            mask,
            LocalEffect::ContactShadow(ContactShadowParams {
                radius_px: 1.0,
                softness_px: 0.0,
                strength: 1.0,
                color_rgb: [0, 0, 0],
                direction_degrees: 90.0,
                directionality: 0.0,
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        let top_inner = (5 + 2) * 4;
        let center = (2 * 5 + 2) * 4;
        let outside = 2 * 4;
        assert_eq!(&out.pixels[top_inner..top_inner + 3], &[0, 0, 0]);
        assert_eq!(&out.pixels[center..center + 3], &[120, 120, 120]);
        assert_eq!(&out.pixels[outside..outside + 3], &[120, 120, 120]);
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 255));
    }

    #[test]
    fn contact_shadow_full_mask_uses_image_border_as_edge() {
        let src = solid(5, 5, [120, 120, 120, 255]);
        let layer = LocalAdjustmentLayer::new(
            "contact",
            LocalMask::Full,
            LocalEffect::ContactShadow(ContactShadowParams {
                radius_px: 1.0,
                softness_px: 0.0,
                strength: 1.0,
                color_rgb: [0, 0, 0],
                direction_degrees: 90.0,
                directionality: 0.0,
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        let top_center = 2 * 4;
        let center = (2 * 5 + 2) * 4;
        assert_eq!(&out.pixels[top_center..top_center + 3], &[0, 0, 0]);
        assert_eq!(&out.pixels[center..center + 3], &[120, 120, 120]);
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 255));
    }

    #[test]
    fn contact_shadow_can_target_shadow_direction() {
        let src = solid(5, 5, [120, 120, 120, 255]);
        let mut alpha = vec![0.0; 25];
        for y in 1..4 {
            for x in 1..4 {
                alpha[y * 5 + x] = 1.0;
            }
        }
        let mask = LocalMask::Raster(RasterMask {
            width: 5,
            height: 5,
            alpha,
        });
        let layer = LocalAdjustmentLayer::new(
            "contact",
            mask,
            LocalEffect::ContactShadow(ContactShadowParams {
                radius_px: 1.0,
                softness_px: 0.0,
                strength: 1.0,
                color_rgb: [0, 0, 0],
                direction_degrees: 90.0,
                directionality: 1.0,
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        let top_inner = (5 + 2) * 4;
        let bottom_inner = (3 * 5 + 2) * 4;
        assert_eq!(&out.pixels[top_inner..top_inner + 3], &[120, 120, 120]);
        assert_eq!(&out.pixels[bottom_inner..bottom_inner + 3], &[0, 0, 0]);
    }

    #[test]
    fn outline_stroke_reports_incremental_progress_for_large_width() {
        let src = solid(32, 32, [200, 200, 200, 255]);
        let mut alpha = vec![0.0; 32 * 32];
        for y in 12..20 {
            for x in 12..20 {
                alpha[y * 32 + x] = 1.0;
            }
        }
        let mask = LocalMask::Raster(RasterMask {
            width: 32,
            height: 32,
            alpha,
        });
        let layer = LocalAdjustmentLayer::new(
            "outline",
            mask,
            LocalEffect::OutlineStroke(OutlineStrokeParams {
                placement: OutlineStrokePlacement::Outside,
                width_px: 24.0,
                softness_px: 0.0,
                opacity: 1.0,
                color_rgb: [0, 0, 0],
            }),
        );

        let mut progress = Vec::new();
        apply_layers_with_progress(src.as_ref(), &[layer], None, |p| {
            if p.effect_name == "縁取り" {
                progress.push(p.percent);
            }
        })
        .unwrap();

        assert!(progress.first().copied().unwrap_or(1.0) <= f32::EPSILON);
        assert!(progress.iter().any(|&p| p > 0.10 && p < 0.95));
        assert!(progress.last().copied().unwrap_or(0.0) >= 1.0);
    }

    #[test]
    fn color_trace_recolors_dark_line_from_neighbor_color() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                240, 80, 40, 255, 240, 80, 40, 255, 0, 0, 0, 255, 240, 80, 40, 255, 240, 80, 40,
                255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "color trace",
            LocalMask::Full,
            LocalEffect::ColorTrace(ColorTraceParams {
                strength: 1.0,
                line_threshold: 0.20,
                softness: 0.05,
                sample_radius_px: 2.0,
                darkness: 0.50,
                saturation: 0.0,
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        let line = 2 * 4;
        assert!(out.pixels[line] > out.pixels[line + 1]);
        assert!(out.pixels[line] > 32);
        assert_eq!(&out.pixels[0..4], &src.pixels[0..4]);
        assert_eq!(out.pixels[line + 3], 255);
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
    fn wind_right_extends_bright_pixel_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                0, 0, 0, 77, 255, 255, 255, 255, 0, 0, 0, 79, 0, 0, 0, 80, 0, 0, 0, 81,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "wind",
            LocalMask::Full,
            LocalEffect::Wind(WindParams {
                direction: WindDirection::Right,
                source: WindSource::Bright,
                distance_px: 3.0,
                threshold: 0.5,
                softness: 0.01,
                turbulence: 0.0,
                strength: 1.0,
                seed: 1,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let trail = 2 * 4;
        assert!(out.pixels[trail] > 96);
        assert_eq!(out.pixels[0], 0);
        assert_eq!(out.pixels[trail + 3], 79);
    }

    #[test]
    fn wind_left_extends_dark_pixel() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255,
                255, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "wind",
            LocalMask::Full,
            LocalEffect::Wind(WindParams {
                direction: WindDirection::Left,
                source: WindSource::Dark,
                distance_px: 2.0,
                threshold: 0.5,
                softness: 0.01,
                turbulence: 0.0,
                strength: 1.0,
                seed: 1,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let trail = 1 * 4;
        assert!(out.pixels[trail] < 200);
        assert_eq!(out.pixels[4 * 4], 255);
    }

    #[test]
    fn wind_zero_strength_is_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "wind",
            LocalMask::Full,
            LocalEffect::Wind(WindParams {
                direction: WindDirection::Down,
                source: WindSource::Edge,
                distance_px: 12.0,
                threshold: 0.0,
                softness: 0.001,
                turbulence: 1.0,
                strength: 0.0,
                seed: 7,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
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
    fn bokeh_sprite_draws_from_highlight_and_preserves_alpha() {
        let mut pixels = vec![0_u8; 7 * 7 * 4];
        for px in pixels.chunks_exact_mut(4) {
            px[3] = 111;
        }
        let center = (3 * 7 + 3) * 4;
        pixels[center..center + 4].copy_from_slice(&[255, 230, 180, 111]);
        let src = RgbaImageBuf::new(7, 7, pixels).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "bokeh",
            LocalMask::Full,
            LocalEffect::BokehSprite(BokehSpriteParams {
                shape: BokehSpriteShape::Circle,
                threshold: 0.20,
                density: 1.0,
                size_px: 5.0,
                softness: 0.40,
                brightness: 1.0,
                color_strength: 0.50,
                seed: 3,
                strength: 1.0,
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        let lifted_neighbor = (0..7 * 7).any(|idx| {
            let i = idx * 4;
            i != center && out.pixels[i] > src.pixels[i]
        });
        assert!(lifted_neighbor);
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 111));
    }

    #[test]
    fn bokeh_sprite_shapes_are_distinct() {
        let circle_corner = bokeh_sprite_shape_alpha(BokehSpriteShape::Circle, 0.72, 0.72, 0.06);
        let star_corner = bokeh_sprite_shape_alpha(BokehSpriteShape::Star, 0.72, 0.72, 0.06);
        let star_point = bokeh_sprite_shape_alpha(BokehSpriteShape::Star, 1.0, 0.0, 0.06);
        let heart_top = bokeh_sprite_shape_alpha(BokehSpriteShape::Heart, 0.0, -0.62, 0.06);
        let heart_bottom = bokeh_sprite_shape_alpha(BokehSpriteShape::Heart, 0.0, 0.92, 0.06);

        assert!(circle_corner < 0.5);
        assert!(star_corner < circle_corner);
        assert!(star_point > star_corner);
        assert!(heart_top > 0.5);
        assert!(heart_bottom > 0.0);
    }

    #[test]
    fn lens_dirt_water_drops_change_image_and_preserve_alpha() {
        let src = solid(8, 8, [90, 100, 120, 173]);
        let layer = LocalAdjustmentLayer::new(
            "lens dirt",
            LocalMask::Full,
            LocalEffect::LensDirt(LensDirtParams {
                mode: LensDirtMode::WaterDrops,
                density: 1.0,
                size_px: 7.0,
                opacity: 1.0,
                softness: 0.35,
                highlight_response: 0.0,
                distortion_px: 6.0,
                seed: 3,
                strength: 1.0,
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        assert_ne!(out.pixels, src.pixels);
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 173));
    }

    #[test]
    fn lens_dirt_keeps_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(1, 1, vec![12, 34, 56, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "lens dirt",
            LocalMask::Full,
            LocalEffect::LensDirt(LensDirtParams {
                mode: LensDirtMode::Dust,
                density: 1.0,
                opacity: 1.0,
                strength: 1.0,
                ..LensDirtParams::default()
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn lens_dirt_modes_create_different_patterns() {
        let src = solid(12, 8, [110, 120, 130, 255]);
        let make = |mode| {
            LocalAdjustmentLayer::new(
                "lens dirt",
                LocalMask::Full,
                LocalEffect::LensDirt(LensDirtParams {
                    mode,
                    density: 0.75,
                    size_px: 10.0,
                    opacity: 0.9,
                    highlight_response: 0.0,
                    seed: 9,
                    strength: 1.0,
                    ..LensDirtParams::default()
                }),
            )
        };

        let dust = apply_layers(src.as_ref(), &[make(LensDirtMode::Dust)]).unwrap();
        let smudges = apply_layers(src.as_ref(), &[make(LensDirtMode::Smudges)]).unwrap();

        assert_ne!(dust.pixels, smudges.pixels);
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
    fn heat_haze_offsets_pixels_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                10, 10, 10, 91, 40, 40, 40, 92, 80, 80, 80, 93, 120, 120, 120, 94, 160, 160, 160,
                95,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "heat haze",
            LocalMask::Full,
            LocalEffect::HeatHaze(HeatHazeParams {
                amplitude_px: 1.0,
                wavelength_px: 72.0,
                rise_px: 0.0,
                turbulence: 0.0,
                blur_px: 0.0,
                phase_degrees: 90.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels[0], src.pixels[4]);
        assert_eq!(out.pixels[3], 91);
        assert_eq!(out.pixels[19], 95);
    }

    #[test]
    fn heat_haze_ignores_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(3, 1, vec![0, 0, 0, 255, 0, 0, 0, 255, 255, 0, 0, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "heat haze",
            LocalMask::Full,
            LocalEffect::HeatHaze(HeatHazeParams {
                amplitude_px: 1.0,
                wavelength_px: 72.0,
                rise_px: 0.0,
                turbulence: 0.0,
                blur_px: 0.0,
                phase_degrees: 90.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[4..8], &[0, 0, 0, 255]);
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
    fn twirl_rotates_pixels_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            3,
            3,
            vec![
                10, 0, 0, 77, 20, 0, 0, 77, 30, 0, 0, 77, 40, 0, 0, 77, 50, 0, 0, 77, 60, 0, 0, 77,
                70, 0, 0, 77, 80, 0, 0, 77, 90, 0, 0, 77,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "twirl",
            LocalMask::Full,
            LocalEffect::Twirl(TwirlParams {
                angle_degrees: 360.0,
                radius_px: 2.0,
                center: [0.5, 0.5],
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let right_of_center = (1 * 3 + 2) * 4;
        assert_ne!(out.pixels[right_of_center], src.pixels[right_of_center]);
        assert_eq!(out.pixels[right_of_center + 3], 77);
    }

    #[test]
    fn twirl_zero_strength_is_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "twirl",
            LocalMask::Full,
            LocalEffect::Twirl(TwirlParams {
                angle_degrees: 360.0,
                radius_px: 8.0,
                center: [0.5, 0.5],
                strength: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn polar_coordinates_rect_to_polar_wraps_angle_axis() {
        let src = RgbaImageBuf::new(
            3,
            3,
            vec![
                10, 0, 0, 66, 20, 0, 0, 66, 30, 0, 0, 66, 40, 0, 0, 66, 50, 0, 0, 66, 60, 0, 0, 66,
                70, 0, 0, 66, 80, 0, 0, 66, 90, 0, 0, 66,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "polar",
            LocalMask::Full,
            LocalEffect::PolarCoordinates(PolarCoordinatesParams {
                mode: PolarCoordinatesMode::RectToPolar,
                center: [0.5, 0.5],
                radius_px: 1.0,
                angle_offset_degrees: 0.0,
                invert_radius: false,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let right_of_center = (3 + 2) * 4;
        assert_eq!(out.pixels[right_of_center], 70);
        assert_eq!(out.pixels[right_of_center + 3], 66);
    }

    #[test]
    fn polar_coordinates_polar_to_rect_unwraps_radius_axis() {
        let src = RgbaImageBuf::new(
            3,
            3,
            vec![
                10, 0, 0, 88, 20, 0, 0, 88, 30, 0, 0, 88, 40, 0, 0, 88, 50, 0, 0, 88, 60, 0, 0, 88,
                70, 0, 0, 88, 80, 0, 0, 88, 90, 0, 0, 88,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "polar",
            LocalMask::Full,
            LocalEffect::PolarCoordinates(PolarCoordinatesParams {
                mode: PolarCoordinatesMode::PolarToRect,
                center: [0.5, 0.5],
                radius_px: 1.0,
                angle_offset_degrees: 0.0,
                invert_radius: false,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let lower_left = (2 * 3) * 4;
        assert_eq!(out.pixels[lower_left], 60);
        assert_eq!(out.pixels[lower_left + 3], 88);
    }

    #[test]
    fn polar_coordinates_zero_strength_is_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "polar",
            LocalMask::Full,
            LocalEffect::PolarCoordinates(PolarCoordinatesParams {
                mode: PolarCoordinatesMode::RectToPolar,
                center: [0.5, 0.5],
                radius_px: 8.0,
                angle_offset_degrees: 90.0,
                invert_radius: true,
                strength: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn glass_displacement_ripple_offsets_pixels_and_preserves_alpha() {
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
            "glass",
            LocalMask::Full,
            LocalEffect::GlassDisplacement(GlassDisplacementParams {
                mode: GlassDisplacementMode::Ripple,
                displacement_px: 1.0,
                scale_px: 4.0,
                detail: 0.0,
                angle_degrees: 0.0,
                seed: 1,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let shifted = 4;
        assert_eq!(out.pixels[shifted], src.pixels[8]);
        assert_eq!(out.pixels[shifted + 3], 99);
    }

    #[test]
    fn glass_displacement_zero_strength_is_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "glass",
            LocalMask::Full,
            LocalEffect::GlassDisplacement(GlassDisplacementParams {
                mode: GlassDisplacementMode::Frosted,
                displacement_px: 8.0,
                scale_px: 16.0,
                detail: 1.0,
                angle_degrees: 35.0,
                seed: 42,
                strength: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn lens_correction_barrel_samples_outward_and_preserves_alpha() {
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
            "lens",
            LocalMask::Full,
            LocalEffect::LensCorrection(LensCorrectionParams {
                distortion: 1.0,
                zoom: 0.0,
                center: [0.0, 0.0],
                vignette_correction: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let mid = 2 * 4;
        assert!(out.pixels[mid] > src.pixels[mid]);
        assert_eq!(out.pixels[mid + 3], 77);
    }

    #[test]
    fn lens_correction_vignette_lifts_edges() {
        let src =
            RgbaImageBuf::new(3, 1, vec![80, 80, 80, 55, 80, 80, 80, 55, 80, 80, 80, 55]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "lens",
            LocalMask::Full,
            LocalEffect::LensCorrection(LensCorrectionParams {
                distortion: 0.0,
                zoom: 0.0,
                center: [0.5, 0.5],
                vignette_correction: 1.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > out.pixels[4]);
        assert_eq!(out.pixels[3], 55);
    }

    #[test]
    fn lens_correction_zero_strength_is_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "lens",
            LocalMask::Full,
            LocalEffect::LensCorrection(LensCorrectionParams {
                distortion: 0.75,
                zoom: 0.12,
                center: [0.5, 0.5],
                vignette_correction: 0.5,
                strength: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn line_extract_black_on_white_draws_dark_edges_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 20, 20, 77, 128, 128, 128, 77, 240, 240, 240, 77],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "lines",
            LocalMask::Full,
            LocalEffect::LineExtract(LineExtractParams {
                mode: LineExtractMode::BlackOnWhite,
                threshold: 0.05,
                softness: 0.02,
                thickness_px: 1.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let edge = 4;
        assert!(out.pixels[edge] < 64);
        assert_eq!(out.pixels[edge + 3], 77);
    }

    #[test]
    fn line_extract_darken_original_leaves_flat_area_and_darkens_edge() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                100, 100, 100, 255, 100, 100, 100, 255, 100, 100, 100, 255, 240, 240, 240, 255,
                240, 240, 240, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "lines",
            LocalMask::Full,
            LocalEffect::LineExtract(LineExtractParams {
                mode: LineExtractMode::DarkenOriginal,
                threshold: 0.05,
                softness: 0.02,
                thickness_px: 1.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels[0], src.pixels[0]);
        let edge = 2 * 4;
        assert!(out.pixels[edge] < src.pixels[edge]);
    }

    #[test]
    fn line_extract_zero_strength_is_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "lines",
            LocalMask::Full,
            LocalEffect::LineExtract(LineExtractParams {
                mode: LineExtractMode::WhiteOnBlack,
                threshold: 0.0,
                softness: 0.001,
                thickness_px: 8.0,
                strength: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn artistic_media_watercolor_smooths_color_and_preserves_alpha() {
        let src =
            RgbaImageBuf::new(3, 1, vec![40, 40, 40, 55, 220, 60, 30, 77, 40, 40, 40, 99]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "watercolor",
            LocalMask::Full,
            LocalEffect::ArtisticMedia(ArtisticMediaParams {
                mode: ArtisticMediaMode::Watercolor,
                radius_px: 1.0,
                edge_strength: 0.0,
                texture: 0.0,
                color_amount: 0.8,
                strength: 1.0,
                seed: 1,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let center = 4;
        assert!(out.pixels[center] < src.pixels[center]);
        assert_eq!(out.pixels[center + 3], 77);
    }

    #[test]
    fn artistic_media_pencil_sketch_outputs_gray_and_preserves_alpha() {
        let src = RgbaImageBuf::new(1, 1, vec![210, 40, 80, 123]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "pencil",
            LocalMask::Full,
            LocalEffect::ArtisticMedia(ArtisticMediaParams {
                mode: ArtisticMediaMode::PencilSketch,
                radius_px: 0.0,
                edge_strength: 0.0,
                texture: 0.0,
                color_amount: 0.0,
                strength: 1.0,
                seed: 1,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels[0], out.pixels[1]);
        assert_eq!(out.pixels[1], out.pixels[2]);
        assert_eq!(out.pixels[3], 123);
    }

    #[test]
    fn artistic_media_zero_strength_is_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "art",
            LocalMask::Full,
            LocalEffect::ArtisticMedia(ArtisticMediaParams {
                mode: ArtisticMediaMode::ColoredPencil,
                radius_px: 12.0,
                edge_strength: 1.0,
                texture: 1.0,
                color_amount: 1.0,
                strength: 0.0,
                seed: 7,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn brush_stroke_paint_daubs_smears_color_and_preserves_alpha() {
        let src = RgbaImageBuf::new(3, 1, vec![0, 0, 0, 55, 240, 40, 20, 77, 0, 0, 0, 99]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "paint",
            LocalMask::Full,
            LocalEffect::BrushStroke(BrushStrokeParams {
                mode: BrushStrokeMode::PaintDaubs,
                length_px: 2.0,
                radius_px: 0.0,
                angle_degrees: 0.0,
                texture: 0.0,
                edge_strength: 0.0,
                color_amount: 1.0,
                strength: 1.0,
                seed: 1,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > src.pixels[0]);
        assert!(out.pixels[4] < src.pixels[4]);
        assert_eq!(out.pixels[7], 77);
    }

    #[test]
    fn brush_stroke_palette_knife_quantizes_color() {
        let src = RgbaImageBuf::new(1, 1, vec![123, 101, 79, 201]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "knife",
            LocalMask::Full,
            LocalEffect::BrushStroke(BrushStrokeParams {
                mode: BrushStrokeMode::PaletteKnife,
                length_px: 2.0,
                radius_px: 0.0,
                angle_degrees: 0.0,
                texture: 0.0,
                edge_strength: 0.0,
                color_amount: 1.0,
                strength: 1.0,
                seed: 1,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_ne!(out.pixels[0], src.pixels[0]);
        assert_eq!(out.pixels[3], 201);
    }

    #[test]
    fn brush_stroke_zero_strength_is_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "brush",
            LocalMask::Full,
            LocalEffect::BrushStroke(BrushStrokeParams {
                mode: BrushStrokeMode::DryBrush,
                length_px: 24.0,
                radius_px: 4.0,
                angle_degrees: -35.0,
                texture: 1.0,
                edge_strength: 1.0,
                color_amount: 1.0,
                strength: 0.0,
                seed: 7,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn cutout_reduces_luminance_levels_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                20, 20, 20, 55, 80, 80, 80, 66, 140, 140, 140, 77, 200, 200, 200, 88, 250, 250,
                250, 99,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "cutout",
            LocalMask::Full,
            LocalEffect::Cutout(CutoutParams {
                levels: 3,
                radius_px: 0.0,
                edge_strength: 0.0,
                color_amount: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let mut values: Vec<u8> = out.pixels.chunks_exact(4).map(|px| px[0]).collect();
        values.sort_unstable();
        values.dedup();
        assert!(values.len() <= 3);
        assert_eq!(out.pixels[3], 55);
        assert_eq!(out.pixels[19], 99);
    }

    #[test]
    fn cutout_radius_smooths_before_quantizing() {
        let src =
            RgbaImageBuf::new(3, 1, vec![0, 0, 0, 201, 255, 255, 255, 202, 0, 0, 0, 203]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "cutout",
            LocalMask::Full,
            LocalEffect::Cutout(CutoutParams {
                levels: 3,
                radius_px: 1.0,
                edge_strength: 0.0,
                color_amount: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[4] < src.pixels[4]);
        assert!(out.pixels[4] > src.pixels[0]);
        assert_eq!(out.pixels[7], 202);
    }

    #[test]
    fn cutout_zero_strength_is_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "cutout",
            LocalMask::Full,
            LocalEffect::Cutout(CutoutParams {
                levels: 3,
                radius_px: 12.0,
                edge_strength: 1.0,
                color_amount: 1.0,
                strength: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn toon_shade_quantizes_lightness_and_preserves_hue() {
        let src = RgbaImageBuf::new(2, 1, vec![120, 60, 60, 201, 60, 120, 60, 77]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "toon",
            LocalMask::Full,
            LocalEffect::ToonShade(ToonShadeParams {
                bands: 3,
                softness: 0.0,
                preserve_hue: true,
                shadow_tint_rgb: [92, 116, 210],
                shadow_tint_strength: 0.0,
                light_tint_rgb: [255, 226, 176],
                light_tint_strength: 0.0,
                outline_strength: 0.0,
                strength: 1.0,
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        assert_ne!(out.pixels[0], src.pixels[0]);
        assert!(out.pixels[0] > out.pixels[1]);
        assert!(out.pixels[5] > out.pixels[4]);
        assert_eq!(out.pixels[3], 201);
        assert_eq!(out.pixels[7], 77);
    }

    #[test]
    fn toon_shade_can_tint_shadows_and_draw_band_edges() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![60, 60, 60, 255, 130, 130, 130, 255, 220, 220, 220, 255],
        )
        .unwrap();
        let mut params = ToonShadeParams {
            bands: 3,
            softness: 0.0,
            preserve_hue: true,
            shadow_tint_rgb: [30, 70, 255],
            shadow_tint_strength: 0.85,
            light_tint_rgb: [255, 235, 180],
            light_tint_strength: 0.35,
            outline_strength: 0.0,
            strength: 1.0,
        };
        let without_outline = apply_layers(
            src.as_ref(),
            &[LocalAdjustmentLayer::new(
                "toon",
                LocalMask::Full,
                LocalEffect::ToonShade(params),
            )],
        )
        .unwrap();
        params.outline_strength = 1.0;
        let with_outline = apply_layers(
            src.as_ref(),
            &[LocalAdjustmentLayer::new(
                "toon",
                LocalMask::Full,
                LocalEffect::ToonShade(params),
            )],
        )
        .unwrap();

        assert!(with_outline.pixels[2] > with_outline.pixels[0]);
        assert!(with_outline.pixels[4] < without_outline.pixels[4]);
    }

    #[test]
    fn emboss_lights_gradient_direction_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![0, 0, 0, 55, 128, 128, 128, 77, 255, 255, 255, 99],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "emboss",
            LocalMask::Full,
            LocalEffect::Emboss(EmbossParams {
                angle_degrees: 0.0,
                depth: 1.0,
                contrast: 0.0,
                color_amount: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[4] > src.pixels[4]);
        assert_eq!(out.pixels[7], 77);
    }

    #[test]
    fn emboss_angle_can_invert_relief() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![0, 0, 0, 55, 128, 128, 128, 77, 255, 255, 255, 99],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "emboss",
            LocalMask::Full,
            LocalEffect::Emboss(EmbossParams {
                angle_degrees: 180.0,
                depth: 1.0,
                contrast: 0.0,
                color_amount: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[4] < src.pixels[4]);
        assert_eq!(out.pixels[7], 77);
    }

    #[test]
    fn emboss_zero_strength_is_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "emboss",
            LocalMask::Full,
            LocalEffect::Emboss(EmbossParams {
                angle_degrees: 45.0,
                depth: 3.0,
                contrast: 1.0,
                color_amount: 1.0,
                strength: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn pixel_stylize_crystallize_groups_cell_color_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            4,
            1,
            vec![
                20, 30, 40, 55, 80, 90, 100, 66, 160, 120, 80, 77, 240, 210, 180, 88,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "crystallize",
            LocalMask::Full,
            LocalEffect::PixelStylize(PixelStylizeParams {
                mode: PixelStylizeMode::Crystallize,
                cell_px: 8.0,
                edge_strength: 0.0,
                color_amount: 1.0,
                randomness: 0.0,
                strength: 1.0,
                seed: 1,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let first_rgb = &out.pixels[0..3];
        for px in out.pixels.chunks_exact(4) {
            assert_eq!(&px[0..3], first_rgb);
        }
        assert_ne!(first_rgb, &src.pixels[0..3]);
        assert_eq!(out.pixels[3], 55);
        assert_eq!(out.pixels[15], 88);
    }

    #[test]
    fn pixel_stylize_mezzotint_outputs_gray_grain_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            4,
            1,
            vec![
                40, 70, 110, 51, 120, 120, 120, 62, 200, 170, 130, 73, 250, 250, 250, 84,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "mezzotint",
            LocalMask::Full,
            LocalEffect::PixelStylize(PixelStylizeParams {
                mode: PixelStylizeMode::Mezzotint,
                cell_px: 1.0,
                edge_strength: 0.0,
                color_amount: 0.0,
                randomness: 1.0,
                strength: 1.0,
                seed: 3,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_ne!(out.pixels, src.pixels);
        for px in out.pixels.chunks_exact(4) {
            assert_eq!(px[0], px[1]);
            assert_eq!(px[1], px[2]);
        }
        assert_eq!(out.pixels[3], 51);
        assert_eq!(out.pixels[15], 84);
    }

    #[test]
    fn pixel_stylize_zero_strength_is_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "pixel stylize",
            LocalMask::Full,
            LocalEffect::PixelStylize(PixelStylizeParams {
                mode: PixelStylizeMode::Pointillize,
                cell_px: 24.0,
                edge_strength: 1.0,
                color_amount: 1.0,
                randomness: 1.0,
                strength: 0.0,
                seed: 9,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
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
    fn frequency_separation_reduces_high_frequency_detail() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                80, 80, 80, 255, 80, 80, 80, 255, 190, 190, 190, 255, 80, 80, 80, 255, 80, 80, 80,
                255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "frequency separation",
            LocalMask::Full,
            LocalEffect::FrequencySeparation(FrequencySeparationParams {
                radius_px: 1.0,
                low_smoothing: 0.0,
                detail_amount: 0.0,
                detail_contrast: 1.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[8] < src.pixels[8]);
        assert_eq!(out.pixels[11], 255);
    }

    #[test]
    fn frequency_separation_can_enhance_detail() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                80, 80, 80, 255, 80, 80, 80, 255, 150, 150, 150, 255, 80, 80, 80, 255, 80, 80, 80,
                255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "frequency separation",
            LocalMask::Full,
            LocalEffect::FrequencySeparation(FrequencySeparationParams {
                radius_px: 1.0,
                low_smoothing: 0.0,
                detail_amount: 1.45,
                detail_contrast: 1.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[8] > src.pixels[8]);
        assert_eq!(out.pixels[11], 255);
    }

    #[test]
    fn frequency_separation_keeps_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![255, 0, 0, 0, 100, 100, 100, 255, 100, 100, 100, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "frequency separation",
            LocalMask::Full,
            LocalEffect::FrequencySeparation(FrequencySeparationParams {
                radius_px: 1.0,
                low_smoothing: 1.0,
                detail_amount: 0.0,
                detail_contrast: 1.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[255, 0, 0, 0]);
        assert!(out.pixels[4] <= 110);
        assert_eq!(out.pixels[7], 255);
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
    fn apply_smart_sharpen_rgba_zero_amount_is_identity() {
        let src = RgbaImageBuf::new(
            4,
            1,
            vec![
                40, 40, 40, 255, 40, 40, 40, 255, 180, 180, 180, 255, 180, 180, 180, 255,
            ],
        )
        .unwrap();
        let out = apply_smart_sharpen_rgba(
            &src.pixels,
            4,
            1,
            &SmartSharpenParams {
                amount: 0.0,
                radius_px: 1.2,
                edge_threshold: 0.08,
                halo_suppression: 0.6,
            },
        );
        assert_eq!(out, src.pixels);
    }

    #[test]
    fn apply_smart_sharpen_rgba_emphasizes_edges_and_keeps_alpha() {
        // 強エッジを跨ぐ 1 行画像 + アルファ境界 (右端は半透明)。
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                40, 40, 40, 255, 40, 40, 40, 255, 180, 180, 180, 255, 180, 180, 180, 255, 180, 180,
                180, 96,
            ],
        )
        .unwrap();
        let out = apply_smart_sharpen_rgba(
            &src.pixels,
            5,
            1,
            &SmartSharpenParams {
                amount: 1.25,
                radius_px: 1.6,
                edge_threshold: 0.05,
                halo_suppression: 0.2,
            },
        );
        // 暗側はより暗く / 明側はより明るく (アンシャープ系の基本挙動)。
        assert!(out[4] < src.pixels[4]);
        assert!(out[8] > src.pixels[8]);
        // alpha は全画素で不変 (RGB のみ強調)。
        for (i, px) in out.chunks_exact(4).enumerate() {
            assert_eq!(px[3], src.pixels[i * 4 + 3], "alpha changed at {i}");
        }
    }

    #[test]
    fn apply_smart_sharpen_rgba_clamps_huge_radius() {
        // radius_px は 3.0 に clamp される (巨大半径で UI を止めないための番人)。
        // clamp 後の radius=3 でもエッジ画素は強調される (= 結果が壊れていない) ことを確認。
        let mut pixels = Vec::new();
        for x in 0..16 {
            let v = if x < 8 { 40 } else { 200 };
            pixels.extend_from_slice(&[v, v, v, 255]);
        }
        let src = RgbaImageBuf::new(16, 1, pixels).unwrap();
        let huge = apply_smart_sharpen_rgba(
            &src.pixels,
            16,
            1,
            &SmartSharpenParams {
                amount: 1.0,
                radius_px: 100.0,
                edge_threshold: 0.05,
                halo_suppression: 0.2,
            },
        );
        let clamped = apply_smart_sharpen_rgba(
            &src.pixels,
            16,
            1,
            &SmartSharpenParams {
                amount: 1.0,
                radius_px: 3.0,
                edge_threshold: 0.05,
                halo_suppression: 0.2,
            },
        );
        assert_eq!(huge, clamped);
        assert!(huge[7 * 4] < src.pixels[7 * 4]);
        assert!(huge[8 * 4] > src.pixels[8 * 4]);
    }

    #[test]
    fn box_blur_rgba_parallel_matches_serial_reference() {
        // 行並列化が逐次版と bit 一致することを、独立実装の逐次リファレンスで確認する。
        let width = 13;
        let height = 7;
        let radius = 2;
        let mut pixels = Vec::with_capacity(width * height * 4);
        for i in 0..(width * height) {
            // 決定論的な疑似パターン (Date 非依存)
            let v = ((i * 37 + 11) % 251) as u8;
            pixels.extend_from_slice(&[v, v.wrapping_add(40), v.wrapping_add(90), 255]);
        }
        let out = box_blur_rgba(&pixels, width, height, radius);

        let mut tmp = vec![0_u8; pixels.len()];
        for y in 0..height {
            for x in 0..width {
                let x0 = x.saturating_sub(radius);
                let x1 = (x + radius).min(width - 1);
                let count = (x1 - x0 + 1) as u32;
                let mut sum = [0_u32; 4];
                for xx in x0..=x1 {
                    let i = (y * width + xx) * 4;
                    for c in 0..4 {
                        sum[c] += pixels[i + c] as u32;
                    }
                }
                let o = (y * width + x) * 4;
                for c in 0..4 {
                    tmp[o + c] = (sum[c] / count) as u8;
                }
            }
        }
        let mut expected = vec![0_u8; pixels.len()];
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
                    expected[o + c] = (sum[c] / count) as u8;
                }
            }
        }
        assert_eq!(out, expected);
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
            LocalEffect::PhotoFilter(PhotoFilterParams::default()),
            LocalEffect::ThreeWayColorGrading(ThreeWayColorGradingParams::default()),
            LocalEffect::SelectiveColor(SelectiveColorParams::default()),
            LocalEffect::PartColor(PartColorParams::default()),
            LocalEffect::ChannelMixer(ChannelMixerParams::default()),
            LocalEffect::MonochromeMixer(MonochromeMixerParams::default()),
            LocalEffect::Clarity(ClarityParams::default()),
            LocalEffect::Texture(TextureParams::default()),
            LocalEffect::HighPass(HighPassParams::default()),
            LocalEffect::FrequencySeparation(FrequencySeparationParams::default()),
            LocalEffect::HighlightsShadows(HighlightsShadowsParams::default()),
            LocalEffect::Dehaze(DehazeParams::default()),
            LocalEffect::Blur(BlurParams::default()),
            LocalEffect::MotionBlur(MotionBlurParams::default()),
            LocalEffect::Wind(WindParams::default()),
            LocalEffect::TiltShift(TiltShiftParams::default()),
            LocalEffect::LensBlur(LensBlurParams::default()),
            LocalEffect::BokehSprite(BokehSpriteParams::default()),
            LocalEffect::LensDirt(LensDirtParams::default()),
            LocalEffect::RadialBlur(RadialBlurParams::default()),
            LocalEffect::WaveDistortion(WaveDistortionParams::default()),
            LocalEffect::HeatHaze(HeatHazeParams::default()),
            LocalEffect::PinchSpherize(PinchSpherizeParams::default()),
            LocalEffect::Twirl(TwirlParams::default()),
            LocalEffect::PolarCoordinates(PolarCoordinatesParams::default()),
            LocalEffect::GlassDisplacement(GlassDisplacementParams::default()),
            LocalEffect::LensCorrection(LensCorrectionParams::default()),
            LocalEffect::LineExtract(LineExtractParams::default()),
            LocalEffect::ArtisticMedia(ArtisticMediaParams::default()),
            LocalEffect::BrushStroke(BrushStrokeParams::default()),
            LocalEffect::Cutout(CutoutParams::default()),
            LocalEffect::ToonShade(ToonShadeParams::default()),
            LocalEffect::Emboss(EmbossParams::default()),
            LocalEffect::PixelStylize(PixelStylizeParams::default()),
            LocalEffect::Solarize(SolarizeParams::default()),
            LocalEffect::GlowingEdges(GlowingEdgesParams::default()),
            LocalEffect::OilPaint(OilPaintParams::default()),
            LocalEffect::SoftFocus(SoftFocusParams::default()),
            LocalEffect::Orton(OrtonParams::default()),
            LocalEffect::Mosaic(MosaicParams::default()),
            LocalEffect::Sharpen(SharpenParams::default()),
            LocalEffect::SmartSharpen(SmartSharpenParams::default()),
            LocalEffect::Hsl(HslParams::default()),
            LocalEffect::ColorMixer(ColorMixerParams::default()),
            LocalEffect::Look(LookParams::default()),
            LocalEffect::CubeLut(CubeLutParams::default()),
            LocalEffect::Posterize(PosterizeParams::default()),
            LocalEffect::RetroPalette(RetroPaletteParams::default()),
            LocalEffect::CrtDisplay(CrtDisplayParams::default()),
            LocalEffect::Threshold(ThresholdParams::default()),
            LocalEffect::Invert(InvertParams::default()),
            LocalEffect::Duotone(DuotoneParams::default()),
            LocalEffect::Equalize(EqualizeParams::default()),
            LocalEffect::GradientMap(GradientMapParams::default()),
            LocalEffect::ColorFill(ColorFillParams::default()),
            LocalEffect::Frame(FrameParams::default()),
            LocalEffect::OutlineStroke(OutlineStrokeParams::default()),
            LocalEffect::RimLight(RimLightParams::default()),
            LocalEffect::ContactShadow(ContactShadowParams::default()),
            LocalEffect::ColorTrace(ColorTraceParams::default()),
            LocalEffect::ColorOverlay(ColorOverlayParams::default()),
            LocalEffect::NeonGlow(NeonGlowParams::default()),
            LocalEffect::DiffuseGlow(DiffuseGlowParams::default()),
            LocalEffect::Bloom(BloomParams::default()),
            LocalEffect::Halation(HalationParams::default()),
            LocalEffect::ColorDodgeGlow(ColorDodgeGlowParams::default()),
            LocalEffect::GodRays(GodRaysParams::default()),
            LocalEffect::LensFlare(LensFlareParams::default()),
            LocalEffect::AnamorphicFlare(AnamorphicFlareParams::default()),
            LocalEffect::LightLeak(LightLeakParams::default()),
            LocalEffect::BacklightHaze(BacklightHazeParams::default()),
            LocalEffect::SpeedLines(SpeedLinesParams::default()),
            LocalEffect::RadialFlash(RadialFlashParams::default()),
            LocalEffect::CloudFog(CloudFogParams::default()),
            LocalEffect::Spotlight(SpotlightParams::default()),
            LocalEffect::Vignette(VignetteParams::default()),
            LocalEffect::FilmGrain(FilmGrainParams::default()),
            LocalEffect::Noise(NoiseParams::default()),
            LocalEffect::ChromaticAberration(ChromaticAberrationParams::default()),
            LocalEffect::Anaglyph3d(AnaglyphParams::default()),
            LocalEffect::Defringe(DefringeParams::default()),
            LocalEffect::ScanlineGlitch(ScanlineGlitchParams::default()),
            LocalEffect::Vhs(VhsParams::default()),
            LocalEffect::DataMosh(DataMoshParams::default()),
            LocalEffect::PixelSort(PixelSortParams::default()),
            LocalEffect::OldFilm(OldFilmParams::default()),
            LocalEffect::WaterCaustics(WaterCausticsParams::default()),
            LocalEffect::ParticleOverlay(ParticleOverlayParams::default()),
            LocalEffect::Aurora(AuroraParams::default()),
            LocalEffect::Halftone(HalftoneParams::default()),
            LocalEffect::ScreenTone(ScreenToneParams::default()),
            LocalEffect::ColorHalftone(ColorHalftoneParams::default()),
            LocalEffect::CmykPlateShift(CmykPlateShiftParams::default()),
            LocalEffect::Lithograph(LithographParams::default()),
            LocalEffect::Engraving(EngravingParams::default()),
            LocalEffect::NewspaperPrint(NewspaperPrintParams::default()),
            LocalEffect::Textureizer(TextureizerParams::default()),
            LocalEffect::StarGlow(StarGlowParams::default()),
            LocalEffect::DiffractionStarburst(DiffractionStarburstParams::default()),
            LocalEffect::EdgeSmooth(EdgeSmoothParams::default()),
            LocalEffect::Despeckle(DespeckleParams::default()),
            LocalEffect::Median(MedianParams::default()),
        ];
        for effect in effects {
            let layer = LocalAdjustmentLayer::new("identity", LocalMask::Full, effect);
            let out = apply_layers(src.as_ref(), &[layer]).unwrap();
            assert_eq!(out.pixels, src.pixels);
        }
    }

    #[test]
    fn risky_max_parameter_effects_finish_quickly() {
        struct RiskyEffectCase {
            name: &'static str,
            mask: LocalMask,
            effect: LocalEffect,
        }

        let width = 48;
        let height = 40;
        let src = patterned(width, height);
        let full = |name, effect| RiskyEffectCase {
            name,
            mask: LocalMask::Full,
            effect,
        };
        let masked = |name, effect| RiskyEffectCase {
            name,
            mask: center_rect_mask(width, height),
            effect,
        };
        let cases = vec![
            full(
                "clarity max radius",
                LocalEffect::Clarity(ClarityParams {
                    amount: 1.0,
                    radius_px: 96.0,
                }),
            ),
            full(
                "texture max radius",
                LocalEffect::Texture(TextureParams {
                    amount: 1.0,
                    radius_px: 96.0,
                }),
            ),
            masked(
                "surrounding repair high quality",
                LocalEffect::Repair(RepairParams {
                    mode: RepairMode::Surrounding,
                    search_radius_px: 512.0,
                    texture_strength: 1.0,
                    color_match_strength: 1.0,
                    quality: RepairQuality::High,
                    seed: 31,
                    ..Default::default()
                }),
            ),
            full(
                "high pass max radius",
                LocalEffect::HighPass(HighPassParams {
                    amount: 2.0,
                    radius_px: 96.0,
                    contrast: 4.0,
                    detail_only: false,
                }),
            ),
            full(
                "frequency separation max radii",
                LocalEffect::FrequencySeparation(FrequencySeparationParams {
                    radius_px: 128.0,
                    low_smoothing: 1.0,
                    detail_amount: 2.0,
                    detail_contrast: 2.0,
                    strength: 1.0,
                }),
            ),
            full(
                "frame rounded matte max",
                LocalEffect::Frame(FrameParams {
                    mode: FrameMode::RoundedMatte,
                    color_rgb: [0, 0, 0],
                    opacity: 1.0,
                    width_px: 96.0,
                    softness_px: 48.0,
                    line_width_px: 8.0,
                    line_opacity: 1.0,
                    corner_radius_px: 160.0,
                    ..Default::default()
                }),
            ),
            full(
                "retro palette famicom dither",
                LocalEffect::RetroPalette(RetroPaletteParams {
                    mode: RetroPaletteMode::Famicom,
                    dither: 1.0,
                    strength: 1.0,
                }),
            ),
            full(
                "retro palette sfc adaptive",
                LocalEffect::RetroPalette(RetroPaletteParams {
                    mode: RetroPaletteMode::Sfc,
                    dither: 1.0,
                    strength: 1.0,
                }),
            ),
            full(
                "crt display max phosphor",
                LocalEffect::CrtDisplay(CrtDisplayParams {
                    mode: CrtDisplayMode::Full,
                    scanline_spacing_px: 2.0,
                    scanline_depth: 1.0,
                    mask_strength: 1.0,
                    curvature: 0.25,
                    bloom: 1.0,
                    horizontal_blur: 1.0,
                    brightness: 2.5,
                    strength: 1.0,
                }),
            ),
            full(
                "dehaze max radius",
                LocalEffect::Dehaze(DehazeParams {
                    amount: 1.0,
                    radius_px: 48.0,
                    min_transmission: 0.01,
                    saturation: 100.0,
                }),
            ),
            full(
                "blur large radius",
                LocalEffect::Blur(BlurParams { radius_px: 240.0 }),
            ),
            full(
                "motion blur max distance",
                LocalEffect::MotionBlur(MotionBlurParams {
                    distance_px: 240.0,
                    angle_degrees: 35.0,
                    strength: 1.0,
                }),
            ),
            full(
                "wind max distance",
                LocalEffect::Wind(WindParams {
                    direction: WindDirection::Right,
                    source: WindSource::Edge,
                    distance_px: 240.0,
                    threshold: 0.0,
                    softness: 0.001,
                    turbulence: 1.0,
                    strength: 1.0,
                    seed: 7,
                }),
            ),
            full(
                "tilt shift max blur",
                LocalEffect::TiltShift(TiltShiftParams {
                    mode: TiltShiftMode::Radial,
                    range_initialized: true,
                    radius: [0.24, 0.32],
                    max_radius_px: 160.0,
                    strength: 1.0,
                    ..Default::default()
                }),
            ),
            full(
                "lens blur max radius",
                LocalEffect::LensBlur(LensBlurParams {
                    radius_px: 96.0,
                    aperture: LensBlurAperture::Octagon,
                    rotation_degrees: 15.0,
                    highlight_threshold: 0.0,
                    highlight_boost: 3.0,
                    strength: 1.0,
                }),
            ),
            full(
                "bokeh sprite dense hearts",
                LocalEffect::BokehSprite(BokehSpriteParams {
                    shape: BokehSpriteShape::Heart,
                    threshold: 0.0,
                    density: 1.0,
                    size_px: 96.0,
                    softness: 1.0,
                    brightness: 2.0,
                    color_strength: 1.0,
                    seed: 31,
                    strength: 1.0,
                }),
            ),
            full(
                "lens dirt dense water drops",
                LocalEffect::LensDirt(LensDirtParams {
                    mode: LensDirtMode::WaterDrops,
                    density: 1.0,
                    size_px: 128.0,
                    opacity: 1.0,
                    softness: 1.0,
                    highlight_response: 1.0,
                    distortion_px: 32.0,
                    seed: 32,
                    strength: 1.0,
                }),
            ),
            full(
                "radial zoom blur max samples",
                LocalEffect::RadialBlur(RadialBlurParams {
                    mode: RadialBlurMode::Zoom,
                    center: [0.35, 0.42],
                    zoom_px: 240.0,
                    spin_degrees: 0.0,
                    samples: 65,
                    strength: 1.0,
                }),
            ),
            full(
                "radial spin blur max samples",
                LocalEffect::RadialBlur(RadialBlurParams {
                    mode: RadialBlurMode::Spin,
                    center: [0.52, 0.48],
                    zoom_px: 0.0,
                    spin_degrees: 180.0,
                    samples: 65,
                    strength: 1.0,
                }),
            ),
            full(
                "ripple wave max amplitude",
                LocalEffect::WaveDistortion(WaveDistortionParams {
                    mode: WaveDistortionMode::Ripple,
                    amplitude_px: 240.0,
                    wavelength_px: 2.0,
                    phase_degrees: 270.0,
                    center: [0.5, 0.5],
                    strength: 1.0,
                }),
            ),
            full(
                "zigzag wave max amplitude",
                LocalEffect::WaveDistortion(WaveDistortionParams {
                    mode: WaveDistortionMode::Zigzag,
                    amplitude_px: 240.0,
                    wavelength_px: 2.0,
                    phase_degrees: 180.0,
                    center: [0.5, 0.5],
                    strength: 1.0,
                }),
            ),
            full(
                "heat haze max shimmer",
                LocalEffect::HeatHaze(HeatHazeParams {
                    amplitude_px: 160.0,
                    wavelength_px: 4.0,
                    rise_px: 160.0,
                    turbulence: 1.0,
                    blur_px: 12.0,
                    phase_degrees: 180.0,
                    strength: 1.0,
                }),
            ),
            full(
                "spherize full radius",
                LocalEffect::PinchSpherize(PinchSpherizeParams {
                    amount: 1.0,
                    radius_px: 0.0,
                    center: [0.5, 0.5],
                    strength: 1.0,
                }),
            ),
            full(
                "pinch full radius",
                LocalEffect::PinchSpherize(PinchSpherizeParams {
                    amount: -1.0,
                    radius_px: 0.0,
                    center: [0.5, 0.5],
                    strength: 1.0,
                }),
            ),
            full(
                "twirl max angle",
                LocalEffect::Twirl(TwirlParams {
                    angle_degrees: 1080.0,
                    radius_px: 0.0,
                    center: [0.5, 0.5],
                    strength: 1.0,
                }),
            ),
            full(
                "polar rect to polar full radius",
                LocalEffect::PolarCoordinates(PolarCoordinatesParams {
                    mode: PolarCoordinatesMode::RectToPolar,
                    center: [0.5, 0.5],
                    radius_px: 0.0,
                    angle_offset_degrees: 360.0,
                    invert_radius: true,
                    strength: 1.0,
                }),
            ),
            full(
                "polar to rect full radius",
                LocalEffect::PolarCoordinates(PolarCoordinatesParams {
                    mode: PolarCoordinatesMode::PolarToRect,
                    center: [0.5, 0.5],
                    radius_px: 0.0,
                    angle_offset_degrees: -360.0,
                    invert_radius: true,
                    strength: 1.0,
                }),
            ),
            full(
                "glass frosted max displacement",
                LocalEffect::GlassDisplacement(GlassDisplacementParams {
                    mode: GlassDisplacementMode::Frosted,
                    displacement_px: 240.0,
                    scale_px: 2.0,
                    detail: 1.0,
                    angle_degrees: 45.0,
                    seed: 5,
                    strength: 1.0,
                }),
            ),
            full(
                "glass ripple max displacement",
                LocalEffect::GlassDisplacement(GlassDisplacementParams {
                    mode: GlassDisplacementMode::Ripple,
                    displacement_px: 240.0,
                    scale_px: 2.0,
                    detail: 1.0,
                    angle_degrees: 45.0,
                    seed: 6,
                    strength: 1.0,
                }),
            ),
            full(
                "glass faceted max displacement",
                LocalEffect::GlassDisplacement(GlassDisplacementParams {
                    mode: GlassDisplacementMode::Faceted,
                    displacement_px: 240.0,
                    scale_px: 2.0,
                    detail: 1.0,
                    angle_degrees: 45.0,
                    seed: 7,
                    strength: 1.0,
                }),
            ),
            full(
                "lens correction max warp",
                LocalEffect::LensCorrection(LensCorrectionParams {
                    distortion: 1.0,
                    zoom: 1.0,
                    center: [0.45, 0.55],
                    vignette_correction: 1.0,
                    strength: 1.0,
                }),
            ),
            full(
                "line extract max thickness",
                LocalEffect::LineExtract(LineExtractParams {
                    mode: LineExtractMode::LightenOriginal,
                    threshold: 0.0,
                    softness: 0.001,
                    thickness_px: 8.0,
                    strength: 1.0,
                }),
            ),
            full(
                "artistic watercolor max radius",
                LocalEffect::ArtisticMedia(ArtisticMediaParams {
                    mode: ArtisticMediaMode::Watercolor,
                    radius_px: 48.0,
                    edge_strength: 1.0,
                    texture: 1.0,
                    color_amount: 1.0,
                    strength: 1.0,
                    seed: 1,
                }),
            ),
            full(
                "artistic pencil max texture",
                LocalEffect::ArtisticMedia(ArtisticMediaParams {
                    mode: ArtisticMediaMode::PencilSketch,
                    radius_px: 48.0,
                    edge_strength: 1.0,
                    texture: 1.0,
                    color_amount: 1.0,
                    strength: 1.0,
                    seed: 2,
                }),
            ),
            full(
                "brush dry max length",
                LocalEffect::BrushStroke(BrushStrokeParams {
                    mode: BrushStrokeMode::DryBrush,
                    length_px: 96.0,
                    radius_px: 16.0,
                    angle_degrees: -35.0,
                    texture: 1.0,
                    edge_strength: 1.0,
                    color_amount: 1.0,
                    strength: 1.0,
                    seed: 3,
                }),
            ),
            full(
                "brush daubs max radius",
                LocalEffect::BrushStroke(BrushStrokeParams {
                    mode: BrushStrokeMode::PaintDaubs,
                    length_px: 96.0,
                    radius_px: 16.0,
                    angle_degrees: 20.0,
                    texture: 1.0,
                    edge_strength: 1.0,
                    color_amount: 1.0,
                    strength: 1.0,
                    seed: 4,
                }),
            ),
            full(
                "brush palette knife max radius",
                LocalEffect::BrushStroke(BrushStrokeParams {
                    mode: BrushStrokeMode::PaletteKnife,
                    length_px: 96.0,
                    radius_px: 16.0,
                    angle_degrees: 10.0,
                    texture: 1.0,
                    edge_strength: 1.0,
                    color_amount: 1.0,
                    strength: 1.0,
                    seed: 5,
                }),
            ),
            full(
                "cutout max radius",
                LocalEffect::Cutout(CutoutParams {
                    levels: 12,
                    radius_px: 32.0,
                    edge_strength: 1.0,
                    color_amount: 1.0,
                    strength: 1.0,
                }),
            ),
            full(
                "pixel crystallize dense cells",
                LocalEffect::PixelStylize(PixelStylizeParams {
                    mode: PixelStylizeMode::Crystallize,
                    cell_px: 1.0,
                    edge_strength: 1.0,
                    color_amount: 1.0,
                    randomness: 1.0,
                    strength: 1.0,
                    seed: 8,
                }),
            ),
            full(
                "pixel pointillize dense cells",
                LocalEffect::PixelStylize(PixelStylizeParams {
                    mode: PixelStylizeMode::Pointillize,
                    cell_px: 1.0,
                    edge_strength: 1.0,
                    color_amount: 1.0,
                    randomness: 1.0,
                    strength: 1.0,
                    seed: 9,
                }),
            ),
            full(
                "pixel facet dense cells",
                LocalEffect::PixelStylize(PixelStylizeParams {
                    mode: PixelStylizeMode::Facet,
                    cell_px: 1.0,
                    edge_strength: 1.0,
                    color_amount: 1.0,
                    randomness: 1.0,
                    strength: 1.0,
                    seed: 10,
                }),
            ),
            full(
                "glowing edges max glow",
                LocalEffect::GlowingEdges(GlowingEdgesParams {
                    threshold: 0.0,
                    softness: 1.0,
                    edge_width_px: 12.0,
                    glow_radius_px: 120.0,
                    edge_brightness: 3.0,
                    glow_strength: 3.0,
                    hue_degrees: 280.0,
                    color_amount: 1.0,
                    background_amount: 1.0,
                    strength: 1.0,
                }),
            ),
            full(
                "oil paint max radius",
                LocalEffect::OilPaint(OilPaintParams {
                    radius_px: 12.0,
                    saturation: 1.0,
                    contrast: 1.0,
                    strength: 1.0,
                }),
            ),
            full(
                "soft focus large radius",
                LocalEffect::SoftFocus(SoftFocusParams {
                    radius_px: 240.0,
                    strength: 1.0,
                }),
            ),
            full(
                "orton max radius",
                LocalEffect::Orton(OrtonParams {
                    radius_px: 160.0,
                    strength: 1.0,
                    brightness: 1.0,
                    contrast: 1.0,
                    saturation: 1.0,
                }),
            ),
            masked(
                "mosaic dense mask-shape tiles",
                LocalEffect::Mosaic(MosaicParams {
                    tile_mode: MosaicTileMode::FixedPx(4),
                    boundary: MosaicBoundary::MaskShape,
                    block_px: 0,
                }),
            ),
            full(
                "sharpen max radius",
                LocalEffect::Sharpen(SharpenParams {
                    amount: 2.0,
                    radius_px: 96.0,
                    threshold: 0.0,
                }),
            ),
            full(
                "smart sharpen max radius",
                LocalEffect::SmartSharpen(SmartSharpenParams {
                    amount: 2.0,
                    radius_px: 96.0,
                    edge_threshold: 0.0,
                    halo_suppression: 1.0,
                }),
            ),
            masked(
                "outline stroke max center width",
                LocalEffect::OutlineStroke(OutlineStrokeParams {
                    placement: OutlineStrokePlacement::Center,
                    width_px: 96.0,
                    softness_px: 32.0,
                    opacity: 1.0,
                    color_rgb: [0, 0, 0],
                }),
            ),
            masked(
                "rim light max width",
                LocalEffect::RimLight(RimLightParams {
                    light_angle_degrees: -35.0,
                    width_px: 96.0,
                    falloff: 1.0,
                    strength: 2.0,
                    color_rgb: [220, 240, 255],
                    wrap: 1.0,
                }),
            ),
            masked(
                "contact shadow max radius",
                LocalEffect::ContactShadow(ContactShadowParams {
                    radius_px: 96.0,
                    softness_px: 32.0,
                    strength: 1.0,
                    color_rgb: [0, 0, 0],
                    direction_degrees: 90.0,
                    directionality: 1.0,
                }),
            ),
            full(
                "color trace max sample radius",
                LocalEffect::ColorTrace(ColorTraceParams {
                    strength: 1.0,
                    line_threshold: 0.0,
                    softness: 0.001,
                    sample_radius_px: 64.0,
                    darkness: 1.0,
                    saturation: 2.0,
                }),
            ),
            full(
                "neon glow max radii",
                LocalEffect::NeonGlow(NeonGlowParams {
                    threshold: 0.05,
                    by_saturation: true,
                    inner_radius_px: 96.0,
                    outer_radius_px: 180.0,
                    strength: 2.0,
                    inner_amount: 2.0,
                    outer_amount: 2.0,
                    glow_saturation: 2.0,
                    tint_rgb: [0, 220, 255],
                    tint_strength: 1.0,
                    screen_blend: true,
                    source_color_enabled: true,
                    source_rgb: [0, 220, 255],
                    source_tolerance: 1.0,
                    source_feather: 1.0,
                }),
            ),
            full(
                "diffuse glow max radius",
                LocalEffect::DiffuseGlow(DiffuseGlowParams {
                    threshold: 0.0,
                    radius_px: 120.0,
                    strength: 2.0,
                    white_mix: 1.0,
                    grain: 1.0,
                    seed: 11,
                }),
            ),
            full(
                "bloom max radius",
                LocalEffect::Bloom(BloomParams {
                    threshold: 0.90,
                    radius_px: 120.0,
                    strength: 2.0,
                }),
            ),
            full(
                "halation max radius",
                LocalEffect::Halation(HalationParams {
                    threshold: 0.05,
                    radius_px: 180.0,
                    strength: 2.0,
                    warmth: 1.0,
                    tint_rgb: [255, 232, 196],
                    edge_bias: 1.0,
                    screen_blend: true,
                }),
            ),
            full(
                "color dodge glow max radius",
                LocalEffect::ColorDodgeGlow(ColorDodgeGlowParams {
                    threshold: 0.0,
                    radius_px: 180.0,
                    strength: 2.0,
                    dodge_amount: 1.0,
                    color_rgb: [255, 220, 128],
                    color_strength: 1.0,
                }),
            ),
            full(
                "god rays max length",
                LocalEffect::GodRays(GodRaysParams {
                    center: [0.15, 0.12],
                    threshold: 0.0,
                    length_px: 360.0,
                    decay: 1.0,
                    strength: 3.0,
                    warm_tint: 1.0,
                }),
            ),
            full(
                "lens flare max radius",
                LocalEffect::LensFlare(LensFlareParams {
                    center: [0.15, 0.18],
                    radius_px: 420.0,
                    strength: 3.0,
                    core_strength: 2.0,
                    halo_strength: 2.0,
                    ghost_strength: 2.0,
                    streak_strength: 2.0,
                    warm_tint: 1.0,
                }),
            ),
            full(
                "anamorphic flare max length",
                LocalEffect::AnamorphicFlare(AnamorphicFlareParams {
                    threshold: 0.0,
                    length_px: 480.0,
                    thickness_px: 48.0,
                    strength: 3.0,
                    color_rgb: [80, 150, 255],
                    color_strength: 1.0,
                }),
            ),
            full(
                "light leak max haze streaks",
                LocalEffect::LightLeak(LightLeakParams {
                    center: [-0.25, 0.20],
                    color_rgb: [255, 120, 60],
                    radius: 1.6,
                    intensity: 2.0,
                    falloff: 0.35,
                    haze: 1.0,
                    streak_strength: 1.0,
                    streak_angle_degrees: -180.0,
                    strength: 1.0,
                    seed: 28,
                }),
            ),
            full(
                "backlight haze max glow",
                LocalEffect::BacklightHaze(BacklightHazeParams {
                    center: [0.5, 0.0],
                    color_rgb: [255, 230, 180],
                    radius: 1.6,
                    falloff: 0.35,
                    haze: 1.0,
                    glow: 2.0,
                    shadow_lift: 1.0,
                    contrast_fade: 1.0,
                    saturation_fade: 1.0,
                    strength: 1.0,
                }),
            ),
            full(
                "speed lines max radial lines",
                LocalEffect::SpeedLines(SpeedLinesParams {
                    mode: SpeedLinesMode::Radial,
                    center: [0.5, 0.5],
                    angle_degrees: 0.0,
                    line_count: 360,
                    line_width_px: 32.0,
                    length: 1.0,
                    inner_radius: 0.0,
                    outer_radius: 1.0,
                    softness: 1.0,
                    strength: 1.0,
                    color_rgb: [255, 255, 255],
                    seed: 12,
                }),
            ),
            full(
                "speed lines max parallel lines",
                LocalEffect::SpeedLines(SpeedLinesParams {
                    mode: SpeedLinesMode::Parallel,
                    center: [0.5, 0.5],
                    angle_degrees: -25.0,
                    line_count: 360,
                    line_width_px: 32.0,
                    length: 1.0,
                    inner_radius: 0.0,
                    outer_radius: 1.0,
                    softness: 1.0,
                    strength: 1.0,
                    color_rgb: [255, 255, 255],
                    seed: 13,
                }),
            ),
            full(
                "radial flash dense rays",
                LocalEffect::RadialFlash(RadialFlashParams {
                    center: [0.5, 0.5],
                    ray_count: 240,
                    rotation_degrees: 180.0,
                    inner_radius: 0.0,
                    outer_radius: 1.0,
                    softness: 1.0,
                    white_amount: 1.0,
                    black_amount: 1.0,
                    invert: true,
                    strength: 1.0,
                }),
            ),
            full(
                "cloud fog max density",
                LocalEffect::CloudFog(CloudFogParams {
                    mode: CloudFogMode::Clouds,
                    scale_px: 2.0,
                    detail: 1.0,
                    density: 1.0,
                    contrast: 1.0,
                    height_fade: 1.0,
                    opacity: 1.0,
                    color_rgb: [235, 242, 255],
                    seed: 14,
                }),
            ),
            full(
                "film grain max amount",
                LocalEffect::FilmGrain(FilmGrainParams {
                    amount: 1.0,
                    size_px: 32,
                    seed: 15,
                }),
            ),
            full(
                "gaussian noise max amount",
                LocalEffect::Noise(NoiseParams {
                    amount: 1.0,
                    distribution: NoiseDistribution::Gaussian,
                    monochrome: false,
                    seed: 16,
                }),
            ),
            full(
                "chromatic aberration max offset",
                LocalEffect::ChromaticAberration(ChromaticAberrationParams { offset_px: 24.0 }),
            ),
            full(
                "anaglyph max disparity",
                LocalEffect::Anaglyph3d(AnaglyphParams {
                    mode: AnaglyphMode::RedCyan,
                    disparity_px: 96.0,
                    angle_degrees: 180.0,
                    luma_mix: 1.0,
                    strength: 1.0,
                }),
            ),
            full(
                "defringe max radius",
                LocalEffect::Defringe(DefringeParams {
                    radius_px: 8.0,
                    edge_threshold: 0.0,
                    color_threshold: 0.0,
                    neutralize: 1.0,
                    strength: 1.0,
                }),
            ),
            full(
                "scanline glitch max jitter",
                LocalEffect::ScanlineGlitch(ScanlineGlitchParams {
                    line_spacing_px: 2.0,
                    line_strength: 1.0,
                    jitter_px: 48.0,
                    rgb_shift_px: 24.0,
                    block_strength: 1.0,
                    noise: 1.0,
                    seed: 19,
                    strength: 1.0,
                }),
            ),
            full(
                "vhs max analog artifacts",
                LocalEffect::Vhs(VhsParams {
                    chroma_bleed_px: 32.0,
                    chroma_shift_px: 24.0,
                    ghost_offset_px: 64.0,
                    ghost_strength: 1.0,
                    tracking_strength: 1.0,
                    scanline_strength: 1.0,
                    noise: 1.0,
                    desaturation: 1.0,
                    seed: 20,
                    strength: 1.0,
                }),
            ),
            full(
                "data mosh max block shift",
                LocalEffect::DataMosh(DataMoshParams {
                    block_size_px: 2.0,
                    displacement_px: 128.0,
                    direction_degrees: 180.0,
                    low_threshold: 0.0,
                    high_threshold: 1.0,
                    freeze: 1.0,
                    smear: 1.0,
                    rgb_shift_px: 32.0,
                    noise: 1.0,
                    seed: 21,
                    strength: 1.0,
                }),
            ),
            full(
                "pixel sort max vertical segment",
                LocalEffect::PixelSort(PixelSortParams {
                    direction: PixelSortDirection::Vertical,
                    order: PixelSortOrder::LightToDark,
                    low_threshold: 0.0,
                    high_threshold: 1.0,
                    max_segment_px: 512,
                    strength: 1.0,
                }),
            ),
            full(
                "old film max artifacts",
                LocalEffect::OldFilm(OldFilmParams {
                    sepia: 1.0,
                    fade: 1.0,
                    vignette: 1.0,
                    grain: 1.0,
                    dust: 1.0,
                    scratches: 1.0,
                    seed: 21,
                    strength: 1.0,
                }),
            ),
            full(
                "water caustics max contrast",
                LocalEffect::WaterCaustics(WaterCausticsParams {
                    scale_px: 8.0,
                    intensity: 2.0,
                    contrast: 1.0,
                    tint: 1.0,
                    depth: 1.0,
                    phase: 0.75,
                    seed: 22,
                    strength: 1.0,
                }),
            ),
            full(
                "particle overlay dense petals",
                LocalEffect::ParticleOverlay(ParticleOverlayParams {
                    mode: ParticleOverlayMode::Petals,
                    density: 1.0,
                    size_px: 48.0,
                    length_px: 240.0,
                    angle_degrees: 180.0,
                    opacity: 1.0,
                    color_rgb: [255, 180, 220],
                    seed: 23,
                    strength: 1.0,
                }),
            ),
            full(
                "aurora max curtains",
                LocalEffect::Aurora(AuroraParams {
                    band_count: 12.0,
                    scale_px: 24.0,
                    height: 1.0,
                    waviness: 1.0,
                    softness: 1.0,
                    brightness: 2.0,
                    color_rgb: [80, 255, 180],
                    secondary_rgb: [180, 80, 255],
                    phase: 0.75,
                    seed: 24,
                    strength: 1.0,
                }),
            ),
            full(
                "halftone dense cells",
                LocalEffect::Halftone(HalftoneParams {
                    cell_px: 2,
                    strength: 1.0,
                }),
            ),
            full(
                "screen tone crosshatch dense cells",
                LocalEffect::ScreenTone(ScreenToneParams {
                    mode: ScreenToneMode::CrossHatch,
                    cell_px: 2.0,
                    angle_degrees: 45.0,
                    density: 1.0,
                    gradation: 1.0,
                    softness: 1.0,
                    strength: 1.0,
                }),
            ),
            full(
                "color halftone dense cells",
                LocalEffect::ColorHalftone(ColorHalftoneParams {
                    cell_px: 3.0,
                    angle_offset_degrees: 45.0,
                    dot_gain: 1.0,
                    black_generation: 1.0,
                    softness: 1.0,
                    strength: 1.0,
                }),
            ),
            full(
                "cmyk plate shift max offset",
                LocalEffect::CmykPlateShift(CmykPlateShiftParams {
                    offset_px: 64.0,
                    angle_degrees: 180.0,
                    black_offset_px: -64.0,
                    black_generation: 1.0,
                    ink_gain: 0.35,
                    strength: 1.0,
                }),
            ),
            full(
                "lithograph max grain offset",
                LocalEffect::Lithograph(LithographParams {
                    ink_a_rgb: [255, 40, 80],
                    ink_b_rgb: [0, 175, 210],
                    paper_rgb: [250, 235, 205],
                    ink_density: 1.6,
                    posterization: 1.0,
                    grain: 1.0,
                    misregistration_px: 32.0,
                    angle_degrees: 180.0,
                    paper_texture: 1.0,
                    strength: 1.0,
                    seed: 26,
                }),
            ),
            full(
                "engraving dense crosshatch",
                LocalEffect::Engraving(EngravingParams {
                    ink_rgb: [20, 15, 10],
                    paper_rgb: [250, 238, 210],
                    line_spacing_px: 2.0,
                    line_width: 1.0,
                    angle_degrees: -180.0,
                    crosshatch: 1.0,
                    contour_strength: 1.0,
                    tone_levels: 16.0,
                    ink_density: 1.8,
                    paper_texture: 1.0,
                    strength: 1.0,
                    seed: 27,
                }),
            ),
            full(
                "newspaper print dense texture",
                LocalEffect::NewspaperPrint(NewspaperPrintParams {
                    cell_px: 3.0,
                    dot_gain: 0.45,
                    ink_bleed: 1.0,
                    paper_age: 1.0,
                    paper_texture: 1.0,
                    contrast: 1.0,
                    fade: 1.0,
                    strength: 1.0,
                    seed: 25,
                }),
            ),
            full(
                "textureizer dense canvas",
                LocalEffect::Textureizer(TextureizerParams {
                    mode: TextureizerMode::Canvas,
                    scale_px: 2.0,
                    depth: 1.0,
                    contrast: 2.0,
                    warmth: 1.0,
                    strength: 1.0,
                    seed: 17,
                }),
            ),
            full(
                "star glow max rays",
                LocalEffect::StarGlow(StarGlowParams {
                    ray_count: 12,
                    rotation_degrees: 15.0,
                    threshold: 0.0,
                    length_px: 240.0,
                    strength: 3.0,
                }),
            ),
            full(
                "diffraction starburst max rays",
                LocalEffect::DiffractionStarburst(DiffractionStarburstParams {
                    blade_count: 12,
                    rotation_degrees: 15.0,
                    threshold: 0.0,
                    length_px: 360.0,
                    width_px: 12.0,
                    halo_radius_px: 96.0,
                    chromatic_shift: 1.0,
                    strength: 3.0,
                }),
            ),
            full(
                "edge smooth max radius",
                LocalEffect::EdgeSmooth(EdgeSmoothParams {
                    radius_px: 8.0,
                    strength: 1.0,
                    edge_threshold: 0.0,
                }),
            ),
            full(
                "median max radius",
                LocalEffect::Median(MedianParams {
                    radius_px: 8.0,
                    strength: 1.0,
                }),
            ),
            full(
                "despeckle max radius",
                LocalEffect::Despeckle(DespeckleParams {
                    radius_px: 4.0,
                    threshold: 1.0,
                    strength: 1.0,
                }),
            ),
        ];

        let per_case_budget = Duration::from_secs(2);
        let mut slowest = ("", Duration::from_millis(0));
        for case in cases {
            let start = Instant::now();
            let out = apply_layers(
                src.as_ref(),
                &[LocalAdjustmentLayer::new(case.name, case.mask, case.effect)],
            )
            .unwrap_or_else(|err| panic!("{} failed: {err}", case.name));
            let elapsed = start.elapsed();
            if elapsed > slowest.1 {
                slowest = (case.name, elapsed);
            }
            assert_eq!(out.width, width, "{}", case.name);
            assert_eq!(out.height, height, "{}", case.name);
            assert_eq!(out.pixels.len(), src.pixels.len(), "{}", case.name);
            assert!(
                elapsed <= per_case_budget,
                "{} took {:?}, over {:?}; slowest so far: {} {:?}",
                case.name,
                elapsed,
                per_case_budget,
                slowest.0,
                slowest.1
            );
        }
    }

    #[test]
    fn color_fill_default_waits_for_shape_selection() {
        let src = RgbaImageBuf::new(1, 1, vec![20, 40, 80, 255]).unwrap();
        let params = ColorFillParams::default();
        assert_eq!(params.shape, ColorOverlayShape::Unselected);
        assert_eq!(params.opacity, 1.0);

        let layer =
            LocalAdjustmentLayer::new("fill", LocalMask::Full, LocalEffect::ColorFill(params));
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);

        let layer = LocalAdjustmentLayer::new(
            "fill",
            LocalMask::Full,
            LocalEffect::ColorFill(ColorFillParams {
                shape: ColorOverlayShape::Solid,
                start_rgb: [240, 220, 180],
                ..params
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, vec![240, 220, 180, 255]);
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
    fn color_fill_replaces_masked_rgb_and_preserves_unmasked_pixel() {
        let src = RgbaImageBuf::new(2, 1, vec![20, 40, 80, 255, 120, 140, 160, 255]).unwrap();
        let mask = RasterMask {
            width: 2,
            height: 1,
            alpha: vec![1.0, 0.0],
        };
        let layer = LocalAdjustmentLayer::new(
            "fill",
            LocalMask::Raster(mask),
            LocalEffect::ColorFill(ColorFillParams {
                shape: ColorOverlayShape::Solid,
                start_rgb: [240, 220, 180],
                opacity: 1.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[240, 220, 180, 255]);
        assert_eq!(&out.pixels[4..8], &src.pixels[4..8]);
    }

    #[test]
    fn color_fill_linear_gradient_can_use_three_colors() {
        let src = RgbaImageBuf::new(3, 1, vec![8, 8, 8, 255, 8, 8, 8, 255, 8, 8, 8, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "fill",
            LocalMask::Full,
            LocalEffect::ColorFill(ColorFillParams {
                shape: ColorOverlayShape::Linear,
                start_rgb: [255, 0, 0],
                middle_rgb: [0, 255, 0],
                end_rgb: [0, 0, 255],
                middle_enabled: true,
                midpoint: 0.5,
                angle_degrees: 0.0,
                opacity: 1.0,
                softness: 0.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[255, 0, 0, 255]);
        assert_eq!(&out.pixels[4..8], &[0, 255, 0, 255]);
        assert_eq!(&out.pixels[8..12], &[0, 0, 255, 255]);
    }

    #[test]
    fn color_fill_linear_gradient_can_use_dragged_points() {
        let src = RgbaImageBuf::new(1, 3, vec![8, 8, 8, 255, 8, 8, 8, 255, 8, 8, 8, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "fill",
            LocalMask::Full,
            LocalEffect::ColorFill(ColorFillParams {
                shape: ColorOverlayShape::Linear,
                start_rgb: [255, 0, 0],
                end_rgb: [0, 0, 255],
                linear_points_enabled: true,
                linear_start: [0.5, 0.0],
                linear_end: [0.5, 1.0],
                opacity: 1.0,
                softness: 0.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[255, 0, 0, 255]);
        assert_eq!(&out.pixels[8..12], &[0, 0, 255, 255]);
    }

    #[test]
    fn color_fill_keeps_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(1, 1, vec![11, 22, 33, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "fill",
            LocalMask::Full,
            LocalEffect::ColorFill(ColorFillParams {
                shape: ColorOverlayShape::Solid,
                start_rgb: [255, 0, 0],
                opacity: 1.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn frame_border_draws_inside_edges_and_preserves_center() {
        let src = RgbaImageBuf::new(4, 3, vec![100, 120, 140, 255].repeat(12)).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "frame",
            LocalMask::Full,
            LocalEffect::Frame(FrameParams {
                mode: FrameMode::Border,
                color_rgb: [0, 0, 0],
                opacity: 1.0,
                width_px: 1.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[0, 0, 0, 255]);
        let center = (1 * 4 + 1) * 4;
        assert_eq!(&out.pixels[center..center + 4], &[100, 120, 140, 255]);
        let right_edge = (1 * 4 + 3) * 4;
        assert_eq!(&out.pixels[right_edge..right_edge + 4], &[0, 0, 0, 255]);
    }

    #[test]
    fn frame_letterbox_adds_bars_without_recoloring_middle() {
        let src = RgbaImageBuf::new(4, 4, vec![180, 170, 160, 255].repeat(16)).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "letterbox",
            LocalMask::Full,
            LocalEffect::Frame(FrameParams {
                mode: FrameMode::Letterbox,
                color_rgb: [0, 0, 0],
                opacity: 1.0,
                aspect_ratio: 2.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[0, 0, 0, 255]);
        let middle = (1 * 4 + 1) * 4;
        assert_eq!(&out.pixels[middle..middle + 4], &[180, 170, 160, 255]);
        let bottom = (3 * 4 + 2) * 4;
        assert_eq!(&out.pixels[bottom..bottom + 4], &[0, 0, 0, 255]);
    }

    #[test]
    fn frame_rounded_matte_colors_corners_and_keeps_center() {
        let src = RgbaImageBuf::new(5, 5, vec![220, 210, 200, 255].repeat(25)).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "rounded matte",
            LocalMask::Full,
            LocalEffect::Frame(FrameParams {
                mode: FrameMode::RoundedMatte,
                color_rgb: [12, 14, 16],
                opacity: 1.0,
                corner_radius_px: 2.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[12, 14, 16, 255]);
        let center = (2 * 5 + 2) * 4;
        assert_eq!(&out.pixels[center..center + 4], &[220, 210, 200, 255]);
    }

    #[test]
    fn color_overlay_multiply_darkens_without_changing_alpha() {
        let src = RgbaImageBuf::new(1, 1, vec![200, 160, 120, 77]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "overlay",
            LocalMask::Full,
            LocalEffect::ColorOverlay(ColorOverlayParams {
                shape: ColorOverlayShape::Solid,
                blend_mode: ColorOverlayBlendMode::Multiply,
                start_rgb: [128, 128, 255],
                opacity: 1.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] < src.pixels[0]);
        assert!(out.pixels[1] < src.pixels[1]);
        assert_eq!(out.pixels[2], src.pixels[2]);
        assert_eq!(out.pixels[3], src.pixels[3]);
    }

    #[test]
    fn color_overlay_linear_gradient_varies_by_position() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![128, 128, 128, 255, 128, 128, 128, 255, 128, 128, 128, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "gradient overlay",
            LocalMask::Full,
            LocalEffect::ColorOverlay(ColorOverlayParams {
                shape: ColorOverlayShape::Linear,
                blend_mode: ColorOverlayBlendMode::Normal,
                start_rgb: [255, 0, 0],
                end_rgb: [0, 0, 255],
                angle_degrees: 0.0,
                opacity: 1.0,
                softness: 0.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > out.pixels[8]);
        assert!(out.pixels[10] > out.pixels[2]);
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn color_overlay_keeps_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(1, 1, vec![11, 22, 33, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "overlay",
            LocalMask::Full,
            LocalEffect::ColorOverlay(ColorOverlayParams {
                shape: ColorOverlayShape::Solid,
                blend_mode: ColorOverlayBlendMode::Normal,
                start_rgb: [255, 0, 0],
                opacity: 1.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn neon_glow_spreads_saturated_color_below_luma_threshold() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                0, 0, 0, 255, 0, 0, 0, 255, 0, 200, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "neon",
            LocalMask::Full,
            LocalEffect::NeonGlow(NeonGlowParams {
                threshold: 0.82,
                by_saturation: true,
                inner_radius_px: 1.0,
                outer_radius_px: 0.0,
                strength: 1.0,
                inner_amount: 1.0,
                outer_amount: 0.0,
                glow_saturation: 0.4,
                screen_blend: true,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(
            out.pixels[5] > src.pixels[5],
            "neighbor should receive green glow"
        );
        assert!(
            out.pixels[6] > src.pixels[6],
            "neighbor should receive blue glow"
        );
        assert_eq!(out.pixels[7], 255);
    }

    #[test]
    fn neon_glow_can_filter_source_color() {
        let src = RgbaImageBuf::new(3, 1, vec![255, 32, 32, 255, 0, 0, 0, 255, 32, 80, 255, 255])
            .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "neon",
            LocalMask::Full,
            LocalEffect::NeonGlow(NeonGlowParams {
                threshold: 0.4,
                by_saturation: true,
                inner_radius_px: 0.0,
                outer_radius_px: 1.0,
                strength: 1.0,
                inner_amount: 0.0,
                outer_amount: 1.0,
                glow_saturation: 0.0,
                source_color_enabled: true,
                source_rgb: [255, 32, 32],
                source_tolerance: 0.05,
                source_feather: 0.02,
                screen_blend: false,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(
            out.pixels[4] > src.pixels[4],
            "red source should glow into center"
        );
        assert_eq!(
            out.pixels[10], src.pixels[10],
            "blue source should be excluded by source color"
        );
    }

    #[test]
    fn neon_glow_ignores_transparent_hidden_rgb() {
        let src =
            RgbaImageBuf::new(3, 1, vec![0, 0, 0, 255, 0, 255, 255, 0, 0, 0, 0, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "neon",
            LocalMask::Full,
            LocalEffect::NeonGlow(NeonGlowParams {
                threshold: 0.2,
                inner_radius_px: 1.0,
                outer_radius_px: 0.0,
                strength: 1.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
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
    fn photo_filter_warms_color_and_preserves_luminosity() {
        let src = RgbaImageBuf::new(2, 1, vec![128, 128, 128, 99, 12, 34, 56, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "photo filter",
            LocalMask::Full,
            LocalEffect::PhotoFilter(PhotoFilterParams {
                preset: PhotoFilterPreset::Warm85,
                density: 0.70,
                preserve_luminosity: true,
                strength: 1.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > out.pixels[2]);
        let original_luma = luma01(128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0);
        let filtered_luma = luma01(
            out.pixels[0] as f32 / 255.0,
            out.pixels[1] as f32 / 255.0,
            out.pixels[2] as f32 / 255.0,
        );
        assert!((filtered_luma - original_luma).abs() < 0.04);
        assert_eq!(out.pixels[3], 99);
        assert_eq!(&out.pixels[4..8], &[12, 34, 56, 0]);
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
    fn part_color_desaturates_non_target_colors() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![220, 20, 20, 255, 20, 220, 20, 255, 20, 20, 220, 128],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "part color",
            LocalMask::Full,
            LocalEffect::PartColor(PartColorParams {
                target_rgb: [220, 20, 20],
                range_degrees: 10.0,
                feather_degrees: 0.0,
                gray_strength: 1.0,
                selected_saturation: 0.0,
                selected_lightness: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        assert_eq!(&out.pixels[0..4], &src.pixels[0..4]);
        assert_eq!(out.pixels[4], out.pixels[5]);
        assert_eq!(out.pixels[5], out.pixels[6]);
        assert_eq!(out.pixels[8], out.pixels[9]);
        assert_eq!(out.pixels[9], out.pixels[10]);
        assert_eq!(out.pixels[11], 128);
    }

    #[test]
    fn part_color_can_boost_selected_color() {
        let src = RgbaImageBuf::new(2, 1, vec![180, 70, 70, 255, 70, 180, 70, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "part color",
            LocalMask::Full,
            LocalEffect::PartColor(PartColorParams {
                target_rgb: [220, 20, 20],
                range_degrees: 18.0,
                feather_degrees: 12.0,
                gray_strength: 1.0,
                selected_saturation: 60.0,
                selected_lightness: 8.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        assert!(out.pixels[0] > src.pixels[0]);
        assert!(out.pixels[1] <= src.pixels[1]);
        assert_eq!(out.pixels[4], out.pixels[5]);
        assert_eq!(out.pixels[5], out.pixels[6]);
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
    fn monochrome_mixer_red_filter_brightens_red_and_darkens_blue() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![220, 20, 20, 255, 20, 220, 20, 255, 20, 20, 220, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "bw mix",
            LocalMask::Full,
            LocalEffect::MonochromeMixer(MonochromeMixerParams {
                red: 70.0,
                yellow: 20.0,
                blue: -70.0,
                cyan: -20.0,
                strength: 1.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        for px in out.pixels.chunks_exact(4) {
            assert_eq!(px[0], px[1]);
            assert_eq!(px[1], px[2]);
        }
        assert!(out.pixels[0] > out.pixels[8]);
        assert!(out.pixels[4] > out.pixels[8]);
    }

    #[test]
    fn monochrome_mixer_tints_and_preserves_transparent_rgb() {
        let src = RgbaImageBuf::new(2, 1, vec![120, 140, 170, 255, 10, 30, 50, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "sepia bw",
            LocalMask::Full,
            LocalEffect::MonochromeMixer(MonochromeMixerParams {
                tint_rgb: [196, 132, 68],
                tint_strength: 0.70,
                strength: 1.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        assert!(out.pixels[0] > out.pixels[1]);
        assert!(out.pixels[1] > out.pixels[2]);
        assert_eq!(out.pixels[3], 255);
        assert_eq!(&out.pixels[4..8], &[10, 30, 50, 0]);
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
    fn retro_palette_gameboy_maps_to_green_palette_and_preserves_alpha() {
        let src = RgbaImageBuf::new(2, 1, vec![16, 16, 16, 77, 240, 240, 240, 99]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "retro palette",
            LocalMask::Full,
            LocalEffect::RetroPalette(RetroPaletteParams {
                mode: RetroPaletteMode::GameBoy,
                dither: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..3], &RETRO_GAMEBOY_PALETTE[0]);
        assert_eq!(&out.pixels[4..7], &RETRO_GAMEBOY_PALETTE[3]);
        assert_eq!(out.pixels[3], 77);
        assert_eq!(out.pixels[7], 99);
    }

    #[test]
    fn retro_palette_dither_1bit_outputs_monochrome() {
        let src = RgbaImageBuf::new(2, 1, vec![40, 90, 130, 255, 220, 210, 200, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "retro palette",
            LocalMask::Full,
            LocalEffect::RetroPalette(RetroPaletteParams {
                mode: RetroPaletteMode::Dither1Bit,
                dither: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        for px in out.pixels.chunks_exact(4) {
            assert_eq!(px[0], px[1]);
            assert_eq!(px[1], px[2]);
            assert!(px[0] == 0 || px[0] == 255);
        }
    }

    #[test]
    fn retro_palette_keeps_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![255, 0, 0, 0, 120, 150, 180, 255, 60, 90, 120, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "retro palette",
            LocalMask::Full,
            LocalEffect::RetroPalette(RetroPaletteParams {
                mode: RetroPaletteMode::Famicom,
                dither: 1.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[255, 0, 0, 0]);
        assert_eq!(out.pixels[7], 255);
    }

    #[test]
    fn retro_palette_pc98_adaptive_limits_visible_colors() {
        let mut pixels = Vec::new();
        for y in 0..8 {
            for x in 0..8 {
                pixels.extend_from_slice(&[
                    (x * 31 + y * 7) as u8,
                    (y * 29 + x * 5) as u8,
                    ((x + y) * 17) as u8,
                    255,
                ]);
            }
        }
        let src = RgbaImageBuf::new(8, 8, pixels).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "retro palette",
            LocalMask::Full,
            LocalEffect::RetroPalette(RetroPaletteParams {
                mode: RetroPaletteMode::Pc98,
                dither: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let colors = out
            .pixels
            .chunks_exact(4)
            .map(|px| [px[0], px[1], px[2]])
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            colors.len() <= 16,
            "PC-98 adaptive used {} colors",
            colors.len()
        );
        assert!(
            colors.len() > 1,
            "PC-98 adaptive should keep image-specific color variety"
        );
    }

    #[test]
    fn retro_palette_mega_drive_uses_3bit_channel_grid() {
        let src = RgbaImageBuf::new(
            4,
            1,
            vec![
                10, 60, 130, 255, 80, 140, 220, 255, 150, 30, 90, 255, 250, 200, 40, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "retro palette",
            LocalMask::Full,
            LocalEffect::RetroPalette(RetroPaletteParams {
                mode: RetroPaletteMode::MegaDrive,
                dither: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        for px in out.pixels.chunks_exact(4) {
            assert!(retro_palette_channel_is_bit_grid(px[0], 3));
            assert!(retro_palette_channel_is_bit_grid(px[1], 3));
            assert!(retro_palette_channel_is_bit_grid(px[2], 3));
        }
    }

    fn retro_palette_channel_is_bit_grid(channel: u8, bits: u8) -> bool {
        let levels = ((1_u32 << bits) - 1).max(1);
        (0..=levels).any(|level| ((level * 255 + levels / 2) / levels) as u8 == channel)
    }

    #[test]
    fn crt_display_draws_scanlines_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            2,
            4,
            vec![
                120, 120, 120, 201, 120, 120, 120, 202, 120, 120, 120, 203, 120, 120, 120, 204,
                120, 120, 120, 205, 120, 120, 120, 206, 120, 120, 120, 207, 120, 120, 120, 208,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "crt",
            LocalMask::Full,
            LocalEffect::CrtDisplay(CrtDisplayParams {
                scanline_spacing_px: 4.0,
                scanline_depth: 1.0,
                mask_strength: 0.0,
                curvature: 0.0,
                bloom: 0.0,
                horizontal_blur: 0.0,
                brightness: 1.0,
                strength: 1.0,
                ..CrtDisplayParams::preset(CrtDisplayMode::Simple)
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] < out.pixels[8]);
        for (src_px, out_px) in src.pixels.chunks_exact(4).zip(out.pixels.chunks_exact(4)) {
            assert_eq!(out_px[3], src_px[3]);
        }
    }

    #[test]
    fn crt_display_aperture_mask_separates_channels() {
        let src = solid(3, 1, [120, 120, 120, 255]);
        let layer = LocalAdjustmentLayer::new(
            "crt",
            LocalMask::Full,
            LocalEffect::CrtDisplay(CrtDisplayParams {
                scanline_depth: 0.0,
                mask_strength: 0.85,
                curvature: 0.0,
                bloom: 0.0,
                horizontal_blur: 0.0,
                brightness: 1.0,
                strength: 1.0,
                ..CrtDisplayParams::preset(CrtDisplayMode::Simple)
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(
            out.pixels
                .chunks_exact(4)
                .any(|px| px[0] != px[1] || px[1] != px[2])
        );
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 255));
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
    fn solarize_reverses_highlights_and_preserves_alpha() {
        let src = RgbaImageBuf::new(2, 1, vec![64, 64, 64, 77, 200, 200, 200, 88]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "solarize",
            LocalMask::Full,
            LocalEffect::Solarize(SolarizeParams {
                threshold: 0.5,
                softness: 0.0,
                inversion: 1.0,
                contrast: 0.0,
                color_amount: 1.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[64, 64, 64, 77]);
        assert_eq!(&out.pixels[4..8], &[55, 55, 55, 88]);
    }

    #[test]
    fn solarize_monochrome_mode_outputs_gray() {
        let src = RgbaImageBuf::new(1, 1, vec![240, 120, 40, 99]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "solarize",
            LocalMask::Full,
            LocalEffect::Solarize(SolarizeParams {
                threshold: 0.3,
                softness: 0.0,
                inversion: 1.0,
                contrast: 0.0,
                color_amount: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels[0], out.pixels[1]);
        assert_eq!(out.pixels[1], out.pixels[2]);
        assert_eq!(out.pixels[3], 99);
    }

    #[test]
    fn glowing_edges_draws_colored_edges_and_preserves_alpha() {
        let src =
            RgbaImageBuf::new(3, 1, vec![0, 0, 0, 55, 255, 255, 255, 66, 0, 0, 0, 77]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "glowing edges",
            LocalMask::Full,
            LocalEffect::GlowingEdges(GlowingEdgesParams {
                threshold: 0.1,
                softness: 0.0,
                edge_width_px: 1.0,
                glow_radius_px: 0.0,
                edge_brightness: 1.0,
                glow_strength: 0.0,
                hue_degrees: 180.0,
                color_amount: 1.0,
                background_amount: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[1] > out.pixels[0]);
        assert!(out.pixels[2] > out.pixels[0]);
        assert_eq!(&out.pixels[4..7], &[0, 0, 0]);
        assert!(out.pixels[9] > out.pixels[8]);
        assert!(out.pixels[10] > out.pixels[8]);
        assert_eq!(out.pixels[3], 55);
        assert_eq!(out.pixels[11], 77);
    }

    #[test]
    fn glowing_edges_radius_spreads_light_to_neighbor() {
        let src =
            RgbaImageBuf::new(3, 1, vec![0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "glowing edges",
            LocalMask::Full,
            LocalEffect::GlowingEdges(GlowingEdgesParams {
                threshold: 0.5,
                softness: 0.0,
                edge_width_px: 1.0,
                glow_radius_px: 1.0,
                edge_brightness: 1.0,
                glow_strength: 1.0,
                hue_degrees: 180.0,
                color_amount: 1.0,
                background_amount: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[5] > 0, "green glow should reach center pixel");
        assert!(out.pixels[6] > 0, "blue glow should reach center pixel");
    }

    #[test]
    fn glowing_edges_zero_strength_is_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "glowing edges",
            LocalMask::Full,
            LocalEffect::GlowingEdges(GlowingEdgesParams {
                threshold: 0.0,
                softness: 0.0,
                edge_width_px: 12.0,
                glow_radius_px: 24.0,
                edge_brightness: 3.0,
                glow_strength: 3.0,
                hue_degrees: 300.0,
                color_amount: 1.0,
                background_amount: 0.0,
                strength: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn oil_paint_preserves_flat_color_and_alpha() {
        let src = RgbaImageBuf::new(
            2,
            2,
            vec![
                90, 120, 150, 44, 90, 120, 150, 55, 90, 120, 150, 66, 90, 120, 150, 77,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "oil paint",
            LocalMask::Full,
            LocalEffect::OilPaint(OilPaintParams {
                radius_px: 5.0,
                saturation: 0.0,
                contrast: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn oil_paint_ignores_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(2, 1, vec![255, 0, 0, 0, 20, 60, 200, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "oil paint",
            LocalMask::Full,
            LocalEffect::OilPaint(OilPaintParams {
                radius_px: 1.0,
                saturation: 0.0,
                contrast: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[4..8], &[20, 60, 200, 255]);
    }

    #[test]
    fn oil_paint_zero_strength_is_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "oil paint",
            LocalMask::Full,
            LocalEffect::OilPaint(OilPaintParams {
                radius_px: 12.0,
                saturation: 1.0,
                contrast: 1.0,
                strength: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
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
    fn halation_spreads_warm_light_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 20, 20, 77, 255, 255, 255, 77, 20, 20, 20, 77],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "halation",
            LocalMask::Full,
            LocalEffect::Halation(HalationParams {
                threshold: 0.50,
                radius_px: 1.0,
                strength: 1.5,
                warmth: 1.0,
                tint_rgb: [255, 220, 176],
                edge_bias: 0.0,
                screen_blend: true,
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        assert!(out.pixels[0] > src.pixels[0]);
        assert!(out.pixels[0] >= out.pixels[2]);
        assert_eq!(out.pixels[3], 77);
    }

    #[test]
    fn color_dodge_glow_spreads_tinted_light_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![24, 24, 24, 99, 255, 255, 255, 99, 24, 24, 24, 99],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "color dodge glow",
            LocalMask::Full,
            LocalEffect::ColorDodgeGlow(ColorDodgeGlowParams {
                threshold: 0.0,
                radius_px: 1.0,
                strength: 1.0,
                dodge_amount: 0.75,
                color_rgb: [255, 80, 32],
                color_strength: 1.0,
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        assert!(out.pixels[0] > src.pixels[0]);
        assert!(out.pixels[0] > out.pixels[2]);
        assert_eq!(out.pixels[3], 99);
    }

    #[test]
    fn color_dodge_glow_can_escape_mask_when_post_mask_is_off() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![24, 24, 24, 255, 255, 255, 255, 255, 24, 24, 24, 255],
        )
        .unwrap();
        let mask = LocalMask::Raster(RasterMask {
            width: 3,
            height: 1,
            alpha: vec![0.0, 1.0, 0.0],
        });
        let layer = LocalAdjustmentLayer::new(
            "color dodge glow",
            mask,
            LocalEffect::ColorDodgeGlow(ColorDodgeGlowParams {
                threshold: 0.0,
                radius_px: 1.0,
                strength: 1.0,
                dodge_amount: 0.0,
                color_rgb: [255, 255, 255],
                color_strength: 0.0,
            }),
        );

        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        assert!(out.pixels[0] > src.pixels[0]);
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn god_rays_extend_bright_pixel_away_from_center() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                20, 20, 20, 255, 255, 245, 220, 255, 20, 20, 20, 255, 20, 20, 20, 255, 20, 20, 20,
                255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "god rays",
            LocalMask::Full,
            LocalEffect::GodRays(GodRaysParams {
                center: [0.0, 0.0],
                threshold: 0.5,
                length_px: 4.0,
                decay: 1.0,
                strength: 2.0,
                warm_tint: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(
            out.pixels[8] > src.pixels[8],
            "pixel to the right of the source should receive a ray"
        );
        assert!(out.pixels[12] > src.pixels[12]);
        assert_eq!(out.pixels[15], 255);
    }

    #[test]
    fn lens_flare_adds_light_and_ghosts_while_preserving_alpha() {
        let src = RgbaImageBuf::new(
            9,
            1,
            vec![
                20, 20, 20, 91, 20, 20, 20, 92, 20, 20, 20, 93, 20, 20, 20, 94, 20, 20, 20, 95, 20,
                20, 20, 96, 20, 20, 20, 97, 20, 20, 20, 98, 20, 20, 20, 99,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "lens flare",
            LocalMask::Full,
            LocalEffect::LensFlare(LensFlareParams {
                center: [0.0, 0.0],
                radius_px: 4.0,
                strength: 1.5,
                core_strength: 1.0,
                halo_strength: 0.0,
                ghost_strength: 1.6,
                streak_strength: 0.0,
                warm_tint: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > src.pixels[0]);
        assert!(
            out.pixels[20] > src.pixels[20],
            "ghost artifacts should appear along the lens axis"
        );
        assert_eq!(out.pixels[3], 91);
        assert_eq!(out.pixels[35], 99);
    }

    #[test]
    fn anamorphic_flare_extends_bright_pixel_horizontally() {
        let mut pixels = vec![0_u8; 7 * 3 * 4];
        for (idx, px) in pixels.chunks_exact_mut(4).enumerate() {
            px[3] = 201 + (idx / 7) as u8;
        }
        let center = (7 + 3) * 4;
        pixels[center..center + 4].copy_from_slice(&[255, 255, 255, 202]);
        let src = RgbaImageBuf::new(7, 3, pixels).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "anamorphic flare",
            LocalMask::Full,
            LocalEffect::AnamorphicFlare(AnamorphicFlareParams {
                threshold: 0.2,
                length_px: 3.0,
                thickness_px: 0.0,
                strength: 1.0,
                color_rgb: [64, 140, 255],
                color_strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let left = (1 * 7 + 1) * 4;
        // (row 0, col 3) * RGBA; `0 * 7` is the explicit row term for readability.
        #[allow(clippy::erasing_op)]
        let above = (0 * 7 + 3) * 4;
        assert!(out.pixels[left + 2] > src.pixels[left + 2]);
        assert!(out.pixels[left + 2] > out.pixels[left]);
        assert_eq!(&out.pixels[above..above + 3], &src.pixels[above..above + 3]);
        assert_eq!(out.pixels[3], 201);
        assert_eq!(out.pixels[83], 203);
    }

    #[test]
    fn anamorphic_flare_can_escape_mask_when_post_mask_is_off() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255,
            ],
        )
        .unwrap();
        let mask = LocalMask::Raster(RasterMask {
            width: 5,
            height: 1,
            alpha: vec![0.0, 1.0, 0.0, 0.0, 0.0],
        });
        let layer = LocalAdjustmentLayer::new(
            "anamorphic flare",
            mask,
            LocalEffect::AnamorphicFlare(AnamorphicFlareParams {
                threshold: 0.0,
                length_px: 3.0,
                thickness_px: 0.0,
                strength: 1.0,
                color_rgb: [255, 255, 255],
                color_strength: 0.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[8] > src.pixels[8]);
        assert_eq!(out.pixels[19], 255);
    }

    #[test]
    fn anamorphic_flare_ignores_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(3, 1, vec![0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "anamorphic flare",
            LocalMask::Full,
            LocalEffect::AnamorphicFlare(AnamorphicFlareParams {
                threshold: 0.0,
                length_px: 2.0,
                thickness_px: 0.0,
                strength: 2.0,
                color_rgb: [64, 140, 255],
                color_strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[0, 0, 0, 255]);
        assert_eq!(&out.pixels[8..12], &[0, 0, 0, 255]);
    }

    #[test]
    fn light_leak_brightens_near_source_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            4,
            1,
            vec![
                24, 24, 24, 201, 24, 24, 24, 202, 24, 24, 24, 203, 24, 24, 24, 204,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "light leak",
            LocalMask::Full,
            LocalEffect::LightLeak(LightLeakParams {
                center: [0.0, 0.0],
                color_rgb: [255, 130, 60],
                radius: 0.95,
                intensity: 1.0,
                falloff: 1.2,
                haze: 0.0,
                streak_strength: 0.0,
                streak_angle_degrees: 0.0,
                strength: 1.0,
                seed: 3,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > src.pixels[0]);
        assert!(out.pixels[0] > out.pixels[12]);
        assert!(out.pixels[0] > out.pixels[2]);
        assert_eq!(out.pixels[3], 201);
        assert_eq!(out.pixels[15], 204);
    }

    #[test]
    fn light_leak_position_moves_bright_corner() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 20, 20, 255, 20, 20, 20, 255, 20, 20, 20, 255],
        )
        .unwrap();
        let left = LocalAdjustmentLayer::new(
            "light leak",
            LocalMask::Full,
            LocalEffect::LightLeak(LightLeakParams {
                center: [0.0, 0.0],
                radius: 0.70,
                intensity: 1.0,
                falloff: 1.0,
                haze: 0.0,
                streak_strength: 0.0,
                strength: 1.0,
                ..LightLeakParams::default()
            }),
        );
        let right = LocalAdjustmentLayer::new(
            "light leak",
            LocalMask::Full,
            LocalEffect::LightLeak(LightLeakParams {
                center: [1.0, 0.0],
                radius: 0.70,
                intensity: 1.0,
                falloff: 1.0,
                haze: 0.0,
                streak_strength: 0.0,
                strength: 1.0,
                ..LightLeakParams::default()
            }),
        );
        let left_out = apply_layers(src.as_ref(), &[left]).unwrap();
        let right_out = apply_layers(src.as_ref(), &[right]).unwrap();
        assert!(left_out.pixels[0] > left_out.pixels[8]);
        assert!(right_out.pixels[8] > right_out.pixels[0]);
    }

    #[test]
    fn light_leak_keeps_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(1, 1, vec![200, 80, 20, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "light leak",
            LocalMask::Full,
            LocalEffect::LightLeak(LightLeakParams {
                strength: 1.0,
                ..LightLeakParams::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn backlight_haze_lifts_near_light_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            4,
            1,
            vec![
                32, 32, 32, 210, 32, 32, 32, 211, 32, 32, 32, 212, 32, 32, 32, 213,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "backlight haze",
            LocalMask::Full,
            LocalEffect::BacklightHaze(BacklightHazeParams {
                center: [0.0, 0.0],
                color_rgb: [255, 226, 180],
                radius: 0.85,
                falloff: 1.0,
                haze: 0.75,
                glow: 0.55,
                shadow_lift: 0.70,
                contrast_fade: 0.35,
                saturation_fade: 0.15,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > src.pixels[0]);
        assert!(out.pixels[0] > out.pixels[12]);
        assert_eq!(out.pixels[3], 210);
        assert_eq!(out.pixels[15], 213);
    }

    #[test]
    fn backlight_haze_tints_toward_light_color() {
        let src = RgbaImageBuf::new(1, 1, vec![80, 80, 80, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "backlight haze",
            LocalMask::Full,
            LocalEffect::BacklightHaze(BacklightHazeParams {
                center: [0.0, 0.0],
                color_rgb: [255, 180, 80],
                radius: 1.0,
                falloff: 1.0,
                haze: 1.0,
                glow: 0.0,
                shadow_lift: 0.0,
                contrast_fade: 0.0,
                saturation_fade: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > out.pixels[2]);
        assert!(out.pixels[1] > out.pixels[2]);
    }

    #[test]
    fn backlight_haze_keeps_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(1, 1, vec![250, 80, 20, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "backlight haze",
            LocalMask::Full,
            LocalEffect::BacklightHaze(BacklightHazeParams {
                strength: 1.0,
                ..BacklightHazeParams::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn radial_speed_lines_keep_center_blank_and_draw_rays() {
        let mut pixels = vec![20_u8; 7 * 7 * 4];
        for chunk in pixels.chunks_exact_mut(4) {
            chunk[3] = 255;
        }
        let src = RgbaImageBuf::new(7, 7, pixels).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "radial speed lines",
            LocalMask::Full,
            LocalEffect::SpeedLines(SpeedLinesParams {
                mode: SpeedLinesMode::Radial,
                center: [0.5, 0.5],
                angle_degrees: 0.0,
                line_count: 4,
                line_width_px: 4.0,
                length: 1.0,
                inner_radius: 0.25,
                outer_radius: 1.0,
                softness: 0.0,
                strength: 1.0,
                color_rgb: [255, 255, 255],
                seed: 1,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let center = (3 * 7 + 3) * 4;
        let ray = (3 * 7 + 6) * 4;
        assert_eq!(out.pixels[center], src.pixels[center]);
        assert!(out.pixels[ray] > src.pixels[ray]);
        assert_eq!(out.pixels[ray + 3], 255);
    }

    #[test]
    fn parallel_speed_lines_can_darken_edge_lines() {
        let mut pixels = vec![220_u8; 5 * 5 * 4];
        for (idx, chunk) in pixels.chunks_exact_mut(4).enumerate() {
            chunk[3] = 180 + idx as u8;
        }
        let src = RgbaImageBuf::new(5, 5, pixels).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "parallel speed lines",
            LocalMask::Full,
            LocalEffect::SpeedLines(SpeedLinesParams {
                mode: SpeedLinesMode::Parallel,
                center: [0.5, 0.5],
                angle_degrees: 0.0,
                line_count: 4,
                line_width_px: 8.0,
                length: 1.0,
                inner_radius: 0.0,
                outer_radius: 1.0,
                softness: 0.0,
                strength: 1.0,
                color_rgb: [0, 0, 0],
                seed: 1,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let edge = 0;
        let center = (2 * 5 + 2) * 4;
        assert!(out.pixels[edge] < src.pixels[edge]);
        assert_eq!(out.pixels[center], src.pixels[center]);
        assert_eq!(out.pixels[edge + 3], 180);
    }

    #[test]
    fn radial_flash_alternates_white_and_black_wedges() {
        let mut pixels = vec![128_u8; 5 * 5 * 4];
        for chunk in pixels.chunks_exact_mut(4) {
            chunk[3] = 255;
        }
        let src = RgbaImageBuf::new(5, 5, pixels).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "radial flash",
            LocalMask::Full,
            LocalEffect::RadialFlash(RadialFlashParams {
                center: [0.5, 0.5],
                ray_count: 4,
                rotation_degrees: -45.0,
                inner_radius: 0.0,
                outer_radius: 1.0,
                softness: 0.4,
                white_amount: 1.0,
                black_amount: 1.0,
                invert: false,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let right = (2 * 5 + 4) * 4;
        let bottom = (4 * 5 + 2) * 4;
        assert!(out.pixels[right] > src.pixels[right]);
        assert!(out.pixels[bottom] < src.pixels[bottom]);
        assert_eq!(out.pixels[right + 3], 255);
    }

    #[test]
    fn radial_flash_invert_swaps_wedge_tone() {
        let mut pixels = vec![128_u8; 5 * 5 * 4];
        for chunk in pixels.chunks_exact_mut(4) {
            chunk[3] = 255;
        }
        let src = RgbaImageBuf::new(5, 5, pixels).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "radial flash",
            LocalMask::Full,
            LocalEffect::RadialFlash(RadialFlashParams {
                center: [0.5, 0.5],
                ray_count: 4,
                rotation_degrees: -45.0,
                inner_radius: 0.0,
                outer_radius: 1.0,
                softness: 0.4,
                white_amount: 1.0,
                black_amount: 1.0,
                invert: true,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let right = (2 * 5 + 4) * 4;
        assert!(out.pixels[right] < src.pixels[right]);
    }

    #[test]
    fn cloud_fog_lifts_dark_pixels_and_preserves_alpha() {
        let src = RgbaImageBuf::new(3, 1, vec![20, 20, 20, 80, 20, 20, 20, 120, 20, 20, 20, 160])
            .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "fog",
            LocalMask::Full,
            LocalEffect::CloudFog(CloudFogParams {
                mode: CloudFogMode::Fog,
                scale_px: 64.0,
                detail: 0.0,
                density: 1.0,
                contrast: 0.0,
                height_fade: 0.0,
                opacity: 1.0,
                color_rgb: [240, 240, 240],
                seed: 1,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > src.pixels[0]);
        assert!(out.pixels[4] > src.pixels[4]);
        assert_eq!(out.pixels[3], 80);
        assert_eq!(out.pixels[11], 160);
    }

    #[test]
    fn cloud_fog_height_fade_can_limit_lower_pixels() {
        let src = RgbaImageBuf::new(
            1,
            3,
            vec![40, 40, 40, 255, 40, 40, 40, 255, 40, 40, 40, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "clouds",
            LocalMask::Full,
            LocalEffect::CloudFog(CloudFogParams {
                mode: CloudFogMode::Fog,
                scale_px: 64.0,
                detail: 0.0,
                density: 1.0,
                contrast: 0.0,
                height_fade: 1.0,
                opacity: 1.0,
                color_rgb: [255, 255, 255],
                seed: 1,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > out.pixels[4]);
        assert_eq!(out.pixels[8], src.pixels[8]);
        assert_eq!(out.pixels[11], 255);
    }

    #[test]
    fn spotlight_brightens_center_darkens_edge_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                96, 96, 96, 91, 96, 96, 96, 92, 96, 96, 96, 93, 96, 96, 96, 94, 96, 96, 96, 95,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "spotlight",
            LocalMask::Full,
            LocalEffect::Spotlight(SpotlightParams {
                center: [0.5, 0.0],
                radius: 0.05,
                feather: 0.35,
                light_strength: 1.0,
                shadow_strength: 0.65,
                tint_rgb: [255, 230, 180],
                tint_strength: 0.25,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let center = 2 * 4;
        let edge = 0;
        assert!(out.pixels[center] > src.pixels[center]);
        assert!(out.pixels[edge] < src.pixels[edge]);
        assert_eq!(out.pixels[center + 3], 93);
        assert_eq!(out.pixels[edge + 3], 91);
    }

    #[test]
    fn diffuse_glow_lifts_neighbor_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![24, 24, 24, 91, 250, 250, 250, 92, 24, 24, 24, 93],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "diffuse",
            LocalMask::Full,
            LocalEffect::DiffuseGlow(DiffuseGlowParams {
                threshold: 0.5,
                radius_px: 1.0,
                strength: 1.0,
                white_mix: 0.5,
                grain: 0.0,
                seed: 1,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > src.pixels[0]);
        assert_eq!(out.pixels[3], 91);
        assert_eq!(out.pixels[7], 92);
        assert_eq!(out.pixels[11], 93);
    }

    #[test]
    fn diffuse_glow_grain_is_deterministic() {
        let src = solid(4, 4, [180, 180, 180, 255]);
        let layer = LocalAdjustmentLayer::new(
            "diffuse",
            LocalMask::Full,
            LocalEffect::DiffuseGlow(DiffuseGlowParams {
                threshold: 0.2,
                radius_px: 1.0,
                strength: 0.8,
                white_mix: 0.35,
                grain: 0.8,
                seed: 42,
            }),
        );
        let out1 = apply_layers(src.as_ref(), &[layer.clone()]).unwrap();
        let out2 = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out1.pixels, out2.pixels);
        assert_ne!(out1.pixels, src.pixels);
    }

    #[test]
    fn diffuse_glow_zero_strength_is_identity() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 40, 80, 255, 120, 80, 40, 255, 230, 220, 210, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "diffuse",
            LocalMask::Full,
            LocalEffect::DiffuseGlow(DiffuseGlowParams {
                threshold: 0.0,
                radius_px: 32.0,
                strength: 0.0,
                white_mix: 1.0,
                grain: 1.0,
                seed: 7,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn orton_lifts_neighbors_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![20, 20, 20, 203, 230, 210, 170, 203, 20, 20, 20, 203],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "orton",
            LocalMask::Full,
            LocalEffect::Orton(OrtonParams {
                radius_px: 1.0,
                strength: 0.85,
                brightness: 0.45,
                contrast: 0.25,
                saturation: 0.20,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > src.pixels[0]);
        assert!(out.pixels[8] > src.pixels[8]);
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 203));
    }

    #[test]
    fn orton_ignores_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(3, 1, vec![0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "orton",
            LocalMask::Full,
            LocalEffect::Orton(OrtonParams {
                radius_px: 1.0,
                strength: 1.0,
                brightness: 0.7,
                contrast: 0.2,
                saturation: 0.8,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels[0], out.pixels[1]);
        assert_eq!(out.pixels[1], out.pixels[2]);
        assert_eq!(out.pixels[8], out.pixels[9]);
        assert_eq!(out.pixels[9], out.pixels[10]);
        assert_eq!(out.pixels[3], 255);
        assert_eq!(out.pixels[7], 0);
        assert_eq!(out.pixels[11], 255);
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
    fn noise_is_deterministic_and_preserves_alpha() {
        let src = solid(4, 4, [128, 128, 128, 203]);
        let layer = LocalAdjustmentLayer::new(
            "noise",
            LocalMask::Full,
            LocalEffect::Noise(NoiseParams {
                amount: 0.45,
                distribution: NoiseDistribution::Uniform,
                monochrome: true,
                seed: 42,
            }),
        );
        let out1 = apply_layers(src.as_ref(), &[layer.clone()]).unwrap();
        let out2 = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out1.pixels, out2.pixels);
        assert_ne!(out1.pixels, src.pixels);
        assert!(out1.pixels.chunks_exact(4).all(|px| px[3] == 203));
    }

    #[test]
    fn monochrome_noise_keeps_gray_channels_equal() {
        let src = solid(3, 3, [128, 128, 128, 255]);
        let layer = LocalAdjustmentLayer::new(
            "noise",
            LocalMask::Full,
            LocalEffect::Noise(NoiseParams {
                amount: 0.70,
                distribution: NoiseDistribution::Gaussian,
                monochrome: true,
                seed: 7,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(
            out.pixels
                .chunks_exact(4)
                .all(|px| px[0] == px[1] && px[1] == px[2])
        );
    }

    #[test]
    fn color_noise_can_shift_channels_independently() {
        let src = solid(4, 4, [128, 128, 128, 255]);
        let layer = LocalAdjustmentLayer::new(
            "noise",
            LocalMask::Full,
            LocalEffect::Noise(NoiseParams {
                amount: 0.70,
                distribution: NoiseDistribution::Uniform,
                monochrome: false,
                seed: 99,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(
            out.pixels
                .chunks_exact(4)
                .any(|px| px[0] != px[1] || px[1] != px[2])
        );
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
    fn anaglyph_3d_splits_left_and_right_channels() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                0, 10, 200, 255, 50, 20, 150, 255, 100, 30, 100, 255, 150, 40, 50, 255, 200, 50, 0,
                255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "anaglyph",
            LocalMask::Full,
            LocalEffect::Anaglyph3d(AnaglyphParams {
                mode: AnaglyphMode::RedCyan,
                disparity_px: 2.0,
                angle_degrees: 0.0,
                luma_mix: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let center = &out.pixels[8..12];
        assert_eq!(center, &[50, 40, 50, 255]);
    }

    #[test]
    fn anaglyph_3d_preserves_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(3, 1, vec![20, 40, 80, 255, 240, 10, 10, 0, 80, 40, 20, 255])
            .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "anaglyph",
            LocalMask::Full,
            LocalEffect::Anaglyph3d(AnaglyphParams {
                mode: AnaglyphMode::RgbSplit,
                disparity_px: 2.0,
                luma_mix: 0.0,
                strength: 1.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[4..8], &[240, 10, 10, 0]);
        assert!(out.pixels[0] < 80);
        assert_eq!(out.pixels[3], 255);
        assert_eq!(out.pixels[11], 255);
    }

    #[test]
    fn defringe_neutralizes_saturated_edge_fringe_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![240, 240, 240, 201, 255, 0, 255, 202, 20, 20, 20, 203],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "defringe",
            LocalMask::Full,
            LocalEffect::Defringe(DefringeParams {
                radius_px: 1.0,
                edge_threshold: 0.0,
                color_threshold: 0.0,
                neutralize: 1.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[4] < src.pixels[4]);
        assert!(out.pixels[5] > src.pixels[5]);
        assert!(out.pixels[6] < src.pixels[6]);
        assert_eq!(out.pixels[7], 202);
    }

    #[test]
    fn defringe_keeps_saturated_edge_when_neighbor_is_same_color() {
        let src = solid(3, 1, [230, 30, 20, 255]);
        let layer = LocalAdjustmentLayer::new(
            "defringe",
            LocalMask::Full,
            LocalEffect::Defringe(DefringeParams {
                radius_px: 1.0,
                edge_threshold: 0.0,
                color_threshold: 0.0,
                neutralize: 1.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn defringe_skips_transparent_pixels() {
        let src = RgbaImageBuf::new(1, 1, vec![255, 0, 255, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "defringe",
            LocalMask::Full,
            LocalEffect::Defringe(DefringeParams {
                radius_px: 8.0,
                edge_threshold: 0.0,
                color_threshold: 0.0,
                neutralize: 1.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn scanline_glitch_darkens_scanline_and_preserves_alpha() {
        let src = RgbaImageBuf::new(1, 2, vec![200, 200, 200, 91, 200, 200, 200, 92]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "scanline",
            LocalMask::Full,
            LocalEffect::ScanlineGlitch(ScanlineGlitchParams {
                line_spacing_px: 2.0,
                line_strength: 1.0,
                jitter_px: 0.0,
                rgb_shift_px: 0.0,
                block_strength: 0.0,
                noise: 0.0,
                seed: 1,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] < src.pixels[0]);
        assert_eq!(out.pixels[3], 91);
        assert_eq!(out.pixels[7], 92);
    }

    #[test]
    fn scanline_glitch_can_shift_red_channel() {
        let src =
            RgbaImageBuf::new(3, 1, vec![0, 0, 0, 255, 255, 0, 0, 255, 0, 0, 0, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "scanline",
            LocalMask::Full,
            LocalEffect::ScanlineGlitch(ScanlineGlitchParams {
                line_spacing_px: 8.0,
                line_strength: 0.0,
                jitter_px: 0.0,
                rgb_shift_px: 1.0,
                block_strength: 0.0,
                noise: 0.0,
                seed: 1,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > 180);
        assert_eq!(out.pixels[1], 0);
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn scanline_glitch_ignores_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(3, 1, vec![0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "scanline",
            LocalMask::Full,
            LocalEffect::ScanlineGlitch(ScanlineGlitchParams {
                line_spacing_px: 8.0,
                line_strength: 0.0,
                jitter_px: 0.0,
                rgb_shift_px: 1.0,
                block_strength: 0.0,
                noise: 0.0,
                seed: 1,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[0, 0, 0, 255]);
    }

    #[test]
    fn vhs_desaturates_chroma_and_preserves_alpha() {
        let src = RgbaImageBuf::new(1, 1, vec![255, 0, 0, 201]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "vhs",
            LocalMask::Full,
            LocalEffect::Vhs(VhsParams {
                chroma_bleed_px: 0.0,
                chroma_shift_px: 0.0,
                ghost_offset_px: 0.0,
                ghost_strength: 0.0,
                tracking_strength: 0.0,
                scanline_strength: 0.0,
                noise: 0.0,
                desaturation: 1.0,
                seed: 1,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] < src.pixels[0]);
        assert!(out.pixels[1] > src.pixels[1]);
        assert_eq!(out.pixels[3], 201);
    }

    #[test]
    fn vhs_chroma_bleed_spreads_neighbor_color() {
        let src =
            RgbaImageBuf::new(3, 1, vec![0, 0, 0, 255, 255, 0, 0, 255, 0, 0, 0, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "vhs",
            LocalMask::Full,
            LocalEffect::Vhs(VhsParams {
                chroma_bleed_px: 1.0,
                chroma_shift_px: 0.0,
                ghost_offset_px: 0.0,
                ghost_strength: 0.0,
                tracking_strength: 0.0,
                scanline_strength: 0.0,
                noise: 0.0,
                desaturation: 0.0,
                seed: 1,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > src.pixels[0]);
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn vhs_ghost_offsets_bright_pixel() {
        let src = RgbaImageBuf::new(
            4,
            1,
            vec![255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "vhs",
            LocalMask::Full,
            LocalEffect::Vhs(VhsParams {
                chroma_bleed_px: 0.0,
                chroma_shift_px: 0.0,
                ghost_offset_px: 2.0,
                ghost_strength: 1.0,
                tracking_strength: 0.0,
                scanline_strength: 0.0,
                noise: 0.0,
                desaturation: 0.0,
                seed: 1,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[8] > 120);
        assert_eq!(out.pixels[11], 255);
    }

    #[test]
    fn vhs_ignores_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(3, 1, vec![0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "vhs",
            LocalMask::Full,
            LocalEffect::Vhs(VhsParams {
                chroma_bleed_px: 1.0,
                chroma_shift_px: 0.0,
                ghost_offset_px: 0.0,
                ghost_strength: 0.0,
                tracking_strength: 0.0,
                scanline_strength: 0.0,
                noise: 0.0,
                desaturation: 0.0,
                seed: 1,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[0, 0, 0, 255]);
    }

    #[test]
    fn data_mosh_offsets_blocks_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            5,
            3,
            vec![
                20, 20, 20, 91, 80, 40, 40, 92, 160, 40, 40, 93, 220, 80, 40, 94, 40, 220, 40, 95,
                20, 20, 20, 96, 80, 40, 40, 97, 160, 40, 40, 98, 220, 80, 40, 99, 40, 220, 40, 100,
                20, 20, 20, 101, 80, 40, 40, 102, 160, 40, 40, 103, 220, 80, 40, 104, 40, 220, 40,
                105,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "data mosh",
            LocalMask::Full,
            LocalEffect::DataMosh(DataMoshParams {
                block_size_px: 2.0,
                displacement_px: 1.0,
                direction_degrees: 0.0,
                low_threshold: 0.0,
                high_threshold: 1.0,
                freeze: 1.0,
                smear: 0.0,
                rgb_shift_px: 0.0,
                noise: 0.0,
                seed: 1,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_ne!(out.pixels, src.pixels);
        for i in (3..out.pixels.len()).step_by(4) {
            assert_eq!(out.pixels[i], src.pixels[i]);
        }
    }

    #[test]
    fn data_mosh_rgb_shift_separates_channels() {
        let src =
            RgbaImageBuf::new(3, 1, vec![0, 0, 0, 255, 255, 0, 0, 255, 0, 0, 0, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "data mosh",
            LocalMask::Full,
            LocalEffect::DataMosh(DataMoshParams {
                block_size_px: 2.0,
                displacement_px: 0.0,
                direction_degrees: 0.0,
                low_threshold: 0.0,
                high_threshold: 1.0,
                freeze: 0.0,
                smear: 0.0,
                rgb_shift_px: 1.0,
                noise: 0.0,
                seed: 1,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > 180);
        assert_eq!(out.pixels[1], 0);
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn data_mosh_ignores_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(3, 1, vec![0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "data mosh",
            LocalMask::Full,
            LocalEffect::DataMosh(DataMoshParams {
                block_size_px: 2.0,
                displacement_px: 0.0,
                direction_degrees: 0.0,
                low_threshold: 0.0,
                high_threshold: 1.0,
                freeze: 0.0,
                smear: 0.0,
                rgb_shift_px: 1.0,
                noise: 0.0,
                seed: 1,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[0, 0, 0, 255]);
    }

    #[test]
    fn pixel_sort_sorts_horizontal_luma_range() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![10, 10, 10, 255, 200, 200, 200, 255, 100, 100, 100, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "pixel sort",
            LocalMask::Full,
            LocalEffect::PixelSort(PixelSortParams {
                direction: PixelSortDirection::Horizontal,
                order: PixelSortOrder::DarkToLight,
                low_threshold: 0.0,
                high_threshold: 1.0,
                max_segment_px: 16,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(
            &out.pixels[0..12],
            &[10, 10, 10, 255, 100, 100, 100, 255, 200, 200, 200, 255]
        );
    }

    #[test]
    fn pixel_sort_sorts_vertical_light_to_dark() {
        let src = RgbaImageBuf::new(
            1,
            3,
            vec![10, 10, 10, 255, 200, 200, 200, 255, 100, 100, 100, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "pixel sort",
            LocalMask::Full,
            LocalEffect::PixelSort(PixelSortParams {
                direction: PixelSortDirection::Vertical,
                order: PixelSortOrder::LightToDark,
                low_threshold: 0.0,
                high_threshold: 1.0,
                max_segment_px: 16,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(
            &out.pixels[0..12],
            &[200, 200, 200, 255, 100, 100, 100, 255, 10, 10, 10, 255]
        );
    }

    #[test]
    fn pixel_sort_threshold_keeps_dark_separator_in_place() {
        let src = RgbaImageBuf::new(
            4,
            1,
            vec![
                20, 20, 20, 255, 180, 180, 180, 255, 100, 100, 100, 255, 220, 220, 220, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "pixel sort",
            LocalMask::Full,
            LocalEffect::PixelSort(PixelSortParams {
                direction: PixelSortDirection::Horizontal,
                order: PixelSortOrder::DarkToLight,
                low_threshold: 0.30,
                high_threshold: 0.95,
                max_segment_px: 16,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(
            &out.pixels[0..16],
            &[
                20, 20, 20, 255, 100, 100, 100, 255, 180, 180, 180, 255, 220, 220, 220, 255
            ]
        );
    }

    #[test]
    fn pixel_sort_transparent_pixel_breaks_segment() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![0, 0, 0, 255, 255, 255, 255, 0, 128, 128, 128, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "pixel sort",
            LocalMask::Full,
            LocalEffect::PixelSort(PixelSortParams {
                direction: PixelSortDirection::Horizontal,
                order: PixelSortOrder::LightToDark,
                low_threshold: 0.0,
                high_threshold: 1.0,
                max_segment_px: 16,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn old_film_sepia_warms_gray_and_preserves_alpha() {
        let src = RgbaImageBuf::new(1, 1, vec![120, 120, 120, 207]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "old film",
            LocalMask::Full,
            LocalEffect::OldFilm(OldFilmParams {
                sepia: 1.0,
                fade: 0.0,
                vignette: 0.0,
                grain: 0.0,
                dust: 0.0,
                scratches: 0.0,
                seed: 1,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] > out.pixels[1]);
        assert!(out.pixels[1] > out.pixels[2]);
        assert_eq!(out.pixels[3], 207);
    }

    #[test]
    fn old_film_vignette_darkens_corner_more_than_center() {
        let src = solid(3, 3, [200, 200, 200, 255]);
        let layer = LocalAdjustmentLayer::new(
            "old film",
            LocalMask::Full,
            LocalEffect::OldFilm(OldFilmParams {
                sepia: 0.0,
                fade: 0.0,
                vignette: 1.0,
                grain: 0.0,
                dust: 0.0,
                scratches: 0.0,
                seed: 1,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let center = (3 + 1) * 4;
        assert!(out.pixels[0] < out.pixels[center]);
        assert_eq!(out.pixels[center + 3], 255);
    }

    #[test]
    fn old_film_grain_changes_flat_image_and_preserves_alpha() {
        let src = solid(4, 4, [128, 128, 128, 77]);
        let layer = LocalAdjustmentLayer::new(
            "old film",
            LocalMask::Full,
            LocalEffect::OldFilm(OldFilmParams {
                sepia: 0.0,
                fade: 0.0,
                vignette: 0.0,
                grain: 1.0,
                dust: 0.0,
                scratches: 0.0,
                seed: 33,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels.chunks_exact(4).any(|px| px[0] != 128));
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 77));
    }

    #[test]
    fn old_film_skips_transparent_pixels() {
        let src = RgbaImageBuf::new(1, 1, vec![255, 0, 0, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "old film",
            LocalMask::Full,
            LocalEffect::OldFilm(OldFilmParams {
                sepia: 1.0,
                fade: 1.0,
                vignette: 1.0,
                grain: 1.0,
                dust: 1.0,
                scratches: 1.0,
                seed: 1,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn water_caustics_lifts_dark_pixels_and_preserves_alpha() {
        let src = solid(8, 8, [32, 48, 60, 213]);
        let layer = LocalAdjustmentLayer::new(
            "water caustics",
            LocalMask::Full,
            LocalEffect::WaterCaustics(WaterCausticsParams {
                scale_px: 8.0,
                intensity: 2.0,
                contrast: 1.0,
                tint: 0.8,
                depth: 0.0,
                phase: 0.0,
                seed: 3,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(
            out.pixels
                .chunks_exact(4)
                .any(|px| px[0] > 32 || px[1] > 48 || px[2] > 60)
        );
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 213));
    }

    #[test]
    fn water_caustics_phase_changes_pattern() {
        let src = solid(8, 8, [64, 80, 96, 255]);
        let params = WaterCausticsParams {
            scale_px: 8.0,
            intensity: 1.4,
            contrast: 1.0,
            tint: 0.5,
            depth: 0.0,
            phase: 0.0,
            seed: 3,
            strength: 1.0,
        };
        let first = LocalAdjustmentLayer::new(
            "water caustics",
            LocalMask::Full,
            LocalEffect::WaterCaustics(params),
        );
        let second = LocalAdjustmentLayer::new(
            "water caustics",
            LocalMask::Full,
            LocalEffect::WaterCaustics(WaterCausticsParams {
                phase: 0.37,
                ..params
            }),
        );
        let out_first = apply_layers(src.as_ref(), &[first]).unwrap();
        let out_second = apply_layers(src.as_ref(), &[second]).unwrap();
        assert_ne!(out_first.pixels, out_second.pixels);
    }

    #[test]
    fn water_caustics_skips_transparent_pixels() {
        let src = RgbaImageBuf::new(1, 1, vec![255, 0, 0, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "water caustics",
            LocalMask::Full,
            LocalEffect::WaterCaustics(WaterCausticsParams {
                scale_px: 8.0,
                intensity: 2.0,
                contrast: 1.0,
                tint: 1.0,
                depth: 1.0,
                phase: 0.4,
                seed: 9,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn particle_overlay_rain_adds_streaks_and_preserves_alpha() {
        let src = solid(24, 24, [24, 30, 36, 211]);
        let layer = LocalAdjustmentLayer::new(
            "particle overlay",
            LocalMask::Full,
            LocalEffect::ParticleOverlay(ParticleOverlayParams {
                mode: ParticleOverlayMode::Rain,
                density: 1.0,
                size_px: 3.0,
                length_px: 80.0,
                angle_degrees: 90.0,
                opacity: 1.0,
                color_rgb: [220, 238, 255],
                seed: 5,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels.chunks_exact(4).any(|px| px[0] > 24));
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 211));
    }

    #[test]
    fn particle_overlay_modes_create_different_patterns() {
        let src = solid(24, 24, [80, 70, 90, 255]);
        let base = ParticleOverlayParams {
            mode: ParticleOverlayMode::Snow,
            density: 0.85,
            size_px: 5.0,
            length_px: 0.0,
            angle_degrees: 100.0,
            opacity: 1.0,
            color_rgb: [255, 245, 255],
            seed: 3,
            strength: 1.0,
        };
        let snow =
            LocalAdjustmentLayer::new("snow", LocalMask::Full, LocalEffect::ParticleOverlay(base));
        let petals = LocalAdjustmentLayer::new(
            "petals",
            LocalMask::Full,
            LocalEffect::ParticleOverlay(ParticleOverlayParams {
                mode: ParticleOverlayMode::Petals,
                color_rgb: [255, 160, 205],
                ..base
            }),
        );
        let out_snow = apply_layers(src.as_ref(), &[snow]).unwrap();
        let out_petals = apply_layers(src.as_ref(), &[petals]).unwrap();
        assert_ne!(out_snow.pixels, out_petals.pixels);
    }

    #[test]
    fn particle_overlay_skips_transparent_pixels() {
        let src = RgbaImageBuf::new(1, 1, vec![255, 0, 0, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "particle overlay",
            LocalMask::Full,
            LocalEffect::ParticleOverlay(ParticleOverlayParams {
                mode: ParticleOverlayMode::Snow,
                density: 1.0,
                size_px: 48.0,
                length_px: 240.0,
                angle_degrees: 0.0,
                opacity: 1.0,
                color_rgb: [255, 255, 255],
                seed: 1,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn aurora_lifts_dark_pixels_and_preserves_alpha() {
        let src = solid(24, 24, [12, 18, 28, 219]);
        let layer = LocalAdjustmentLayer::new(
            "aurora",
            LocalMask::Full,
            LocalEffect::Aurora(AuroraParams {
                band_count: 6.0,
                scale_px: 32.0,
                height: 1.0,
                waviness: 0.8,
                softness: 0.55,
                brightness: 2.0,
                color_rgb: [80, 255, 160],
                secondary_rgb: [160, 90, 255],
                phase: 0.0,
                seed: 2,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(
            out.pixels
                .chunks_exact(4)
                .any(|px| px[0] > 12 || px[1] > 18 || px[2] > 28)
        );
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 219));
    }

    #[test]
    fn aurora_phase_changes_curtain_pattern() {
        let src = solid(24, 24, [30, 34, 44, 255]);
        let params = AuroraParams {
            band_count: 5.0,
            scale_px: 36.0,
            height: 1.0,
            waviness: 0.9,
            softness: 0.45,
            brightness: 1.5,
            color_rgb: [70, 250, 170],
            secondary_rgb: [160, 80, 255],
            phase: 0.0,
            seed: 4,
            strength: 1.0,
        };
        let first =
            LocalAdjustmentLayer::new("aurora", LocalMask::Full, LocalEffect::Aurora(params));
        let second = LocalAdjustmentLayer::new(
            "aurora",
            LocalMask::Full,
            LocalEffect::Aurora(AuroraParams {
                phase: 0.42,
                ..params
            }),
        );
        let out_first = apply_layers(src.as_ref(), &[first]).unwrap();
        let out_second = apply_layers(src.as_ref(), &[second]).unwrap();
        assert_ne!(out_first.pixels, out_second.pixels);
    }

    #[test]
    fn aurora_skips_transparent_pixels() {
        let src = RgbaImageBuf::new(1, 1, vec![255, 0, 0, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "aurora",
            LocalMask::Full,
            LocalEffect::Aurora(AuroraParams {
                band_count: 12.0,
                scale_px: 24.0,
                height: 1.0,
                waviness: 1.0,
                softness: 1.0,
                brightness: 2.0,
                color_rgb: [80, 255, 180],
                secondary_rgb: [180, 80, 255],
                phase: 0.5,
                seed: 8,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
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
    fn screen_tone_dots_darken_mid_gray_and_preserve_alpha() {
        let src = solid(8, 8, [160, 160, 160, 203]);
        let layer = LocalAdjustmentLayer::new(
            "screen tone",
            LocalMask::Full,
            LocalEffect::ScreenTone(ScreenToneParams {
                mode: ScreenToneMode::Dots,
                cell_px: 4.0,
                angle_degrees: 0.0,
                density: 0.85,
                gradation: 0.25,
                softness: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels.chunks_exact(4).any(|px| px[0] < 160));
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 203));
    }

    #[test]
    fn screen_tone_lines_create_directional_stripes() {
        let src = solid(8, 8, [180, 180, 180, 255]);
        let layer = LocalAdjustmentLayer::new(
            "screen tone",
            LocalMask::Full,
            LocalEffect::ScreenTone(ScreenToneParams {
                mode: ScreenToneMode::Lines,
                cell_px: 4.0,
                angle_degrees: 0.0,
                density: 0.45,
                gradation: 0.0,
                softness: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let row0 = out.pixels[0];
        let row2 = out.pixels[(2 * 8) * 4];
        assert_ne!(row0, row2);
    }

    #[test]
    fn color_halftone_creates_colored_plates_and_preserves_alpha() {
        let src = solid(8, 8, [80, 160, 220, 211]);
        let layer = LocalAdjustmentLayer::new(
            "color halftone",
            LocalMask::Full,
            LocalEffect::ColorHalftone(ColorHalftoneParams {
                cell_px: 4.0,
                angle_offset_degrees: 0.0,
                dot_gain: 0.08,
                black_generation: 0.55,
                softness: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_ne!(out.pixels, src.pixels);
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 211));
        assert!(
            out.pixels
                .chunks_exact(4)
                .any(|px| px[0] != px[1] || px[1] != px[2])
        );
    }

    #[test]
    fn cmyk_plate_shift_offsets_cyan_plate_and_preserves_alpha() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![255, 255, 255, 201, 0, 255, 255, 202, 255, 255, 255, 203],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "cmyk shift",
            LocalMask::Full,
            LocalEffect::CmykPlateShift(CmykPlateShiftParams {
                offset_px: 1.0,
                angle_degrees: 0.0,
                black_offset_px: 0.0,
                black_generation: 0.70,
                ink_gain: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] < 80);
        assert!(out.pixels[1] > 220);
        assert!(out.pixels[2] > 220);
        assert_eq!(out.pixels[3], 201);
        assert_eq!(out.pixels[11], 203);
    }

    #[test]
    fn cmyk_plate_shift_zero_offset_keeps_color_when_ink_gain_is_zero() {
        let src = RgbaImageBuf::new(2, 1, vec![80, 130, 210, 250, 24, 92, 140, 240]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "cmyk shift",
            LocalMask::Full,
            LocalEffect::CmykPlateShift(CmykPlateShiftParams {
                offset_px: 0.0,
                angle_degrees: 35.0,
                black_offset_px: 0.0,
                black_generation: 0.35,
                ink_gain: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn cmyk_plate_shift_ignores_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![255, 255, 255, 255, 0, 255, 255, 0, 255, 255, 255, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "cmyk shift",
            LocalMask::Full,
            LocalEffect::CmykPlateShift(CmykPlateShiftParams {
                offset_px: 1.0,
                angle_degrees: 0.0,
                black_offset_px: 0.0,
                black_generation: 0.70,
                ink_gain: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(&out.pixels[0..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn lithograph_maps_colors_to_two_spot_inks_and_preserves_alpha() {
        let src = RgbaImageBuf::new(2, 1, vec![220, 30, 60, 211, 30, 130, 220, 212]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "lithograph",
            LocalMask::Full,
            LocalEffect::Lithograph(LithographParams {
                ink_a_rgb: [235, 35, 72],
                ink_b_rgb: [30, 145, 220],
                paper_rgb: [248, 238, 214],
                ink_density: 1.0,
                posterization: 0.0,
                grain: 0.0,
                misregistration_px: 0.0,
                angle_degrees: 0.0,
                paper_texture: 0.0,
                strength: 1.0,
                seed: 2,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_ne!(out.pixels, src.pixels);
        assert_eq!(out.pixels[3], 211);
        assert_eq!(out.pixels[7], 212);
        assert!(out.pixels[0] > out.pixels[1]);
        assert!(out.pixels[6] > out.pixels[4]);
    }

    #[test]
    fn lithograph_misregistration_offsets_second_ink() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![255, 255, 255, 255, 30, 130, 220, 255, 255, 255, 255, 255],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "lithograph",
            LocalMask::Full,
            LocalEffect::Lithograph(LithographParams {
                ink_a_rgb: [235, 35, 72],
                ink_b_rgb: [30, 145, 220],
                paper_rgb: [248, 238, 214],
                ink_density: 1.0,
                posterization: 0.0,
                grain: 0.0,
                misregistration_px: 1.0,
                angle_degrees: 0.0,
                paper_texture: 0.0,
                strength: 1.0,
                seed: 3,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[0] < 248);
        assert!(out.pixels[1] < 238);
        assert!(out.pixels[2] > out.pixels[0]);
    }

    #[test]
    fn lithograph_keeps_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(1, 1, vec![20, 120, 220, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "lithograph",
            LocalMask::Full,
            LocalEffect::Lithograph(LithographParams {
                strength: 1.0,
                ..LithographParams::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn engraving_draws_hatch_lines_and_preserves_alpha() {
        let src = solid(4, 2, [150, 150, 150, 210]);
        let layer = LocalAdjustmentLayer::new(
            "engraving",
            LocalMask::Full,
            LocalEffect::Engraving(EngravingParams {
                ink_rgb: [30, 24, 18],
                paper_rgb: [246, 238, 216],
                line_spacing_px: 2.0,
                line_width: 0.82,
                angle_degrees: 0.0,
                crosshatch: 0.0,
                contour_strength: 0.0,
                tone_levels: 6.0,
                ink_density: 1.0,
                paper_texture: 0.0,
                strength: 1.0,
                seed: 4,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_ne!(out.pixels, src.pixels);
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 210));
        assert!(out.pixels[0] < out.pixels[4]);
        assert!(out.pixels[0] < 180);
        assert!(out.pixels[4] > 220);
    }

    #[test]
    fn engraving_crosshatch_adds_secondary_lines() {
        let src = solid(3, 3, [50, 50, 50, 255]);
        let base_params = EngravingParams {
            ink_rgb: [30, 24, 18],
            paper_rgb: [246, 238, 216],
            line_spacing_px: 2.0,
            line_width: 0.55,
            angle_degrees: 0.0,
            crosshatch: 0.0,
            contour_strength: 0.0,
            tone_levels: 6.0,
            ink_density: 1.0,
            paper_texture: 0.0,
            strength: 1.0,
            seed: 5,
        };
        let no_cross = LocalAdjustmentLayer::new(
            "engraving",
            LocalMask::Full,
            LocalEffect::Engraving(base_params),
        );
        let with_cross = LocalAdjustmentLayer::new(
            "engraving",
            LocalMask::Full,
            LocalEffect::Engraving(EngravingParams {
                crosshatch: 1.0,
                ..base_params
            }),
        );
        let without = apply_layers(src.as_ref(), &[no_cross]).unwrap();
        let with = apply_layers(src.as_ref(), &[with_cross]).unwrap();
        assert!(with.pixels[4] < without.pixels[4]);
    }

    #[test]
    fn engraving_keeps_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(1, 1, vec![12, 34, 56, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "engraving",
            LocalMask::Full,
            LocalEffect::Engraving(EngravingParams {
                strength: 1.0,
                ..EngravingParams::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn newspaper_print_turns_gray_into_ink_dots_and_preserves_alpha() {
        let src = solid(8, 8, [150, 150, 150, 209]);
        let layer = LocalAdjustmentLayer::new(
            "newspaper",
            LocalMask::Full,
            LocalEffect::NewspaperPrint(NewspaperPrintParams {
                cell_px: 4.0,
                dot_gain: 0.05,
                ink_bleed: 0.0,
                paper_age: 0.35,
                paper_texture: 0.0,
                contrast: 0.35,
                fade: 0.0,
                strength: 1.0,
                seed: 3,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_ne!(out.pixels, src.pixels);
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 209));
        assert!(out.pixels.chunks_exact(4).any(|px| px[0] < 80));
        assert!(out.pixels.chunks_exact(4).any(|px| px[0] > 200));
    }

    #[test]
    fn newspaper_print_ages_blank_paper_without_adding_white_ink() {
        let src = solid(4, 4, [255, 255, 255, 255]);
        let layer = LocalAdjustmentLayer::new(
            "newspaper",
            LocalMask::Full,
            LocalEffect::NewspaperPrint(NewspaperPrintParams {
                cell_px: 4.0,
                dot_gain: 0.0,
                ink_bleed: 0.0,
                paper_age: 1.0,
                paper_texture: 0.0,
                contrast: 0.0,
                fade: 0.0,
                strength: 1.0,
                seed: 4,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let px = &out.pixels[0..4];
        assert!(px[0] > px[1]);
        assert!(px[1] > px[2]);
        assert_eq!(px[3], 255);
    }

    #[test]
    fn newspaper_print_keeps_transparent_hidden_rgb() {
        let src = RgbaImageBuf::new(1, 1, vec![12, 34, 56, 0]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "newspaper",
            LocalMask::Full,
            LocalEffect::NewspaperPrint(NewspaperPrintParams {
                strength: 1.0,
                ..NewspaperPrintParams::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn textureizer_adds_paper_texture_and_preserves_alpha() {
        let src = solid(8, 8, [170, 160, 145, 207]);
        let layer = LocalAdjustmentLayer::new(
            "textureizer",
            LocalMask::Full,
            LocalEffect::Textureizer(TextureizerParams {
                mode: TextureizerMode::Paper,
                scale_px: 4.0,
                depth: 0.85,
                contrast: 1.3,
                warmth: 0.25,
                strength: 1.0,
                seed: 7,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_ne!(out.pixels, src.pixels);
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 207));
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
    fn diffraction_starburst_odd_blades_produce_double_rays() {
        assert_eq!(diffraction_ray_count(5), 10);
        assert_eq!(diffraction_ray_count(6), 6);
    }

    #[test]
    fn diffraction_starburst_extends_bright_pixel_and_keeps_alpha() {
        let src = RgbaImageBuf::new(
            5,
            1,
            vec![
                0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "diffraction",
            LocalMask::Full,
            LocalEffect::DiffractionStarburst(DiffractionStarburstParams {
                blade_count: 6,
                rotation_degrees: 0.0,
                threshold: 0.5,
                length_px: 4.0,
                width_px: 0.5,
                halo_radius_px: 0.0,
                chromatic_shift: 0.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[4] > src.pixels[4]);
        assert_eq!(out.pixels[3], 255);
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

    #[test]
    fn despeckle_removes_isolated_speckle_and_preserves_alpha() {
        let mut src = solid(5, 5, [100, 100, 100, 211]);
        let center = (2 * 5 + 2) * 4;
        src.pixels[center] = 255;
        src.pixels[center + 1] = 255;
        src.pixels[center + 2] = 255;
        let layer = LocalAdjustmentLayer::new(
            "despeckle",
            LocalMask::Full,
            LocalEffect::Despeckle(DespeckleParams {
                radius_px: 1.0,
                threshold: 30.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert!(out.pixels[center] < 140);
        assert!(out.pixels.chunks_exact(4).all(|px| px[3] == 211));
    }

    #[test]
    fn despeckle_preserves_regular_hard_edge() {
        let mut pixels = Vec::new();
        for _y in 0..3 {
            for x in 0..5 {
                let v = if x < 2 { 0 } else { 255 };
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let src = RgbaImageBuf::new(5, 3, pixels).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "despeckle",
            LocalMask::Full,
            LocalEffect::Despeckle(DespeckleParams {
                radius_px: 1.0,
                threshold: 20.0,
                strength: 1.0,
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn repair_solid_replaces_only_masked_rgb_even_if_mask_flags_are_disabled() {
        let src = RgbaImageBuf::new(
            3,
            1,
            vec![10, 20, 30, 255, 40, 50, 60, 211, 70, 80, 90, 255],
        )
        .unwrap();
        let mut layer = LocalAdjustmentLayer::new(
            "repair",
            LocalMask::Raster(RasterMask {
                width: 3,
                height: 1,
                alpha: vec![0.0, 1.0, 0.0],
            }),
            LocalEffect::Repair(RepairParams {
                mode: RepairMode::Solid,
                sampled_rgb: [200, 100, 50],
                ..Default::default()
            }),
        );
        layer.mask_before_effect = false;
        layer.mask_after_effect = false;
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        assert_eq!(&out.pixels[0..4], &src.pixels[0..4]);
        assert_eq!(&out.pixels[4..8], &[200, 100, 50, 211]);
        assert_eq!(&out.pixels[8..12], &src.pixels[8..12]);
    }

    #[test]
    fn repair_preserve_luminance_keeps_hsl_lightness() {
        let src = RgbaImageBuf::new(1, 1, vec![35, 115, 205, 255]).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "repair color",
            LocalMask::Full,
            LocalEffect::Repair(RepairParams {
                mode: RepairMode::PreserveLuminance,
                sampled_rgb: [225, 45, 60],
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let source_lightness =
            (35.0_f32.max(115.0).max(205.0) + 35.0_f32.min(115.0).min(205.0)) * 0.5;
        let output_lightness = (*out.pixels[0..3].iter().max().unwrap() as f32
            + *out.pixels[0..3].iter().min().unwrap() as f32)
            * 0.5;

        assert!((source_lightness - output_lightness).abs() <= 1.0);
        assert!(out.pixels[0] > out.pixels[2]);
        assert_eq!(out.pixels[3], 255);
    }

    #[test]
    fn repair_clone_uses_fixed_source_destination_offset() {
        let src = RgbaImageBuf::new(
            4,
            1,
            vec![
                220, 20, 20, 255, 20, 220, 20, 255, 20, 20, 220, 255, 240, 220, 20, 255,
            ],
        )
        .unwrap();
        let layer = LocalAdjustmentLayer::new(
            "clone",
            LocalMask::Raster(RasterMask {
                width: 4,
                height: 1,
                alpha: vec![0.0, 0.0, 0.0, 1.0],
            }),
            LocalEffect::Repair(RepairParams {
                mode: RepairMode::Clone,
                clone_source_uv: Some([0.0, 0.0]),
                clone_destination_uv: Some([1.0, 0.0]),
                texture_strength: 1.0,
                color_match_strength: 0.0,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();

        assert_eq!(&out.pixels[0..12], &src.pixels[0..12]);
        assert_eq!(&out.pixels[12..16], &[220, 20, 20, 255]);
    }

    #[test]
    fn surrounding_repair_copies_texture_without_touching_mask_exterior() {
        let (width, height) = (12, 8);
        let mut pixels = Vec::with_capacity(width * height * 4);
        let mut alpha = vec![0.0; width * height];
        for y in 0..height {
            for x in 0..width {
                let value = if (x + y) % 2 == 0 { 55 } else { 185 };
                let inside = (4..8).contains(&x) && (2..6).contains(&y);
                if inside {
                    pixels.extend_from_slice(&[255, 0, 255, 255]);
                    alpha[y * width + x] = 1.0;
                } else {
                    pixels.extend_from_slice(&[value, value + 10, value + 20, 255]);
                }
            }
        }
        let src = RgbaImageBuf::new(width, height, pixels).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "surrounding repair",
            LocalMask::Raster(RasterMask {
                width,
                height,
                alpha: alpha.clone(),
            }),
            LocalEffect::Repair(RepairParams {
                mode: RepairMode::Surrounding,
                search_radius_px: 16.0,
                texture_strength: 1.0,
                color_match_strength: 0.0,
                quality: RepairQuality::High,
                seed: 7,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer.clone()]).unwrap();
        let repeated = apply_layers(src.as_ref(), &[layer]).unwrap();
        let mut repaired_values = std::collections::BTreeSet::new();
        for (index, amount) in alpha.iter().copied().enumerate() {
            let pixel = index * 4;
            if amount == 0.0 {
                assert_eq!(&out.pixels[pixel..pixel + 4], &src.pixels[pixel..pixel + 4]);
            } else {
                assert_ne!(&out.pixels[pixel..pixel + 3], &[255, 0, 255]);
                repaired_values.insert(out.pixels[pixel]);
            }
        }
        assert!(repaired_values.len() >= 2);
        assert_eq!(out.pixels, repeated.pixels);
    }

    #[test]
    fn repair_patch_size_defaults_to_auto() {
        assert_eq!(RepairPatchSize::default(), RepairPatchSize::Auto);
        assert_eq!(RepairParams::default().patch_size, RepairPatchSize::Auto);
    }

    #[test]
    fn manual_repair_patch_sizes_select_larger_geometry() {
        assert_eq!(
            repair_patch_geometry(RepairQuality::High, RepairPatchSize::Auto),
            (10, 5, 4, 32)
        );
        assert_eq!(
            repair_patch_geometry(RepairQuality::High, RepairPatchSize::Standard),
            (24, 12, 6, 32)
        );
        assert_eq!(
            repair_patch_geometry(RepairQuality::High, RepairPatchSize::Large),
            (48, 24, 10, 32)
        );
    }

    #[test]
    fn surrounding_repair_does_not_add_coarse_tile_seams_to_smooth_texture() {
        let (width, height) = (64, 48);
        let mut pixels = Vec::with_capacity(width * height * 4);
        let mut alpha = vec![0.0; width * height];
        for y in 0..height {
            for x in 0..width {
                let inside = (20..44).contains(&x) && (14..34).contains(&y);
                if inside {
                    pixels.extend_from_slice(&[255, 0, 255, 255]);
                    alpha[y * width + x] = 1.0;
                } else {
                    let value = (50 + x * 2 + y).min(245) as u8;
                    pixels.extend_from_slice(&[value, value, value, 255]);
                }
            }
        }
        let src = RgbaImageBuf::new(width, height, pixels).unwrap();
        let layer = LocalAdjustmentLayer::new(
            "smooth surrounding repair",
            LocalMask::Raster(RasterMask {
                width,
                height,
                alpha: alpha.clone(),
            }),
            LocalEffect::Repair(RepairParams {
                mode: RepairMode::Surrounding,
                search_radius_px: 24.0,
                texture_strength: 1.0,
                color_match_strength: 0.0,
                quality: RepairQuality::Standard,
                seed: 11,
                ..Default::default()
            }),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        let mut max_inside_jump = 0_u8;
        for y in 14..34 {
            for x in 20..44 {
                let index = y * width + x;
                if x + 1 < 44 {
                    let right = index + 1;
                    max_inside_jump =
                        max_inside_jump.max(out.pixels[index * 4].abs_diff(out.pixels[right * 4]));
                }
                if y + 1 < 34 {
                    let below = index + width;
                    max_inside_jump =
                        max_inside_jump.max(out.pixels[index * 4].abs_diff(out.pixels[below * 4]));
                }
            }
        }

        assert!(
            max_inside_jump <= 12,
            "smooth repair introduced a coarse tile seam: {max_inside_jump}"
        );
    }

    #[test]
    fn surrounding_repair_feather_only_blends_the_generated_boundary() {
        let (width, height) = (40, 32);
        let mut pixels = Vec::with_capacity(width * height * 4);
        let mut alpha = vec![0.0; width * height];
        for y in 0..height {
            for x in 0..width {
                let inside = (12..28).contains(&x) && (8..24).contains(&y);
                if inside {
                    pixels.extend_from_slice(&[255, 0, 255, 255]);
                    alpha[y * width + x] = 1.0;
                } else {
                    let value = ((x * 17 + y * 29 + (x * y) % 31) % 180 + 35) as u8;
                    pixels.extend_from_slice(&[
                        value,
                        value.saturating_add(13),
                        value.saturating_add(27),
                        255,
                    ]);
                }
            }
        }
        let src = RgbaImageBuf::new(width, height, pixels).unwrap();
        let mut layer = LocalAdjustmentLayer::new(
            "feathered surrounding repair",
            LocalMask::Raster(RasterMask {
                width,
                height,
                alpha,
            }),
            LocalEffect::Repair(RepairParams {
                mode: RepairMode::Surrounding,
                search_radius_px: 24.0,
                texture_strength: 1.0,
                color_match_strength: 0.0,
                quality: RepairQuality::Standard,
                seed: 19,
                ..Default::default()
            }),
        );
        let hard = apply_layers(src.as_ref(), &[layer.clone()]).unwrap();
        layer.mask_feather_px = 4.0;
        let feathered = apply_layers(src.as_ref(), &[layer]).unwrap();

        // ぼかし半径より内側では、探索元と生成テクスチャが同一である。
        for y in 12..20 {
            for x in 16..24 {
                let pixel = (y * width + x) * 4;
                assert_eq!(
                    &feathered.pixels[pixel..pixel + 4],
                    &hard.pixels[pixel..pixel + 4]
                );
            }
        }
        // 境界では同じ生成結果を元画像へ alpha 合成してなじませる。
        let boundary = (16 * width + 12) * 4;
        assert_ne!(
            &feathered.pixels[boundary..boundary + 3],
            &hard.pixels[boundary..boundary + 3]
        );
    }

    #[test]
    fn surrounding_repair_with_no_external_source_is_identity() {
        let src = solid(3, 3, [80, 100, 120, 255]);
        let layer = LocalAdjustmentLayer::new(
            "full repair",
            LocalMask::Full,
            LocalEffect::Repair(RepairParams::default()),
        );
        let out = apply_layers(src.as_ref(), &[layer]).unwrap();
        assert_eq!(out.pixels, src.pixels);
    }
}
