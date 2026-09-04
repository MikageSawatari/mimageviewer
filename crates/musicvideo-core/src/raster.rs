use std::fmt;

use rayon::prelude::*;

use crate::assets::{Assets, ImageAsset};
use crate::model::{Fit, NormRect, Rgba8, TextAlign};
use crate::scene::{Scene, SceneItemKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl PixelRect {
    pub fn width(self) -> usize {
        self.x1.saturating_sub(self.x0)
    }

    pub fn height(self) -> usize {
        self.y1.saturating_sub(self.y0)
    }

    pub fn is_empty(self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }
}

/// Converts normalized edges independently with nearest-integer rounding, then
/// clips them to the canvas. All layer kinds share this one conversion rule.
pub fn norm_rect_to_pixels(rect: NormRect, width: u32, height: u32) -> PixelRect {
    let edge = |value: f32, extent: u32| {
        let value = if value.is_finite() { value } else { 0.0 };
        (value.clamp(0.0, 1.0) * extent as f32).round() as usize
    };
    let x0 = edge(rect.x, width);
    let y0 = edge(rect.y, height);
    let x1 = edge(rect.x + rect.width.max(0.0), width).max(x0);
    let y1 = edge(rect.y + rect.height.max(0.0), height).max(y0);
    PixelRect { x0, y0, x1, y1 }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    pub fn new(width: u32, height: u32) -> Result<Self, RasterError> {
        let len = frame_len(width, height)?;
        let mut frame = Self {
            width,
            height,
            rgba: vec![0; len],
        };
        frame.clear_black();
        Ok(frame)
    }

    fn prepare(&mut self, width: u32, height: u32) -> Result<(), RasterError> {
        let len = frame_len(width, height)?;
        self.width = width;
        self.height = height;
        self.rgba.resize(len, 0);
        self.clear_black();
        Ok(())
    }

    fn clear_black(&mut self) {
        self.rgba.par_chunks_exact_mut(4).for_each(|pixel| {
            pixel.copy_from_slice(&[0, 0, 0, 255]);
        });
    }
}

fn frame_len(width: u32, height: u32) -> Result<usize, RasterError> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(RasterError::DimensionsOverflow)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RasterError {
    DimensionsOverflow,
}

impl fmt::Display for RasterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("frame dimensions overflow address space")
    }
}

impl std::error::Error for RasterError {}

pub fn render_frame(scene: &Scene, assets: &Assets, frame: &mut Frame) -> Result<(), RasterError> {
    frame.prepare(scene.width, scene.height)?;
    for item in &scene.items {
        let rect = norm_rect_to_pixels(item.rect, scene.width, scene.height);
        if rect.is_empty() {
            continue;
        }
        match &item.kind {
            SceneItemKind::Solid { color } => fill_rect(frame, rect, *color, item.opacity),
            SceneItemKind::Image { image, fit } => {
                if let Some(image) = assets.image(image) {
                    draw_image(frame, rect, &image, *fit, item.opacity);
                }
            }
            SceneItemKind::Spectrum {
                values,
                gap_ratio,
                min_height_px,
                thickness,
                color,
            } => draw_spectrum(
                frame,
                rect,
                values,
                *gap_ratio,
                *min_height_px,
                *thickness,
                *color,
                item.opacity,
            ),
            SceneItemKind::ProgressBar {
                progress,
                border_thickness_px,
                color,
                border_color,
            } => draw_progress_bar(
                frame,
                rect,
                *progress,
                *border_thickness_px,
                *color,
                *border_color,
                item.opacity,
            ),
            SceneItemKind::TimeText {
                text,
                font_key,
                size_px,
                align,
                color,
            } => draw_text(
                frame,
                rect,
                assets,
                text,
                font_key,
                *size_px,
                *align,
                *color,
                item.opacity,
            ),
        }
    }
    Ok(())
}

fn blend_pixel(destination: &mut [u8], source: Rgba8, opacity: f32) {
    let alpha = (source.a as f32 * opacity.clamp(0.0, 1.0)).round() as u32;
    if alpha == 0 {
        return;
    }
    let inverse = 255 - alpha;
    for (channel, source_channel) in destination[..3]
        .iter_mut()
        .zip([source.r, source.g, source.b])
    {
        *channel = ((source_channel as u32 * alpha + *channel as u32 * inverse + 127) / 255) as u8;
    }
    destination[3] = 255;
}

fn fill_rect(frame: &mut Frame, rect: PixelRect, color: Rgba8, opacity: f32) {
    if color.a == 0 || opacity <= 0.0 {
        return;
    }
    if rect.width().saturating_mul(rect.height()) < 32_768 {
        fill_rect_sequential(frame, rect, color, opacity);
        return;
    }
    let row_bytes = frame.width as usize * 4;
    frame
        .rgba
        .par_chunks_exact_mut(row_bytes)
        .enumerate()
        .filter(|(y, _)| *y >= rect.y0 && *y < rect.y1)
        .for_each(|(_, row)| {
            for x in rect.x0..rect.x1 {
                blend_pixel(&mut row[x * 4..x * 4 + 4], color, opacity);
            }
        });
}

fn fill_rect_sequential(frame: &mut Frame, rect: PixelRect, color: Rgba8, opacity: f32) {
    let width = frame.width as usize;
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            let offset = (y * width + x) * 4;
            blend_pixel(&mut frame.rgba[offset..offset + 4], color, opacity);
        }
    }
}

fn draw_image(frame: &mut Frame, rect: PixelRect, image: &ImageAsset, fit: Fit, opacity: f32) {
    if rect.is_empty() || opacity <= 0.0 {
        return;
    }
    if rect.x0 == 0
        && rect.y0 == 0
        && rect.x1 == frame.width as usize
        && rect.y1 == frame.height as usize
        && image.width == frame.width
        && image.height == frame.height
        && image.opaque
        && opacity >= 1.0
    {
        frame.rgba.copy_from_slice(&image.rgba);
        return;
    }

    let destination_w = rect.width() as f32;
    let destination_h = rect.height() as f32;
    let source_w = image.width as f32;
    let source_h = image.height as f32;
    let (draw_rect, origin_x, origin_y, scale_x, scale_y) = match fit {
        Fit::Stretch => (
            rect,
            rect.x0 as f32,
            rect.y0 as f32,
            destination_w / source_w,
            destination_h / source_h,
        ),
        Fit::Contain => {
            let scale = (destination_w / source_w).min(destination_h / source_h);
            let draw_w = (source_w * scale).round().max(1.0) as usize;
            let draw_h = (source_h * scale).round().max(1.0) as usize;
            let x0 = rect.x0 + rect.width().saturating_sub(draw_w) / 2;
            let y0 = rect.y0 + rect.height().saturating_sub(draw_h) / 2;
            let draw = PixelRect {
                x0,
                y0,
                x1: (x0 + draw_w).min(rect.x1),
                y1: (y0 + draw_h).min(rect.y1),
            };
            (
                draw,
                draw.x0 as f32,
                draw.y0 as f32,
                draw.width() as f32 / source_w,
                draw.height() as f32 / source_h,
            )
        }
        Fit::Cover => {
            let scale = (destination_w / source_w).max(destination_h / source_h);
            (
                rect,
                rect.x0 as f32 + (destination_w - source_w * scale) * 0.5,
                rect.y0 as f32 + (destination_h - source_h * scale) * 0.5,
                scale,
                scale,
            )
        }
    };

    let row_bytes = frame.width as usize * 4;
    frame
        .rgba
        .par_chunks_exact_mut(row_bytes)
        .enumerate()
        .filter(|(y, _)| *y >= draw_rect.y0 && *y < draw_rect.y1)
        .for_each(|(y, row)| {
            let source_y = (y as f32 + 0.5 - origin_y) / scale_y - 0.5;
            for x in draw_rect.x0..draw_rect.x1 {
                let source_x = (x as f32 + 0.5 - origin_x) / scale_x - 0.5;
                let pixel = sample_bilinear(image, source_x, source_y);
                blend_pixel(
                    &mut row[x * 4..x * 4 + 4],
                    Rgba8::new(pixel[0], pixel[1], pixel[2], pixel[3]),
                    opacity,
                );
            }
        });
}

fn sample_bilinear(image: &ImageAsset, x: f32, y: f32) -> [u8; 4] {
    let x = x.clamp(0.0, image.width.saturating_sub(1) as f32);
    let y = y.clamp(0.0, image.height.saturating_sub(1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(image.width as usize - 1);
    let y1 = (y0 + 1).min(image.height as usize - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let pixel = |px: usize, py: usize, channel: usize| {
        image.rgba[(py * image.width as usize + px) * 4 + channel] as f32
    };
    let mut result = [0; 4];
    for (channel, output) in result.iter_mut().enumerate() {
        let top = pixel(x0, y0, channel) * (1.0 - tx) + pixel(x1, y0, channel) * tx;
        let bottom = pixel(x0, y1, channel) * (1.0 - tx) + pixel(x1, y1, channel) * tx;
        *output = (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8;
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn draw_spectrum(
    frame: &mut Frame,
    rect: PixelRect,
    values: &[f32],
    gap_ratio: f32,
    min_height_px: f32,
    thickness: f32,
    color: Rgba8,
    opacity: f32,
) {
    if values.is_empty() {
        return;
    }
    let slot_width = rect.width() as f32 / values.len() as f32;
    let available_height = rect.height() as f32;
    let minimum = min_height_px.clamp(0.0, available_height);
    for (index, value) in values.iter().copied().enumerate() {
        let natural_width = slot_width * (1.0 - gap_ratio.clamp(0.0, 1.0));
        let bar_width = if thickness > 0.0 {
            natural_width.min(thickness)
        } else {
            natural_width
        }
        .max(1.0);
        let left =
            rect.x0 as f32 + index as f32 * slot_width + (slot_width - bar_width).max(0.0) * 0.5;
        let right = (left + bar_width).min(rect.x1 as f32);
        let height = (minimum + value.clamp(0.0, 1.0) * (available_height - minimum))
            .round()
            .clamp(0.0, available_height) as usize;
        let bar = PixelRect {
            x0: (left.round() as usize).clamp(rect.x0, rect.x1),
            y0: rect.y1.saturating_sub(height),
            x1: (right.round() as usize).clamp(rect.x0, rect.x1),
            y1: rect.y1,
        };
        if !bar.is_empty() {
            fill_rect(frame, bar, color, opacity);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_progress_bar(
    frame: &mut Frame,
    rect: PixelRect,
    progress: f32,
    border_thickness_px: f32,
    color: Rgba8,
    border_color: Rgba8,
    opacity: f32,
) {
    let border = border_thickness_px
        .round()
        .max(0.0)
        .min((rect.width().min(rect.height()) / 2) as f32) as usize;
    if border > 0 {
        fill_rect(
            frame,
            PixelRect {
                y1: rect.y0 + border,
                ..rect
            },
            border_color,
            opacity,
        );
        fill_rect(
            frame,
            PixelRect {
                y0: rect.y1 - border,
                ..rect
            },
            border_color,
            opacity,
        );
        fill_rect(
            frame,
            PixelRect {
                y0: rect.y0 + border,
                y1: rect.y1 - border,
                x1: rect.x0 + border,
                ..rect
            },
            border_color,
            opacity,
        );
        fill_rect(
            frame,
            PixelRect {
                x0: rect.x1 - border,
                y0: rect.y0 + border,
                y1: rect.y1 - border,
                ..rect
            },
            border_color,
            opacity,
        );
    }
    let inner = PixelRect {
        x0: rect.x0 + border,
        y0: rect.y0 + border,
        x1: rect.x1 - border,
        y1: rect.y1 - border,
    };
    let filled_width = (inner.width() as f32 * progress.clamp(0.0, 1.0)).round() as usize;
    fill_rect(
        frame,
        PixelRect {
            x1: inner.x0 + filled_width,
            ..inner
        },
        color,
        opacity,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    frame: &mut Frame,
    rect: PixelRect,
    assets: &Assets,
    text: &str,
    font_key: &str,
    size_px: f32,
    align: TextAlign,
    color: Rgba8,
    opacity: f32,
) {
    let Some(bitmap) = assets.rasterized_text(text, font_key, size_px, color) else {
        return;
    };
    let logical_x = match align {
        TextAlign::Start => rect.x0 as f32,
        TextAlign::Center => rect.x0 as f32 + (rect.width() as f32 - bitmap.logical_width) * 0.5,
        TextAlign::End => rect.x1 as f32 - bitmap.logical_width,
    };
    let logical_y = rect.y0 as f32 + (rect.height() as f32 - bitmap.logical_height) * 0.5;
    let destination_x = logical_x.round() as i32 - bitmap.padding as i32;
    let destination_y = logical_y.round() as i32 - bitmap.padding as i32;
    let frame_width = frame.width as usize;
    for source_y in 0..bitmap.height {
        let y = destination_y + source_y as i32;
        if y < 0 || y >= frame.height as i32 {
            continue;
        }
        for source_x in 0..bitmap.width {
            let x = destination_x + source_x as i32;
            if x < 0 || x >= frame.width as i32 {
                continue;
            }
            let source_offset = (source_y * bitmap.width + source_x) * 4;
            let source = Rgba8::new(
                bitmap.rgba[source_offset],
                bitmap.rgba[source_offset + 1],
                bitmap.rgba[source_offset + 2],
                bitmap.rgba[source_offset + 3],
            );
            let destination_offset = (y as usize * frame_width + x as usize) * 4;
            blend_pixel(
                &mut frame.rgba[destination_offset..destination_offset + 4],
                source,
                opacity,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::SceneItem;

    #[test]
    fn normalized_rect_rounding_is_fixed_at_boundaries() {
        assert_eq!(
            norm_rect_to_pixels(NormRect::FULL, 64, 36),
            PixelRect {
                x0: 0,
                y0: 0,
                x1: 64,
                y1: 36,
            }
        );
        assert_eq!(
            norm_rect_to_pixels(NormRect::new(1.0, 1.0, 0.0, 0.0), 64, 36),
            PixelRect {
                x0: 64,
                y0: 36,
                x1: 64,
                y1: 36,
            }
        );
        assert_eq!(
            norm_rect_to_pixels(NormRect::new(0.25, 0.25, 0.0, 0.0), 7, 7),
            PixelRect {
                x0: 2,
                y0: 2,
                x1: 2,
                y1: 2,
            }
        );
        assert_eq!(
            norm_rect_to_pixels(NormRect::new(0.25, 0.25, 0.5, 0.5), 7, 7),
            PixelRect {
                x0: 2,
                y0: 2,
                x1: 5,
                y1: 5,
            }
        );
    }

    fn positioned_scene() -> Scene {
        Scene {
            width: 64,
            height: 36,
            items: vec![SceneItem {
                rect: NormRect::new(0.25, 0.25, 0.5, 0.5),
                opacity: 1.0,
                kind: SceneItemKind::Solid {
                    color: Rgba8::new(200, 30, 40, 255),
                },
            }],
        }
    }

    fn pixel(frame: &Frame, x: usize, y: usize) -> &[u8] {
        let offset = (y * frame.width as usize + x) * 4;
        &frame.rgba[offset..offset + 4]
    }

    #[test]
    fn render_frame_is_byte_deterministic() {
        let scene = positioned_scene();
        let assets = Assets::default();
        let mut first = Frame::new(1, 1).unwrap();
        let mut second = Frame::new(80, 40).unwrap();
        render_frame(&scene, &assets, &mut first).unwrap();
        render_frame(&scene, &assets, &mut second).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn solid_layer_lands_on_expected_small_canvas_pixels() {
        let scene = positioned_scene();
        let mut frame = Frame::new(64, 36).unwrap();
        render_frame(&scene, &Assets::default(), &mut frame).unwrap();

        assert_eq!(pixel(&frame, 16, 9), &[200, 30, 40, 255]);
        assert_eq!(pixel(&frame, 47, 26), &[200, 30, 40, 255]);
        assert_eq!(pixel(&frame, 15, 9), &[0, 0, 0, 255]);
        assert_eq!(pixel(&frame, 48, 26), &[0, 0, 0, 255]);
        assert!(frame.rgba.chunks_exact(4).all(|pixel| pixel[3] == 255));
    }
}
