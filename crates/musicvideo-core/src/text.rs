use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use comic_core::{AnnotationObject, FontSet, Rgba, TextBlock, bake_overlay, layout_text};

use crate::model::Rgba8;

const TEXT_PADDING: usize = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TextCacheKey {
    text: String,
    font_key: String,
    size_bits: u32,
    color: Rgba8,
}

#[derive(Debug)]
pub(crate) struct TextBitmap {
    pub width: usize,
    pub height: usize,
    pub logical_width: f32,
    pub logical_height: f32,
    pub padding: usize,
    pub rgba: Vec<u8>,
}

#[derive(Default)]
pub(crate) struct TextCache {
    entries: Mutex<HashMap<TextCacheKey, Arc<TextBitmap>>>,
}

impl TextCache {
    pub fn get_or_rasterize(
        &self,
        fonts: &FontSet,
        text: &str,
        font_key: &str,
        size_px: f32,
        color: Rgba8,
    ) -> Option<Arc<TextBitmap>> {
        let size_px = if size_px.is_finite() {
            size_px.max(1.0)
        } else {
            1.0
        };
        let key = TextCacheKey {
            text: text.to_owned(),
            font_key: font_key.to_owned(),
            size_bits: size_px.to_bits(),
            color,
        };
        if let Some(bitmap) = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&key)
            .cloned()
        {
            return Some(bitmap);
        }

        let font = fonts.get(font_key)?;
        let block = TextBlock {
            text: text.to_owned(),
            font_key: font_key.to_owned(),
            size_px,
            color: Rgba {
                r: color.r,
                g: color.g,
                b: color.b,
                a: color.a,
            },
            ..TextBlock::default()
        };
        // comic-core remains the sole owner of shaping, metrics, and glyph
        // rasterization. This crate only caches and positions its RGBA result.
        let layout = layout_text(&block, font);
        let width = layout.bounds.0.ceil().max(1.0) as usize + TEXT_PADDING * 2;
        let height = layout.bounds.1.ceil().max(1.0) as usize + TEXT_PADDING * 2;
        let object =
            AnnotationObject::new_text(1, (TEXT_PADDING as f32, TEXT_PADDING as f32), block);
        let overlay = bake_overlay(&[object], width, height, fonts);
        let bitmap = Arc::new(TextBitmap {
            width,
            height,
            logical_width: layout.bounds.0,
            logical_height: layout.bounds.1,
            padding: TEXT_PADDING,
            rgba: overlay.pixels,
        });
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Some(Arc::clone(entries.entry(key).or_insert(bitmap)))
    }
}
