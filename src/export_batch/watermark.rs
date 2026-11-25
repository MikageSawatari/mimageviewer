//! Worker-owned watermark assets. All pixels remain straight RGBA through encoding.
use super::{BatchWatermark, ImageWatermarkSize, WatermarkAnchor, WatermarkKind};
use image::{ImageDecoder, RgbaImage};
use std::borrow::Cow;
use std::path::Path;

pub(super) enum WatermarkAssets {
    None,
    Text(comic_core::FontSet),
    Image {
        original: RgbaImage,
        resized: Option<RgbaImage>,
    },
}

impl WatermarkAssets {
    pub fn load(options: &BatchWatermark) -> Result<Self, String> {
        if !options.enabled {
            return Ok(Self::None);
        }
        match options.kind {
            WatermarkKind::Text => crate::comic_overlay::load_comic_fonts()
                .map(Self::Text)
                .ok_or_else(|| "透かしのフォントを読めません".into()),
            WatermarkKind::Image => {
                let path = options.image_path.as_deref().unwrap_or(Path::new(""));
                let original = load_logo(path).map_err(|error| {
                    format!("透かしの画像を読めません: {}: {error}", path.display())
                })?;
                Ok(Self::Image {
                    original,
                    resized: None,
                })
            }
        }
    }

    pub fn apply(
        &mut self,
        base: &eframe::egui::ColorImage,
        options: &BatchWatermark,
    ) -> Result<Option<Vec<u8>>, String> {
        // Keep the exact legacy path (including its encoded bytes) for opacity zero.
        if matches!(self, Self::None) || options.opacity_percent == 0 {
            return Ok(None);
        }
        let size = base.size.map(|n| n as u32);
        let margin = margin(size, options.margin_percent);
        let overlay = match self {
            Self::None => return Ok(None),
            Self::Text(fonts) => Cow::Owned(render_text(options, size, fonts)),
            Self::Image { original, resized } => {
                let target = logo_size(original.dimensions(), size, margin, options.image_size);
                if target == original.dimensions() {
                    Cow::Borrowed(&*original)
                } else {
                    if resized.as_ref().map(RgbaImage::dimensions) != Some(target) {
                        *resized = Some(resize_logo(original, target)?);
                    }
                    Cow::Borrowed(resized.as_ref().unwrap())
                }
            }
        };
        let mut rgba = crate::save_with_metadata::color_image_to_rgba(base);
        composite(
            &mut rgba,
            size,
            &overlay,
            options.anchor,
            margin,
            options.opacity_percent,
        );
        Ok(Some(rgba))
    }
}

pub(super) fn load_logo(path: &Path) -> Result<RgbaImage, String> {
    if !super::supported_logo_path(path) {
        return Err("この形式は透かしに使えません".into());
    }
    let reader = image::ImageReader::open(path).map_err(|e| e.to_string())?;
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    // JPEG orientation belongs to the decoder, before any size calculation.
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    image.apply_orientation(orientation);
    Ok(image.into_rgba8())
}

fn resize_logo(original: &RgbaImage, size: (u32, u32)) -> Result<RgbaImage, String> {
    use fast_image_resize::{FilterType, ResizeAlg, ResizeOptions, Resizer};
    let mut resized = RgbaImage::new(size.0, size.1);
    let options = ResizeOptions::new()
        .resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3))
        .use_alpha(true);
    Resizer::new()
        .resize(original, &mut resized, &options)
        .map_err(|e| e.to_string())?;
    Ok(resized)
}

fn margin(size: [u32; 2], percent: u32) -> u32 {
    (size[0].min(size[1]) as f64 * percent.min(20) as f64 / 100.0).round() as u32
}

fn logo_size(
    original: (u32, u32),
    size: [u32; 2],
    margin: u32,
    mode: ImageWatermarkSize,
) -> (u32, u32) {
    let factor = match mode {
        ImageWatermarkSize::Original => 1.0,
        ImageWatermarkSize::PercentOfWidth(percent) => {
            size[0] as f64 * percent.clamp(1, 100) as f64 / 100.0 / original.0 as f64
        }
    };
    let inner = size.map(|n| n.saturating_sub(2 * margin).max(1));
    let factor = factor
        .min(inner[0] as f64 / original.0 as f64)
        .min(inner[1] as f64 / original.1 as f64);
    (
        (original.0 as f64 * factor).round().max(1.0) as u32,
        (original.1 as f64 * factor).round().max(1.0) as u32,
    )
}

fn text_block(options: &BatchWatermark, size: [u32; 2]) -> comic_core::TextBlock {
    let size_px = (size[0].min(size[1]) as f32 * options.text_size_percent.clamp(1, 50) as f32
        / 100.0)
        .max(1.0);
    let [r, g, b] = options.text_color;
    comic_core::TextBlock {
        text: options.text.replace(['\r', '\n'], " "),
        font_key: crate::comic_overlay::COMIC_FONT_KEY.into(),
        size_px,
        color: comic_core::Rgba::new(r, g, b, 255),
        outline: options.text_outline.then_some(comic_core::StrokeStyle {
            color: comic_core::Rgba::BLACK,
            width_px: (size_px / 16.0).max(1.0),
        }),
        ..Default::default()
    }
}

fn render_text(options: &BatchWatermark, size: [u32; 2], fonts: &comic_core::FontSet) -> RgbaImage {
    let block = text_block(options, size);
    let font = fonts
        .get(&block.font_key)
        .expect("loaded default annotation font");
    let (w, h) = comic_core::layout_text(&block, font).bounds;
    // Allow negative glyph bearings and the outline; trim to actual ink below.
    let padding = block.size_px.ceil() + comic_core::text_effect_padding(&block).ceil();
    let width = (w.ceil() + 2.0 * padding).max(1.0) as usize;
    let height = (h.ceil() + 2.0 * padding).max(1.0) as usize;
    let object = comic_core::AnnotationObject::new_text(0, (padding, padding), block);
    let overlay = comic_core::bake_overlay(&[object], width, height, fonts);
    let image = RgbaImage::from_raw(width as u32, height as u32, overlay.pixels).unwrap();
    let mut bounds = (image.width(), image.height(), 0, 0);
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] != 0 {
            bounds.0 = bounds.0.min(x);
            bounds.1 = bounds.1.min(y);
            bounds.2 = bounds.2.max(x + 1);
            bounds.3 = bounds.3.max(y + 1);
        }
    }
    if bounds.2 == 0 {
        return RgbaImage::new(1, 1);
    }
    image::imageops::crop_imm(
        &image,
        bounds.0,
        bounds.1,
        bounds.2 - bounds.0,
        bounds.3 - bounds.1,
    )
    .to_image()
}

fn offset(size: u32, overlay: u32, axis: u32, margin: u32) -> i64 {
    match axis {
        0 => margin as i64,
        1 => (size as i64 - overlay as i64) / 2,
        _ => size as i64 - overlay as i64 - margin as i64,
    }
}

fn composite(
    base: &mut [u8],
    size: [u32; 2],
    overlay: &RgbaImage,
    anchor: WatermarkAnchor,
    margin: u32,
    opacity: u32,
) {
    let (ax, ay) = anchor.axes();
    let ox = offset(size[0], overlay.width(), ax, margin);
    let oy = offset(size[1], overlay.height(), ay, margin);
    for (x, y, src) in overlay.enumerate_pixels() {
        let (x, y) = (ox + x as i64, oy + y as i64);
        if x < 0 || y < 0 || x >= size[0] as i64 || y >= size[1] as i64 {
            continue;
        }
        let a = src[3] as f64 / 255.0 * opacity.min(100) as f64 / 100.0;
        if a == 0.0 {
            continue;
        }
        let index = (y as usize * size[0] as usize + x as usize) * 4;
        let dst = &mut base[index..index + 4];
        let da = dst[3] as f64 / 255.0;
        let out_a = a + da * (1.0 - a);
        for channel in 0..3 {
            dst[channel] = ((src[channel] as f64 * a + dst[channel] as f64 * da * (1.0 - a))
                / out_a)
                .round() as u8;
        }
        dst[3] = (out_a * 255.0).round() as u8;
    }
}

#[cfg(test)]
mod tests;
