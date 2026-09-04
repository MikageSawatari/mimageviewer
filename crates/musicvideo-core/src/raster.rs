use std::fmt;

use rayon::prelude::*;

use crate::assets::{AssetError, Assets, ImageAsset};
use crate::model::{ImageId, NormRect, Rgba8, TextAlign};
use crate::scene::{Scene, SceneItemKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UnresolvedAsset {
    Image(ImageId),
    Font(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[must_use]
pub struct RenderReport {
    pub unresolved_assets: Vec<UnresolvedAsset>,
}

impl RenderReport {
    fn record_unresolved(&mut self, asset: UnresolvedAsset) {
        if !self.unresolved_assets.contains(&asset) {
            self.unresolved_assets.push(asset);
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RasterError {
    DimensionsOverflow,
    Asset(AssetError),
}

impl fmt::Display for RasterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Self::Asset(error) = self {
            return error.fmt(f);
        }
        f.write_str("frame dimensions overflow address space")
    }
}

impl std::error::Error for RasterError {}

impl From<AssetError> for RasterError {
    fn from(error: AssetError) -> Self {
        Self::Asset(error)
    }
}

pub fn render_frame(
    scene: &Scene,
    assets: &Assets,
    frame: &mut Frame,
) -> Result<RenderReport, RasterError> {
    frame.prepare(scene.width, scene.height)?;
    let mut report = RenderReport::default();
    for item in &scene.items {
        let rect = norm_rect_to_pixels(item.rect, scene.width, scene.height);
        if rect.is_empty() {
            continue;
        }
        match &item.kind {
            SceneItemKind::Solid { color } => fill_rect(frame, rect, *color, item.opacity),
            SceneItemKind::Image { image, fit } => {
                if let Some(image) =
                    assets.scaled_image(image, rect.width() as u32, rect.height() as u32, *fit)?
                {
                    draw_image(frame, rect, &image, item.opacity);
                } else {
                    report.record_unresolved(UnresolvedAsset::Image(image.clone()));
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
            } => {
                if !draw_text(
                    frame,
                    rect,
                    assets,
                    text,
                    font_key,
                    *size_px,
                    *align,
                    *color,
                    item.opacity,
                ) {
                    report.record_unresolved(UnresolvedAsset::Font(font_key.clone()));
                }
            }
        }
    }
    Ok(report)
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

fn draw_image(frame: &mut Frame, rect: PixelRect, image: &ImageAsset, opacity: f32) {
    if rect.is_empty() || opacity <= 0.0 {
        return;
    }
    debug_assert_eq!(image.width as usize, rect.width());
    debug_assert_eq!(image.height as usize, rect.height());
    if rect.x0 == 0
        && rect.y0 == 0
        && rect.x1 == frame.width as usize
        && rect.y1 == frame.height as usize
        && image.opaque
        && opacity >= 1.0
    {
        frame.rgba.copy_from_slice(&image.rgba);
        return;
    }

    let frame_row_bytes = frame.width as usize * 4;
    let image_row_bytes = image.width as usize * 4;
    if image.opaque && opacity >= 1.0 {
        frame
            .rgba
            .par_chunks_exact_mut(frame_row_bytes)
            .enumerate()
            .filter(|(y, _)| *y >= rect.y0 && *y < rect.y1)
            .for_each(|(y, row)| {
                let source_y = y - rect.y0;
                let source =
                    &image.rgba[source_y * image_row_bytes..(source_y + 1) * image_row_bytes];
                row[rect.x0 * 4..rect.x1 * 4].copy_from_slice(source);
            });
        return;
    }

    frame
        .rgba
        .par_chunks_exact_mut(frame_row_bytes)
        .enumerate()
        .filter(|(y, _)| *y >= rect.y0 && *y < rect.y1)
        .for_each(|(y, row)| {
            let source_y = y - rect.y0;
            for x in rect.x0..rect.x1 {
                let source_x = x - rect.x0;
                let source_offset = source_y * image_row_bytes + source_x * 4;
                let pixel = &image.rgba[source_offset..source_offset + 4];
                blend_pixel(
                    &mut row[x * 4..x * 4 + 4],
                    Rgba8::new(pixel[0], pixel[1], pixel[2], pixel[3]),
                    opacity,
                );
            }
        });
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
) -> bool {
    let Some(bitmap) = assets.rasterized_text(text, font_key, size_px, color) else {
        return false;
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
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_SCALED_IMAGE_CACHE_ENTRIES;
    use crate::model::Fit;
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
        assert_eq!(
            render_frame(&scene, &assets, &mut first).unwrap(),
            RenderReport::default()
        );
        assert_eq!(
            render_frame(&scene, &assets, &mut second).unwrap(),
            RenderReport::default()
        );
        assert_eq!(first, second);
    }

    #[test]
    fn solid_layer_lands_on_expected_small_canvas_pixels() {
        let scene = positioned_scene();
        let mut frame = Frame::new(64, 36).unwrap();
        assert_eq!(
            render_frame(&scene, &Assets::default(), &mut frame).unwrap(),
            RenderReport::default()
        );

        assert_eq!(pixel(&frame, 16, 9), &[200, 30, 40, 255]);
        assert_eq!(pixel(&frame, 47, 26), &[200, 30, 40, 255]);
        assert_eq!(pixel(&frame, 15, 9), &[0, 0, 0, 255]);
        assert_eq!(pixel(&frame, 48, 26), &[0, 0, 0, 255]);
        assert!(frame.rgba.chunks_exact(4).all(|pixel| pixel[3] == 255));
    }

    fn image_scene(width: u32, height: u32, id: &str, fit: Fit) -> Scene {
        Scene {
            width,
            height,
            items: vec![SceneItem {
                rect: NormRect::FULL,
                opacity: 1.0,
                kind: SceneItemKind::Image {
                    image: ImageId::new(id),
                    fit,
                },
            }],
        }
    }

    fn patterned_image(width: u32, height: u32, seed: u8) -> ImageAsset {
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&[
                    seed.wrapping_add((x * 17) as u8),
                    seed.wrapping_add((y * 29) as u8),
                    seed.wrapping_add(((x + y) * 11) as u8),
                    255,
                ]);
            }
        }
        ImageAsset::new(width, height, rgba).unwrap()
    }

    #[test]
    fn scaled_image_is_bit_exact_and_reused_after_first_render() {
        let id = ImageId::new("background");
        let scene = image_scene(19, 11, &id.0, Fit::Cover);
        let assets = Assets::default();
        assets.insert_image(id.clone(), patterned_image(31, 17, 7));
        let mut first = Frame::new(1, 1).unwrap();
        let mut second = Frame::new(1, 1).unwrap();

        assert_eq!(
            render_frame(&scene, &assets, &mut first).unwrap(),
            RenderReport::default()
        );
        let after_first = assets.scaled_image_cache_stats();
        assert_eq!(after_first.scale_operations, 1);
        assert_eq!(after_first.misses, 1);

        assert_eq!(
            render_frame(&scene, &assets, &mut second).unwrap(),
            RenderReport::default()
        );
        let after_second = assets.scaled_image_cache_stats();
        assert_eq!(after_second.scale_operations, 1);
        assert_eq!(after_second.hits, 1);
        assert_eq!(first, second);

        let cold_assets = Assets::default();
        cold_assets.insert_image(id, patterned_image(31, 17, 7));
        let mut cold = Frame::new(1, 1).unwrap();
        assert_eq!(
            render_frame(&scene, &cold_assets, &mut cold).unwrap(),
            RenderReport::default()
        );
        assert_eq!(first, cold);
    }

    #[test]
    fn scaled_image_cache_has_a_fixed_entry_bound() {
        let id = ImageId::new("background");
        let assets = Assets::default();
        assets.insert_image(id.clone(), patterned_image(13, 7, 3));
        let mut frame = Frame::new(1, 1).unwrap();
        for index in 0..MAX_SCALED_IMAGE_CACHE_ENTRIES + 3 {
            let scene = image_scene(20 + index as u32, 12, &id.0, Fit::Stretch);
            assert_eq!(
                render_frame(&scene, &assets, &mut frame).unwrap(),
                RenderReport::default()
            );
        }

        let stats = assets.scaled_image_cache_stats();
        assert_eq!(stats.entries, MAX_SCALED_IMAGE_CACHE_ENTRIES);
        assert_eq!(stats.evictions, 3);
    }

    #[test]
    fn scaled_image_cache_key_separates_fit_modes() {
        let id = ImageId::new("background");
        let assets = Assets::default();
        assets.insert_image(id.clone(), patterned_image(8, 4, 11));
        let mut contain = Frame::new(1, 1).unwrap();
        let mut cover = Frame::new(1, 1).unwrap();

        let contain_scene = image_scene(8, 8, &id.0, Fit::Contain);
        let cover_scene = image_scene(8, 8, &id.0, Fit::Cover);
        assert_eq!(
            render_frame(&contain_scene, &assets, &mut contain).unwrap(),
            RenderReport::default()
        );
        assert_eq!(
            render_frame(&cover_scene, &assets, &mut cover).unwrap(),
            RenderReport::default()
        );

        let stats = assets.scaled_image_cache_stats();
        assert_eq!(stats.entries, 2);
        assert_eq!(stats.scale_operations, 2);
        assert_ne!(contain, cover);
    }

    #[test]
    fn replacing_an_image_invalidates_its_scaled_entries() {
        let id = ImageId::new("background");
        let scene = image_scene(19, 11, &id.0, Fit::Cover);
        let assets = Assets::default();
        let mut before = Frame::new(1, 1).unwrap();
        let mut after = Frame::new(1, 1).unwrap();
        assets.insert_image(id.clone(), patterned_image(31, 17, 7));
        assert_eq!(
            render_frame(&scene, &assets, &mut before).unwrap(),
            RenderReport::default()
        );

        assets.insert_image(id, patterned_image(31, 17, 91));
        assert_eq!(assets.scaled_image_cache_stats().entries, 0);
        assert_eq!(
            render_frame(&scene, &assets, &mut after).unwrap(),
            RenderReport::default()
        );

        assert_eq!(assets.scaled_image_cache_stats().scale_operations, 2);
        assert_ne!(before, after);
    }

    #[test]
    fn unresolved_assets_are_reported_without_failing_the_frame() {
        let scene = image_scene(8, 6, "missing-background", Fit::Cover);
        let mut frame = Frame::new(1, 1).unwrap();

        let report = render_frame(&scene, &Assets::default(), &mut frame).unwrap();

        assert_eq!(
            report.unresolved_assets,
            vec![UnresolvedAsset::Image(ImageId::new("missing-background"))]
        );
        assert!(
            frame
                .rgba
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 0, 0, 255])
        );
    }
}
