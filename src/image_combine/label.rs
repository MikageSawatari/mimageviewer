//! Resolve labels and measure decorated single-line boxes once in output coordinates.
use super::{
    layout::{Layout, Rect},
    options::*,
};
use image::RgbaImage;

#[derive(Clone, Debug, PartialEq)]
pub struct PlacedLabel {
    pub text: String,
    pub text_size_px: f64,
    /// Includes the line box, outline, badge padding and Pill minimum width.
    pub rect: Rect,
    pub text_rect: Rect,
    /// Local line/ink bounds used to shift negative bearings into the text raster.
    pub text_bounds: Rect,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LabelLayout {
    pub labels: Vec<Option<PlacedLabel>>,
    /// Explicitly empty overrides and disabled labels do not count as failures.
    pub not_drawn: usize,
}

pub fn resolve_label(spec: &CombineItemSpec, index: usize, options: &LabelOptions) -> String {
    if let Some(text) = &spec.label_override {
        return text.replace(['\r', '\n'], " ").chars().take(200).collect();
    }
    let number = options.number_start as u64 + index as u64;
    match options.number_format {
        NumberFormat::Arabic => number.to_string(),
        NumberFormat::ZeroPadded => format!("{number:02}"),
        // 1 = A; preserve an explicitly requested zero without repeating A.
        NumberFormat::UpperAlpha | NumberFormat::LowerAlpha if number == 0 => "0".into(),
        NumberFormat::UpperAlpha | NumberFormat::LowerAlpha => {
            let mut number = number;
            let base = if options.number_format == NumberFormat::UpperAlpha {
                b'A'
            } else {
                b'a'
            };
            let mut letters = Vec::new();
            while number > 0 {
                number -= 1;
                letters.push((base + (number % 26) as u8) as char);
                number /= 26;
            }
            letters.into_iter().rev().collect()
        }
    }
}

pub(super) fn text_block(text: &str, size: f64, o: &LabelOptions) -> comic_core::TextBlock {
    let [r, g, b] = o.text_color;
    comic_core::TextBlock {
        text: text.into(),
        font_key: crate::comic_overlay::COMIC_FONT_KEY.into(),
        size_px: size as f32,
        color: comic_core::Rgba::new(r, g, b, 255),
        outline: (o.style == BadgeStyle::Plain && o.outline).then_some(comic_core::StrokeStyle {
            color: comic_core::Rgba::BLACK,
            width_px: (size as f32 / 16.0).max(1.0),
        }),
        ..Default::default()
    }
}

fn measure(
    text: &str,
    size: f64,
    o: &LabelOptions,
    fonts: &comic_core::FontSet,
) -> Option<(Rect, f64, f64)> {
    let block = text_block(text, size, o);
    let font = fonts.get(&block.font_key)?;
    let measured = comic_core::layout_text(&block, font);
    let outline = block.outline.as_ref().map_or(0.0, |s| s.width_px);
    let effect = comic_core::text_effect_padding(&block).ceil() as f64;
    let mut bounds = Rect::new(
        -effect,
        -effect,
        measured.bounds.0 as f64 + 2.0 * effect,
        measured.bounds.1 as f64 + 2.0 * effect,
    );
    // The line box retains ascenders/descenders and whitespace. Union the actual
    // raster extents as well, so negative bearings and the outline cannot be cut.
    let mut right = bounds.right();
    let mut bottom = bounds.bottom();
    let pad = if outline > 0.0 {
        outline.ceil() + 1.0
    } else {
        0.0
    };
    for glyph in &measured.glyphs {
        // Match comic-core's bitmap extents without allocating glyph rasters
        // for each trial size during shrink/ellipsis measurement.
        if let Some((left, top, width, height)) =
            font.glyph_px_bounds_gid(glyph.glyph_id, glyph.size)
        {
            let x = (glyph.x + left - pad).floor() as f64;
            let y = (glyph.y + top - pad).floor() as f64;
            bounds.x = bounds.x.min(x);
            bounds.y = bounds.y.min(y);
            right = right.max(x + (width.ceil() + 2.0 * pad) as f64);
            bottom = bottom.max(y + (height.ceil() + 2.0 * pad) as f64);
        }
    }
    bounds.max_x = bounds.x + (right - bounds.x).ceil();
    bounds.max_y = bounds.y + (bottom - bounds.y).ceil();
    let padding = if o.style == BadgeStyle::Plain {
        0.0
    } else {
        size * 0.35
    };
    let height = bounds.height() + 2.0 * padding;
    let width = (bounds.width() + 2.0 * padding).max(if o.style == BadgeStyle::Pill {
        height
    } else {
        0.0
    });
    Some((bounds, width, height))
}

/// Fonts are loaded by the worker, never here. Missing default fonts make active
/// nonempty labels unplaceable; the stage-B caller reports its font-load error.
pub fn label_layout(
    layout: &Layout,
    specs: &[CombineItemSpec],
    options: &CombineOptions,
    fonts: &comic_core::FontSet,
) -> LabelLayout {
    let o = options.normalized();
    let labels = &o.labels;
    let mut result = LabelLayout::default();
    for (index, card) in layout.cards.iter().enumerate() {
        if labels.placement == LabelPlacement::None {
            result.labels.push(None);
            continue;
        }
        let text = specs
            .get(index)
            .map(|spec| resolve_label(spec, index, labels))
            .unwrap_or_default();
        if text.is_empty() {
            result.labels.push(None);
            continue;
        }
        let margin = labels.overlay_margin_px as f64;
        let area = match labels.placement {
            LabelPlacement::Overlay => card.card.inset(
                o.border.width_px as f64 + margin,
                o.border.width_px as f64 + margin,
            ),
            LabelPlacement::Band => card.band.unwrap_or_default().inset(margin, 0.0),
            LabelPlacement::None => unreachable!(),
        };
        let placed = (area.width() > 0.0 && area.height() > 0.0)
            .then(|| fit_label(&text, area, labels, fonts))
            .flatten();
        if placed.is_none() {
            result.not_drawn += 1;
        }
        result.labels.push(placed);
    }
    result
}

fn fit_label(
    text: &str,
    area: Rect,
    o: &LabelOptions,
    fonts: &comic_core::FontSet,
) -> Option<PlacedLabel> {
    let initial = o.text_size_px as f64;
    let min_size = initial * 0.6;
    let fits = |size: f64, text: &str| -> Option<(Rect, f64, f64)> {
        measure(text, size, o, fonts).filter(|&(_, w, h)| w <= area.width() && h <= area.height())
    };
    let mut displayed = text.to_owned();
    let (size, measurement) = if let Some(m) = fits(initial, text) {
        (initial, m)
    } else {
        let minimum = measure(text, min_size, o, fonts)?;
        if minimum.2 > area.height() {
            return None;
        }
        if minimum.1 <= area.width() {
            // Finite bisection of font measurement, not the closed-form image layout.
            // Always retain the measured fitting endpoint (raster metrics are stepped).
            let mut low = min_size;
            let mut high = initial;
            let mut best = minimum;
            for _ in 0..20 {
                let middle = (low + high) * 0.5;
                if let Some(m) = fits(middle, text) {
                    low = middle;
                    best = m;
                } else {
                    high = middle;
                }
            }
            (low, best)
        } else {
            // At the minimum size, shorten only the width. Even the ellipsis must
            // satisfy the decorated height and Pill minimum-width requirements.
            let mut chars: Vec<char> = text.chars().collect();
            let measurement = loop {
                chars.pop()?;
                displayed = chars.iter().chain(std::iter::once(&'…')).collect();
                if let Some(m) = fits(min_size, &displayed) {
                    break m;
                }
            };
            (min_size, measurement)
        }
    };
    let (bounds, w, h) = measurement;
    let (ax, ay) = if o.placement == LabelPlacement::Band {
        (
            match o.band_align {
                HAlign::Left => 0.0,
                HAlign::Center => 0.5,
                HAlign::Right => 1.0,
            },
            0.5,
        )
    } else {
        o.overlay_anchor.fractions()
    };
    let rect = Rect::new(
        area.x + (area.width() - w) * ax,
        area.y + (area.height() - h) * ay,
        w,
        h,
    );
    let text_rect = Rect::new(
        rect.x + (w - bounds.width()) * 0.5,
        rect.y + (h - bounds.height()) * 0.5,
        bounds.width(),
        bounds.height(),
    );
    Some(PlacedLabel {
        text: displayed,
        text_size_px: size,
        rect,
        text_rect,
        text_bounds: bounds,
    })
}

pub(super) fn render_text(
    label: &PlacedLabel,
    o: &LabelOptions,
    fonts: &comic_core::FontSet,
) -> RgbaImage {
    let block = text_block(&label.text, label.text_size_px, o);
    let object = comic_core::AnnotationObject::new_text(
        0,
        (-label.text_bounds.x as f32, -label.text_bounds.y as f32),
        block,
    );
    let (w, h) = (
        label.text_bounds.width() as u32,
        label.text_bounds.height() as u32,
    );
    let overlay = comic_core::bake_overlay(&[object], w as usize, h as usize, fonts);
    RgbaImage::from_raw(w, h, overlay.pixels).expect("comic-core text raster dimensions")
}
