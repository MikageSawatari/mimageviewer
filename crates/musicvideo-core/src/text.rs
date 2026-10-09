use crate::TextStyle;
use comic_core::{
    AnnotationObject, FontSet, Rgba, TextBlock, bake_overlay, layout_text, text_effect_padding,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const MAX_TEXT_CACHE_BYTES: usize = 64 * 1024 * 1024;
const TRANSIENT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TextCacheKey {
    text: String,
    font_key: String,
    size_bits: u32,
    style: Vec<u32>,
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
#[derive(Default)]
struct CacheState {
    stable: Lane,
    transient: Lane,
}
#[derive(Default)]
pub(crate) struct TextCache {
    entries: Mutex<CacheState>,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct TextCacheStats {
    pub stable_bytes: usize,
    pub transient_bytes: usize,
    pub entries: usize,
}
impl TextCache {
    pub fn stats(&self) -> TextCacheStats {
        let s = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        TextCacheStats {
            stable_bytes: s.stable.bytes,
            transient_bytes: s.transient.bytes,
            entries: s.stable.entries.len() + s.transient.entries.len(),
        }
    }
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
        {
            let mut s = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(v) = s.stable.get(&key).or_else(|| s.transient.get(&key)) {
                return Some(v);
            }
        }
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
        width.checked_mul(height)?.checked_mul(4)?;
        let bake = |block| {
            bake_overlay(
                &[AnnotationObject::new_text(
                    1,
                    (origin_offset.0 as f32, origin_offset.1 as f32),
                    block,
                )],
                width,
                height,
                fonts,
            )
            .pixels
        };
        let bitmap = Arc::new(TextBitmap {
            width,
            height,
            logical_width: layout.bounds.0,
            logical_height: layout.bounds.1,
            origin_offset,
            rgba: bake(glyph),
            decoration: style.has_paint().then(|| bake(decoration)),
        });
        let mut s = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let (lane, limit) = if transient {
            (&mut s.transient, TRANSIENT_BYTES)
        } else {
            (&mut s.stable, MAX_TEXT_CACHE_BYTES - TRANSIENT_BYTES)
        };
        if let Some(existing) = lane.get(&key) {
            return Some(existing);
        }
        lane.insert(key, Arc::clone(&bitmap), limit);
        Some(bitmap)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
            rgba: vec![255; n * 4],
            decoration: None,
        })
    }
    fn key(n: usize) -> TextCacheKey {
        TextCacheKey::new(&n.to_string(), "font", 32.0, &TextStyle::default())
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
