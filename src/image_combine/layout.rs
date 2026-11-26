//! Closed-form layout in output pixels. No fonts, files, or raster allocations.
use super::options::*;

pub const MAX_CANVAS_PIXELS: u64 = 150_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
    pub fn right(self) -> f64 {
        self.x + self.width
    }
    pub fn bottom(self) -> f64 {
        self.y + self.height
    }
    pub fn inset(self, x: f64, y: f64) -> Self {
        Self::new(
            self.x + x,
            self.y + y,
            self.width - 2.0 * x,
            self.height - 2.0 * y,
        )
    }
    fn finite(self) -> bool {
        [
            self.x,
            self.y,
            self.width,
            self.height,
            self.right(),
            self.bottom(),
        ]
        .iter()
        .all(|v| v.is_finite())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CardLayout {
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
    let mut cells = Vec::with_capacity(n);
    match o.layout {
        CombineLayout::Row => {
            let mut x = m;
            let max_h = if matched {
                s
            } else {
                heights.iter().copied().fold(0.0, f64::max) * s
            };
            for i in 0..n {
                let iw = if matched {
                    aspects[i] * s
                } else {
                    widths[i] * s
                };
                let ih = if matched { s * k } else { heights[i] * s };
                cells.push(Rect::new(
                    x,
                    m + (max_h * k - ih) * o.line_align.fraction(),
                    iw,
                    ih,
                ));
                x += iw + gx;
            }
        }
        CombineLayout::Column => {
            let mut y = m;
            let max_w = if matched {
                s
            } else {
                widths.iter().copied().fold(0.0, f64::max) * s
            };
            for i in 0..n {
                let iw = if matched { s * k } else { widths[i] * s };
                let ih = if matched {
                    s / aspects[i]
                } else {
                    heights[i] * s
                };
                cells.push(Rect::new(
                    m + (max_w * k - iw) * o.line_align.fraction(),
                    y,
                    iw,
                    ih,
                ));
                y += ih + t + gy;
            }
        }
        CombineLayout::Grid => {
            for i in 0..n {
                cells.push(Rect::new(
                    m + (i % columns) as f64 * (s + gx),
                    m + (i / columns) as f64 * (s / r + t + gy),
                    s,
                    s / r,
                ));
            }
        }
        CombineLayout::JustifiedRows | CombineLayout::JustifiedColumns => {
            let mut index = 0;
            let mut cross = m;
            for (group, (&count, &(a, b))) in split.iter().zip(&group_terms).enumerate() {
                let base = a * s + b;
                if base <= 0.0 {
                    return Err(LayoutError::NonPositiveImageArea { group });
                }
                let mut along = m;
                for _ in 0..count {
                    if horizontal {
                        let iw = aspects[index] * base;
                        cells.push(Rect::new(along, cross, iw, base * k));
                        along += iw + gx;
                    } else {
                        let ih = base / aspects[index];
                        cells.push(Rect::new(cross, along, base * k, ih));
                        along += ih + t + gy;
                    }
                    index += 1;
                }
                cross += if horizontal {
                    base * k + t + gy
                } else {
                    base * k + gx
                };
            }
        }
    }
    let mut cards = Vec::with_capacity(n);
    let mut max_crop: f64 = 0.0;
    let mut upscaled = false;
    for (index, cell) in cells.into_iter().enumerate() {
        let size = dims[index];
        let full = Rect::new(0.0, 0.0, size.0 as f64, size.1 as f64);
        let (mut image, crop) = if o.layout == CombineLayout::Grid && o.cell_fit == CellFit::Fit {
            let factor = (cell.width / full.width).min(cell.height / full.height);
            let (ax, ay) = o.anchor.fractions();
            let (iw, ih) = (full.width * factor, full.height * factor);
            (
                Rect::new(
                    cell.x + (cell.width - iw) * ax,
                    cell.y + (cell.height - ih) * ay,
                    iw,
                    ih,
                ),
                full,
            )
        } else {
            (cell, fill_crop(full, cell, o.anchor))
        };
        let mut card = Rect::new(cell.x, cell.y, cell.width, cell.height + t);
        let mut band = (t > 0.0).then(|| Rect::new(cell.x, cell.bottom(), cell.width, t));
        if o.direction == CombineDirection::RightToLeft && o.layout != CombineLayout::Column {
            let mirror = |rect: &mut Rect| {
                rect.x = canvas_size.0 as f64 - rect.right();
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
        if image.width < 1.0 - 1e-9 || image.height < 1.0 - 1e-9 {
            return Err(LayoutError::ImageTooSmall { index });
        }
        max_crop = max_crop
            .max(1.0 - crop.width / full.width)
            .max(1.0 - crop.height / full.height);
        upscaled |= image.width > crop.width + 1e-9 || image.height > crop.height + 1e-9;
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
    let factor = (target.width / source.width).max(target.height / source.height);
    let (cw, ch) = (target.width / factor, target.height / factor);
    let (ax, ay) = anchor.fractions();
    Rect::new((source.width - cw) * ax, (source.height - ch) * ay, cw, ch)
}
