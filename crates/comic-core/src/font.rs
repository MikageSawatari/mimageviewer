//! Font loading, OpenType shaping, and glyph rasterization.
//!
//! Stack (per docs/vertical-text-opentype-plan.md, 案B):
//! - **`rustybuzz`** = the single font parser + HarfBuzz shaper. Vertical text is
//!   shaped with `Direction::TopToBottom`, so the font's `vert` feature is
//!   applied automatically and we get the correct vertical glyph forms
//!   (`。、…ー` brackets) + vertical metrics — no per-character workarounds.
//! - **`ab_glyph_rasterizer`** = the coverage engine (the same rasterizer
//!   `ab_glyph` uses internally). We feed it `rustybuzz`/`ttf-parser` glyph
//!   outlines, so there is ONE font parser and the AA quality matches the old
//!   `ab_glyph` path. 袋文字 dilation / `rotate_cw` operate on the coverage.
//!
//! 縦中横 / 横倒し / column arrangement / kinsoku wrap stay in `layout.rs`
//! (they are layout composition, not font features).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ab_glyph_rasterizer::{Point, Rasterizer, point};
use rustybuzz::ttf_parser::{GlyphId, OutlineBuilder};
use rustybuzz::{Direction, Face, UnicodeBuffer};

/// One shaped glyph (output of `shape_vertical` / `shape_horizontal`): the
/// substituted glyph id, the source cluster (byte index into the shaped text),
/// and the offsets/advances in PIXELS at the requested size. For vertical text
/// `y_advance` is negative (the pen moves down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    pub gid: u16,
    pub cluster: u32,
    pub x_offset: f32,
    pub y_offset: f32,
    pub x_advance: f32,
    pub y_advance: f32,
}

// Owns the font bytes + a borrowing `rustybuzz::Face` parsed ONCE (self-cell makes
// the self-reference safe). Rebuilding the Face per call was a severe perf bug
// (~46µs each on a .ttc) — caching it makes metric/shape/raster calls ~free.
self_cell::self_cell!(
    struct FaceCell {
        owner: Vec<u8>,
        #[covariant]
        dependent: RbFace,
    }
);
type RbFace<'a> = Face<'a>;

/// Process-unique id assigned to each loaded font, used as the glyph-cache key
/// prefix so two fonts never collide (the per-bake cache is shared across fonts).
static FONT_ID_SEQ: AtomicU64 = AtomicU64::new(1);

/// Per-bake glyph coverage cache (thread-local). Memoizes the expensive outline
/// scan-conversion (`base`) and the dilated halos (`dilated`) so every repeated
/// glyph, effect pass, and text block within one bake reuses the result. Cleared
/// by [`reset_glyph_cache`] at the start of each overlay bake, so memory stays
/// bounded to roughly one page's glyphs.
#[derive(Default)]
struct GlyphCache {
    base: HashMap<(u64, u16, u32), Arc<GlyphBitmap>>,
    dilated: HashMap<(u64, u16, u32, u32), Arc<GlyphBitmap>>,
}

thread_local! {
    static GLYPH_CACHE: RefCell<GlyphCache> = RefCell::new(GlyphCache::default());
}

/// Clear the per-bake glyph cache. Called at the start of every overlay bake
/// (`bake_overlay_with_stamps`) so the cache does not accumulate across pages.
pub fn reset_glyph_cache() {
    GLYPH_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.base.clear();
        c.dilated.clear();
    });
}

/// A single loaded font face: the bytes + the parsed face (cached) + design metrics.
pub struct LoadedFont {
    pub key: String,
    /// Process-unique id (glyph-cache key prefix).
    id: u64,
    cell: FaceCell,
    upem: f32,
    ascent_fu: f32,
    descent_fu: f32, // negative
    line_gap_fu: f32,
}

impl LoadedFont {
    /// Load a face from raw TTF/OTF/TTC bytes. For TTC, the first face is used.
    pub fn from_bytes(key: impl Into<String>, bytes: Vec<u8>) -> Result<Self, String> {
        let cell = FaceCell::try_new(bytes, |b| {
            Face::from_slice(b, 0).ok_or_else(|| "failed to parse font".to_string())
        })?;
        let face = cell.borrow_dependent();
        let upem = (face.units_per_em() as f32).max(1.0);
        let ascent_fu = face.ascender() as f32;
        let descent_fu = face.descender() as f32;
        let line_gap_fu = face.line_gap() as f32;
        Ok(LoadedFont {
            key: key.into(),
            id: FONT_ID_SEQ.fetch_add(1, Ordering::Relaxed),
            cell,
            upem,
            ascent_fu,
            descent_fu,
            line_gap_fu,
        })
    }

    /// The parsed font face (cached; cheap to call).
    fn face(&self) -> &RbFace<'_> {
        self.cell.borrow_dependent()
    }

    pub fn units_per_em(&self) -> f32 {
        self.upem
    }

    /// Horizontal advance (in px) for a char at the given pixel size.
    pub fn h_advance(&self, ch: char, size_px: f32) -> f32 {
        let s = size_px / self.upem;
        let face = self.face();
        if let Some(gid) = face.glyph_index(ch)
            && let Some(adv) = face.glyph_hor_advance(gid)
        {
            return adv as f32 * s;
        }
        size_px
    }

    /// Font ascent (positive, px) at the given size.
    pub fn ascent(&self, size_px: f32) -> f32 {
        self.ascent_fu * size_px / self.upem
    }

    /// Font descent magnitude (positive, px) at the given size.
    pub fn descent(&self, size_px: f32) -> f32 {
        -self.descent_fu * size_px / self.upem
    }

    /// Line height (ascent + descent + line_gap) at the given size.
    pub fn line_height(&self, size_px: f32) -> f32 {
        (self.ascent_fu - self.descent_fu + self.line_gap_fu) * size_px / self.upem
    }

    /// Glyph id for `ch` (0 / .notdef when the font lacks it).
    pub fn glyph_id(&self, ch: char) -> u16 {
        self.face().glyph_index(ch).map(|g| g.0).unwrap_or(0)
    }

    /// True if the font has a real (non-.notdef) glyph for `ch`.
    pub fn covers(&self, ch: char) -> bool {
        self.face().glyph_index(ch).is_some_and(|g| g.0 != 0)
    }

    /// Ink height (px) of a char's outline bounding box. 0 for no outline.
    pub fn glyph_height(&self, ch: char, size_px: f32) -> f32 {
        self.glyph_px_bounds(ch, size_px)
            .map(|(_, _, _, h)| h)
            .unwrap_or(0.0)
    }

    /// Ink bounding box of a char's outline at `size_px`, as
    /// `(min_x, min_y, width, height)` relative to the pen origin (x at pen,
    /// y at baseline; `min_y` negative = above baseline). None for no outline.
    pub fn glyph_px_bounds(&self, ch: char, size_px: f32) -> Option<(f32, f32, f32, f32)> {
        self.glyph_px_bounds_gid(self.glyph_id(ch), size_px)
    }

    /// Like `glyph_px_bounds` but by glyph id (for shaped/substituted glyphs).
    pub fn glyph_px_bounds_gid(&self, gid: u16, size_px: f32) -> Option<(f32, f32, f32, f32)> {
        let face = self.face();
        let mut nb = NoopOutline;
        let r = face.outline_glyph(GlyphId(gid), &mut nb)?;
        let s = size_px / self.upem;
        Some((
            r.x_min as f32 * s,
            -(r.y_max as f32) * s,
            (r.x_max - r.x_min) as f32 * s,
            (r.y_max - r.y_min) as f32 * s,
        ))
    }

    /// Shape a run with the given direction, returning substituted glyph ids +
    /// offsets/advances in pixels. Vertical shaping (`TopToBottom`) applies the
    /// font's `vert` feature automatically (HarfBuzz default), giving the correct
    /// vertical glyph forms and metrics.
    pub fn shape_run(&self, text: &str, size_px: f32, vertical: bool) -> Vec<ShapedGlyph> {
        let face = self.face();
        let mut buf = UnicodeBuffer::new();
        buf.push_str(text);
        buf.set_direction(if vertical {
            Direction::TopToBottom
        } else {
            Direction::LeftToRight
        });
        let gb = rustybuzz::shape(face, &[], buf);
        let s = size_px / self.upem;
        let infos = gb.glyph_infos();
        let pos = gb.glyph_positions();
        infos
            .iter()
            .zip(pos.iter())
            .map(|(i, p)| ShapedGlyph {
                gid: i.glyph_id as u16,
                cluster: i.cluster,
                x_offset: p.x_offset as f32 * s,
                y_offset: p.y_offset as f32 * s,
                x_advance: p.x_advance as f32 * s,
                y_advance: p.y_advance as f32 * s,
            })
            .collect()
    }

    /// Rasterize a glyph id into a coverage bitmap via `ab_glyph_rasterizer`
    /// (the same coverage engine `ab_glyph` uses). `outline_dilate_px` expands
    /// the mask (袋文字 halo) by an 8-neighbour max dilation.
    /// The bare glyph coverage (no dilation) at `size_px`, memoized in the per-bake
    /// glyph cache by (font, gid, size). The outline scan-conversion is the
    /// expensive part and only depends on these, so caching it lets every effect
    /// pass (which differ only in dilation) and every repeat of the glyph reuse it.
    fn rasterize_base(&self, gid: u16, size_px: f32) -> Option<Arc<GlyphBitmap>> {
        let key = (self.id, gid, size_px.to_bits());
        if let Some(b) = GLYPH_CACHE.with(|c| c.borrow().base.get(&key).cloned()) {
            return Some(b);
        }
        let face = self.face();
        let mut col = OutlineCollector::default();
        let r = face.outline_glyph(GlyphId(gid), &mut col)?;
        let s = size_px / self.upem;
        let x_min = r.x_min as f32 * s;
        let y_max = r.y_max as f32 * s;
        let base_w = ((r.x_max - r.x_min) as f32 * s).ceil() as i32;
        let base_h = ((r.y_max - r.y_min) as f32 * s).ceil() as i32;
        if base_w <= 0 || base_h <= 0 {
            return None;
        }
        let mut ras = Rasterizer::new(base_w as usize, base_h as usize);
        // font (fx,fy)[y-up] -> pixel (x-right, y-down) within the bitmap.
        let tf = |x: f32, y: f32| -> Point { point(x * s - x_min, y_max - y * s) };
        col.replay(&mut ras, &tf);
        let mut cov = vec![0.0f32; (base_w * base_h) as usize];
        ras.for_each_pixel(|idx, a| {
            if idx < cov.len() {
                cov[idx] = a;
            }
        });
        let base = Arc::new(GlyphBitmap {
            width: base_w as usize,
            height: base_h as usize,
            coverage: cov,
            left: x_min,
            top: -y_max,
        });
        GLYPH_CACHE.with(|c| c.borrow_mut().base.insert(key, Arc::clone(&base)));
        Some(base)
    }

    /// Coverage mask for `gid` at `size_px`, optionally dilated by
    /// `outline_dilate_px` (袋文字 halo / glow / shadow base). Memoized per
    /// (font, gid, size, dilate) in the per-bake cache so repeated glyphs and the
    /// many effect passes that share a dilation reuse the result.
    pub fn rasterize_gid(
        &self,
        gid: u16,
        size_px: f32,
        outline_dilate_px: f32,
    ) -> Option<Arc<GlyphBitmap>> {
        let dilate = outline_dilate_px.max(0.0);
        let dkey = (self.id, gid, size_px.to_bits(), dilate.to_bits());
        if let Some(b) = GLYPH_CACHE.with(|c| c.borrow().dilated.get(&dkey).cloned()) {
            return Some(b);
        }
        let base = self.rasterize_base(gid, size_px)?;
        let result = if dilate <= 0.0 {
            Arc::clone(&base)
        } else {
            Arc::new(dilate_coverage(&base, dilate))
        };
        GLYPH_CACHE.with(|c| c.borrow_mut().dilated.insert(dkey, Arc::clone(&result)));
        Some(result)
    }

    /// Convenience: rasterize by char (cmap glyph). Shaped/substituted glyphs use
    /// `rasterize_gid`.
    pub fn rasterize(
        &self,
        ch: char,
        size_px: f32,
        outline_dilate_px: f32,
    ) -> Option<Arc<GlyphBitmap>> {
        self.rasterize_gid(self.glyph_id(ch), size_px, outline_dilate_px)
    }
}

/// Circular dilation of a coverage bitmap by `dilate_px` (袋文字 halo / glow /
/// shadow base), via a two-pass chamfer distance transform.
///
/// The shape (any non-zero source coverage) is expanded outward so it is **fully
/// opaque out to radius `dilate_px`**, then fades over a 1px anti-aliased ramp
/// (`dilate_px`..`dilate_px + 1`). This keeps the old solid outline weight (a
/// hard disk of radius `dilate`) while adding a smooth edge. The padding is
/// `ceil(dilate) + 1` so the AA ramp is never clipped. Cost is **O(output area),
/// independent of radius**.
///
/// The previous implementation was a brute-force disk max-filter — for every
/// covered source pixel it wrote into a (2r+1)² neighbourhood, i.e. O(area·r²).
/// Combined with `draw_layout_soft_mask` re-dilating every glyph for up to ~8
/// blur passes (× shadow/glow/outlines), a full-resolution text-effect bake of a
/// few annotated glyphs took ~8s on the UI thread (perf log 2026-06-07,
/// `fs/comic_composite_build`). The chamfer DT makes each pass radius-independent.
fn dilate_coverage(base: &GlyphBitmap, dilate_px: f32) -> GlyphBitmap {
    let dilate = dilate_px.max(0.0);
    if dilate <= 0.0 {
        return base.clone();
    }
    // Solid out to `dilate` + 1px AA fade beyond → pad by ceil(dilate)+1.
    let pad = dilate.ceil() as i32 + 1;
    let base_w = base.width as i32;
    let base_h = base.height as i32;
    let out_w = base_w + 2 * pad;
    let out_h = base_h + 2 * pad;
    let n = (out_w * out_h) as usize;

    // Distance field: 0 on the (thresholded) glyph silhouette, FAR elsewhere.
    // Chamfer weights: D1 (orthogonal) = 1, D2 (diagonal) = √2 — exact on the
    // axes/diagonals, ~octagonal in between (imperceptible for a halo).
    const FAR: f32 = 1.0e9;
    const D1: f32 = 1.0;
    const D2: f32 = std::f32::consts::SQRT_2;
    let mut dist = vec![FAR; n];
    let idx = |x: i32, y: i32| (y * out_w + x) as usize;
    // Seed on ANY coverage (matches the old max-filter, which dilated every
    // non-zero source pixel) so the halo fully contains the glyph incl. its AA
    // fringe — keeps thin strokes from dropping out.
    for sy in 0..base_h {
        for sx in 0..base_w {
            if base.coverage[(sy * base_w + sx) as usize] > 0.0 {
                dist[idx(sx + pad, sy + pad)] = 0.0;
            }
        }
    }
    // Forward pass (top-left → bottom-right).
    for y in 0..out_h {
        for x in 0..out_w {
            let mut d = dist[idx(x, y)];
            if x > 0 {
                d = d.min(dist[idx(x - 1, y)] + D1);
            }
            if y > 0 {
                d = d.min(dist[idx(x, y - 1)] + D1);
                if x > 0 {
                    d = d.min(dist[idx(x - 1, y - 1)] + D2);
                }
                if x < out_w - 1 {
                    d = d.min(dist[idx(x + 1, y - 1)] + D2);
                }
            }
            dist[idx(x, y)] = d;
        }
    }
    // Backward pass (bottom-right → top-left).
    for y in (0..out_h).rev() {
        for x in (0..out_w).rev() {
            let mut d = dist[idx(x, y)];
            if x < out_w - 1 {
                d = d.min(dist[idx(x + 1, y)] + D1);
            }
            if y < out_h - 1 {
                d = d.min(dist[idx(x, y + 1)] + D1);
                if x < out_w - 1 {
                    d = d.min(dist[idx(x + 1, y + 1)] + D2);
                }
                if x > 0 {
                    d = d.min(dist[idx(x - 1, y + 1)] + D2);
                }
            }
            dist[idx(x, y)] = d;
        }
    }
    // Coverage: fully opaque out to `dilate`, then a 1px AA fade to `dilate + 1`
    // (matches the old solid disk radius, with a smooth edge instead of a hard cut).
    let mut out = vec![0.0f32; n];
    for (o, &d) in out.iter_mut().zip(dist.iter()) {
        *o = (dilate + 1.0 - d).clamp(0.0, 1.0);
    }
    GlyphBitmap {
        width: out_w as usize,
        height: out_h as usize,
        coverage: out,
        left: base.left - pad as f32,
        top: base.top - pad as f32,
    }
}

/// `OutlineBuilder` that just lets `outline_glyph` return the bbox (no segments).
#[derive(Default)]
struct NoopOutline;
impl OutlineBuilder for NoopOutline {
    fn move_to(&mut self, _x: f32, _y: f32) {}
    fn line_to(&mut self, _x: f32, _y: f32) {}
    fn quad_to(&mut self, _x1: f32, _y1: f32, _x: f32, _y: f32) {}
    fn curve_to(&mut self, _x1: f32, _y1: f32, _x2: f32, _y2: f32, _x: f32, _y: f32) {}
    fn close(&mut self) {}
}

/// Collects glyph outline segments (font units, y-up) for replay into a
/// `Rasterizer` after applying the px transform.
#[derive(Default)]
struct OutlineCollector {
    segs: Vec<Seg>,
}

enum Seg {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Curve(f32, f32, f32, f32, f32, f32),
    Close,
}

impl OutlineBuilder for OutlineCollector {
    fn move_to(&mut self, x: f32, y: f32) {
        self.segs.push(Seg::Move(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.segs.push(Seg::Line(x, y));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.segs.push(Seg::Quad(x1, y1, x, y));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.segs.push(Seg::Curve(x1, y1, x2, y2, x, y));
    }
    fn close(&mut self) {
        self.segs.push(Seg::Close);
    }
}

impl OutlineCollector {
    fn replay(&self, ras: &mut Rasterizer, tf: &dyn Fn(f32, f32) -> Point) {
        let (mut cx, mut cy) = (0.0f32, 0.0f32);
        let (mut sx, mut sy) = (0.0f32, 0.0f32);
        for seg in &self.segs {
            match *seg {
                Seg::Move(x, y) => {
                    cx = x;
                    cy = y;
                    sx = x;
                    sy = y;
                }
                Seg::Line(x, y) => {
                    ras.draw_line(tf(cx, cy), tf(x, y));
                    cx = x;
                    cy = y;
                }
                Seg::Quad(x1, y1, x, y) => {
                    ras.draw_quad(tf(cx, cy), tf(x1, y1), tf(x, y));
                    cx = x;
                    cy = y;
                }
                Seg::Curve(x1, y1, x2, y2, x, y) => {
                    ras.draw_cubic(tf(cx, cy), tf(x1, y1), tf(x2, y2), tf(x, y));
                    cx = x;
                    cy = y;
                }
                Seg::Close => {
                    ras.draw_line(tf(cx, cy), tf(sx, sy));
                    cx = sx;
                    cy = sy;
                }
            }
        }
    }
}

/// Rotate a glyph coverage bitmap 90° clockwise.
///
/// New width = old height, new height = old width. A char rotated 90° CW reads
/// top-to-bottom in a vertical column (横倒し): the old top edge becomes the new
/// right edge. The mapping `new[x*newW + y] = old[(oldH-1-y)*oldW + x]` places
/// old row `r` (top-most when `r` small) at new column `newW-1-r` (right-most),
/// so reading the rotated glyph downward follows the original baseline left→right.
///
/// `left` / `top` are reset to 0; the rasterizer centers sideways glyphs by
/// their (rw, rh) dimensions, so the original pen offsets are not reused.
pub fn rotate_cw(bmp: &GlyphBitmap) -> GlyphBitmap {
    let old_w = bmp.width;
    let old_h = bmp.height;
    let new_w = old_h;
    let new_h = old_w;
    let mut out = vec![0.0f32; new_w * new_h];
    if old_w == 0 || old_h == 0 {
        return GlyphBitmap {
            width: new_w,
            height: new_h,
            coverage: out,
            left: 0.0,
            top: 0.0,
        };
    }
    // new[x*new_w + y] = old[(old_h-1-y)*old_w + x]
    //   x in 0..new_h (== old_w), y in 0..new_w (== old_h)
    for x in 0..new_h {
        for y in 0..new_w {
            let src = (old_h - 1 - y) * old_w + x;
            out[x * new_w + y] = bmp.coverage[src];
        }
    }
    GlyphBitmap {
        width: new_w,
        height: new_h,
        coverage: out,
        left: 0.0,
        top: 0.0,
    }
}

/// A rasterized glyph coverage mask.
#[derive(Debug, Clone)]
pub struct GlyphBitmap {
    pub width: usize,
    pub height: usize,
    /// Row-major coverage in [0,1].
    pub coverage: Vec<f32>,
    /// Offset from the pen origin (x at pen, y at baseline) to the bitmap's
    /// top-left, in pixels (px_bounds().min).
    pub left: f32,
    pub top: f32,
}

/// A named collection of fonts. The lab registers one (the Windows JP font)
/// under both its real key and an empty key, so `font_key: ""` resolves.
#[derive(Default)]
pub struct FontSet {
    fonts: HashMap<String, LoadedFont>,
    default_key: Option<String>,
}

impl FontSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, font: LoadedFont) {
        let key = font.key.clone();
        if self.default_key.is_none() {
            self.default_key = Some(key.clone());
        }
        self.fonts.insert(key, font);
    }

    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }

    /// Resolve a font by key, falling back to the default (first-registered).
    pub fn get(&self, key: &str) -> Option<&LoadedFont> {
        if let Some(f) = self.fonts.get(key) {
            return Some(f);
        }
        self.default_key.as_ref().and_then(|k| self.fonts.get(k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize) -> GlyphBitmap {
        GlyphBitmap {
            width: w,
            height: h,
            coverage: vec![1.0; w * h],
            left: 0.0,
            top: 0.0,
        }
    }

    #[test]
    fn dilate_zero_returns_base_unchanged() {
        let b = solid(4, 4);
        let out = dilate_coverage(&b, 0.0);
        assert_eq!((out.width, out.height), (4, 4));
        assert_eq!(out.coverage, b.coverage);
    }

    #[test]
    fn dilate_expands_dims_and_offsets() {
        let out = dilate_coverage(&solid(4, 4), 5.0);
        // base + 2*(ceil(dilate)+1) on each axis (the +1 holds the AA fade);
        // origin shifts by -pad.
        assert_eq!((out.width, out.height), (16, 16));
        assert_eq!((out.left, out.top), (-6.0, -6.0));
    }

    #[test]
    fn dilate_grows_a_single_pixel_into_a_disk() {
        // One covered pixel, dilated by r: the chamfer DT must be fully opaque out
        // to r and clear beyond r+1 (a roughly circular halo). The old brute force
        // was O(area·r²); this asserts the new DT keeps the shape + solid radius.
        let r = 6.0f32;
        let base = GlyphBitmap {
            width: 1,
            height: 1,
            coverage: vec![1.0],
            left: 0.0,
            top: 0.0,
        };
        let out = dilate_coverage(&base, r);
        // Grid is base + 2*(ceil(r)+1) = 15x15; the single seed sits at the centre.
        assert_eq!((out.width, out.height), (15, 15));
        let cx = (out.width / 2) as i32; // 7
        let cy = (out.height / 2) as i32; // 7
        let cov = |x: i32, y: i32| out.coverage[(y as usize) * out.width + x as usize];
        assert!(cov(cx, cy) > 0.99, "centre solid");
        assert!(cov(cx + 3, cy) > 0.99, "inside the radius solid");
        // Solid all the way out to the requested radius (dist == r).
        assert!(cov(cx + 6, cy) > 0.99, "solid out to radius r");
        // Just past the radius the AA has fully faded.
        assert!(cov(cx + 7, cy) < 0.01, "beyond radius+1 is clear");
        // Roughly circular: the corner (distance r·√2 ≈ 9.9 > r) is clear.
        assert!(
            cov(0, 0) < 0.01,
            "corner beyond radius is clear, got {}",
            cov(0, 0)
        );
    }

    #[test]
    fn dilate_thin_outline_keeps_solid_weight() {
        // A 2px outline must stay fully opaque out to 2px (the old hard disk),
        // not soften to ~0.5α at the boundary (Codex P3 follow-up). Single seed.
        let base = GlyphBitmap {
            width: 1,
            height: 1,
            coverage: vec![1.0],
            left: 0.0,
            top: 0.0,
        };
        let out = dilate_coverage(&base, 2.0);
        let c = (out.width / 2) as i32;
        let cov = |x: i32, y: i32| out.coverage[(y as usize) * out.width + x as usize];
        assert!(cov(c + 2, c) > 0.99, "outline solid out to its width");
    }
}
