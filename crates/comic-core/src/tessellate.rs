//! Shape tessellation: bubble outlines and the straight tail triangle.
//!
//! Produces simple closed polygons (`Vec<(f32,f32)>`) in source-image space.
//! The rasterizer scanline-fills and strokes these. Coordinates are absolute
//! image coords (the lab passes the object pivot through).

use crate::model::{BubbleShape, DecoKind, DecoPlacement, DecorationLayer, Tail, TailKind};

/// Number of segments used to approximate an ellipse / rounded corners.
const ELLIPSE_SEGMENTS: usize = 64;
const CORNER_SEGMENTS: usize = 12;

/// Tessellate a bubble shape into a closed polygon centered at `pivot`.
pub fn tessellate_bubble(shape: &BubbleShape, pivot: (f32, f32)) -> Vec<(f32, f32)> {
    let (cx, cy) = pivot;
    match *shape {
        BubbleShape::Ellipse { rx, ry } => {
            let mut pts = Vec::with_capacity(ELLIPSE_SEGMENTS);
            for i in 0..ELLIPSE_SEGMENTS {
                let t = (i as f32) / (ELLIPSE_SEGMENTS as f32) * std::f32::consts::TAU;
                pts.push((cx + rx * t.cos(), cy + ry * t.sin()));
            }
            pts
        }
        BubbleShape::RoundRect {
            half_w,
            half_h,
            corner_px,
        } => round_rect(cx, cy, half_w, half_h, corner_px),
        BubbleShape::Burst {
            rx,
            ry,
            spikes,
            jag,
            shape_seed,
        } => burst(cx, cy, rx, ry, spikes, jag, shape_seed),
        BubbleShape::Cloud {
            rx,
            ry,
            lobes,
            amp,
            shape_seed,
        } => cloud(cx, cy, rx, ry, lobes, amp, shape_seed),
    }
}

/// Resize a bubble shape so its interior contains a text box of `text_w × text_h`
/// plus `padding` on every side, preserving the variant and its style params
/// (corner / spikes / jag / lobes / amp / seed). Used by auto-size.
///
/// Ellipses (and the elliptical base of burst/cloud) use a √2 factor so the
/// inscribed text rectangle's corners land on the ellipse. Burst valleys
/// (`jag`) and cloud indentation (`amp`) shrink the usable interior, so those
/// variants divide by the corresponding factor to keep the text inside.
pub fn fit_bubble_shape(
    shape: &BubbleShape,
    text_w: f32,
    text_h: f32,
    padding: f32,
) -> BubbleShape {
    const SQRT2: f32 = std::f32::consts::SQRT_2;
    let hw = (text_w * 0.5 + padding).max(8.0);
    let hh = (text_h * 0.5 + padding).max(8.0);
    // Keep auto-sized bubbles from becoming extremely tall/narrow (a single
    // vertical line) or flat/wide (a single long horizontal line): widen the
    // shorter half-extent so the aspect ratio stays within MAX_ASPECT. This only
    // ENLARGES the box, so the text still fits inside.
    const MAX_ASPECT: f32 = 1.8;
    let (hw, hh) = if hh > hw * MAX_ASPECT {
        (hh / MAX_ASPECT, hh)
    } else if hw > hh * MAX_ASPECT {
        (hw, hw / MAX_ASPECT)
    } else {
        (hw, hh)
    };
    match *shape {
        BubbleShape::Ellipse { .. } => BubbleShape::Ellipse {
            rx: hw * SQRT2,
            ry: hh * SQRT2,
        },
        BubbleShape::RoundRect { corner_px, .. } => BubbleShape::RoundRect {
            half_w: hw,
            half_h: hh,
            corner_px: corner_px.min(hw.min(hh)),
        },
        BubbleShape::Burst {
            spikes,
            jag,
            shape_seed,
            ..
        } => {
            // Text must fit inside the inner valleys. The renderer clamps jag to
            // 0.4..=0.75 and then subtracts up to 0.05 of jitter, so size against
            // that MINIMUM valley ratio (not the raw slider value, which can be
            // higher) or the text pokes through a deep valley.
            let j = (jag.clamp(0.4, 0.75) - 0.05).max(0.3);
            BubbleShape::Burst {
                rx: hw * SQRT2 / j,
                ry: hh * SQRT2 / j,
                spikes,
                jag,
                shape_seed,
            }
        }
        BubbleShape::Cloud {
            lobes,
            amp,
            shape_seed,
            ..
        } => {
            // Lobes bulge outward; the indented base radius is ≈ r * (1 - amp).
            let k = (1.0 - amp).clamp(0.4, 1.0);
            BubbleShape::Cloud {
                rx: hw * SQRT2 / k,
                ry: hh * SQRT2 / k,
                lobes,
                amp,
                shape_seed,
            }
        }
    }
}

/// Tiny deterministic hash → f32 in [0,1). Mixes `seed` with `index` so each
/// spike/lobe gets a stable pseudo-random value without pulling in `rand`.
fn hash01(seed: u32, index: u32) -> f32 {
    // SplitMix32-style avalanche.
    let mut x = seed
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(index.wrapping_mul(0x85EB_CA6B))
        .wrapping_add(0x2754_5A57);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    (x as f32) / (u32::MAX as f32)
}

/// Symmetric jitter in [-amp, +amp] from a deterministic hash.
fn jitter(seed: u32, index: u32, amp: f32) -> f32 {
    (hash01(seed, index) * 2.0 - 1.0) * amp
}

/// Spiky burst outline with the hand-drawn manga-explosion look (爆発フキダシ).
///
/// `spikes` sharp triangles around the perimeter; deep valleys at `jag` so the
/// spikes read as triangles, slightly irregular via deterministic per-spike
/// jitter from `shape_seed` (outer radius ±~12% + small angle wobble).
fn burst(
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    spikes: u32,
    jag: f32,
    shape_seed: u32,
) -> Vec<(f32, f32)> {
    let spikes = spikes.max(5);
    // Valleys sit clearly inside so spikes are sharp triangles.
    let jag = jag.clamp(0.4, 0.75);
    let n = spikes * 2;
    let mut pts = Vec::with_capacity(n as usize);
    let step = std::f32::consts::TAU / (spikes as f32);
    for s in 0..spikes {
        let base = (s as f32) * step;
        // Outer spike tip: jittered radius (±12%) + small angle wobble.
        let r_jit = 1.0 + jitter(shape_seed, s * 2 + 1, 0.12);
        let a_jit = jitter(shape_seed, s * 2 + 7, step * 0.18);
        let outer_a = base + a_jit;
        let outer_r = r_jit.max(0.6);
        pts.push((
            cx + rx * outer_r * outer_a.cos(),
            cy + ry * outer_r * outer_a.sin(),
        ));
        // Inner valley between this spike and the next (sharp triangle floor),
        // with a gentle jitter so the explosion isn't perfectly regular.
        let valley_a = base + step * 0.5;
        let valley_r = (jag + jitter(shape_seed, s * 2 + 13, 0.05)).clamp(0.3, 0.85);
        pts.push((
            cx + rx * valley_r * valley_a.cos(),
            cy + ry * valley_r * valley_a.sin(),
        ));
    }
    pts
}

/// Cloud / thought outline: gentle rounded bumps (`lobes` of them) of depth
/// `amp` around an ellipse, with a subtle per-lobe jitter from `shape_seed`.
fn cloud(
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    lobes: u32,
    amp: f32,
    shape_seed: u32,
) -> Vec<(f32, f32)> {
    let lobes = lobes.max(3);
    let amp = amp.clamp(0.0, 0.5);
    const SEG: usize = 160;
    let mut pts = Vec::with_capacity(SEG);
    for i in 0..SEG {
        let t = (i as f32) / (SEG as f32) * std::f32::consts::TAU;
        let bump = 0.5 + 0.5 * (lobes as f32 * t).cos(); // 0..1
        // Which lobe is this sample nearest to? Use it to vary lobe depth gently.
        let lobe_idx = (((lobes as f32 * t) / std::f32::consts::TAU).round() as i64)
            .rem_euclid(lobes as i64) as u32;
        let lobe_jit = jitter(shape_seed, lobe_idx + 1, 0.18); // ±18% of amp
        let r = (1.0 - amp) + amp * bump * (1.0 + lobe_jit);
        pts.push((cx + rx * r * t.cos(), cy + ry * r * t.sin()));
    }
    pts
}

fn round_rect(cx: f32, cy: f32, half_w: f32, half_h: f32, corner: f32) -> Vec<(f32, f32)> {
    let corner = corner.max(0.0).min(half_w.min(half_h));
    if corner <= 0.5 {
        // Plain rectangle (narration frame).
        return vec![
            (cx - half_w, cy - half_h),
            (cx + half_w, cy - half_h),
            (cx + half_w, cy + half_h),
            (cx - half_w, cy + half_h),
        ];
    }
    let left = cx - half_w;
    let right = cx + half_w;
    let top = cy - half_h;
    let bottom = cy + half_h;
    let mut pts = Vec::new();
    // Corner centers.
    let corners = [
        (right - corner, top + corner, -0.5 * std::f32::consts::PI), // top-right arc start
        (right - corner, bottom - corner, 0.0),                      // bottom-right
        (left + corner, bottom - corner, 0.5 * std::f32::consts::PI), // bottom-left
        (left + corner, top + corner, std::f32::consts::PI),         // top-left
    ];
    for &(ccx, ccy, start) in &corners {
        for i in 0..=CORNER_SEGMENTS {
            let t = start + (i as f32 / CORNER_SEGMENTS as f32) * (0.5 * std::f32::consts::PI);
            pts.push((ccx + corner * t.cos(), ccy + corner * t.sin()));
        }
    }
    pts
}

/// Build a straight tail triangle. `base_center` is a point on the bubble
/// outline (the rasterizer/lab computes it from `base_t`); `tip` is the tail
/// tip. The base is `width_px` wide, perpendicular to the base->tip direction.
pub fn tessellate_tail(base_center: (f32, f32), tip: (f32, f32), width_px: f32) -> Vec<(f32, f32)> {
    let (bx, by) = base_center;
    let (tx, ty) = tip;
    let dx = tx - bx;
    let dy = ty - by;
    let len = (dx * dx + dy * dy).sqrt().max(1e-3);
    // Perpendicular unit vector.
    let nx = -dy / len;
    let ny = dx / len;
    let hw = (width_px * 0.5).max(0.5);
    vec![
        (bx + nx * hw, by + ny * hw),
        (bx - nx * hw, by - ny * hw),
        (tx, ty),
    ]
}

/// Point on an axis-aligned ellipse outline at parameter t in 0..1.
pub fn ellipse_point(cx: f32, cy: f32, rx: f32, ry: f32, t: f32) -> (f32, f32) {
    let a = t * std::f32::consts::TAU;
    (cx + rx * a.cos(), cy + ry * a.sin())
}

/// Geometry for a bubble with an optional tail.
///
/// `body` / `tail` are convex pieces (fast convex fill for the live egui
/// preview); `outline` is the single seamless closed contour where the tail
/// spike is spliced into the body perimeter (used for the WYSIWYG raster fill +
/// stroke). Splicing the tail into one contour is what makes the つの share the
/// same outline as the bubble — no overlapping double stroke at the junction.
pub struct BubbleGeometry {
    /// Body-only outline (convex), for live convex fill.
    pub body: Vec<(f32, f32)>,
    /// Tail triangle `[base0, base1, tip]` (convex), for live convex fill.
    /// `None` when there is no spike tail (no tail, or a thought tail).
    pub tail: Option<[(f32, f32); 3]>,
    /// Unified closed contour (body perimeter with the spike tail spliced in;
    /// just the body for no-tail / thought-tail bubbles).
    pub outline: Vec<(f32, f32)>,
    /// Thought-tail circles `(cx, cy, r)` (shrinking toward the speaker). Empty
    /// unless the tail kind is `Thought`.
    pub thought: Vec<(f32, f32, f32)>,
}

/// The bubble's half-extents (rx/ry or half_w/half_h), regardless of variant.
fn shape_half_extents(shape: &BubbleShape) -> (f32, f32) {
    match *shape {
        BubbleShape::Ellipse { rx, ry } => (rx, ry),
        BubbleShape::RoundRect { half_w, half_h, .. } => (half_w, half_h),
        BubbleShape::Burst { rx, ry, .. } => (rx, ry),
        BubbleShape::Cloud { rx, ry, .. } => (rx, ry),
    }
}

/// Cap the requested tail base width to the bubble's half-extent perpendicular
/// to the tail direction. A bottom tail on a tall/narrow bubble (perp = small
/// width) gets a slim base; a side tail (perp = tall height) or a wide bubble
/// allows a wider base. Keeps a small minimum so a tail never vanishes.
fn effective_tail_base_width(
    shape: &BubbleShape,
    pivot: (f32, f32),
    tip: (f32, f32),
    requested: f32,
) -> f32 {
    let (hx, hy) = shape_half_extents(shape);
    let (dx, dy) = (tip.0 - pivot.0, tip.1 - pivot.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 {
        return requested.max(6.0);
    }
    // Unit vector perpendicular to the center→tip direction.
    let (px, py) = (-dy / len, dx / len);
    // Ellipse-style support half-extent along that perpendicular.
    let perp_half = ((hx * px).powi(2) + (hy * py).powi(2)).sqrt();
    let cap = (perp_half * 0.85).max(6.0);
    requested.clamp(6.0, cap)
}

/// Build body + tail geometry. When a tail is present, `outline` is one closed
/// polygon in which the tail is an outward spike that is part of the same
/// contour as the bubble body.
pub fn bubble_geometry(
    shape: &BubbleShape,
    pivot: (f32, f32),
    tail: Option<&Tail>,
) -> BubbleGeometry {
    let body = tessellate_bubble(shape, pivot);
    match tail {
        Some(t) if t.width_px > 0.0 => {
            // Auto base: attach where the center→tip ray exits the outline.
            let base_t = if t.base_auto {
                auto_base_t(&body, pivot, t.tip)
            } else {
                t.base_t
            };
            // Cap the base width to the bubble's extent perpendicular to the tail
            // so a bottom tail on a tall/narrow bubble stays slim, while a side
            // tail (or a wide bubble) can be wider — proportionate, not fixed-px.
            let eff_w = effective_tail_base_width(shape, pivot, t.tip, t.width_px);
            match t.kind {
                TailKind::Spike => {
                    let (outline, b0, b1) = splice_tail(&body, base_t, t.tip, eff_w);
                    BubbleGeometry {
                        body,
                        tail: Some([b0, b1, t.tip]),
                        outline,
                        thought: Vec::new(),
                    }
                }
                TailKind::Thought => {
                    let thought = thought_trail(&body, base_t, t.tip, eff_w);
                    let outline = body.clone();
                    BubbleGeometry {
                        body,
                        tail: None,
                        outline,
                        thought,
                    }
                }
            }
        }
        _ => {
            let outline = body.clone();
            BubbleGeometry {
                body,
                tail: None,
                outline,
                thought: Vec::new(),
            }
        }
    }
}

/// A point on a closed polygon at arc-length fraction `t` (0..1).
fn arclen_point(body: &[(f32, f32)], t: f32) -> (f32, f32) {
    let n = body.len();
    if n < 2 {
        return body.first().copied().unwrap_or((0.0, 0.0));
    }
    let mut cum = vec![0.0f32; n + 1];
    for i in 0..n {
        let a = body[i];
        let b = body[(i + 1) % n];
        cum[i + 1] = cum[i] + ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    }
    let total = cum[n];
    if total <= 1e-3 {
        return body[0];
    }
    let mut s = (t % 1.0) * total;
    if s < 0.0 {
        s += total;
    }
    for i in 0..n {
        if s <= cum[i + 1] {
            let seg = (cum[i + 1] - cum[i]).max(1e-6);
            let f = ((s - cum[i]) / seg).clamp(0.0, 1.0);
            let a = body[i];
            let b = body[(i + 1) % n];
            return (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
        }
    }
    body[0]
}

/// Arc-length fraction (0..1) on the closed `body` polygon where the ray from
/// `center` toward `tip` exits the outline. Returns the **outermost** forward
/// crossing so spiky/cloud silhouettes attach the tail at the rim, not at an
/// inner valley. Falls back to 0.0 for degenerate input.
///
/// This is the "auto" tail base: the tail naturally points from the bubble
/// toward the speaker.
pub fn auto_base_t(body: &[(f32, f32)], center: (f32, f32), tip: (f32, f32)) -> f32 {
    let n = body.len();
    if n < 2 {
        return 0.0;
    }
    let dx = tip.0 - center.0;
    let dy = tip.1 - center.1;
    if (dx * dx + dy * dy) < 1e-6 {
        return 0.0;
    }
    let mut cum = vec![0.0f32; n + 1];
    for i in 0..n {
        let a = body[i];
        let b = body[(i + 1) % n];
        cum[i + 1] = cum[i] + ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    }
    let total = cum[n];
    if total <= 1e-3 {
        return 0.0;
    }
    // Ray: center + t*D (t>0); Segment: A + u*E (u in [0,1]).
    let mut best_t = -1.0f32;
    let mut best_s = 0.0f32;
    for i in 0..n {
        let a = body[i];
        let b = body[(i + 1) % n];
        let ex = b.0 - a.0;
        let ey = b.1 - a.1;
        let det = ex * dy - dx * ey;
        if det.abs() < 1e-9 {
            continue;
        }
        let wx = a.0 - center.0;
        let wy = a.1 - center.1;
        let t = (ex * wy - ey * wx) / det; // along ray
        let u = (dx * wy - dy * wx) / det; // along edge
        if t > 0.0 && (0.0..=1.0).contains(&u) && t > best_t {
            best_t = t;
            best_s = cum[i] + u * (cum[i + 1] - cum[i]);
        }
    }
    if best_t < 0.0 {
        return 0.0;
    }
    (best_s / total).clamp(0.0, 1.0)
}

/// The image-space point where the tail base attaches, resolving `base_auto`.
/// Used by the lab to draw / hit-test the tail-base handle so it matches the
/// baked geometry.
pub fn resolve_tail_base(shape: &BubbleShape, pivot: (f32, f32), tail: &Tail) -> (f32, f32) {
    let body = tessellate_bubble(shape, pivot);
    let base_t = if tail.base_auto {
        auto_base_t(&body, pivot, tail.tip)
    } else {
        tail.base_t
    };
    arclen_point(&body, base_t)
}

/// Arc-length fraction (0..1) of the point on the bubble outline closest to
/// `point` (in image space). Used to convert a tail-base handle drag into a
/// `base_t`. Walks each edge and projects `point` onto it.
pub fn nearest_base_t(shape: &BubbleShape, pivot: (f32, f32), point: (f32, f32)) -> f32 {
    let body = tessellate_bubble(shape, pivot);
    let n = body.len();
    if n < 2 {
        return 0.0;
    }
    let mut cum = vec![0.0f32; n + 1];
    for i in 0..n {
        let a = body[i];
        let b = body[(i + 1) % n];
        cum[i + 1] = cum[i] + ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    }
    let total = cum[n];
    if total <= 1e-3 {
        return 0.0;
    }
    let mut best_d = f32::INFINITY;
    let mut best_s = 0.0f32;
    for i in 0..n {
        let a = body[i];
        let b = body[(i + 1) % n];
        let ex = b.0 - a.0;
        let ey = b.1 - a.1;
        let seg2 = ex * ex + ey * ey;
        let u = if seg2 < 1e-9 {
            0.0
        } else {
            (((point.0 - a.0) * ex + (point.1 - a.1) * ey) / seg2).clamp(0.0, 1.0)
        };
        let px = a.0 + ex * u;
        let py = a.1 + ey * u;
        let d = (point.0 - px).powi(2) + (point.1 - py).powi(2);
        if d < best_d {
            best_d = d;
            best_s = cum[i] + u * (cum[i + 1] - cum[i]);
        }
    }
    (best_s / total).clamp(0.0, 1.0)
}

/// Thought-tail circles: shrink from near the bubble outline toward the tip.
fn thought_trail(
    body: &[(f32, f32)],
    base_t: f32,
    tip: (f32, f32),
    width_px: f32,
) -> Vec<(f32, f32, f32)> {
    let start = arclen_point(body, base_t);
    const N: usize = 4;
    let mut out = Vec::with_capacity(N);
    for i in 0..N {
        let s = i as f32 / (N as f32 - 1.0); // 0..1
        let f = 0.18 + s * 0.74; // position along base->tip
        let cx = start.0 + (tip.0 - start.0) * f;
        let cy = start.1 + (tip.1 - start.1) * f;
        let r = (width_px * 0.5) * (0.95 - s * 0.72);
        out.push((cx, cy, r.max(3.0)));
    }
    out
}

/// Splice a tail spike into a closed body polygon. Returns the unified contour
/// plus the two base points. Arc-length parameterized: `base_t` (0..1) is a
/// uniform position along the perimeter; `width_px` is the base width measured
/// along the outline. The short arc within `width_px/2` of the base center is
/// removed and replaced by `base0 -> tip -> base1`, yielding one closed loop.
fn splice_tail(
    body: &[(f32, f32)],
    base_t: f32,
    tip: (f32, f32),
    width_px: f32,
) -> (Vec<(f32, f32)>, (f32, f32), (f32, f32)) {
    let n = body.len();
    if n < 3 {
        return (body.to_vec(), tip, tip);
    }
    let mut cum = vec![0.0f32; n + 1];
    for i in 0..n {
        let a = body[i];
        let b = body[(i + 1) % n];
        cum[i + 1] = cum[i] + ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    }
    let total = cum[n];
    if total <= 1e-3 {
        return (body.to_vec(), tip, tip);
    }
    let wrap = |s: f32| {
        let mut x = s % total;
        if x < 0.0 {
            x += total;
        }
        x
    };
    let point_at = |s: f32| -> (f32, f32) {
        let s = wrap(s);
        for i in 0..n {
            if s <= cum[i + 1] {
                let seg = (cum[i + 1] - cum[i]).max(1e-6);
                let t = ((s - cum[i]) / seg).clamp(0.0, 1.0);
                let a = body[i];
                let b = body[(i + 1) % n];
                return (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            }
        }
        body[0]
    };
    let sc = wrap(base_t * total);
    // Guard the clamp: for tiny perimeters `total*0.45` can be < 0.5, and
    // `f32::clamp` panics when min > max. Keep the upper bound >= the lower.
    let max_half = (total * 0.45).max(0.5);
    let half = (width_px * 0.5).clamp(0.5, max_half);
    let b0 = point_at(sc - half);
    let b1 = point_at(sc + half);
    // A vertex is dropped if within `half` arc-length of sc (circular distance).
    let in_window = |sv: f32| -> bool {
        let mut d = (sv - sc).abs() % total;
        if d > total * 0.5 {
            d = total - d;
        }
        d < half
    };
    // Trace the long way: start at b1 (= sc+half), walk increasing arc-length
    // skipping windowed vertices, end at b0, then the tip.
    let s1 = wrap(sc + half);
    let mut idxs: Vec<usize> = (0..n).collect();
    idxs.sort_by(|&a, &b| {
        wrap(cum[a] - s1)
            .partial_cmp(&wrap(cum[b] - s1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut outline = Vec::with_capacity(n + 3);
    outline.push(b1);
    for &i in &idxs {
        if !in_window(cum[i]) {
            outline.push(body[i]);
        }
    }
    outline.push(b0);
    outline.push(tip);
    (outline, b0, b1)
}

/// One placed decoration instance (resolved position / size / rotation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedDeco {
    pub kind: DecoKind,
    /// Center in image space.
    pub cx: f32,
    pub cy: f32,
    /// Radius (half extent) in pixels.
    pub size: f32,
    /// Rotation in radians.
    pub rot: f32,
    pub color: crate::model::Rgba,
}

/// Total arc length of a closed polygon.
fn poly_arclen(poly: &[(f32, f32)]) -> f32 {
    let n = poly.len();
    if n < 2 {
        return 0.0;
    }
    let mut total = 0.0;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        total += ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    }
    total
}

/// Point + outward unit normal on a closed polygon at arc-length fraction `t`.
fn point_and_normal(poly: &[(f32, f32)], center: (f32, f32), t: f32) -> ((f32, f32), (f32, f32)) {
    let p = arclen_point(poly, t);
    // Outward normal approximated as the direction from the polygon centroid /
    // center toward the point (good enough for convex-ish bubble bodies).
    let dx = p.0 - center.0;
    let dy = p.1 - center.1;
    let len = (dx * dx + dy * dy).sqrt().max(1e-3);
    (p, (dx / len, dy / len))
}

/// Resolve a decoration layer into concrete placed instances along the bubble
/// `body` outline. `short_side` is min(rx-ish, ry-ish) used for sizing; `center`
/// is the bubble pivot; `tail_base` is the outline point where the tail leaves
/// (for `Tail` placement). Deterministic jitter from `layer.seed + index`.
pub fn place_decorations(
    layer: &DecorationLayer,
    body: &[(f32, f32)],
    center: (f32, f32),
    short_side: f32,
    tail_base: Option<(f32, f32)>,
) -> Vec<PlacedDeco> {
    if body.len() < 3 {
        return Vec::new();
    }
    let arclen = poly_arclen(body);
    if arclen <= 1.0 {
        return Vec::new();
    }
    // Items per ~100px of outline.
    let count = ((arclen / 100.0) * layer.density.max(0.0)).round() as i32;
    let count = count.clamp(0, 400);
    if count == 0 {
        return Vec::new();
    }
    let base_size = (short_side * layer.size_ratio.max(0.01)).max(2.0);
    let offset = base_size * 0.9; // outward/inward step for Outside/Inside

    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let idx = i as u32;
        // Even spacing along the outline + small positional jitter.
        let t =
            (i as f32 + 0.5) / count as f32 + jitter(layer.seed, idx * 3 + 1, 0.5 / count as f32);
        let (p, normal) = point_and_normal(body, center, t.rem_euclid(1.0));
        let (px, py) = match layer.placement {
            DecoPlacement::Outline => p,
            DecoPlacement::Outside => (p.0 + normal.0 * offset, p.1 + normal.1 * offset),
            DecoPlacement::Inside => (p.0 - normal.0 * offset, p.1 - normal.1 * offset),
            DecoPlacement::Tail => {
                // Cluster near the tail base (fallback to the outline point).
                let base = tail_base.unwrap_or(p);
                let spread = base_size * 1.5;
                (
                    base.0 + jitter(layer.seed, idx * 3 + 1, spread),
                    base.1 + jitter(layer.seed, idx * 3 + 2, spread),
                )
            }
        };
        let size = base_size * (1.0 + jitter(layer.seed, idx * 3 + 17, 0.35));
        let rot = jitter(layer.seed, idx * 3 + 23, std::f32::consts::PI);
        out.push(PlacedDeco {
            kind: layer.kind,
            cx: px,
            cy: py,
            size: size.max(1.5),
            rot,
            color: layer.color,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ellipse_has_points() {
        let pts = tessellate_bubble(&BubbleShape::Ellipse { rx: 10.0, ry: 5.0 }, (0.0, 0.0));
        assert_eq!(pts.len(), ELLIPSE_SEGMENTS);
    }

    #[test]
    fn plain_rect_has_four_corners() {
        let pts = tessellate_bubble(
            &BubbleShape::RoundRect {
                half_w: 10.0,
                half_h: 5.0,
                corner_px: 0.0,
            },
            (0.0, 0.0),
        );
        assert_eq!(pts.len(), 4);
    }

    #[test]
    fn tail_triangle_has_three_points() {
        let t = tessellate_tail((0.0, 0.0), (10.0, 10.0), 4.0);
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn fit_preserves_variant_and_contains_text() {
        // Ellipse: the inscribed text rect + padding must fit, i.e.
        // (w/rx)² + (h/ry)² <= 1 for the half-extents incl. padding. The box is
        // within MAX_ASPECT so the aspect clamp doesn't widen it (exact extents).
        let pad = 10.0;
        let (tw, th) = (200.0, 130.0);
        let fitted = fit_bubble_shape(&BubbleShape::Ellipse { rx: 1.0, ry: 1.0 }, tw, th, pad);
        let BubbleShape::Ellipse { rx, ry } = fitted else {
            panic!("ellipse stays ellipse");
        };
        let hw = tw * 0.5 + pad;
        let hh = th * 0.5 + pad;
        let cover = (hw / rx).powi(2) + (hh / ry).powi(2);
        assert!(
            cover <= 1.0 + 1e-3,
            "text rect corner inside ellipse: {cover}"
        );

        // RoundRect: half extents are exactly text/2 + padding; corner preserved.
        let fitted = fit_bubble_shape(
            &BubbleShape::RoundRect {
                half_w: 1.0,
                half_h: 1.0,
                corner_px: 12.0,
            },
            tw,
            th,
            pad,
        );
        let BubbleShape::RoundRect {
            half_w,
            half_h,
            corner_px,
        } = fitted
        else {
            panic!("roundrect stays roundrect");
        };
        assert!((half_w - hw).abs() < 1e-3 && (half_h - hh).abs() < 1e-3);
        assert!(corner_px <= half_w.min(half_h));

        // Burst keeps spikes/jag/seed; Cloud keeps lobes/amp/seed.
        let fitted = fit_bubble_shape(
            &BubbleShape::Burst {
                rx: 1.0,
                ry: 1.0,
                spikes: 11,
                jag: 0.6,
                shape_seed: 42,
            },
            tw,
            th,
            pad,
        );
        let BubbleShape::Burst {
            spikes,
            jag,
            shape_seed,
            ..
        } = fitted
        else {
            panic!("burst stays burst");
        };
        assert_eq!((spikes, jag, shape_seed), (11, 0.6, 42));
    }

    #[test]
    fn fit_clamps_extreme_aspect() {
        // A single tall/narrow vertical line (e.g. 40×400) would otherwise make a
        // very tall, thin ellipse. The aspect clamp widens it so height/width
        // stays within ~1.8, while still containing the text.
        let pad = 12.0;
        let fitted = fit_bubble_shape(&BubbleShape::Ellipse { rx: 1.0, ry: 1.0 }, 40.0, 400.0, pad);
        let BubbleShape::Ellipse { rx, ry } = fitted else {
            panic!("ellipse stays ellipse");
        };
        assert!(ry >= rx, "tall text still taller than wide");
        assert!(
            ry / rx <= 1.8 + 1e-3,
            "aspect clamped within limit, got {}",
            ry / rx
        );
        // Text must still fit inside the (widened) ellipse.
        let (hw, hh) = (40.0 * 0.5 + pad, 400.0 * 0.5 + pad);
        let cover = (hw / rx).powi(2) + (hh / ry).powi(2);
        assert!(cover <= 1.0 + 1e-3, "text still inside: {cover}");
    }

    #[test]
    fn tail_base_width_capped_by_perp_extent() {
        // Tall narrow bubble (rx=40, ry=160). A bottom tail's base is capped to
        // ~rx (perp = width); a side tail's base can be much wider (perp = height).
        let shape = BubbleShape::Ellipse {
            rx: 40.0,
            ry: 160.0,
        };
        let pivot = (0.0, 0.0);
        let bottom = effective_tail_base_width(&shape, pivot, (0.0, 300.0), 200.0);
        let side = effective_tail_base_width(&shape, pivot, (300.0, 0.0), 200.0);
        assert!(
            bottom <= 40.0 * 0.85 + 1e-3,
            "bottom tail base capped to ~rx, got {bottom}"
        );
        assert!(
            side > bottom * 2.0,
            "side tail base much wider ({side}) than bottom ({bottom})"
        );
    }

    #[test]
    fn burst_and_cloud_have_points() {
        let b = tessellate_bubble(
            &BubbleShape::Burst {
                rx: 10.0,
                ry: 10.0,
                spikes: 8,
                jag: 0.5,
                shape_seed: 0,
            },
            (0.0, 0.0),
        );
        // spikes*2 vertices (outer tip + inner valley per spike).
        assert_eq!(b.len(), 16);
        let c = tessellate_bubble(
            &BubbleShape::Cloud {
                rx: 10.0,
                ry: 10.0,
                lobes: 8,
                amp: 0.12,
                shape_seed: 0,
            },
            (0.0, 0.0),
        );
        assert!(c.len() >= 16);
    }

    #[test]
    fn burst_seed_changes_outline_deterministically() {
        let mk = |seed: u32| {
            tessellate_bubble(
                &BubbleShape::Burst {
                    rx: 100.0,
                    ry: 100.0,
                    spikes: 20,
                    jag: 0.55,
                    shape_seed: seed,
                },
                (0.0, 0.0),
            )
        };
        let a0 = mk(0);
        let a0_again = mk(0);
        let a1 = mk(1);
        // Same seed -> identical (deterministic).
        assert_eq!(a0, a0_again);
        // Different seed -> at least one vertex moves.
        assert!(a0.iter().zip(a1.iter()).any(|(p, q)| p != q));
    }

    #[test]
    fn tail_splice_makes_unified_outline() {
        let shape = BubbleShape::Ellipse {
            rx: 100.0,
            ry: 60.0,
        };
        let body = tessellate_bubble(&shape, (0.0, 0.0));
        let tail = Tail {
            tip: (0.0, 200.0),
            base_t: 0.25,
            base_auto: false,
            width_px: 40.0,
            kind: TailKind::Spike,
        };
        let geo = bubble_geometry(&shape, (0.0, 0.0), Some(&tail));
        assert!(geo.tail.is_some());
        // The unified contour must include the tail tip vertex.
        assert!(
            geo.outline
                .iter()
                .any(|&p| p.0.abs() < 1e-3 && (p.1 - 200.0).abs() < 1e-3),
            "unified outline should include the tail tip"
        );
        // No tail -> outline is just the body.
        let geo2 = bubble_geometry(&shape, (0.0, 0.0), None);
        assert!(geo2.tail.is_none());
        assert_eq!(geo2.outline.len(), body.len());
    }

    #[test]
    fn auto_base_t_points_toward_tip() {
        // Ellipse rx=100, ry=60 at origin. The axis crossings sit at arc-length
        // fractions 0 (right), 0.25 (bottom), 0.5 (left), 0.75 (top).
        let shape = BubbleShape::Ellipse {
            rx: 100.0,
            ry: 60.0,
        };
        let body = tessellate_bubble(&shape, (0.0, 0.0));
        // Tip below -> base near the bottom crossing (~0.25).
        let t_down = auto_base_t(&body, (0.0, 0.0), (0.0, 200.0));
        assert!(
            (t_down - 0.25).abs() < 0.03,
            "down tip -> ~0.25, got {t_down}"
        );
        // Tip above -> base near the top crossing (~0.75).
        let t_up = auto_base_t(&body, (0.0, 0.0), (0.0, -200.0));
        assert!((t_up - 0.75).abs() < 0.03, "up tip -> ~0.75, got {t_up}");

        // resolve_tail_base (auto) lands on the bottom of the ellipse.
        let tail = Tail {
            tip: (0.0, 200.0),
            base_t: 0.5,
            base_auto: true,
            width_px: 40.0,
            kind: TailKind::Spike,
        };
        let base = resolve_tail_base(&shape, (0.0, 0.0), &tail);
        assert!(
            base.0.abs() < 4.0 && (base.1 - 60.0).abs() < 4.0,
            "base near (0,60): {base:?}"
        );
    }

    #[test]
    fn nearest_base_t_snaps_to_outline() {
        let shape = BubbleShape::Ellipse {
            rx: 100.0,
            ry: 60.0,
        };
        // A point just outside the bottom maps to the bottom crossing (~0.25).
        let t = nearest_base_t(&shape, (0.0, 0.0), (0.0, 90.0));
        assert!((t - 0.25).abs() < 0.03, "bottom point -> ~0.25, got {t}");
    }
}
