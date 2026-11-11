//! Conservative jacket detection. Position comes from binding order and panel
//! structure; visual richness can reject a position, never choose another one.
use crate::folder_thumb_pins::CoverCrop;
use crate::settings::ReadingDirection;
use image::{DynamicImage, RgbImage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AutoCropIdentity {
    pub binding: ReadingDirection,
}

impl AutoCropIdentity {
    pub(crate) fn new(binding: ReadingDirection) -> Self {
        Self { binding }
    }

    pub(crate) fn suffix(self) -> &'static str {
        match self.binding {
            ReadingDirection::Rtl => "v1:rtl",
            ReadingDirection::Ltr => "v1:ltr",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutoCropDecline {
    RawAspectGate,
    PageAspectMissing,
    NotWide,
    NoJacketEvidence,
    NoStructure,
    NoLocalEvidence,
    NoSpine,
    WeakFront,
    RicherElsewhere,
}

impl AutoCropDecline {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RawAspectGate => "not-wide-raw",
            Self::PageAspectMissing => "no-page-aspect",
            Self::NotWide => "not-wide",
            Self::NoJacketEvidence => "no-jacket-evidence",
            Self::NoStructure => "no-structure",
            Self::NoLocalEvidence => "no-local-evidence",
            Self::NoSpine => "no-spine",
            Self::WeakFront => "weak",
            Self::RicherElsewhere => "richer-elsewhere",
        }
    }
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

/// Integrate source pixel areas instead of introducing cubic ringing. Each
/// analysis column covers one output pixel; each row covers two. The vertical
/// averaging suppresses isolated scan noise consistently even for an already
/// 256-row decode. Horizontal edges stay sharp: averaging two columns would
/// broaden a spine edge enough to create two separate detected boundaries.
fn analysis_area_average(source: &RgbImage, width: u32) -> RgbImage {
    let taps = |source_size: u32, output_size: u32, footprint: f64| {
        let scale = f64::from(source_size) / f64::from(output_size);
        (0..output_size)
            .map(|position| {
                let center = (f64::from(position) + 0.5) * scale;
                let radius = scale * footprint * 0.5;
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
    let columns = taps(source.width(), width, 1.0);
    let rows = taps(source.height(), 256, 2.0);
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

/// Each edge has its own reference color and 20% cap; a few dust pixels do not
/// terminate an otherwise blank margin.
fn content_bounds(raster: &Raster) -> [usize; 4] {
    let walk = |vertical: bool, reverse: bool| {
        let (depth, length) = if vertical {
            (raster.height, raster.width)
        } else {
            (raster.width, raster.height)
        };
        let pixel = |d, i| {
            let d = if reverse { depth - 1 - d } else { d };
            if vertical {
                raster.get(i, d)
            } else {
                raster.get(d, i)
            }
        };
        let reference = color_median((0..length).map(|i| pixel(0, i)));
        (0..depth / 5)
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

pub(crate) fn cover_auto_crop(
    image: &RgbImage,
    original_dims: (u32, u32),
    page_aspect: Option<f64>,
    binding: ReadingDirection,
) -> Result<CoverCrop, AutoCropDecline> {
    use AutoCropDecline::*;
    if original_dims.1 == 0 || f64::from(original_dims.0) / f64::from(original_dims.1) < 0.85 {
        return Err(RawAspectGate);
    }
    let aspect = page_aspect
        .filter(|r| r.is_finite() && *r > 0.0)
        .ok_or(PageAspectMissing)?;
    if image.width() == 0 || image.height() == 0 {
        return Err(NoStructure);
    }
    let source = Raster::from_image(image);
    let bounds = content_bounds(&source);
    let sub = source.region(bounds, binding == ReadingDirection::Ltr);
    if sub.width as f64 / (sub.height as f64) < aspect * 1.25 {
        return Err(NotWide);
    }
    let width = aspect * sub.height as f64;
    let features = Features::new(&sub);
    let edges = panel_edges(&sub);
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
    let mut end = detected_end.unwrap_or_else(|| target.round_ties_even() as usize);
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
    let score = features.score(start, end);
    if score < 0.6 {
        return Err(WeakFront);
    }
    let span = end - start;
    for x in (0..=sub.width - span).step_by(1.max(span / 20)) {
        // Signed overlap is intentional: disjoint intervals must qualify.
        let overlap = (x + span).min(end) as i64 - x.max(start) as i64;
        if (overlap as f64) < 0.5 * span as f64 && features.score(x, x + span) - score >= 0.5 {
            return Err(RicherElsewhere);
        }
    }
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

#[cfg(test)]
mod evaluation;
#[cfg(test)]
mod tests;
