use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;

use rayon::prelude::*;

use crate::assets::{AssetError, Assets, ImageAsset};
use crate::model::{
    Fill, Fit, Glow, ImageId, NormRect, Rgba8, SpectrumDirection, SpectrumShape, TextAlign,
};
use crate::scene::{Scene, SceneItem, SceneItemKind};

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
        let mut frame = Self {
            width,
            height,
            rgba: vec![0; frame_len(width, height)?],
        };
        frame.clear_black();
        Ok(frame)
    }

    fn prepare(&mut self, width: u32, height: u32, clear: bool) -> Result<(), RasterError> {
        self.width = width;
        self.height = height;
        self.rgba.resize(frame_len(width, height)?, 0);
        if clear {
            self.clear_black();
        }
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
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
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
    let prepared_cover = opaque_cover_image(scene, assets)?;
    let starts_with_opaque_solid = scene_starts_with_opaque_solid(scene);
    frame.prepare(
        scene.width,
        scene.height,
        prepared_cover.is_none() && !starts_with_opaque_solid,
    )?;
    let mut report = RenderReport::default();
    for (index, item) in scene.items.iter().enumerate() {
        let rect = norm_rect_to_pixels(item.rect, scene.width, scene.height);
        if rect.is_empty() {
            continue;
        }
        if index == 0
            && let Some(image) = &prepared_cover
        {
            draw_image(frame, rect, image, 1.0, 0.0, false);
            continue;
        }
        let rotation = normalized_rotation(item.rotation_deg);
        let glow = item
            .glow
            .filter(|glow| glow.radius_px > 0.0 && glow.color.a > 0);
        if rotation.abs() > 1.0e-4 {
            draw_rotated_item(frame, rect, item, assets, &mut report, glow)?;
        } else {
            if let Some(glow) = glow {
                let mask = render_item_mask(rect, item, assets, &mut report)?;
                draw_glow(frame, &mask, glow)?;
                if composite_solid_body(frame, &mask, item) {
                    continue;
                }
            }
            draw_item(frame, rect, item, assets, &mut report, false)?;
        }
    }
    Ok(report)
}

fn opaque_cover_image(
    scene: &Scene,
    assets: &Assets,
) -> Result<Option<Arc<ImageAsset>>, RasterError> {
    let Some(item) = scene.items.first() else {
        return Ok(None);
    };
    if item.opacity < 1.0
        || normalized_rotation(item.rotation_deg).abs() > 1.0e-4
        || item.corner_radius_px > 0.0
        || item
            .glow
            .is_some_and(|glow| glow.radius_px > 0.0 && glow.color.a > 0)
        || norm_rect_to_pixels(item.rect, scene.width, scene.height)
            != (PixelRect {
                x0: 0,
                y0: 0,
                x1: scene.width as usize,
                y1: scene.height as usize,
            })
    {
        return Ok(None);
    }
    match &item.kind {
        SceneItemKind::Image { image, fit } => {
            if !matches!(fit, Fit::Stretch | Fit::Cover) {
                return Ok(None);
            }
            let prepared = assets.scaled_image(image, scene.width, scene.height, *fit)?;
            Ok(prepared.filter(|image| image.opaque))
        }
        _ => Ok(None),
    }
}

fn scene_starts_with_opaque_solid(scene: &Scene) -> bool {
    let Some(item) = scene.items.first() else {
        return false;
    };
    if item.opacity < 1.0
        || normalized_rotation(item.rotation_deg).abs() > 1.0e-4
        || item.corner_radius_px > 0.0
        || item
            .glow
            .is_some_and(|glow| glow.radius_px > 0.0 && glow.color.a > 0)
        || norm_rect_to_pixels(item.rect, scene.width, scene.height)
            != (PixelRect {
                x0: 0,
                y0: 0,
                x1: scene.width as usize,
                y1: scene.height as usize,
            })
    {
        return false;
    }
    match &item.kind {
        SceneItemKind::Solid {
            fill: Fill::Solid(color),
        } => color.a == 255,
        _ => false,
    }
}

fn composite_solid_body(frame: &mut Frame, mask: &MaskSurface, item: &SceneItem) -> bool {
    let color = match &item.kind {
        SceneItemKind::Solid {
            fill: Fill::Solid(color),
        }
        | SceneItemKind::Spectrum {
            fill: Fill::Solid(color),
            ..
        }
        | SceneItemKind::TimeText {
            fill: Fill::Solid(color),
            ..
        } => *color,
        SceneItemKind::ProgressBar {
            fill: Fill::Solid(fill),
            border_fill: Fill::Solid(border),
            ..
        } if fill == border => *fill,
        _ => return false,
    };
    let frame_width = frame.width as usize;
    frame
        .rgba
        .par_chunks_exact_mut(frame_width * 4)
        .enumerate()
        .for_each(|(global_y, row)| {
            let local_y = global_y as i32 - mask.origin_y;
            if local_y < 0 || local_y >= mask.height as i32 {
                return;
            }
            for x in 0..mask.width {
                let alpha = mask.alpha[local_y as usize * mask.width + x];
                let global_x = mask.origin_x + x as i32;
                if alpha == 0 || global_x < 0 || global_x >= frame_width as i32 {
                    continue;
                }
                let source = Rgba8::new(color.r, color.g, color.b, alpha);
                let offset = global_x as usize * 4;
                blend_pixel(&mut row[offset..offset + 4], source, 1.0);
            }
        });
    true
}

fn normalized_rotation(rotation_deg: f32) -> f32 {
    if rotation_deg.is_finite() {
        (rotation_deg + 180.0).rem_euclid(360.0) - 180.0
    } else {
        0.0
    }
}

fn draw_item(
    frame: &mut Frame,
    rect: PixelRect,
    item: &SceneItem,
    assets: &Assets,
    report: &mut RenderReport,
    mask_mode: bool,
) -> Result<(), RasterError> {
    match &item.kind {
        SceneItemKind::Solid { fill } => draw_filled_rect(
            frame,
            rect,
            fill,
            item.opacity,
            item.corner_radius_px,
            mask_mode,
        ),
        SceneItemKind::Image { image, fit } => {
            if let Some(image_asset) =
                assets.scaled_image(image, rect.width() as u32, rect.height() as u32, *fit)?
            {
                draw_image(
                    frame,
                    rect,
                    &image_asset,
                    item.opacity,
                    item.corner_radius_px,
                    mask_mode,
                );
            } else {
                report.record_unresolved(UnresolvedAsset::Image(image.clone()));
            }
        }
        SceneItemKind::Spectrum {
            values,
            point_count,
            shape,
            fill,
        } => draw_spectrum(
            frame,
            rect,
            values,
            *point_count,
            *shape,
            fill,
            item.opacity,
            item.corner_radius_px,
            mask_mode,
        ),
        SceneItemKind::ProgressBar {
            progress,
            border_thickness_px,
            fill,
            border_fill,
        } => draw_progress_bar(
            frame,
            rect,
            *progress,
            *border_thickness_px,
            fill,
            border_fill,
            item.opacity,
            item.corner_radius_px,
            mask_mode,
        ),
        SceneItemKind::TimeText {
            text,
            font_key,
            size_px,
            align,
            fill,
        } => {
            if !draw_text(
                frame,
                rect,
                assets,
                text,
                font_key,
                *size_px,
                *align,
                fill,
                item.opacity,
                item.corner_radius_px,
                mask_mode,
            ) {
                report.record_unresolved(UnresolvedAsset::Font(font_key.clone()));
            }
        }
    }
    Ok(())
}

fn blend_pixel(destination: &mut [u8], source: Rgba8, opacity: f32) {
    let alpha = (source.a as f32 * opacity.clamp(0.0, 1.0)).round() as u32;
    if alpha == 0 {
        return;
    }
    if alpha >= 255 {
        destination.copy_from_slice(&[source.r, source.g, source.b, 255]);
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

struct FillSampler<'a> {
    fill: &'a Fill,
    rect: PixelRect,
    linear: (f32, f32, f32),
}

impl<'a> FillSampler<'a> {
    fn new(fill: &'a Fill, rect: PixelRect) -> Self {
        let linear = if let Fill::LinearGradient { angle_deg, .. } = fill {
            let radians = angle_deg.to_radians();
            let dx = radians.cos();
            let dy = radians.sin();
            (dx, dy, (dx.abs() + dy.abs()).max(1.0e-6))
        } else {
            (1.0, 0.0, 1.0)
        };
        Self { fill, rect, linear }
    }

    fn sample(&self, x: f32, y: f32, band_position: Option<f32>, mask: bool) -> Rgba8 {
        let width = self.rect.width().max(1) as f32;
        let height = self.rect.height().max(1) as f32;
        let nx = (x - self.rect.x0 as f32) / width;
        let ny = (y - self.rect.y0 as f32) / height;
        let color = match self.fill {
            Fill::Solid(color) => *color,
            Fill::LinearGradient { stops, .. } => {
                let (dx, dy, extent) = self.linear;
                stops.sample(0.5 + ((nx - 0.5) * dx + (ny - 0.5) * dy) / extent)
            }
            Fill::RadialGradient {
                stops,
                center,
                radius,
            } => {
                let rx = (nx - center.0) * width;
                let ry = (ny - center.1) * height;
                let radius_px = (radius.max(1.0e-6) * width.min(height)).max(1.0e-6);
                stops.sample(rx.hypot(ry) / radius_px)
            }
            Fill::BandGradient { stops } => stops.sample(band_position.unwrap_or(nx)),
        };
        if mask {
            Rgba8::new(255, 255, 255, color.a)
        } else {
            color
        }
    }
}

fn fill_rect(frame: &mut Frame, rect: PixelRect, color: Rgba8, opacity: f32) {
    if color.a == 0 || opacity <= 0.0 || rect.is_empty() {
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

fn draw_filled_rect(
    frame: &mut Frame,
    rect: PixelRect,
    fill: &Fill,
    opacity: f32,
    corner_radius_px: f32,
    mask_mode: bool,
) {
    if let Fill::Solid(color) = fill
        && corner_radius_px <= 0.0
    {
        let color = if mask_mode {
            Rgba8::new(255, 255, 255, color.a)
        } else {
            *color
        };
        fill_rect(frame, rect, color, opacity);
        return;
    }
    let sampler = FillSampler::new(fill, rect);
    let row_bytes = frame.width as usize * 4;
    frame
        .rgba
        .par_chunks_exact_mut(row_bytes)
        .enumerate()
        .filter(|(y, _)| *y >= rect.y0 && *y < rect.y1)
        .for_each(|(y, row)| {
            for x in rect.x0..rect.x1 {
                let coverage =
                    rounded_rect_coverage(x as f32 + 0.5, y as f32 + 0.5, rect, corner_radius_px);
                if coverage > 0.0 {
                    let color = sampler.sample(x as f32 + 0.5, y as f32 + 0.5, None, mask_mode);
                    blend_pixel(&mut row[x * 4..x * 4 + 4], color, opacity * coverage);
                }
            }
        });
}

fn rounded_rect_coverage(x: f32, y: f32, rect: PixelRect, radius_px: f32) -> f32 {
    let width = rect.width() as f32;
    let height = rect.height() as f32;
    let radius = radius_px.max(0.0).min(width.min(height) * 0.5);
    if radius <= 0.0 {
        return 1.0;
    }
    let center_x = (rect.x0 + rect.x1) as f32 * 0.5;
    let center_y = (rect.y0 + rect.y1) as f32 * 0.5;
    let qx = (x - center_x).abs() - (width * 0.5 - radius);
    let qy = (y - center_y).abs() - (height * 0.5 - radius);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    let inside = qx.max(qy).min(0.0);
    (0.5 - (outside + inside - radius)).clamp(0.0, 1.0)
}

fn draw_image(
    frame: &mut Frame,
    rect: PixelRect,
    image: &ImageAsset,
    opacity: f32,
    corner_radius_px: f32,
    mask_mode: bool,
) {
    if rect.is_empty() || opacity <= 0.0 {
        return;
    }
    debug_assert_eq!(image.width as usize, rect.width());
    debug_assert_eq!(image.height as usize, rect.height());
    if !mask_mode
        && corner_radius_px <= 0.0
        && rect.x0 == 0
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
    if !mask_mode && corner_radius_px <= 0.0 && image.opaque && opacity >= 1.0 {
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
                let offset = source_y * image_row_bytes + source_x * 4;
                let coverage =
                    rounded_rect_coverage(x as f32 + 0.5, y as f32 + 0.5, rect, corner_radius_px);
                let color = if mask_mode {
                    Rgba8::new(255, 255, 255, image.rgba[offset + 3])
                } else {
                    Rgba8::new(
                        image.rgba[offset],
                        image.rgba[offset + 1],
                        image.rgba[offset + 2],
                        image.rgba[offset + 3],
                    )
                };
                blend_pixel(&mut row[x * 4..x * 4 + 4], color, opacity * coverage);
            }
        });
}

#[derive(Clone, Copy)]
struct SpectrumBar {
    start: (f32, f32),
    end: (f32, f32),
    thickness: f32,
    band_position: f32,
    round_caps: bool,
}

fn resample_spectrum(values: &[f32], point_count: usize) -> Vec<f32> {
    let point_count = point_count.max(1);
    if values.is_empty() {
        return vec![0.0; point_count];
    }
    if values.len() == 1 || point_count == 1 {
        return vec![values[0].clamp(0.0, 1.0); point_count];
    }
    (0..point_count)
        .map(|index| {
            let position = index as f32 / (point_count - 1) as f32;
            let source = position * (values.len() - 1) as f32;
            let left = source.floor() as usize;
            let right = (left + 1).min(values.len() - 1);
            let amount = source - left as f32;
            (values[left] + (values[right] - values[left]) * amount).clamp(0.0, 1.0)
        })
        .collect()
}

fn spectrum_bars(
    rect: PixelRect,
    values: &[f32],
    point_count: usize,
    shape: SpectrumShape,
) -> Vec<SpectrumBar> {
    let values = resample_spectrum(values, point_count);
    let count = values.len();
    match shape {
        SpectrumShape::Bars {
            gap_ratio,
            min_height_px,
            thickness_px,
        } => {
            let slot = rect.width() as f32 / count as f32;
            let natural_width = slot * (1.0 - gap_ratio.clamp(0.0, 1.0));
            let width = if thickness_px > 0.0 {
                natural_width.min(thickness_px)
            } else {
                natural_width
            }
            .max(1.0);
            let available = rect.height() as f32;
            let minimum = min_height_px.clamp(0.0, available);
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    let x = rect.x0 as f32 + (index as f32 + 0.5) * slot;
                    let height = minimum + value * (available - minimum);
                    SpectrumBar {
                        start: (x, rect.y1 as f32),
                        end: (x, rect.y1 as f32 - height),
                        thickness: width,
                        band_position: band_position(index, count),
                        round_caps: false,
                    }
                })
                .collect()
        }
        SpectrumShape::Radial {
            radius_px,
            thickness_px,
            start_angle_deg,
            sweep_deg,
            direction,
        } => radial_spectrum_bars(
            rect,
            &values,
            radius_px,
            thickness_px,
            start_angle_deg,
            sweep_deg,
            direction,
        ),
    }
}

fn band_position(index: usize, count: usize) -> f32 {
    if count <= 1 {
        0.0
    } else {
        index as f32 / (count - 1) as f32
    }
}

#[allow(clippy::too_many_arguments)]
fn radial_spectrum_bars(
    rect: PixelRect,
    values: &[f32],
    radius_px: f32,
    thickness_px: f32,
    start_angle_deg: f32,
    sweep_deg: f32,
    direction: SpectrumDirection,
) -> Vec<SpectrumBar> {
    let count = values.len();
    let center = (
        (rect.x0 + rect.x1) as f32 * 0.5,
        (rect.y0 + rect.y1) as f32 * 0.5,
    );
    let thickness = thickness_px.max(1.0);
    let outer_radius = rect.width().min(rect.height()) as f32 * 0.5;
    let radius = radius_px.clamp(0.0, (outer_radius - thickness * 0.5).max(0.0));
    let available = (outer_radius - radius - thickness * 0.5).max(0.0);
    let closed = sweep_deg.abs() >= 359.999;
    values
        .iter()
        .copied()
        .enumerate()
        .map(|(index, value)| {
            let position = band_position(index, count);
            let angular_position = if closed && count > 0 {
                index as f32 / count as f32
            } else {
                position
            };
            let direction_sign = match direction {
                SpectrumDirection::Clockwise => 1.0,
                SpectrumDirection::CounterClockwise => -1.0,
            };
            let angle =
                (start_angle_deg + sweep_deg * angular_position * direction_sign).to_radians();
            let unit = (angle.cos(), angle.sin());
            let length = (thickness + value * (available - thickness).max(0.0)).min(available);
            SpectrumBar {
                start: (center.0 + unit.0 * radius, center.1 + unit.1 * radius),
                end: (
                    center.0 + unit.0 * (radius + length),
                    center.1 + unit.1 * (radius + length),
                ),
                thickness,
                band_position: position,
                round_caps: true,
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn draw_spectrum(
    frame: &mut Frame,
    rect: PixelRect,
    values: &[f32],
    point_count: usize,
    shape: SpectrumShape,
    fill: &Fill,
    opacity: f32,
    corner_radius_px: f32,
    mask_mode: bool,
) {
    let bars = spectrum_bars(rect, values, point_count, shape);
    let sampler = FillSampler::new(fill, rect);
    for bar in bars {
        draw_spectrum_bar(
            frame,
            rect,
            bar,
            &sampler,
            opacity,
            corner_radius_px,
            mask_mode,
        );
    }
}

fn draw_spectrum_bar(
    frame: &mut Frame,
    clip_rect: PixelRect,
    bar: SpectrumBar,
    sampler: &FillSampler,
    opacity: f32,
    corner_radius_px: f32,
    mask_mode: bool,
) {
    let pad = bar.thickness * 0.5 + 1.0;
    let min_x = (bar.start.0.min(bar.end.0) - pad)
        .floor()
        .max(clip_rect.x0 as f32) as usize;
    let min_y = (bar.start.1.min(bar.end.1) - pad)
        .floor()
        .max(clip_rect.y0 as f32) as usize;
    let max_x = (bar.start.0.max(bar.end.0) + pad)
        .ceil()
        .min(clip_rect.x1 as f32) as usize;
    let max_y = (bar.start.1.max(bar.end.1) + pad)
        .ceil()
        .min(clip_rect.y1 as f32) as usize;
    let width = frame.width as usize;
    for y in min_y..max_y {
        for x in min_x..max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let coverage = bar_coverage(px, py, bar)
                * rounded_rect_coverage(px, py, clip_rect, corner_radius_px);
            if coverage <= 0.0 {
                continue;
            }
            let color = sampler.sample(px, py, Some(bar.band_position), mask_mode);
            let offset = (y * width + x) * 4;
            blend_pixel(
                &mut frame.rgba[offset..offset + 4],
                color,
                opacity * coverage,
            );
        }
    }
}

fn bar_coverage(x: f32, y: f32, bar: SpectrumBar) -> f32 {
    let vx = bar.end.0 - bar.start.0;
    let vy = bar.end.1 - bar.start.1;
    let length = vx.hypot(vy);
    let radius = bar.thickness * 0.5;
    let distance = if length <= 1.0e-6 {
        (x - bar.start.0).hypot(y - bar.start.1) - radius
    } else if bar.round_caps {
        let projection = ((x - bar.start.0) * vx + (y - bar.start.1) * vy) / (length * length);
        if projection <= 0.0 {
            (x - bar.start.0).hypot(y - bar.start.1) - radius
        } else if projection >= 1.0 {
            (x - bar.end.0).hypot(y - bar.end.1) - radius
        } else {
            ((x - bar.start.0) * vy - (y - bar.start.1) * vx).abs() / length - radius
        }
    } else {
        let center_x = (bar.start.0 + bar.end.0) * 0.5;
        let center_y = (bar.start.1 + bar.end.1) * 0.5;
        let along = ((x - center_x) * vx + (y - center_y) * vy).abs() / length;
        let across = ((x - center_x) * -vy + (y - center_y) * vx).abs() / length;
        let qx = along - length * 0.5;
        let qy = across - radius;
        let outside_x = qx.max(0.0);
        let outside_y = qy.max(0.0);
        let outside = if outside_x == 0.0 {
            outside_y
        } else if outside_y == 0.0 {
            outside_x
        } else {
            outside_x.hypot(outside_y)
        };
        outside + qx.max(qy).min(0.0)
    };
    (0.5 - distance).clamp(0.0, 1.0)
}

#[allow(clippy::too_many_arguments)]
fn draw_progress_bar(
    frame: &mut Frame,
    rect: PixelRect,
    progress: f32,
    border_thickness_px: f32,
    fill: &Fill,
    border_fill: &Fill,
    opacity: f32,
    corner_radius_px: f32,
    mask_mode: bool,
) {
    let border = border_thickness_px
        .max(0.0)
        .min(rect.width().min(rect.height()) as f32 * 0.5);
    let inset = border.ceil() as usize;
    let inner = PixelRect {
        x0: (rect.x0 + inset).min(rect.x1),
        y0: (rect.y0 + inset).min(rect.y1),
        x1: rect.x1.saturating_sub(inset).max(rect.x0),
        y1: rect.y1.saturating_sub(inset).max(rect.y0),
    };
    let fill_edge = inner.x0 as f32 + inner.width() as f32 * progress.clamp(0.0, 1.0);
    let fill_sampler = FillSampler::new(fill, rect);
    let border_sampler = FillSampler::new(border_fill, rect);
    let row_bytes = frame.width as usize * 4;
    frame
        .rgba
        .par_chunks_exact_mut(row_bytes)
        .enumerate()
        .filter(|(y, _)| *y >= rect.y0 && *y < rect.y1)
        .for_each(|(y, row)| {
            for x in rect.x0..rect.x1 {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let outer = rounded_rect_coverage(px, py, rect, corner_radius_px);
                if outer <= 0.0 {
                    continue;
                }
                let inner_coverage = if border > 0.0 && !inner.is_empty() {
                    rounded_rect_coverage(px, py, inner, (corner_radius_px - border).max(0.0))
                } else {
                    outer
                };
                let offset = x * 4;
                let border_coverage = (outer - inner_coverage).max(0.0);
                if border_coverage > 0.0 {
                    let color = border_sampler.sample(px, py, None, mask_mode);
                    blend_pixel(
                        &mut row[offset..offset + 4],
                        color,
                        opacity * border_coverage,
                    );
                }
                let progress_coverage = (fill_edge + 0.5 - px).clamp(0.0, 1.0);
                let coverage = inner_coverage * progress_coverage;
                if coverage > 0.0 {
                    let color = fill_sampler.sample(px, py, None, mask_mode);
                    blend_pixel(&mut row[offset..offset + 4], color, opacity * coverage);
                }
            }
        });
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
    fill: &Fill,
    opacity: f32,
    corner_radius_px: f32,
    mask_mode: bool,
) -> bool {
    let Some(bitmap) = assets.rasterized_text(text, font_key, size_px, Rgba8::WHITE) else {
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
    let sampler = FillSampler::new(fill, rect);
    let frame_width = frame.width as usize;
    for source_y in 0..bitmap.height {
        let y = destination_y + source_y as i32;
        if y < rect.y0 as i32 || y >= rect.y1 as i32 {
            continue;
        }
        for source_x in 0..bitmap.width {
            let x = destination_x + source_x as i32;
            if x < rect.x0 as i32 || x >= rect.x1 as i32 {
                continue;
            }
            let source_offset = (source_y * bitmap.width + source_x) * 4;
            let glyph_alpha = bitmap.rgba[source_offset + 3] as f32 / 255.0;
            if glyph_alpha <= 0.0 {
                continue;
            }
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let clip = rounded_rect_coverage(px, py, rect, corner_radius_px);
            let color = sampler.sample(px, py, None, mask_mode);
            let destination_offset = (y as usize * frame_width + x as usize) * 4;
            blend_pixel(
                &mut frame.rgba[destination_offset..destination_offset + 4],
                color,
                opacity * glyph_alpha * clip,
            );
        }
    }
    true
}

struct MaskSurface {
    origin_x: i32,
    origin_y: i32,
    width: usize,
    height: usize,
    alpha: Vec<u8>,
}

fn render_item_mask(
    rect: PixelRect,
    item: &SceneItem,
    assets: &Assets,
    report: &mut RenderReport,
) -> Result<MaskSurface, RasterError> {
    let mut alpha = vec![0; rect.width() * rect.height()];
    match &item.kind {
        SceneItemKind::Spectrum {
            values,
            point_count,
            shape,
            fill,
        } => draw_spectrum_mask(
            &mut alpha,
            rect.width(),
            rect.height(),
            values,
            *point_count,
            *shape,
            fill,
            item.opacity,
            item.corner_radius_px,
        ),
        SceneItemKind::ProgressBar {
            progress,
            border_thickness_px,
            fill,
            border_fill,
        } => draw_progress_mask(
            &mut alpha,
            rect.width(),
            rect.height(),
            *progress,
            *border_thickness_px,
            fill,
            border_fill,
            item.opacity,
            item.corner_radius_px,
        ),
        SceneItemKind::TimeText {
            text,
            font_key,
            size_px,
            align,
            fill,
        } => {
            if !draw_text_mask(
                &mut alpha,
                rect.width(),
                rect.height(),
                assets,
                text,
                font_key,
                *size_px,
                *align,
                fill,
                item.opacity,
                item.corner_radius_px,
            ) {
                report.record_unresolved(UnresolvedAsset::Font(font_key.clone()));
            }
        }
        _ => return render_item_mask_fallback(rect, item, assets, report),
    }
    Ok(MaskSurface {
        origin_x: rect.x0 as i32,
        origin_y: rect.y0 as i32,
        width: rect.width(),
        height: rect.height(),
        alpha,
    })
}

fn render_item_mask_fallback(
    rect: PixelRect,
    item: &SceneItem,
    assets: &Assets,
    report: &mut RenderReport,
) -> Result<MaskSurface, RasterError> {
    let mut mask_frame = Frame::new(rect.width() as u32, rect.height() as u32)?;
    let local_rect = PixelRect {
        x0: 0,
        y0: 0,
        x1: rect.width(),
        y1: rect.height(),
    };
    draw_item(&mut mask_frame, local_rect, item, assets, report, true)?;
    let alpha = mask_frame
        .rgba
        .chunks_exact(4)
        .map(|pixel| pixel[0])
        .collect();
    Ok(MaskSurface {
        origin_x: rect.x0 as i32,
        origin_y: rect.y0 as i32,
        width: rect.width(),
        height: rect.height(),
        alpha,
    })
}

fn blend_mask_pixel(destination: &mut u8, source_alpha: u8, opacity: f32) {
    if source_alpha == 255 && opacity >= 1.0 {
        *destination = 255;
        return;
    }
    let source = (source_alpha as f32 * opacity.clamp(0.0, 1.0)).round() as u32;
    let destination_alpha = *destination as u32;
    *destination = (source + (destination_alpha * (255 - source) + 127) / 255).min(255) as u8;
}

#[allow(clippy::too_many_arguments)]
fn draw_spectrum_mask(
    alpha: &mut [u8],
    width: usize,
    height: usize,
    values: &[f32],
    point_count: usize,
    shape: SpectrumShape,
    fill: &Fill,
    opacity: f32,
    corner_radius_px: f32,
) {
    let rect = PixelRect {
        x0: 0,
        y0: 0,
        x1: width,
        y1: height,
    };
    let bars = spectrum_bars(rect, values, point_count, shape);
    let sampler = FillSampler::new(fill, rect);
    let solid_alpha = match fill {
        Fill::Solid(color) => Some(color.a),
        _ => None,
    };
    let operations = bars
        .par_iter()
        .map(|bar| {
            let bar = *bar;
            let mut operations = Vec::new();
            let pad = bar.thickness * 0.5 + 1.0;
            let min_x = (bar.start.0.min(bar.end.0) - pad).floor().max(0.0) as usize;
            let min_y = (bar.start.1.min(bar.end.1) - pad).floor().max(0.0) as usize;
            let max_x = (bar.start.0.max(bar.end.0) + pad).ceil().min(width as f32) as usize;
            let max_y = (bar.start.1.max(bar.end.1) + pad).ceil().min(height as f32) as usize;
            for y in min_y..max_y {
                for x in min_x..max_x {
                    let px = x as f32 + 0.5;
                    let py = y as f32 + 0.5;
                    let coverage = bar_coverage(px, py, bar)
                        * rounded_rect_coverage(px, py, rect, corner_radius_px);
                    if coverage <= 0.0 {
                        continue;
                    }
                    let source_alpha = solid_alpha
                        .unwrap_or_else(|| sampler.sample(px, py, Some(bar.band_position), true).a);
                    let alpha = (source_alpha as f32 * opacity * coverage)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                    if alpha > 0 {
                        operations.push((y * width + x, alpha));
                    }
                }
            }
            operations
        })
        .collect::<Vec<_>>();
    for bar_operations in operations {
        for (offset, source) in bar_operations {
            let destination = alpha[offset] as u32;
            alpha[offset] =
                (source as u32 + (destination * (255 - source as u32) + 127) / 255).min(255) as u8;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_progress_mask(
    alpha: &mut [u8],
    width: usize,
    height: usize,
    progress: f32,
    border_thickness_px: f32,
    fill: &Fill,
    border_fill: &Fill,
    opacity: f32,
    corner_radius_px: f32,
) {
    let rect = PixelRect {
        x0: 0,
        y0: 0,
        x1: width,
        y1: height,
    };
    let border = border_thickness_px
        .max(0.0)
        .min(width.min(height) as f32 * 0.5);
    let inset = border.ceil() as usize;
    let inner = PixelRect {
        x0: inset.min(width),
        y0: inset.min(height),
        x1: width.saturating_sub(inset),
        y1: height.saturating_sub(inset),
    };
    let fill_edge = inner.x0 as f32 + inner.width() as f32 * progress.clamp(0.0, 1.0);
    let fill_sampler = FillSampler::new(fill, rect);
    let border_sampler = FillSampler::new(border_fill, rect);
    alpha
        .par_chunks_exact_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..width {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let outer = rounded_rect_coverage(px, py, rect, corner_radius_px);
                let inner_coverage = if border > 0.0 && !inner.is_empty() {
                    rounded_rect_coverage(px, py, inner, (corner_radius_px - border).max(0.0))
                } else {
                    outer
                };
                let border_coverage = (outer - inner_coverage).max(0.0);
                if border_coverage > 0.0 {
                    let color = border_sampler.sample(px, py, None, true);
                    blend_mask_pixel(&mut row[x], color.a, opacity * border_coverage);
                }
                let coverage = inner_coverage * (fill_edge + 0.5 - px).clamp(0.0, 1.0);
                if coverage > 0.0 {
                    let color = fill_sampler.sample(px, py, None, true);
                    blend_mask_pixel(&mut row[x], color.a, opacity * coverage);
                }
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn draw_text_mask(
    alpha: &mut [u8],
    width: usize,
    height: usize,
    assets: &Assets,
    text: &str,
    font_key: &str,
    size_px: f32,
    align: TextAlign,
    fill: &Fill,
    opacity: f32,
    corner_radius_px: f32,
) -> bool {
    let Some(bitmap) = assets.rasterized_text(text, font_key, size_px, Rgba8::WHITE) else {
        return false;
    };
    let rect = PixelRect {
        x0: 0,
        y0: 0,
        x1: width,
        y1: height,
    };
    let logical_x = match align {
        TextAlign::Start => 0.0,
        TextAlign::Center => (width as f32 - bitmap.logical_width) * 0.5,
        TextAlign::End => width as f32 - bitmap.logical_width,
    };
    let logical_y = (height as f32 - bitmap.logical_height) * 0.5;
    let destination_x = logical_x.round() as i32 - bitmap.padding as i32;
    let destination_y = logical_y.round() as i32 - bitmap.padding as i32;
    let sampler = FillSampler::new(fill, rect);
    for source_y in 0..bitmap.height {
        let y = destination_y + source_y as i32;
        if y < 0 || y >= height as i32 {
            continue;
        }
        for source_x in 0..bitmap.width {
            let x = destination_x + source_x as i32;
            if x < 0 || x >= width as i32 {
                continue;
            }
            let glyph_alpha = bitmap.rgba[(source_y * bitmap.width + source_x) * 4 + 3];
            if glyph_alpha == 0 {
                continue;
            }
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let fill_alpha = sampler.sample(px, py, None, true).a as u32;
            let source_alpha = ((glyph_alpha as u32 * fill_alpha + 127) / 255) as u8;
            let coverage = rounded_rect_coverage(px, py, rect, corner_radius_px);
            blend_mask_pixel(
                &mut alpha[y as usize * width + x as usize],
                source_alpha,
                opacity * coverage,
            );
        }
    }
    true
}

fn draw_rotated_item(
    frame: &mut Frame,
    rect: PixelRect,
    item: &SceneItem,
    assets: &Assets,
    report: &mut RenderReport,
    glow: Option<Glow>,
) -> Result<(), RasterError> {
    let mask = render_item_mask(rect, item, assets, report)?;
    let rotated_mask = rotate_mask(&mask, item.rotation_deg)?;
    if let Some(glow) = glow {
        draw_glow(frame, &rotated_mask, glow)?;
    }
    let mut body = Frame::new(rect.width() as u32, rect.height() as u32)?;
    let local_rect = PixelRect {
        x0: 0,
        y0: 0,
        x1: rect.width(),
        y1: rect.height(),
    };
    draw_item(&mut body, local_rect, item, assets, report, false)?;
    composite_rotated_body(frame, &body, &mask, &rotated_mask, item.rotation_deg);
    Ok(())
}

fn rotated_bounds(mask: &MaskSurface, angle_deg: f32) -> (i32, i32, usize, usize) {
    let angle = angle_deg.to_radians();
    let (sin, cos) = angle.sin_cos();
    let center_x = mask.origin_x as f32 + mask.width as f32 * 0.5;
    let center_y = mask.origin_y as f32 + mask.height as f32 * 0.5;
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (x, y) in [
        (mask.origin_x as f32, mask.origin_y as f32),
        (
            mask.origin_x as f32 + mask.width as f32,
            mask.origin_y as f32,
        ),
        (
            mask.origin_x as f32,
            mask.origin_y as f32 + mask.height as f32,
        ),
        (
            mask.origin_x as f32 + mask.width as f32,
            mask.origin_y as f32 + mask.height as f32,
        ),
    ] {
        let dx = x - center_x;
        let dy = y - center_y;
        let rx = center_x + dx * cos - dy * sin;
        let ry = center_y + dx * sin + dy * cos;
        min_x = min_x.min(rx);
        min_y = min_y.min(ry);
        max_x = max_x.max(rx);
        max_y = max_y.max(ry);
    }
    let x0 = min_x.floor() as i32;
    let y0 = min_y.floor() as i32;
    (
        x0,
        y0,
        (max_x.ceil() as i32 - x0) as usize,
        (max_y.ceil() as i32 - y0) as usize,
    )
}

fn rotate_mask(mask: &MaskSurface, angle_deg: f32) -> Result<MaskSurface, RasterError> {
    let (origin_x, origin_y, width, height) = rotated_bounds(mask, angle_deg);
    let len = width
        .checked_mul(height)
        .ok_or(RasterError::DimensionsOverflow)?;
    let mut alpha = vec![0; len];
    for y in 0..height {
        for x in 0..width {
            let (source_x, source_y) = inverse_rotated_source(
                mask,
                origin_x as f32 + x as f32 + 0.5,
                origin_y as f32 + y as f32 + 0.5,
                angle_deg,
            );
            alpha[y * width + x] =
                sample_bilinear(&mask.alpha, mask.width, mask.height, source_x, source_y);
        }
    }
    Ok(MaskSurface {
        origin_x,
        origin_y,
        width,
        height,
        alpha,
    })
}

fn inverse_rotated_source(
    source: &MaskSurface,
    destination_x: f32,
    destination_y: f32,
    angle_deg: f32,
) -> (f32, f32) {
    let angle = -angle_deg.to_radians();
    let (sin, cos) = angle.sin_cos();
    let center_x = source.origin_x as f32 + source.width as f32 * 0.5;
    let center_y = source.origin_y as f32 + source.height as f32 * 0.5;
    let dx = destination_x - center_x;
    let dy = destination_y - center_y;
    let global_x = center_x + dx * cos - dy * sin;
    let global_y = center_y + dx * sin + dy * cos;
    (
        global_x - source.origin_x as f32 - 0.5,
        global_y - source.origin_y as f32 - 0.5,
    )
}

fn sample_bilinear(data: &[u8], width: usize, height: usize, x: f32, y: f32) -> u8 {
    if x < -0.5 || y < -0.5 || x > width as f32 - 0.5 || y > height as f32 - 0.5 {
        return 0;
    }
    let x = x.clamp(0.0, width.saturating_sub(1) as f32);
    let y = y.clamp(0.0, height.saturating_sub(1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let top = data[y0 * width + x0] as f32 * (1.0 - tx) + data[y0 * width + x1] as f32 * tx;
    let bottom = data[y1 * width + x0] as f32 * (1.0 - tx) + data[y1 * width + x1] as f32 * tx;
    (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8
}

fn sample_bilinear_rgba_channel(
    data: &[u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    channel: usize,
) -> u8 {
    if x < -0.5 || y < -0.5 || x > width as f32 - 0.5 || y > height as f32 - 0.5 {
        return 0;
    }
    let x = x.clamp(0.0, width.saturating_sub(1) as f32);
    let y = y.clamp(0.0, height.saturating_sub(1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let get = |px: usize, py: usize| data[(py * width + px) * 4 + channel] as f32;
    let top = get(x0, y0) * (1.0 - tx) + get(x1, y0) * tx;
    let bottom = get(x0, y1) * (1.0 - tx) + get(x1, y1) * tx;
    (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8
}

fn composite_rotated_body(
    destination: &mut Frame,
    body: &Frame,
    source_mask: &MaskSurface,
    rotated_mask: &MaskSurface,
    angle_deg: f32,
) {
    let destination_width = destination.width as usize;
    for y in 0..rotated_mask.height {
        let global_y = rotated_mask.origin_y + y as i32;
        if global_y < 0 || global_y >= destination.height as i32 {
            continue;
        }
        for x in 0..rotated_mask.width {
            let alpha = rotated_mask.alpha[y * rotated_mask.width + x] as u32;
            if alpha == 0 {
                continue;
            }
            let global_x = rotated_mask.origin_x + x as i32;
            if global_x < 0 || global_x >= destination.width as i32 {
                continue;
            }
            let (source_x, source_y) = inverse_rotated_source(
                source_mask,
                global_x as f32 + 0.5,
                global_y as f32 + 0.5,
                angle_deg,
            );
            let inverse = 255 - alpha;
            let offset = (global_y as usize * destination_width + global_x as usize) * 4;
            for channel in 0..3 {
                let premultiplied = sample_bilinear_rgba_channel(
                    &body.rgba,
                    body.width as usize,
                    body.height as usize,
                    source_x,
                    source_y,
                    channel,
                ) as u32;
                destination.rgba[offset + channel] = (premultiplied
                    + (destination.rgba[offset + channel] as u32 * inverse + 127) / 255)
                    .min(255) as u8;
            }
        }
    }
}

fn draw_glow(frame: &mut Frame, source: &MaskSurface, glow: Glow) -> Result<(), RasterError> {
    if glow.radius_px <= 0.0 || glow.color.a == 0 {
        return Ok(());
    }
    let Some(bounds) = mask_bounds(source) else {
        return Ok(());
    };
    let spread = glow.spread_px.ceil().max(0.0) as usize;
    let blur = glow.radius_px.ceil().max(1.0) as usize;
    let pad = spread.saturating_add(blur);
    let source_width = bounds.width();
    let source_height = bounds.height();
    let width = source_width
        .checked_add(pad.saturating_mul(2))
        .ok_or(RasterError::DimensionsOverflow)?;
    let height = source_height
        .checked_add(pad.saturating_mul(2))
        .ok_or(RasterError::DimensionsOverflow)?;
    let len = width
        .checked_mul(height)
        .ok_or(RasterError::DimensionsOverflow)?;
    let mut alpha = vec![0; len];
    for y in 0..source_height {
        let destination = (y + pad) * width + pad;
        let source_offset = (bounds.y0 + y) * source.width + bounds.x0;
        alpha[destination..destination + source_width]
            .copy_from_slice(&source.alpha[source_offset..source_offset + source_width]);
    }
    let scale = if blur >= 16 {
        8
    } else if blur >= 8 {
        4
    } else if blur >= 4 {
        2
    } else {
        1
    };
    let (blurred, blurred_width, blurred_height) = if scale > 1 {
        let (mut reduced, reduced_width, reduced_height) =
            downsample_max(&alpha, width, height, scale);
        if spread > 0 {
            reduced = max_filter(
                &reduced,
                reduced_width,
                reduced_height,
                spread.div_ceil(scale),
            );
        }
        (
            box_blur(
                &reduced,
                reduced_width,
                reduced_height,
                blur.div_ceil(scale),
            ),
            reduced_width,
            reduced_height,
        )
    } else {
        if spread > 0 {
            alpha = max_filter(&alpha, width, height, spread);
        }
        (box_blur(&alpha, width, height, blur), width, height)
    };
    let origin_x = source.origin_x + bounds.x0 as i32 - pad as i32;
    let origin_y = source.origin_y + bounds.y0 as i32 - pad as i32;
    composite_glow_mask(
        frame,
        &blurred,
        blurred_width,
        blurred_height,
        width,
        height,
        origin_x,
        origin_y,
        scale,
        glow.color,
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn composite_glow_mask(
    frame: &mut Frame,
    blurred: &[u8],
    blurred_width: usize,
    blurred_height: usize,
    width: usize,
    height: usize,
    origin_x: i32,
    origin_y: i32,
    scale: usize,
    glow: Rgba8,
) {
    let frame_width = frame.width as usize;
    let row_bytes = frame_width * 4;
    frame
        .rgba
        .par_chunks_exact_mut(row_bytes)
        .enumerate()
        .for_each(|(global_y, row)| {
            let local_y = global_y as i32 - origin_y;
            if local_y < 0 || local_y >= height as i32 {
                return;
            }
            let y = local_y as usize;
            if scale == 1 {
                let x0 = (-origin_x).max(0) as usize;
                let x1 = width.min((frame_width as i32 - origin_x).max(0) as usize);
                for x in x0..x1 {
                    blend_glow_row_pixel(row, origin_x + x as i32, glow, blurred[y * width + x]);
                }
                return;
            }
            let reduced_y0 = (y / scale).min(blurred_height - 1);
            let reduced_y1 = (reduced_y0 + 1).min(blurred_height - 1);
            for reduced_x in 0..blurred_width {
                let next_x = (reduced_x + 1).min(blurred_width - 1);
                if blurred[reduced_y0 * blurred_width + reduced_x] == 0
                    && blurred[reduced_y0 * blurred_width + next_x] == 0
                    && blurred[reduced_y1 * blurred_width + reduced_x] == 0
                    && blurred[reduced_y1 * blurred_width + next_x] == 0
                {
                    continue;
                }
                let x0 = reduced_x * scale;
                let x1 = (x0 + scale).min(width);
                for x in x0..x1 {
                    let global_x = origin_x + x as i32;
                    if global_x < 0 || global_x >= frame_width as i32 {
                        continue;
                    }
                    let mask_alpha =
                        sample_upsampled(blurred, blurred_width, blurred_height, x, y, scale);
                    blend_glow_row_pixel(row, global_x, glow, mask_alpha);
                }
            }
        });
}

fn blend_glow_row_pixel(row: &mut [u8], x: i32, glow: Rgba8, mask_alpha: u8) {
    if mask_alpha == 0 {
        return;
    }
    let color = Rgba8::new(
        glow.r,
        glow.g,
        glow.b,
        ((glow.a as u32 * mask_alpha as u32 + 127) / 255) as u8,
    );
    let offset = x as usize * 4;
    blend_pixel(&mut row[offset..offset + 4], color, 1.0);
}

fn sample_upsampled(
    data: &[u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    scale: usize,
) -> u8 {
    let x0 = (x / scale).min(width - 1);
    let y0 = (y / scale).min(height - 1);
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = ((x % scale) * 256 / scale) as u32;
    let ty = ((y % scale) * 256 / scale) as u32;
    let inverse_x = 256 - tx;
    let inverse_y = 256 - ty;
    let top = data[y0 * width + x0] as u32 * inverse_x + data[y0 * width + x1] as u32 * tx;
    let bottom = data[y1 * width + x0] as u32 * inverse_x + data[y1 * width + x1] as u32 * tx;
    (((top * inverse_y + bottom * ty) + 32_768) >> 16).min(255) as u8
}

fn downsample_max(
    input: &[u8],
    width: usize,
    height: usize,
    scale: usize,
) -> (Vec<u8>, usize, usize) {
    let output_width = width.div_ceil(scale);
    let output_height = height.div_ceil(scale);
    let mut output = vec![0; output_width * output_height];
    output
        .par_chunks_exact_mut(output_width)
        .enumerate()
        .for_each(|(output_y, row)| {
            let y0 = output_y * scale;
            let y1 = (y0 + scale).min(height);
            for (output_x, value) in row.iter_mut().enumerate() {
                let x0 = output_x * scale;
                let x1 = (x0 + scale).min(width);
                let mut maximum = 0;
                for y in y0..y1 {
                    for x in x0..x1 {
                        maximum = maximum.max(input[y * width + x]);
                    }
                }
                *value = maximum;
            }
        });
    (output, output_width, output_height)
}

fn mask_bounds(source: &MaskSurface) -> Option<PixelRect> {
    source
        .alpha
        .par_chunks_exact(source.width)
        .enumerate()
        .filter_map(|(y, row)| {
            let x0 = row.iter().position(|alpha| *alpha > 0)?;
            let x1 = row.iter().rposition(|alpha| *alpha > 0)? + 1;
            Some(PixelRect {
                x0,
                y0: y,
                x1,
                y1: y + 1,
            })
        })
        .reduce_with(|left, right| PixelRect {
            x0: left.x0.min(right.x0),
            y0: left.y0.min(right.y0),
            x1: left.x1.max(right.x1),
            y1: left.y1.max(right.y1),
        })
}

fn max_filter(input: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
    let radius_x = radius.min(width.saturating_sub(1));
    let mut horizontal = vec![0; input.len()];
    horizontal
        .par_chunks_exact_mut(width)
        .enumerate()
        .for_each(|(y, output)| {
            sliding_max(&input[y * width..(y + 1) * width], output, radius_x);
        });
    let transposed = transpose(&horizontal, width, height);
    let radius_y = radius.min(height.saturating_sub(1));
    let mut filtered = vec![0; input.len()];
    filtered
        .par_chunks_exact_mut(height)
        .enumerate()
        .for_each(|(x, output)| {
            sliding_max(&transposed[x * height..(x + 1) * height], output, radius_y);
        });
    transpose(&filtered, height, width)
}

fn sliding_max(input: &[u8], output: &mut [u8], radius: usize) {
    let mut queue = VecDeque::new();
    let mut next = 0;
    for index in 0..input.len() {
        let right = index.saturating_add(radius).min(input.len() - 1);
        while next <= right {
            while queue
                .back()
                .is_some_and(|entry: &(usize, u8)| entry.1 <= input[next])
            {
                queue.pop_back();
            }
            queue.push_back((next, input[next]));
            next += 1;
        }
        let left = index.saturating_sub(radius);
        while queue.front().is_some_and(|entry| entry.0 < left) {
            queue.pop_front();
        }
        output[index] = queue.front().map_or(0, |entry| entry.1);
    }
}

fn box_blur(input: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
    let mut horizontal = vec![0; input.len()];
    horizontal
        .par_chunks_exact_mut(width)
        .enumerate()
        .for_each(|(y, output)| {
            sliding_average(&input[y * width..(y + 1) * width], output, radius);
        });
    let transposed = transpose(&horizontal, width, height);
    let mut blurred = vec![0; input.len()];
    blurred
        .par_chunks_exact_mut(height)
        .enumerate()
        .for_each(|(x, output)| {
            sliding_average(&transposed[x * height..(x + 1) * height], output, radius);
        });
    transpose(&blurred, height, width)
}

fn transpose(input: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut output = vec![0; input.len()];
    output
        .par_chunks_exact_mut(height)
        .enumerate()
        .for_each(|(x, column)| {
            for y in 0..height {
                column[y] = input[y * width + x];
            }
        });
    output
}

fn sliding_average(input: &[u8], output: &mut [u8], radius: usize) {
    let radius = radius.min(input.len());
    let window = radius.saturating_mul(2).saturating_add(1) as u64;
    let mut sum = input
        .iter()
        .take(radius.saturating_add(1))
        .map(|value| *value as u64)
        .sum::<u64>();
    for index in 0..input.len() {
        output[index] = ((sum + window / 2) / window).min(255) as u8;
        if index >= radius {
            sum -= input[index - radius] as u64;
        }
        let entering = index.saturating_add(radius).saturating_add(1);
        if entering < input.len() {
            sum += input[entering] as u64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_SCALED_IMAGE_CACHE_ENTRIES;
    use crate::model::{Fit, GradientStop, GradientStops};

    fn item(kind: SceneItemKind) -> SceneItem {
        SceneItem {
            rect: NormRect::FULL,
            opacity: 1.0,
            rotation_deg: 0.0,
            corner_radius_px: 0.0,
            glow: None,
            kind,
        }
    }

    fn scene(width: u32, height: u32, item: SceneItem) -> Scene {
        Scene {
            width,
            height,
            items: vec![item],
        }
    }

    fn pixel(frame: &Frame, x: usize, y: usize) -> &[u8] {
        let offset = (y * frame.width as usize + x) * 4;
        &frame.rgba[offset..offset + 4]
    }

    #[test]
    fn normalized_rect_rounding_is_fixed_at_boundaries() {
        assert_eq!(
            norm_rect_to_pixels(NormRect::new(0.25, 0.25, 0.5, 0.5), 7, 7),
            PixelRect {
                x0: 2,
                y0: 2,
                x1: 5,
                y1: 5,
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
    }

    #[test]
    fn linear_and_radial_gradients_use_layer_local_coordinates() {
        let stops = GradientStops::new(vec![
            GradientStop {
                position: 0.0,
                color: Rgba8::BLACK,
            },
            GradientStop {
                position: 1.0,
                color: Rgba8::WHITE,
            },
        ])
        .unwrap();
        let mut linear_frame = Frame::new(1, 1).unwrap();
        let linear = item(SceneItemKind::Solid {
            fill: Fill::LinearGradient {
                stops: stops.clone(),
                angle_deg: 0.0,
            },
        });
        let _ = render_frame(&scene(8, 2, linear), &Assets::default(), &mut linear_frame).unwrap();
        assert!(pixel(&linear_frame, 0, 0)[0] < pixel(&linear_frame, 7, 0)[0]);
        assert_eq!(pixel(&linear_frame, 0, 0), pixel(&linear_frame, 0, 1));

        let mut radial_frame = Frame::new(1, 1).unwrap();
        let radial = item(SceneItemKind::Solid {
            fill: Fill::RadialGradient {
                stops,
                center: (0.5, 0.5),
                radius: 0.5,
            },
        });
        let _ = render_frame(&scene(5, 5, radial), &Assets::default(), &mut radial_frame).unwrap();
        assert_eq!(pixel(&radial_frame, 2, 2), &[0, 0, 0, 255]);
        assert_eq!(pixel(&radial_frame, 0, 0), &[255, 255, 255, 255]);
    }

    #[test]
    fn spectrum_resampling_uses_linear_endpoint_interpolation() {
        assert_eq!(
            resample_spectrum(&[0.0, 1.0], 5),
            vec![0.0, 0.25, 0.5, 0.75, 1.0]
        );
        assert_eq!(resample_spectrum(&[0.4], 3), vec![0.4; 3]);
        assert_eq!(resample_spectrum(&[], 2), vec![0.0; 2]);
    }

    #[test]
    fn complex_frame_with_glow_gradient_radial_and_rotation_is_deterministic() {
        let stops = GradientStops::new(vec![
            GradientStop {
                position: 0.0,
                color: Rgba8::new(120, 255, 35, 255),
            },
            GradientStop {
                position: 1.0,
                color: Rgba8::new(255, 136, 255, 200),
            },
        ])
        .unwrap();
        let mut layer = item(SceneItemKind::Spectrum {
            values: vec![0.1, 0.8, 0.3, 1.0],
            point_count: 31,
            shape: SpectrumShape::Radial {
                radius_px: 24.0,
                thickness_px: 3.0,
                start_angle_deg: -90.0,
                sweep_deg: 360.0,
                direction: SpectrumDirection::Clockwise,
            },
            fill: Fill::BandGradient { stops },
        });
        layer.rotation_deg = 7.0;
        layer.glow = Some(Glow {
            color: Rgba8::new(255, 0, 120, 220),
            radius_px: 8.0,
            spread_px: 2.0,
        });
        let scene = scene(128, 96, layer);
        let mut first = Frame::new(1, 1).unwrap();
        let mut second = Frame::new(2, 2).unwrap();
        let _ = render_frame(&scene, &Assets::default(), &mut first).unwrap();
        let _ = render_frame(&scene, &Assets::default(), &mut second).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn rotation_is_applied_around_the_layer_center() {
        let mut layer = item(SceneItemKind::Solid {
            fill: Fill::Solid(Rgba8::WHITE),
        });
        layer.rect = NormRect::new(0.3, 0.4, 0.4, 0.2);
        layer.rotation_deg = 90.0;
        let mut frame = Frame::new(1, 1).unwrap();
        let _ = render_frame(&scene(100, 100, layer), &Assets::default(), &mut frame).unwrap();
        assert_eq!(pixel(&frame, 50, 32), &[255, 255, 255, 255]);
        assert_eq!(pixel(&frame, 32, 50), &[0, 0, 0, 255]);
    }

    #[test]
    fn radial_thin_bars_have_analytic_antialiasing() {
        let layer = item(SceneItemKind::Spectrum {
            values: vec![0.2, 0.8, 0.4, 1.0],
            point_count: 37,
            shape: SpectrumShape::Radial {
                radius_px: 32.0,
                thickness_px: 2.0,
                start_angle_deg: -90.0,
                sweep_deg: 360.0,
                direction: SpectrumDirection::Clockwise,
            },
            fill: Fill::Solid(Rgba8::WHITE),
        });
        let mut frame = Frame::new(1, 1).unwrap();
        let _ = render_frame(&scene(128, 128, layer), &Assets::default(), &mut frame).unwrap();
        assert!(
            frame
                .rgba
                .chunks_exact(4)
                .any(|pixel| pixel[0] > 0 && pixel[0] < 255)
        );
    }

    #[test]
    fn rounded_progress_border_has_antialiased_edges() {
        let mut layer = item(SceneItemKind::ProgressBar {
            progress: 0.65,
            border_thickness_px: 2.0,
            fill: Fill::Solid(Rgba8::WHITE),
            border_fill: Fill::Solid(Rgba8::WHITE),
        });
        layer.rect = NormRect::new(0.1, 0.25, 0.8, 0.5);
        layer.corner_radius_px = 12.0;
        let mut frame = Frame::new(1, 1).unwrap();
        let _ = render_frame(&scene(100, 40, layer), &Assets::default(), &mut frame).unwrap();
        assert!(
            frame
                .rgba
                .chunks_exact(4)
                .any(|pixel| pixel[0] > 0 && pixel[0] < 255)
        );
        assert_eq!(pixel(&frame, 10, 10), &[0, 0, 0, 255]);
    }

    #[test]
    fn full_hd_reference_geometry_keeps_partial_coverage_pixels() {
        let radial = item(SceneItemKind::Spectrum {
            values: (0..96)
                .map(|index| 0.2 + 0.8 * (index as f32 * 0.37).sin().abs())
                .collect(),
            point_count: 200,
            shape: SpectrumShape::Radial {
                radius_px: 380.0,
                thickness_px: 4.0,
                start_angle_deg: -90.0,
                sweep_deg: 360.0,
                direction: SpectrumDirection::Clockwise,
            },
            fill: Fill::Solid(Rgba8::WHITE),
        });
        let mut radial_frame = Frame::new(1, 1).unwrap();
        let _ = render_frame(
            &scene(1920, 1080, radial),
            &Assets::default(),
            &mut radial_frame,
        )
        .unwrap();
        let radial_partial = radial_frame
            .rgba
            .chunks_exact(4)
            .filter(|pixel| pixel[0] > 0 && pixel[0] < 255)
            .count();
        assert!(radial_partial > 500);

        let mut progress = item(SceneItemKind::ProgressBar {
            progress: 0.65,
            border_thickness_px: 2.0,
            fill: Fill::Solid(Rgba8::WHITE),
            border_fill: Fill::Solid(Rgba8::WHITE),
        });
        progress.rect = NormRect::new(
            60.0 / 1920.0,
            1000.0 / 1080.0,
            1800.0 / 1920.0,
            50.0 / 1080.0,
        );
        progress.corner_radius_px = 20.0;
        let mut progress_frame = Frame::new(1, 1).unwrap();
        let _ = render_frame(
            &scene(1920, 1080, progress),
            &Assets::default(),
            &mut progress_frame,
        )
        .unwrap();
        let progress_partial = progress_frame
            .rgba
            .chunks_exact(4)
            .filter(|pixel| pixel[0] > 0 && pixel[0] < 255)
            .count();
        assert!(progress_partial > 50);
    }

    #[test]
    fn glow_expands_beyond_layer_rect_and_zero_radius_is_noop() {
        let mut glowing = item(SceneItemKind::Solid {
            fill: Fill::Solid(Rgba8::WHITE),
        });
        glowing.rect = NormRect::new(0.4, 0.4, 0.2, 0.2);
        glowing.glow = Some(Glow {
            color: Rgba8::new(255, 0, 0, 255),
            radius_px: 5.0,
            spread_px: 2.0,
        });
        let mut glow_frame = Frame::new(1, 1).unwrap();
        let _ = render_frame(
            &scene(50, 50, glowing.clone()),
            &Assets::default(),
            &mut glow_frame,
        )
        .unwrap();
        assert!(pixel(&glow_frame, 18, 25)[0] > 0);

        glowing.glow = Some(Glow {
            color: Rgba8::new(255, 0, 0, 255),
            radius_px: 0.0,
            spread_px: 20.0,
        });
        let mut zero_radius = Frame::new(1, 1).unwrap();
        let _ = render_frame(
            &scene(50, 50, glowing.clone()),
            &Assets::default(),
            &mut zero_radius,
        )
        .unwrap();
        glowing.glow = None;
        let mut no_glow = Frame::new(1, 1).unwrap();
        let _ = render_frame(&scene(50, 50, glowing), &Assets::default(), &mut no_glow).unwrap();
        assert_eq!(zero_radius, no_glow);
    }

    fn image_scene(width: u32, height: u32, id: &str, fit: Fit) -> Scene {
        scene(
            width,
            height,
            item(SceneItemKind::Image {
                image: ImageId::new(id),
                fit,
            }),
        )
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
        let _ = render_frame(&scene, &assets, &mut first).unwrap();
        let after_first = assets.scaled_image_cache_stats();
        assert_eq!(after_first.scale_operations, 1);
        assert_eq!(after_first.misses, 1);
        let _ = render_frame(&scene, &assets, &mut second).unwrap();
        let after_second = assets.scaled_image_cache_stats();
        assert_eq!(after_second.scale_operations, 1);
        assert_eq!(after_second.hits, 1);
        assert_eq!(first, second);
    }

    #[test]
    fn scaled_image_cache_has_a_fixed_entry_bound() {
        let id = ImageId::new("background");
        let assets = Assets::default();
        assets.insert_image(id.clone(), patterned_image(13, 7, 3));
        let mut frame = Frame::new(1, 1).unwrap();
        for index in 0..MAX_SCALED_IMAGE_CACHE_ENTRIES + 3 {
            let scene = image_scene(20 + index as u32, 12, &id.0, Fit::Stretch);
            let _ = render_frame(&scene, &assets, &mut frame).unwrap();
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
        let _ = render_frame(
            &image_scene(8, 8, &id.0, Fit::Contain),
            &assets,
            &mut contain,
        )
        .unwrap();
        let _ = render_frame(&image_scene(8, 8, &id.0, Fit::Cover), &assets, &mut cover).unwrap();
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
        let _ = render_frame(&scene, &assets, &mut before).unwrap();
        assets.insert_image(id, patterned_image(31, 17, 91));
        assert_eq!(assets.scaled_image_cache_stats().entries, 0);
        let _ = render_frame(&scene, &assets, &mut after).unwrap();
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
