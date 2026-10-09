use crate::TextStyle;
use comic_core::{
    AnnotationObject, FontSet, PreparedTextRaster, Rgba, TextBlock, bake_overlay, layout_text,
    prepare_text_regions, text_effect_padding,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const MAX_TEXT_CACHE_BYTES: usize = 64 * 1024 * 1024;
// A full-HD RGBA pair needs ~16 MiB; keep 24 MiB for static results.
// Prepared multi-pass coverage is larger than the RGBA of a small text block,
// so give it 32 MiB and preserve the original 8 MiB transient RGBA lane.
const TRANSIENT_BYTES: usize = 40 * 1024 * 1024;
const PREPARED_BYTES: usize = 32 * 1024 * 1024;
/// Maximum combined RGBA allocation per text bake (cache retention is separate).
pub const MAX_TEXT_RASTER_BYTES: usize = 8 * 1024 * 1024;
const TILE_EDGE: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TextRasterArea {
    Full,
    Region {
        x: usize,
        y: usize,
        width: usize,
        height: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TextCacheKey {
    text: String,
    font_key: String,
    size_bits: u32,
    style: Vec<u32>,
    area: TextRasterArea,
}
impl TextCacheKey {
    fn new(text: &str, font_key: &str, size: f32, s: &TextStyle) -> Self {
        let mut style = vec![s.letter_gap.to_bits(), s.line_gap.to_bits()];
        let color = |c: Rgba| u32::from_le_bytes([c.r, c.g, c.b, c.a]);
        style.push(s.outline.is_some() as u32);
        if let Some(v) = s.outline {
            style.extend([color(v.color), v.width_px.to_bits()]);
        }
        style.push(s.extra_outlines.len() as u32);
        for v in &s.extra_outlines {
            style.extend([color(v.color), v.width_px.to_bits()]);
        }
        style.push(s.shadow.is_some() as u32);
        if let Some(v) = s.shadow {
            style.extend([
                color(v.color),
                v.offset.0.to_bits(),
                v.offset.1.to_bits(),
                v.blur_px.to_bits(),
                v.spread_px.to_bits(),
            ]);
        }
        style.push(s.echo.is_some() as u32);
        if let Some(v) = s.echo {
            style.extend([
                color(v.color),
                v.offset.0.to_bits(),
                v.offset.1.to_bits(),
                v.count,
            ]);
        }
        style.push(s.background.is_some() as u32);
        if let Some(v) = s.background {
            style.extend([color(v.fill), v.padding_px.to_bits(), v.corner_px.to_bits()]);
        }
        Self {
            text: text.to_owned(),
            font_key: font_key.to_owned(),
            size_bits: size.to_bits(),
            style,
            area: TextRasterArea::Full,
        }
    }
    fn bytes(&self) -> usize {
        self.text.capacity()
            + self.font_key.capacity()
            + self.style.capacity() * 4
            + std::mem::size_of::<Self>()
    }
}
#[derive(Debug)]
pub(crate) struct TextBitmap {
    pub width: usize,
    pub height: usize,
    pub logical_width: f32,
    pub logical_height: f32,
    pub origin_offset: (usize, usize),
    pub region_origin: (usize, usize),
    pub rgba: Vec<u8>,
    pub decoration: Option<Vec<u8>>,
}
struct Entry {
    bitmap: Arc<TextBitmap>,
    used: u64,
    bytes: usize,
}
#[derive(Default)]
struct Lane {
    entries: HashMap<TextCacheKey, Entry>,
    bytes: usize,
    clock: u64,
}
impl Lane {
    fn get(&mut self, key: &TextCacheKey) -> Option<Arc<TextBitmap>> {
        let entry = self.entries.get_mut(key)?;
        self.clock += 1;
        entry.used = self.clock;
        Some(Arc::clone(&entry.bitmap))
    }
    fn insert(&mut self, key: TextCacheKey, bitmap: Arc<TextBitmap>, limit: usize) {
        let bytes = key.bytes()
            + bitmap.rgba.capacity()
            + bitmap.decoration.as_ref().map_or(0, Vec::capacity)
            + std::mem::size_of::<TextBitmap>()
            + std::mem::size_of::<Entry>();
        // Oversized results are rendered but never retained.
        if bytes > limit {
            return;
        }
        while self.bytes + bytes > limit {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, v)| v.used)
                .map(|(k, _)| k.clone())
                .unwrap();
            self.bytes -= self.entries.remove(&oldest).unwrap().bytes;
        }
        self.clock += 1;
        self.bytes += bytes;
        self.entries.insert(
            key,
            Entry {
                bitmap,
                used: self.clock,
                bytes,
            },
        );
    }
}
// Prepared paint programs are an accelerator, independent of bitmap retention
// priority. They own their glyph Arc values, so mask/body, all tiles and later
// frames reuse them even if another comic bake resets the thread-local cache.
#[derive(Default)]
struct PreparedLane {
    entries: HashMap<TextCacheKey, (Arc<TextBake>, u64, usize)>,
    bytes: usize,
    clock: u64,
}
impl PreparedLane {
    fn get(&mut self, key: &TextCacheKey) -> Option<Arc<TextBake>> {
        let (bake, used, _) = self.entries.get_mut(key)?;
        self.clock += 1;
        *used = self.clock;
        Some(Arc::clone(bake))
    }
    fn publish(&mut self, key: TextCacheKey, bake: Arc<TextBake>) -> Arc<TextBake> {
        if let Some(existing) = self.get(&key) {
            return existing;
        }
        let bytes = key.bytes()
            + bake.retained_bytes()
            + std::mem::size_of::<(Arc<TextBake>, u64, usize)>();
        if bytes > PREPARED_BYTES {
            return bake;
        }
        while self.bytes + bytes > PREPARED_BYTES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used, _))| *used)
                .map(|(key, _)| key.clone())
                .unwrap();
            self.bytes -= self.entries.remove(&oldest).unwrap().2;
        }
        self.clock += 1;
        self.bytes += bytes;
        self.entries
            .insert(key, (Arc::clone(&bake), self.clock, bytes));
        bake
    }
}
#[derive(Default)]
struct CacheState {
    stable: Lane,
    transient: Lane,
    prepared: PreparedLane,
}
impl CacheState {
    fn lookup(&mut self, key: &TextCacheKey, transient: bool) -> Option<Arc<TextBitmap>> {
        if let Some(bitmap) = self.stable.get(key) {
            return Some(bitmap);
        }
        if transient {
            return self.transient.get(key);
        }
        // A static consumer owns retention priority regardless of producer order.
        let (key, entry) = self.transient.entries.remove_entry(key)?;
        self.transient.bytes -= entry.bytes;
        self.stable.insert(
            key,
            Arc::clone(&entry.bitmap),
            MAX_TEXT_CACHE_BYTES - TRANSIENT_BYTES,
        );
        Some(entry.bitmap)
    }
    fn publish(
        &mut self,
        key: TextCacheKey,
        bitmap: Arc<TextBitmap>,
        transient: bool,
    ) -> Arc<TextBitmap> {
        // Recheck both lanes under the same lock after concurrent raster misses.
        if let Some(existing) = self.lookup(&key, transient) {
            return existing;
        }
        let (lane, limit) = if transient {
            (&mut self.transient, TRANSIENT_BYTES - PREPARED_BYTES)
        } else {
            (&mut self.stable, MAX_TEXT_CACHE_BYTES - TRANSIENT_BYTES)
        };
        lane.insert(key, Arc::clone(&bitmap), limit);
        bitmap
    }
}
#[derive(Default)]
pub(crate) struct TextCache {
    entries: Mutex<CacheState>,
    #[cfg(feature = "text-measure")]
    preparation_ns: std::sync::atomic::AtomicU64,
    #[cfg(feature = "text-measure")]
    max_raster_bytes: std::sync::atomic::AtomicUsize,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct TextCacheStats {
    pub stable_bytes: usize,
    /// Non-static budget: transient RGBA plus prepared paint programs.
    pub transient_bytes: usize,
    /// Prepared programs, already included in transient_bytes (do not add twice).
    pub prepared_bytes: usize,
    pub entries: usize,
    /// Measurement-only cumulative layout/coverage/RGBA preparation time.
    #[cfg(feature = "text-measure")]
    pub preparation_ns: u64,
    /// Measurement-only high water of combined glyph/decoration RGBA per bake.
    #[cfg(feature = "text-measure")]
    pub max_raster_bytes: usize,
}
impl TextCache {
    pub fn stats(&self) -> TextCacheStats {
        let s = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        TextCacheStats {
            stable_bytes: s.stable.bytes,
            transient_bytes: s.transient.bytes + s.prepared.bytes,
            prepared_bytes: s.prepared.bytes,
            entries: s.stable.entries.len() + s.transient.entries.len() + s.prepared.entries.len(),
            #[cfg(feature = "text-measure")]
            preparation_ns: self
                .preparation_ns
                .load(std::sync::atomic::Ordering::Relaxed),
            #[cfg(feature = "text-measure")]
            max_raster_bytes: self
                .max_raster_bytes
                .load(std::sync::atomic::Ordering::Relaxed),
        }
    }
    #[cfg(test)]
    pub fn get_or_rasterize(
        &self,
        fonts: &FontSet,
        text: &str,
        font_key: &str,
        size_px: f32,
        style: &TextStyle,
        transient: bool,
    ) -> Option<Arc<TextBitmap>> {
        let size_px = if size_px.is_finite() {
            size_px.max(1.0)
        } else {
            1.0
        };
        let key = TextCacheKey::new(text, font_key, size_px, style);
        if let Some(bitmap) = self.lookup(&key, transient) {
            return Some(bitmap);
        }
        let bake = self.prepare(fonts, text, font_key, size_px, style, &key)?;
        (bake.bytes() <= MAX_TEXT_RASTER_BYTES).then(|| self.bake(fonts, &bake, key, transient))
    }

    fn prepare(
        &self,
        fonts: &FontSet,
        text: &str,
        font_key: &str,
        size_px: f32,
        style: &TextStyle,
        key: &TextCacheKey,
    ) -> Option<Arc<TextBake>> {
        if let Some(bake) = self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .prepared
            .get(key)
        {
            return Some(bake);
        }
        #[cfg(feature = "text-measure")]
        let start = std::time::Instant::now();
        let bake = Arc::new(TextBake::new(fonts, text, font_key, size_px, style)?);
        #[cfg(feature = "text-measure")]
        self.preparation_ns.fetch_add(
            start.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        if bake.painters.is_empty() {
            return Some(bake);
        }
        Some(
            self.entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .prepared
                .publish(key.clone(), bake),
        )
    }

    fn lookup(&self, key: &TextCacheKey, transient: bool) -> Option<Arc<TextBitmap>> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lookup(key, transient)
    }

    pub fn visit(
        &self,
        fonts: &FontSet,
        text: &str,
        font_key: &str,
        size_px: f32,
        style: &TextStyle,
        transient: bool,
        align: crate::TextAlign,
        rect: crate::raster::PixelRect,
        mut visit: impl FnMut(&TextBitmap, i32, i32),
    ) -> Option<()> {
        let size_px = if size_px.is_finite() {
            size_px.max(1.0)
        } else {
            1.0
        };
        let key = TextCacheKey::new(text, font_key, size_px, style);
        let destination = |logical_width: f32, logical_height: f32, origin: (usize, usize)| {
            let x = match align {
                crate::TextAlign::Start => rect.x0 as f32,
                crate::TextAlign::Center => {
                    rect.x0 as f32 + (rect.width() as f32 - logical_width) * 0.5
                }
                crate::TextAlign::End => rect.x1 as f32 - logical_width,
            };
            let y = rect.y0 as f32 + (rect.height() as f32 - logical_height) * 0.5;
            (
                x.round() as i32 - origin.0 as i32,
                y.round() as i32 - origin.1 as i32,
            )
        };
        if let Some(bitmap) = self.lookup(&key, transient) {
            let (x, y) = destination(
                bitmap.logical_width,
                bitmap.logical_height,
                bitmap.origin_offset,
            );
            visit(&bitmap, x, y);
            return Some(());
        }
        let bake = self.prepare(fonts, text, font_key, size_px, style, &key)?;
        let (dx, dy) = destination(bake.logical_width, bake.logical_height, bake.origin_offset);
        if bake.bytes() <= MAX_TEXT_RASTER_BYTES {
            let bitmap = self.bake(fonts, &bake, key, transient);
            visit(&bitmap, dx, dy);
        } else {
            // Intersect in the original full-canvas coordinates. Keep that origin
            // for comic-core's floating-point calculations and integer rounding.
            let x0 = (rect.x0 as i64 - dx as i64).clamp(0, bake.width as i64) as usize;
            let y0 = (rect.y0 as i64 - dy as i64).clamp(0, bake.height as i64) as usize;
            let x1 = (rect.x1 as i64 - dx as i64).clamp(0, bake.width as i64) as usize;
            let y1 = (rect.y1 as i64 - dy as i64).clamp(0, bake.height as i64) as usize;
            for y in (y0..y1).step_by(TILE_EDGE) {
                for x in (x0..x1).step_by(TILE_EDGE) {
                    let mut key = key.clone();
                    key.area = TextRasterArea::Region {
                        x,
                        y,
                        width: (x1 - x).min(TILE_EDGE),
                        height: (y1 - y).min(TILE_EDGE),
                    };
                    let bitmap = self
                        .lookup(&key, transient)
                        .unwrap_or_else(|| self.bake(fonts, &bake, key, transient));
                    visit(
                        &bitmap,
                        (dx as i64 + bitmap.region_origin.0 as i64) as i32,
                        (dy as i64 + bitmap.region_origin.1 as i64) as i32,
                    );
                }
            }
        }
        Some(())
    }

    fn bake(
        &self,
        fonts: &FontSet,
        prepared: &TextBake,
        key: TextCacheKey,
        transient: bool,
    ) -> Arc<TextBitmap> {
        let (x, y, width, height) = match key.area {
            TextRasterArea::Full => (0, 0, prepared.width, prepared.height),
            TextRasterArea::Region {
                x,
                y,
                width,
                height,
            } => (x, y, width, height),
        };
        let bytes = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(if prepared.painted { 8 } else { 4 }))
            .unwrap();
        assert!(
            bytes <= MAX_TEXT_RASTER_BYTES,
            "text bake exceeds allocation budget"
        );
        #[cfg(feature = "text-measure")]
        let start = std::time::Instant::now();
        let paint = |block: TextBlock, surface: usize| {
            let origin = (
                prepared.origin_offset.0 as f32,
                prepared.origin_offset.1 as f32,
            );
            match key.area {
                TextRasterArea::Full => bake_overlay(
                    &[AnnotationObject::new_text(1, origin, block)],
                    width,
                    height,
                    fonts,
                ),
                TextRasterArea::Region { .. } => {
                    prepared.painters[surface].bake_region((x as i32, y as i32), width, height)
                }
            }
            .pixels
        };
        let bitmap = Arc::new(TextBitmap {
            width,
            height,
            logical_width: prepared.logical_width,
            logical_height: prepared.logical_height,
            origin_offset: prepared.origin_offset,
            region_origin: (x, y),
            rgba: paint(prepared.glyph.clone(), 0),
            decoration: prepared
                .painted
                .then(|| paint(prepared.decoration.clone(), 1)),
        });
        #[cfg(feature = "text-measure")]
        {
            self.preparation_ns.fetch_add(
                start.elapsed().as_nanos() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            self.max_raster_bytes.fetch_max(
                bitmap.rgba.capacity() + bitmap.decoration.as_ref().map_or(0, Vec::capacity),
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .publish(key, bitmap, transient)
    }
}

struct TextBake {
    glyph: TextBlock,
    decoration: TextBlock,
    width: usize,
    height: usize,
    logical_width: f32,
    logical_height: f32,
    origin_offset: (usize, usize),
    painted: bool,
    painters: Vec<PreparedTextRaster>,
}
impl TextBake {
    fn retained_bytes(&self) -> usize {
        let blocks = [&self.glyph, &self.decoration];
        std::mem::size_of::<Self>()
            + blocks
                .iter()
                .map(|b| {
                    b.text.capacity()
                        + b.font_key.capacity()
                        + b.extra_outlines.capacity()
                            * std::mem::size_of::<comic_core::StrokeStyle>()
                })
                .sum::<usize>()
            + self.painters.capacity() * std::mem::size_of::<PreparedTextRaster>()
            + self
                .painters
                .iter()
                .map(PreparedTextRaster::retained_bytes)
                .sum::<usize>()
    }

    fn bytes(&self) -> usize {
        self.width
            .checked_mul(self.height)
            .and_then(|n| n.checked_mul(if self.painted { 8 } else { 4 }))
            .unwrap_or(usize::MAX)
    }
    fn new(
        fonts: &FontSet,
        text: &str,
        font_key: &str,
        size_px: f32,
        style: &TextStyle,
    ) -> Option<Self> {
        let font = fonts.get(font_key)?;
        let glyph = TextBlock {
            text: text.to_owned(),
            font_key: font_key.to_owned(),
            size_px,
            color: Rgba::WHITE,
            letter_gap: style.letter_gap,
            line_gap: style.line_gap,
            ..TextBlock::default()
        };
        let decoration = TextBlock {
            color: Rgba::TRANSPARENT,
            outline: style.outline,
            extra_outlines: style.extra_outlines.clone(),
            shadow: style.shadow,
            echo: style.echo,
            background: style.background,
            ..glyph.clone()
        };
        let layout = layout_text(&glyph, font);
        // Glyph overhang and negative spacing can extend ink beyond comic-core's
        // advance-based logical bounds. Preserve the legacy undecorated,
        // zero-spacing allocation exactly; styled text keeps the same logical
        // alignment anchor but includes ink bounds in both paint surfaces.
        // Track each side independently: a long run with negative letter gap
        // must not enlarge the vertical allocation by its horizontal reach.
        let mut ink_margins = [0.0_f32; 4];
        if style.has_paint() || style.letter_gap != 0.0 || style.line_gap != 0.0 {
            for g in &layout.glyphs {
                if let Some((x, y, w, h)) = font.glyph_px_bounds_gid(g.glyph_id, g.size) {
                    for (margin, reach) in ink_margins.iter_mut().zip([
                        -(g.x + x),
                        -(g.y + y),
                        g.x + x + w - layout.bounds.0,
                        g.y + y + h - layout.bounds.1,
                    ]) {
                        *margin = margin.max(reach);
                    }
                }
            }
        }
        let paint_padding = text_effect_padding(&decoration) + 1.0;
        let margins = ink_margins.map(|ink| (paint_padding + ink).ceil() as usize);
        let origin_offset = (margins[0], margins[1]);
        let width = (layout.bounds.0.ceil().max(1.0) as usize)
            .checked_add(margins[0].checked_add(margins[2])?)?;
        let height = (layout.bounds.1.ceil().max(1.0) as usize)
            .checked_add(margins[1].checked_add(margins[3])?)?;
        // Pixel coordinates in comic-core are i32, even for a sparse region.
        if width > i32::MAX as usize || height > i32::MAX as usize {
            return None;
        }
        let painted = style.has_paint();
        let bytes = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(if painted { 8 } else { 4 }))
            .unwrap_or(usize::MAX);
        let painters = if bytes > MAX_TEXT_RASTER_BYTES {
            let blocks = if painted {
                vec![&glyph, &decoration]
            } else {
                vec![&glyph]
            };
            prepare_text_regions(
                &layout,
                font,
                (origin_offset.0 as f32, origin_offset.1 as f32),
                &blocks,
            )
        } else {
            Vec::new()
        };
        Some(Self {
            width,
            height,
            logical_width: layout.bounds.0,
            logical_height: layout.bounds.1,
            origin_offset,
            glyph,
            decoration,
            painted,
            painters,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use comic_core::bake_text_region;
    fn assert_eviction_rgba(expected: &[u8], actual: &[u8]) {
        assert_eq!(actual, expected, "eviction must preserve glyph RGBA");
    }
    #[test]
    fn t1_prepared_tiles_reuse_one_logical_bake_across_mask_body_and_keep_total_budget() {
        let mut fonts = FontSet::new();
        fonts.insert(
            comic_core::LoadedFont::from_bytes(
                "font",
                include_bytes!("../../../tools/comic_lab/assets/fonts/ZenKurenaido-Regular.ttf")
                    .to_vec(),
            )
            .unwrap(),
        );
        let cache = TextCache::default();
        let style = TextStyle {
            echo: Some(crate::TextEchoStyle {
                offset: (512.0, 512.0),
                count: 12,
                ..Default::default()
            }),
            outline: Some(Default::default()),
            ..Default::default()
        };
        let key = TextCacheKey::new("日本語 A", "font", 64.0, &style);
        let first = cache
            .prepare(&fonts, "日本語 A", "font", 64.0, &style, &key)
            .unwrap();
        assert_eq!(first.painters.len(), 2);
        // The logical glyph rectangle, not merely an allocated transparent tile.
        let origin = (first.origin_offset.0 as i32, first.origin_offset.1 as i32);
        let width = first.logical_width.ceil() as usize;
        let height = first.logical_height.ceil() as usize;
        let before = first.painters[0].bake_region(origin, width, height).pixels;
        assert!(before.chunks_exact(4).any(|pixel| pixel[3] != 0));
        let rect = crate::raster::PixelRect {
            x0: 0,
            y0: 0,
            x1: 1920,
            y1: 1080,
        };
        // Transient 1024x1024 RGBA pairs cannot fit, but the coverage program can.
        for transient in [true, false, true] {
            cache
                .visit(
                    &fonts,
                    "日本語 A",
                    "font",
                    64.0,
                    &style,
                    transient,
                    crate::TextAlign::Center,
                    rect,
                    |bitmap, _, _| {
                        assert!(
                            bitmap.rgba.capacity() + bitmap.decoration.as_ref().unwrap().capacity()
                                <= MAX_TEXT_RASTER_BYTES
                        );
                    },
                )
                .unwrap();
            let again = cache
                .prepare(&fonts, "日本語 A", "font", 64.0, &style, &key)
                .unwrap();
            assert!(Arc::ptr_eq(&first, &again));
            let stats = cache.stats();
            assert!(stats.stable_bytes + stats.transient_bytes <= MAX_TEXT_CACHE_BYTES);
            #[cfg(feature = "text-measure")]
            {
                assert_eq!(stats.max_raster_bytes, MAX_TEXT_RASTER_BYTES);
                assert!(stats.preparation_ns > 0);
            }
        }
        // Many changing styles must evict old preparation without retaining all
        // dilation coverage. The oldest program stays usable through its Arc.
        for n in 0..300 {
            let mut next = style.clone();
            next.outline.as_mut().unwrap().width_px = 1.0 + n as f32 * 0.125;
            let key = TextCacheKey::new("日本語 ABCDEFGHIJKLMNOPQRSTUVWXYZ", "font", 64.0, &next);
            cache
                .prepare(
                    &fonts,
                    "日本語 ABCDEFGHIJKLMNOPQRSTUVWXYZ",
                    "font",
                    64.0,
                    &next,
                    &key,
                )
                .unwrap();
            assert!(cache.entries.lock().unwrap().prepared.bytes <= PREPARED_BYTES);
        }
        assert!(cache.entries.lock().unwrap().prepared.entries.len() < 300);
        assert!(cache.entries.lock().unwrap().prepared.get(&key).is_none());
        let after = first.painters[0].bake_region(origin, width, height).pixels;
        assert_eviction_rgba(&before, &after);
        let rebuilt = cache
            .prepare(&fonts, "日本語 A", "font", 64.0, &style, &key)
            .unwrap();
        assert!(!Arc::ptr_eq(&first, &rebuilt));
        assert_eviction_rgba(
            &before,
            &rebuilt.painters[0]
                .bake_region(origin, width, height)
                .pixels,
        );
        // Guard the oracle itself: reverting it to !is_empty() must fail this
        // test even if today's painter still happens to produce correct ink.
        let transparent = vec![0; before.len()];
        assert!(std::panic::catch_unwind(|| assert_eviction_rgba(&before, &transparent)).is_err());
    }
    #[test]
    fn t1_shared_effect_padding_matches_legacy_without_glow() {
        // Frozen pre-review formula, only as a compatibility oracle in this test.
        fn legacy_padding(block: &TextBlock) -> f32 {
            let mut pad = block.outline.map_or(0.0, |s| s.width_px).max(0.0);
            for s in &block.extra_outlines {
                pad = pad.max(s.width_px.max(0.0));
            }
            if let Some(bg) = block.background {
                pad = pad.max(bg.padding_px.max(0.0));
            }
            if let Some(sh) = block.shadow {
                pad = pad.max(
                    sh.spread_px.max(0.0)
                        + sh.blur_px.max(0.0)
                        + sh.offset.0.abs().max(sh.offset.1.abs()),
                );
            }
            if let Some(echo) = block.echo {
                pad = pad.max(
                    echo.offset.0.abs().max(echo.offset.1.abs()) * echo.count.clamp(1, 12) as f32,
                );
            }
            pad
        }
        // All presence combinations, fractional/negative widths, signed offsets,
        // and both echo count clamps. No font or raster fixture is needed.
        for enabled in 0..32 {
            for width in [-3.0, 0.0, 0.25, 2.5, 21.75] {
                for count in [0, 1, 3, 12, 13, u32::MAX] {
                    let stroke = crate::StrokeStyle {
                        width_px: width,
                        ..Default::default()
                    };
                    let block = TextBlock {
                        outline: (enabled & 1 != 0).then_some(stroke),
                        extra_outlines: if enabled & 2 != 0 {
                            vec![
                                stroke,
                                crate::StrokeStyle {
                                    width_px: width + 0.125,
                                    ..stroke
                                },
                            ]
                        } else {
                            Vec::new()
                        },
                        background: (enabled & 4 != 0).then_some(crate::TextBackgroundStyle {
                            padding_px: width * 2.0,
                            ..Default::default()
                        }),
                        shadow: (enabled & 8 != 0).then_some(crate::TextShadowStyle {
                            spread_px: width,
                            blur_px: width * 0.5,
                            offset: (-width, width * 0.25),
                            ..Default::default()
                        }),
                        echo: (enabled & 16 != 0).then_some(crate::TextEchoStyle {
                            offset: (width * 0.5, -width),
                            count,
                            ..Default::default()
                        }),
                        ..Default::default()
                    };
                    assert!(block.glow.is_none());
                    assert_eq!(
                        text_effect_padding(&block),
                        legacy_padding(&block),
                        "enabled={enabled}, width={width}, count={count}"
                    );
                }
            }
        }
    }
    fn bitmap(n: usize) -> Arc<TextBitmap> {
        Arc::new(TextBitmap {
            width: n,
            height: 1,
            logical_width: n as f32,
            logical_height: 1.0,
            origin_offset: (0, 0),
            region_origin: (0, 0),
            rgba: vec![255; n * 4],
            decoration: None,
        })
    }
    fn key(n: usize) -> TextCacheKey {
        TextCacheKey::new(&n.to_string(), "font", 32.0, &TextStyle::default())
    }
    #[test]
    fn t1_static_request_promotes_transient_hit_and_survives_pressure() {
        let mut fonts = FontSet::new();
        fonts.insert(
            comic_core::LoadedFont::from_bytes(
                "font",
                std::fs::read(r"C:\Windows\Fonts\YuGothM.ttc").unwrap(),
            )
            .unwrap(),
        );
        let cache = TextCache::default();
        let style = TextStyle::default();
        let first = cache
            .get_or_rasterize(&fonts, "Static", "font", 32.0, &style, true)
            .unwrap();
        assert_eq!(cache.stats().stable_bytes, 0);
        let second = cache
            .get_or_rasterize(&fonts, "Static", "font", 32.0, &style, false)
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cache.stats().transient_bytes, 0);
        let stable_bytes = cache.stats().stable_bytes;
        {
            let mut state = cache.entries.lock().unwrap();
            for n in 0..20 {
                state.publish(key(n), bitmap(300_000), true);
            }
        }
        let third = cache
            .get_or_rasterize(&fonts, "Static", "font", 32.0, &style, true)
            .unwrap();
        assert!(Arc::ptr_eq(&first, &third));
        assert_eq!(cache.stats().stable_bytes, stable_bytes);
    }
    #[test]
    fn t1_publication_after_parallel_misses_checks_both_lanes() {
        for first_transient in [false, true] {
            let mut state = CacheState::default();
            let key = key(1);
            // Both workers missed before either completed rasterization.
            assert!(state.lookup(&key, true).is_none());
            assert!(state.lookup(&key, false).is_none());
            let first = bitmap(20);
            let second = bitmap(20);
            let retained = state.publish(key.clone(), first.clone(), first_transient);
            let reused = state.publish(key.clone(), second, !first_transient);
            assert!(Arc::ptr_eq(&retained, &reused));
            assert_eq!(state.stable.entries.len(), 1);
            assert!(state.transient.entries.is_empty());
            assert_eq!(state.transient.bytes, 0);
            assert_eq!(state.stable.bytes, state.stable.entries[&key].bytes);
        }
    }
    #[test]
    fn t1_maximum_ui_effects_are_bounded_before_baking_and_tiles_match_uncropped_pixels() {
        let mut fonts = FontSet::new();
        fonts.insert(
            comic_core::LoadedFont::from_bytes(
                "font",
                std::fs::read(r"C:\Windows\Fonts\YuGothM.ttc").unwrap(),
            )
            .unwrap(),
        );
        let width = 1050;
        let height = 1100;
        let rect = crate::raster::PixelRect {
            x0: 0,
            y0: 0,
            x1: width,
            y1: height,
        };
        for (offset, painted_extras) in [
            ((512.0, 0.0), false),
            ((0.0, -512.0), false),
            ((512.0, 512.0), false),
            ((-512.0, -512.0), false),
            ((512.0, -512.0), true),
            ((-512.0, 512.0), true),
        ] {
            let style = TextStyle {
                echo: Some(crate::TextEchoStyle {
                    offset,
                    count: 12,
                    ..Default::default()
                }),
                outline: painted_extras.then_some(crate::StrokeStyle {
                    width_px: 128.0,
                    ..Default::default()
                }),
                extra_outlines: if painted_extras {
                    vec![crate::StrokeStyle {
                        width_px: 128.0,
                        ..Default::default()
                    }]
                } else {
                    Vec::new()
                },
                shadow: painted_extras.then_some(crate::TextShadowStyle {
                    offset: (512.0, -512.0),
                    blur_px: 128.0,
                    spread_px: 128.0,
                    ..Default::default()
                }),
                background: painted_extras.then_some(crate::TextBackgroundStyle {
                    padding_px: 256.0,
                    corner_px: 256.0,
                    ..Default::default()
                }),
                letter_gap: if painted_extras { 256.0 } else { 0.0 },
                line_gap: if painted_extras { -128.0 } else { 0.0 },
            };
            let cache = TextCache::default();
            let prepared = TextBake::new(&fonts, "A", "font", 63.25, &style).unwrap();
            assert!(
                prepared.bytes() > 1024 * 1024 * 1024,
                "fixture reproduces the old >1GiB allocation"
            );
            let origin = (
                prepared.origin_offset.0 as f32,
                prepared.origin_offset.1 as f32,
            );
            let top = ((height as f32 - prepared.logical_height) * 0.5).round() as i32;
            // One unsplit visible-region reference, using the original canvas
            // coordinates; comic-core separately tests this against full baking.
            let region = (
                prepared.origin_offset.0 as i32,
                prepared.origin_offset.1 as i32 - top,
            );
            let expected_glyph =
                bake_text_region(&prepared.glyph, origin, region, width, height, &fonts).pixels;
            let expected_decoration =
                bake_text_region(&prepared.decoration, origin, region, width, height, &fonts)
                    .pixels;
            for transient in [true, false] {
                let mut glyph = vec![0; width * height * 4];
                let mut decoration = glyph.clone();
                let mut tiles = 0;
                cache
                    .visit(
                        &fonts,
                        "A",
                        "font",
                        63.25,
                        &style,
                        transient,
                        crate::TextAlign::Start,
                        rect,
                        |bitmap, dx, dy| {
                            tiles += 1;
                            let paint = bitmap.decoration.as_ref().unwrap();
                            assert!(
                                bitmap.rgba.capacity() + paint.capacity() <= MAX_TEXT_RASTER_BYTES
                            );
                            for y in 0..bitmap.height {
                                let source = y * bitmap.width * 4;
                                let target = ((dy as usize + y) * width + dx as usize) * 4;
                                glyph[target..target + bitmap.width * 4].copy_from_slice(
                                    &bitmap.rgba[source..source + bitmap.width * 4],
                                );
                                decoration[target..target + bitmap.width * 4]
                                    .copy_from_slice(&paint[source..source + bitmap.width * 4]);
                            }
                        },
                    )
                    .unwrap();
                assert!(tiles > 1);
                assert_eq!(
                    glyph, expected_glyph,
                    "glyph offset={offset:?}, extras={painted_extras}"
                );
                assert_eq!(
                    decoration, expected_decoration,
                    "decoration offset={offset:?}, extras={painted_extras}"
                );
            }
        }
    }
    #[test]
    fn t1_lru_evicts_least_recently_used_by_bytes_and_bypasses_oversized() {
        let mut lane = Lane::default();
        lane.insert(key(1), bitmap(20), 10000);
        let cost = lane.bytes;
        let limit = cost * 2 + 16;
        lane.insert(key(2), bitmap(20), limit);
        assert!(lane.get(&key(1)).is_some());
        lane.insert(key(3), bitmap(20), limit);
        assert!(lane.get(&key(1)).is_some());
        assert!(lane.get(&key(2)).is_none());
        assert!(lane.get(&key(3)).is_some());
        assert!(lane.bytes <= limit);
        lane.insert(key(4), bitmap(limit), limit);
        assert!(lane.get(&key(4)).is_none());
        assert_eq!(lane.entries.len(), 2);
    }
    #[test]
    fn t1_transient_pressure_cannot_evict_stable_entry() {
        let mut state = CacheState::default();
        let stable = bitmap(20);
        state.stable.insert(key(1), stable.clone(), 10000);
        for n in 2..100 {
            state.transient.insert(key(n), bitmap(20), 1000);
            assert!(state.transient.bytes <= 1000);
        }
        assert!(Arc::ptr_eq(&state.stable.get(&key(1)).unwrap(), &stable));
        assert!(state.transient.entries.len() < 10);
    }
    #[test]
    fn t1_style_cache_key_includes_all_effects_colors_spacing_and_font() {
        let a = TextStyle::default();
        let k = TextCacheKey::new("text", "font", 32.0, &a);
        for b in [
            TextStyle {
                letter_gap: 1.0,
                ..a.clone()
            },
            TextStyle {
                line_gap: 1.0,
                ..a.clone()
            },
            TextStyle {
                outline: Some(Default::default()),
                ..a.clone()
            },
            TextStyle {
                extra_outlines: vec![Default::default()],
                ..a.clone()
            },
            TextStyle {
                shadow: Some(Default::default()),
                ..a.clone()
            },
            TextStyle {
                echo: Some(Default::default()),
                ..a.clone()
            },
            TextStyle {
                background: Some(Default::default()),
                ..a.clone()
            },
        ] {
            assert_ne!(k, TextCacheKey::new("text", "font", 32.0, &b));
        }
        let b = TextStyle {
            outline: Some(Default::default()),
            ..a
        };
        let mut c = b.clone();
        c.outline.as_mut().unwrap().color = Rgba::WHITE;
        assert_ne!(
            TextCacheKey::new("text", "font", 32.0, &b),
            TextCacheKey::new("text", "font", 32.0, &c)
        );
        assert_ne!(
            k,
            TextCacheKey::new("text", "other", 32.0, &TextStyle::default())
        );
    }
    #[test]
    fn t1_cache_reuses_identical_input_and_rebakes_identically_after_eviction() {
        let mut fonts = FontSet::new();
        fonts.insert(
            comic_core::LoadedFont::from_bytes(
                "font",
                std::fs::read(r"C:\Windows\Fonts\YuGothM.ttc").unwrap(),
            )
            .unwrap(),
        );
        let cache = TextCache::default();
        let style = TextStyle {
            outline: Some(Default::default()),
            ..Default::default()
        };
        let a = cache
            .get_or_rasterize(&fonts, "日本語", "font", 32.0, &style, false)
            .unwrap();
        let b = cache
            .get_or_rasterize(&fonts, "日本語", "font", 32.0, &style, true)
            .unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        cache.entries.lock().unwrap().stable = Lane::default();
        let c = cache
            .get_or_rasterize(&fonts, "日本語", "font", 32.0, &style, false)
            .unwrap();
        assert_eq!(a.rgba, c.rgba);
        assert_eq!(a.decoration, c.decoration);
    }
}
