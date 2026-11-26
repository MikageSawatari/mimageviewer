//! Shared preview/output primitives. Callers can release each full source after draw_card.
use super::{
    label::{LabelLayout, PlacedLabel, render_text},
    layout::{CardLayout, Layout, MAX_CANVAS_PIXELS, Rect},
    options::*,
};
use image::{Rgba, RgbaImage};

#[derive(Debug, PartialEq)]
pub enum RenderError {
    InvalidScale,
    InvalidDimensions,
    ItemCountMismatch,
    Allocation,
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidScale => "描画の倍率が無効です",
            Self::InvalidDimensions => "描画の寸法が無効です",
            Self::ItemCountMismatch => "画像またはラベルの数が一致しません",
            Self::Allocation => "キャンバスのメモリを確保できません",
        })
    }
}

impl std::error::Error for RenderError {}

fn check_scale(scale: f64) -> Result<(), RenderError> {
    if !scale.is_finite() || scale <= 0.0 || scale > 1.0 {
        Err(RenderError::InvalidScale)
    } else {
        Ok(())
    }
}

/// Output layout endpoints are already quantized. Only previews round edges
/// after scaling; widths must never be rounded independently of those edges.
pub fn scaled_rect(rect: Rect, scale: f64) -> Rect {
    if scale == 1.0 {
        debug_assert!(
            [rect.x, rect.y, rect.right(), rect.bottom()]
                .iter()
                .all(|v| v.fract() == 0.0)
        );
        return rect;
    }
    let (x, y) = ((rect.x * scale).round(), (rect.y * scale).round());
    Rect::from_edges(
        x,
        y,
        (rect.right() * scale).round(),
        (rect.bottom() * scale).round(),
    )
}

fn rgba(rgb: [u8; 3], alpha: u8) -> Rgba<u8> {
    Rgba([rgb[0], rgb[1], rgb[2], alpha])
}

fn background(o: &CombineOptions) -> Rgba<u8> {
    match o.background {
        Background::Color(rgb) => rgba(rgb, 255),
        Background::Transparent => Rgba([0; 4]),
    }
}

pub fn create_canvas(
    layout: &Layout,
    options: &CombineOptions,
    scale: f64,
) -> Result<RgbaImage, RenderError> {
    check_scale(scale)?;
    let (w, h) = if scale == 1.0 {
        layout.canvas_size
    } else {
        (
            (layout.canvas_size.0 as f64 * scale).round().max(1.0) as u32,
            (layout.canvas_size.1 as f64 * scale).round().max(1.0) as u32,
        )
    };
    if layout.canvas_size.0 == 0
        || layout.canvas_size.1 == 0
        || w as u64 * h as u64 > MAX_CANVAS_PIXELS
    {
        return Err(RenderError::InvalidDimensions);
    }
    let bytes = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(RenderError::InvalidDimensions)?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(bytes)
        .map_err(|_| RenderError::Allocation)?;
    pixels.resize(bytes, 0);
    let mut canvas = RgbaImage::from_raw(w, h, pixels).ok_or(RenderError::InvalidDimensions)?;
    let color = background(&options.normalized());
    for pixel in canvas.pixels_mut() {
        *pixel = color;
    }
    Ok(canvas)
}

/// Signed-distance coverage of a rounded rectangle at the pixel center. All
/// rounded cards, inner borders and badges share this one antialiasing rule.
pub fn rounded_coverage(rect: Rect, radius: f64, x: f64, y: f64) -> f64 {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return 0.0;
    }
    let radius = radius.max(0.0).min(rect.width().min(rect.height()) * 0.5);
    let qx = (x - (rect.x + rect.width() * 0.5)).abs() - (rect.width() * 0.5 - radius);
    let qy = (y - (rect.y + rect.height() * 0.5)).abs() - (rect.height() * 0.5 - radius);
    // Only the corner quadrants need a circular distance. On a straight side
    // (including radius zero), the same signed-distance formula reduces to max.
    let distance = if qx > 0.0 && qy > 0.0 {
        qx.hypot(qy) - radius
    } else {
        qx.max(qy) - radius
    };
    (0.5 - distance).clamp(0.0, 1.0)
}

/// Straight-alpha src-over, identical to the export watermark's composite rule.
pub fn composite(dst: &mut Rgba<u8>, src: Rgba<u8>, coverage: f64) {
    let a = src[3] as f64 / 255.0 * coverage;
    if a <= 0.0 {
        return;
    }
    let da = dst[3] as f64 / 255.0;
    let out_a = a + da * (1.0 - a);
    for channel in 0..3 {
        dst[channel] = ((src[channel] as f64 * a + dst[channel] as f64 * da * (1.0 - a)) / out_a)
            .round() as u8;
    }
    dst[3] = (out_a * 255.0).round() as u8;
}

fn paint(canvas: &mut RgbaImage, rect: Rect, mut sample: impl FnMut(f64, f64) -> (Rgba<u8>, f64)) {
    let x0 = rect.x.max(0.0) as u32;
    let y0 = rect.y.max(0.0) as u32;
    let x1 = rect.right().max(0.0).min(canvas.width() as f64) as u32;
    let y1 = rect.bottom().max(0.0).min(canvas.height() as f64) as u32;
    for y in y0..y1 {
        for x in x0..x1 {
            let (src, coverage) = sample(x as f64 + 0.5, y as f64 + 0.5);
            composite(canvas.get_pixel_mut(x, y), src, coverage);
        }
    }
}

/// Non-overlapping scanline spans covering only the border bands and corner
/// squares. Bounds are scaled/rounded already; ceil includes the AA fringe for
/// fractional preview widths. Clip only after deriving spans from the card.
pub(super) fn for_each_border_span(
    outer: Rect,
    radius: f64,
    border: f64,
    canvas_size: (u32, u32),
    mut visit: impl FnMut(u32, u32, u32),
) {
    if border <= 0.0 || outer.width() <= 0.0 || outer.height() <= 0.0 {
        return;
    }
    let radius = radius.max(0.0).min(outer.width().min(outer.height()) * 0.5);
    let (left, right, top, bottom) = (
        outer.x as i64,
        outer.right() as i64,
        outer.y as i64,
        outer.bottom() as i64,
    );
    let band = border.ceil() as i64;
    let corner = radius.max(border).ceil() as i64;
    let mut emit = |x0: i64, x1: i64, y: i64| {
        let x0 = x0.clamp(0, canvas_size.0 as i64) as u32;
        let x1 = x1.clamp(0, canvas_size.0 as i64) as u32;
        if x0 < x1 {
            visit(x0, x1, y as u32);
        }
    };
    for y in top.max(0)..bottom.min(canvas_size.1 as i64) {
        let edge_distance = (y - top).min(bottom - 1 - y);
        if edge_distance < band {
            emit(left, right, y);
        } else {
            let extent = if edge_distance < corner { corner } else { band };
            let left_end = (left + extent).min(right);
            let right_start = (right - extent).max(left_end);
            emit(left, left_end, y);
            emit(right_start, right, y);
        }
    }
}

pub(super) fn paint_border(
    canvas: &mut RgbaImage,
    outer: Rect,
    radius: f64,
    border: f64,
    color: Rgba<u8>,
) {
    let inner = outer.inset(border, border);
    let canvas_size = canvas.dimensions();
    for_each_border_span(outer, radius, border, canvas_size, |left, right, y| {
        paint(
            canvas,
            Rect::from_edges(left as f64, y as f64, right as f64, (y + 1) as f64),
            |x, y| {
                let ring = (rounded_coverage(outer, radius, x, y)
                    - rounded_coverage(inner, (radius - border).max(0.0), x, y))
                .max(0.0);
                (color, ring)
            },
        );
    });
}

/// Keep fractional source coordinates, including for downscaled previews. FIR's
/// crop option reads directly from the source, avoiding a second full-size crop.
fn resize_source(
    source: &RgbaImage,
    card: &CardLayout,
    width: u32,
    height: u32,
) -> Result<RgbaImage, RenderError> {
    if source.width() == 0
        || source.height() == 0
        || card.source_size.0 == 0
        || card.source_size.1 == 0
    {
        return Err(RenderError::InvalidDimensions);
    }
    let crop = card.source_crop;
    let sx = source.width() as f64 / card.source_size.0 as f64;
    let sy = source.height() as f64 / card.source_size.1 as f64;
    let (x0, y0) = ((crop.x * sx).max(0.0), (crop.y * sy).max(0.0));
    let (x1, y1) = (
        (crop.right() * sx).min(source.width() as f64),
        (crop.bottom() * sy).min(source.height() as f64),
    );
    if ![x0, y0, x1, y1].iter().all(|n| n.is_finite()) || x1 <= x0 || y1 <= y0 {
        return Err(RenderError::InvalidDimensions);
    }
    if x0 == 0.0 && y0 == 0.0 && x1 == source.width() as f64 && y1 == source.height() as f64 {
        return Ok(crate::fast_resize::resize_rgba8_exact(
            source,
            width,
            height,
            crate::fast_resize::Quality::Lanczos3,
        ));
    }
    let mut resized = RgbaImage::new(width, height);
    // Same engine and Lanczos3 options as fast_resize::resize_rgba8_exact;
    // that helper has no fractional-crop argument.
    let resize_options = fast_image_resize::ResizeOptions::new()
        .resize_alg(crate::fast_resize::Quality::Lanczos3.into())
        .crop(x0, y0, x1 - x0, y1 - y0);
    fast_image_resize::Resizer::new()
        .resize(source, &mut resized, &resize_options)
        .map_err(|_| RenderError::InvalidDimensions)?;
    Ok(resized)
}

pub fn draw_card(
    canvas: &mut RgbaImage,
    card: &CardLayout,
    source: &RgbaImage,
    options: &CombineOptions,
    scale: f64,
) -> Result<(), RenderError> {
    draw_card_timed(canvas, card, source, options, scale).map(|_| ())
}

/// Worker measurements use the same primitive; resize and drawing are distinct.
pub(crate) fn draw_card_timed(
    canvas: &mut RgbaImage,
    card: &CardLayout,
    source: &RgbaImage,
    options: &CombineOptions,
    scale: f64,
) -> Result<(f64, f64), RenderError> {
    let started = std::time::Instant::now();
    let mut resize_ms = 0.0;
    check_scale(scale)?;
    let o = options.normalized();
    let outer = scaled_rect(card.card, scale);
    let radius =
        (o.corner_radius as f64).min(card.card.width().min(card.card.height()) * 0.5) * scale;
    if let Some(band) = card.band {
        let band = scaled_rect(band, scale);
        let fill = match o.labels.band_color {
            BandColor::Background => background(&o),
            BandColor::Color(rgb) => rgba(rgb, 255),
        };
        paint(canvas, band, |x, y| {
            (fill, rounded_coverage(outer, radius, x, y))
        });
    }
    let target = scaled_rect(card.image, scale);
    if target.width() > 0.0 && target.height() > 0.0 {
        let resize_start = std::time::Instant::now();
        let resized = resize_source(source, card, target.width() as u32, target.height() as u32)?;
        resize_ms = resize_start.elapsed().as_secs_f64() * 1000.0;
        paint(canvas, target, |x, y| {
            (
                *resized.get_pixel((x - target.x) as u32, (y - target.y) as u32),
                rounded_coverage(outer, radius, x, y),
            )
        });
    }
    let border = o.border.width_px as f64 * scale;
    if border > 0.0 {
        paint_border(canvas, outer, radius, border, rgba(o.border.color, 255));
    }
    Ok((
        resize_ms,
        (started.elapsed().as_secs_f64() * 1000.0 - resize_ms).max(0.0),
    ))
}

pub fn draw_label(
    canvas: &mut RgbaImage,
    card: &CardLayout,
    label: &PlacedLabel,
    options: &CombineOptions,
    scale: f64,
    fonts: &comic_core::FontSet,
) -> Result<(), RenderError> {
    check_scale(scale)?;
    let o = options.normalized();
    if o.labels.placement == LabelPlacement::None {
        return Ok(());
    }
    let card_rect = scaled_rect(card.card, scale);
    let card_radius =
        (o.corner_radius as f64).min(card.card.width().min(card.card.height()) * 0.5) * scale;
    let band = card.band.map(|band| scaled_rect(band, scale));
    let clip = |x: f64, y: f64| {
        let card = rounded_coverage(card_rect, card_radius, x, y);
        if o.labels.placement == LabelPlacement::Band {
            card * band.map_or(0.0, |band| rounded_coverage(band, 0.0, x, y))
        } else {
            card
        }
    };
    if o.labels.style != BadgeStyle::Plain {
        let rect = scaled_rect(label.rect, scale);
        let radius = if o.labels.style == BadgeStyle::Pill {
            rect.height() * 0.5
        } else {
            label.text_size_px * 0.3 * scale
        };
        let color = rgba(
            o.labels.fill_color,
            (o.labels.fill_opacity as f64 * 255.0 / 100.0).round() as u8,
        );
        paint(canvas, rect, |x, y| {
            (color, clip(x, y) * rounded_coverage(rect, radius, x, y))
        });
    }
    let target = scaled_rect(label.text_rect, scale);
    if target.width() > 0.0 && target.height() > 0.0 {
        let original = render_text(label, &o.labels, fonts);
        let text = if original.dimensions() == (target.width() as u32, target.height() as u32) {
            original
        } else {
            crate::fast_resize::resize_rgba8_exact(
                &original,
                target.width() as u32,
                target.height() as u32,
                crate::fast_resize::Quality::Lanczos3,
            )
        };
        paint(canvas, target, |x, y| {
            (
                *text.get_pixel((x - target.x) as u32, (y - target.y) as u32),
                clip(x, y),
            )
        });
    }
    Ok(())
}

/// Convenience for previews/tests. The writer uses create_canvas, draw_card and
/// draw_label directly, keeping only the current composed source in memory.
pub fn render_all(
    layout: &Layout,
    sources: &[RgbaImage],
    options: &CombineOptions,
    labels: &LabelLayout,
    scale: f64,
    fonts: &comic_core::FontSet,
) -> Result<RgbaImage, RenderError> {
    if sources.len() != layout.cards.len() || labels.labels.len() != layout.cards.len() {
        return Err(RenderError::ItemCountMismatch);
    }
    let mut canvas = create_canvas(layout, options, scale)?;
    for ((card, source), label) in layout.cards.iter().zip(sources).zip(&labels.labels) {
        draw_card(&mut canvas, card, source, options, scale)?;
        if let Some(label) = label {
            draw_label(&mut canvas, card, label, options, scale, fonts)?;
        }
    }
    Ok(canvas)
}
