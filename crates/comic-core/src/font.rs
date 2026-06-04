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

use std::collections::HashMap;

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

/// A single loaded font face: the bytes + the parsed face (cached) + design metrics.
pub struct LoadedFont {
    pub key: String,
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
    pub fn rasterize_gid(
        &self,
        gid: u16,
        size_px: f32,
        outline_dilate_px: f32,
    ) -> Option<GlyphBitmap> {
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
        let base = GlyphBitmap {
            width: base_w as usize,
            height: base_h as usize,
            coverage: cov,
            left: x_min,
            top: -y_max,
        };
        Some(dilate_coverage(base, outline_dilate_px))
    }

    /// Convenience: rasterize by char (cmap glyph). Shaped/substituted glyphs use
    /// `rasterize_gid`.
    pub fn rasterize(&self, ch: char, size_px: f32, outline_dilate_px: f32) -> Option<GlyphBitmap> {
        self.rasterize_gid(self.glyph_id(ch), size_px, outline_dilate_px)
    }
}

/// 8-neighbour max dilation of a coverage bitmap by `dilate_px` (袋文字 halo).
fn dilate_coverage(base: GlyphBitmap, dilate_px: f32) -> GlyphBitmap {
    let dilate = dilate_px.max(0.0).ceil() as i32;
    if dilate == 0 {
        return base;
    }
    let base_w = base.width as i32;
    let base_h = base.height as i32;
    let out_w = base_w + 2 * dilate;
    let out_h = base_h + 2 * dilate;
    let mut out = vec![0.0f32; (out_w * out_h) as usize];
    let r2 = (dilate as f32) * (dilate as f32);
    for sy in 0..base_h {
        for sx in 0..base_w {
            let v = base.coverage[(sy * base_w + sx) as usize];
            if v <= 0.0 {
                continue;
            }
            for dy in -dilate..=dilate {
                for dx in -dilate..=dilate {
                    if (dx * dx + dy * dy) as f32 > r2 {
                        continue;
                    }
                    let tx = sx + dilate + dx;
                    let ty = sy + dilate + dy;
                    if tx < 0 || ty < 0 || tx >= out_w || ty >= out_h {
                        continue;
                    }
                    let oi = (ty * out_w + tx) as usize;
                    if v > out[oi] {
                        out[oi] = v;
                    }
                }
            }
        }
    }
    GlyphBitmap {
        width: out_w as usize,
        height: out_h as usize,
        coverage: out,
        left: base.left - dilate as f32,
        top: base.top - dilate as f32,
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
