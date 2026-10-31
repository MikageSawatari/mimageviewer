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

#[cfg(test)]
pub(crate) mod glyph_preparation_probe {
    use std::cell::Cell;

    thread_local! {
        static DRY_RUN: Cell<Option<usize>> = const { Cell::new(None) };
    }

    struct Restore(Option<usize>);

    impl Drop for Restore {
        fn drop(&mut self) {
            DRY_RUN.with(|probe| probe.set(self.0));
        }
    }

    pub(crate) fn with_dry_run<R>(run: impl FnOnce() -> R) -> (R, usize) {
        let restore = Restore(DRY_RUN.with(|probe| probe.replace(Some(0))));
        let result = run();
        let completed = DRY_RUN.with(|probe| probe.take().unwrap());
        drop(restore);
        (result, completed)
    }

    pub(super) fn stop_after_checked_plan() -> bool {
        DRY_RUN.with(|probe| match probe.get() {
            Some(completed) => {
                probe.set(Some(completed + 1));
                true
            }
            None => false,
        })
    }
}

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
            if bake.painters.is_empty() || !existing.painters.is_empty() {
                return existing;
            }
            // A small full bitmap may later be requested through a tiny visible
            // window. Replace its layout-only preparation with region painters,
            // accounting their coverage in the same prepared-cache budget.
            self.bytes -= self.entries.remove(&key).unwrap().2;
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
        visit: impl FnMut(&TextBitmap, i32, i32),
    ) -> Option<()> {
        self.visit_window(
            fonts, text, font_key, size_px, style, transient, align, rect, rect, visit,
        )
    }

    /// Align against the logical layer rectangle, but only rasterize the visible
    /// window. The window is in the same full-canvas coordinates as `rect`.
    pub fn visit_window(
        &self,
        fonts: &FontSet,
        text: &str,
        font_key: &str,
        size_px: f32,
        style: &TextStyle,
        transient: bool,
        align: crate::TextAlign,
        rect: crate::raster::PixelRect,
        clip: crate::raster::PixelRect,
        mut visit: impl FnMut(&TextBitmap, i32, i32),
    ) -> Option<()> {
        let clip = crate::raster::PixelRect {
            x0: rect.x0.max(clip.x0),
            y0: rect.y0.max(clip.y0),
            x1: rect.x1.min(clip.x1),
            y1: rect.y1.min(clip.y1),
        };
        if clip.is_empty() {
            return Some(());
        }
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
        let visible_budget = clip
            .width()
            .saturating_mul(clip.height())
            .saturating_mul(if bake.painted { 8 } else { 4 })
            .saturating_mul(4);
        if bake.bytes() <= MAX_TEXT_RASTER_BYTES.min(visible_budget) {
            let bitmap = self.bake(fonts, &bake, key, transient);
            visit(&bitmap, dx, dy);
        } else {
            // Intersect before preparing painters or visiting tiles: even an
            // enormous logical text canvas must not enumerate invisible tiles.
            let x0 = (clip.x0 as i64 - dx as i64).clamp(0, bake.width as i64) as usize;
            let y0 = (clip.y0 as i64 - dy as i64).clamp(0, bake.height as i64) as usize;
            let x1 = (clip.x1 as i64 - dx as i64).clamp(0, bake.width as i64) as usize;
            let y1 = (clip.y1 as i64 - dy as i64).clamp(0, bake.height as i64) as usize;
            if x0 >= x1 || y0 >= y1 {
                return Some(());
            }
            let bake = if bake.painters.is_empty() {
                let regions = Arc::new(bake.with_regions(fonts)?);
                self.entries
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .prepared
                    .publish(key.clone(), regions)
            } else {
                bake
            };
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
    fn with_regions(&self, fonts: &FontSet) -> Option<Self> {
        let font = fonts.get(&self.glyph.font_key)?;
        let layout = layout_text(&self.glyph, font);
        let blocks = if self.painted {
            vec![&self.glyph, &self.decoration]
        } else {
            vec![&self.glyph]
        };
        Some(Self {
            glyph: self.glyph.clone(),
            decoration: self.decoration.clone(),
            width: self.width,
            height: self.height,
            logical_width: self.logical_width,
            logical_height: self.logical_height,
            origin_offset: self.origin_offset,
            painted: self.painted,
            painters: prepare_text_regions(
                &layout,
                font,
                (self.origin_offset.0 as f32, self.origin_offset.1 as f32),
                &blocks,
            ),
        })
    }

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
        // Test-only preflight includes comic-core's intermediate glyph masks,
        // which exist before the visible text tiles. Reject a mutation's huge
        // glyph before prepare_text_regions or baking can allocate its coverage.
        #[cfg(test)]
        {
            let outline = style
                .extra_outlines
                .iter()
                .chain(style.outline.iter())
                .map(|stroke| stroke.width_px.max(0.0))
                .fold(0.0_f32, f32::max);
            let shadow = style.shadow.map_or(0.0, |shadow| {
                shadow.spread_px.max(0.0) + shadow.blur_px.clamp(0.0, 48.0)
            });
            let dilation = outline.max(shadow);
            // font::dilate_coverage keeps one extra antialiasing pixel beyond
            // ceil(dilate), while the undilated base has no padding.
            let pad = if dilation > 0.0 {
                (dilation.ceil() as u32).checked_add(1)?
            } else {
                0
            };
            for placement in &layout.glyphs {
                if let Some((_, _, width, height)) =
                    font.glyph_px_bounds_gid(placement.glyph_id, placement.size)
                {
                    let width = (width.ceil().max(0.0) as u32).checked_add(pad.checked_mul(2)?)?;
                    let height =
                        (height.ceil().max(0.0) as u32).checked_add(pad.checked_mul(2)?)?;
                    crate::raster::frame_allocation_probe::check(width, height).ok()?;
                }
            }
            if glyph_preparation_probe::stop_after_checked_plan() {
                return None;
            }
        }
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
    fn window_test_fonts() -> FontSet {
        let mut fonts = FontSet::new();
        fonts.insert(
            comic_core::LoadedFont::from_bytes(
                "font",
                include_bytes!("../../../tools/comic_lab/assets/fonts/ZenKurenaido-Regular.ttf")
                    .to_vec(),
            )
            .unwrap(),
        );
        fonts
    }

    #[test]
    fn d17_nine_scale_99_all_text_kinds_bound_glyph_preparation_and_visible_work() {
        use crate::raster::frame_allocation_probe;
        use crate::render_limits::*;
        use crate::*;

        let mut project = default_project(10.0);
        project.output.width = 1920;
        project.output.height = 1080;
        let layers = [
            new_text_layer(&project),
            new_time_text_layer(&project),
            new_lyrics_layer(&project),
        ];
        let output_pixels = u64::from(project.output.width) * u64::from(project.output.height);
        let edge_limit = project.output.width.max(project.output.height) * 4;
        for (index, mut layer) in layers.into_iter().enumerate() {
            layer.rect = Animatable::Const(NormRect::FULL);
            layer.glow = Animatable::Const(None);
            layer.modulations = vec![ModulationBinding {
                target: ModulationTarget::Scale,
                amount: 99.0,
                modulator: ModulatorId(1),
                curve: ModulationCurve::Linear,
            }];
            let (font_key, size, align, style) = match &mut layer.kind {
                LayerKind::Text(text) => {
                    text.text = "H".into();
                    (
                        &mut text.font_key,
                        &mut text.size_px,
                        &mut text.align,
                        &mut text.style,
                    )
                }
                LayerKind::TimeText(text) => (
                    &mut text.font_key,
                    &mut text.size_px,
                    &mut text.align,
                    &mut text.style,
                ),
                LayerKind::Lyrics(text) => {
                    text.lines = vec![LyricLine {
                        text: "H".into(),
                        time_secs: Some(0.0),
                    }];
                    (
                        &mut text.font_key,
                        &mut text.size_px,
                        &mut text.align,
                        &mut text.style,
                    )
                }
                _ => unreachable!(),
            };
            *font_key = "font".into();
            *size = MAX_TEXT_SIZE_PX;
            *align = TextAlign::Center;
            *style = Animatable::Const(TextStyle {
                outline: Some(StrokeStyle {
                    width_px: MAX_TEXT_OUTLINE_PX,
                    ..Default::default()
                }),
                extra_outlines: vec![StrokeStyle {
                    width_px: MAX_TEXT_OUTLINE_PX,
                    ..Default::default()
                }],
                shadow: Some(TextShadowStyle {
                    offset: (0.0, 0.0),
                    spread_px: MAX_TEXT_SHADOW_SPREAD_PX,
                    blur_px: 48.0,
                    ..Default::default()
                }),
                ..Default::default()
            });
            project.layers = vec![layer];
            // Fresh assets keep every route's first glyph preparation observable.
            let fonts = window_test_fonts();
            let font = fonts.get("font").unwrap();
            let (_, _, glyph_width, glyph_height) = font
                .glyph_px_bounds(if index == 1 { '0' } else { 'H' }, MAX_TEXT_SIZE_PX)
                .unwrap();
            let aa_padding = 2 * ((MAX_TEXT_SHADOW_SPREAD_PX + 48.0).ceil() as u32 + 1);
            let expected_coverage = (
                glyph_width.ceil() as u32 + aa_padding,
                glyph_height.ceil() as u32 + aa_padding,
            );
            let assets = Assets::new(fonts);
            let scene = evaluate(&project, 1.0, &crate::p5a_tests::frame());
            assert_eq!(
                scene.items[0].rect,
                NormRect::new(-49.5, -49.5, 100.0, 100.0)
            );
            let mut frame = Frame::new(1, 1).unwrap();
            let ((result, completed_plans), dimensions) =
                frame_allocation_probe::with_limits(output_pixels * 3, edge_limit, || {
                    glyph_preparation_probe::with_dry_run(|| {
                        render_frame(&scene, &assets, &mut frame)
                    })
                });
            let report = result.unwrap_or_else(|error| {
                panic!("text kind {index}: {error:?}; requested {dimensions:?}")
            });
            // The upper-bound fixture stops after the checked plan, before
            // preparing the large 1000px multi-pass coverage. A cap mutation is
            // rejected by the dimension check before it can complete this plan.
            assert!(
                completed_plans > 0,
                "text kind {index}: no bounded glyph plan"
            );
            assert!(
                !report.unresolved_assets.is_empty(),
                "dry-run must stop before baking"
            );
            assert!(
                dimensions.contains(&expected_coverage),
                "glyph preparation missing for kind {index}: {expected_coverage:?} in {dimensions:?}"
            );
            for &(width, height) in &dimensions {
                assert!(u64::from(width) * u64::from(height) <= output_pixels * 3);
                assert!(width.max(height) <= edge_limit);
            }
            // Exercise the same production route with normal coverage sizes.
            // Keeping Scale99 also keeps this on the no-Glow visible-tile path.
            let (size, style) = match &mut project.layers[0].kind {
                LayerKind::Text(text) => (&mut text.size_px, &mut text.style),
                LayerKind::TimeText(text) => (&mut text.size_px, &mut text.style),
                LayerKind::Lyrics(text) => (&mut text.size_px, &mut text.style),
                _ => unreachable!(),
            };
            *size = 1.0;
            *style = Animatable::Const(TextStyle {
                outline: Some(StrokeStyle {
                    width_px: 0.01,
                    ..Default::default()
                }),
                shadow: Some(TextShadowStyle {
                    offset: (0.0, 0.0),
                    spread_px: 0.01,
                    blur_px: 0.01,
                    ..Default::default()
                }),
                ..Default::default()
            });
            let scene = evaluate(&project, 1.0, &crate::p5a_tests::frame());
            let (result, dimensions) =
                frame_allocation_probe::with_limits(output_pixels * 3, edge_limit, || {
                    render_frame(&scene, &assets, &mut frame)
                });
            let report = result.unwrap();
            assert!(
                report.unresolved_assets.is_empty(),
                "text kind {index}: {report:?}"
            );
            for &(width, height) in &dimensions {
                assert!(u64::from(width) * u64::from(height) <= output_pixels * 3);
                assert!(width.max(height) <= edge_limit);
            }
        }
    }

    #[test]
    fn p5a_text_window_preserves_full_alignment_and_clips_below_fixed_budget() {
        let fonts = window_test_fonts();
        let rect = crate::raster::PixelRect {
            x0: 100,
            y0: 200,
            x1: 192_100,
            y1: 108_200,
        };
        for painted in [false, true] {
            let style = TextStyle {
                outline: painted.then_some(crate::StrokeStyle {
                    width_px: 7.0,
                    ..Default::default()
                }),
                shadow: painted.then_some(crate::TextShadowStyle {
                    offset: (13.0, -9.0),
                    blur_px: 8.0,
                    spread_px: 2.0,
                    ..Default::default()
                }),
                ..Default::default()
            };
            let reference = TextCache::default()
                .get_or_rasterize(
                    &fonts,
                    "\u{65e5}\u{672c}\u{8a9e} Ag",
                    "font",
                    128.0,
                    &style,
                    false,
                )
                .unwrap();
            let ink = reference
                .rgba
                .chunks_exact(4)
                .enumerate()
                .find(|(n, pixel)| {
                    pixel[3] != 0
                        && n % reference.width >= reference.origin_offset.0
                        && n / reference.width >= reference.origin_offset.1
                })
                .unwrap()
                .0;
            let source_x = ink % reference.width;
            let source_y = ink / reference.width;
            assert!(reference.rgba.len() < MAX_TEXT_RASTER_BYTES);
            for align in [
                crate::TextAlign::Start,
                crate::TextAlign::Center,
                crate::TextAlign::End,
            ] {
                let dx = match align {
                    crate::TextAlign::Start => rect.x0 as f32,
                    crate::TextAlign::Center => {
                        rect.x0 as f32 + (rect.width() as f32 - reference.logical_width) * 0.5
                    }
                    crate::TextAlign::End => rect.x1 as f32 - reference.logical_width,
                }
                .round() as i32
                    - reference.origin_offset.0 as i32;
                let dy = (rect.y0 as f32 + (rect.height() as f32 - reference.logical_height) * 0.5)
                    .round() as i32
                    - reference.origin_offset.1 as i32;
                let clip = crate::raster::PixelRect {
                    x0: (dx + source_x as i32) as usize,
                    y0: (dy + source_y as i32) as usize,
                    x1: (dx + source_x as i32) as usize + 19,
                    y1: (dy + source_y as i32) as usize + 17,
                };
                let cache = TextCache::default();
                let key = TextCacheKey::new("\u{65e5}\u{672c}\u{8a9e} Ag", "font", 128.0, &style);
                let first = cache
                    .prepare(
                        &fonts,
                        "\u{65e5}\u{672c}\u{8a9e} Ag",
                        "font",
                        128.0,
                        &style,
                        &key,
                    )
                    .unwrap();
                assert!(first.painters.is_empty());
                let mut visited = 0;
                cache
                    .visit_window(
                        &fonts,
                        "\u{65e5}\u{672c}\u{8a9e} Ag",
                        "font",
                        128.0,
                        &style,
                        false,
                        align,
                        rect,
                        clip,
                        |bitmap, x, y| {
                            visited += 1;
                            assert!(bitmap.width <= clip.width() && bitmap.height <= clip.height());
                            assert!(x >= clip.x0 as i32 && y >= clip.y0 as i32);
                            assert!(x as usize + bitmap.width <= clip.x1);
                            assert!(y as usize + bitmap.height <= clip.y1);
                            for by in 0..bitmap.height {
                                for bx in 0..bitmap.width {
                                    let expected = (((y - dy) as usize + by) * reference.width
                                        + (x - dx) as usize
                                        + bx)
                                        * 4;
                                    let actual = (by * bitmap.width + bx) * 4;
                                    assert_eq!(
                                        bitmap.rgba[actual..actual + 4],
                                        reference.rgba[expected..expected + 4]
                                    );
                                    if painted {
                                        assert_eq!(
                                            bitmap.decoration.as_ref().unwrap()[actual..actual + 4],
                                            reference.decoration.as_ref().unwrap()
                                                [expected..expected + 4]
                                        );
                                    }
                                }
                            }
                            assert!(bitmap.rgba.chunks_exact(4).any(|p| p[3] != 0));
                        },
                    )
                    .unwrap();
                assert_eq!(visited, 1);
                let again = cache
                    .prepare(
                        &fonts,
                        "\u{65e5}\u{672c}\u{8a9e} Ag",
                        "font",
                        128.0,
                        &style,
                        &key,
                    )
                    .unwrap();
                assert!(!again.painters.is_empty());
                assert!(!Arc::ptr_eq(&first, &again));
                let state = cache.entries.lock().unwrap();
                assert_eq!(
                    state.prepared.bytes,
                    state
                        .prepared
                        .entries
                        .iter()
                        .map(|(key, (bake, _, _))| {
                            key.bytes()
                                + bake.retained_bytes()
                                + std::mem::size_of::<(Arc<TextBake>, u64, usize)>()
                        })
                        .sum::<usize>()
                );
                assert!(state.prepared.bytes <= PREPARED_BYTES);
                assert!(
                    state
                        .stable
                        .entries
                        .keys()
                        .all(|k| k.area != TextRasterArea::Full)
                );
            }
        }
    }

    #[test]
    fn p5a_text_window_huge_effect_canvas_only_bakes_visible_tile() {
        let fonts = window_test_fonts();
        let style = TextStyle {
            echo: Some(crate::TextEchoStyle {
                offset: (50_000.0, -50_000.0),
                count: 12,
                ..Default::default()
            }),
            outline: Some(Default::default()),
            ..Default::default()
        };
        let prepared = TextBake::new(&fonts, "A", "font", 64.0, &style).unwrap();
        assert!(prepared.width > 1_000_000 && prepared.height > 1_000_000);
        let rect = crate::raster::PixelRect {
            x0: 0,
            y0: 0,
            x1: 192_000,
            y1: 108_000,
        };
        let dx = -(prepared.origin_offset.0 as i32);
        let dy = ((rect.height() as f32 - prepared.logical_height) * 0.5).round() as i32
            - prepared.origin_offset.1 as i32;
        let clip = crate::raster::PixelRect {
            x0: 0,
            y0: 53_990,
            x1: 64,
            y1: 54_054,
        };
        let region = (clip.x0 as i32 - dx, clip.y0 as i32 - dy);
        let origin = (
            prepared.origin_offset.0 as f32,
            prepared.origin_offset.1 as f32,
        );
        let glyph = bake_text_region(
            &prepared.glyph,
            origin,
            region,
            clip.width(),
            clip.height(),
            &fonts,
        )
        .pixels;
        let decoration = bake_text_region(
            &prepared.decoration,
            origin,
            region,
            clip.width(),
            clip.height(),
            &fonts,
        )
        .pixels;
        assert!(glyph.chunks_exact(4).any(|p| p[3] != 0));
        let mut count = 0;
        TextCache::default()
            .visit_window(
                &fonts,
                "A",
                "font",
                64.0,
                &style,
                false,
                crate::TextAlign::Start,
                rect,
                clip,
                |bitmap, x, y| {
                    count += 1;
                    assert_eq!((x, y), (clip.x0 as i32, clip.y0 as i32));
                    assert_eq!((bitmap.width, bitmap.height), (clip.width(), clip.height()));
                    assert_eq!(bitmap.rgba, glyph);
                    assert_eq!(bitmap.decoration.as_ref().unwrap(), &decoration);
                    assert!(
                        bitmap.rgba.capacity() + bitmap.decoration.as_ref().unwrap().capacity()
                            <= clip.width() * clip.height() * 8
                    );
                },
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn p5a_text_window_reuses_existing_full_bitmap_without_rebaking() {
        let fonts = window_test_fonts();
        let cache = TextCache::default();
        let style = TextStyle::default();
        let full = cache
            .get_or_rasterize(&fonts, "ABC", "font", 32.0, &style, false)
            .unwrap();
        let rect = crate::raster::PixelRect {
            x0: 0,
            y0: 0,
            x1: 640,
            y1: 320,
        };
        let clip = crate::raster::PixelRect {
            x0: 1,
            y0: 155,
            x1: 2,
            y1: 156,
        };
        let mut count = 0;
        cache
            .visit_window(
                &fonts,
                "ABC",
                "font",
                32.0,
                &style,
                false,
                crate::TextAlign::Start,
                rect,
                clip,
                |bitmap, _, _| {
                    count += 1;
                    assert!(std::ptr::eq(bitmap, full.as_ref()));
                },
            )
            .unwrap();
        assert_eq!(count, 1);
        let mut via_legacy = Vec::new();
        let mut via_window = Vec::new();
        cache
            .visit(
                &fonts,
                "ABC",
                "font",
                32.0,
                &style,
                false,
                crate::TextAlign::Center,
                rect,
                |bitmap, x, y| via_legacy.push((bitmap.rgba.clone(), x, y)),
            )
            .unwrap();
        cache
            .visit_window(
                &fonts,
                "ABC",
                "font",
                32.0,
                &style,
                false,
                crate::TextAlign::Center,
                rect,
                rect,
                |bitmap, x, y| via_window.push((bitmap.rgba.clone(), x, y)),
            )
            .unwrap();
        assert_eq!(via_legacy, via_window);
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
