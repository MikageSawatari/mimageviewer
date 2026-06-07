//! CPU rasterizer: bakes a list of annotation objects into an RGBA8 overlay
//! (straight alpha) at source resolution.
//!
//! For each z-sorted object:
//!   1. (bubbles) fill the tessellated shape (scanline) + the tail triangle,
//!   2. (bubbles) stroke the outline,
//!   3. rasterize text glyphs via the font rasterizer, drawing 袋文字 (outline)
//!      first as a dilated halo in the outline color, then the fill on top.
//!
//! This is the WYSIWYG "truth" the lab compares its live egui preview against.

use std::collections::HashMap;

use crate::font::{FontSet, GlyphBitmap, LoadedFont, rotate_cw};
use crate::layout::{GlyphForm, TextLayout, layout_text, layout_text_wrapped};
use crate::model::{
    AnnotationKind, AnnotationObject, BubbleObject, BubbleShape, DecoKind, DecorationLayer,
    FillMode, FrameStyle, IndicatorKind, MessageWindowObject, NamePlateMode, Orientation,
    PortraitSide, Rgba, SizeMode, StampObject, StrokeStyle, TextAlign, TextBlock, VAnchor,
};
use crate::tessellate::{
    PlacedDeco, bubble_geometry, fit_bubble_shape, place_decorations, resolve_tail_base,
    tessellate_bubble,
};

/// The bubble shape actually drawn: when `auto_size` is on (and the bubble has
/// non-empty text whose font is loaded), the dimensions are fitted to the text
/// layout via `fit_bubble_shape`; otherwise the stored `bubble.shape` is used.
///
/// Shared by the rasterizer and the lab (hit-test / handles) so the picked
/// region always matches the baked pixels.
pub fn effective_bubble_shape(bubble: &BubbleObject, fonts: &FontSet) -> BubbleShape {
    if bubble.auto_size && !bubble.text.text.is_empty() {
        if let Some(font) = fonts.get(&bubble.text.font_key) {
            let (tw, th) = layout_text(&bubble.text, font).bounds;
            return fit_bubble_shape(&bubble.shape, tw, th, bubble.padding_px);
        }
    }
    bubble.shape
}

/// An RGBA8 (straight alpha) overlay buffer.
#[derive(Debug, Clone)]
pub struct RgbaOverlay {
    pub w: usize,
    pub h: usize,
    /// Row-major RGBA8, length == w*h*4.
    pub pixels: Vec<u8>,
}

impl RgbaOverlay {
    pub fn new(w: usize, h: usize) -> Self {
        RgbaOverlay {
            w,
            h,
            pixels: vec![0u8; w * h * 4],
        }
    }

    #[inline]
    fn blend_px(&mut self, x: i32, y: i32, color: Rgba, coverage: f32) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let a = (color.a as f32 / 255.0) * coverage.clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let idx = (y as usize * self.w + x as usize) * 4;
        let (dr, dg, db, da) = (
            self.pixels[idx] as f32 / 255.0,
            self.pixels[idx + 1] as f32 / 255.0,
            self.pixels[idx + 2] as f32 / 255.0,
            self.pixels[idx + 3] as f32 / 255.0,
        );
        let sr = color.r as f32 / 255.0;
        let sg = color.g as f32 / 255.0;
        let sb = color.b as f32 / 255.0;
        // Straight-alpha "source over".
        let out_a = a + da * (1.0 - a);
        let (or, og, ob) = if out_a <= 0.0 {
            (0.0, 0.0, 0.0)
        } else {
            (
                (sr * a + dr * da * (1.0 - a)) / out_a,
                (sg * a + dg * da * (1.0 - a)) / out_a,
                (sb * a + db * da * (1.0 - a)) / out_a,
            )
        };
        self.pixels[idx] = (or * 255.0).round().clamp(0.0, 255.0) as u8;
        self.pixels[idx + 1] = (og * 255.0).round().clamp(0.0, 255.0) as u8;
        self.pixels[idx + 2] = (ob * 255.0).round().clamp(0.0, 255.0) as u8;
        self.pixels[idx + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
    }
}

/// Pre-rasterized stamp images keyed by annotation-object id. The lab/app
/// resolves each `StampObject::source` into straight-alpha RGBA at (roughly)
/// display size and supplies this map; comic-core stays decode-free.
///
/// `Arc` so the caller can rebuild this map per bake without deep-copying decoded
/// pixels (it shares its decode cache by `Arc::clone`).
pub type StampImages = HashMap<u64, std::sync::Arc<RgbaOverlay>>;

/// Bake all enabled objects (z-sorted ascending) into a fresh overlay. Stamps
/// render as missing-image placeholders (use [`bake_overlay_with_stamps`] to
/// supply real stamp pixels).
pub fn bake_overlay(
    objects: &[AnnotationObject],
    w: usize,
    h: usize,
    fonts: &FontSet,
) -> RgbaOverlay {
    bake_overlay_with_stamps(objects, w, h, fonts, &StampImages::new())
}

/// Bake all enabled objects, compositing image stamps from `stamps` (keyed by
/// object id). A stamp with no entry draws a placeholder so the user still sees
/// it (e.g. a moved/missing user image).
pub fn bake_overlay_with_stamps(
    objects: &[AnnotationObject],
    w: usize,
    h: usize,
    fonts: &FontSet,
    stamps: &StampImages,
) -> RgbaOverlay {
    let mut overlay = RgbaOverlay::new(w, h);
    if w == 0 || h == 0 {
        return overlay;
    }
    // Start a fresh per-bake glyph cache: repeated glyphs / effect passes / text
    // blocks on this page memoize their coverage, but the cache never accumulates
    // across pages (memory stays bounded to ~one page's glyphs).
    crate::font::reset_glyph_cache();
    let mut order: Vec<usize> = (0..objects.len()).filter(|&i| objects[i].enabled).collect();
    order.sort_by_key(|&i| objects[i].z);

    // Group consecutive (in z) bubbles into merge chains: an upper bubble with
    // `merge_with_below` fuses with the bubble directly beneath it. A chain
    // renders as one union outline (rotated members composite at their rotated
    // position via bake_into); everything else bakes individually.
    // Only shapes with a solid fillable polygon body can take part in a merge
    // chain (the fill→stroke→erase union trick). Fuzzy / line-field / text-only
    // shapes always bake standalone, so they neither start nor join a chain.
    let mergeable = |i: usize| match &objects[i].kind {
        AnnotationKind::Bubble(b) => crate::tessellate::shape_is_mergeable(&b.shape),
        _ => false,
    };
    let mut gi = 0;
    while gi < order.len() {
        let mut group = vec![order[gi]];
        while gi + 1 < order.len() {
            let nxt = order[gi + 1];
            let nxt_merges_down = match &objects[nxt].kind {
                AnnotationKind::Bubble(b) => b.merge_with_below,
                _ => false,
            };
            if mergeable(order[gi]) && nxt_merges_down && mergeable(nxt) {
                group.push(nxt);
                gi += 1;
            } else {
                break;
            }
        }
        gi += 1;
        if group.len() == 1 {
            bake_object(&mut overlay, &objects[group[0]], fonts, stamps);
        } else {
            bake_merge_group(&mut overlay, &group, objects, fonts);
        }
    }
    overlay
}

/// Bake one object, honoring `obj.rotation_rad`, with `draw` selecting WHAT to
/// render (full object, or just a part for the merge passes). For a non-trivial
/// rotation the part is drawn unrotated into a tight temp buffer and
/// rotate-blitted into `overlay` around the object's edit pivot (bilinear,
/// premultiplied). Standalone text stores its pivot as the layout top-left, but
/// rotates around the layout center so SFX/text handles behave like stamps.
fn bake_into(
    overlay: &mut RgbaOverlay,
    obj: &AnnotationObject,
    fonts: &FontSet,
    mut draw: impl FnMut(&mut RgbaOverlay, &AnnotationObject),
) {
    if obj.rotation_rad.abs() < 1e-4 {
        draw(overlay, obj);
        return;
    }
    let Some((minx, miny, maxx, maxy)) = object_local_aabb(obj, fonts) else {
        return;
    };
    let rotation_pivot = object_rotation_pivot(obj, fonts);
    let pad = 2.0;
    let (minx, miny) = (minx - pad, miny - pad);
    let (maxx, maxy) = (maxx + pad, maxy + pad);
    let tw = (maxx - minx).ceil() as usize;
    let th = (maxy - miny).ceil() as usize;
    if tw == 0 || th == 0 || tw > 8192 || th > 8192 {
        // Degenerate / pathological size: fall back to an unrotated draw.
        draw(overlay, obj);
        return;
    }
    let mut tmp = RgbaOverlay::new(tw, th);
    // Shift a copy of the object so its content lands inside `tmp`'s local space.
    let mut shifted = obj.clone();
    shifted.rotation_rad = 0.0;
    shifted.pivot = (obj.pivot.0 - minx, obj.pivot.1 - miny);
    if let AnnotationKind::Bubble(b) = &mut shifted.kind {
        if let Some(t) = &mut b.tail {
            t.tip = (t.tip.0 - minx, t.tip.1 - miny);
        }
    }
    draw(&mut tmp, &shifted);
    rotate_blit(
        overlay,
        &tmp,
        (minx, miny),
        rotation_pivot,
        obj.rotation_rad,
    );
}

/// Bake a single object fully (all parts), honoring rotation.
fn bake_object(
    overlay: &mut RgbaOverlay,
    obj: &AnnotationObject,
    fonts: &FontSet,
    stamps: &StampImages,
) {
    bake_into(overlay, obj, fonts, |ov, o| {
        bake_object_unrotated(ov, o, fonts, stamps)
    });
}

fn bake_object_unrotated(
    overlay: &mut RgbaOverlay,
    obj: &AnnotationObject,
    fonts: &FontSet,
    stamps: &StampImages,
) {
    match &obj.kind {
        AnnotationKind::Bubble(bubble) => {
            draw_bubble_parts(
                overlay, obj.pivot, bubble, fonts, true, true, true, false, 1.0,
            );
        }
        AnnotationKind::Text(text) => {
            bake_text(overlay, text, obj.pivot, fonts, false);
        }
        AnnotationKind::MessageWindow(win) => {
            draw_message_window_parts(overlay, obj.pivot, win, fonts);
        }
        AnnotationKind::Stamp(stamp) => match stamps.get(&obj.id) {
            Some(img) => draw_stamp(overlay, obj.pivot, stamp, img.as_ref()),
            None => draw_stamp_placeholder(overlay, obj.pivot, stamp),
        },
    }
}

/// Multiply applied to a member's outline width on the merge stroke pass. The
/// stroke is centered on the outline, and the merge's interior-erase fill (pass 3)
/// wipes the half that lies inside the body — leaving only the outer half. Drawing
/// it at 2× makes that surviving outer half equal the configured width, so a
/// merged balloon's outline isn't ~half as thick as an unmerged bubble's.
const MERGE_STROKE_SCALE: f32 = 2.0;

/// Max outward jitter (px) a 線 (sketch) outline pass can add to a vertex beyond
/// the stroke itself (see `draw_sketch_outline`'s `amp` clamp). `object_local_aabb`
/// adds this to the temp-buffer slack for `Strokes` so a rotated/merged sketch
/// outline isn't clipped before rotate_blit. Keep the two in sync.
const SKETCH_MAX_JITTER: f32 = 6.0;

/// A copy of `s` with its width multiplied by `scale`.
fn scaled_stroke(s: &StrokeStyle, scale: f32) -> StrokeStyle {
    StrokeStyle {
        color: s.color,
        width_px: s.width_px * scale,
    }
}

/// Draw selected parts of a bubble (unrotated, in `overlay` coords). The merge
/// passes use this to draw fill-only / stroke-only / deco+text-only so the union
/// "fill → stroke → fill" trick can erase interior strokes. `opaque_fill` forces
/// the fill alpha to 255 (used by the merge fill passes). `stroke_scale` widens
/// the outline on the stroke pass (the merge passes use `MERGE_STROKE_SCALE` to
/// offset the interior-erase thinning; the single-object path uses 1.0).
#[allow(clippy::too_many_arguments)]
fn draw_bubble_parts(
    overlay: &mut RgbaOverlay,
    pivot: (f32, f32),
    bubble: &BubbleObject,
    fonts: &FontSet,
    do_fill: bool,
    do_stroke: bool,
    do_decotext: bool,
    opaque_fill: bool,
    stroke_scale: f32,
) {
    // Unified body+tail contour: the tail spike is part of the same closed
    // outline as the body, so the fill and the stroke are seamless.
    let shape = effective_bubble_shape(bubble, fonts);

    // Line-field effects (集中線 / 流線) render as many strokes radiating /
    // streaking around a clear central ellipse — not as a filled polygon. Draw
    // the lines once (on the stroke pass), then the centered text on top.
    if matches!(
        shape,
        BubbleShape::MotionLines { .. } | BubbleShape::SpeedLines { .. }
    ) {
        if do_stroke {
            draw_line_field(overlay, pivot, bubble, &shape);
        }
        if do_decotext {
            bake_text(overlay, &bubble.text, pivot, fonts, true);
        }
        return;
    }

    // なし (text-only): no fill / stroke / tail — only the centered text.
    if matches!(shape, BubbleShape::TextOnly { .. }) {
        if do_decotext {
            bake_text(overlay, &bubble.text, pivot, fonts, true);
        }
        return;
    }

    // 意識 (concentration): a fuzzy feathered ellipse. Rendered once, on the
    // decotext pass — a merge group runs do_fill TWICE (fill + opaque-erase), so
    // gating on do_fill would double-composite the feather and darken it; the
    // decotext pass runs exactly once in both the single and merge paths.
    if let BubbleShape::Concentration { rx, ry, shape_seed } = shape {
        if do_decotext {
            draw_concentration(overlay, pivot, bubble, rx, ry, shape_seed, opaque_fill);
            let geo = bubble_geometry(&shape, pivot, bubble.tail.as_ref());
            bubble_decorations(overlay, bubble, pivot, &shape, &geo, fonts);
            bake_text(overlay, &bubble.text, pivot, fonts, true);
        }
        return;
    }

    let geo = bubble_geometry(&shape, pivot, bubble.tail.as_ref());
    if do_fill {
        bubble_fill(overlay, bubble, &geo, opaque_fill);
    }
    if do_stroke {
        match shape {
            // 線: rough multi-pass hand-drawn outline (instead of one clean line).
            BubbleShape::Strokes { shape_seed, .. } => {
                let stroke = scaled_stroke(&bubble.outline, stroke_scale);
                draw_sketch_outline(overlay, pivot, &geo, &stroke, shape_seed);
            }
            // 二重線: the clean OUTER line (incl. spliced tail) here; the inner ring
            // is drawn on the decotext pass so a merge group's interior-erase fill
            // (pass 3) can't wipe it.
            _ => bubble_stroke(overlay, bubble, &geo, stroke_scale),
        }
    }
    if do_decotext {
        // 二重線: inner concentric ring, after any merge erase-fill pass.
        if let BubbleShape::DoubleStroke {
            half_w,
            half_h,
            corner_px,
            gap_px,
        } = shape
        {
            if bubble.outline.width_px > 0.0 {
                let g = gap_px.max(1.0);
                let inner = BubbleShape::RoundRect {
                    half_w: (half_w - g).max(1.0),
                    half_h: (half_h - g).max(1.0),
                    corner_px: (corner_px - g).max(0.0),
                };
                stroke_polygon(overlay, &tessellate_bubble(&inner, pivot), &bubble.outline);
            }
        }
        bubble_decorations(overlay, bubble, pivot, &shape, &geo, fonts);
        // Embedded text, centered in the bubble body.
        bake_text(overlay, &bubble.text, pivot, fonts, true);
    }
}

/// Draw a 意識 (concentration) fuzzy ellipse: a feathered fill (opaque inside
/// `CONCENTRATION_SOLID_RATIO`, fading to transparent by a wobbling rim) plus a
/// soft outline ring just inside the rim. The rim wobbles with `shape_seed` so
/// the blob reads as hand-drawn / もやもや. Rendered per-pixel over the ellipse
/// AABB. `opaque_fill` forces the fill to full opacity (merge passes).
fn draw_concentration(
    overlay: &mut RgbaOverlay,
    pivot: (f32, f32),
    bubble: &BubbleObject,
    rx: f32,
    ry: f32,
    shape_seed: u32,
    opaque_fill: bool,
) {
    let (cx, cy) = pivot;
    let rx = rx.max(1.0);
    let ry = ry.max(1.0);
    let inner = crate::tessellate::CONCENTRATION_SOLID_RATIO; // ~0.78
    let ph1 = lf_hash(shape_seed, 1) * std::f32::consts::TAU;
    let ph2 = lf_hash(shape_seed, 2) * std::f32::consts::TAU;
    let fill = bubble.fill;
    let fill_a_base = if opaque_fill {
        1.0
    } else {
        bubble.fill_opacity.clamp(0.0, 1.0)
    };
    let ring = bubble.outline;
    let ring_on = ring.width_px > 0.0 && ring.color.a > 0;
    // Clamp the scan box to the overlay so a huge / off-canvas ellipse (corrupt
    // sidecar or extreme auto-size) doesn't iterate millions of off-screen pixels.
    let x0 = ((cx - rx).floor() as i32).max(0);
    let x1 = ((cx + rx).ceil() as i32).min(overlay.w as i32 - 1);
    let y0 = ((cy - ry).floor() as i32).max(0);
    let y1 = ((cy + ry).ceil() as i32).min(overlay.h as i32 - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let nx = (x as f32 + 0.5 - cx) / rx;
            let ny = (y as f32 + 0.5 - cy) / ry;
            let d = (nx * nx + ny * ny).sqrt();
            if d > 1.0 {
                continue;
            }
            let ang = ny.atan2(nx);
            // Rim wobble: the fade endpoint, kept strictly inside the ellipse.
            let wob = 0.05 * (3.0 * ang + ph1).sin() + 0.03 * (7.0 * ang + ph2).sin();
            let rim = (1.0 + wob).clamp(0.6, 0.99);
            // Feathered fill: opaque to `inner`, fading to 0 at `rim`. `blend_px`
            // multiplies in the color's own alpha, so pass only the coverage here
            // (do NOT pre-multiply by fc.a — that would double-apply the alpha).
            // For a merge erase pass, force the color opaque so it actually erases.
            if let Some(fc) = fill {
                let cov = if d <= inner {
                    1.0
                } else {
                    ((rim - d) / (rim - inner).max(1e-3)).clamp(0.0, 1.0)
                };
                if cov > 0.0 {
                    let col = if opaque_fill {
                        Rgba { a: 255, ..fc }
                    } else {
                        fc
                    };
                    overlay.blend_px(x, y, col, fill_a_base * cov);
                }
            }
            // Soft ring just inside the rim (a fuzzy "edge"). `blend_px` applies
            // ring.color.a, so pass only the band coverage.
            if ring_on {
                let ring_c = rim * 0.93;
                let half_band = 0.10;
                let band = (1.0 - (d - ring_c).abs() / half_band).clamp(0.0, 1.0);
                if band > 0.0 {
                    overlay.blend_px(x, y, ring.color, band);
                }
            }
        }
    }
}

/// Draw a 線 (sketchy) outline: stroke the contour a few times, each pass
/// perturbing every vertex along its outward radial by a smooth seed-driven wave
/// (different phase per pass). Smooth = no self-crossing; the overlapping passes
/// read as one rough, hand-drawn line.
fn draw_sketch_outline(
    overlay: &mut RgbaOverlay,
    pivot: (f32, f32),
    geo: &crate::tessellate::BubbleGeometry,
    stroke: &StrokeStyle,
    shape_seed: u32,
) {
    if stroke.width_px <= 0.0 || stroke.color.a == 0 {
        return;
    }
    let n = geo.outline.len();
    if n < 3 {
        return;
    }
    let amp = stroke.width_px.clamp(1.5, SKETCH_MAX_JITTER);
    const PASSES: u32 = 2;
    for pass in 0..PASSES {
        let salt = shape_seed.wrapping_add(pass.wrapping_mul(131));
        let ph1 = lf_hash(salt, 1) * std::f32::consts::TAU;
        let ph2 = lf_hash(salt, 2) * std::f32::consts::TAU;
        let jittered: Vec<(f32, f32)> = geo
            .outline
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| {
                let t = i as f32 / n as f32 * std::f32::consts::TAU;
                let dx = x - pivot.0;
                let dy = y - pivot.1;
                let len = (dx * dx + dy * dy).sqrt().max(1e-3);
                let off = amp * (0.6 * (5.0 * t + ph1).sin() + 0.4 * (11.0 * t + ph2).sin());
                (x + dx / len * off, y + dy / len * off)
            })
            .collect();
        stroke_polygon(overlay, &jittered, stroke);
    }
}

/// Tiny deterministic hash → f32 in [0,1) for line-field jitter (no `rand`).
fn lf_hash(seed: u32, i: u32) -> f32 {
    let mut x = seed
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(i.wrapping_mul(0x85EB_CA6B))
        .wrapping_add(0x2754_5A57);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    (x as f32) / (u32::MAX as f32)
}

/// Draw a 集中線/流線 line field. Uses the bubble's outline color + width as the
/// line color + base thickness (defaulting to a visible width if unset).
fn draw_line_field(
    overlay: &mut RgbaOverlay,
    pivot: (f32, f32),
    bubble: &BubbleObject,
    shape: &BubbleShape,
) {
    let color = bubble.outline.color;
    let width = bubble.outline.width_px.max(1.5);
    let clear = crate::tessellate::LINE_FIELD_CLEAR_RATIO;
    match *shape {
        BubbleShape::MotionLines {
            rx,
            ry,
            count,
            shape_seed,
        } => {
            let count = count.clamp(8, 1000);
            let step = std::f32::consts::TAU / count as f32;
            for i in 0..count {
                // Even angular spacing + small jitter so it isn't mechanical.
                let a = i as f32 * step + (lf_hash(shape_seed, i) - 0.5) * step * 0.7;
                let (c, s) = (a.cos(), a.sin());
                let inner = (pivot.0 + rx * clear * c, pivot.1 + ry * clear * s);
                let outer = (pivot.0 + rx * c, pivot.1 + ry * s);
                // Tapered streak: full width at the outer rim, a point at the
                // clear-center edge.
                let dx = outer.0 - inner.0;
                let dy = outer.1 - inner.1;
                let len = (dx * dx + dy * dy).sqrt().max(1e-3);
                let (px, py) = (-dy / len, dx / len);
                let hw = width * (0.5 + lf_hash(shape_seed, i + 9013) * 0.6);
                let tri = [
                    (outer.0 + px * hw, outer.1 + py * hw),
                    (outer.0 - px * hw, outer.1 - py * hw),
                    inner,
                ];
                fill_polygon(overlay, &tri, color);
            }
        }
        BubbleShape::SpeedLines {
            half_w,
            half_h,
            dir_rad,
            count,
            shape_seed,
        } => {
            let count = count.clamp(8, 1000);
            let (dx, dy) = (dir_rad.cos(), dir_rad.sin()); // along-line dir
            let (px, py) = (-dy, dx); // perpendicular (offset axis)
            let (rx, ry) = (half_w.max(1.0), half_h.max(1.0));
            // Support half-extent along the perpendicular: the max offset where a
            // line still meets the outer ellipse. Lines are spread within ±this.
            let perp_max = ((rx * px).powi(2) + (ry * py).powi(2)).sqrt();
            let stroke = StrokeStyle {
                color,
                width_px: width,
            };
            // Intersect the line {pivot + p*off + t*d} with an axis ellipse of
            // half-extents (ex,ey); returns the two t roots (lo, hi) or None.
            let cross = |off: f32, ex: f32, ey: f32| -> Option<(f32, f32)> {
                let a = (dx / ex).powi(2) + (dy / ey).powi(2);
                let cterm = (px * off / ex).powi(2) + (py * off / ey).powi(2) - 1.0;
                let b = 2.0 * (dx * px * off / (ex * ex) + dy * py * off / (ey * ey));
                let disc = b * b - 4.0 * a * cterm;
                if disc <= 0.0 || a <= 0.0 {
                    return None;
                }
                let sq = disc.sqrt();
                Some(((-b - sq) / (2.0 * a), (-b + sq) / (2.0 * a)))
            };
            for i in 0..count {
                let f = if count == 1 {
                    0.0
                } else {
                    i as f32 / (count - 1) as f32 * 2.0 - 1.0
                };
                // Inset slightly so the extreme lines don't sit exactly on the rim.
                let off = f * perp_max * 0.985;
                let cx = pivot.0 + px * off;
                let cy = pivot.1 + py * off;
                let Some((t_lo, t_hi)) = cross(off, rx, ry) else {
                    continue; // line misses the outer ellipse
                };
                let at = |t: f32| (cx + dx * t, cy + dy * t);
                match cross(off, rx * clear, ry * clear) {
                    // Crosses the clear center: draw the two outer segments only.
                    Some((c_lo, c_hi)) => {
                        stroke_segment(overlay, at(t_lo), at(c_lo), &stroke);
                        stroke_segment(overlay, at(c_hi), at(t_hi), &stroke);
                    }
                    // Outside the clear center: one full chord.
                    None => stroke_segment(overlay, at(t_lo), at(t_hi), &stroke),
                }
            }
        }
        _ => {}
    }
}

/// Fill a bubble's body+tail outline and any thought circles (no stroke). When
/// `opaque`, the fill alpha is forced to 255 — used by the merge passes so a
/// translucent fill isn't darkened by the double fill, and interiors are erased
/// solidly regardless of the member's opacity.
fn bubble_fill(
    overlay: &mut RgbaOverlay,
    bubble: &BubbleObject,
    geo: &crate::tessellate::BubbleGeometry,
    opaque: bool,
) {
    if let Some(fill) = bubble.fill {
        let mut fc = fill;
        fc.a = if opaque {
            255
        } else {
            (fc.a as f32 * bubble.fill_opacity.clamp(0.0, 1.0)).round() as u8
        };
        // Even-odd scanline fill handles the concave tail spike.
        fill_polygon(overlay, &geo.outline, fc);
        for &(cx, cy, r) in &geo.thought {
            let poly = circle_poly(cx, cy, r, 24);
            fill_polygon(overlay, &poly, fc);
        }
    }
}

/// Stroke a bubble's body+tail outline and any thought circles. `scale` widens
/// the outline (merge passes use `MERGE_STROKE_SCALE`; single objects use 1.0).
fn bubble_stroke(
    overlay: &mut RgbaOverlay,
    bubble: &BubbleObject,
    geo: &crate::tessellate::BubbleGeometry,
    scale: f32,
) {
    if bubble.outline.width_px > 0.0 {
        let stroke = scaled_stroke(&bubble.outline, scale);
        stroke_polygon(overlay, &geo.outline, &stroke);
        for &(cx, cy, r) in &geo.thought {
            let poly = circle_poly(cx, cy, r, 24);
            stroke_polygon(overlay, &poly, &stroke);
        }
    }
}

/// Draw a bubble's procedural decorations (after body/tail, before text).
fn bubble_decorations(
    overlay: &mut RgbaOverlay,
    bubble: &BubbleObject,
    pivot: (f32, f32),
    shape: &BubbleShape,
    geo: &crate::tessellate::BubbleGeometry,
    _fonts: &FontSet,
) {
    if bubble.decorations.is_empty() {
        return;
    }
    let short_side = match *shape {
        BubbleShape::Ellipse { rx, ry } => rx.min(ry),
        BubbleShape::RoundRect { half_w, half_h, .. } => half_w.min(half_h),
        BubbleShape::Burst { rx, ry, .. } => rx.min(ry),
        BubbleShape::Cloud { rx, ry, .. } => rx.min(ry),
        BubbleShape::Polygon { rx, ry, .. } => rx.min(ry),
        BubbleShape::Diamond { half_w, half_h } => half_w.min(half_h),
        BubbleShape::Heart { rx, ry } => rx.min(ry),
        BubbleShape::Arrow { half_w, half_h, .. } => half_w.min(half_h),
        BubbleShape::Soft { half_w, half_h, .. } => half_w.min(half_h),
        BubbleShape::MotionLines { rx, ry, .. } => rx.min(ry),
        BubbleShape::SpeedLines { half_w, half_h, .. } => half_w.min(half_h),
        BubbleShape::TextOnly { half_w, half_h } => half_w.min(half_h),
        BubbleShape::Concentration { rx, ry, .. } => rx.min(ry),
        BubbleShape::Strokes { half_w, half_h, .. } => half_w.min(half_h),
        BubbleShape::DoubleStroke { half_w, half_h, .. } => half_w.min(half_h),
    };
    // Tail base point for `Tail` decoration placement. Spike tails use the
    // spliced base midpoint; thought tails have no spliced base (geo.tail is
    // None), so resolve it from the outline.
    let tail_base = match (&geo.tail, &bubble.tail) {
        (Some(t), _) => Some(((t[0].0 + t[1].0) * 0.5, (t[0].1 + t[1].1) * 0.5)),
        (None, Some(t)) => Some(resolve_tail_base(shape, pivot, t)),
        (None, None) => None,
    };
    for layer in &bubble.decorations {
        let placed = place_decorations(layer, &geo.body, pivot, short_side, tail_base);
        for deco in &placed {
            draw_decoration(overlay, deco, layer);
        }
    }
}

/// Bake a chain of overlapping bubbles as one merged shape: fill all → stroke
/// all → fill all again → decorations+text. The second fill pass paints over
/// interior strokes that fall inside another member's fill, leaving only the
/// union's outer outline (Codex design §4.8). Each part is drawn through
/// `bake_into`, so rotated members are composited at their rotated screen
/// position and the union still works. Assumes opaque, similarly-colored fills.
fn bake_merge_group(
    overlay: &mut RgbaOverlay,
    group: &[usize],
    objects: &[AnnotationObject],
    fonts: &FontSet,
) {
    let draw_part =
        |overlay: &mut RgbaOverlay, i: usize, do_fill: bool, do_stroke: bool, do_decotext: bool| {
            bake_into(overlay, &objects[i], fonts, |ov, o| {
                if let AnnotationKind::Bubble(b) = &o.kind {
                    // 2× stroke offsets the pass-3 interior-erase thinning so a merged
                    // outline keeps its configured width (only matters on the stroke pass).
                    draw_bubble_parts(
                        ov,
                        o.pivot,
                        b,
                        fonts,
                        do_fill,
                        do_stroke,
                        do_decotext,
                        true,
                        MERGE_STROKE_SCALE,
                    );
                }
            });
        };
    // Pass 1: fill all (opaque). Pass 2: stroke all. Pass 3: fill all again to
    // erase interior strokes. Pass 4: decorations + text on top.
    for &i in group {
        draw_part(overlay, i, true, false, false);
    }
    for &i in group {
        draw_part(overlay, i, false, true, false);
    }
    for &i in group {
        draw_part(overlay, i, true, false, false);
    }
    for &i in group {
        draw_part(overlay, i, false, false, true);
    }
}

/// Axis-aligned local (unrotated) bounding box of an object's baked ink, in
/// image coords: union of the bubble outline / tail / thought circles /
/// decorations / text, plus the stroke half-width. Used to size the temp buffer
/// for rotated baking. None if there's nothing to draw.
fn object_local_aabb(obj: &AnnotationObject, fonts: &FontSet) -> Option<(f32, f32, f32, f32)> {
    let mut minx = f32::INFINITY;
    let mut miny = f32::INFINITY;
    let mut maxx = f32::NEG_INFINITY;
    let mut maxy = f32::NEG_INFINITY;
    let mut acc = |x: f32, y: f32| {
        minx = minx.min(x);
        miny = miny.min(y);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
    };
    match &obj.kind {
        AnnotationKind::Bubble(bubble) => {
            let shape = effective_bubble_shape(bubble, fonts);
            // Tailless shapes (line fields / 意識 / なし) draw no tail, so don't let
            // a stale tail splice into the contour or inflate the AABB.
            let tail = bubble
                .tail
                .as_ref()
                .filter(|_| crate::tessellate::shape_renders_tail(&shape));
            let geo = bubble_geometry(&shape, obj.pivot, tail);
            for &(x, y) in &geo.outline {
                acc(x, y);
            }
            for &(cx, cy, r) in &geo.thought {
                acc(cx - r, cy - r);
                acc(cx + r, cy + r);
            }
            if let Some(t) = tail {
                acc(t.tip.0, t.tip.1);
            }
            // Decoration extents.
            if !bubble.decorations.is_empty() {
                let short_side = match shape {
                    BubbleShape::Ellipse { rx, ry } => rx.min(ry),
                    BubbleShape::RoundRect { half_w, half_h, .. } => half_w.min(half_h),
                    BubbleShape::Burst { rx, ry, .. } => rx.min(ry),
                    BubbleShape::Cloud { rx, ry, .. } => rx.min(ry),
                    BubbleShape::Polygon { rx, ry, .. } => rx.min(ry),
                    BubbleShape::Diamond { half_w, half_h } => half_w.min(half_h),
                    BubbleShape::Heart { rx, ry } => rx.min(ry),
                    BubbleShape::Arrow { half_w, half_h, .. } => half_w.min(half_h),
                    BubbleShape::Soft { half_w, half_h, .. } => half_w.min(half_h),
                    BubbleShape::MotionLines { rx, ry, .. } => rx.min(ry),
                    BubbleShape::SpeedLines { half_w, half_h, .. } => half_w.min(half_h),
                    BubbleShape::TextOnly { half_w, half_h } => half_w.min(half_h),
                    BubbleShape::Concentration { rx, ry, .. } => rx.min(ry),
                    BubbleShape::Strokes { half_w, half_h, .. } => half_w.min(half_h),
                    BubbleShape::DoubleStroke { half_w, half_h, .. } => half_w.min(half_h),
                };
                let tail_base = match (&geo.tail, &bubble.tail) {
                    (Some(t), _) => Some(((t[0].0 + t[1].0) * 0.5, (t[0].1 + t[1].1) * 0.5)),
                    (None, Some(t)) => Some(resolve_tail_base(&shape, obj.pivot, t)),
                    (None, None) => None,
                };
                for layer in &bubble.decorations {
                    for d in &place_decorations(layer, &geo.body, obj.pivot, short_side, tail_base)
                    {
                        // Flowers reach ~1.05*size beyond center; add outline too.
                        let ext = d.size * 1.1 + layer.outline_width.max(0.0);
                        acc(d.cx - ext, d.cy - ext);
                        acc(d.cx + ext, d.cy + ext);
                    }
                }
            }
            // Embedded text (centered on pivot). Pad by the text's own 袋文字
            // width so a rotated bubble's text halo isn't clipped by the temp
            // buffer before rotate_blit (the trailing `m` only covers the
            // bubble frame stroke, not the per-glyph outline dilation).
            if let Some(font) = fonts.get(&bubble.text.font_key) {
                if !bubble.text.text.is_empty() {
                    let (lw, lh) = layout_text(&bubble.text, font).bounds;
                    let tm = text_effect_padding(&bubble.text);
                    acc(obj.pivot.0 - lw * 0.5 - tm, obj.pivot.1 - lh * 0.5 - tm);
                    acc(obj.pivot.0 + lw * 0.5 + tm, obj.pivot.1 + lh * 0.5 + tm);
                }
            }
            // Pad by the stroke's OUTER reach so a rotated bubble's outline isn't
            // clipped by the temp buffer before rotate_blit. A merged member draws
            // its stroke at MERGE_STROKE_SCALE (the inner half is erased, leaving the
            // outer half), so the outline can reach `width/2 * scale` outward. Pad for
            // that worst case unconditionally (over-padding an unmerged bubble only
            // enlarges the temp buffer slightly — rotate_blit copies opaque pixels
            // only, so there's no visual effect). 線 (sketch) also jitters vertices
            // outward by up to SKETCH_MAX_JITTER beyond the stroke.
            let sketch_extra = if matches!(shape, BubbleShape::Strokes { .. }) {
                SKETCH_MAX_JITTER
            } else {
                0.0
            };
            let m =
                bubble.outline.width_px.max(0.0) * 0.5 * MERGE_STROKE_SCALE + 1.0 + sketch_extra;
            minx -= m;
            miny -= m;
            maxx += m;
            maxy += m;
        }
        AnnotationKind::Text(text) => {
            let font = fonts.get(&text.font_key)?;
            if text.text.is_empty() {
                return None;
            }
            let (lw, lh) = layout_text(text, font).bounds;
            // Standalone text: pivot is the layout top-left. Pad for text effects.
            let m = text_effect_padding(text) + 1.0;
            acc(obj.pivot.0 - m, obj.pivot.1 - m);
            acc(obj.pivot.0 + lw + m, obj.pivot.1 + lh + m);
        }
        AnnotationKind::MessageWindow(win) => {
            // Panel rect (centered on pivot) + outline half-width + frame gap,
            // unioned with the shadow offset and any 名前プレート placed above.
            let (hw, hh) = effective_window_half_extents(win, fonts);
            let m = win.outline.width_px.max(0.0) * 0.5 + win.frame_gap_px.max(0.0) + 1.0;
            acc(obj.pivot.0 - hw - m, obj.pivot.1 - hh - m);
            acc(obj.pivot.0 + hw + m, obj.pivot.1 + hh + m);
            if let Some(sh) = win.shadow {
                acc(
                    obj.pivot.0 + hw + sh.offset.0,
                    obj.pivot.1 + hh + sh.offset.1,
                );
                acc(
                    obj.pivot.0 - hw + sh.offset.0,
                    obj.pivot.1 - hh + sh.offset.1,
                );
            }
            if win.name_plate.mode == NamePlateMode::Above {
                // The plate sits above the top edge (offset.1 is typically < 0).
                // Bound its full rect (it can be wide / horizontally offset).
                let np = &win.name_plate;
                let (nw, nh) = fonts
                    .get(&np.name.font_key)
                    .map(|f| layout_text(&np.name, f).bounds)
                    .unwrap_or((np.name.size_px * 4.0, np.name.size_px));
                let pad = np.padding_px.max(0.0);
                let (pw, ph) = (nw + pad * 2.0, nh + pad * 2.0);
                // Slack for the plate stroke + the name's 袋文字 halo.
                let s = np.outline.width_px.max(0.0) * 0.5 + text_effect_padding(&np.name) + 1.0;
                let px = obj.pivot.0 - hw + np.offset.0;
                let py = obj.pivot.1 - hh + np.offset.1 - ph;
                acc(px - s, py - s);
                acc(px + pw + s, py + ph + s);
            }
        }
        AnnotationKind::Stamp(stamp) => {
            // Rect centered on pivot, plus the sticker outline width.
            let m = stamp.outline.map(|s| s.width_px).unwrap_or(0.0).max(0.0) + 1.0;
            acc(
                obj.pivot.0 - stamp.half_w - m,
                obj.pivot.1 - stamp.half_h - m,
            );
            acc(
                obj.pivot.0 + stamp.half_w + m,
                obj.pivot.1 + stamp.half_h + m,
            );
        }
    }
    if minx > maxx || miny > maxy {
        return None;
    }
    Some((minx, miny, maxx, maxy))
}

/// The image-space pivot used for object rotation. Most objects store their
/// pivot at their visual center. Standalone text keeps `obj.pivot` as the
/// layout top-left for placement, so derive a center pivot from its layout.
fn object_rotation_pivot(obj: &AnnotationObject, fonts: &FontSet) -> (f32, f32) {
    if let AnnotationKind::Text(text) = &obj.kind {
        if !text.text.is_empty() {
            if let Some(font) = fonts.get(&text.font_key) {
                let (lw, lh) = layout_text(text, font).bounds;
                return (obj.pivot.0 + lw * 0.5, obj.pivot.1 + lh * 0.5);
            }
        }
    }
    obj.pivot
}

/// Bilinear-sample `src` (straight-alpha RGBA) at fractional pixel (x, y),
/// premultiplying so transparent neighbours don't bleed dark fringes. Out-of
/// bounds samples are treated as fully transparent. Returns straight RGBA.
fn sample_bilinear(src: &RgbaOverlay, x: f32, y: f32) -> (f32, f32, f32, f32) {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let at = |px: i32, py: i32| -> (f32, f32, f32, f32) {
        if px < 0 || py < 0 || px as usize >= src.w || py as usize >= src.h {
            return (0.0, 0.0, 0.0, 0.0);
        }
        let i = (py as usize * src.w + px as usize) * 4;
        let a = src.pixels[i + 3] as f32 / 255.0;
        // Premultiplied.
        (
            src.pixels[i] as f32 / 255.0 * a,
            src.pixels[i + 1] as f32 / 255.0 * a,
            src.pixels[i + 2] as f32 / 255.0 * a,
            a,
        )
    };
    let p00 = at(x0, y0);
    let p10 = at(x0 + 1, y0);
    let p01 = at(x0, y0 + 1);
    let p11 = at(x0 + 1, y0 + 1);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let mix = |a: (f32, f32, f32, f32), b: (f32, f32, f32, f32), t: f32| {
        (
            lerp(a.0, b.0, t),
            lerp(a.1, b.1, t),
            lerp(a.2, b.2, t),
            lerp(a.3, b.3, t),
        )
    };
    let top = mix(p00, p10, fx);
    let bot = mix(p01, p11, fx);
    let (pr, pg, pb, pa) = mix(top, bot, fy);
    if pa <= 0.0 {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        // Un-premultiply back to straight alpha.
        (pr / pa, pg / pa, pb / pa, pa)
    }
}

/// Rotate-blit `src` into `dst`. `src_origin` is the image-space position of
/// `src`'s (0,0); the content is rotated by `theta` (radians, CW in image space)
/// around `pivot`. Inverse-mapped per dest pixel with bilinear sampling.
fn rotate_blit(
    dst: &mut RgbaOverlay,
    src: &RgbaOverlay,
    src_origin: (f32, f32),
    pivot: (f32, f32),
    theta: f32,
) {
    let (sin, cos) = theta.sin_cos();
    // Dest AABB = rotated corners of src's absolute rect.
    let (ox, oy) = src_origin;
    let corners = [
        (ox, oy),
        (ox + src.w as f32, oy),
        (ox + src.w as f32, oy + src.h as f32),
        (ox, oy + src.h as f32),
    ];
    let mut minx = f32::INFINITY;
    let mut miny = f32::INFINITY;
    let mut maxx = f32::NEG_INFINITY;
    let mut maxy = f32::NEG_INFINITY;
    for &(cx, cy) in &corners {
        let rx = cx - pivot.0;
        let ry = cy - pivot.1;
        // Forward rotation R(theta).
        let px = pivot.0 + rx * cos - ry * sin;
        let py = pivot.1 + rx * sin + ry * cos;
        minx = minx.min(px);
        miny = miny.min(py);
        maxx = maxx.max(px);
        maxy = maxy.max(py);
    }
    let x0 = (minx.floor() as i32).max(0);
    let y0 = (miny.floor() as i32).max(0);
    let x1 = (maxx.ceil() as i32).min(dst.w as i32 - 1);
    let y1 = (maxy.ceil() as i32).min(dst.h as i32 - 1);
    for dy in y0..=y1 {
        for dx in x0..=x1 {
            let relx = dx as f32 + 0.5 - pivot.0;
            let rely = dy as f32 + 0.5 - pivot.1;
            // Inverse rotation R(-theta).
            let ux = relx * cos + rely * sin;
            let uy = -relx * sin + rely * cos;
            let sx = pivot.0 + ux - ox - 0.5;
            let sy = pivot.1 + uy - oy - 0.5;
            let (r, g, b, a) = sample_bilinear(src, sx, sy);
            if a <= 0.0 {
                continue;
            }
            dst.blend_px(
                dx,
                dy,
                Rgba::new(
                    (r * 255.0).round().clamp(0.0, 255.0) as u8,
                    (g * 255.0).round().clamp(0.0, 255.0) as u8,
                    (b * 255.0).round().clamp(0.0, 255.0) as u8,
                    (a * 255.0).round().clamp(0.0, 255.0) as u8,
                ),
                1.0,
            );
        }
    }
}

/// Composite a pre-rasterized stamp image, centered on `pivot`, scaled to the
/// stamp's `2*half_w × 2*half_h` on-canvas size (bilinear), with optional
/// horizontal/vertical flips, a sticker outline (silhouette-alpha dilation laid
/// behind), and `opacity`. Object rotation is handled by `bake_into` (this draws
/// unrotated). `img` is straight-alpha RGBA in any source resolution.
///
/// Delegates the image+halo composite to [`composite_stamp_sticker`] (the same fn
/// the lab's GPU quad path uses) and blits the result as **one unit** with
/// `opacity`. So an outlined stamp fades identically whether it renders here (CPU
/// bake, e.g. below a bubble) or as a GPU quad (top-z run). Source-over is
/// associative, so at opacity 1 this is pixel-identical to compositing halo then
/// fill straight onto the overlay; at opacity < 1 both paths now fade the
/// composited sticker as a whole (see `sticker_matches_cpu_bake`).
fn draw_stamp(
    overlay: &mut RgbaOverlay,
    pivot: (f32, f32),
    stamp: &StampObject,
    img: &RgbaOverlay,
) {
    if img.w == 0 || img.h == 0 {
        return;
    }
    let opacity = stamp.opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }
    // Hard-cap the rasterized size so a corrupt/huge sidecar value can't OOM
    // (legit UI sizes are far below this; matches the rotated-bake temp cap).
    const STAMP_MAX_PX: usize = 8192;
    let tw = ((stamp.half_w * 2.0).round().max(1.0) as usize).min(STAMP_MAX_PX);
    let th = ((stamp.half_h * 2.0).round().max(1.0) as usize).min(STAMP_MAX_PX);

    let (sticker, rad) =
        composite_stamp_sticker(img, stamp.flip_h, stamp.flip_v, tw, th, stamp.outline);
    // The sticker's image occupies the centered (capped) tw×th rect; its halo adds
    // `rad` px of padding on every side, so its top-left lands at this float origin.
    let left = pivot.0 - tw as f32 * 0.5 - rad as f32;
    let top = pivot.1 - th as f32 * 0.5 - rad as f32;
    blit_overlay_with_opacity(overlay, &sticker, left, top, opacity);
}

/// Blit a standalone straight-alpha `src` overlay onto `dst` at float origin
/// (`ox`,`oy`) with a uniform `opacity` (source-over). Truncating `ox + sx` per
/// pixel reproduces the legacy per-texel placement exactly. Used to lay a
/// pre-composited stamp sticker onto the bake so the CPU path and the GPU quad
/// path fade an outlined stamp identically (as one unit).
fn blit_overlay_with_opacity(
    dst: &mut RgbaOverlay,
    src: &RgbaOverlay,
    ox: f32,
    oy: f32,
    opacity: f32,
) {
    let dh = dst.h as i32;
    for sy in 0..src.h {
        let dy = (oy + sy as f32) as i32;
        if dy < 0 || dy >= dh {
            continue;
        }
        for sx in 0..src.w {
            let i = (sy * src.w + sx) * 4;
            let a = src.pixels[i + 3];
            if a == 0 {
                continue;
            }
            let dx = (ox + sx as f32) as i32;
            // blend_px clamps x to the destination bounds.
            dst.blend_px(
                dx,
                dy,
                Rgba::new(src.pixels[i], src.pixels[i + 1], src.pixels[i + 2], a),
                opacity,
            );
        }
    }
}

/// Resolve a source stamp image into a `tw`×`th` straight-alpha RGBA buffer,
/// applying horizontal/vertical flips with bilinear sampling (source-center of
/// each target texel). Shared by the CPU stamp bake ([`draw_stamp`]) and the GPU
/// sticker pre-composite ([`composite_stamp_sticker`]) so both sample identically.
fn resolve_stamp_buf(
    img: &RgbaOverlay,
    flip_h: bool,
    flip_v: bool,
    tw: usize,
    th: usize,
) -> Vec<u8> {
    let mut buf = vec![0u8; tw * th * 4];
    for ty in 0..th {
        for tx in 0..tw {
            let u = (tx as f32 + 0.5) / tw as f32;
            let v = (ty as f32 + 0.5) / th as f32;
            let su = if flip_h { 1.0 - u } else { u };
            let sv = if flip_v { 1.0 - v } else { v };
            let sx = su * img.w as f32 - 0.5;
            let sy = sv * img.h as f32 - 0.5;
            let (r, g, b, a) = sample_bilinear(img, sx, sy);
            let i = (ty * tw + tx) * 4;
            buf[i] = (r * 255.0).round().clamp(0.0, 255.0) as u8;
            buf[i + 1] = (g * 255.0).round().clamp(0.0, 255.0) as u8;
            buf[i + 2] = (b * 255.0).round().clamp(0.0, 255.0) as u8;
            buf[i + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    buf
}

/// Max circular silhouette-dilation coverage of the `tw`×`th` alpha in `buf`
/// (straight-alpha RGBA) at local pixel (`tx`,`ty`) within radius `rad`
/// (`r2 = rad²`). Shared by the CPU bake and the GPU sticker pre-composite so the
/// halo edge is identical. Returns early at full coverage.
#[inline]
fn sticker_halo_coverage(
    buf: &[u8],
    tw: usize,
    th: usize,
    tx: i32,
    ty: i32,
    rad: i32,
    r2: f32,
) -> f32 {
    let alpha_at = |x: i32, y: i32| -> f32 {
        if x < 0 || y < 0 || x as usize >= tw || y as usize >= th {
            0.0
        } else {
            buf[(y as usize * tw + x as usize) * 4 + 3] as f32 / 255.0
        }
    };
    let mut m = 0.0f32;
    for dy in -rad..=rad {
        for dx in -rad..=rad {
            if (dx * dx + dy * dy) as f32 > r2 {
                continue;
            }
            let a = alpha_at(tx + dx, ty + dy);
            if a > m {
                m = a;
                if m >= 1.0 {
                    return 1.0;
                }
            }
        }
    }
    m
}

/// Pre-composite a stamp's image + sticker-outline halo into a standalone
/// straight-alpha RGBA overlay at **opacity 1**, so a caller (e.g. the lab's GPU
/// quad path) can upload it once and reuse it across duplicates instead of
/// re-rasterizing the halo on every bake. The image is rendered at `tw`×`th`
/// (flips baked in) and centered with `rad`-px transparent padding on every side,
/// so the returned overlay is `(tw + 2·rad)`×`(th + 2·rad)` and the halo can
/// extend beyond the image edge exactly as [`draw_stamp`] draws it. Returns the
/// overlay and `rad` (px the sticker extends past the image half-extents); apply
/// opacity, rotation and on-canvas placement at draw time. `outline = None` (or
/// width ≤ 0) yields the bare image with `rad = 0`.
///
/// Keeps the halo math in sync with [`draw_stamp`] — both use [`resolve_stamp_buf`]
/// + [`sticker_halo_coverage`]; `sticker_matches_cpu_bake` guards against drift.
pub fn composite_stamp_sticker(
    img: &RgbaOverlay,
    flip_h: bool,
    flip_v: bool,
    tw: usize,
    th: usize,
    outline: Option<StrokeStyle>,
) -> (RgbaOverlay, usize) {
    if img.w == 0 || img.h == 0 || tw == 0 || th == 0 {
        return (RgbaOverlay::new(tw.max(1), th.max(1)), 0);
    }
    let buf = resolve_stamp_buf(img, flip_h, flip_v, tw, th);
    let rad = outline
        .filter(|s| s.width_px > 0.0)
        .map(|s| (s.width_px.round().max(0.0) as i32).min(256))
        .unwrap_or(0)
        .max(0) as usize;
    let mut out = RgbaOverlay::new(tw + 2 * rad, th + 2 * rad);
    // Halo first (behind). Local image coords (tx,ty) land at (tx+rad, ty+rad) in
    // the padded frame.
    if rad > 0 {
        if let Some(stroke) = outline {
            let r = rad as i32;
            let r2 = (r * r) as f32;
            for ty in -r..(th as i32 + r) {
                for tx in -r..(tw as i32 + r) {
                    let m = sticker_halo_coverage(&buf, tw, th, tx, ty, r, r2);
                    if m > 0.0 {
                        out.blend_px(tx + r, ty + r, stroke.color, m);
                    }
                }
            }
        }
    }
    // Fill: the stamp pixels over the halo at offset (rad, rad).
    for ty in 0..th {
        for tx in 0..tw {
            let i = (ty * tw + tx) * 4;
            let a = buf[i + 3];
            if a == 0 {
                continue;
            }
            out.blend_px(
                (tx + rad) as i32,
                (ty + rad) as i32,
                Rgba::new(buf[i], buf[i + 1], buf[i + 2], a),
                1.0,
            );
        }
    }
    (out, rad)
}

/// Placeholder for a stamp whose image is unavailable (e.g. a moved/missing user
/// file): a translucent gray box with a dashed-ish border + diagonal cross so the
/// object is visible and selectable but obviously "needs its image".
fn draw_stamp_placeholder(overlay: &mut RgbaOverlay, pivot: (f32, f32), stamp: &StampObject) {
    let hw = stamp.half_w.max(2.0);
    let hh = stamp.half_h.max(2.0);
    let rect = [
        (pivot.0 - hw, pivot.1 - hh),
        (pivot.0 + hw, pivot.1 - hh),
        (pivot.0 + hw, pivot.1 + hh),
        (pivot.0 - hw, pivot.1 + hh),
    ];
    fill_polygon(overlay, &rect, Rgba::new(140, 140, 150, 90));
    let border = StrokeStyle {
        color: Rgba::new(90, 90, 100, 220),
        width_px: 2.0,
    };
    stroke_polygon(overlay, &rect, &border);
    // Diagonal cross.
    stroke_segment(
        overlay,
        (pivot.0 - hw, pivot.1 - hh),
        (pivot.0 + hw, pivot.1 + hh),
        &border,
    );
    stroke_segment(
        overlay,
        (pivot.0 + hw, pivot.1 - hh),
        (pivot.0 - hw, pivot.1 + hh),
        &border,
    );
}

/// Lay out + draw a TextBlock. When `centered` (bubble text), the layout is
/// centered on the pivot; otherwise the pivot is the layout's top-left.
fn bake_text(
    overlay: &mut RgbaOverlay,
    block: &TextBlock,
    pivot: (f32, f32),
    fonts: &FontSet,
    centered: bool,
) {
    if block.text.is_empty() {
        return;
    }
    let Some(font) = fonts.get(&block.font_key) else {
        return;
    };
    let layout = layout_text(block, font);
    let (lw, lh) = layout.bounds;
    let (origin_x, origin_y) = if centered {
        (pivot.0 - lw * 0.5, pivot.1 - lh * 0.5)
    } else {
        (pivot.0, pivot.1)
    };
    draw_layout_glyphs(overlay, &layout, block, font, origin_x, origin_y, None);
}

/// Bake a TextBlock flowed into a content rect `(x0, y0, x1, y1)`: wrap to the
/// rect (Japanese kinsoku) when `wrap`, align horizontally by `block.align`,
/// anchor vertically by `v_anchor`, and clip glyphs to the rect. Used by message
/// windows (the bubble/text paths use `bake_text`, which is unconstrained).
fn bake_text_in_rect(
    overlay: &mut RgbaOverlay,
    block: &TextBlock,
    rect: (f32, f32, f32, f32),
    v_anchor: VAnchor,
    wrap: bool,
    fonts: &FontSet,
) {
    if block.text.is_empty() {
        return;
    }
    let Some(font) = fonts.get(&block.font_key) else {
        return;
    };
    let cw = (rect.2 - rect.0).max(1.0);
    let ch = (rect.3 - rect.1).max(1.0);
    let wrap_axis = if wrap {
        match block.orientation {
            Orientation::Horizontal => Some(cw),
            Orientation::Vertical => Some(ch),
        }
    } else {
        None
    };
    let layout = layout_text_wrapped(block, font, wrap_axis);
    let (lw, lh) = layout.bounds;
    let oy = match v_anchor {
        VAnchor::Top => rect.1,
        VAnchor::Center => rect.1 + (ch - lh) * 0.5,
        VAnchor::Bottom => rect.1 + (ch - lh),
    };
    let ox = match block.orientation {
        // Horizontal: align the block in the content width by `align`.
        Orientation::Horizontal => match block.align {
            TextAlign::Start => rect.0,
            TextAlign::Center => rect.0 + (cw - lw) * 0.5,
            TextAlign::End => rect.0 + (cw - lw),
        },
        // Vertical: columns advance right-to-left, so right-anchor the block.
        Orientation::Vertical => (rect.0 + (cw - lw)).max(rect.0),
    };
    draw_layout_glyphs(overlay, &layout, block, font, ox, oy, Some(rect));
}

/// Draw a laid-out block's glyphs at `(origin_x, origin_y)`, optionally clipped
/// to a rect `(x0, y0, x1, y1)`. 袋文字 halo first, fill on top; 横倒し glyphs are
/// rotated 90° CW and centered. Shared by `bake_text` (no clip) and
/// `bake_text_in_rect` (clipped to the content rect).
fn draw_layout_glyphs(
    overlay: &mut RgbaOverlay,
    layout: &TextLayout,
    block: &TextBlock,
    font: &LoadedFont,
    origin_x: f32,
    origin_y: f32,
    clip: Option<(f32, f32, f32, f32)>,
) {
    draw_text_background(overlay, layout, block, origin_x, origin_y, clip);

    if let Some(shadow) = block.shadow {
        draw_layout_soft_mask(
            overlay,
            layout,
            font,
            origin_x + shadow.offset.0,
            origin_y + shadow.offset.1,
            shadow.color,
            shadow.spread_px,
            shadow.blur_px,
            clip,
        );
    }

    if let Some(echo) = block.echo {
        let count = echo.count.clamp(1, 12);
        for i in (1..=count).rev() {
            let t = i as f32;
            let alpha_scale = 1.0 - (i - 1) as f32 / count as f32 * 0.45;
            draw_layout_mask(
                overlay,
                layout,
                font,
                origin_x + echo.offset.0 * t,
                origin_y + echo.offset.1 * t,
                0.0,
                color_with_alpha_scale(echo.color, alpha_scale),
                clip,
            );
        }
    }

    if let Some(glow) = block.glow {
        draw_layout_soft_mask(
            overlay,
            layout,
            font,
            origin_x,
            origin_y,
            glow.color,
            glow.spread_px,
            glow.radius_px,
            clip,
        );
    }

    let mut outlines: Vec<StrokeStyle> = block
        .extra_outlines
        .iter()
        .copied()
        .filter(|s| s.width_px > 0.0 && s.color.a > 0)
        .collect();
    if let Some(outline) = block.outline.filter(|s| s.width_px > 0.0 && s.color.a > 0) {
        outlines.push(outline);
    }
    outlines.sort_by(|a, b| {
        b.width_px
            .partial_cmp(&a.width_px)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for stroke in outlines {
        draw_layout_mask(
            overlay,
            layout,
            font,
            origin_x,
            origin_y,
            stroke.width_px,
            stroke.color,
            clip,
        );
    }

    draw_layout_mask(
        overlay,
        layout,
        font,
        origin_x,
        origin_y,
        0.0,
        block.color,
        clip,
    );
}

/// Blit a coverage bitmap centered at (cx, cy) in image space, in `color`,
/// optionally clipped to a rect `(x0, y0, x1, y1)`.
fn blit_centered(
    overlay: &mut RgbaOverlay,
    bmp: &GlyphBitmap,
    cx: f32,
    cy: f32,
    color: Rgba,
    clip: Option<(f32, f32, f32, f32)>,
) {
    let left = cx - bmp.width as f32 * 0.5;
    let top = cy - bmp.height as f32 * 0.5;
    blit_bitmap(overlay, bmp, left, top, color, clip);
}

fn blit_bitmap(
    overlay: &mut RgbaOverlay,
    bmp: &GlyphBitmap,
    left: f32,
    top: f32,
    color: Rgba,
    clip: Option<(f32, f32, f32, f32)>,
) {
    for py in 0..bmp.height {
        for px in 0..bmp.width {
            let c = bmp.coverage[py * bmp.width + px];
            if c > 0.0 {
                let (ix, iy) = ((left + px as f32) as i32, (top + py as f32) as i32);
                let ok = match clip {
                    None => true,
                    Some((x0, y0, x1, y1)) => {
                        let (xf, yf) = (ix as f32, iy as f32);
                        xf >= x0 && xf < x1 && yf >= y0 && yf < y1
                    }
                };
                if ok {
                    overlay.blend_px(ix, iy, color, c);
                }
            }
        }
    }
}

fn draw_layout_mask(
    overlay: &mut RgbaOverlay,
    layout: &TextLayout,
    font: &LoadedFont,
    origin_x: f32,
    origin_y: f32,
    dilate_px: f32,
    color: Rgba,
    clip: Option<(f32, f32, f32, f32)>,
) {
    if color.a == 0 {
        return;
    }
    for g in &layout.glyphs {
        let Some(bmp) = font.rasterize_gid(g.glyph_id, g.size, dilate_px.max(0.0)) else {
            continue;
        };
        if g.form == GlyphForm::Sideways {
            let rot = rotate_cw(&bmp);
            blit_centered(overlay, &rot, origin_x + g.x, origin_y + g.y, color, clip);
        } else {
            blit_bitmap(
                overlay,
                &bmp,
                origin_x + g.x + bmp.left,
                origin_y + g.y + bmp.top,
                color,
                clip,
            );
        }
    }
}

fn draw_layout_soft_mask(
    overlay: &mut RgbaOverlay,
    layout: &TextLayout,
    font: &LoadedFont,
    origin_x: f32,
    origin_y: f32,
    color: Rgba,
    spread_px: f32,
    blur_px: f32,
    clip: Option<(f32, f32, f32, f32)>,
) {
    let spread = spread_px.max(0.0);
    let blur = blur_px.max(0.0).min(48.0);
    if blur <= 0.1 {
        draw_layout_mask(
            overlay, layout, font, origin_x, origin_y, spread, color, clip,
        );
        return;
    }
    let steps = ((blur / 3.0).ceil() as u32).clamp(2, 8);
    // A translucent crisp core keeps the effect readable; wider low-alpha passes
    // approximate blur without introducing a separate full-image blur buffer.
    draw_layout_mask(
        overlay,
        layout,
        font,
        origin_x,
        origin_y,
        spread,
        color_with_alpha_scale(color, 0.45),
        clip,
    );
    for i in (1..=steps).rev() {
        let t = i as f32 / steps as f32;
        let dilate = spread + blur * t;
        let alpha_scale = 0.75 / steps as f32 * (0.35 + 0.65 * t);
        draw_layout_mask(
            overlay,
            layout,
            font,
            origin_x,
            origin_y,
            dilate,
            color_with_alpha_scale(color, alpha_scale),
            clip,
        );
    }
}

fn draw_text_background(
    overlay: &mut RgbaOverlay,
    layout: &TextLayout,
    block: &TextBlock,
    origin_x: f32,
    origin_y: f32,
    clip: Option<(f32, f32, f32, f32)>,
) {
    let Some(bg) = block.background else {
        return;
    };
    if bg.fill.a == 0 {
        return;
    }
    let pad = bg.padding_px.max(0.0);
    let (lw, lh) = layout.bounds;
    let x0 = origin_x - pad;
    let y0 = origin_y - pad;
    let x1 = origin_x + lw + pad;
    let y1 = origin_y + lh + pad;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let poly = tessellate_bubble(
        &BubbleShape::RoundRect {
            half_w: (x1 - x0) * 0.5,
            half_h: (y1 - y0) * 0.5,
            corner_px: bg.corner_px.max(0.0),
        },
        ((x0 + x1) * 0.5, (y0 + y1) * 0.5),
    );
    fill_polygon_clipped(overlay, &poly, bg.fill, clip);
}

fn text_effect_padding(block: &TextBlock) -> f32 {
    let mut pad = block.outline.map(|s| s.width_px).unwrap_or(0.0).max(0.0);
    for st in &block.extra_outlines {
        pad = pad.max(st.width_px.max(0.0));
    }
    if let Some(bg) = block.background {
        pad = pad.max(bg.padding_px.max(0.0));
    }
    if let Some(sh) = block.shadow {
        let reach =
            sh.spread_px.max(0.0) + sh.blur_px.max(0.0) + sh.offset.0.abs().max(sh.offset.1.abs());
        pad = pad.max(reach);
    }
    if let Some(glow) = block.glow {
        pad = pad.max(glow.spread_px.max(0.0) + glow.radius_px.max(0.0));
    }
    if let Some(echo) = block.echo {
        let reach = echo.offset.0.abs().max(echo.offset.1.abs()) * echo.count.clamp(1, 12) as f32;
        pad = pad.max(reach);
    }
    pad
}

fn color_with_alpha_scale(color: Rgba, scale: f32) -> Rgba {
    Rgba {
        a: (color.a as f32 * scale.clamp(0.0, 1.0))
            .round()
            .clamp(0.0, 255.0) as u8,
        ..color
    }
}

/// Approximate a circle as a closed polygon (for thought-tail circles).
fn circle_poly(cx: f32, cy: f32, r: f32, segs: usize) -> Vec<(f32, f32)> {
    let segs = segs.max(8);
    let mut p = Vec::with_capacity(segs);
    for i in 0..segs {
        let t = i as f32 / segs as f32 * std::f32::consts::TAU;
        p.push((cx + r * t.cos(), cy + r * t.sin()));
    }
    p
}

/// Draw one procedural decoration. Styling (outline width/color, star points,
/// petal count, flower center, 泡 gradient) comes from the source `layer`.
fn draw_decoration(overlay: &mut RgbaOverlay, deco: &PlacedDeco, layer: &DecorationLayer) {
    // Outline alpha follows the fill alpha so a translucent decoration keeps a
    // matching translucent edge. `outline_width <= 0` disables the outline.
    let stroke = StrokeStyle {
        color: Rgba {
            a: layer.outline_color.a.min(deco.color.a),
            ..layer.outline_color
        },
        width_px: layer.outline_width,
    };
    let do_stroke = layer.outline_width > 0.0;
    match deco.kind {
        DecoKind::Sparkle => {
            // N-point star (sharp): outer tips at size, inner notches at ~0.32.
            let points = layer.points.max(3);
            let poly = star_poly(
                deco.cx,
                deco.cy,
                deco.size,
                deco.size * 0.32,
                points,
                deco.rot,
            );
            fill_polygon(overlay, &poly, deco.color);
            if do_stroke {
                stroke_polygon(overlay, &poly, &stroke);
            }
        }
        DecoKind::Flower => {
            // `petals` rounded petals around a center + a small center dot.
            let petals = layer.petals.max(3);
            for petal in 0..petals {
                let a = deco.rot + petal as f32 / petals as f32 * std::f32::consts::TAU;
                let pcx = deco.cx + a.cos() * deco.size * 0.55;
                let pcy = deco.cy + a.sin() * deco.size * 0.55;
                let poly = circle_poly(pcx, pcy, deco.size * 0.5, 16);
                fill_polygon(overlay, &poly, deco.color);
                if do_stroke {
                    stroke_polygon(overlay, &poly, &stroke);
                }
            }
            let center = Rgba {
                a: layer.center_color.a.min(deco.color.a),
                ..layer.center_color
            };
            let cpoly = circle_poly(deco.cx, deco.cy, deco.size * 0.36, 14);
            fill_polygon(overlay, &cpoly, center);
            if do_stroke {
                stroke_polygon(overlay, &cpoly, &stroke);
            }
        }
        DecoKind::Bubble => {
            if layer.gradient {
                draw_soap_bubble(overlay, deco.cx, deco.cy, deco.size * 0.7, deco.color);
                if do_stroke {
                    let poly = circle_poly(deco.cx, deco.cy, deco.size * 0.7, 18);
                    stroke_polygon(overlay, &poly, &stroke);
                }
            } else {
                // A small filled + stroked circle (泡).
                let poly = circle_poly(deco.cx, deco.cy, deco.size * 0.7, 18);
                fill_polygon(overlay, &poly, deco.color);
                if do_stroke {
                    stroke_polygon(overlay, &poly, &stroke);
                }
            }
        }
    }
}

/// A translucent soap-bubble: a radial gradient that is most transparent at the
/// center and densest at the rim (per-pixel so the alpha truly increases with
/// radius), plus a small bright highlight near the upper-left.
fn draw_soap_bubble(overlay: &mut RgbaOverlay, cx: f32, cy: f32, r: f32, color: Rgba) {
    if r < 0.5 {
        return;
    }
    let base_a = color.a as f32 / 255.0;
    let x0 = (cx - r).floor() as i32;
    let x1 = (cx + r).ceil() as i32;
    let y0 = (cy - r).floor() as i32;
    let y1 = (cy + r).ceil() as i32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > r {
                continue;
            }
            let t = dist / r; // 0 center .. 1 rim
            // Denser toward the rim; small base so the center stays see-through.
            let mut cov = 0.10 + 0.85 * t * t;
            // Antialias the last pixel of the rim.
            if dist > r - 1.0 {
                cov *= (r - dist).clamp(0.0, 1.0);
            }
            overlay.blend_px(x, y, color, base_a * cov);
        }
    }
    // Specular highlight (upper-left), white, small.
    let hx = cx - r * 0.32;
    let hy = cy - r * 0.32;
    let hl = Rgba::new(255, 255, 255, (color.a as f32 * 0.75) as u8);
    let poly = circle_poly(hx, hy, (r * 0.22).max(1.0), 12);
    fill_polygon(overlay, &poly, hl);
}

/// Star polygon: alternating outer (`r_out`) and inner (`r_in`) radii over
/// `points` spikes, rotated by `rot`.
fn star_poly(cx: f32, cy: f32, r_out: f32, r_in: f32, points: u32, rot: f32) -> Vec<(f32, f32)> {
    let points = points.max(3);
    let n = points * 2;
    let mut p = Vec::with_capacity(n as usize);
    for i in 0..n {
        let a = rot + (i as f32) / (n as f32) * std::f32::consts::TAU;
        let r = if i % 2 == 0 { r_out } else { r_in };
        p.push((cx + r * a.cos(), cy + r * a.sin()));
    }
    p
}

/// The panel half-extents actually drawn. `AutoFitText` derives them from the
/// (un-wrapped) text + padding (+ portrait slot); other modes use the stored
/// `half_w`/`half_h`. Shared by the rasterizer and the lab (hit-test / handles)
/// so the picked region matches the baked pixels.
pub fn effective_window_half_extents(win: &MessageWindowObject, fonts: &FontSet) -> (f32, f32) {
    match win.size_mode {
        SizeMode::AutoFitText => {
            if let Some(font) = fonts.get(&win.text.font_key) {
                let (tw, th) = layout_text(&win.text, font).bounds;
                // Match the draw-time content-rect math so AutoFitText never
                // clips: a portrait consumes width + 2*margin on its side, and an
                // inline/boxed name plate pushes the body text down.
                let extra_w = if win.portrait.side != PortraitSide::None {
                    win.portrait.width_px + win.portrait.margin_px * 2.0
                } else {
                    0.0
                };
                let np_h = name_plate_content_offset(win, fonts);
                let pw = tw + win.padding.left + win.padding.right + extra_w;
                let ph = th + win.padding.top + win.padding.bottom + np_h;
                ((pw * 0.5).max(40.0), (ph * 0.5).max(24.0))
            } else {
                (win.half_w.max(1.0), win.half_h.max(1.0))
            }
        }
        _ => (win.half_w.max(1.0), win.half_h.max(1.0)),
    }
}

/// Vertical space (px) an inline/boxed name plate consumes at the top of the
/// content rect (0 for None / Above — those don't push the body text down).
/// Shared by the AutoFitText sizing and the draw-time `content_top`.
fn name_plate_content_offset(win: &MessageWindowObject, fonts: &FontSet) -> f32 {
    let np = &win.name_plate;
    if !matches!(np.mode, NamePlateMode::Inline | NamePlateMode::Boxed) {
        return 0.0;
    }
    let nh = fonts
        .get(&np.name.font_key)
        .map(|f| layout_text(&np.name, f).bounds.1)
        .unwrap_or(np.name.size_px);
    let pad = if np.mode == NamePlateMode::Boxed {
        np.padding_px.max(0.0) * 2.0
    } else {
        0.0
    };
    // Mirror draw-time content_top = (top+padding) + offset.1 + plate_h + 4; a
    // positive y-offset pushes the body further down (negative can't pull it
    // above the padding, hence the clamp to 0).
    (np.offset.1 + nh + pad + 4.0).max(0.0)
}

/// Draw a full message window: shadow → fill → portrait → frame → name plate →
/// body text (wrapped + clipped to the content rect) → continue indicator. Like
/// the bubble path, the whole thing is rotation-wrapped by `bake_into`.
fn draw_message_window_parts(
    overlay: &mut RgbaOverlay,
    pivot: (f32, f32),
    win: &MessageWindowObject,
    fonts: &FontSet,
) {
    let (hw, hh) = effective_window_half_extents(win, fonts);
    let (cx, cy) = pivot;
    let (left, top, right, bottom) = (cx - hw, cy - hh, cx + hw, cy + hh);
    let corner = win.corner_px.clamp(0.0, hw.min(hh));
    let shape = BubbleShape::RoundRect {
        half_w: hw,
        half_h: hh,
        corner_px: corner,
    };
    let poly = tessellate_bubble(&shape, pivot);

    // 1. Drop shadow.
    if let Some(sh) = win.shadow {
        let shadow_poly = tessellate_bubble(&shape, (cx + sh.offset.0, cy + sh.offset.1));
        fill_polygon(overlay, &shadow_poly, sh.color);
    }

    // 2. Background fill.
    match win.fill_mode {
        FillMode::None => {}
        FillMode::Solid | FillMode::Translucent => {
            if let Some(fill) = win.fill {
                let a = (fill.a as f32 * win.fill_opacity.clamp(0.0, 1.0)).round() as u8;
                fill_polygon(overlay, &poly, Rgba { a, ..fill });
            }
        }
        FillMode::GradientScrim => {
            if let Some(fill) = win.fill {
                let base_a = fill.a as f32 * win.fill_opacity.clamp(0.0, 1.0);
                let dense = win.scrim_dense_side;
                let span = (bottom - top).max(1.0);
                fill_polygon_shaded(overlay, &poly, |_x, y| {
                    let f = ((y - top) / span).clamp(0.0, 1.0);
                    // t = 1 at the dense edge, fading to 0 at the opposite edge.
                    let t = match dense {
                        VAnchor::Bottom => f,
                        VAnchor::Top => 1.0 - f,
                        VAnchor::Center => 1.0 - (f - 0.5).abs() * 2.0,
                    };
                    Rgba {
                        a: (base_a * t).round().clamp(0.0, 255.0) as u8,
                        ..fill
                    }
                });
            }
        }
        FillMode::LinearGradient => {
            if let Some(fill) = win.fill {
                let to = win.gradient_to.unwrap_or(fill);
                let base_a = win.fill_opacity.clamp(0.0, 1.0);
                let span = (bottom - top).max(1.0);
                fill_polygon_shaded(overlay, &poly, |_x, y| {
                    let f = ((y - top) / span).clamp(0.0, 1.0);
                    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * f).round() as u8;
                    Rgba {
                        r: lerp(fill.r, to.r),
                        g: lerp(fill.g, to.g),
                        b: lerp(fill.b, to.b),
                        a: (lerp(fill.a, to.a) as f32 * base_a).round() as u8,
                    }
                });
            }
        }
    }

    // 3. Portrait placeholder slot (narrows the content rect on its side).
    let mut content_left = left + win.padding.left;
    let mut content_right = right - win.padding.right;
    if win.portrait.side != PortraitSide::None {
        let pw = win.portrait.width_px.max(1.0);
        let pm = win.portrait.margin_px.max(0.0);
        let (px0, px1) = match win.portrait.side {
            PortraitSide::Left => (left + pm, left + pm + pw),
            PortraitSide::Right => (right - pm - pw, right - pm),
            PortraitSide::None => (0.0, 0.0),
        };
        let prect = vec![
            (px0, top + pm),
            (px1, top + pm),
            (px1, bottom - pm),
            (px0, bottom - pm),
        ];
        if let Some(pf) = win.portrait.fill {
            fill_polygon(overlay, &prect, pf);
        }
        if win.portrait.outline.width_px > 0.0 {
            stroke_polygon(overlay, &prect, &win.portrait.outline);
        }
        match win.portrait.side {
            PortraitSide::Left => content_left = (px1 + pm).max(content_left),
            PortraitSide::Right => content_right = (px0 - pm).min(content_right),
            PortraitSide::None => {}
        }
    }

    // 4. Frame.
    match win.frame {
        FrameStyle::None => {}
        FrameStyle::SolidRounded => {
            if win.outline.width_px > 0.0 {
                stroke_polygon(overlay, &poly, &win.outline);
            }
        }
        FrameStyle::DoubleLine => {
            if win.outline.width_px > 0.0 {
                stroke_polygon(overlay, &poly, &win.outline);
                let gap = win.frame_gap_px.max(1.0);
                let inner = BubbleShape::RoundRect {
                    half_w: (hw - gap).max(1.0),
                    half_h: (hh - gap).max(1.0),
                    corner_px: (corner - gap).max(0.0),
                };
                stroke_polygon(overlay, &tessellate_bubble(&inner, pivot), &win.outline);
            }
        }
    }

    // 5. Name plate (may push the body text down for inline/boxed modes).
    let mut content_top = top + win.padding.top;
    let content_bottom = bottom - win.padding.bottom;
    let np = &win.name_plate;
    if np.mode != NamePlateMode::None {
        if let Some(font) = fonts.get(&np.name.font_key) {
            let (nw, nh) = layout_text(&np.name, font).bounds;
            let pad = np.padding_px.max(0.0);
            let boxed = matches!(np.mode, NamePlateMode::Boxed | NamePlateMode::Above);
            let (plate_w, plate_h) = if boxed {
                (nw + pad * 2.0, nh + pad * 2.0)
            } else {
                (nw, nh)
            };
            let (plate_x, plate_y) = match np.mode {
                NamePlateMode::Above => (left + np.offset.0, top + np.offset.1 - plate_h),
                _ => (content_left + np.offset.0, content_top + np.offset.1),
            };
            let prect = vec![
                (plate_x, plate_y),
                (plate_x + plate_w, plate_y),
                (plate_x + plate_w, plate_y + plate_h),
                (plate_x, plate_y + plate_h),
            ];
            if boxed {
                if let Some(f) = np.fill {
                    fill_polygon(overlay, &prect, f);
                }
                if np.outline.width_px > 0.0 {
                    stroke_polygon(overlay, &prect, &np.outline);
                }
            }
            let tx = plate_x + if boxed { pad } else { 0.0 };
            let ty = plate_y + if boxed { pad } else { 0.0 };
            bake_text(overlay, &np.name, (tx, ty), fonts, false);
            if matches!(np.mode, NamePlateMode::Inline | NamePlateMode::Boxed) {
                content_top = (plate_y + plate_h + 4.0).max(content_top);
            }
        }
    }

    // 6. Body text, wrapped + clipped to the content rect.
    let body_rect = (content_left, content_top, content_right, content_bottom);
    let wrap = win.wrap && win.size_mode != SizeMode::AutoFitText;
    bake_text_in_rect(overlay, &win.text, body_rect, win.v_anchor, wrap, fonts);

    // 7. Continue indicator at the content's bottom-right. With `indicator_auto`
    // it appears only when the body text overflows (a "there's more" cue).
    let show_indicator = win.indicator != IndicatorKind::None
        && (!win.indicator_auto || body_overflows(&win.text, body_rect, wrap, fonts));
    if show_indicator {
        draw_window_indicator(
            overlay,
            win.indicator,
            content_right - 6.0,
            content_bottom - 6.0,
            win.text.color,
            win.text.size_px,
        );
    }
}

/// The body-text content rect (x0,y0,x1,y1) for a window centered on `pivot`,
/// after padding, the portrait slot, and an inline/boxed name plate. Mirrors the
/// rect `draw_message_window_parts` flows text into. Shared with overflow checks.
fn window_content_rect(
    win: &MessageWindowObject,
    fonts: &FontSet,
    hw: f32,
    hh: f32,
    pivot: (f32, f32),
) -> (f32, f32, f32, f32) {
    let (cx, cy) = pivot;
    let (left, top, right, bottom) = (cx - hw, cy - hh, cx + hw, cy + hh);
    let mut cl = left + win.padding.left;
    let mut cr = right - win.padding.right;
    let pm = win.portrait.margin_px.max(0.0);
    let pw = win.portrait.width_px.max(1.0);
    match win.portrait.side {
        PortraitSide::Left => cl = (left + pm + pw + pm).max(cl),
        PortraitSide::Right => cr = (right - pm - pw - pm).min(cr),
        PortraitSide::None => {}
    }
    let ct = top + win.padding.top + name_plate_content_offset(win, fonts);
    let cb = bottom - win.padding.bottom;
    (cl, ct, cr, cb)
}

/// True if `block` (wrapped to `rect` when `wrap`) is taller (horizontal) /
/// wider (vertical) than the content rect — i.e. the text overflows.
fn body_overflows(
    block: &TextBlock,
    rect: (f32, f32, f32, f32),
    wrap: bool,
    fonts: &FontSet,
) -> bool {
    if block.text.is_empty() {
        return false;
    }
    let Some(font) = fonts.get(&block.font_key) else {
        return false;
    };
    let cw = (rect.2 - rect.0).max(1.0);
    let ch = (rect.3 - rect.1).max(1.0);
    let wrap_axis = if wrap {
        match block.orientation {
            Orientation::Horizontal => Some(cw),
            Orientation::Vertical => Some(ch),
        }
    } else {
        None
    };
    let layout = layout_text_wrapped(block, font, wrap_axis);
    // Check BOTH axes: the wrap axis is bounded by construction, but the other
    // axis (or both, when wrap is off) can still exceed the content rect.
    let (lw, lh) = layout.bounds;
    lw > cw + 0.5 || lh > ch + 0.5
}

/// Whether a window's body text overflows its content rect (used by the lab to
/// flag the text field). `AutoFitText` windows grow to fit, so never overflow.
pub fn message_window_overflows(win: &MessageWindowObject, fonts: &FontSet) -> bool {
    if matches!(win.size_mode, SizeMode::AutoFitText) {
        return false;
    }
    let (hw, hh) = effective_window_half_extents(win, fonts);
    // Content-rect DIMENSIONS are pivot-independent, so origin is fine here.
    let rect = window_content_rect(win, fonts, hw, hh, (0.0, 0.0));
    body_overflows(&win.text, rect, win.wrap, fonts)
}

/// Draw the "continue / next" indicator as a baked polygon (font-independent).
fn draw_window_indicator(
    overlay: &mut RgbaOverlay,
    kind: IndicatorKind,
    x: f32,
    y: f32,
    color: Rgba,
    size: f32,
) {
    let s = (size * 0.4).clamp(8.0, 28.0);
    match kind {
        IndicatorKind::None => {}
        IndicatorKind::Triangle => {
            let poly = vec![(x - s * 0.5, y - s), (x + s * 0.5, y - s), (x, y)];
            fill_polygon(overlay, &poly, color);
        }
        IndicatorKind::Diamond => {
            let poly = vec![
                (x, y - s),
                (x + s * 0.5, y - s * 0.5),
                (x, y),
                (x - s * 0.5, y - s * 0.5),
            ];
            fill_polygon(overlay, &poly, color);
        }
        IndicatorKind::Chevron => {
            let stroke = StrokeStyle {
                color,
                width_px: (s * 0.18).max(2.0),
            };
            stroke_segment(overlay, (x - s * 0.5, y - s), (x, y), &stroke);
            stroke_segment(overlay, (x, y), (x + s * 0.5, y - s), &stroke);
        }
        IndicatorKind::Dots => {
            for i in 0..3 {
                let dx = x - s + i as f32 * s * 0.7;
                let poly = circle_poly(dx, y - s * 0.4, (s * 0.16).max(2.0), 12);
                fill_polygon(overlay, &poly, color);
            }
        }
    }
}

/// Scanline-fill a polygon, calling `shade(x, y)` per covered pixel for the
/// blend color (alpha via the returned `Rgba::a`). Used for gradient / scrim
/// fills so they share the exact polygon edge with the frame stroke.
fn fill_polygon_shaded(
    overlay: &mut RgbaOverlay,
    poly: &[(f32, f32)],
    shade: impl Fn(f32, f32) -> Rgba,
) {
    if poly.len() < 3 {
        return;
    }
    let min_y = poly.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let max_y = poly.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
    let y0 = (min_y.floor() as i32).max(0);
    let y1 = (max_y.ceil() as i32).min(overlay.h as i32 - 1);
    let n = poly.len();
    for y in y0..=y1 {
        let yc = y as f32 + 0.5;
        let mut xs: Vec<f32> = Vec::new();
        for i in 0..n {
            let (ax, ay) = poly[i];
            let (bx, by) = poly[(i + 1) % n];
            if (ay <= yc && by > yc) || (by <= yc && ay > yc) {
                let t = (yc - ay) / (by - ay);
                xs.push(ax + t * (bx - ax));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut i = 0;
        while i + 1 < xs.len() {
            let xa = xs[i].ceil() as i32;
            let xb = xs[i + 1].floor() as i32;
            for x in xa..=xb {
                let col = shade(x as f32 + 0.5, yc);
                overlay.blend_px(x, y, col, 1.0);
            }
            i += 2;
        }
    }
}

/// Scanline polygon fill (even-odd) with no anti-aliasing on edges (fast,
/// adequate for opaque bubble interiors).
fn fill_polygon(overlay: &mut RgbaOverlay, poly: &[(f32, f32)], color: Rgba) {
    if poly.len() < 3 || color.a == 0 {
        return;
    }
    let min_y = poly.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let max_y = poly.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
    let y0 = (min_y.floor() as i32).max(0);
    let y1 = (max_y.ceil() as i32).min(overlay.h as i32 - 1);
    for y in y0..=y1 {
        let yc = y as f32 + 0.5;
        let mut xs: Vec<f32> = Vec::new();
        let n = poly.len();
        for i in 0..n {
            let (ax, ay) = poly[i];
            let (bx, by) = poly[(i + 1) % n];
            if (ay <= yc && by > yc) || (by <= yc && ay > yc) {
                let t = (yc - ay) / (by - ay);
                xs.push(ax + t * (bx - ax));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut i = 0;
        while i + 1 < xs.len() {
            let xa = xs[i].ceil() as i32;
            let xb = xs[i + 1].floor() as i32;
            for x in xa..=xb {
                overlay.blend_px(x, y, color, 1.0);
            }
            i += 2;
        }
    }
}

fn fill_polygon_clipped(
    overlay: &mut RgbaOverlay,
    poly: &[(f32, f32)],
    color: Rgba,
    clip: Option<(f32, f32, f32, f32)>,
) {
    if poly.len() < 3 || color.a == 0 {
        return;
    }
    let min_y = poly.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let max_y = poly.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
    let mut y0 = (min_y.floor() as i32).max(0);
    let mut y1 = (max_y.ceil() as i32).min(overlay.h as i32 - 1);
    if let Some((_, cy0, _, cy1)) = clip {
        y0 = y0.max(cy0.floor() as i32);
        y1 = y1.min(cy1.ceil() as i32);
    }
    for y in y0..=y1 {
        let yc = y as f32 + 0.5;
        let mut xs: Vec<f32> = Vec::new();
        let n = poly.len();
        for i in 0..n {
            let (ax, ay) = poly[i];
            let (bx, by) = poly[(i + 1) % n];
            if (ay <= yc && by > yc) || (by <= yc && ay > yc) {
                let t = (yc - ay) / (by - ay);
                xs.push(ax + t * (bx - ax));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut i = 0;
        while i + 1 < xs.len() {
            let mut xa = xs[i].ceil() as i32;
            let mut xb = xs[i + 1].floor() as i32;
            if let Some((cx0, _, cx1, _)) = clip {
                xa = xa.max(cx0.floor() as i32);
                xb = xb.min(cx1.ceil() as i32);
            }
            for x in xa..=xb {
                overlay.blend_px(x, y, color, 1.0);
            }
            i += 2;
        }
    }
}

/// Stroke a closed polygon as a sequence of segments.
fn stroke_polygon(overlay: &mut RgbaOverlay, poly: &[(f32, f32)], stroke: &StrokeStyle) {
    let n = poly.len();
    for i in 0..n {
        stroke_segment(overlay, poly[i], poly[(i + 1) % n], stroke);
    }
}

/// Stroke a single segment by stamping square caps along it (thick line).
fn stroke_segment(overlay: &mut RgbaOverlay, a: (f32, f32), b: (f32, f32), stroke: &StrokeStyle) {
    if stroke.color.a == 0 || stroke.width_px <= 0.0 {
        return;
    }
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len = (dx * dx + dy * dy).sqrt();
    let steps = (len.ceil() as i32).max(1);
    let half = (stroke.width_px * 0.5).max(0.5);
    let r = half.ceil() as i32;
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        let cx = a.0 + dx * t;
        let cy = a.1 + dy * t;
        let cxi = cx.round() as i32;
        let cyi = cy.round() as i32;
        for oy in -r..=r {
            for ox in -r..=r {
                if (ox * ox + oy * oy) as f32 <= half * half + 0.25 {
                    overlay.blend_px(cxi + ox, cyi + oy, stroke.color, 1.0);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BubbleObject, BubbleShape};

    const FONT_CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
        r"C:\Windows\Fonts\YuGothR.ttc",
    ];

    fn load_test_font() -> Option<LoadedFont> {
        for path in FONT_CANDIDATES {
            if let Ok(bytes) = std::fs::read(path) {
                if let Ok(font) = LoadedFont::from_bytes("test", bytes) {
                    return Some(font);
                }
            }
        }
        None
    }

    #[test]
    fn empty_objects_make_transparent_overlay() {
        let fonts = FontSet::new();
        let ov = bake_overlay(&[], 8, 8, &fonts);
        assert_eq!(ov.w, 8);
        assert_eq!(ov.h, 8);
        assert!(ov.pixels.iter().all(|&p| p == 0));
    }

    #[test]
    fn bubble_fill_writes_pixels() {
        let fonts = FontSet::new();
        let mut b = BubbleObject::default();
        b.shape = BubbleShape::Ellipse { rx: 20.0, ry: 12.0 };
        b.fill = Some(Rgba::WHITE);
        b.text = TextBlock::default(); // empty text -> no glyphs
        let obj = AnnotationObject::new_bubble(1, (32.0, 24.0), b);
        let ov = bake_overlay(&[obj], 64, 48, &fonts);
        let any_opaque = ov.pixels.chunks_exact(4).any(|p| p[3] > 0);
        assert!(any_opaque, "bubble fill should write some opaque pixels");
    }

    #[test]
    fn standalone_text_rotation_pivot_is_layout_center() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows Japanese test font");
            return;
        };
        let mut fonts = FontSet::new();
        fonts.insert(font);
        let text = TextBlock {
            text: "テキスト".to_string(),
            font_key: "test".to_string(),
            size_px: 42.0,
            ..TextBlock::default()
        };
        let font = fonts.get("test").expect("registered font");
        let (w, h) = layout_text(&text, font).bounds;
        let obj = AnnotationObject::new_text(1, (30.0, 40.0), text);
        let pivot = object_rotation_pivot(&obj, &fonts);
        assert!((pivot.0 - (30.0 + w * 0.5)).abs() < 0.01);
        assert!((pivot.1 - (40.0 + h * 0.5)).abs() < 0.01);
    }

    #[test]
    fn text_effects_expand_baked_coverage() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows Japanese test font");
            return;
        };
        let mut fonts = FontSet::new();
        fonts.insert(font);
        let base = TextBlock {
            text: "A".to_string(),
            font_key: "test".to_string(),
            size_px: 72.0,
            color: Rgba::BLACK,
            ..TextBlock::default()
        };
        let mut fx = base.clone();
        fx.extra_outlines.push(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 5.0,
        });
        fx.shadow = Some(crate::model::TextShadowStyle {
            color: Rgba::new(0, 0, 0, 160),
            offset: (8.0, 7.0),
            blur_px: 6.0,
            spread_px: 2.0,
        });
        fx.glow = Some(crate::model::TextGlowStyle {
            color: Rgba::new(80, 220, 255, 170),
            radius_px: 10.0,
            spread_px: 2.0,
        });
        fx.background = Some(crate::model::TextBackgroundStyle {
            fill: Rgba::new(0, 0, 0, 90),
            padding_px: 12.0,
            corner_px: 4.0,
        });
        fx.echo = Some(crate::model::TextEchoStyle {
            color: Rgba::new(40, 80, 220, 120),
            offset: (5.0, 4.0),
            count: 3,
        });
        let plain = bake_overlay(
            &[AnnotationObject::new_text(1, (40.0, 40.0), base)],
            180,
            160,
            &fonts,
        );
        let effect = bake_overlay(
            &[AnnotationObject::new_text(1, (40.0, 40.0), fx)],
            180,
            160,
            &fonts,
        );
        let alpha_count = |ov: &RgbaOverlay| ov.pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(
            alpha_count(&effect) > alpha_count(&plain),
            "effects should add visible pixels beyond the plain glyph"
        );
    }

    #[test]
    fn hollow_text_outline_bakes_with_transparent_fill() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows Japanese test font");
            return;
        };
        let mut fonts = FontSet::new();
        fonts.insert(font);
        let text = TextBlock {
            text: "A".to_string(),
            font_key: "test".to_string(),
            size_px: 72.0,
            color: Rgba::TRANSPARENT,
            outline: Some(StrokeStyle {
                color: Rgba::WHITE,
                width_px: 4.0,
            }),
            extra_outlines: vec![StrokeStyle {
                color: Rgba::BLACK,
                width_px: 7.0,
            }],
            ..TextBlock::default()
        };
        let ov = bake_overlay(
            &[AnnotationObject::new_text(1, (40.0, 40.0), text)],
            180,
            160,
            &fonts,
        );
        assert!(
            ov.pixels.chunks_exact(4).any(|p| p[3] > 0),
            "hollow text should still draw its outlines"
        );
    }

    #[test]
    fn line_field_shapes_bake_pixels_with_clear_center() {
        let fonts = FontSet::new();
        for shape in [
            BubbleShape::MotionLines {
                rx: 90.0,
                ry: 70.0,
                count: 48,
                shape_seed: 1,
            },
            BubbleShape::SpeedLines {
                half_w: 90.0,
                half_h: 70.0,
                dir_rad: 0.0,
                count: 40,
                shape_seed: 1,
            },
        ] {
            let mut b = BubbleObject::default();
            b.shape = shape;
            b.fill = None;
            b.outline = StrokeStyle {
                color: Rgba::BLACK,
                width_px: 3.0,
            };
            b.text = TextBlock::default();
            let obj = AnnotationObject::new_bubble(1, (110.0, 110.0), b);
            let ov = bake_overlay(&[obj], 220, 220, &fonts);
            // Lines are drawn (opaque pixels exist).
            assert!(
                ov.pixels.chunks_exact(4).any(|p| p[3] > 0),
                "line field should draw lines: {shape:?}"
            );
            // The clear central ellipse (≈55%) stays empty: the exact pivot pixel
            // has no line through it.
            let i = (110 * 220 + 110) * 4;
            assert_eq!(
                ov.pixels[i + 3],
                0,
                "line-field center must be clear: {shape:?}"
            );
        }
    }

    #[test]
    fn speed_lines_stay_within_outer_ellipse_diagonal() {
        // Regression (Codex P1): a diagonal 流線 must not escape the outer ellipse
        // (= the AABB / hit-test / rotated-bake bound). Every opaque pixel must lie
        // within the outer ellipse + a small margin for line width.
        let fonts = FontSet::new();
        let (cx, cy) = (130.0f32, 130.0f32);
        let (hw, hh) = (100.0f32, 70.0f32);
        let mut b = BubbleObject::default();
        b.shape = BubbleShape::SpeedLines {
            half_w: hw,
            half_h: hh,
            dir_rad: std::f32::consts::FRAC_PI_4, // 45° diagonal
            count: 60,
            shape_seed: 7,
        };
        b.fill = None;
        b.outline = StrokeStyle {
            color: Rgba::BLACK,
            width_px: 3.0,
        };
        b.text = TextBlock::default();
        let obj = AnnotationObject::new_bubble(1, (cx, cy), b);
        let ov = bake_overlay(&[obj], 260, 260, &fonts);
        let m = 6.0; // line half-width + AA slack
        for y in 0..ov.h {
            for x in 0..ov.w {
                if ov.pixels[(y * ov.w + x) * 4 + 3] == 0 {
                    continue;
                }
                let nx = (x as f32 - cx) / (hw + m);
                let ny = (y as f32 - cy) / (hh + m);
                assert!(
                    nx * nx + ny * ny <= 1.05,
                    "speed line escaped outer ellipse at ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn text_only_with_empty_text_draws_nothing() {
        // なし must not draw any box: with empty text and a fill/outline set, the
        // overlay stays fully transparent (proves fill + stroke are skipped).
        let fonts = FontSet::new();
        let mut b = BubbleObject::default();
        b.shape = BubbleShape::TextOnly {
            half_w: 60.0,
            half_h: 40.0,
        };
        b.fill = Some(Rgba::WHITE);
        b.outline = StrokeStyle {
            color: Rgba::BLACK,
            width_px: 4.0,
        };
        b.auto_size = false;
        b.text = TextBlock::default(); // empty
        let obj = AnnotationObject::new_bubble(1, (100.0, 100.0), b);
        let ov = bake_overlay(&[obj], 200, 200, &fonts);
        assert!(
            ov.pixels.iter().all(|&p| p == 0),
            "text-only with no text must draw nothing (no box)"
        );
    }

    #[test]
    fn concentration_fills_center_and_fades_outside() {
        let fonts = FontSet::new();
        let (cx, cy) = (110.0f32, 110.0f32);
        let (rx, ry) = (80.0f32, 60.0f32);
        let mut b = BubbleObject::default();
        b.shape = BubbleShape::Concentration {
            rx,
            ry,
            shape_seed: 2,
        };
        b.fill = Some(Rgba::WHITE);
        b.outline = StrokeStyle {
            color: Rgba::BLACK,
            width_px: 3.0,
        };
        b.auto_size = false;
        b.text = TextBlock::default();
        let obj = AnnotationObject::new_bubble(1, (cx, cy), b);
        let ov = bake_overlay(&[obj], 220, 220, &fonts);
        let alpha = |x: usize, y: usize| ov.pixels[(y * ov.w + x) * 4 + 3];
        // Center is opaque (solid fill region).
        assert!(
            alpha(cx as usize, cy as usize) > 200,
            "concentration center should be opaque"
        );
        // A point well outside the ellipse is transparent.
        assert_eq!(alpha(5, 5), 0, "outside the ellipse must be clear");
        // The fill feathers: a pixel just inside the rim is less opaque than the
        // center (soft edge, not a hard cut).
        let near_rim = alpha((cx + rx * 0.97) as usize, cy as usize);
        let center = alpha(cx as usize, cy as usize);
        assert!(
            near_rim < center,
            "rim ({near_rim}) should be softer than center ({center})"
        );
    }

    #[test]
    fn tailless_shape_does_not_render_a_set_tail() {
        // A Concentration with a far-away tail must NOT draw the tail (the shape is
        // edgeless); pixels near the tail tip stay clear.
        let fonts = FontSet::new();
        let mut b = BubbleObject::default();
        b.shape = BubbleShape::Concentration {
            rx: 50.0,
            ry: 40.0,
            shape_seed: 1,
        };
        b.fill = Some(Rgba::WHITE);
        b.outline = StrokeStyle {
            color: Rgba::BLACK,
            width_px: 3.0,
        };
        b.auto_size = false;
        b.text = TextBlock::default();
        b.tail = Some(crate::model::Tail {
            tip: (100.0, 230.0), // far below the ellipse
            base_t: 0.25,
            base_auto: true,
            width_px: 40.0,
            kind: crate::model::TailKind::Spike,
        });
        let obj = AnnotationObject::new_bubble(1, (100.0, 100.0), b);
        let ov = bake_overlay(&[obj], 200, 260, &fonts);
        // The whole lower region toward the tail tip must be clear (no tail spike).
        for y in 160..260 {
            for x in 0..200 {
                assert_eq!(
                    ov.pixels[(y * 200 + x) * 4 + 3],
                    0,
                    "tailless shape drew a tail at ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn concentration_with_merge_flag_bakes_standalone() {
        // Regression (Codex P2): a Concentration must not join a merge chain (its
        // feather can't fill/erase). Even with merge_with_below set above a normal
        // bubble, its half-alpha fill must stay ~half (not forced opaque by the
        // merge erase pass).
        let fonts = FontSet::new();
        let mut lower = BubbleObject::default();
        lower.shape = BubbleShape::Ellipse { rx: 40.0, ry: 30.0 };
        lower.fill = Some(Rgba::WHITE);
        lower.auto_size = false;
        lower.text = TextBlock::default();
        let lower = {
            let mut o = AnnotationObject::new_bubble(1, (70.0, 90.0), lower);
            o.z = 0;
            o
        };
        let mut upper = BubbleObject::default();
        upper.shape = BubbleShape::Concentration {
            rx: 50.0,
            ry: 40.0,
            shape_seed: 0,
        };
        upper.fill = Some(Rgba::new(255, 255, 255, 128));
        upper.outline.width_px = 0.0;
        upper.fill_opacity = 1.0;
        upper.auto_size = false;
        upper.merge_with_below = true; // would force-opaque if it merged
        upper.text = TextBlock::default();
        let upper = {
            let mut o = AnnotationObject::new_bubble(2, (150.0, 90.0), upper);
            o.z = 1;
            o
        };
        let ov = bake_overlay(&[lower, upper], 240, 180, &fonts);
        // Concentration center (150,90) should remain ~half-alpha (baked standalone).
        let a = ov.pixels[(90 * 240 + 150) * 4 + 3];
        assert!(
            (110..=140).contains(&a),
            "merged-flag concentration center should stay ~128, got {a}"
        );
    }

    #[test]
    fn concentration_translucent_fill_respects_alpha() {
        // Regression (Codex P2): the fill alpha must not be double-applied. A
        // half-alpha fill should yield a center alpha near 128, not ~64.
        let fonts = FontSet::new();
        let mut b = BubbleObject::default();
        b.shape = BubbleShape::Concentration {
            rx: 60.0,
            ry: 50.0,
            shape_seed: 0,
        };
        b.fill = Some(Rgba::new(255, 255, 255, 128));
        b.outline = StrokeStyle {
            color: Rgba::BLACK,
            width_px: 0.0, // no ring, isolate the fill
        };
        b.fill_opacity = 1.0;
        b.auto_size = false;
        b.text = TextBlock::default();
        let obj = AnnotationObject::new_bubble(1, (90.0, 80.0), b);
        let ov = bake_overlay(&[obj], 180, 160, &fonts);
        let center = ov.pixels[(80 * 180 + 90) * 4 + 3];
        assert!(
            (110..=140).contains(&center),
            "half-alpha fill center should be ~128, got {center}"
        );
    }

    #[test]
    fn double_stroke_draws_more_than_single() {
        let fonts = FontSet::new();
        let count_opaque = |shape: BubbleShape| {
            let mut b = BubbleObject::default();
            b.shape = shape;
            b.fill = None; // isolate stroke pixels
            b.outline = StrokeStyle {
                color: Rgba::BLACK,
                width_px: 3.0,
            };
            b.auto_size = false;
            b.text = TextBlock::default();
            let obj = AnnotationObject::new_bubble(1, (130.0, 100.0), b);
            let ov = bake_overlay(&[obj], 260, 200, &fonts);
            ov.pixels.chunks_exact(4).filter(|p| p[3] > 0).count()
        };
        let single = count_opaque(BubbleShape::RoundRect {
            half_w: 90.0,
            half_h: 60.0,
            corner_px: 20.0,
        });
        let double = count_opaque(BubbleShape::DoubleStroke {
            half_w: 90.0,
            half_h: 60.0,
            corner_px: 20.0,
            gap_px: 10.0,
        });
        assert!(single > 0, "single rounded rect should stroke");
        assert!(
            double > single,
            "double-stroke ({double}) should draw more than single ({single})"
        );
    }

    #[test]
    fn sketch_strokes_bake_pixels_within_box() {
        let fonts = FontSet::new();
        let (cx, cy) = (130.0f32, 110.0f32);
        let (hw, hh) = (90.0f32, 60.0f32);
        let mut b = BubbleObject::default();
        b.shape = BubbleShape::Strokes {
            half_w: hw,
            half_h: hh,
            corner_px: 24.0,
            shape_seed: 4,
        };
        b.fill = None;
        b.outline = StrokeStyle {
            color: Rgba::BLACK,
            width_px: 3.0,
        };
        b.auto_size = false;
        b.text = TextBlock::default();
        let obj = AnnotationObject::new_bubble(1, (cx, cy), b);
        let ov = bake_overlay(&[obj], 260, 220, &fonts);
        let mut any = false;
        let m = 10.0; // sketch jitter + line half-width slack
        for y in 0..ov.h {
            for x in 0..ov.w {
                if ov.pixels[(y * ov.w + x) * 4 + 3] == 0 {
                    continue;
                }
                any = true;
                assert!(
                    (x as f32) >= cx - hw - m
                        && (x as f32) <= cx + hw + m
                        && (y as f32) >= cy - hh - m
                        && (y as f32) <= cy + hh + m,
                    "sketch stroke escaped the box at ({x},{y})"
                );
            }
        }
        assert!(any, "sketch outline should draw pixels");
    }

    /// A solid `n×n` straight-alpha RGBA stamp image in `color`.
    fn solid_stamp_img(n: usize, color: Rgba) -> RgbaOverlay {
        let mut img = RgbaOverlay::new(n, n);
        for p in img.pixels.chunks_exact_mut(4) {
            p[0] = color.r;
            p[1] = color.g;
            p[2] = color.b;
            p[3] = color.a;
        }
        img
    }

    fn stamp_obj(id: u64, half: f32, src: crate::model::StampSource) -> AnnotationObject {
        let stamp = StampObject {
            source: src,
            half_w: half,
            half_h: half,
            ..StampObject::default()
        };
        AnnotationObject::new_stamp(id, (50.0, 50.0), stamp)
    }

    #[test]
    fn stamp_bakes_image_pixels() {
        let fonts = FontSet::new();
        let obj = stamp_obj(7, 16.0, crate::model::StampSource::Emoji("x".into()));
        let mut stamps = StampImages::new();
        stamps.insert(
            7,
            std::sync::Arc::new(solid_stamp_img(32, Rgba::new(220, 30, 40, 255))),
        );
        let ov = bake_overlay_with_stamps(&[obj], 100, 100, &fonts, &stamps);
        // Center pixel should be opaque-ish red.
        let i = (50 * 100 + 50) * 4;
        assert!(ov.pixels[i + 3] > 200, "stamp center should be opaque");
        assert!(
            ov.pixels[i] > 150 && ov.pixels[i + 1] < 90,
            "stamp center should be reddish"
        );
    }

    #[test]
    fn stamp_outline_widens_alpha() {
        let fonts = FontSet::new();
        let count_opaque = |outline: Option<StrokeStyle>| {
            let mut stamp = StampObject {
                source: crate::model::StampSource::Emoji("x".into()),
                half_w: 16.0,
                half_h: 16.0,
                ..StampObject::default()
            };
            stamp.outline = outline;
            let obj = AnnotationObject::new_stamp(7, (50.0, 50.0), stamp);
            let mut stamps = StampImages::new();
            stamps.insert(
                7,
                std::sync::Arc::new(solid_stamp_img(32, Rgba::new(220, 30, 40, 255))),
            );
            let ov = bake_overlay_with_stamps(&[obj], 100, 100, &fonts, &stamps);
            ov.pixels.chunks_exact(4).filter(|p| p[3] > 0).count()
        };
        let plain = count_opaque(None);
        let outlined = count_opaque(Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 5.0,
        }));
        assert!(
            outlined > plain,
            "sticker outline should add opaque pixels: {outlined} vs {plain}"
        );
    }

    #[test]
    fn sticker_matches_cpu_bake() {
        // The GPU sticker pre-composite (composite_stamp_sticker) must produce the
        // SAME pixels as the CPU bake (draw_stamp) for an outlined stamp at opacity
        // 1 / no rotation, so duplicates rendered as GPU quads look identical to the
        // bake. Guards the shared-helper refactor (resolve_stamp_buf +
        // sticker_halo_coverage) against drift.
        let fonts = FontSet::new();
        let img = solid_stamp_img(32, Rgba::new(220, 30, 40, 255));
        let outline = StrokeStyle {
            color: Rgba::WHITE,
            width_px: 5.0,
        };
        let pivot = (50.0f32, 50.0f32);
        let half = 16.0f32;
        // CPU bake on a transparent 100×100 canvas (large enough: no clamping).
        let obj = AnnotationObject::new_stamp(
            7,
            pivot,
            StampObject {
                source: crate::model::StampSource::Emoji("x".into()),
                half_w: half,
                half_h: half,
                outline: Some(outline),
                ..StampObject::default()
            },
        );
        let mut stamps = StampImages::new();
        stamps.insert(7, std::sync::Arc::new(img.clone()));
        let ov = bake_overlay_with_stamps(&[obj], 100, 100, &fonts, &stamps);

        // Standalone sticker (= what the GPU quad uploads).
        let tw = (half * 2.0) as usize; // 32
        let th = tw;
        let (st, rad) = composite_stamp_sticker(&img, false, false, tw, th, Some(outline));
        assert_eq!(rad, 5, "rad should follow the 5px outline");
        assert_eq!((st.w, st.h), (tw + 2 * rad, th + 2 * rad));

        // Sticker pixel (sx,sy) maps to canvas (left - rad + sx, top - rad + sy).
        let left = (pivot.0 - tw as f32 * 0.5) as i32; // 34
        let top = (pivot.1 - th as f32 * 0.5) as i32; // 34
        let mut compared = 0u32;
        for sy in 0..st.h {
            for sx in 0..st.w {
                let cx = left - rad as i32 + sx as i32;
                let cy = top - rad as i32 + sy as i32;
                if cx < 0 || cy < 0 || cx >= 100 || cy >= 100 {
                    continue;
                }
                let si = (sy * st.w + sx) * 4;
                let ci = (cy as usize * 100 + cx as usize) * 4;
                for k in 0..4 {
                    let a = st.pixels[si + k] as i32;
                    let b = ov.pixels[ci + k] as i32;
                    assert!(
                        (a - b).abs() <= 1,
                        "sticker vs CPU bake mismatch at sticker({sx},{sy}) ch{k}: {a} vs {b}"
                    );
                }
                compared += 1;
            }
        }
        assert!(compared > 1000, "should compare the overlapping footprint");

        // Opacity < 1 must fade the composited sticker AS ONE UNIT (the GPU quad
        // tints the whole texture; the CPU path now blits with the same opacity).
        // A fully-opaque interior pixel should land at ~opacity·255 over the
        // transparent canvas.
        let faded = AnnotationObject::new_stamp(
            7,
            pivot,
            StampObject {
                source: crate::model::StampSource::Emoji("x".into()),
                half_w: half,
                half_h: half,
                opacity: 0.5,
                outline: Some(outline),
                ..StampObject::default()
            },
        );
        let ov2 = bake_overlay_with_stamps(&[faded], 100, 100, &fonts, &stamps);
        let center = (50usize * 100 + 50) * 4;
        let a = ov2.pixels[center + 3] as i32;
        assert!(
            (a - 128).abs() <= 4,
            "opacity 0.5 interior alpha should be ~128, got {a}"
        );
    }

    #[test]
    fn stamp_missing_image_draws_placeholder() {
        let fonts = FontSet::new();
        // No entry in the stamps map -> placeholder must still write pixels.
        let obj = stamp_obj(7, 20.0, crate::model::StampSource::File("nope.png".into()));
        let ov = bake_overlay_with_stamps(&[obj], 100, 100, &fonts, &StampImages::new());
        assert!(
            ov.pixels.chunks_exact(4).any(|p| p[3] > 0),
            "missing stamp should draw a placeholder"
        );
    }

    #[test]
    fn stamp_rotation_bakes_without_panic() {
        let fonts = FontSet::new();
        let mut obj = stamp_obj(7, 16.0, crate::model::StampSource::Emoji("x".into()));
        obj.rotation_rad = 0.5;
        let mut stamps = StampImages::new();
        stamps.insert(
            7,
            std::sync::Arc::new(solid_stamp_img(32, Rgba::new(20, 200, 60, 255))),
        );
        let ov = bake_overlay_with_stamps(&[obj], 120, 120, &fonts, &stamps);
        assert!(
            ov.pixels.chunks_exact(4).any(|p| p[3] > 0),
            "rotated stamp should bake pixels"
        );
    }

    #[test]
    fn merge_erases_interior_outline() {
        let fonts = FontSet::new();
        let mk = |x: f32, id: u64, z: i32, merge: bool| {
            let mut b = BubbleObject::default();
            b.shape = BubbleShape::Ellipse { rx: 35.0, ry: 22.0 };
            b.fill = Some(Rgba::WHITE);
            b.outline = crate::model::StrokeStyle {
                color: Rgba::BLACK,
                width_px: 3.0,
            };
            b.text = TextBlock::default();
            b.auto_size = false;
            b.merge_with_below = merge;
            let mut o = AnnotationObject::new_bubble(id, (x, 60.0), b);
            o.z = z;
            o
        };
        let bake = |merge: bool| {
            // a (lower) and bb (upper) overlap; bb optionally merges down.
            let a = mk(70.0, 1, 0, false);
            let bb = mk(110.0, 2, 1, merge);
            bake_overlay(&[a, bb], 200, 120, &fonts)
        };
        let count_dark = |ov: &RgbaOverlay| {
            ov.pixels
                .chunks_exact(4)
                .filter(|p| p[3] > 200 && p[0] < 70 && p[1] < 70 && p[2] < 70)
                .count()
        };
        let unmerged = count_dark(&bake(false));
        let merged = count_dark(&bake(true));
        assert!(
            merged < unmerged,
            "merge should erase interior strokes (merged {merged} >= unmerged {unmerged})"
        );
    }

    #[test]
    fn merge_keeps_outline_width() {
        // A merged member's OUTER outline must keep ~its configured width. The
        // interior-erase pass (pass 3) wipes the inner half of a centered stroke,
        // so the stroke pass draws at MERGE_STROKE_SCALE (2×) to compensate.
        // Regression: merging used to halve the visible outline thickness.
        let fonts = FontSet::new();
        let mk = |x: f32, id: u64, z: i32, merge: bool| {
            let mut b = BubbleObject::default();
            b.shape = BubbleShape::RoundRect {
                half_w: 50.0,
                half_h: 34.0,
                corner_px: 14.0,
            };
            b.fill = Some(Rgba::WHITE);
            b.outline = StrokeStyle {
                color: Rgba::BLACK,
                width_px: 6.0,
            };
            b.text = TextBlock::default();
            b.auto_size = false;
            b.merge_with_below = merge;
            let mut o = AnnotationObject::new_bubble(id, (x, 80.0), b);
            o.z = z;
            o
        };
        // Count dark outline pixels only in the RIGHT bubble's region (x >= 220).
        let count_dark_right = |ov: &RgbaOverlay| {
            let mut n = 0usize;
            for y in 0..ov.h {
                for x in 220..ov.w {
                    let p = &ov.pixels[(y * ov.w + x) * 4..][..4];
                    if p[3] > 200 && p[0] < 70 && p[1] < 70 && p[2] < 70 {
                        n += 1;
                    }
                }
            }
            n
        };
        // Right bubble alone (single-object path, 1× stroke).
        let single = count_dark_right(&bake_overlay(&[mk(320.0, 1, 0, false)], 440, 160, &fonts));
        // Right bubble (z=1, merges down) grouped with a far-left bubble (z=0).
        // The outlines don't overlap, so the right bubble's outline is the only
        // dark ink in its region — its width should match the single case, NOT halve.
        let merged = count_dark_right(&bake_overlay(
            &[mk(100.0, 2, 0, false), mk(320.0, 1, 1, true)],
            440,
            160,
            &fonts,
        ));
        assert!(
            merged as f32 >= single as f32 * 0.7,
            "merged outline must not be ~halved (merged {merged} vs single {single})"
        );
        assert!(
            merged as f32 <= single as f32 * 1.6,
            "merged outline must stay near its configured width (merged {merged} vs single {single})"
        );
    }

    #[test]
    fn merge_works_for_rotated_members() {
        // Merge must also erase interior strokes when the members are rotated
        // (the chain composites each part through the rotation path).
        let fonts = FontSet::new();
        let mk = |x: f32, id: u64, z: i32, merge: bool, rot: f32| {
            let mut b = BubbleObject::default();
            b.shape = BubbleShape::Ellipse { rx: 35.0, ry: 22.0 };
            b.fill = Some(Rgba::WHITE);
            b.outline = crate::model::StrokeStyle {
                color: Rgba::BLACK,
                width_px: 3.0,
            };
            b.text = TextBlock::default();
            b.auto_size = false;
            b.merge_with_below = merge;
            let mut o = AnnotationObject::new_bubble(id, (x, 100.0), b);
            o.z = z;
            o.rotation_rad = rot;
            o
        };
        let bake = |merge: bool| {
            let rot = 0.3;
            let a = mk(95.0, 1, 0, false, rot);
            let bb = mk(135.0, 2, 1, merge, rot);
            bake_overlay(&[a, bb], 260, 200, &fonts)
        };
        let count_dark = |ov: &RgbaOverlay| {
            ov.pixels
                .chunks_exact(4)
                .filter(|p| p[3] > 200 && p[0] < 70 && p[1] < 70 && p[2] < 70)
                .count()
        };
        let unmerged = count_dark(&bake(false));
        let merged = count_dark(&bake(true));
        assert!(
            merged < unmerged,
            "rotated merge should erase interior strokes (merged {merged} >= unmerged {unmerged})"
        );
    }

    #[test]
    fn merge_thick_outline_rotated_not_clipped() {
        // A rotated, merged member with a THICK outline must not lose its (2×) outer
        // stroke to the rotated-bake temp buffer — object_local_aabb pads for
        // MERGE_STROKE_SCALE (+ SKETCH_MAX_JITTER for 線). At rotation 0 the bubble
        // draws directly (no buffer); a rotated bake with too-small a buffer would
        // clip the outer outline and drop a large fraction of the dark pixels.
        let fonts = FontSet::new();
        let mk = |shape: BubbleShape, x: f32, id: u64, z: i32, merge: bool, rot: f32| {
            let mut b = BubbleObject::default();
            b.shape = shape;
            b.fill = Some(Rgba::WHITE);
            b.outline = StrokeStyle {
                color: Rgba::BLACK,
                width_px: 16.0,
            };
            b.text = TextBlock::default();
            b.auto_size = false;
            b.merge_with_below = merge;
            let mut o = AnnotationObject::new_bubble(id, (x, 110.0), b);
            o.z = z;
            o.rotation_rad = rot;
            o
        };
        let count_dark = |ov: &RgbaOverlay| {
            ov.pixels
                .chunks_exact(4)
                .filter(|p| p[3] > 200 && p[0] < 70 && p[1] < 70 && p[2] < 70)
                .count()
        };
        // Both a solid ellipse and a 線 (sketch, +jitter) outline go through the 2×
        // merge stroke. Far-apart members (no overlap) so each full outline survives;
        // the top one (z=1) merges down → both bake through the merge path.
        for shape in [
            BubbleShape::Ellipse { rx: 40.0, ry: 26.0 },
            BubbleShape::Strokes {
                half_w: 40.0,
                half_h: 26.0,
                corner_px: 12.0,
                shape_seed: 3,
            },
        ] {
            let bake = |rot: f32| {
                let a = mk(shape, 80.0, 1, 0, false, rot);
                let bb = mk(shape, 230.0, 2, 1, true, rot);
                bake_overlay(&[a, bb], 320, 220, &fonts)
            };
            let flat = count_dark(&bake(0.0));
            let rotated = count_dark(&bake(0.5));
            assert!(
                flat > 0 && rotated > 0,
                "should bake outline pixels for {shape:?}"
            );
            assert!(
                rotated as f32 >= flat as f32 * 0.8,
                "rotated merged thick outline lost pixels (temp-buffer clip?) for \
                 {shape:?}: rotated {rotated} vs flat {flat}"
            );
        }
    }

    #[test]
    fn rotation_turns_wide_bubble_tall() {
        let fonts = FontSet::new();
        let make = |rot: f32| {
            let mut b = BubbleObject::default();
            b.shape = BubbleShape::Ellipse { rx: 40.0, ry: 10.0 };
            b.fill = Some(Rgba::WHITE);
            b.text = TextBlock::default();
            b.auto_size = false;
            let mut obj = AnnotationObject::new_bubble(1, (60.0, 60.0), b);
            obj.rotation_rad = rot;
            bake_overlay(&[obj], 120, 120, &fonts)
        };
        let bbox = |ov: &RgbaOverlay| {
            let (mut minx, mut miny, mut maxx, mut maxy) = (999i32, 999i32, -1i32, -1i32);
            for y in 0..ov.h {
                for x in 0..ov.w {
                    if ov.pixels[(y * ov.w + x) * 4 + 3] > 0 {
                        minx = minx.min(x as i32);
                        maxx = maxx.max(x as i32);
                        miny = miny.min(y as i32);
                        maxy = maxy.max(y as i32);
                    }
                }
            }
            (maxx - minx, maxy - miny)
        };
        let (w0, h0) = bbox(&make(0.0));
        assert!(w0 > h0, "unrotated ellipse is wider than tall ({w0}x{h0})");
        let (w1, h1) = bbox(&make(std::f32::consts::FRAC_PI_2));
        assert!(
            h1 > w1,
            "90deg-rotated ellipse is taller than wide ({w1}x{h1})"
        );
    }

    #[test]
    fn soap_bubble_is_denser_at_rim_than_center() {
        let mut ov = RgbaOverlay::new(80, 80);
        draw_soap_bubble(&mut ov, 40.0, 40.0, 30.0, Rgba::new(120, 180, 255, 255));
        let alpha = |x: usize, y: usize| ov.pixels[(y * 80 + x) * 4 + 3];
        let center = alpha(40, 40);
        let rim = alpha(64, 40); // ~0.8r to the right (away from the highlight)
        assert!(
            rim > center,
            "soap bubble rim alpha {rim} should exceed center {center}"
        );
    }

    #[test]
    fn decorations_with_styling_bake_without_panic() {
        use crate::model::{DecoKind, DecorationLayer};
        let fonts = FontSet::new();
        // Each kind exercised: outline disabled (sparkle), custom petals/center
        // (flower), and the translucent gradient (bubble).
        for (kind, outline_width, gradient) in [
            (DecoKind::Sparkle, 0.0, false),
            (DecoKind::Flower, 2.0, false),
            (DecoKind::Bubble, 0.0, true),
        ] {
            let mut b = BubbleObject::default();
            b.shape = BubbleShape::Ellipse { rx: 80.0, ry: 60.0 };
            b.fill = None; // isolate decoration pixels
            b.outline.width_px = 0.0;
            b.decorations.push(DecorationLayer {
                kind,
                density: 6.0,
                size_ratio: 0.3,
                outline_width,
                gradient,
                ..DecorationLayer::default()
            });
            let obj = AnnotationObject::new_bubble(1, (128.0, 100.0), b);
            let ov = bake_overlay(&[obj], 256, 200, &fonts);
            let any_opaque = ov.pixels.chunks_exact(4).any(|p| p[3] > 0);
            assert!(any_opaque, "decoration {kind:?} should write pixels");
        }
    }

    // ---- message window (font-free geometry tests) ----

    use crate::model::{
        FillMode, FrameStyle, MessageWindowObject, SizeMode, VAnchor, WindowPosition,
    };

    fn window(fill: FillMode, frame: FrameStyle) -> MessageWindowObject {
        MessageWindowObject {
            size_mode: SizeMode::Inset,
            position: WindowPosition::Free,
            half_w: 80.0,
            half_h: 40.0,
            corner_px: 8.0,
            fill_mode: fill,
            frame,
            text: TextBlock::default(), // no font registered -> no glyphs
            ..MessageWindowObject::default()
        }
    }

    #[test]
    fn message_window_solid_fill_writes_pixels() {
        let fonts = FontSet::new();
        let win = window(FillMode::Solid, FrameStyle::SolidRounded);
        let obj = AnnotationObject::new_message_window(1, (100.0, 60.0), win);
        let ov = bake_overlay(&[obj], 200, 120, &fonts);
        assert!(
            ov.pixels.chunks_exact(4).any(|p| p[3] > 0),
            "window fill should write opaque pixels"
        );
    }

    #[test]
    fn message_window_double_line_has_more_frame_than_solid() {
        let fonts = FontSet::new();
        let count_opaque = |frame: FrameStyle| {
            // No fill: only the frame stroke writes pixels.
            let win = window(FillMode::None, frame);
            let obj = AnnotationObject::new_message_window(1, (100.0, 60.0), win);
            let ov = bake_overlay(&[obj], 200, 120, &fonts);
            ov.pixels.chunks_exact(4).filter(|p| p[3] > 0).count()
        };
        let single = count_opaque(FrameStyle::SolidRounded);
        let double = count_opaque(FrameStyle::DoubleLine);
        assert!(single > 0, "single frame should draw");
        assert!(
            double > single,
            "double-line frame ({double}) should draw more than single ({single})"
        );
    }

    #[test]
    fn message_window_scrim_denser_at_dense_side() {
        let fonts = FontSet::new();
        let mut win = window(FillMode::GradientScrim, FrameStyle::None);
        win.fill = Some(Rgba::new(0, 0, 0, 255));
        win.scrim_dense_side = VAnchor::Bottom;
        let obj = AnnotationObject::new_message_window(1, (100.0, 60.0), win);
        let ov = bake_overlay(&[obj], 200, 120, &fonts);
        let alpha = |x: usize, y: usize| ov.pixels[(y * 200 + x) * 4 + 3];
        // Panel spans y in [20, 100]; sample near top vs near bottom (x in panel).
        let near_top = alpha(100, 28);
        let near_bottom = alpha(100, 92);
        assert!(
            near_bottom > near_top,
            "bottom-dense scrim: bottom alpha {near_bottom} should exceed top {near_top}"
        );
    }

    #[test]
    fn fullwidth_uses_stored_half_extents() {
        let fonts = FontSet::new();
        let mut win = window(FillMode::Solid, FrameStyle::SolidRounded);
        win.size_mode = SizeMode::FullWidth;
        win.half_w = 123.0;
        win.half_h = 45.0;
        let (hw, hh) = effective_window_half_extents(&win, &fonts);
        assert_eq!((hw, hh), (123.0, 45.0));
    }
}
