//! Conservative jacket detection. Position comes from binding order and panel
//! structure; visual richness can reject a position, never choose another one.
use crate::cover_auto_material::AutoCropResults;
use crate::folder_thumb_pins::CoverCrop;
use crate::rotation_db::Rotation;
use crate::settings::ReadingDirection;
use image::{DynamicImage, RgbImage};
use std::borrow::Cow;

mod layout;
mod scope;
pub(crate) use scope::CoverAutoCropScope;
pub(crate) mod photo;

const CONTENT_MARGIN_CAP: f64 = 0.05;
const TWO_SPINE_MIN: f64 = 0.04;
const TWO_SPINE_MAX: f64 = 0.16;
const TWO_SHAPE_MIN: f64 = 1.25;
const TWO_SHAPE_MAX: f64 = 1.8;
const TWO_FRONT_MIN: f64 = 0.95;
const TWO_FRONT_MAX: f64 = 1.15;
const TWO_BAND_FRACTION: f64 = 0.08;
const TWO_BAND_MIN: usize = 3;
const TWO_BOUNDARY_MIN: f64 = 40.0;
const TWO_STD_WEIGHT: f64 = 0.5;
const TWO_EDGE_WEIGHT: f64 = 0.3;
const TWO_OUTER_MIN: f64 = 0.2;
const TWO_OUTER_MAX: f64 = 0.8;
const TWO_PAPER_INK_MAX: f64 = 0.35;
const TWO_INK_GAP_MIN: f64 = 0.2;
const FRONT_SCORE_MIN: f64 = 0.6;
const RICHER_SCORE_GAP: f64 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AutoCropIdentity {
    pub binding: ReadingDirection,
    pub folders: bool,
}

impl AutoCropIdentity {
    pub(crate) fn new(binding: ReadingDirection) -> Self {
        Self {
            binding,
            folders: false,
        }
    }

    pub(crate) fn suffix(self) -> &'static str {
        self.suffix_for_key("")
    }

    pub(crate) fn suffix_for_key(self, key: &str) -> &'static str {
        match (
            self.folders && key.starts_with(crate::thumb_loader::CACHE_KEY_FOLDER),
            self.binding,
        ) {
            (true, ReadingDirection::Rtl) => "v4f:rtl",
            (true, ReadingDirection::Ltr) => "v4f:ltr",
            (false, ReadingDirection::Rtl) => "v4:rtl",
            (false, ReadingDirection::Ltr) => "v4:ltr",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutoCropDecline {
    RawAspectGate,
    PageAspectMissing,
    PhotoLike {
        reason: photo::PhotoLikeReason,
    },
    NotWide,
    NoJacketEvidence,
    NoStructure,
    NoLocalEvidence,
    NoSpine,
    WeakFront,
    RicherElsewhere,
    TwoPanel {
        v5: TwoPanelTrigger,
        reason: TwoPanelDecline,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TwoPanelTrigger {
    NoJacketEvidence,
    NoLocalEvidence,
    NoSpine,
    WeakFront,
}

impl TwoPanelTrigger {
    fn decline(self) -> AutoCropDecline {
        match self {
            Self::NoJacketEvidence => AutoCropDecline::NoJacketEvidence,
            Self::NoLocalEvidence => AutoCropDecline::NoLocalEvidence,
            Self::NoSpine => AutoCropDecline::NoSpine,
            Self::WeakFront => AutoCropDecline::WeakFront,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TwoPanelDecline {
    Shape,
    NoRange,
    WeakBoundary,
    OuterWide,
    NotPaper,
    WeakFront,
    RicherElsewhere,
}

impl TwoPanelDecline {
    fn as_str(self) -> &'static str {
        match self {
            Self::Shape => "u-out",
            Self::NoRange => "no-range",
            Self::WeakBoundary => "two-weak",
            Self::OuterWide => "outer-wide",
            Self::NotPaper => "flap-not-paper",
            Self::WeakFront => "two-weakfront",
            Self::RicherElsewhere => "two-richer",
        }
    }
}

impl AutoCropDecline {
    pub(crate) fn as_str(self) -> Cow<'static, str> {
        let label = match self {
            Self::RawAspectGate => "not-wide-raw",
            Self::PageAspectMissing => "no-page-aspect",
            Self::PhotoLike { reason } => return format!("photo-like/{}", reason.as_str()).into(),
            Self::NotWide => "not-wide",
            Self::NoJacketEvidence => "no-jacket-evidence",
            Self::NoStructure => "no-structure",
            Self::NoLocalEvidence => "no-local-evidence",
            Self::NoSpine => "no-spine",
            Self::WeakFront => "weak",
            Self::RicherElsewhere => "richer-elsewhere",
            Self::TwoPanel { v5, reason } => {
                return format!("{}/{}", v5.decline().as_str(), reason.as_str()).into();
            }
        };
        label.into()
    }

    fn two_panel_trigger(self) -> Option<TwoPanelTrigger> {
        match self {
            Self::NoJacketEvidence => Some(TwoPanelTrigger::NoJacketEvidence),
            Self::NoLocalEvidence => Some(TwoPanelTrigger::NoLocalEvidence),
            Self::NoSpine => Some(TwoPanelTrigger::NoSpine),
            Self::WeakFront => Some(TwoPanelTrigger::WeakFront),
            _ => None,
        }
    }

    #[cfg(test)]
    fn v5_reason(self) -> Self {
        match self {
            Self::TwoPanel { v5, .. } => v5.decline(),
            other => other,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutoCropLayout {
    T1,
    T2,
    T3,
    T4,
}

impl AutoCropLayout {
    fn from_index(index: usize) -> Self {
        match index {
            0 => Self::T1,
            1 => Self::T2,
            2 => Self::T3,
            3 => Self::T4,
            _ => unreachable!("layout prediction has exactly four types"),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::T1 => "T1",
            Self::T2 => "T2",
            Self::T3 => "T3",
            Self::T4 => "T4",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutoCropMethod {
    Current,
    LayoutFallback(AutoCropLayout),
    LayoutReplaced(AutoCropLayout),
}

impl AutoCropMethod {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::LayoutFallback(_) => "layout-fallback",
            Self::LayoutReplaced(_) => "layout-replaced",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AutoCropSuccess {
    /// Final crop in the EXIF-upright source coordinates, before saved rotation.
    pub crop: CoverCrop,
    pub method: AutoCropMethod,
}

impl AutoCropSuccess {
    pub(crate) fn layout(self) -> Option<AutoCropLayout> {
        match self.method {
            AutoCropMethod::Current => None,
            AutoCropMethod::LayoutFallback(layout) | AutoCropMethod::LayoutReplaced(layout) => {
                Some(layout)
            }
        }
    }

    pub(crate) fn as_str(self) -> Cow<'static, str> {
        match self.layout() {
            None => "crop".into(),
            Some(layout) => format!("{}/{}", self.method.as_str(), layout.as_str()).into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(crop: CoverCrop, method: AutoCropMethod) -> Self {
        Self { crop, method }
    }
}

/// Analyze all saved rotations once. Each result belongs to the unrotated,
/// EXIF-upright source, so changing the saved angle never invalidates material.
/// The caller supplies the shared fixed-size analysis decode, not a display
/// thumbnail. In particular, rotate before reducing to the 256-row raster.
pub(crate) fn detect_cover_crops(
    image: &DynamicImage,
    original_dims: (u32, u32),
    page_aspect: Option<f64>,
    binding: ReadingDirection,
) -> AutoCropResults {
    detect_cover_crops_with_report(image, original_dims, page_aspect, binding, |_, _| {})
}

pub(crate) fn detect_cover_crops_with_report(
    image: &DynamicImage,
    original_dims: (u32, u32),
    page_aspect: Option<f64>,
    binding: ReadingDirection,
    report: impl FnMut(Rotation, Result<CoverCrop, AutoCropDecline>),
) -> AutoCropResults {
    detect_cover_crops_with_photo(image, original_dims, page_aspect, binding, false, report)
}

/// Worker, range dialog, and evaluation share the same metadata gates and all
/// four saved-angle decisions. Shooting metadata comes from the existing decode.
pub(crate) fn detect_cover_crops_with_photo_report(
    image: &DynamicImage,
    original_dims: (u32, u32),
    page_aspect: Option<f64>,
    shot_exif: bool,
    binding: ReadingDirection,
    report: impl FnMut(Rotation, Result<CoverCrop, AutoCropDecline>),
) -> AutoCropResults {
    detect_cover_crops_with_photo(
        image,
        original_dims,
        page_aspect,
        binding,
        shot_exif,
        report,
    )
}

fn detect_cover_crops_with_photo(
    image: &DynamicImage,
    original_dims: (u32, u32),
    page_aspect: Option<f64>,
    binding: ReadingDirection,
    shot_exif: bool,
    mut report: impl FnMut(Rotation, Result<CoverCrop, AutoCropDecline>),
) -> AutoCropResults {
    detect_cover_crops_with_photo_detailed_report(
        image,
        original_dims,
        page_aspect,
        shot_exif,
        binding,
        |rotation, result| report(rotation, result.map(|success| success.crop)),
    )
}

/// All production entry points share this decision and its typed explanation.
pub(crate) fn detect_cover_crops_with_photo_detailed_report(
    image: &DynamicImage,
    original_dims: (u32, u32),
    page_aspect: Option<f64>,
    shot_exif: bool,
    binding: ReadingDirection,
    mut report: impl FnMut(Rotation, Result<AutoCropSuccess, AutoCropDecline>),
) -> AutoCropResults {
    AutoCropResults::new(
        [
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ]
        .map(|rotation| {
            let result = detect_cover_crop_at_rotation_with_photo(
                image,
                original_dims,
                page_aspect,
                binding,
                rotation,
                shot_exif,
            );
            report(rotation, result);
            result.ok().map(|success| success.crop)
        }),
    )
}

#[cfg(test)]
fn detect_cover_crop_at_rotation(
    image: &DynamicImage,
    original_dims: (u32, u32),
    page_aspect: Option<f64>,
    binding: ReadingDirection,
    rotation: Rotation,
) -> Result<CoverCrop, AutoCropDecline> {
    detect_cover_crop_at_rotation_with_photo(
        image,
        original_dims,
        page_aspect,
        binding,
        rotation,
        false,
    )
    .map(|success| success.crop)
}

fn detect_cover_crop_at_rotation_with_photo(
    image: &DynamicImage,
    original_dims: (u32, u32),
    page_aspect: Option<f64>,
    binding: ReadingDirection,
    rotation: Rotation,
    shot_exif: bool,
) -> Result<AutoCropSuccess, AutoCropDecline> {
    let dimensions = match rotation {
        Rotation::Cw90 | Rotation::Cw270 => (original_dims.1, original_dims.0),
        Rotation::None | Rotation::Cw180 => original_dims,
    };
    // Reject from dimensions before allocating a rotated copy or analysis raster.
    raw_aspect_gate(dimensions)?;
    // Preserve the dialog's pre-analysis page-aspect stop before photo reasons.
    page_aspect
        .filter(|aspect| aspect.is_finite() && *aspect > 0.0)
        .ok_or(AutoCropDecline::PageAspectMissing)?;
    if let Some(reason) = photo::photo_like_reason(shot_exif) {
        return Err(AutoCropDecline::PhotoLike { reason });
    }
    let rotated;
    let image = match rotation {
        Rotation::None => image,
        Rotation::Cw90 => {
            rotated = image.rotate90();
            &rotated
        }
        Rotation::Cw180 => {
            rotated = image.rotate180();
            &rotated
        }
        Rotation::Cw270 => {
            rotated = image.rotate270();
            &rotated
        }
    };
    let raster = analysis_raster(image);
    let mut success = production_crop(&raster, dimensions, page_aspect, binding)?;
    success.crop = inverse_rotated_crop(success.crop, rotation);
    Ok(success)
}

fn inverse_rotated_crop(crop: CoverCrop, rotation: Rotation) -> CoverCrop {
    let [x0, y0, x1, y1] = crop.values();
    let inverse = match rotation {
        Rotation::None => [x0, y0, x1, y1],
        Rotation::Cw90 => [y0, 1_000_000 - x1, y1, 1_000_000 - x0],
        Rotation::Cw180 => [
            1_000_000 - x1,
            1_000_000 - y1,
            1_000_000 - x0,
            1_000_000 - y0,
        ],
        Rotation::Cw270 => [1_000_000 - y1, x0, 1_000_000 - y0, x1],
    };
    // Quarter turns preserve CoverCrop's bounds, minimum edges and non-full
    // invariant. Failure here is a programming error, never a detector decline.
    CoverCrop::new(inverse)
        .expect("inverse rotation preserves valid crop bounds")
        .expect("inverse rotation preserves a non-full crop")
}

/// Use one deterministic analysis height, including when the decoder supplied
/// a smaller thumbnail. EXIF orientation is applied by the caller.
pub(crate) fn analysis_raster(image: &DynamicImage) -> RgbImage {
    let rgb = image.to_rgb8();
    if rgb.height() == 0 || rgb.width() == 0 {
        return rgb;
    }
    let width = (f64::from(rgb.width()) * 256.0 / f64::from(rgb.height()))
        .round_ties_even()
        .max(1.0) as u32;
    // A portrait panel (including the 10% edge tolerance) cannot occupy the
    // minimum 1% crop width beyond 100:1. Avoid magnifying scan slivers into an
    // unbounded analysis allocation; the detector declines the empty raster.
    if width > 25_600 {
        return RgbImage::new(0, 0);
    }
    analysis_area_average(&rgb, width)
}

/// Integrate each output pixel's source area without an additional smoothing
/// footprint. In particular, a native 256-row decode must retain its ink,
/// luminance variance and colorfulness: smoothing alternating rows can turn
/// printed art into false flat-panel evidence. Both axes use the same support.
fn analysis_area_average(source: &RgbImage, width: u32) -> RgbImage {
    let taps = |source_size: u32, output_size: u32| {
        let scale = f64::from(source_size) / f64::from(output_size);
        (0..output_size)
            .map(|position| {
                let center = (f64::from(position) + 0.5) * scale;
                let radius = scale * 0.5;
                let (left, right) = (center - radius, center + radius);
                (left.floor() as i64..right.ceil() as i64)
                    .filter_map(|pixel| {
                        let overlap =
                            (right.min((pixel + 1) as f64) - left.max(pixel as f64)).max(0.0);
                        (overlap > 0.0).then_some((
                            pixel.clamp(0, i64::from(source_size) - 1) as usize,
                            overlap / (right - left),
                        ))
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    let columns = taps(source.width(), width);
    let rows = taps(source.height(), 256);
    let source_width = source.width() as usize;
    // Only one accumulated source row is retained; allocating an intermediate
    // source-height raster would multiply memory for a tall full-size decode.
    let mut row = vec![[0.0; 3]; source_width];
    let mut output = RgbImage::new(width, 256);
    for (y, samples) in rows.iter().enumerate() {
        row.fill([0.0; 3]);
        for &(source_y, weight) in samples {
            let start = source_y * source_width * 3;
            let pixels = &source.as_raw()[start..start + source_width * 3];
            for (sum, pixel) in row.iter_mut().zip(pixels.chunks_exact(3)) {
                for channel in 0..3 {
                    sum[channel] += f64::from(pixel[channel]) * weight;
                }
            }
        }
        for (x, samples) in columns.iter().enumerate() {
            let mut sum = [0.0; 3];
            for &(source_x, weight) in samples {
                for channel in 0..3 {
                    sum[channel] += row[source_x][channel] * weight;
                }
            }
            output.put_pixel(
                x as u32,
                y as u32,
                image::Rgb(sum.map(|value| value.round().clamp(0.0, 255.0) as u8)),
            );
        }
    }
    output
}

type Pixel = [f64; 3];

struct Raster {
    width: usize,
    height: usize,
    pixels: Vec<Pixel>,
}

impl Raster {
    fn get(&self, x: usize, y: usize) -> Pixel {
        self.pixels[y * self.width + x]
    }

    fn from_image(image: &RgbImage) -> Self {
        Self {
            width: image.width() as usize,
            height: image.height() as usize,
            pixels: image.pixels().map(|p| p.0.map(f64::from)).collect(),
        }
    }

    fn region(&self, bounds: [usize; 4], reverse: bool) -> Self {
        let [left, top, right, bottom] = bounds;
        let mut pixels = Vec::with_capacity((right - left) * (bottom - top));
        for y in top..bottom {
            for x in left..right {
                pixels.push(self.get(if reverse { right - 1 - (x - left) } else { x }, y));
            }
        }
        Self {
            width: right - left,
            height: bottom - top,
            pixels,
        }
    }
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_unstable_by(f64::total_cmp);
    let n = values.len();
    (values[(n - 1) / 2] + values[n / 2]) * 0.5
}

fn color_median(pixels: impl Iterator<Item = Pixel>) -> Pixel {
    let mut channels = [Vec::new(), Vec::new(), Vec::new()];
    for p in pixels {
        for c in 0..3 {
            channels[c].push(p[c]);
        }
    }
    channels.map(|mut c| median(&mut c))
}

fn distance_squared(a: Pixel, b: Pixel) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}

fn luminance(p: Pixel) -> f64 {
    p[0] * 0.299 + p[1] * 0.587 + p[2] * 0.114
}

/// Each edge has its own reference color and 5% cap; a few dust pixels do not
/// terminate an otherwise blank margin.
fn content_bounds(raster: &Raster, sample: impl Fn(usize, usize) -> Pixel) -> [usize; 4] {
    let walk = |vertical: bool, reverse: bool| {
        let (depth, length) = if vertical {
            (raster.height, raster.width)
        } else {
            (raster.width, raster.height)
        };
        let pixel = |d, i| {
            let d = if reverse { depth - 1 - d } else { d };
            if vertical { sample(i, d) } else { sample(d, i) }
        };
        let reference = color_median((0..length).map(|i| pixel(0, i)));
        (0..(depth as f64 * CONTENT_MARGIN_CAP) as usize)
            .take_while(|&d| {
                let close = (0..length)
                    .filter(|&i| distance_squared(pixel(d, i), reference) <= 40.0 * 40.0)
                    .count();
                close as f64 / length as f64 >= 0.97
            })
            .count()
    };
    [
        walk(false, false),
        walk(true, false),
        raster.width - walk(false, true),
        raster.height - walk(true, true),
    ]
}

#[derive(Default, Clone)]
struct Column {
    rg: f64,
    rg2: f64,
    yb: f64,
    yb2: f64,
    ink: f64,
    lum: f64,
    lum2: f64,
}

struct Features {
    columns: Vec<Column>,
    height: usize,
}

impl Features {
    fn new(raster: &Raster) -> Self {
        let mut lum: Vec<_> = raster.pixels.iter().copied().map(luminance).collect();
        lum.sort_unstable_by(f64::total_cmp);
        let pos = (lum.len() - 1) as f64 * 0.95;
        let threshold = lum[pos.floor() as usize] * (1.0 - pos.fract())
            + lum[pos.ceil() as usize] * pos.fract();
        let paper = color_median(
            raster
                .pixels
                .iter()
                .copied()
                .filter(|&p| luminance(p) >= threshold),
        );
        let mut columns = vec![Column::default(); raster.width];
        for (i, &p) in raster.pixels.iter().enumerate() {
            let column = &mut columns[i % raster.width];
            let rg = p[0] - p[1];
            let yb = (p[0] + p[1]) * 0.5 - p[2];
            let lum = luminance(p);
            column.rg += rg;
            column.rg2 += rg * rg;
            column.yb += yb;
            column.yb2 += yb * yb;
            column.ink += if distance_squared(p, paper) > 45.0 * 45.0 {
                1.0
            } else {
                0.0
            };
            column.lum += lum;
            column.lum2 += lum * lum;
        }
        for c in &mut columns {
            c.ink /= raster.height as f64;
        }
        Self {
            columns,
            height: raster.height,
        }
    }

    fn ink(&self, start: usize, end: usize) -> f64 {
        self.columns[start..end].iter().map(|c| c.ink).sum::<f64>() / (end - start) as f64
    }

    fn flat(&self, start: usize, end: usize) -> bool {
        let n = ((end - start) * self.height) as f64;
        let lum = self.columns[start..end].iter().map(|c| c.lum).sum::<f64>() / n;
        let lum2 = self.columns[start..end].iter().map(|c| c.lum2).sum::<f64>() / n;
        self.ink(start, end) < 0.35 || (lum2 - lum * lum).max(0.0).sqrt() < 14.0
    }

    fn score(&self, start: usize, end: usize) -> f64 {
        let columns = &self.columns[start..end];
        let n = ((end - start) * self.height) as f64;
        let rg = columns.iter().map(|c| c.rg).sum::<f64>() / n;
        let yb = columns.iter().map(|c| c.yb).sum::<f64>() / n;
        let vrg = (columns.iter().map(|c| c.rg2).sum::<f64>() / n - rg * rg).max(0.0);
        let vyb = (columns.iter().map(|c| c.yb2).sum::<f64>() / n - yb * yb).max(0.0);
        let richness = (vrg + vyb).sqrt() + 0.3 * rg.hypot(yb);
        let blank = columns.iter().filter(|c| c.ink < 0.15).count() as f64 / columns.len() as f64;
        richness / 60.0 + 0.6 * self.ink(start, end) - 0.8 * blank
    }
}

fn panel_edges(raster: &Raster) -> Vec<usize> {
    let mut support = vec![0usize; raster.width.saturating_sub(1)];
    for y in 0..raster.height {
        let differences: Vec<_> = (1..raster.width)
            .map(|x| {
                raster
                    .get(x - 1, y)
                    .into_iter()
                    .zip(raster.get(x, y))
                    .map(|(a, b)| (a - b).abs())
                    .sum::<f64>()
            })
            .collect();
        for (i, support) in support.iter_mut().enumerate() {
            if differences[i.saturating_sub(3)..(i + 4).min(differences.len())]
                .iter()
                .any(|&d| d > 45.0)
            {
                *support += 1;
            }
        }
    }
    let mut candidates: Vec<_> = support
        .into_iter()
        .enumerate()
        .filter(|&(_, n)| n as f64 / raster.height as f64 >= 0.9)
        .map(|(i, n)| (n, i + 1))
        .collect();
    // Match the prototype's deterministic stronger-first, rightmost-first tie.
    candidates.sort_unstable_by(|a, b| b.cmp(a));
    let gap = 2.max(raster.width / 50);
    let mut picked = Vec::new();
    for (_, x) in candidates {
        if picked.iter().all(|&other| x.abs_diff(other) >= gap) {
            picked.push(x);
        }
    }
    picked.extend([0, raster.width]);
    picked.sort_unstable();
    picked.dedup();
    picked
}

fn raw_aspect_gate(original_dims: (u32, u32)) -> Result<(), AutoCropDecline> {
    if original_dims.1 == 0 || f64::from(original_dims.0) / f64::from(original_dims.1) < 0.85 {
        Err(AutoCropDecline::RawAspectGate)
    } else {
        Ok(())
    }
}

/// The existing structural detector remains the first choice. Layout changes
/// only structural declines and the specifically approved conflicting T2 case.
fn production_crop(
    image: &RgbImage,
    original_dims: (u32, u32),
    page_aspect: Option<f64>,
    binding: ReadingDirection,
) -> Result<AutoCropSuccess, AutoCropDecline> {
    let current = cover_auto_crop(image, original_dims, page_aspect, binding);
    apply_layout_policy(image, original_dims, page_aspect, binding, current)
}

fn structural_decline(reason: AutoCropDecline) -> bool {
    matches!(
        reason,
        AutoCropDecline::NoJacketEvidence
            | AutoCropDecline::NoStructure
            | AutoCropDecline::NoLocalEvidence
            | AutoCropDecline::NoSpine
            | AutoCropDecline::WeakFront
            | AutoCropDecline::RicherElsewhere
            | AutoCropDecline::TwoPanel { .. }
    )
}

fn horizontal_iou(first: [f64; 2], second: [f64; 2]) -> f64 {
    let overlap = (first[1].min(second[1]) - first[0].max(second[0])).max(0.0);
    overlap / (first[1] - first[0] + second[1] - second[0] - overlap)
}

fn full_height(crop: CoverCrop) -> CoverCrop {
    let [x0, _, x1, _] = crop.values();
    CoverCrop::new([x0, 0, x1, 1_000_000])
        .expect("full height preserves valid horizontal crop bounds")
        .expect("an automatic portrait panel does not span the source width")
}

fn apply_layout_policy(
    image: &RgbImage,
    original_dims: (u32, u32),
    page_aspect: Option<f64>,
    binding: ReadingDirection,
    current: Result<CoverCrop, AutoCropDecline>,
) -> Result<AutoCropSuccess, AutoCropDecline> {
    if let Err(reason) = current {
        if !structural_decline(reason) {
            return Err(reason);
        }
    }
    let aspect = page_aspect
        .filter(|aspect| aspect.is_finite() && *aspect > 0.0)
        .ok_or(AutoCropDecline::PageAspectMissing)?;
    let width = f64::from(original_dims.0);
    let prediction = layout::predict(width, aspect * f64::from(original_dims.1), binding);
    let kind = prediction.map(|(kind, _, _)| AutoCropLayout::from_index(kind));
    let previous = current.map(full_height);
    let method = match previous {
        Ok(crop) => {
            let [x0, _, x1, _] = crop.values();
            let current_x = [x0, x1].map(|x| f64::from(x) * width / 1_000_000.0);
            let replace = prediction.is_some_and(|(kind, _, predicted)| {
                kind == 1 && horizontal_iou(current_x, predicted) < 0.6
            });
            if !replace {
                return Ok(AutoCropSuccess {
                    crop,
                    method: AutoCropMethod::Current,
                });
            }
            AutoCropMethod::LayoutReplaced(kind.expect("T2 prediction has a layout type"))
        }
        Err(reason) => {
            if prediction.is_none() {
                return Err(reason);
            }
            AutoCropMethod::LayoutFallback(kind.expect("fallback prediction has a layout type"))
        }
    };
    let (crop, _) = layout::crop(image, original_dims, aspect, binding, true)
        .expect("an eligible layout prediction always produces a valid portrait crop");
    Ok(AutoCropSuccess { crop, method })
}

/// Original structural candidate calculation. Production applies full height
/// and the shared layout policy once, at the saved-angle ownership boundary.
pub(crate) fn cover_auto_crop(
    image: &RgbImage,
    original_dims: (u32, u32),
    page_aspect: Option<f64>,
    binding: ReadingDirection,
) -> Result<CoverCrop, AutoCropDecline> {
    use AutoCropDecline::*;
    raw_aspect_gate(original_dims)?;
    let aspect = page_aspect
        .filter(|r| r.is_finite() && *r > 0.0)
        .ok_or(PageAspectMissing)?;
    if image.width() == 0 || image.height() == 0 {
        return Err(NoStructure);
    }
    let source = Raster::from_image(image);
    let bounds = content_bounds(&source, |x, y| source.get(x, y));
    let sub = source.region(bounds, binding == ReadingDirection::Ltr);
    if sub.width as f64 / (sub.height as f64) < aspect * 1.25 {
        return Err(NotWide);
    }
    let width = aspect * sub.height as f64;
    let features = Features::new(&sub);
    let edges = panel_edges(&sub);
    let (mut start, mut end) = match v5_candidate(&sub, width, &features, &edges) {
        Ok(candidate) => candidate,
        Err(reason) => {
            let Some(v5) = reason.two_panel_trigger() else {
                return Err(reason);
            };
            two_panel_candidate(&sub, width, &features, &edges)
                .map_err(|reason| AutoCropDecline::TwoPanel { v5, reason })?
        }
    };
    if binding == ReadingDirection::Ltr {
        (start, end) = (sub.width - end, sub.width - start);
    }
    let normalized =
        |value: usize, extent: usize| (value as f64 * 1_000_000.0 / extent as f64).round() as u32;
    CoverCrop::new([
        normalized(bounds[0] + start, source.width),
        normalized(bounds[1], source.height),
        normalized(bounds[0] + end, source.width),
        normalized(bounds[3], source.height),
    ])
    .ok()
    .flatten()
    .ok_or(NoStructure)
}

fn v5_candidate(
    sub: &Raster,
    width: f64,
    features: &Features,
    edges: &[usize],
) -> Result<(usize, usize), AutoCropDecline> {
    use AutoCropDecline::*;
    let panels: Vec<_> = edges.windows(2).map(|p| (p[0], p[1])).collect();
    let spines: Vec<_> = panels
        .iter()
        .copied()
        .filter(|&(a, b)| {
            let size = (b - a) as f64;
            size >= 0.04 * width && size <= 0.16 * width && features.ink(a, b) >= 0.6
        })
        .collect();
    if spines.is_empty()
        && !panels
            .iter()
            .any(|&(a, b)| (b - a) as f64 >= 0.15 * width && features.flat(a, b))
    {
        return Err(NoJacketEvidence);
    }
    let mut start = 0;
    let mut leading = panels.iter().copied();
    let first = leading.find(|&(a, b)| {
        if ((b - a) as f64) < 0.04 * width {
            start = b;
            false
        } else {
            true
        }
    });
    let flap = first.is_some_and(|(a, b)| {
        let size = (b - a) as f64;
        size >= 0.2 * width && size <= 0.8 * width && features.ink(a, b) >= 0.03
    });
    if flap {
        start = first.unwrap().1;
    }
    let target = start as f64 + width;
    let detected_end = edges
        .iter()
        .copied()
        .filter(|&e| (e as f64 - target).abs() <= 0.10 * width)
        .min_by(|a, b| ((*a as f64 - target).abs()).total_cmp(&(*b as f64 - target).abs()));
    let end = detected_end.unwrap_or_else(|| target.round_ties_even() as usize);
    if end > sub.width || end <= start {
        return Err(NoStructure);
    }
    if !flap && detected_end.is_none() {
        return Err(NoLocalEvidence);
    }
    let near = 2.max((0.02 * width) as usize);
    if !flap && !spines.iter().any(|&(a, _)| a.abs_diff(end) <= near) {
        return Err(NoSpine);
    }
    candidate_veto(features, start, end)?;
    Ok((start, end))
}

fn candidate_veto(features: &Features, start: usize, end: usize) -> Result<(), AutoCropDecline> {
    use AutoCropDecline::*;
    let score = features.score(start, end);
    if score < FRONT_SCORE_MIN {
        return Err(WeakFront);
    }
    let span = end - start;
    for x in (0..=features.columns.len() - span).step_by(1.max(span / 20)) {
        // Signed overlap is intentional: disjoint intervals must qualify.
        let overlap = (x + span).min(end) as i64 - x.max(start) as i64;
        if (overlap as f64) < 0.5 * span as f64
            && features.score(x, x + span) - score >= RICHER_SCORE_GAP
        {
            return Err(RicherElsewhere);
        }
    }
    Ok(())
}

/// Per-channel means, and population standard deviation over all RGB values
/// together (the prototype's array.std(), not luminance / per-channel std).
fn rgb_band_stats(raster: &Raster, start: usize, end: usize) -> (Pixel, f64) {
    let mut sum = [0.0; 3];
    let mut squared = 0.0;
    for y in 0..raster.height {
        for x in start..end {
            for (channel, value) in raster.get(x, y).into_iter().enumerate() {
                sum[channel] += value;
                squared += value * value;
            }
        }
    }
    let count = ((end - start) * raster.height) as f64;
    let mean = sum.map(|value| value / count);
    let scalar_mean = mean.iter().sum::<f64>() / 3.0;
    (
        mean,
        (squared / (count * 3.0) - scalar_mean * scalar_mean)
            .max(0.0)
            .sqrt(),
    )
}

fn two_panel_candidate(
    sub: &Raster,
    width: f64,
    features: &Features,
    edges: &[usize],
) -> Result<(usize, usize), TwoPanelDecline> {
    use TwoPanelDecline::*;
    let end = edges
        .iter()
        .copied()
        .filter(|&edge| {
            edge as f64 >= sub.width as f64 - TWO_SPINE_MAX * width
                && edge as f64 <= sub.width as f64 - TWO_SPINE_MIN * width
        })
        .max()
        .unwrap_or(sub.width);
    if !(TWO_SHAPE_MIN..=TWO_SHAPE_MAX).contains(&(end as f64 / width)) {
        return Err(Shape);
    }
    // Python int truncates these positive bounds; retain its inclusive search
    // and leftmost-first tie, then check the final band's exact width below.
    let lo = ((end as f64 - TWO_FRONT_MAX * width) as usize).max((TWO_OUTER_MIN * width) as usize);
    let hi = ((end as f64 - TWO_FRONT_MIN * width) as usize).min(sub.width - 1);
    if lo >= hi {
        return Err(NoRange);
    }
    let band = TWO_BAND_MIN.max((TWO_BAND_FRACTION * width) as usize);
    let mut best = None;
    for x in lo..=hi {
        // A zero-width left band has NaN statistics in the prototype and never
        // wins. Avoid evaluating it rather than dividing by zero.
        if x == 0 {
            continue;
        }
        let (left, left_std) = rgb_band_stats(sub, x.saturating_sub(band), x);
        let (right, right_std) = rgb_band_stats(sub, x, (x + band).min(sub.width));
        let edge = (0..sub.height)
            .map(|y| {
                sub.get(x - 1, y)
                    .into_iter()
                    .zip(sub.get(x, y))
                    .map(|(a, b)| (a - b).abs())
                    .sum::<f64>()
            })
            .sum::<f64>()
            / sub.height as f64;
        let score = distance_squared(left, right).sqrt()
            + TWO_STD_WEIGHT * (left_std - right_std).abs()
            + TWO_EDGE_WEIGHT * edge;
        if best.is_none_or(|(_, previous)| score > previous) {
            best = Some((x, score));
        }
    }
    let Some((start, _)) = best.filter(|&(_, score)| score >= TWO_BOUNDARY_MIN) else {
        return Err(WeakBoundary);
    };
    if (start as f64) < TWO_OUTER_MIN * width || start as f64 > TWO_OUTER_MAX * width {
        return Err(OuterWide);
    }
    let outer_ink = features.ink(0, start);
    let front_ink = features.ink(start, end);
    if outer_ink > TWO_PAPER_INK_MAX || front_ink - outer_ink < TWO_INK_GAP_MIN {
        return Err(NotPaper);
    }
    candidate_veto(features, start, end).map_err(|reason| match reason {
        AutoCropDecline::WeakFront => WeakFront,
        AutoCropDecline::RicherElsewhere => RicherElsewhere,
        _ => unreachable!("common candidate veto has only two rejection reasons"),
    })?;
    Ok((start, end))
}

#[cfg(test)]
mod evaluation;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod two_panel_tests;
