use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};

use comic_core::FontSet;
use rayon::prelude::*;

use crate::model::{Fit, ImageId, Rgba8};
use crate::text::{TextBitmap, TextCache};

pub const MAX_SCALED_IMAGE_CACHE_ENTRIES: usize = 16;

#[derive(Debug)]
pub struct ImageAsset {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub opaque: bool,
}

impl ImageAsset {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, AssetError> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(AssetError::DimensionsOverflow)?;
        if width == 0 || height == 0 || rgba.len() != expected {
            return Err(AssetError::InvalidImageBuffer {
                expected,
                actual: rgba.len(),
            });
        }
        let opaque = rgba.chunks_exact(4).all(|pixel| pixel[3] == 255);
        Ok(Self {
            width,
            height,
            rgba,
            opaque,
        })
    }
}

pub struct Assets {
    images: RwLock<HashMap<ImageId, ImageRecord>>,
    next_image_generation: AtomicU64,
    scaled_images: Mutex<ScaledImageCache>,
    scaled_image_ready: Condvar,
    pub fonts: Arc<FontSet>,
    text_cache: TextCache,
}

#[derive(Clone)]
struct ImageRecord {
    generation: u64,
    image: Arc<ImageAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScaledImageCacheKey {
    image: ImageId,
    width: u32,
    height: u32,
    fit: Fit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InFlightScale {
    key: ScaledImageCacheKey,
    generation: u64,
}

struct ScaledImageCacheEntry {
    key: ScaledImageCacheKey,
    generation: u64,
    image: Arc<ImageAsset>,
}

#[derive(Clone, Copy)]
struct ScaleGeometry {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    origin_x: f32,
    origin_y: f32,
    scale_x: f32,
    scale_y: f32,
}

#[derive(Default)]
struct ScaledImageCache {
    entries: VecDeque<ScaledImageCacheEntry>,
    in_flight: Vec<InFlightScale>,
    hits: u64,
    misses: u64,
    scale_operations: u64,
    evictions: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScaledImageCacheStats {
    pub entries: usize,
    pub capacity: usize,
    pub hits: u64,
    pub misses: u64,
    pub scale_operations: u64,
    pub evictions: u64,
}

impl Assets {
    pub fn new(fonts: FontSet) -> Self {
        Self {
            images: RwLock::new(HashMap::new()),
            next_image_generation: AtomicU64::new(1),
            scaled_images: Mutex::new(ScaledImageCache::default()),
            scaled_image_ready: Condvar::new(),
            fonts: Arc::new(fonts),
            text_cache: TextCache::default(),
        }
    }

    pub fn insert_image(&self, id: ImageId, image: ImageAsset) -> Arc<ImageAsset> {
        let image = Arc::new(image);
        self.insert_shared_image(id, Arc::clone(&image));
        image
    }

    pub fn insert_shared_image(&self, id: ImageId, image: Arc<ImageAsset>) {
        let generation = self.next_image_generation.fetch_add(1, Ordering::Relaxed);
        self.images
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id.clone(), ImageRecord { generation, image });
        self.scaled_images
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entries
            .retain(|entry| entry.key.image != id);
    }

    /// Creates an independent image bundle that shares only the requested
    /// immutable source images and the font collection.
    pub fn fork_retain_images(&self, retained: &[ImageId]) -> Self {
        let images = self
            .images
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let images = images
            .iter()
            .filter(|(id, _)| retained.iter().any(|retained_id| retained_id == *id))
            .map(|(id, record)| (id.clone(), record.clone()))
            .collect();
        Self {
            images: RwLock::new(images),
            next_image_generation: AtomicU64::new(
                self.next_image_generation.load(Ordering::Relaxed),
            ),
            scaled_images: Mutex::new(ScaledImageCache::default()),
            scaled_image_ready: Condvar::new(),
            fonts: Arc::clone(&self.fonts),
            text_cache: TextCache::default(),
        }
    }

    pub fn image(&self, id: &ImageId) -> Option<Arc<ImageAsset>> {
        self.image_record(id).map(|record| record.image)
    }

    fn image_record(&self, id: &ImageId) -> Option<ImageRecord> {
        self.images
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(id)
            .cloned()
    }

    pub fn has_image(&self, id: &ImageId) -> bool {
        self.images
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(id)
    }

    /// Removes an image and every prepared size derived from it.
    pub fn remove_image(&self, id: &ImageId) -> bool {
        let removed = self
            .images
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(id)
            .is_some();
        self.scaled_images
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entries
            .retain(|entry| &entry.key.image != id);
        removed
    }

    /// Returns an image rasterized for this ID, destination size, and fit mode.
    pub(crate) fn scaled_image(
        &self,
        id: &ImageId,
        width: u32,
        height: u32,
        fit: Fit,
    ) -> Result<Option<Arc<ImageAsset>>, AssetError> {
        let key = ScaledImageCacheKey {
            image: id.clone(),
            width,
            height,
            fit,
        };

        loop {
            let Some(source) = self.image_record(id) else {
                return Ok(None);
            };
            let in_flight = InFlightScale {
                key: key.clone(),
                generation: source.generation,
            };
            let mut cache = self
                .scaled_images
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(index) = cache
                .entries
                .iter()
                .position(|entry| entry.key == key && entry.generation == source.generation)
            {
                let Some(entry) = cache.entries.remove(index) else {
                    unreachable!()
                };
                let image = Arc::clone(&entry.image);
                cache.entries.push_back(entry);
                cache.hits += 1;
                return Ok(Some(image));
            }
            cache
                .entries
                .retain(|entry| entry.key != key || entry.generation == source.generation);

            if cache.in_flight.contains(&in_flight) {
                drop(
                    self.scaled_image_ready
                        .wait(cache)
                        .unwrap_or_else(|poisoned| poisoned.into_inner()),
                );
                continue;
            }
            cache.in_flight.push(in_flight.clone());
            cache.misses += 1;
            drop(cache);
            let prepared = prepare_image(&source.image, width, height, fit);
            let mut cache = self
                .scaled_images
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.in_flight.retain(|pending| pending != &in_flight);
            if prepared.as_ref().is_ok_and(|(_, scaled)| *scaled) {
                cache.scale_operations += 1;
            }

            let prepared = match prepared {
                Ok((prepared, _)) => prepared,
                Err(error) => {
                    self.scaled_image_ready.notify_all();
                    return Err(error);
                }
            };
            let images = self
                .images
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let current_generation = images.get(id).map(|record| record.generation);
            if current_generation != Some(source.generation) {
                self.scaled_image_ready.notify_all();
                drop(images);
                drop(cache);
                continue;
            }

            while cache.entries.len() >= MAX_SCALED_IMAGE_CACHE_ENTRIES {
                cache.entries.pop_front();
                cache.evictions += 1;
            }
            cache.entries.push_back(ScaledImageCacheEntry {
                key: key.clone(),
                generation: source.generation,
                image: Arc::clone(&prepared),
            });
            self.scaled_image_ready.notify_all();
            return Ok(Some(prepared));
        }
    }

    pub fn scaled_image_cache_stats(&self) -> ScaledImageCacheStats {
        let cache = self
            .scaled_images
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ScaledImageCacheStats {
            entries: cache.entries.len(),
            capacity: MAX_SCALED_IMAGE_CACHE_ENTRIES,
            hits: cache.hits,
            misses: cache.misses,
            scale_operations: cache.scale_operations,
            evictions: cache.evictions,
        }
    }

    pub(crate) fn rasterized_text(
        &self,
        text: &str,
        font_key: &str,
        size_px: f32,
        color: Rgba8,
    ) -> Option<Arc<TextBitmap>> {
        self.text_cache
            .get_or_rasterize(&self.fonts, text, font_key, size_px, color)
    }
}

fn prepare_image(
    source: &Arc<ImageAsset>,
    width: u32,
    height: u32,
    fit: Fit,
) -> Result<(Arc<ImageAsset>, bool), AssetError> {
    if width == 0 || height == 0 {
        return Err(AssetError::InvalidImageBuffer {
            expected: 1,
            actual: 0,
        });
    }
    if source.width == width && source.height == height {
        return Ok((Arc::clone(source), false));
    }
    let len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(AssetError::DimensionsOverflow)?;
    let mut rgba = vec![0; len];
    let destination_w = width as f32;
    let destination_h = height as f32;
    let source_w = source.width as f32;
    let source_h = source.height as f32;
    let geometry = match fit {
        Fit::Stretch => ScaleGeometry {
            x0: 0,
            y0: 0,
            x1: width as usize,
            y1: height as usize,
            origin_x: 0.0,
            origin_y: 0.0,
            scale_x: destination_w / source_w,
            scale_y: destination_h / source_h,
        },
        Fit::Contain => {
            let scale = (destination_w / source_w).min(destination_h / source_h);
            let draw_w = (source_w * scale).round().max(1.0) as usize;
            let draw_h = (source_h * scale).round().max(1.0) as usize;
            let x0 = (width as usize).saturating_sub(draw_w) / 2;
            let y0 = (height as usize).saturating_sub(draw_h) / 2;
            let x1 = (x0 + draw_w).min(width as usize);
            let y1 = (y0 + draw_h).min(height as usize);
            ScaleGeometry {
                x0,
                y0,
                x1,
                y1,
                origin_x: x0 as f32,
                origin_y: y0 as f32,
                scale_x: (x1 - x0) as f32 / source_w,
                scale_y: (y1 - y0) as f32 / source_h,
            }
        }
        Fit::Cover => {
            let scale = (destination_w / source_w).max(destination_h / source_h);
            ScaleGeometry {
                x0: 0,
                y0: 0,
                x1: width as usize,
                y1: height as usize,
                origin_x: (destination_w - source_w * scale) * 0.5,
                origin_y: (destination_h - source_h * scale) * 0.5,
                scale_x: scale,
                scale_y: scale,
            }
        }
    };
    let row_bytes = width as usize * 4;
    rgba.par_chunks_exact_mut(row_bytes)
        .enumerate()
        .filter(|(y, _)| *y >= geometry.y0 && *y < geometry.y1)
        .for_each(|(y, row)| {
            let source_y = (y as f32 + 0.5 - geometry.origin_y) / geometry.scale_y - 0.5;
            for x in geometry.x0..geometry.x1 {
                let source_x = (x as f32 + 0.5 - geometry.origin_x) / geometry.scale_x - 0.5;
                let pixel = sample_bilinear(source, source_x, source_y);
                row[x * 4..x * 4 + 4].copy_from_slice(&pixel);
            }
        });
    Ok((Arc::new(ImageAsset::new(width, height, rgba)?), true))
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

impl Default for Assets {
    fn default() -> Self {
        Self::new(FontSet::new())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetError {
    DimensionsOverflow,
    InvalidImageBuffer { expected: usize, actual: usize },
}

impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DimensionsOverflow => f.write_str("image dimensions overflow address space"),
            Self::InvalidImageBuffer { expected, actual } => {
                write!(
                    f,
                    "invalid RGBA buffer length: expected {expected}, got {actual}"
                )
            }
        }
    }
}

impl std::error::Error for AssetError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(value: u8) -> ImageAsset {
        ImageAsset::new(1, 1, vec![value, value, value, 255]).unwrap()
    }

    #[test]
    fn fork_shares_only_retained_images_and_replacement_is_isolated() {
        let assets = Assets::default();
        let retained_id = ImageId::new("retained");
        let removed_id = ImageId::new("removed");
        let retained = assets.insert_image(retained_id.clone(), pixel(10));
        let removed = assets.insert_image(removed_id.clone(), pixel(20));

        let fork = assets.fork_retain_images(std::slice::from_ref(&retained_id));
        assert!(Arc::ptr_eq(&fork.image(&retained_id).unwrap(), &retained));
        assert!(!fork.has_image(&removed_id));
        fork.insert_image(retained_id.clone(), pixel(30));
        assert_eq!(assets.image(&retained_id).unwrap().rgba[0], 10);
        assert_eq!(fork.image(&retained_id).unwrap().rgba[0], 30);

        drop(assets);
        assert_eq!(Arc::strong_count(&removed), 1);
        assert_eq!(Arc::strong_count(&retained), 1);
    }
}
