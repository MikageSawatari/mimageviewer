use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use comic_core::FontSet;

use crate::model::{ImageId, Rgba8};
use crate::text::{TextBitmap, TextCache};

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
    pub images: RwLock<HashMap<ImageId, Arc<ImageAsset>>>,
    pub fonts: FontSet,
    text_cache: TextCache,
}

impl Assets {
    pub fn new(fonts: FontSet) -> Self {
        Self {
            images: RwLock::new(HashMap::new()),
            fonts,
            text_cache: TextCache::default(),
        }
    }

    pub fn insert_image(&self, id: ImageId, image: ImageAsset) -> Arc<ImageAsset> {
        let image = Arc::new(image);
        self.images
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, Arc::clone(&image));
        image
    }

    pub fn image(&self, id: &ImageId) -> Option<Arc<ImageAsset>> {
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
