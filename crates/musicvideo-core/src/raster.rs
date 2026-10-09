use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;

use rayon::prelude::*;

use music_core::{
    KeyPlacement, KeyboardFrame, KeyboardLayoutCache, KeyboardRect, MivSpectrumFrame,
    MivSpectrumPrimitive, SpectrumVisualRect, VisualizationColor,
    brighten_color as brighten_visualization_color, key_fill as miv_key_fill,
    layout_keyboard_cached, lerp_color as miv_lerp_color, spectrum_color as miv_spectrum_color,
    unlit_key_base as miv_unlit_key_base, visit_miv_spectrum_primitives,
};

use crate::assets::{AssetError, Assets, ImageAsset};
use crate::model::{
    Fill, Fit, Glow, ImageId, MusicColorMode, NormRect, Rgba8, SpectrumDirection, SpectrumShape,
    TextAlign,
};
use crate::scene::{Scene, SceneItem, SceneItemKind, brighten_color};

thread_local! {
    static KEYBOARD_LAYOUT_CACHE: RefCell<KeyboardLayoutCache> =
        RefCell::new(KeyboardLayoutCache::default());
}

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
            draw_image(frame, rect, image, 1.0, 0.0, false, 1.0);
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
        SceneItemKind::Image { image, fit, zoom } => {
            if !matches!(fit, Fit::Stretch | Fit::Cover) {
                return Ok(None);
            }
            if (*zoom - 1.0).abs() > f32::EPSILON {
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
        } => *color,
        SceneItemKind::Spectrum {
            shape,
            color_mode: MusicColorMode::Custom,
            fill: Fill::Solid(color),
            ..
        } if *shape != SpectrumShape::MivHorizontal => *color,
        SceneItemKind::TimeText {
            fill: Fill::Solid(color),
            style,
            ..
        }
        | SceneItemKind::Text {
            fill: Fill::Solid(color),
            style,
            ..
        } if !style.has_paint() => *color,
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
        SceneItemKind::Image { image, fit, zoom } => {
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
                    *zoom,
                );
            } else {
                report.record_unresolved(UnresolvedAsset::Image(image.clone()));
            }
        }
        SceneItemKind::Spectrum {
            values,
            miv_frame,
            point_count,
            shape,
            color_mode,
            miv_brightness,
            fill,
        } => draw_spectrum(
            frame,
            rect,
            values,
            miv_frame,
            *point_count,
            *shape,
            *color_mode,
            *miv_brightness,
            fill,
            item.opacity,
            item.corner_radius_px,
            mask_mode,
        ),
        SceneItemKind::PitchKeyboard {
            keyboard,
            background,
            color_mode,
            miv_brightness,
            fill,
        } => draw_pitch_keyboard(
            frame,
            rect,
            keyboard,
            background,
            *color_mode,
            *miv_brightness,
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
            style,
            transient_style,
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
                style,
                *transient_style,
                fill,
                item.opacity,
                item.corner_radius_px,
                mask_mode,
            ) {
                report.record_unresolved(UnresolvedAsset::Font(font_key.clone()));
            }
        }
        SceneItemKind::Text {
            text,
            font_key,
            size_px,
            align,
            style,
            transient_style,
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
                style,
                *transient_style,
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
    zoom: f32,
) {
    if rect.is_empty() || opacity <= 0.0 {
        return;
    }
    debug_assert_eq!(image.width as usize, rect.width());
    debug_assert_eq!(image.height as usize, rect.height());
    let zoom = if zoom.is_finite() {
        zoom.max(0.01)
    } else {
        1.0
    };
    let identity_zoom = (zoom - 1.0).abs() <= f32::EPSILON;
    if identity_zoom
        && !mask_mode
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
    if identity_zoom && !mask_mode && corner_radius_px <= 0.0 && image.opaque && opacity >= 1.0 {
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

    if identity_zoom {
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
                    let coverage = rounded_rect_coverage(
                        x as f32 + 0.5,
                        y as f32 + 0.5,
                        rect,
                        corner_radius_px,
                    );
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
        return;
    }

    frame
        .rgba
        .par_chunks_exact_mut(frame_row_bytes)
        .enumerate()
        .filter(|(y, _)| *y >= rect.y0 && *y < rect.y1)
        .for_each(|(y, row)| {
            let local_y = y - rect.y0;
            let source_y = (local_y as f32 + 0.5 - image.height as f32 * 0.5) / zoom
                + image.height as f32 * 0.5
                - 0.5;
            for x in rect.x0..rect.x1 {
                let local_x = x - rect.x0;
                let source_x = (local_x as f32 + 0.5 - image.width as f32 * 0.5) / zoom
                    + image.width as f32 * 0.5
                    - 0.5;
                let coverage =
                    rounded_rect_coverage(x as f32 + 0.5, y as f32 + 0.5, rect, corner_radius_px);
                let sampled = sample_bilinear_rgba(
                    &image.rgba,
                    image.width as usize,
                    image.height as usize,
                    source_x,
                    source_y,
                );
                let color = if mask_mode {
                    Rgba8::new(255, 255, 255, sampled[3])
                } else {
                    Rgba8::new(sampled[0], sampled[1], sampled[2], sampled[3])
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
    index: usize,
    total: usize,
    value: f32,
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
                        index,
                        total: count,
                        value,
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
        SpectrumShape::MivHorizontal => Vec::new(),
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
                index,
                total: count,
                value,
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
    miv_frame: &MivSpectrumFrame,
    point_count: usize,
    shape: SpectrumShape,
    color_mode: MusicColorMode,
    miv_brightness: f32,
    fill: &Fill,
    opacity: f32,
    corner_radius_px: f32,
    mask_mode: bool,
) {
    if shape == SpectrumShape::MivHorizontal {
        draw_miv_spectrum(
            frame,
            rect,
            miv_frame,
            color_mode,
            miv_brightness,
            fill,
            opacity,
            corner_radius_px,
            mask_mode,
        );
        return;
    }
    let bars = spectrum_bars(rect, values, point_count, shape);
    let sampler = FillSampler::new(fill, rect);
    for bar in bars {
        draw_spectrum_bar(
            frame,
            rect,
            bar,
            &sampler,
            color_mode,
            miv_brightness,
            opacity,
            corner_radius_px,
            mask_mode,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_miv_spectrum(
    frame: &mut Frame,
    clip_rect: PixelRect,
    visual: &MivSpectrumFrame,
    color_mode: MusicColorMode,
    miv_brightness: f32,
    fill: &Fill,
    opacity: f32,
    corner_radius_px: f32,
    mask_mode: bool,
) {
    let sampler = FillSampler::new(fill, clip_rect);
    let mut primitives = Vec::with_capacity(visual.bands.len() * 4);
    visit_miv_spectrum_primitives(
        visual,
        SpectrumVisualRect::new(
            clip_rect.x0 as f32,
            clip_rect.y0 as f32,
            clip_rect.x1 as f32,
            clip_rect.y1 as f32,
        ),
        |primitive| primitives.push(primitive),
    );
    let prepared_colors = primitives
        .iter()
        .map(|primitive| prepare_miv_color(*primitive, color_mode, miv_brightness, mask_mode))
        .collect::<Vec<_>>();
    let row_bytes = frame.width as usize * 4;
    frame
        .rgba
        .par_chunks_exact_mut(row_bytes)
        .enumerate()
        .filter(|(y, _)| *y >= clip_rect.y0 && *y < clip_rect.y1)
        .for_each(|(y, row)| {
            let py = y as f32 + 0.5;
            for (primitive, prepared_color) in primitives.iter().zip(&prepared_colors) {
                if py < primitive.rect.top || py >= primitive.rect.bottom {
                    continue;
                }
                let min_x = primitive.rect.left.floor().max(clip_rect.x0 as f32) as usize;
                let max_x = primitive.rect.right.ceil().min(clip_rect.x1 as f32) as usize;
                let band_position = band_position(primitive.band_index, primitive.band_count);
                for x in min_x..max_x {
                    let px = x as f32 + 0.5;
                    let coverage =
                        visual_rect_coverage(px, py, primitive.rect, primitive.corner_radius_px)
                            * rounded_rect_coverage(px, py, clip_rect, corner_radius_px);
                    if coverage <= 0.0 {
                        continue;
                    }
                    let mut color = prepared_color
                        .unwrap_or_else(|| sampler.sample(px, py, Some(band_position), mask_mode));
                    if prepared_color.is_none() && primitive.brightness_scale != 1.0 {
                        color = scale_rgb(color, primitive.brightness_scale);
                    }
                    if let Some(alpha) = primitive.alpha_override {
                        color.a = alpha;
                    }
                    blend_pixel(&mut row[x * 4..x * 4 + 4], color, opacity * coverage);
                }
            }
        });
}

fn prepare_miv_color(
    primitive: MivSpectrumPrimitive,
    color_mode: MusicColorMode,
    miv_brightness: f32,
    mask_mode: bool,
) -> Option<Rgba8> {
    (color_mode == MusicColorMode::Miv).then(|| {
        let mut color = miv_spectrum_color(
            primitive.band_index,
            primitive.band_count,
            primitive.color_value,
        );
        if primitive.brightness_scale != 1.0 {
            color = brighten_visualization_color(color, primitive.brightness_scale);
        }
        let color = brighten_color(rgba8(color), miv_brightness);
        if mask_mode {
            Rgba8::new(255, 255, 255, color.a)
        } else {
            color
        }
    })
}

fn visual_rect_coverage(x: f32, y: f32, rect: SpectrumVisualRect, radius_px: f32) -> f32 {
    let width = (rect.right - rect.left).max(0.0);
    let height = (rect.bottom - rect.top).max(0.0);
    let radius = radius_px.max(0.0).min(width.min(height) * 0.5);
    let center_x = (rect.left + rect.right) * 0.5;
    let center_y = (rect.top + rect.bottom) * 0.5;
    let qx = (x - center_x).abs() - (width * 0.5 - radius);
    let qy = (y - center_y).abs() - (height * 0.5 - radius);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    let inside = qx.max(qy).min(0.0);
    (0.5 - (outside + inside - radius)).clamp(0.0, 1.0)
}

fn scale_rgb(color: Rgba8, scale: f32) -> Rgba8 {
    Rgba8::new(
        (f32::from(color.r) * scale).min(255.0) as u8,
        (f32::from(color.g) * scale).min(255.0) as u8,
        (f32::from(color.b) * scale).min(255.0) as u8,
        color.a,
    )
}

fn draw_spectrum_bar(
    frame: &mut Frame,
    clip_rect: PixelRect,
    bar: SpectrumBar,
    sampler: &FillSampler,
    color_mode: MusicColorMode,
    miv_brightness: f32,
    opacity: f32,
    corner_radius_px: f32,
    mask_mode: bool,
) {
    let miv_color = (color_mode == MusicColorMode::Miv).then(|| {
        brighten_color(
            rgba8(miv_spectrum_color(bar.index, bar.total, bar.value)),
            miv_brightness,
        )
    });
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
            let color = match miv_color {
                None => sampler.sample(px, py, Some(bar.band_position), mask_mode),
                Some(color) if mask_mode => Rgba8::new(255, 255, 255, color.a),
                Some(color) => color,
            };
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
fn draw_pitch_keyboard(
    frame: &mut Frame,
    rect: PixelRect,
    keyboard: &KeyboardFrame,
    background: &Fill,
    color_mode: MusicColorMode,
    miv_brightness: f32,
    fill: &Fill,
    opacity: f32,
    corner_radius_px: f32,
    mask_mode: bool,
) {
    draw_filled_rect(
        frame,
        rect,
        background,
        opacity,
        corner_radius_px,
        mask_mode,
    );

    let sampler = FillSampler::new(fill, rect);
    KEYBOARD_LAYOUT_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let layout = layout_keyboard_cached(
            keyboard,
            KeyboardRect::new(
                rect.x0 as f32,
                rect.y0 as f32,
                rect.x1 as f32,
                rect.y1 as f32,
            ),
            &mut cache,
        );
        let clip_coverage = KeyboardClipCoverage::new(rect, corner_radius_px);
        draw_keyboard_guides(
            frame,
            rect,
            &layout.octave_guides_x,
            opacity,
            mask_mode,
            &clip_coverage,
        );
        draw_keyboard_keys(
            frame,
            rect,
            &layout.keys,
            &sampler,
            color_mode,
            miv_brightness,
            opacity,
            mask_mode,
            &clip_coverage,
        );
    });
}

struct KeyboardClipCoverage {
    rect: PixelRect,
    values: Option<Vec<f32>>,
}

impl KeyboardClipCoverage {
    fn new(rect: PixelRect, corner_radius_px: f32) -> Self {
        if corner_radius_px <= 0.0 || rect.is_empty() {
            return Self { rect, values: None };
        }
        let width = rect.width();
        let mut values = vec![0.0; width * rect.height()];
        values
            .par_chunks_exact_mut(width)
            .enumerate()
            .for_each(|(local_y, row)| {
                let y = rect.y0 + local_y;
                for (local_x, value) in row.iter_mut().enumerate() {
                    let x = rect.x0 + local_x;
                    *value = rounded_rect_coverage(
                        x as f32 + 0.5,
                        y as f32 + 0.5,
                        rect,
                        corner_radius_px,
                    );
                }
            });
        Self {
            rect,
            values: Some(values),
        }
    }

    fn at(&self, x: usize, y: usize) -> f32 {
        self.values.as_ref().map_or(1.0, |values| {
            values[(y - self.rect.y0) * self.rect.width() + (x - self.rect.x0)]
        })
    }
}

fn draw_keyboard_guides(
    frame: &mut Frame,
    clip: PixelRect,
    guides: &[f32],
    opacity: f32,
    mask_mode: bool,
    clip_coverage: &KeyboardClipCoverage,
) {
    let color = if mask_mode {
        Rgba8::WHITE
    } else {
        Rgba8::new(120, 134, 148, 82)
    };
    let row_bytes = frame.width as usize * 4;
    frame
        .rgba
        .par_chunks_exact_mut(row_bytes)
        .enumerate()
        .filter(|(y, _)| *y >= clip.y0 && *y < clip.y1)
        .for_each(|(y, row)| {
            for &guide_x in guides {
                let min_x = (guide_x - 1.0).floor().max(clip.x0 as f32) as usize;
                let max_x = (guide_x + 1.0).ceil().min(clip.x1 as f32) as usize;
                for pixel_x in min_x..max_x {
                    let px = pixel_x as f32 + 0.5;
                    let line_coverage = (0.5 - ((px - guide_x).abs() - 0.4)).clamp(0.0, 1.0);
                    let clip_coverage = clip_coverage.at(pixel_x, y);
                    if line_coverage <= 0.0 || clip_coverage <= 0.0 {
                        continue;
                    }
                    let offset = pixel_x * 4;
                    blend_pixel(
                        &mut row[offset..offset + 4],
                        color,
                        opacity * line_coverage * clip_coverage,
                    );
                }
            }
        });
}

fn draw_keyboard_keys(
    frame: &mut Frame,
    clip: PixelRect,
    keys: &[KeyPlacement],
    sampler: &FillSampler<'_>,
    color_mode: MusicColorMode,
    miv_brightness: f32,
    opacity: f32,
    mask_mode: bool,
    clip_coverage: &KeyboardClipCoverage,
) {
    let miv_gradients = (color_mode == MusicColorMode::Miv).then(|| {
        keys.iter()
            .map(|key| MivKeyGradient {
                top: miv_key_fill(key.midi, key.levels.center * key.levels.odd, key.black),
                center: miv_key_fill(key.midi, key.levels.center, key.black),
                bottom: miv_key_fill(key.midi, key.levels.center * key.levels.even, key.black),
            })
            .collect::<Vec<_>>()
    });
    let row_bytes = frame.width as usize * 4;
    frame
        .rgba
        .par_chunks_exact_mut(row_bytes)
        .enumerate()
        .filter(|(y, _)| *y >= clip.y0 && *y < clip.y1)
        .for_each(|(y, row)| {
            let py = y as f32 + 0.5;
            for (key_index, &key) in keys.iter().enumerate() {
                let min_y = key.rect.top.floor().max(clip.y0 as f32) as usize;
                let max_y = key.rect.bottom.ceil().min(clip.y1 as f32) as usize;
                if y < min_y || y >= max_y {
                    continue;
                }
                let min_x = key.rect.left.floor().max(clip.x0 as f32) as usize;
                let max_x = key.rect.right.ceil().min(clip.x1 as f32) as usize;
                let key_corner = if key.black { 1.0 } else { 0.0 };
                let stroke_width = if key.black { 0.75 } else { 0.8 };
                let stroke_color = if key.black {
                    Rgba8::new(255, 255, 255, 80)
                } else {
                    Rgba8::new(0, 0, 0, 155)
                };
                let base = rgba8(miv_unlit_key_base(key.black, key.real_key));
                let band_position = key.note_position;
                for x in min_x..max_x {
                    let px = x as f32 + 0.5;
                    let outer = float_rounded_rect_coverage(px, py, key.rect, key_corner);
                    let clip_coverage = clip_coverage.at(x, y);
                    let coverage = outer * clip_coverage;
                    if coverage <= 0.0 {
                        continue;
                    }
                    let color = if mask_mode {
                        Rgba8::WHITE
                    } else if key.real_key {
                        match color_mode {
                            MusicColorMode::Custom => {
                                let level = keyboard_vertical_level(key, py);
                                let active = sampler.sample(px, py, band_position, false);
                                let amount =
                                    (level * if key.black { 0.96 } else { 0.94 }).clamp(0.0, 1.0);
                                mix_rgba(base, active, amount)
                            }
                            MusicColorMode::Miv => brighten_color(
                                miv_keyboard_vertical_color(
                                    miv_gradients
                                        .as_ref()
                                        .expect("mIV 配色では鍵ごとの色が計算済みです")[key_index],
                                    key,
                                    py,
                                ),
                                miv_brightness,
                            ),
                        }
                    } else {
                        base
                    };
                    let offset = x * 4;
                    blend_pixel(&mut row[offset..offset + 4], color, opacity * coverage);

                    if mask_mode {
                        continue;
                    }

                    let inner = KeyboardRect::new(
                        key.rect.left + stroke_width,
                        key.rect.top + stroke_width,
                        key.rect.right - stroke_width,
                        key.rect.bottom - stroke_width,
                    );
                    let inner_coverage = if inner.right > inner.left && inner.bottom > inner.top {
                        float_rounded_rect_coverage(
                            px,
                            py,
                            inner,
                            (key_corner - stroke_width).max(0.0),
                        )
                    } else {
                        0.0
                    };
                    let stroke_coverage = (outer - inner_coverage).max(0.0) * clip_coverage;
                    if stroke_coverage <= 0.0 {
                        continue;
                    }
                    blend_pixel(
                        &mut row[offset..offset + 4],
                        stroke_color,
                        opacity * stroke_coverage,
                    );
                }
            }
        });
}

#[cfg(test)]
fn draw_keyboard_guide_serial(
    frame: &mut Frame,
    clip: PixelRect,
    x: f32,
    opacity: f32,
    corner_radius_px: f32,
    mask_mode: bool,
) {
    let min_x = (x - 1.0).floor().max(clip.x0 as f32) as usize;
    let max_x = (x + 1.0).ceil().min(clip.x1 as f32) as usize;
    let width = frame.width as usize;
    let color = if mask_mode {
        Rgba8::WHITE
    } else {
        Rgba8::new(120, 134, 148, 82)
    };
    for y in clip.y0..clip.y1 {
        for pixel_x in min_x..max_x {
            let px = pixel_x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let line_coverage = (0.5 - ((px - x).abs() - 0.4)).clamp(0.0, 1.0);
            let clip_coverage = rounded_rect_coverage(px, py, clip, corner_radius_px);
            if line_coverage <= 0.0 || clip_coverage <= 0.0 {
                continue;
            }
            let offset = (y * width + pixel_x) * 4;
            blend_pixel(
                &mut frame.rgba[offset..offset + 4],
                color,
                opacity * line_coverage * clip_coverage,
            );
        }
    }
}

#[cfg(test)]
fn draw_keyboard_key_serial(
    frame: &mut Frame,
    clip: PixelRect,
    key: KeyPlacement,
    sampler: &FillSampler<'_>,
    opacity: f32,
    corner_radius_px: f32,
    mask_mode: bool,
) {
    let min_x = key.rect.left.floor().max(clip.x0 as f32) as usize;
    let min_y = key.rect.top.floor().max(clip.y0 as f32) as usize;
    let max_x = key.rect.right.ceil().min(clip.x1 as f32) as usize;
    let max_y = key.rect.bottom.ceil().min(clip.y1 as f32) as usize;
    let key_corner = if key.black { 1.0 } else { 0.0 };
    let stroke_width = if key.black { 0.75 } else { 0.8 };
    let stroke_color = if key.black {
        Rgba8::new(255, 255, 255, 80)
    } else {
        Rgba8::new(0, 0, 0, 155)
    };
    let base = rgba8(miv_unlit_key_base(key.black, key.real_key));
    let band_position = key.note_position;
    let width = frame.width as usize;
    for y in min_y..max_y {
        for x in min_x..max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let outer = float_rounded_rect_coverage(px, py, key.rect, key_corner);
            let clip_coverage = rounded_rect_coverage(px, py, clip, corner_radius_px);
            let coverage = outer * clip_coverage;
            if coverage <= 0.0 {
                continue;
            }
            let color = if mask_mode {
                Rgba8::WHITE
            } else if key.real_key {
                let level = keyboard_vertical_level(key, py);
                let active = sampler.sample(px, py, band_position, false);
                let amount = (level * if key.black { 0.96 } else { 0.94 }).clamp(0.0, 1.0);
                mix_rgba(base, active, amount)
            } else {
                base
            };
            let offset = (y * width + x) * 4;
            blend_pixel(
                &mut frame.rgba[offset..offset + 4],
                color,
                opacity * coverage,
            );

            if mask_mode {
                continue;
            }

            let inner = KeyboardRect::new(
                key.rect.left + stroke_width,
                key.rect.top + stroke_width,
                key.rect.right - stroke_width,
                key.rect.bottom - stroke_width,
            );
            let inner_coverage = if inner.right > inner.left && inner.bottom > inner.top {
                float_rounded_rect_coverage(px, py, inner, (key_corner - stroke_width).max(0.0))
            } else {
                0.0
            };
            let stroke_coverage = (outer - inner_coverage).max(0.0) * clip_coverage;
            if stroke_coverage > 0.0 {
                blend_pixel(
                    &mut frame.rgba[offset..offset + 4],
                    stroke_color,
                    opacity * stroke_coverage,
                );
            }
        }
    }
}

fn keyboard_vertical_level(key: music_core::KeyPlacement, y: f32) -> f32 {
    let middle = (key.rect.top + key.rect.bottom) * 0.5;
    if y <= middle {
        let amount = ((y - key.rect.top) / (middle - key.rect.top).max(1.0e-6)).clamp(0.0, 1.0);
        let top = key.levels.center * key.levels.odd;
        top + (key.levels.center - top) * amount
    } else {
        let amount = ((y - middle) / (key.rect.bottom - middle).max(1.0e-6)).clamp(0.0, 1.0);
        let bottom = key.levels.center * key.levels.even;
        key.levels.center + (bottom - key.levels.center) * amount
    }
}

#[derive(Clone, Copy)]
struct MivKeyGradient {
    top: VisualizationColor,
    center: VisualizationColor,
    bottom: VisualizationColor,
}

fn miv_keyboard_vertical_color(
    gradient: MivKeyGradient,
    key: music_core::KeyPlacement,
    y: f32,
) -> Rgba8 {
    let middle = (key.rect.top + key.rect.bottom) * 0.5;
    let color = if y <= middle {
        let amount = ((y - key.rect.top) / (middle - key.rect.top).max(1.0e-6)).clamp(0.0, 1.0);
        miv_lerp_color(gradient.top, gradient.center, amount)
    } else {
        let amount = ((y - middle) / (key.rect.bottom - middle).max(1.0e-6)).clamp(0.0, 1.0);
        miv_lerp_color(gradient.center, gradient.bottom, amount)
    };
    rgba8(color)
}

fn rgba8(color: VisualizationColor) -> Rgba8 {
    Rgba8::new(color.r, color.g, color.b, color.a)
}

fn mix_rgba(from: Rgba8, to: Rgba8, amount: f32) -> Rgba8 {
    let amount = amount.clamp(0.0, 1.0);
    let channel = |from: u8, to: u8| {
        (from as f32 + (to as f32 - from as f32) * amount).clamp(0.0, 255.0) as u8
    };
    Rgba8::new(
        channel(from.r, to.r),
        channel(from.g, to.g),
        channel(from.b, to.b),
        channel(from.a, to.a),
    )
}

fn float_rounded_rect_coverage(x: f32, y: f32, rect: KeyboardRect, radius_px: f32) -> f32 {
    let width = rect.width();
    let height = rect.height();
    let radius = radius_px.max(0.0).min(width.min(height) * 0.5);
    let center_x = (rect.left + rect.right) * 0.5;
    let center_y = (rect.top + rect.bottom) * 0.5;
    let qx = (x - center_x).abs() - (width * 0.5 - radius);
    let qy = (y - center_y).abs() - (height * 0.5 - radius);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    let inside = qx.max(qy).min(0.0);
    (0.5 - (outside + inside - radius)).clamp(0.0, 1.0)
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
    style: &crate::TextStyle,
    transient_style: bool,
    fill: &Fill,
    opacity: f32,
    corner_radius_px: f32,
    mask_mode: bool,
) -> bool {
    let Some(bitmap) = assets.rasterized_text(text, font_key, size_px, style, transient_style)
    else {
        return false;
    };
    let logical_x = match align {
        TextAlign::Start => rect.x0 as f32,
        TextAlign::Center => rect.x0 as f32 + (rect.width() as f32 - bitmap.logical_width) * 0.5,
        TextAlign::End => rect.x1 as f32 - bitmap.logical_width,
    };
    let logical_y = rect.y0 as f32 + (rect.height() as f32 - bitmap.logical_height) * 0.5;
    let destination_x = logical_x.round() as i32 - bitmap.origin_offset.0 as i32;
    let destination_y = logical_y.round() as i32 - bitmap.origin_offset.1 as i32;
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
            if glyph_alpha <= 0.0 && bitmap.decoration.is_none() {
                continue;
            }
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let clip = rounded_rect_coverage(px, py, rect, corner_radius_px);
            let color = sampler.sample(px, py, None, mask_mode);
            let destination_offset = (y as usize * frame_width + x as usize) * 4;
            let (color, coverage) = if let Some(decoration) = &bitmap.decoration {
                (
                    composite_text_pixel(
                        &decoration[source_offset..source_offset + 4],
                        color,
                        bitmap.rgba[source_offset + 3],
                    ),
                    1.0,
                )
            } else {
                (color, glyph_alpha)
            };
            blend_pixel(
                &mut frame.rgba[destination_offset..destination_offset + 4],
                color,
                opacity * coverage * clip,
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
            miv_frame,
            point_count,
            shape,
            color_mode,
            fill,
            ..
        } => draw_spectrum_mask(
            &mut alpha,
            rect.width(),
            rect.height(),
            values,
            miv_frame,
            *point_count,
            *shape,
            *color_mode,
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
            style,
            transient_style,
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
                style,
                *transient_style,
                fill,
                item.opacity,
                item.corner_radius_px,
            ) {
                report.record_unresolved(UnresolvedAsset::Font(font_key.clone()));
            }
        }
        SceneItemKind::Text {
            text,
            font_key,
            size_px,
            align,
            style,
            transient_style,
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
                style,
                *transient_style,
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
    miv_frame: &MivSpectrumFrame,
    point_count: usize,
    shape: SpectrumShape,
    color_mode: MusicColorMode,
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
    if shape == SpectrumShape::MivHorizontal {
        draw_miv_spectrum_mask(
            alpha,
            rect,
            miv_frame,
            color_mode,
            fill,
            opacity,
            corner_radius_px,
        );
        return;
    }
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
            let miv_alpha = (color_mode == MusicColorMode::Miv)
                .then(|| miv_spectrum_color(bar.index, bar.total, bar.value).a);
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
                    let source_alpha = match miv_alpha {
                        None => solid_alpha.unwrap_or_else(|| {
                            sampler.sample(px, py, Some(bar.band_position), true).a
                        }),
                        Some(alpha) => alpha,
                    };
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
fn draw_miv_spectrum_mask(
    alpha: &mut [u8],
    clip_rect: PixelRect,
    visual: &MivSpectrumFrame,
    color_mode: MusicColorMode,
    fill: &Fill,
    opacity: f32,
    corner_radius_px: f32,
) {
    let sampler = FillSampler::new(fill, clip_rect);
    let width = clip_rect.width();
    let mut primitives = Vec::with_capacity(visual.bands.len() * 4);
    visit_miv_spectrum_primitives(
        visual,
        SpectrumVisualRect::new(
            0.0,
            0.0,
            clip_rect.width() as f32,
            clip_rect.height() as f32,
        ),
        |primitive| primitives.push(primitive),
    );
    let prepared_alphas = primitives
        .iter()
        .map(|primitive| {
            if let Some(alpha) = primitive.alpha_override {
                Some(alpha)
            } else if color_mode == MusicColorMode::Miv {
                Some(
                    miv_spectrum_color(
                        primitive.band_index,
                        primitive.band_count,
                        primitive.color_value,
                    )
                    .a,
                )
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    alpha
        .par_chunks_exact_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            let py = y as f32 + 0.5;
            for (primitive, prepared_alpha) in primitives.iter().zip(&prepared_alphas) {
                if py < primitive.rect.top || py >= primitive.rect.bottom {
                    continue;
                }
                let min_x = primitive.rect.left.floor().max(0.0) as usize;
                let max_x = primitive.rect.right.ceil().min(width as f32) as usize;
                let band_position = band_position(primitive.band_index, primitive.band_count);
                for x in min_x..max_x {
                    let px = x as f32 + 0.5;
                    let coverage =
                        visual_rect_coverage(px, py, primitive.rect, primitive.corner_radius_px)
                            * rounded_rect_coverage(px, py, clip_rect, corner_radius_px);
                    if coverage <= 0.0 {
                        continue;
                    }
                    let source_alpha = prepared_alpha
                        .unwrap_or_else(|| sampler.sample(px, py, Some(band_position), true).a);
                    blend_mask_pixel(&mut row[x], source_alpha, opacity * coverage);
                }
            }
        });
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
    style: &crate::TextStyle,
    transient_style: bool,
    fill: &Fill,
    opacity: f32,
    corner_radius_px: f32,
) -> bool {
    let Some(bitmap) = assets.rasterized_text(text, font_key, size_px, style, transient_style)
    else {
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
    let destination_x = logical_x.round() as i32 - bitmap.origin_offset.0 as i32;
    let destination_y = logical_y.round() as i32 - bitmap.origin_offset.1 as i32;
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
            if glyph_alpha == 0 && bitmap.decoration.is_none() {
                continue;
            }
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let fill_alpha = sampler.sample(px, py, None, true).a as u32;
            let source_alpha = if let Some(decoration) = &bitmap.decoration {
                let offset = (source_y * bitmap.width + source_x) * 4;
                composite_text_pixel(
                    &decoration[offset..offset + 4],
                    Rgba8::new(255, 255, 255, fill_alpha as u8),
                    glyph_alpha,
                )
                .a
            } else {
                ((glyph_alpha as u32 * fill_alpha + 127) / 255) as u8
            };
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

fn sample_bilinear_rgba(data: &[u8], width: usize, height: usize, x: f32, y: f32) -> [u8; 4] {
    if x < -0.5 || y < -0.5 || x > width as f32 - 0.5 || y > height as f32 - 0.5 {
        return [0; 4];
    }
    let x = x.clamp(0.0, width.saturating_sub(1) as f32);
    let y = y.clamp(0.0, height.saturating_sub(1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let mut sampled = [0; 4];
    for (channel, value) in sampled.iter_mut().enumerate() {
        let get = |px: usize, py: usize| data[(py * width + px) * 4 + channel] as f32;
        let top = get(x0, y0) * (1.0 - tx) + get(x1, y0) * tx;
        let bottom = get(x0, y1) * (1.0 - tx) + get(x1, y1) * tx;
        *value = (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8;
    }
    sampled
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GlowRasterPlan {
    scale: usize,
    spread_radius: usize,
    blur_radius: usize,
    pad: usize,
}

impl GlowRasterPlan {
    fn new(glow: Glow) -> Result<Self, RasterError> {
        let spread_px = glow.spread_px.ceil().max(0.0) as usize;
        let blur_px = glow.radius_px.ceil().max(1.0) as usize;
        let scale = if blur_px >= 16 {
            8
        } else if blur_px >= 8 {
            4
        } else if blur_px >= 4 {
            2
        } else {
            1
        };
        let spread_radius = spread_px.div_ceil(scale);
        let blur_radius = blur_px.div_ceil(scale);
        let filter_radius = spread_radius
            .checked_add(blur_radius)
            .ok_or(RasterError::DimensionsOverflow)?;
        // The reduced-space filters reach `filter_radius * scale` pixels. One further
        // block covers the max-downsample expansion (`scale - 1`) plus a zero-valued
        // guard pixel. Keeping pad aligned to scale also makes both source edges safe
        // regardless of the source dimensions.
        let pad = filter_radius
            .checked_add(1)
            .and_then(|radius| radius.checked_mul(scale))
            .ok_or(RasterError::DimensionsOverflow)?;
        Ok(Self {
            scale,
            spread_radius,
            blur_radius,
            pad,
        })
    }
}

fn draw_glow(frame: &mut Frame, source: &MaskSurface, glow: Glow) -> Result<(), RasterError> {
    if glow.radius_px <= 0.0 || glow.color.a == 0 {
        return Ok(());
    }
    let Some(bounds) = mask_bounds(source) else {
        return Ok(());
    };
    let plan = GlowRasterPlan::new(glow)?;
    let source_width = bounds.width();
    let source_height = bounds.height();
    let padding = plan
        .pad
        .checked_mul(2)
        .ok_or(RasterError::DimensionsOverflow)?;
    let width = source_width
        .checked_add(padding)
        .ok_or(RasterError::DimensionsOverflow)?;
    let height = source_height
        .checked_add(padding)
        .ok_or(RasterError::DimensionsOverflow)?;
    let len = width
        .checked_mul(height)
        .ok_or(RasterError::DimensionsOverflow)?;
    let mut alpha = vec![0; len];
    for y in 0..source_height {
        let destination = (y + plan.pad) * width + plan.pad;
        let source_offset = (bounds.y0 + y) * source.width + bounds.x0;
        alpha[destination..destination + source_width]
            .copy_from_slice(&source.alpha[source_offset..source_offset + source_width]);
    }
    let (mut reduced, reduced_width, reduced_height) = if plan.scale > 1 {
        downsample_max(&alpha, width, height, plan.scale)
    } else {
        (alpha, width, height)
    };
    if plan.spread_radius > 0 {
        reduced = max_filter(&reduced, reduced_width, reduced_height, plan.spread_radius);
    }
    let blurred = box_blur(&reduced, reduced_width, reduced_height, plan.blur_radius);
    let origin_x = source.origin_x + bounds.x0 as i32 - plan.pad as i32;
    let origin_y = source.origin_y + bounds.y0 as i32 - plan.pad as i32;
    composite_glow_mask(
        frame,
        &blurred,
        reduced_width,
        reduced_height,
        width,
        height,
        origin_x,
        origin_y,
        plan.scale,
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
    use music_core::{
        KeyboardAnimationState, KeyboardLevels, MivSpectrumState, SPECTRUM_BAND_COUNT,
        SPECTRUM_BAND_MIN_MIDI, SPECTRUM_NOTE_MIN_MIDI, SpectrumVisualRect, layout_keyboard,
        midi_to_hz, prepare_keyboard_frame, spectrum_axis_x,
    };

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
    fn miv_horizontal_peak_and_keyboard_key_share_the_same_pixel_axis() {
        let width = 640;
        let height = 240;
        let spectrum_rect = NormRect::new(0.05, 0.05, 0.9, 0.55);
        let keyboard_rect = NormRect::new(0.05, 0.65, 0.9, 0.2);
        let spectrum_pixels = norm_rect_to_pixels(spectrum_rect, width, height);
        let keyboard_pixels = norm_rect_to_pixels(keyboard_rect, width, height);
        let midi = 60_u8;
        let band_index = (midi - SPECTRUM_BAND_MIN_MIDI) as usize;
        let mut bands = vec![0.0; SPECTRUM_BAND_COUNT];
        bands[band_index] = 1.0;
        let mut visual_state = MivSpectrumState::default();
        let visual = visual_state.frame_at(&bands, 1.0).clone();
        let mut keyboard = KeyboardFrame::default();
        keyboard.levels[(midi - keyboard.note_min_midi) as usize] = KeyboardLevels {
            center: 1.0,
            even: 0.5,
            odd: 0.5,
        };

        let axis_rect = SpectrumVisualRect::new(
            spectrum_pixels.x0 as f32,
            spectrum_pixels.y0 as f32,
            spectrum_pixels.x1 as f32,
            spectrum_pixels.y1 as f32,
        );
        let peak_x = spectrum_axis_x(axis_rect, midi_to_hz(midi));
        let keyboard_layout = layout_keyboard(
            &keyboard,
            KeyboardRect::new(
                keyboard_pixels.x0 as f32,
                keyboard_pixels.y0 as f32,
                keyboard_pixels.x1 as f32,
                keyboard_pixels.y1 as f32,
            ),
        );
        let key = keyboard_layout
            .keys
            .iter()
            .find(|key| key.midi == midi)
            .expect("C4 key is visible");
        assert!((peak_x - key.rect.center_x()).abs() < 0.001);

        let scene = Scene {
            width,
            height,
            items: vec![
                SceneItem {
                    rect: spectrum_rect,
                    opacity: 1.0,
                    rotation_deg: 0.0,
                    corner_radius_px: 0.0,
                    glow: None,
                    kind: SceneItemKind::Spectrum {
                        values: bands,
                        miv_frame: visual,
                        point_count: SPECTRUM_BAND_COUNT,
                        shape: SpectrumShape::MivHorizontal,
                        color_mode: MusicColorMode::Miv,
                        miv_brightness: 0.0,
                        fill: Fill::Solid(Rgba8::WHITE),
                    },
                },
                SceneItem {
                    rect: keyboard_rect,
                    opacity: 1.0,
                    rotation_deg: 0.0,
                    corner_radius_px: 0.0,
                    glow: None,
                    kind: SceneItemKind::PitchKeyboard {
                        keyboard,
                        background: Fill::Solid(Rgba8::new(0, 0, 0, 0)),
                        color_mode: MusicColorMode::Miv,
                        miv_brightness: 0.0,
                        fill: Fill::Solid(Rgba8::WHITE),
                    },
                },
            ],
        };
        let mut first = Frame::new(1, 1).unwrap();
        let mut second = Frame::new(1, 1).unwrap();
        let _ = render_frame(&scene, &Assets::default(), &mut first).unwrap();
        let _ = render_frame(&scene, &Assets::default(), &mut second).unwrap();
        assert_eq!(first, second);

        let peak_column = peak_x.round() as usize;
        assert!((spectrum_pixels.y0..spectrum_pixels.y1 - 3).any(|y| pixel(
            &first,
            peak_column,
            y
        ) != [0, 0, 0, 255]));
        let key_y = ((key.rect.top + key.rect.bottom) * 0.5).round() as usize;
        let key_pixel = pixel(&first, peak_column, key_y);
        assert!(key_pixel[0] != key_pixel[1] || key_pixel[1] != key_pixel[2]);
    }

    #[test]
    fn pitch_keyboard_is_deterministic_and_uses_fill_opacity_and_glow() {
        let mut notes = vec![0.0; music_core::KEYBOARD_NOTE_COUNT];
        notes[(60 - SPECTRUM_NOTE_MIN_MIDI) as usize] = 0.9;
        notes[(64 - SPECTRUM_NOTE_MIN_MIDI) as usize] = 0.65;
        notes[(67 - SPECTRUM_NOTE_MIN_MIDI) as usize] = 0.7;
        let mut state = KeyboardAnimationState::default();
        let mut keyboard = KeyboardFrame::default();
        for _ in 0..4 {
            keyboard = prepare_keyboard_frame(&notes, SPECTRUM_NOTE_MIN_MIDI, 0.0, &mut state);
        }
        let keyboard_item = |fill, opacity, glow| SceneItem {
            rect: NormRect::new(0.1, 0.3, 0.8, 0.35),
            opacity,
            rotation_deg: 0.0,
            corner_radius_px: 6.0,
            glow,
            kind: SceneItemKind::PitchKeyboard {
                keyboard: keyboard.clone(),
                background: Fill::Solid(Rgba8::BLACK),
                color_mode: MusicColorMode::Custom,
                miv_brightness: 0.0,
                fill: Fill::Solid(fill),
            },
        };
        let glow = Some(Glow {
            color: Rgba8::new(0, 180, 255, 220),
            radius_px: 6.0,
            spread_px: 2.0,
        });
        let keyboard_scene = scene(
            320,
            120,
            keyboard_item(Rgba8::new(255, 40, 20, 255), 0.75, glow),
        );
        let mut first = Frame::new(1, 1).unwrap();
        let mut second = Frame::new(2, 2).unwrap();
        let _ = render_frame(&keyboard_scene, &Assets::default(), &mut first).unwrap();
        let _ = render_frame(&keyboard_scene, &Assets::default(), &mut second).unwrap();
        assert_eq!(first, second);

        let keyboard_rect = norm_rect_to_pixels(
            keyboard_scene.items[0].rect,
            keyboard_scene.width,
            keyboard_scene.height,
        );
        assert!(
            first
                .rgba
                .chunks_exact(4)
                .enumerate()
                .any(|(index, pixel)| {
                    let x = index % first.width as usize;
                    let y = index / first.width as usize;
                    (x < keyboard_rect.x0
                        || x >= keyboard_rect.x1
                        || y < keyboard_rect.y0
                        || y >= keyboard_rect.y1)
                        && pixel[..3] != [0, 0, 0]
                })
        );

        let mut other_fill = Frame::new(1, 1).unwrap();
        let _ = render_frame(
            &scene(
                320,
                120,
                keyboard_item(Rgba8::new(20, 255, 40, 255), 0.75, glow),
            ),
            &Assets::default(),
            &mut other_fill,
        )
        .unwrap();
        assert_ne!(first, other_fill);

        let mut other_opacity = Frame::new(1, 1).unwrap();
        let _ = render_frame(
            &scene(
                320,
                120,
                keyboard_item(Rgba8::new(255, 40, 20, 255), 0.35, glow),
            ),
            &Assets::default(),
            &mut other_opacity,
        )
        .unwrap();
        assert_ne!(first, other_opacity);
    }

    #[test]
    fn miv_palette_is_deterministic_and_does_not_use_the_custom_fill() {
        let mut notes = vec![0.0; music_core::KEYBOARD_NOTE_COUNT];
        notes[(60 - SPECTRUM_NOTE_MIN_MIDI) as usize] = 0.9;
        notes[(64 - SPECTRUM_NOTE_MIN_MIDI) as usize] = 0.65;
        notes[(67 - SPECTRUM_NOTE_MIN_MIDI) as usize] = 0.7;
        let mut state = KeyboardAnimationState::default();
        let keyboard = prepare_keyboard_frame(&notes, SPECTRUM_NOTE_MIN_MIDI, 6.0, &mut state);
        let make_scene = |fill, miv_brightness| Scene {
            width: 320,
            height: 160,
            items: vec![
                item(SceneItemKind::Spectrum {
                    values: vec![0.1, 0.8, 0.3, 1.0],
                    miv_frame: MivSpectrumFrame::default(),
                    point_count: 24,
                    shape: SpectrumShape::Bars {
                        gap_ratio: 0.25,
                        min_height_px: 2.0,
                        thickness_px: 8.0,
                    },
                    color_mode: MusicColorMode::Miv,
                    miv_brightness,
                    fill: Fill::Solid(fill),
                }),
                SceneItem {
                    rect: NormRect::new(0.05, 0.65, 0.9, 0.25),
                    opacity: 1.0,
                    rotation_deg: 0.0,
                    corner_radius_px: 0.0,
                    glow: None,
                    kind: SceneItemKind::PitchKeyboard {
                        keyboard: keyboard.clone(),
                        background: Fill::Solid(Rgba8::new(0, 0, 0, 0)),
                        color_mode: MusicColorMode::Miv,
                        miv_brightness,
                        fill: Fill::Solid(fill),
                    },
                },
            ],
        };
        let first_scene = make_scene(Rgba8::new(255, 0, 0, 255), 0.0);
        let second_scene = make_scene(Rgba8::new(0, 255, 0, 255), 0.0);
        let brighter_scene = make_scene(Rgba8::new(255, 0, 0, 255), 0.2);
        let mut first = Frame::new(1, 1).unwrap();
        let mut repeated = Frame::new(1, 1).unwrap();
        let mut other_fill = Frame::new(1, 1).unwrap();
        let mut brighter = Frame::new(1, 1).unwrap();
        let _ = render_frame(&first_scene, &Assets::default(), &mut first).unwrap();
        let _ = render_frame(&first_scene, &Assets::default(), &mut repeated).unwrap();
        let _ = render_frame(&second_scene, &Assets::default(), &mut other_fill).unwrap();
        let _ = render_frame(&brighter_scene, &Assets::default(), &mut brighter).unwrap();
        assert_eq!(first, repeated);
        assert_eq!(first, other_fill);
        assert_ne!(first, brighter);
        assert!(
            first
                .rgba
                .chunks_exact(4)
                .any(|pixel| { pixel[0] != pixel[1] || pixel[1] != pixel[2] })
        );
    }

    #[test]
    fn parallel_keyboard_matches_the_serial_pixel_order_bit_for_bit() {
        let mut notes = vec![0.0; music_core::KEYBOARD_NOTE_COUNT];
        for (index, level) in notes.iter_mut().enumerate() {
            *level = ((index * 37 % 101) as f32 / 100.0).powi(2);
        }
        let mut state = KeyboardAnimationState::default();
        let keyboard = prepare_keyboard_frame(&notes, SPECTRUM_NOTE_MIN_MIDI, 0.0, &mut state);
        let rect = PixelRect {
            x0: 13,
            y0: 17,
            x1: 387,
            y1: 143,
        };
        let keyboard_rect = KeyboardRect::new(
            rect.x0 as f32,
            rect.y0 as f32,
            rect.x1 as f32,
            rect.y1 as f32,
        );
        let layout = layout_keyboard(&keyboard, keyboard_rect);
        let background = Fill::Solid(Rgba8::new(11, 19, 31, 96));
        let fill = Fill::LinearGradient {
            stops: GradientStops::new(vec![
                GradientStop {
                    position: 0.0,
                    color: Rgba8::new(30, 220, 255, 255),
                },
                GradientStop {
                    position: 1.0,
                    color: Rgba8::new(255, 60, 170, 210),
                },
            ])
            .unwrap(),
            angle_deg: 17.0,
        };

        for corner_radius_px in [0.0, 6.0] {
            for mask_mode in [false, true] {
                let mut parallel = Frame::new(400, 160).unwrap();
                let mut serial = parallel.clone();
                draw_pitch_keyboard(
                    &mut parallel,
                    rect,
                    &keyboard,
                    &background,
                    MusicColorMode::Custom,
                    0.0,
                    &fill,
                    0.73,
                    corner_radius_px,
                    mask_mode,
                );

                draw_filled_rect(
                    &mut serial,
                    rect,
                    &background,
                    0.73,
                    corner_radius_px,
                    mask_mode,
                );
                let sampler = FillSampler::new(&fill, rect);
                for &x in &layout.octave_guides_x {
                    draw_keyboard_guide_serial(
                        &mut serial,
                        rect,
                        x,
                        0.73,
                        corner_radius_px,
                        mask_mode,
                    );
                }
                for &key in &layout.keys {
                    draw_keyboard_key_serial(
                        &mut serial,
                        rect,
                        key,
                        &sampler,
                        0.73,
                        corner_radius_px,
                        mask_mode,
                    );
                }
                assert_eq!(
                    parallel, serial,
                    "corner_radius_px={corner_radius_px}, mask_mode={mask_mode}"
                );
            }
        }
    }

    #[test]
    fn pitch_keyboard_key_edges_have_partial_analytic_coverage() {
        let keyboard = KeyboardFrame::default();
        let layout = layout_keyboard(&keyboard, KeyboardRect::new(0.0, 0.0, 320.0, 40.0));
        let black = layout.keys.into_iter().find(|key| key.black).unwrap();
        let min_x = black.rect.left.floor() as i32 - 1;
        let min_y = black.rect.top.floor() as i32 - 1;
        let max_x = black.rect.right.ceil() as i32 + 1;
        let max_y = black.rect.bottom.ceil() as i32 + 1;
        assert!((min_y..max_y).any(|y| {
            (min_x..max_x).any(|x| {
                let coverage =
                    float_rounded_rect_coverage(x as f32 + 0.5, y as f32 + 0.5, black.rect, 1.0);
                coverage > 0.0 && coverage < 1.0
            })
        }));
    }

    #[test]
    fn transparent_keyboard_background_preserves_pixels_behind_the_layer() {
        let base = Rgba8::new(17, 33, 65, 255);
        let render = |background| {
            let keyboard_rect = NormRect::new(0.1, 0.2, 0.8, 0.6);
            let scene = Scene {
                width: 320,
                height: 80,
                items: vec![
                    item(SceneItemKind::Solid {
                        fill: Fill::Solid(base),
                    }),
                    SceneItem {
                        rect: keyboard_rect,
                        opacity: 1.0,
                        rotation_deg: 0.0,
                        corner_radius_px: 0.0,
                        glow: None,
                        kind: SceneItemKind::PitchKeyboard {
                            keyboard: KeyboardFrame::default(),
                            background: Fill::Solid(background),
                            color_mode: MusicColorMode::Custom,
                            miv_brightness: 0.0,
                            fill: Fill::Solid(Rgba8::WHITE),
                        },
                    },
                ],
            };
            let rect = norm_rect_to_pixels(keyboard_rect, scene.width, scene.height);
            let mut frame = Frame::new(1, 1).unwrap();
            let _ = render_frame(&scene, &Assets::default(), &mut frame).unwrap();
            (frame, rect)
        };

        let (transparent, rect) = render(Rgba8::new(0, 0, 0, 0));
        let (opaque, _) = render(Rgba8::BLACK);
        assert!((rect.y0..rect.y1).any(|y| {
            (rect.x0..rect.x1).any(|x| pixel(&transparent, x, y) != pixel(&opaque, x, y))
        }));
        let transparent_energy: u64 = transparent.rgba.iter().map(|value| u64::from(*value)).sum();
        let opaque_energy: u64 = opaque.rgba.iter().map(|value| u64::from(*value)).sum();
        assert!(transparent_energy > opaque_energy);
    }

    #[test]
    fn parallel_box_blur_matches_serial_passes_bit_for_bit() {
        let width = 79_usize;
        let height = 53_usize;
        let radius = 7;
        let input = (0..width * height)
            .map(|index| {
                let x = index % width;
                let y = index / width;
                (x.wrapping_mul(29) ^ y.wrapping_mul(17)) as u8
            })
            .collect::<Vec<_>>();

        let mut horizontal = vec![0; input.len()];
        for (y, output) in horizontal.chunks_exact_mut(width).enumerate() {
            sliding_average(&input[y * width..(y + 1) * width], output, radius);
        }
        let mut transposed = vec![0; input.len()];
        for x in 0..width {
            for y in 0..height {
                transposed[x * height + y] = horizontal[y * width + x];
            }
        }
        let mut vertical = vec![0; input.len()];
        for (x, output) in vertical.chunks_exact_mut(height).enumerate() {
            sliding_average(&transposed[x * height..(x + 1) * height], output, radius);
        }
        let mut expected = vec![0; input.len()];
        for x in 0..height {
            for y in 0..width {
                expected[x * width + y] = vertical[y * height + x];
            }
        }

        assert_eq!(box_blur(&input, width, height, radius), expected);
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
            miv_frame: MivSpectrumFrame::default(),
            point_count: 31,
            shape: SpectrumShape::Radial {
                radius_px: 24.0,
                thickness_px: 3.0,
                start_angle_deg: -90.0,
                sweep_deg: 360.0,
                direction: SpectrumDirection::Clockwise,
            },
            color_mode: MusicColorMode::Custom,
            miv_brightness: 0.0,
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
            miv_frame: MivSpectrumFrame::default(),
            point_count: 37,
            shape: SpectrumShape::Radial {
                radius_px: 32.0,
                thickness_px: 2.0,
                start_angle_deg: -90.0,
                sweep_deg: 360.0,
                direction: SpectrumDirection::Clockwise,
            },
            color_mode: MusicColorMode::Custom,
            miv_brightness: 0.0,
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
            miv_frame: MivSpectrumFrame::default(),
            point_count: 200,
            shape: SpectrumShape::Radial {
                radius_px: 380.0,
                thickness_px: 4.0,
                start_angle_deg: -90.0,
                sweep_deg: 360.0,
                direction: SpectrumDirection::Clockwise,
            },
            color_mode: MusicColorMode::Custom,
            miv_brightness: 0.0,
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

    #[test]
    fn glow_tail_uses_actual_filter_support_at_scale_boundaries() {
        let source = MaskSurface {
            origin_x: 70,
            origin_y: 70,
            width: 40,
            height: 20,
            alpha: vec![255; 40 * 20],
        };
        let center_x = source.origin_x as usize + source.width / 2;

        for radius in [3_usize, 4, 8, 16, 20] {
            let scale = if radius >= 16 {
                8
            } else if radius >= 8 {
                4
            } else if radius >= 4 {
                2
            } else {
                1
            };
            let actual_pad = (radius.div_ceil(scale) + 1) * scale;
            let mut frame = Frame::new(180, 180).unwrap();
            draw_glow(
                &mut frame,
                &source,
                Glow {
                    color: Rgba8::WHITE,
                    radius_px: radius as f32,
                    spread_px: 0.0,
                },
            )
            .unwrap();

            let tail = (1..=actual_pad)
                .rev()
                .map(|distance| pixel(&frame, center_x, source.origin_y as usize - distance)[0])
                .collect::<Vec<_>>();
            assert_eq!(tail[0], 0, "radius={radius}, scale={scale}");
            assert!(
                tail.windows(2).all(|pair| pair[0] <= pair[1]),
                "glow tail is clipped or non-monotonic: radius={radius}, scale={scale}, tail={tail:?}"
            );
            assert!(tail.last().copied().unwrap_or_default() > 0);
            if scale > 1 {
                assert!(
                    ((radius + 1)..actual_pad).any(|distance| {
                        pixel(&frame, center_x, source.origin_y as usize - distance)[0] > 0
                    }),
                    "downsampled glow was cut at its nominal radius: radius={radius}, scale={scale}"
                );
            }
        }
    }

    fn image_scene(width: u32, height: u32, id: &str, fit: Fit) -> Scene {
        scene(
            width,
            height,
            item(SceneItemKind::Image {
                image: ImageId::new(id),
                fit,
                zoom: 1.0,
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
    fn image_zoom_resamples_about_the_center_and_is_byte_deterministic() {
        let id = ImageId::new("zoom-source");
        let assets = Assets::default();
        let source = patterned_image(12, 8, 7);
        let source_rgba = source.rgba.clone();
        assets.insert_image(id.clone(), source);
        let mut zoomed_scene = image_scene(12, 8, &id.0, Fit::Cover);
        let SceneItemKind::Image { zoom, .. } = &mut zoomed_scene.items[0].kind else {
            unreachable!()
        };
        *zoom = 1.2;
        let mut first = Frame::new(1, 1).unwrap();
        let mut second = Frame::new(1, 1).unwrap();
        let _ = render_frame(&zoomed_scene, &assets, &mut first).unwrap();
        let _ = render_frame(&zoomed_scene, &assets, &mut second).unwrap();
        assert_eq!(first, second);

        let x = 0;
        let y = 0;
        let source_x = (x as f32 + 0.5 - 6.0) / 1.2 + 6.0 - 0.5;
        let source_y = (y as f32 + 0.5 - 4.0) / 1.2 + 4.0 - 0.5;
        let expected = (0..4)
            .map(|channel| {
                sample_bilinear_rgba_channel(&source_rgba, 12, 8, source_x, source_y, channel)
            })
            .collect::<Vec<_>>();
        assert_eq!(pixel(&first, x, y), expected.as_slice());

        let mut identity = Frame::new(1, 1).unwrap();
        let _ = render_frame(
            &image_scene(12, 8, &id.0, Fit::Cover),
            &assets,
            &mut identity,
        )
        .unwrap();
        assert_ne!(pixel(&first, x, y), pixel(&identity, x, y));
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

// Composite at full layer opacity first. The caller applies clip/opacity once.
fn composite_text_pixel(decoration: &[u8], fill: Rgba8, coverage: u8) -> Rgba8 {
    let mut pixel: [u8; 4] = decoration.try_into().unwrap();
    let a = (fill.a as u32 * coverage as u32 + 127) / 255;
    let back = pixel[3] as u32 * (255 - a);
    let total = a * 255 + back;
    if total == 0 {
        return Rgba8::new(0, 0, 0, 0);
    }
    for (channel, source) in pixel[..3].iter_mut().zip([fill.r, fill.g, fill.b]) {
        *channel = ((source as u32 * a * 255 + *channel as u32 * back + total / 2) / total) as u8;
    }
    pixel[3] = ((total + 127) / 255) as u8;
    Rgba8::new(pixel[0], pixel[1], pixel[2], pixel[3])
}

#[cfg(test)]
#[path = "decoration_tests.rs"]
mod decoration_tests;
