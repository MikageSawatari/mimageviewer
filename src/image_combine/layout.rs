//! Closed-form layout in output pixels. No fonts, files, or raster allocations.
use super::options::*;

pub const MAX_CANVAS_PIXELS: u64 = 150_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self::from_edges(x, y, x + width, y + height)
    }
    /// Endpoints are canonical: reconstructing an edge as x + width can change
    /// its last bit and put a shared half-pixel boundary on opposite sides.
    pub fn from_edges(x: f64, y: f64, max_x: f64, max_y: f64) -> Self {
        Self { x, y, max_x, max_y }
    }
    pub fn width(self) -> f64 {
        self.max_x - self.x
    }
    pub fn height(self) -> f64 {
        self.max_y - self.y
    }
    pub fn right(self) -> f64 {
        self.max_x
    }
    pub fn bottom(self) -> f64 {
        self.max_y
    }
    pub fn inset(self, x: f64, y: f64) -> Self {
        Self::from_edges(self.x + x, self.y + y, self.max_x - x, self.max_y - y)
    }
    fn finite(self) -> bool {
        [
            self.x,
            self.y,
            self.width(),
            self.height(),
            self.right(),
            self.bottom(),
        ]
        .iter()
        .all(|v| v.is_finite())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CardLayout {
    /// Card/image/band endpoints are quantized output pixels. Only source_crop
    /// remains fractional; the renderer scales these endpoints for previews.
    pub card: Rect,
    pub image: Rect,
    /// Coordinates in the full composed source, even when rendering preview sources.
    pub source_crop: Rect,
    pub source_size: (u32, u32),
    pub band: Option<Rect>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub canvas_size: (u32, u32),
    pub cards: Vec<CardLayout>,
    /// Logical order, before right-to-left mirroring. Larger groups are later.
    pub split: Vec<u32>,
    pub max_crop_percent: f64,
    pub any_upscaled: bool,
    pub s: f64,
    pub thickness: f64,
    pub cell_ratio: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LayoutError {
    Empty,
    TooManyImages,
    InvalidSourceSize { index: usize },
    SplitCountMismatch,
    RatioWithUnmatchedLine,
    NonPositiveImageArea { group: usize },
    NonPositiveThickness,
    ImageTooSmall { index: usize },
    NonFinite,
    InvalidCanvasSize,
    CanvasPixelLimit,
    WebpSideLimit,
    JpegSideLimit,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("画像がありません"),
            Self::TooManyImages => f.write_str("結合できるのは100枚までです"),
            Self::InvalidSourceSize { index } => write!(f, "画像の寸法が0です (画像 {index})"),
            Self::SplitCountMismatch => f.write_str("行または列の枚数が一致しません"),
            Self::RatioWithUnmatchedLine => {
                f.write_str("そろえない配置では全体の比率を固定できません")
            }
            Self::NonPositiveImageArea { group } => {
                write!(f, "画像部の大きさが0以下です (行または列 {group})")
            }
            Self::NonPositiveThickness => f.write_str("固定部分が大きすぎて比率に合わせられません"),
            Self::ImageTooSmall { index } => write!(f, "画像の矩形が1px未満です (画像 {index})"),
            Self::NonFinite => f.write_str("配置の計算結果が有限ではありません"),
            Self::InvalidCanvasSize => f.write_str("出力の寸法が無効です"),
            Self::CanvasPixelLimit => f.write_str("出力が150000000画素を超えます"),
            Self::WebpSideLimit => f.write_str("WebPの各辺は16383pxまでです"),
            Self::JpegSideLimit => f.write_str("JPEGの各辺は65500pxまでです"),
        }
    }
}

impl std::error::Error for LayoutError {}

/// Use `options.labels.band_height()` for `label_band`; the explicit argument also
/// permits pure layout tests and callers with a previously derived band height.
pub fn layout(
    options: &CombineOptions,
    dims: &[(u32, u32)],
    label_band: Option<u32>,
) -> Result<Layout, LayoutError> {
    if dims.is_empty() {
        return Err(LayoutError::Empty);
    }
    if dims.len() > 100 {
        return Err(LayoutError::TooManyImages);
    }
    if let Some(index) = dims.iter().position(|&(w, h)| w == 0 || h == 0) {
        return Err(LayoutError::InvalidSourceSize { index });
    }
    let o = options.normalized();
    if o.canvas_ratio.is_some()
        && o.line_size == LineSize::None
        && matches!(o.layout, CombineLayout::Row | CombineLayout::Column)
    {
        return Err(LayoutError::RatioWithUnmatchedLine);
    }
    let n = dims.len();
    let candidates = match o.layout {
        CombineLayout::JustifiedRows | CombineLayout::JustifiedColumns => match &o.split {
            Split::Auto => (1..=n)
                .map(|groups| balanced(n, groups))
                .collect::<Vec<_>>(),
            Split::Counts(counts) => {
                if counts.is_empty()
                    || counts.iter().any(|&c| !(1..=100).contains(&c))
                    || counts.iter().map(|&c| c as u64).sum::<u64>() != n as u64
                {
                    return Err(LayoutError::SplitCountMismatch);
                }
                vec![counts.clone()]
            }
        },
        CombineLayout::Grid => {
            let columns = o
                .grid_columns
                .map_or_else(|| (1..=n as u32).collect(), |c| vec![c]);
            columns.into_iter().map(|c| vec![c]).collect()
        }
        CombineLayout::Row => vec![vec![n as u32]],
        CombineLayout::Column => vec![vec![n as u32]],
    };
    let t = label_band.unwrap_or(0) as f64;
    let mut best: Option<(f64, Layout)> = None;
    let mut first_error = None;
    for split in candidates {
        match solve(&o, dims, t, &split) {
            Ok((result, score)) => {
                // Candidates are in increasing group/column order; keep the first tie.
                if best.as_ref().is_none_or(|(old, _)| score < *old - 1e-12) {
                    best = Some((score, result));
                }
            }
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    best.map(|(_, result)| result)
        .ok_or_else(|| first_error.unwrap_or(LayoutError::InvalidCanvasSize))
}

fn balanced(n: usize, groups: usize) -> Vec<u32> {
    (0..groups)
        .map(|i| (n / groups + usize::from(i >= groups - n % groups)) as u32)
        .collect()
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[(values.len() - 1) / 2]
}

/// W = a*s+b; H = c*s+d.
struct Affine {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    s0: f64,
}

impl Affine {
    fn size(&self, s: f64) -> (f64, f64) {
        (self.a * s + self.b, self.c * s + self.d)
    }
    fn solve_size(&self, size: OutputSize) -> f64 {
        match size {
            OutputSize::Original => self.s0,
            OutputSize::Width(w) => (w as f64 - self.b) / self.a,
            OutputSize::Height(h) => (h as f64 - self.d) / self.c,
            OutputSize::LongEdge(e) => {
                ((e as f64 - self.b) / self.a).min((e as f64 - self.d) / self.c)
            }
        }
    }
}

/// Round cumulative content lengths only. Fixed bands and gaps are then added
/// as integers, so a half-pixel decision can change content by one pixel but
/// cannot change a fixed part. The last content edge preserves the canvas sum.
fn partition(start: f64, end: f64, lengths: &[f64], gap: f64, band: f64) -> Vec<(f64, f64)> {
    debug_assert!([start, end, gap, band].iter().all(|n| n.fract() == 0.0));
    let (start, end, gap, band) = (start as i64, end as i64, gap as i64, band as i64);
    let content_total =
        end - start - gap * (lengths.len() as i64 - 1) - band * lengths.len() as i64;
    let mut content_sum = 0.0;
    let mut previous = 0;
    lengths
        .iter()
        .enumerate()
        .map(|(i, &length)| {
            content_sum += length;
            let content_end = if i + 1 == lengths.len() {
                content_total
            } else {
                content_sum.round() as i64
            };
            let fixed_before = start + i as i64 * (gap + band);
            let span = (
                (fixed_before + previous) as f64,
                (fixed_before + content_end + band) as f64,
            );
            previous = content_end;
            span
        })
        .collect()
}

fn aligned_edges(start: f64, end: f64, length: f64, fraction: f64) -> (f64, f64) {
    // Both the container and the chosen content size are output pixels. Round
    // the offset once, then add the integer length instead of rounding two ends.
    let length = length.round();
    if fraction == 0.0 {
        (start, start + length)
    } else if fraction == 1.0 {
        (end - length, end)
    } else {
        let start = start + ((end - start - length) * fraction).round();
        (start, start + length)
    }
}

struct Cell {
    image_area: Rect,
    card_bottom: f64,
}

impl Cell {
    fn new(left: f64, top: f64, right: f64, image_bottom: f64, card_bottom: f64) -> Self {
        Self {
            image_area: Rect::from_edges(left, top, right, image_bottom),
            card_bottom,
        }
    }
}

fn solve(
    o: &CombineOptions,
    dims: &[(u32, u32)],
    t: f64,
    split: &[u32],
) -> Result<(Layout, f64), LayoutError> {
    let n = dims.len();
    let (gx, gy, m) = (o.gap_x as f64, o.gap_y as f64, o.margin as f64);
    let aspects: Vec<f64> = dims.iter().map(|&(w, h)| w as f64 / h as f64).collect();
    let widths: Vec<f64> = dims.iter().map(|&(w, _)| w as f64).collect();
    let heights: Vec<f64> = dims.iter().map(|&(_, h)| h as f64).collect();
    let matched = o.line_size != LineSize::None;
    let line_base = |values: &[f64]| match o.line_size {
        LineSize::Largest => values.iter().copied().fold(0.0, f64::max),
        LineSize::Smallest => values.iter().copied().fold(f64::INFINITY, f64::min),
        LineSize::None => 1.0,
    };
    let mut r = match o.cell_ratio {
        CellRatio::FirstImage => aspects[0],
        CellRatio::Ratio(w, h) => w as f64 / h as f64,
    };
    let mut group_terms = Vec::new();
    if matches!(
        o.layout,
        CombineLayout::JustifiedRows | CombineLayout::JustifiedColumns
    ) {
        let mut index = 0;
        for &count in split {
            let end = index + count as usize;
            let (sum, fixed) = if o.layout == CombineLayout::JustifiedRows {
                (
                    aspects[index..end].iter().sum::<f64>(),
                    (count - 1) as f64 * gx,
                )
            } else {
                (
                    aspects[index..end].iter().map(|a| 1.0 / a).sum::<f64>(),
                    (count - 1) as f64 * gy + count as f64 * t,
                )
            };
            group_terms.push((1.0 / sum, -fixed / sum));
            index = end;
        }
    }
    let groups = split.len() as f64;
    let sum_a = group_terms.iter().map(|v| v.0).sum::<f64>();
    let sum_b = group_terms.iter().map(|v| v.1).sum::<f64>();
    let columns = split[0] as usize;
    let rows = n.div_ceil(columns);
    let f = match o.layout {
        CombineLayout::Row => Affine {
            a: if matched {
                aspects.iter().sum()
            } else {
                widths.iter().sum()
            },
            b: (n - 1) as f64 * gx + 2.0 * m,
            c: if matched {
                1.0
            } else {
                heights.iter().copied().fold(0.0, f64::max)
            },
            d: t + 2.0 * m,
            s0: line_base(&heights),
        },
        CombineLayout::Column => Affine {
            a: if matched {
                1.0
            } else {
                widths.iter().copied().fold(0.0, f64::max)
            },
            b: 2.0 * m,
            c: if matched {
                aspects.iter().map(|a| 1.0 / a).sum()
            } else {
                heights.iter().sum()
            },
            d: n as f64 * t + (n - 1) as f64 * gy + 2.0 * m,
            s0: line_base(&widths),
        },
        CombineLayout::Grid => {
            let mut by_area = dims.to_vec();
            by_area.sort_by_key(|&(w, h)| w as u64 * h as u64);
            let (w, h) = by_area[(n - 1) / 2];
            let s0 = match o.cell_fit {
                CellFit::Fit => (w as f64).max(h as f64 * r),
                CellFit::Fill => (w as f64).min(h as f64 * r),
            };
            Affine {
                a: columns as f64,
                b: (columns - 1) as f64 * gx + 2.0 * m,
                c: rows as f64 / r,
                d: rows as f64 * t + (rows - 1) as f64 * gy + 2.0 * m,
                s0,
            }
        }
        CombineLayout::JustifiedRows => Affine {
            a: 1.0,
            b: 2.0 * m,
            c: sum_a,
            d: sum_b + groups * t + (groups - 1.0) * gy + 2.0 * m,
            s0: (median(heights.clone()) * groups - sum_b) / sum_a,
        },
        CombineLayout::JustifiedColumns => Affine {
            a: sum_a,
            b: sum_b + (groups - 1.0) * gx + 2.0 * m,
            c: 1.0,
            d: 2.0 * m,
            s0: (median(widths.clone()) * groups - sum_b) / sum_a,
        },
    };
    let horizontal = matches!(
        o.layout,
        CombineLayout::Row | CombineLayout::JustifiedRows | CombineLayout::Grid
    );
    let (s, w, h, k) = if let Some((rw, rh)) = o.canvas_ratio {
        let ratio = rw as f64 / rh as f64;
        let (w, h) = match o.output_size {
            OutputSize::Width(w) => (w as f64, w as f64 / ratio),
            OutputSize::Height(h) => (h as f64 * ratio, h as f64),
            OutputSize::LongEdge(e) if ratio >= 1.0 => (e as f64, e as f64 / ratio),
            OutputSize::LongEdge(e) => (e as f64 * ratio, e as f64),
            OutputSize::Original if horizontal => {
                let w = f.size(f.s0).0;
                (w, w / ratio)
            }
            OutputSize::Original => {
                let h = f.size(f.s0).1;
                (h * ratio, h)
            }
        };
        let s = if horizontal {
            (w - f.b) / f.a
        } else {
            (h - f.d) / f.c
        };
        let (natural_w, natural_h) = f.size(s);
        let k = if o.layout == CombineLayout::Grid {
            let image_height = (h - f.d) / rows as f64;
            if image_height <= 0.0 {
                return Err(LayoutError::NonPositiveThickness);
            }
            r = s / image_height;
            1.0
        } else if horizontal {
            let fixed = match o.layout {
                CombineLayout::Row => t + 2.0 * m,
                _ => groups * t + (groups - 1.0) * gy + 2.0 * m,
            };
            (h - fixed) / (natural_h - fixed)
        } else {
            let fixed = if o.layout == CombineLayout::Column {
                2.0 * m
            } else {
                (groups - 1.0) * gx + 2.0 * m
            };
            (w - fixed) / (natural_w - fixed)
        };
        (s, w, h, k)
    } else {
        let s = f.solve_size(o.output_size);
        let (w, h) = f.size(s);
        (s, w, h, 1.0)
    };
    if ![s, w, h, k, r].iter().all(|v| v.is_finite()) {
        return Err(LayoutError::NonFinite);
    }
    if s <= 0.0 {
        return Err(LayoutError::NonPositiveImageArea { group: 0 });
    }
    if k <= 0.0 || r <= 0.0 {
        return Err(LayoutError::NonPositiveThickness);
    }
    let canvas_size = checked_canvas(w, h, o.format)?;
    let inner_right = canvas_size.0 as f64 - m;
    let inner_bottom = canvas_size.1 as f64 - m;
    let mut cells = Vec::with_capacity(n);
    match o.layout {
        CombineLayout::Row => {
            let lengths: Vec<_> = (0..n)
                .map(|i| {
                    if matched {
                        aspects[i] * s
                    } else {
                        widths[i] * s
                    }
                })
                .collect();
            let max_height = heights.iter().copied().fold(0.0, f64::max);
            let card_bottom = inner_bottom;
            let image_bottom = card_bottom - t;
            for (i, (left, right)) in partition(m, inner_right, &lengths, gx, 0.0)
                .into_iter()
                .enumerate()
            {
                let ih = if matched { s * k } else { heights[i] * s };
                let (top, bottom) = if matched || heights[i] == max_height {
                    (m, image_bottom)
                } else {
                    aligned_edges(m, image_bottom, ih, o.line_align.fraction())
                };
                let bottom_with_band = if bottom == image_bottom {
                    card_bottom
                } else {
                    bottom + t
                };
                cells.push(Cell::new(left, top, right, bottom, bottom_with_band));
            }
        }
        CombineLayout::Column => {
            let lengths: Vec<_> = (0..n)
                .map(|i| {
                    if matched {
                        s / aspects[i]
                    } else {
                        heights[i] * s
                    }
                })
                .collect();
            let max_width = widths.iter().copied().fold(0.0, f64::max);
            for (i, (top, bottom)) in partition(m, inner_bottom, &lengths, gy, t)
                .into_iter()
                .enumerate()
            {
                let iw = if matched { s * k } else { widths[i] * s };
                let (left, right) = if matched || widths[i] == max_width {
                    (m, inner_right)
                } else {
                    aligned_edges(m, inner_right, iw, o.line_align.fraction())
                };
                cells.push(Cell::new(left, top, right, bottom - t, bottom));
            }
        }
        CombineLayout::Grid => {
            let xs = partition(m, inner_right, &vec![s; columns], gx, 0.0);
            let ys = partition(m, inner_bottom, &vec![s / r; rows], gy, t);
            for i in 0..n {
                let (left, right) = xs[i % columns];
                let (top, bottom) = ys[i / columns];
                cells.push(Cell::new(left, top, right, bottom - t, bottom));
            }
        }
        CombineLayout::JustifiedRows | CombineLayout::JustifiedColumns => {
            let mut bases = Vec::with_capacity(split.len());
            for (group, &(a, b)) in group_terms.iter().enumerate() {
                let base = a * s + b;
                if base <= 0.0 {
                    return Err(LayoutError::NonPositiveImageArea { group });
                }
                bases.push(base);
            }
            let cross_lengths: Vec<_> = bases.iter().map(|base| base * k).collect();
            let cross_spans = partition(
                m,
                if horizontal {
                    inner_bottom
                } else {
                    inner_right
                },
                &cross_lengths,
                if horizontal { gy } else { gx },
                if horizontal { t } else { 0.0 },
            );
            let mut index = 0;
            for ((&count, &base), (cross_start, cross_end)) in
                split.iter().zip(&bases).zip(cross_spans)
            {
                let lengths: Vec<_> = aspects[index..index + count as usize]
                    .iter()
                    .map(|aspect| {
                        if horizontal {
                            aspect * base
                        } else {
                            base / aspect
                        }
                    })
                    .collect();
                let spans = partition(
                    m,
                    if horizontal {
                        inner_right
                    } else {
                        inner_bottom
                    },
                    &lengths,
                    if horizontal { gx } else { gy },
                    if horizontal { 0.0 } else { t },
                );
                for (start, end) in spans {
                    if horizontal {
                        cells.push(Cell::new(start, cross_start, end, cross_end - t, cross_end));
                    } else {
                        cells.push(Cell::new(cross_start, start, cross_end, end - t, end));
                    }
                    index += 1;
                }
            }
        }
    }
    let mut cards = Vec::with_capacity(n);
    let mut max_crop: f64 = 0.0;
    let mut upscaled = false;
    for (index, placement) in cells.into_iter().enumerate() {
        let cell = placement.image_area;
        let size = dims[index];
        let full = Rect::new(0.0, 0.0, size.0 as f64, size.1 as f64);
        let mut image = if o.layout == CombineLayout::Grid && o.cell_fit == CellFit::Fit {
            let sx = cell.width() / full.width();
            let sy = cell.height() / full.height();
            let factor = sx.min(sy);
            let (ax, ay) = o.anchor.fractions();
            let (iw, ih) = (full.width() * factor, full.height() * factor);
            let (left, right) = if sx <= sy {
                (cell.x, cell.right())
            } else {
                aligned_edges(cell.x, cell.right(), iw, ax)
            };
            let (top, bottom) = if sy <= sx {
                (cell.y, cell.bottom())
            } else {
                aligned_edges(cell.y, cell.bottom(), ih, ay)
            };
            Rect::from_edges(left, top, right, bottom)
        } else {
            cell
        };
        if image.width() < 1.0 || image.height() < 1.0 {
            return Err(LayoutError::ImageTooSmall { index });
        }
        // Derive sampling from the final pixel rectangle, including the small
        // aspect change from quantization. Keep the requested source anchor.
        let crop = fill_crop(full, image, o.anchor);
        let mut card = Rect::from_edges(cell.x, cell.y, cell.right(), placement.card_bottom);
        let mut band = (t > 0.0)
            .then(|| Rect::from_edges(cell.x, cell.bottom(), cell.right(), placement.card_bottom));
        if o.direction == CombineDirection::RightToLeft && o.layout != CombineLayout::Column {
            let mirror = |rect: &mut Rect| {
                *rect = Rect::from_edges(
                    canvas_size.0 as f64 - rect.right(),
                    rect.y,
                    canvas_size.0 as f64 - rect.x,
                    rect.bottom(),
                );
            };
            mirror(&mut card);
            mirror(&mut image);
            if let Some(band) = &mut band {
                mirror(band);
            }
        }
        if ![cell, image, crop, card].iter().all(|r| r.finite()) {
            return Err(LayoutError::NonFinite);
        }
        max_crop = max_crop
            .max(1.0 - crop.width() / full.width())
            .max(1.0 - crop.height() / full.height());
        upscaled |= image.width() > crop.width() + 1e-9 || image.height() > crop.height() + 1e-9;
        cards.push(CardLayout {
            card,
            image,
            source_crop: crop,
            source_size: size,
            band,
        });
    }
    let chosen_split = if o.layout == CombineLayout::Grid {
        (0..rows)
            .map(|row| (n - row * columns).min(columns) as u32)
            .collect()
    } else {
        split.to_vec()
    };
    let score = if o.canvas_ratio.is_none() {
        (w / h).ln().abs()
    } else if o.layout == CombineLayout::Grid {
        (r / median(aspects)).ln().abs()
    } else {
        k.ln().abs()
    };
    Ok((
        Layout {
            canvas_size,
            cards,
            split: chosen_split,
            max_crop_percent: max_crop * 100.0,
            any_upscaled: upscaled,
            s,
            thickness: k,
            cell_ratio: (o.layout == CombineLayout::Grid).then_some(r),
        },
        score,
    ))
}

fn checked_canvas(w: f64, h: f64, format: CombineFormat) -> Result<(u32, u32), LayoutError> {
    let (w, h) = (w.round(), h.round());
    if w < 1.0 || h < 1.0 || w > u32::MAX as f64 || h > u32::MAX as f64 {
        return Err(LayoutError::InvalidCanvasSize);
    }
    let (w, h) = (w as u32, h as u32);
    match format {
        CombineFormat::Webp { .. } if w > 16383 || h > 16383 => {
            return Err(LayoutError::WebpSideLimit);
        }
        CombineFormat::Jpeg { .. } if w > 65500 || h > 65500 => {
            return Err(LayoutError::JpegSideLimit);
        }
        _ => {}
    }
    if w as u64 * h as u64 > MAX_CANVAS_PIXELS {
        return Err(LayoutError::CanvasPixelLimit);
    }
    Ok((w, h))
}

fn fill_crop(source: Rect, target: Rect, anchor: Anchor9) -> Rect {
    let factor = (target.width() / source.width()).max(target.height() / source.height());
    let (cw, ch) = (target.width() / factor, target.height() / factor);
    let (ax, ay) = anchor.fractions();
    Rect::new(
        (source.width() - cw) * ax,
        (source.height() - ch) * ay,
        cw,
        ch,
    )
}
